// SPDX-License-Identifier: GPL-2.0-or-later
/* The function table nvidia-drm asks for, and what is refused or does nothing. */
#include "standin.h"

/*
 * The table's size where every function in it is filled in below. A driver that adds one must fail
 * here, in the build, which turns the GPU off and says why. Otherwise nvidia-drm would call a null
 * pointer in the guest's kernel.
 */
#define KNOWN_TABLE_BYTES 424
_Static_assert(sizeof(struct NvKmsKapiFunctionsTable) == KNOWN_TABLE_BYTES,
	       "this driver's KAPI table has changed: the stand-in needs its new functions");

static struct NvKmsKapiChannelEvent *allocateChannelEvent(struct NvKmsKapiDevice *d, NvKmsChannelEventProc *proc, void *data, NvU64 u, NvU64 s) { return NULL; }
static void freeChannelEvent(struct NvKmsKapiDevice *d, struct NvKmsKapiChannelEvent *cb) { }
static NvBool getCRC32(struct NvKmsKapiDevice *d, NvU32 head, struct NvKmsKapiCrcs *crc) { return NV_FALSE; }
static NvBool getMemoryPages(const struct NvKmsKapiDevice *d, const struct NvKmsKapiMemory *m, NvU64 **p, NvU32 *n) { return NV_FALSE; }
static void freeMemoryPages(NvU64 *p) { }
static struct NvKmsKapiMemory *fromSgt(struct NvKmsKapiDevice *d, NvP64 sgt, NvP64 gem, NvU32 limit) { return NULL; }
static struct NvKmsKapiMemory *fromDmaBuf(struct NvKmsKapiDevice *d, NvP64 buf, NvU32 limit) { return NULL; }
static void setCallbacks(const struct NvKmsKapiCallbacks *c) { callbacks = c; }
static NvBool tryInitDisplaySemaphore(struct NvKmsKapiDevice *d, NvU32 i)
{
	return atomic_cmpxchg(&d->semaphores[i], SEM_IDLE, SEM_WAITING) == SEM_IDLE;
}

/* From a fence callback, so possibly in an interrupt: showing the frame is left to the work. */
static void signalDisplaySemaphore(struct NvKmsKapiDevice *d, NvU32 i)
{
	if (atomic_cmpxchg(&d->semaphores[i], SEM_ABANDONED, SEM_IDLE) == SEM_ABANDONED)
		return;
	atomic_cmpxchg(&d->semaphores[i], SEM_WAITING, SEM_READY);
	schedule_work(&d->held_work);
}

static void cancelDisplaySemaphore(struct NvKmsKapiDevice *d, NvU32 i)
{
	atomic_set(&d->semaphores[i], SEM_IDLE);
}
static struct NvKmsKapiVblankIntrCallback *registerVblank(struct NvKmsKapiDevice *d, const NvU32 head, NVRgInterruptCallbackProc p, NvU64 param) { return NULL; }
static void unregisterVblank(struct NvKmsKapiDevice *d, const NvU32 head, struct NvKmsKapiVblankIntrCallback *p) { }

