// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * A stand-in for nvidia-modeset.ko, under NVIDIA's own nvidia-drm.ko in a guest: one virtual monitor,
 * no display hardware. Memory goes through an RM client of our own, by the guest module's slots.
 * Flips go to the show path, so the viewer takes the framebuffer where it lies in video memory. No
 * display class reaches the host. Semaphore surfaces are real.
 *
 * This header is what its parts share.
 */
#ifndef NVFWD_STANDIN_H
#define NVFWD_STANDIN_H

#include <linux/module.h>
#include <linux/mm.h>
#include <linux/slab.h>
#include <linux/file.h>
#include <linux/uaccess.h>
#include <linux/workqueue.h>
#include <linux/atomic.h>
#include <linux/ioctl.h>
#include <linux/io.h>
#include <linux/kthread.h>
#include <linux/hrtimer.h>
#include <linux/delay.h>
#include <linux/spinlock.h>
#include <linux/wait.h>
#include "nvkms-kapi.h"
#include "nvkms-kapi-private.h"
#include "nvos.h"
#include "nv-unix-nvos-params-wrappers.h"
#pragma push_macro("current") /* a field in this header has the kernel macro's name */
#undef current
#include "ctrl/ctrl0000/ctrl0000unix.h"
#pragma pop_macro("current")
#include "ctrl/ctrl0000/ctrl0000gpu.h"
#include "ctrl/ctrl00da.h"
#include "ctrl/ctrl2080/ctrl2080fb.h"
#include "nvfwd_proto.h"

#define CTL_ESC(nr, size) _IOC(_IOC_READ | _IOC_WRITE, 'F', nr, size)
#define ESC_RM_ALLOC 0x2b
#define ESC_RM_CONTROL 0x2a
#define ESC_RM_FREE 0x29
#define ESC_RM_DUP_OBJECT 0x34
#define ESC_RM_MAP_MEMORY 0x4e
#define ESC_RM_UNMAP_MEMORY 0x4f
#define ESC_REGISTER_FD 201
#define HANDLE_DEVICE 0xcaf00001
#define HANDLE_SUBDEVICE 0xcaf00002
#define HANDLE_MEMORY 0xcaf10000
#define DISPLAY_HANDLE 1
#define CONNECTOR_HANDLE 1
#define DISPLAY_SEMAPHORES 8
enum { SEM_IDLE, SEM_WAITING, SEM_READY, SEM_ABANDONED };
#define WIDTH 1920
#define HEIGHT 1080
#define FOURCC_XR24 0x34325258
#define FOURCC_AR24 0x34325241
#define FOURCC_XB24 0x34324258
#define FOURCC_AB24 0x34324241

struct nvfwd_krm {
	u32 fdid, cmd;
	void *top;
	u32 top_size, ptr_off;
	void *sub;
	u32 sub_size;
	s32 fd_top, fd_sub;
	u32 fd_id;
};
long nvfwd_rm(struct nvfwd_krm *a);
long nvfwd_kopen(int kind);
void nvfwd_kclose(u32 fdid);
long nvfwd_kmap(u32 fdid, u64 len, u64 *phys, u64 *window);
void nvfwd_kunmap(u64 window, u64 len);
long nvfwd_show_surface(u32 fdid, u64 id, u32 width, u32 height, u32 pitch, u32 fourcc, u64 size);
void nvfwd_show_flip(u32 id, u64 number);
u64 nvfwd_show_reading(void);
u64 nvfwd_show_rate(void);
u32 nvfwd_file_id(struct file *f);
struct device *nvfwd_device(void);

