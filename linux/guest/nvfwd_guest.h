// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * What the parts of the guest's nvidia module share. The module is a stand-in for nvidia.ko: real
 * /dev/nvidiactl, /dev/nvidia0 and /dev/nvidia-uvm nodes whose calls go to the host's driver through
 * QEMU's nvfwd device. It is named nvidia so that /sys/module/nvidia and /proc/modules look right to
 * the user-mode driver.
 */
#ifndef NVFWD_GUEST_H
#define NVFWD_GUEST_H

#include <linux/fs.h>
#include <linux/hrtimer.h>
#include <linux/pci.h>
#include <linux/semaphore.h>
#include <linux/wait.h>
#include "nvfwd_proto.h"

#define ESC_RM_ALLOC_MEMORY 0x27
#define ESC_RM_CONTROL 0x2a
#define ESC_RM_ALLOC 0x2b
#define ESC_RM_MAP_MEMORY 0x4e
#define ESC_RM_VID_HEAP_CONTROL 0x4a
#define ESC_RM_IDLE_CHANNELS 0x41
#define ESC_REGISTER_FD 201
#define ESC_ALLOC_OS_EVENT 206
#define ESC_FREE_OS_EVENT 207
#define ESC_XFER 211
#define CLASS_OS_DESCRIPTOR 0x71
#define CLASS_OS_EVENT 0x79
#define CLASS_EVENT 0x5

struct nvf_file {
	u32 fdid;
	int kind;
	u32 map_flags; /* NVOS33 flags of the last mapping aimed at this file */
	int uvm_process; /* its place in uvm_processes */
};

struct nvf_map {
	refcount_t refs;
	u64 off, len, addr;
	int uvm;
};

struct nvf_state {
	void __iomem *mmio;
	u8 *slots;
	resource_size_t win_base;
	u64 win_size;
	struct semaphore free;
	spinlock_t lock;
	unsigned long used;
	wait_queue_head_t pollq;
	struct hrtimer poll_timer;
	struct class *cls;
	int uvm_major;
	struct device *dev;
	const struct file_operations *fops; /* how a guest file of ours is told from any other */
};

extern struct nvf_state nv;

/* One call being assembled in a slot. */
struct saved {
	u32 at, size;
	u64 value;
};

struct outbuf {
	u32 off, len;
	u64 user;
};

struct call {
	int slot;
	struct nvfwd_req *req;
	u8 *data;
	u32 next;
	int nsaved, nout;
	struct saved saved[NVFWD_MAX_FIX];
	struct outbuf out[NVFWD_MAX_FIX];
};

/* slots.c */
int slot_get(void);
void slot_put(int i);
struct nvfwd_req *slot_req(int i);
void kick(int i);

/* call.c */
int place(struct call *c, u64 user, u32 len, u32 *off);
int follow(struct call *c, u32 at, u32 len, u32 *off);
int fix_fd(struct call *c, u32 at);
int pin_range(struct call *c, u32 at, u64 size);
struct call *call_new(u32 fdid, u32 cmd);
void call_free(struct call *c);

/* rm.c */
long rm_ioctl(struct nvf_file *nf, unsigned int cmd, unsigned long arg);

/* uvm.c */
long uvm_ioctl(struct nvf_file *nf, unsigned int cmd, unsigned long arg);
int uvm_process_join(void);
void uvm_process_leave(int place);

/* show.c */
long show_ioctl(unsigned int cmd, unsigned long arg);

/* files.c */
extern const struct file_operations nvf_fops;
enum hrtimer_restart poll_tick(struct hrtimer *t);

#endif
