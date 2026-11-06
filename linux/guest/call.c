// SPDX-License-Identifier: GPL-2.0-or-later
/* Copying a call and what it points to into a slot, and saying where its pointers and files are. */
#include <linux/file.h>
#include <linux/mm.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include "nvfwd_guest.h"

int place(struct call *c, u64 user, u32 len, u32 *off)
{
	u32 at = ALIGN(c->next, 16);

	if (len > NVFWD_DATA_SIZE || at + len > NVFWD_DATA_SIZE || c->nout >= NVFWD_MAX_FIX)
		return -E2BIG;
	if (copy_from_user(c->data + at, u64_to_user_ptr(user), len))
		return -EFAULT;
	c->next = at + len;
	c->out[c->nout++] = (struct outbuf){ at, len, user };
	*off = at;
	return 0;
}

static int add_fix(struct call *c, u32 kind, u32 at, u32 target, u32 size)
{
	u32 n = c->req->nfix;

	if (n >= NVFWD_MAX_FIX)
		return -E2BIG;
	c->saved[c->nsaved].at = at;
	c->saved[c->nsaved].size = size;
	c->saved[c->nsaved].value = 0;
	memcpy(&c->saved[c->nsaved].value, c->data + at, size);
	c->nsaved++;
	c->req->fix[n] = (struct nvfwd_fix){ kind, at, target, 0 };
	c->req->nfix = n + 1;
	return 0;
}

/* The u64 at `at` in the data points to `len` bytes of user memory. */
int follow(struct call *c, u32 at, u32 len, u32 *off)
{
	u64 user;
	int rc;

	memcpy(&user, c->data + at, 8);
	if (!user || !len) {
		*off = 0;
		return 0;
	}
	rc = place(c, user, len, off);
	if (rc)
		return rc;
	return add_fix(c, NVFWD_FIX_PTR, at, *off, 8);
}

/* The s32 at `at` is a guest fd of ours. Anything else reaches the host as -1. */
int fix_fd(struct call *c, u32 at)
{
	s32 fd;
	struct file *f;
	u32 id = 0;

	memcpy(&fd, c->data + at, 4);
	if (fd >= 0) {
		f = fget(fd);
		if (f) {
			if (f->f_op == nv.fops)
				id = ((struct nvf_file *)f->private_data)->fdid;
			fput(f);
		}
	}
	if (id)
		return add_fix(c, NVFWD_FIX_FD, at, id, 4);
	/* Saved so that the caller sees its own value again. */
	c->saved[c->nsaved].at = at;
	c->saved[c->nsaved].size = 4;
	c->saved[c->nsaved].value = (u32)fd;
	c->nsaved++;
	fd = -1;
	memcpy(c->data + at, &fd, 4);
	return 0;
}

/* Pins user memory and gives the host a run list of the guest pages behind it. */
int pin_range(struct call *c, u32 at, u64 size)
{
	u64 user;
	unsigned long start, npages, i;
	struct page **pages;
	struct nvfwd_runs *runs;
	u32 off;
	long got;
	int rc = 0;

	memcpy(&user, c->data + at, 8);
	if (!size || user + size < user)
		return -EINVAL;
	start = user & PAGE_MASK;
	npages = (user + size - start + PAGE_SIZE - 1) >> PAGE_SHIFT;
	off = ALIGN(c->next, 16);
	pages = kvmalloc_array(npages, sizeof(*pages), GFP_KERNEL);
	if (!pages)
		return -ENOMEM;
	got = pin_user_pages_fast(start, npages, FOLL_WRITE | FOLL_LONGTERM, pages);
	if (got != npages) {
		pr_warn("nvfwd: pin %lx+%lx got %ld\n", start, npages, got);
		if (got > 0)
			unpin_user_pages(pages, got);
		kvfree(pages);
		return got < 0 ? got : -EFAULT;
	}
	/* The pins stay until the guest reboots: nothing here hears when the host lets go. */
	runs = (struct nvfwd_runs *)(c->data + off);
	runs->count = 0;
	runs->first_offset = user - start;
	for (i = 0; i < npages; i++) {
		u64 gpa = page_to_phys(pages[i]);
		struct nvfwd_run *last = runs->count ? &runs->run[runs->count - 1] : NULL;

		if (last && last->gpa + last->len == gpa) {
			last->len += PAGE_SIZE;
			continue;
		}
		if (off + sizeof(*runs) + (runs->count + 1) * sizeof(*last) > NVFWD_DATA_SIZE) {
			rc = -E2BIG;
			break;
		}
		runs->run[runs->count].gpa = gpa;
		runs->run[runs->count++].len = PAGE_SIZE;
	}
	kvfree(pages);
	if (rc)
		return rc;
	c->next = off + sizeof(*runs) + runs->count * sizeof(runs->run[0]);
	return add_fix(c, NVFWD_FIX_PAGES, at, off, 8);
}

/* A call on this file, with a slot of its own, or NULL. */
struct call *call_new(u32 fdid, u32 cmd)
{
	struct call *c = kmalloc(sizeof(*c), GFP_KERNEL);

	if (!c)
		return NULL;
	c->slot = slot_get();
	c->req = slot_req(c->slot);
	c->data = (u8 *)c->req + NVFWD_HDR_SIZE;
	c->next = 0;
	c->nsaved = c->nout = 0;
	c->req->op = NVFWD_OP_IOCTL;
	c->req->fdid = fdid;
	c->req->cmd = cmd;
	c->req->nfix = 0;
	return c;
}

void call_free(struct call *c)
{
	slot_put(c->slot);
	kfree(c);
}
