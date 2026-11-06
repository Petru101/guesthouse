// SPDX-License-Identifier: GPL-2.0-or-later
/* The one device, its one virtual monitor, and what nvidia-drm reads about them. */
#include "standin.h"

const struct NvKmsKapiCallbacks *callbacks;

bool log_flips;
module_param(log_flips, bool, 0644);
MODULE_PARM_DESC(log_flips, "Log when each flip was committed and shown");

bool follow_viewer = true;
module_param(follow_viewer, bool, 0644);
MODULE_PARM_DESC(follow_viewer, "Complete flips as the viewer takes them, rather than on a grid");

static u32 attached_gpu(struct NvKmsKapiDevice *d)
{
	static NV0000_CTRL_GPU_GET_ATTACHED_IDS_PARAMS ids;

	if (rm_control(d, d->client, NV0000_CTRL_CMD_GPU_GET_ATTACHED_IDS, &ids, sizeof(ids), -1, 0))
		return 0;
	return ids.gpuIds[0];
}

static int device_open(struct NvKmsKapiDevice *d)
{
	struct {
		NvHandle hClient;
		NvU32 processID;
		char name[100];
		u32 pad;
		u64 pid_info;
	} root = { 0 };
	u8 device[56] = { 0 }; /* NV0080_ALLOC_PARAMETERS, deviceId 0 */
	u32 subdevice = 0; /* NV2080_ALLOC_PARAMETERS */
	struct { int ctl_fd; } reg = { 0 };
	long rc, h;
	u32 handle = 0;

	h = nvfwd_kopen(NVFWD_DEV_CTL);
	if (h < 0)
		return h;
	d->ctl = h;
	h = nvfwd_kopen(NVFWD_DEV_GPU);
	if (h < 0)
		return h;
	d->gpu = h;
	{
		struct nvfwd_krm a = { d->gpu, CTL_ESC(ESC_REGISTER_FD, sizeof(reg)), &reg, sizeof(reg), 0, NULL, 0, 0, -1, d->ctl };

		rc = nvfwd_rm(&a);
		if (rc)
			return rc;
	}
	strscpy(root.name, "nvidia-modeset stand-in", sizeof(root.name));
	d->client = 0;
	rc = rm_alloc(d, 0, &handle, 0x41, &root, sizeof(root));
	if (rc)
		return rc;
	d->client = handle;
	d->gpu_id = attached_gpu(d);
	handle = HANDLE_DEVICE;
	rc = rm_alloc(d, d->client, &handle, 0x80, device, sizeof(device));
	if (rc)
		return rc;
	handle = HANDLE_SUBDEVICE;
	rc = rm_alloc(d, HANDLE_DEVICE, &handle, 0x2080, &subdevice, sizeof(subdevice));
	if (rc)
		return rc;
	pr_info("nvidia-modeset stand-in: client %x, gpu id %x\n", d->client, d->gpu_id);
	return 0;
}

NvU32 enumerateGpus(void (*found)(const struct NvKmsKapiGpuInfo *info))
{
	struct NvKmsKapiDevice probe = { 0 };
	struct NvKmsKapiGpuInfo info = { 0 };
	long rc = device_open(&probe);

	if (rc) {
		pr_err("nvidia-modeset stand-in: no device: %ld\n", rc);
		return 0;
	}
	info.gpuInfo.gpu_id = probe.gpu_id;
	info.gpuInfo.os_device_ptr = nvfwd_device();
	nvfwd_kclose(probe.gpu);
	nvfwd_kclose(probe.ctl);
	found(&info);
	return 1;
}

struct NvKmsKapiDevice *allocateDevice(const struct NvKmsKapiAllocateDeviceParams *params)
{
	struct NvKmsKapiDevice *d = kzalloc(sizeof(*d), GFP_KERNEL);
	long rc;

	if (!d)
		return NULL;
	rc = device_open(d);
	if (rc) {
		pr_err("nvidia-modeset stand-in: device: %ld\n", rc);
		kfree(d);
		return NULL;
	}
	d->private_data = params->privateData;
	d->event = params->eventCallback;
	d->next_handle = HANDLE_MEMORY;
	d->period_ns = 1000000000 / 60;
	INIT_DELAYED_WORK(&d->rate_work, rate_check);
	INIT_WORK(&d->flip_work, flip_done);
	INIT_WORK(&d->held_work, held_done);
	spin_lock_init(&d->held_lock);
	hrtimer_setup(&d->flip_timer, flip_timer_done, CLOCK_MONOTONIC, HRTIMER_MODE_ABS);
	schedule_delayed_work(&d->rate_work, HZ);
	return d;
}

void freeDevice(struct NvKmsKapiDevice *d)
{
	cancel_delayed_work_sync(&d->rate_work);
	hrtimer_cancel(&d->flip_timer);
	cancel_work_sync(&d->flip_work);
	cancel_work_sync(&d->held_work);
	nvfwd_kclose(d->gpu);
	nvfwd_kclose(d->ctl);
	kfree(d);
}

NvBool yes_device(struct NvKmsKapiDevice *d) { return NV_TRUE; }
void nothing_device(struct NvKmsKapiDevice *d) { }
NvBool declareEventInterest(const struct NvKmsKapiDevice *d, const NvU32 mask) { return NV_TRUE; }
NvBool yes_const_device(const struct NvKmsKapiDevice *d) { return NV_TRUE; }
NvBool grantPermissions(NvS32 fd, struct NvKmsKapiDevice *d, NvU32 head, NvKmsKapiDisplay display) { return NV_FALSE; }
NvBool revokePermissions(struct NvKmsKapiDevice *d, NvU32 head, NvKmsKapiDisplay display) { return NV_FALSE; }
NvBool grantSubOwnership(NvS32 fd, struct NvKmsKapiDevice *d) { return NV_FALSE; }
NvBool revokeSubOwnership(struct NvKmsKapiDevice *d) { return NV_FALSE; }