struct NvKmsKapiDevice {
	u32 ctl, gpu, client, next_handle;
	u32 gpu_id;
	void *private_data;
	void (*event)(const struct NvKmsKapiEvent *);
	atomic_t flips;
	struct work_struct flip_work;
	struct hrtimer flip_timer;
	/*
	 * A frame is not shown until the GPU has finished it: a commit with a fence waits here for its display semaphore,
	 * which nvidia-drm signals from the fence. These are only state in the guest, there being no display to wait.
	 */
	atomic_t semaphores[DISPLAY_SEMAPHORES];
	struct work_struct held_work;
	spinlock_t held_lock;
	struct NvKmsKapiSurface *held;
	u32 held_index;
	bool held_event;
	/* Who has each show id. A compositor scanning out a client buffer can bring new memory every frame. */
	struct NvKmsKapiMemory *shown[NVFWD_SHOW_MAX_SURFACES];
	u32 next_shown;
	u64 flip_number;
	u64 outgoing; /* the frame the newest flip replaces */
	u64 committed_ns;
	/*
	 * The monitor here is the viewer's. While one is ticking a flip completes when it takes the frame, at its monitor's real
	 * rate. Without one, flips complete on a grid at the last rate seen, and the mode says that rate either way.
	 */
	u64 period_ns;
	bool viewer;
	u32 viewer_ticks;
	struct delayed_work rate_work;
	bool had_surface;
	u64 semsurf_stride, semsurf_max_offset;
};

struct NvKmsKapiMemory {
	refcount_t refs;
	u32 handle;
	u64 size;
	struct file *import; /* its host side holds the exported object */
	u32 export_id; /* a host file holding it: the import's, or one we made */
	struct NvKmsKapiPrivSurfaceParams layout;
	u64 phys, window;
	int mapped, sysmem;
	int show_id; /* one and up: compositors make a new framebuffer object for the same memory every frame */
};

struct NvKmsKapiSurface {
	struct NvKmsKapiMemory *memory;
	u32 width, height, pitch, fourcc;
	u32 layout, gobs_y;
};

extern const struct NvKmsKapiCallbacks *callbacks;
extern bool log_flips, follow_viewer;
extern struct task_struct *waiters_thread;

/* rm.c */
long rm_alloc(struct NvKmsKapiDevice *d, u32 parent, u32 *handle, u32 hclass, void *params, u32 size);
long rm_control(struct NvKmsKapiDevice *d, u32 object, u32 cmd, void *params, u32 size, s32 fd_off, u32 fd_id);
void rm_free(struct NvKmsKapiDevice *d, u32 parent, u32 handle);

/* memory.c */
struct NvKmsKapiMemory *allocateMemory(struct NvKmsKapiDevice *d, struct NvKmsKapiAllocateMemoryParams *params);
struct NvKmsKapiMemory *dupMemory(struct NvKmsKapiDevice *d, const struct NvKmsKapiDevice *source, const struct NvKmsKapiMemory *memory);
NvBool exportMemory(const struct NvKmsKapiDevice *cd, const struct NvKmsKapiMemory *memory, NvU64 user, NvU64 user_size);
void freeMemory(struct NvKmsKapiDevice *d, struct NvKmsKapiMemory *m);
struct NvKmsKapiMemory *importMemory(struct NvKmsKapiDevice *d, NvU64 size, NvU64 user, NvU64 user_size);
NvBool isContiguous(const struct NvKmsKapiMemory *m);
NvBool isVidmem(const struct NvKmsKapiMemory *m);
NvBool mapMemory(const struct NvKmsKapiDevice *cd, const struct NvKmsKapiMemory *memory, NvKmsKapiMappingType type, void **out);
long memory_export_file(struct NvKmsKapiDevice *d, struct NvKmsKapiMemory *m);
void unmapMemory(const struct NvKmsKapiDevice *d, const struct NvKmsKapiMemory *m, NvKmsKapiMappingType type, const void *address);

