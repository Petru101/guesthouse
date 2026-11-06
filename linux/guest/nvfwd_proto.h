/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Wire format between the guest's modules and QEMU's nvfwd device. The device has its own copy of this. */
#ifndef NVFWD_PROTO_H
#define NVFWD_PROTO_H
#ifndef __KERNEL__
#include <stdint.h>
#include <sys/ioctl.h>
#endif

#define NVFWD_SLOT_SIZE  (2u << 20)
#define NVFWD_SLOTS      16u
#define NVFWD_HDR_SIZE   8192u
#define NVFWD_DATA_SIZE  (NVFWD_SLOT_SIZE - NVFWD_HDR_SIZE)
#define NVFWD_MAX_FIX    128u
#define NVFWD_MAGIC      0x6e766664u

enum { NVFWD_OP_OPEN = 1, NVFWD_OP_CLOSE, NVFWD_OP_IOCTL, NVFWD_OP_MMAP, NVFWD_OP_MUNMAP, NVFWD_OP_POLL,
       NVFWD_OP_SHOW_BUFFER, NVFWD_OP_SHOW_NEXT, NVFWD_OP_SHOW_FRAME, NVFWD_OP_SHOW_SURFACE, NVFWD_OP_SHOW_READING, NVFWD_OP_SHOW_RATE };
/* First argument of SHOW_SURFACE: which surface, and how its memory is laid out, as NVKMS describes it. */
#define NVFWD_SURFACE_ID(id, layout, generic, gobs_y, pitch_blocks) \
    ((id) | (uint64_t)(layout) << 8 | (uint64_t)!!(generic) << 9 | (uint64_t)(gobs_y) << 12 | (uint64_t)(pitch_blocks) << 32)
enum { NVFWD_DEV_CTL = 0, NVFWD_DEV_GPU = 1, NVFWD_DEV_UVM = 2 };
enum {
    NVFWD_FIX_PTR = 0,   /* u64 at `at` becomes the host address of data + target */
    NVFWD_FIX_FD = 1,    /* s32 at `at` becomes the host fd of table entry `target` */
    NVFWD_FIX_PAGES = 2, /* u64 at `at` becomes a host address made from the run list at data + target */
};

struct nvfwd_fix {
    uint32_t kind, at, target, pad;
};

struct nvfwd_run {
    uint64_t gpa, len;
};

/* At data + target for NVFWD_FIX_PAGES. */
struct nvfwd_runs {
    uint32_t count, pad;
    uint64_t first_offset;
    struct nvfwd_run run[];
};

struct nvfwd_req {
    uint32_t op, fdid, cmd, nfix, len;
    int32_t ret;
    uint64_t a0, a1, a2, a3;
    struct nvfwd_fix fix[NVFWD_MAX_FIX];
};

/* The "show this surface" call. The fd is one vkGetMemoryFdKHR made, so the host side holds the exported object. */
#define NVFWD_SHOW_MAX_SURFACES 32u /* a compositor's framebuffers, with no fixed set */
#define NVFWD_SHOW_SLOTS 5u /* the viewer holds up to 3, one is the newest, and the next frame needs one more */
struct nvfwd_show_buffer {
    int32_t fd;
    uint32_t slot, width, height, pitch, format;
    uint64_t size;
};
#define NVFWD_SHOW_IOC_BUFFER _IOW('S', 1, struct nvfwd_show_buffer)
#define NVFWD_SHOW_IOC_NEXT _IOR('S', 2, uint32_t)
/* low 8 bits the slot, the rest the present number of this frame, for checking */
#define NVFWD_SHOW_IOC_FRAME _IOW('S', 3, uint64_t)

#endif