NvBool nvKmsKapiGetFunctionsTable(struct NvKmsKapiFunctionsTable *t)
{
	t->systemInfo.bAllowWriteCombining = NV_TRUE;
	t->enumerateGpus = enumerateGpus;
	t->allocateDevice = allocateDevice;
	t->freeDevice = freeDevice;
	t->grabOwnership = yes_device;
	t->releaseOwnership = nothing_device;
	t->grantPermissions = grantPermissions;
	t->revokePermissions = revokePermissions;
	t->grantSubOwnership = grantSubOwnership;
	t->revokeSubOwnership = revokeSubOwnership;
	t->declareEventInterest = declareEventInterest;
	t->getDeviceResourcesInfo = getDeviceResourcesInfo;
	t->getDisplays = getDisplays;
	t->getConnectorInfo = getConnectorInfo;
	t->getStaticDisplayInfo = getStaticDisplayInfo;
	t->getDynamicDisplayInfo = getDynamicDisplayInfo;
	t->allocateMemory = allocateMemory;
	t->importMemory = importMemory;
	t->dupMemory = dupMemory;
	t->exportMemory = exportMemory;
	t->freeMemory = freeMemory;
	t->mapMemory = mapMemory;
	t->unmapMemory = unmapMemory;
	t->isVidmem = isVidmem;
	t->isContiguous = isContiguous;
	t->gc6BlockerRefCntInc = yes_const_device;
	t->gc6BlockerRefCntDec = yes_const_device;
	t->createSurface = createSurface;
	t->destroySurface = destroySurface;
	t->getDisplayMode = getDisplayMode;
	t->validateDisplayMode = validateDisplayMode;
	t->applyModeSetConfig = applyModeSetConfig;
	t->getFlipPendingStatus = getFlipPendingStatus;
	t->allocateChannelEvent = allocateChannelEvent;
	t->freeChannelEvent = freeChannelEvent;
	t->getCRC32 = getCRC32;
	t->getMemoryPages = getMemoryPages;
	t->freeMemoryPages = freeMemoryPages;
	t->getSystemMemoryHandleFromSgt = fromSgt;
	t->getSystemMemoryHandleFromDmaBuf = fromDmaBuf;
	t->importSemaphoreSurface = importSemaphoreSurface;
	t->freeSemaphoreSurface = freeSemaphoreSurface;
	t->registerSemaphoreSurfaceCallback = registerSemaphore;
	t->unregisterSemaphoreSurfaceCallback = unregisterSemaphore;
	t->setSemaphoreSurfaceValue = setSemaphore;
	t->setCallbacks = setCallbacks;
	t->tryInitDisplaySemaphore = tryInitDisplaySemaphore;
	t->signalDisplaySemaphore = signalDisplaySemaphore;
	t->cancelDisplaySemaphore = cancelDisplaySemaphore;
	t->framebufferConsoleDisabled = nothing_device;
	t->registerVblankIntrCallback = registerVblank;
	t->unregisterVblankIntrCallback = unregisterVblank;
	return NV_TRUE;
}
EXPORT_SYMBOL(nvKmsKapiGetFunctionsTable);

/* Colour tables only. Nothing here sets a LUT or a matrix, so these stay unused. */
NvU32 nvKmsKapiF16ToF32(NvU16 a) { return 0; }
EXPORT_SYMBOL(nvKmsKapiF16ToF32);
NvU16 nvKmsKapiF32ToF16(NvU32 a) { return 0; }
EXPORT_SYMBOL(nvKmsKapiF32ToF16);
NvU32 nvKmsKapiF32Mul(NvU32 a, NvU32 b) { return 0; }
EXPORT_SYMBOL(nvKmsKapiF32Mul);
NvU32 nvKmsKapiF32Div(NvU32 a, NvU32 b) { return 0; }
EXPORT_SYMBOL(nvKmsKapiF32Div);
NvU32 nvKmsKapiF32Add(NvU32 a, NvU32 b) { return 0; }
EXPORT_SYMBOL(nvKmsKapiF32Add);
NvU32 nvKmsKapiF32ToUI32RMinMag(NvU32 a, NvBool exact) { return 0; }
EXPORT_SYMBOL(nvKmsKapiF32ToUI32RMinMag);
NvU32 nvKmsKapiUI32ToF32(NvU32 a) { return 0; }
EXPORT_SYMBOL(nvKmsKapiUI32ToF32);

static int __init standin_init(void)
{
	waiters_thread = kthread_run(waiters_main, NULL, "nvfwd-semaphores");
	return PTR_ERR_OR_ZERO(waiters_thread);
}
module_init(standin_init);

static void __exit standin_exit(void)
{
	kthread_stop(waiters_thread);
}
module_exit(standin_exit);