/* flips.c */
NvBool applyModeSetConfig(struct NvKmsKapiDevice *d, const struct NvKmsKapiRequestedModeSetConfig *request, struct NvKmsKapiModeSetReplyConfig *reply, const NvBool commit);
struct NvKmsKapiSurface *createSurface(struct NvKmsKapiDevice *d, struct NvKmsKapiCreateSurfaceParams *params);
void destroySurface(struct NvKmsKapiDevice *d, struct NvKmsKapiSurface *s);
void flip_done(struct work_struct *w);
enum hrtimer_restart flip_timer_done(struct hrtimer *t);
NvBool getFlipPendingStatus(const struct NvKmsKapiDevice *d, const NvU32 head, const NvU32 layer, NvBool *pending);
void held_done(struct work_struct *w);
void rate_check(struct work_struct *w);

/* device.c */
struct NvKmsKapiDevice *allocateDevice(const struct NvKmsKapiAllocateDeviceParams *params);
NvBool declareEventInterest(const struct NvKmsKapiDevice *d, const NvU32 mask);
NvU32 enumerateGpus(void (*found)(const struct NvKmsKapiGpuInfo *info));
void freeDevice(struct NvKmsKapiDevice *d);
NvBool getConnectorInfo(struct NvKmsKapiDevice *d, NvKmsKapiConnector connector, struct NvKmsKapiConnectorInfo *info);
NvBool getDeviceResourcesInfo(struct NvKmsKapiDevice *d, struct NvKmsKapiDeviceResourcesInfo *info);
int getDisplayMode(struct NvKmsKapiDevice *d, NvKmsKapiDisplay display, NvU32 index, struct NvKmsKapiDisplayMode *mode, NvBool *valid, NvBool *preferred);
NvBool getDisplays(struct NvKmsKapiDevice *d, NvU32 *count, NvKmsKapiDisplay *handles);
NvBool getDynamicDisplayInfo(struct NvKmsKapiDevice *d, struct NvKmsKapiDynamicDisplayParams *params);
NvBool getStaticDisplayInfo(struct NvKmsKapiDevice *d, NvKmsKapiDisplay display, struct NvKmsKapiStaticDisplayInfo *info);
NvBool grantPermissions(NvS32 fd, struct NvKmsKapiDevice *d, NvU32 head, NvKmsKapiDisplay display);
NvBool grantSubOwnership(NvS32 fd, struct NvKmsKapiDevice *d);
void nothing_device(struct NvKmsKapiDevice *d);
NvBool revokePermissions(struct NvKmsKapiDevice *d, NvU32 head, NvKmsKapiDisplay display);
NvBool revokeSubOwnership(struct NvKmsKapiDevice *d);
NvBool validateDisplayMode(struct NvKmsKapiDevice *d, NvKmsKapiDisplay display, const struct NvKmsKapiDisplayMode *mode);
NvBool yes_const_device(const struct NvKmsKapiDevice *d);
NvBool yes_device(struct NvKmsKapiDevice *d);

/* semaphores.c */
void freeSemaphoreSurface(struct NvKmsKapiDevice *d, struct NvKmsKapiSemaphoreSurface *s);
struct NvKmsKapiSemaphoreSurface *importSemaphoreSurface(struct NvKmsKapiDevice *d, NvU64 user, NvU64 user_size, void **sem_map, void **max_map);
NvKmsKapiRegisterWaiterResult registerSemaphore(struct NvKmsKapiDevice *d, struct NvKmsKapiSemaphoreSurface *s, NvKmsSemaphoreSurfaceCallbackProc *proc, void *data, NvU64 index, NvU64 wait, NvU64 value, struct NvKmsKapiSemaphoreSurfaceCallback **h);
NvBool setSemaphore(struct NvKmsKapiDevice *d, struct NvKmsKapiSemaphoreSurface *s, NvU64 index, NvU64 value);
NvBool unregisterSemaphore(struct NvKmsKapiDevice *d, struct NvKmsKapiSemaphoreSurface *s, NvU64 index, NvU64 wait, struct NvKmsKapiSemaphoreSurfaceCallback *h);
int waiters_main(void *unused);

#endif