NvBool getDeviceResourcesInfo(struct NvKmsKapiDevice *d, struct NvKmsKapiDeviceResourcesInfo *info)
{
	u64 formats = 1ull << NvKmsSurfaceMemoryFormatA8R8G8B8 | 1ull << NvKmsSurfaceMemoryFormatX8R8G8B8 |
		      1ull << NvKmsSurfaceMemoryFormatA8B8G8R8 | 1ull << NvKmsSurfaceMemoryFormatX8B8G8R8;

	NV2080_CTRL_FB_GET_SEMAPHORE_SURFACE_LAYOUT_PARAMS layout = { 0 };

	memset(info, 0, sizeof(*info));
	if (!rm_control(d, HANDLE_SUBDEVICE, NV2080_CTRL_CMD_FB_GET_SEMAPHORE_SURFACE_LAYOUT, &layout, sizeof(layout), -1, 0)) {
		d->semsurf_stride = info->caps.semsurf.stride = layout.size;
		d->semsurf_max_offset = info->caps.semsurf.maxSubmittedOffset = layout.maxSubmittedSemaphoreValueOffset;
	}
	info->caps.numDisplaySemaphores = DISPLAY_SEMAPHORES;
	info->numHeads = 1;
	info->numLayers[0] = 1;
	info->numConnectors = 1;
	info->connectorHandles[0] = CONNECTOR_HANDLE;
	info->caps.validCursorCompositionModes = 1 << NVKMS_COMPOSITION_BLENDING_MODE_OPAQUE | 1 << NVKMS_COMPOSITION_BLENDING_MODE_PREMULT_ALPHA;
	info->caps.supportedCursorSurfaceMemoryFormats = 1ull << NvKmsSurfaceMemoryFormatA8R8G8B8;
	info->caps.layer[0].validCompositionModes = 1 << NVKMS_COMPOSITION_BLENDING_MODE_OPAQUE;
	info->caps.minWidthInPixels = 1;
	info->caps.maxWidthInPixels = 16384;
	info->caps.minHeightInPixels = 1;
	info->caps.maxHeightInPixels = 16384;
	info->caps.maxCursorSizeInPixels = 256;
	info->caps.pitchAlignment = 256;
	info->caps.hasVideoMemory = 1;
	info->caps.genericPageKind = 0x06;
	info->supportedSurfaceMemoryFormats[0] = formats;
	return NV_TRUE;
}

NvBool getDisplays(struct NvKmsKapiDevice *d, NvU32 *count, NvKmsKapiDisplay *handles)
{
	if (handles && *count >= 1)
		handles[0] = DISPLAY_HANDLE;
	*count = 1;
	return NV_TRUE;
}

NvBool getConnectorInfo(struct NvKmsKapiDevice *d, NvKmsKapiConnector connector, struct NvKmsKapiConnectorInfo *info)
{
	memset(info, 0, sizeof(*info));
	info->handle = connector;
	info->signalFormat = NVKMS_CONNECTOR_SIGNAL_FORMAT_TMDS;
	info->type = NVKMS_CONNECTOR_TYPE_HDMI;
	return NV_TRUE;
}

NvBool getStaticDisplayInfo(struct NvKmsKapiDevice *d, NvKmsKapiDisplay display, struct NvKmsKapiStaticDisplayInfo *info)
{
	memset(info, 0, sizeof(*info));
	info->handle = display;
	info->connectorHandle = CONNECTOR_HANDLE;
	info->headMask = 1;
	return NV_TRUE;
}

NvBool getDynamicDisplayInfo(struct NvKmsKapiDevice *d, struct NvKmsKapiDynamicDisplayParams *params)
{
	params->connected = 1;
	params->maxWidthInPixels = 16384;
	params->maxHeightInPixels = 16384;
	return NV_TRUE;
}

int getDisplayMode(struct NvKmsKapiDevice *d, NvKmsKapiDisplay display, NvU32 index, struct NvKmsKapiDisplayMode *mode,
			  NvBool *valid, NvBool *preferred)
{
	if (index != 0)
		return 0;
	memset(mode, 0, sizeof(*mode));
	mode->timings.refreshRate = div64_u64(1000000000000000ull, d->period_ns) / 1000;
	mode->timings.pixelClockHz = div64_u64(2200ull * 1125 * 1000000000, d->period_ns);
	mode->timings.hVisible = WIDTH;
	mode->timings.hSyncStart = 2008;
	mode->timings.hSyncEnd = 2052;
	mode->timings.hTotal = 2200;
	mode->timings.vVisible = HEIGHT;
	mode->timings.vSyncStart = 1084;
	mode->timings.vSyncEnd = 1089;
	mode->timings.vTotal = 1125;
	mode->timings.flags.hSyncPos = 1;
	mode->timings.flags.vSyncPos = 1;
	strscpy(mode->name, "1920x1080", sizeof(mode->name));
	*valid = NV_TRUE;
	*preferred = NV_TRUE;
	return 1;
}

NvBool validateDisplayMode(struct NvKmsKapiDevice *d, NvKmsKapiDisplay display, const struct NvKmsKapiDisplayMode *mode)
{
	return mode->timings.hVisible == WIDTH && mode->timings.vVisible == HEIGHT;
}
