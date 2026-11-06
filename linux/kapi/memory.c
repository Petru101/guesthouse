// SPDX-License-Identifier: GPL-2.0-or-later
/* Memory: what nvidia-drm asks for, through the stand-in's RM client. */
#include "standin.h"

static struct NvKmsKapiMemory *memory_new(struct NvKmsKapiDevice *d, u64 size)
{
	struct NvKmsKapiMemory *m = kzalloc(sizeof(*m), GFP_KERNEL);

	if (m) {
		refcount_set(&m->refs, 1);
		m->size = size;
		m->handle = __sync_fetch_and_add(&d->next_handle, 1);
	}
	return m;
}

struct NvKmsKapiMemory *importMemory(struct NvKmsKapiDevice *d, NvU64 size, NvU64 user, NvU64 user_size)
{
	struct NvKmsKapiPrivImportMemoryParams p;
	NV0000_CTRL_OS_UNIX_IMPORT_OBJECT_FROM_FD_PARAMS import = { 0 };
	struct NvKmsKapiMemory *m;
	struct file *f;
	u32 id;
	long rc;

	if (user_size != sizeof(p) || copy_from_user(&p, u64_to_user_ptr(user), sizeof(p)))
		return NULL;
	f = fget(p.memFd);
	id = nvfwd_file_id(f);
	if (!id) {
		if (f)
			fput(f);
		pr_warn("nvidia-modeset stand-in: import from a file that is not ours\n");
		return NULL;
	}
	m = memory_new(d, size);
	if (!m) {
		fput(f);
		return NULL;
	}
	import.object.type = NV0000_CTRL_OS_UNIX_EXPORT_OBJECT_TYPE_RM;
	import.object.data.rmObject.hDevice = HANDLE_DEVICE;
	import.object.data.rmObject.hParent = HANDLE_DEVICE;
	import.object.data.rmObject.hObject = m->handle;
	rc = rm_control(d, d->client, NV0000_CTRL_CMD_OS_UNIX_IMPORT_OBJECT_FROM_FD, &import, sizeof(import),
			offsetof(NV0000_CTRL_OS_UNIX_IMPORT_OBJECT_FROM_FD_PARAMS, fd), id);
	if (rc) {
		pr_warn("nvidia-modeset stand-in: import: %lx\n", rc);
		fput(f);
		kfree(m);
		return NULL;
	}
	m->import = f;
	m->export_id = id;
	m->layout = p.surfaceParams;
	return m;
}

/* A host file holding this object, for the show path. */
long memory_export_file(struct NvKmsKapiDevice *d, struct NvKmsKapiMemory *m)
{
	NV0000_CTRL_OS_UNIX_EXPORT_OBJECT_TO_FD_PARAMS p = { 0 };
	long id, rc;

	if (m->export_id)
		return 0;
	id = nvfwd_kopen(NVFWD_DEV_CTL);
	if (id < 0)
		return id;
	p.object.type = NV0000_CTRL_OS_UNIX_EXPORT_OBJECT_TYPE_RM;
	p.object.data.rmObject.hDevice = HANDLE_DEVICE;
	p.object.data.rmObject.hParent = HANDLE_DEVICE;
	p.object.data.rmObject.hObject = m->handle;
	rc = rm_control(d, d->client, NV0000_CTRL_CMD_OS_UNIX_EXPORT_OBJECT_TO_FD, &p, sizeof(p),
			offsetof(NV0000_CTRL_OS_UNIX_EXPORT_OBJECT_TO_FD_PARAMS, fd), id);
	if (rc) {
		nvfwd_kclose(id);
		return rc;
	}
	m->export_id = id;
	return 0;
}

struct NvKmsKapiMemory *allocateMemory(struct NvKmsKapiDevice *d, struct NvKmsKapiAllocateMemoryParams *params)
{
	NV_MEMORY_ALLOCATION_PARAMS p = { 0 };
	struct NvKmsKapiMemory *m = memory_new(d, params->size);
	u32 handle;
	long rc;

	if (!m)
		return NULL;
	handle = m->handle;
	p.owner = 0x6b617069;
	p.flags = NVOS32_ALLOC_FLAGS_MEMORY_HANDLE_PROVIDED;
	p.size = params->size;
	rc = rm_alloc(d, HANDLE_DEVICE, &handle, 0x40, &p, sizeof(p));
	if (rc) {
		pr_warn("nvidia-modeset stand-in: allocate %llx: %lx\n", params->size, rc);
		kfree(m);
		return NULL;
	}
	if (params->compressible)
		*params->compressible = 0;
	m->layout.layout = NvKmsSurfaceMemoryLayoutPitch;
	return m;
}

struct NvKmsKapiMemory *dupMemory(struct NvKmsKapiDevice *d, const struct NvKmsKapiDevice *source, const struct NvKmsKapiMemory *memory)
{
	struct NvKmsKapiMemory *m = (struct NvKmsKapiMemory *)memory;

	refcount_inc(&m->refs);
	return m;
}

