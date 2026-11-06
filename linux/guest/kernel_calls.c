// SPDX-License-Identifier: GPL-2.0-or-later
/* The same calls for nvidia-modeset, from kernel memory. */
#include <linux/file.h>
#include <linux/mm.h>
#include <linux/mman.h>
#include <linux/module.h>
#include <linux/string.h>
#include "nvfwd_guest.h"

struct nvfwd_krm {
	u32 fdid, cmd;
	void *top;
	u32 top_size, ptr_off; /* ptr_off 0: no second buffer */
	void *sub;
	u32 sub_size;
	s32 fd_top, fd_sub; /* offset of an fd field, or -1 */
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

long nvfwd_rm(struct nvfwd_krm *a)
{
	int slot = slot_get();
	struct nvfwd_req *r = slot_req(slot);
	u8 *data = (u8 *)r + NVFWD_HDR_SIZE;
	u32 sub_at = ALIGN(a->top_size, 16), n = 0;
	long rc;

	r->op = NVFWD_OP_IOCTL;
	r->fdid = a->fdid;
	r->cmd = a->cmd;
	memcpy(data, a->top, a->top_size);
	if (a->sub) {
		memcpy(data + sub_at, a->sub, a->sub_size);
		r->fix[n++] = (struct nvfwd_fix){ NVFWD_FIX_PTR, a->ptr_off, sub_at, 0 };
	}
	if (a->fd_top >= 0)
		r->fix[n++] = (struct nvfwd_fix){ NVFWD_FIX_FD, a->fd_top, a->fd_id, 0 };
	if (a->fd_sub >= 0)
		r->fix[n++] = (struct nvfwd_fix){ NVFWD_FIX_FD, sub_at + a->fd_sub, a->fd_id, 0 };
	r->nfix = n;
	r->len = sub_at + (a->sub ? a->sub_size : 0);
	kick(slot);
	rc = r->ret;
	if (rc >= 0) {
		memcpy(a->top, data, a->top_size);
		if (a->sub)
			memcpy(a->sub, data + sub_at, a->sub_size);
	}
	slot_put(slot);
	return rc;
}
EXPORT_SYMBOL_GPL(nvfwd_rm);

static long simple_op(u32 op, u32 fdid, u64 a0, u64 a1, u64 a2, u64 a3, u64 *out)
{
	int slot = slot_get();
	struct nvfwd_req *r = slot_req(slot);
	long rc;

	r->op = op;
	r->fdid = fdid;
	r->a0 = a0;
	r->a1 = a1;
	r->a2 = a2;
	r->a3 = a3;
	kick(slot);
	rc = r->ret;
	if (out)
		*out = r->a3;
	slot_put(slot);
	return rc;
}

long nvfwd_kopen(int kind)
{
	return simple_op(NVFWD_OP_OPEN, 0, kind, 0, 0, 0, NULL);
}
EXPORT_SYMBOL_GPL(nvfwd_kopen);

void nvfwd_kclose(u32 fdid)
{
	simple_op(NVFWD_OP_CLOSE, fdid, 0, 0, 0, 0, NULL);
}
EXPORT_SYMBOL_GPL(nvfwd_kclose);

/* Maps what the last RM_MAP_MEMORY aimed at this file, and returns its physical address in the guest. */
long nvfwd_kmap(u32 fdid, u64 len, u64 *phys, u64 *window)
{
	u64 off = 0;
	long rc = simple_op(NVFWD_OP_MMAP, fdid, len, PROT_READ | PROT_WRITE, 0, 0, &off);

	if (!rc) {
		*window = off;
		*phys = nv.win_base + off;
	}
	return rc;
}
EXPORT_SYMBOL_GPL(nvfwd_kmap);

void nvfwd_kunmap(u64 window, u64 len)
{
	simple_op(NVFWD_OP_MUNMAP, 0, window, len, 0, 0, NULL);
}
EXPORT_SYMBOL_GPL(nvfwd_kunmap);

long nvfwd_show_surface(u32 fdid, u64 id, u32 width, u32 height, u32 pitch, u32 fourcc, u64 size)
{
	return simple_op(NVFWD_OP_SHOW_SURFACE, fdid, id, width | (u64)height << 32, pitch | (u64)fourcc << 32, size, NULL);
}
EXPORT_SYMBOL_GPL(nvfwd_show_surface);

u64 nvfwd_show_reading(void)
{
	u64 number = 0;

	simple_op(NVFWD_OP_SHOW_READING, 0, 0, 0, 0, 0, &number);
	return number;
}
EXPORT_SYMBOL_GPL(nvfwd_show_reading);

u64 nvfwd_show_rate(void)
{
	u64 rate = 0;

	simple_op(NVFWD_OP_SHOW_RATE, 0, 0, 0, 0, 0, &rate);
	return rate;
}
EXPORT_SYMBOL_GPL(nvfwd_show_rate);

void nvfwd_show_flip(u32 id, u64 number)
{
	simple_op(NVFWD_OP_SHOW_FRAME, 0, id | number << 8, 0, 0, 0, NULL);
}
EXPORT_SYMBOL_GPL(nvfwd_show_flip);

/* The fd table id of a guest file of ours, or 0 if it is not one. */
u32 nvfwd_file_id(struct file *f)
{
	return f && f->f_op == nv.fops ? ((struct nvf_file *)f->private_data)->fdid : 0;
}
EXPORT_SYMBOL_GPL(nvfwd_file_id);

struct device *nvfwd_device(void)
{
	return nv.dev;
}
EXPORT_SYMBOL_GPL(nvfwd_device);