NvBool exportMemory(const struct NvKmsKapiDevice *cd, const struct NvKmsKapiMemory *memory, NvU64 user, NvU64 user_size)
{
	struct NvKmsKapiDevice *d = (struct NvKmsKapiDevice *)cd;
	struct NvKmsKapiPrivExportMemoryParams p;
	NV0000_CTRL_OS_UNIX_EXPORT_OBJECT_TO_FD_PARAMS e = { 0 };
	struct file *f;
	u32 id;
	long rc;

	if (user_size != sizeof(p) || copy_from_user(&p, u64_to_user_ptr(user), sizeof(p)))
		return NV_FALSE;
	f = fget(p.memFd);
	id = nvfwd_file_id(f);
	if (f)
		fput(f);
	if (!id)
		return NV_FALSE;
	e.object.type = NV0000_CTRL_OS_UNIX_EXPORT_OBJECT_TYPE_RM;
	e.object.data.rmObject.hDevice = HANDLE_DEVICE;
	e.object.data.rmObject.hParent = HANDLE_DEVICE;
	e.object.data.rmObject.hObject = memory->handle;
	rc = rm_control(d, d->client, NV0000_CTRL_CMD_OS_UNIX_EXPORT_OBJECT_TO_FD, &e, sizeof(e),
			offsetof(NV0000_CTRL_OS_UNIX_EXPORT_OBJECT_TO_FD_PARAMS, fd), id);
	return rc == 0;
}

NvBool mapMemory(const struct NvKmsKapiDevice *cd, const struct NvKmsKapiMemory *memory, NvKmsKapiMappingType type, void **out)
{
	struct NvKmsKapiDevice *d = (struct NvKmsKapiDevice *)cd;
	struct NvKmsKapiMemory *m = (struct NvKmsKapiMemory *)memory;
	long rc;

	if (!m->mapped) {
		/* A file holds one mapping context, so each mapping has a file of its own, as the user-mode driver does it. */
		nv_ioctl_nvos33_parameters_with_fd p = { 0 };
		struct { int ctl_fd; } reg = { 0 };
		long gpu = nvfwd_kopen(m->sysmem ? NVFWD_DEV_CTL : NVFWD_DEV_GPU); /* RM ties system memory to the control device */
		struct nvfwd_krm r = { gpu, CTL_ESC(ESC_REGISTER_FD, sizeof(reg)), &reg, sizeof(reg), 0, NULL, 0, 0, -1, d->ctl };
		struct nvfwd_krm a = { d->ctl, CTL_ESC(ESC_RM_MAP_MEMORY, sizeof(p)), &p, sizeof(p), 0, NULL, 0, offsetof(nv_ioctl_nvos33_parameters_with_fd, fd), -1, gpu };

		if (gpu < 0)
			return NV_FALSE;
		rc = m->sysmem ? 0 : nvfwd_rm(&r);
		p.params.hClient = d->client;
		p.params.hDevice = HANDLE_SUBDEVICE;
		p.params.hMemory = m->handle;
		p.params.length = m->size;
		/*
		 * As the user-mode driver maps a framebuffer: write combined, reflected. The kernel KAPI's MEM_SPACE_USER is refused
		 * here, since QEMU is a normal client of the host's RM.
		 */
		p.params.flags = m->sysmem ? 0 : 0x1010000;
		if (!rc)
			rc = nvfwd_rm(&a);
		if (rc || p.params.status) {
			pr_warn("nvidia-modeset stand-in: map: %ld %x\n", rc, p.params.status);
			nvfwd_kclose(gpu);
			return NV_FALSE;
		}
		rc = nvfwd_kmap(gpu, m->size, &m->phys, &m->window);
		nvfwd_kclose(gpu); /* the mapping keeps what it needs */
		if (rc) {
			pr_warn("nvidia-modeset stand-in: window: %ld\n", rc);
			return NV_FALSE;
		}
		m->mapped = 1;
	}
	if (type == NVKMS_KAPI_MAPPING_TYPE_USER) {
		*out = (void *)(uintptr_t)m->phys; /* nvidia-drm treats this one as a physical address */
		return NV_TRUE;
	}
	*out = ioremap_wc(m->phys, m->size);
	return *out != NULL;
}

void unmapMemory(const struct NvKmsKapiDevice *d, const struct NvKmsKapiMemory *m, NvKmsKapiMappingType type, const void *address)
{
	if (type == NVKMS_KAPI_MAPPING_TYPE_KERNEL)
		iounmap((void __iomem *)address);
}

void freeMemory(struct NvKmsKapiDevice *d, struct NvKmsKapiMemory *m)
{
	if (!refcount_dec_and_test(&m->refs))
		return;
	spin_lock_irq(&d->held_lock);
	if (m->show_id)
		d->shown[m->show_id - 1] = NULL;
	spin_unlock_irq(&d->held_lock);
	if (m->mapped)
		nvfwd_kunmap(m->window, m->size);
	rm_free(d, HANDLE_DEVICE, m->handle);
	if (m->import)
		fput(m->import);
	else if (m->export_id)
		nvfwd_kclose(m->export_id);
	kfree(m);
}

NvBool isVidmem(const struct NvKmsKapiMemory *m) { return NV_TRUE; }
NvBool isContiguous(const struct NvKmsKapiMemory *m) { return NV_TRUE; }
