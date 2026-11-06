// SPDX-License-Identifier: GPL-2.0-or-later
/* RM calls: which of their arguments point to more, and where files sit in them. */
#include <linux/file.h>
#include <linux/overflow.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include "nvfwd_guest.h"

struct embedded {
	u32 cmd, ptr_off, count_off, count_size, elem;
};
static const struct embedded embedded[] = {
#include "nv_tables.h"
#include "nv_tables_extra.h"
};

struct fd_control {
	u32 cmd, off;
};
static const struct fd_control fd_controls[] = {
#include "fd_controls.h"
};

struct class_size {
	u32 hclass, size;
};
static const struct class_size class_sizes[] = {
#include "class_tables.h"
};

static int rm_control(struct call *c, u32 base)
{
	u32 cmd, size, params;
	size_t i;
	int rc;

	memcpy(&cmd, c->data + base + 8, 4);
	memcpy(&size, c->data + base + 24, 4);
	rc = follow(c, base + 16, size, &params);
	if (rc || !params)
		return rc;
	for (i = 0; i < ARRAY_SIZE(fd_controls); i++)
		if (fd_controls[i].cmd == cmd) {
			rc = fix_fd(c, params + fd_controls[i].off);
			if (rc)
				return rc;
		}
	for (i = 0; i < ARRAY_SIZE(embedded); i++) {
		const struct embedded *e = &embedded[i];
		u32 count = 0, inner, bytes;

		if (e->cmd != cmd)
			continue;
		memcpy(&count, c->data + params + e->count_off, e->count_size);
		if (check_mul_overflow(count, e->elem, &bytes))
			return -EINVAL;
		rc = follow(c, params + e->ptr_off, bytes, &inner);
		if (rc)
			return rc;
	}
	return 0;
}

static int rm_alloc(struct call *c, u32 base, u32 top)
{
	u32 hclass, size, params, rights;
	int rc;

	memcpy(&hclass, c->data + base + 12, 4);
	memcpy(&size, c->data + base + (top == 32 ? 24 : 32), 4);
	if (!size) { /* RM then takes the size from its class table */
		size_t i;

		for (i = 0; i < ARRAY_SIZE(class_sizes); i++)
			if (class_sizes[i].hclass == hclass)
				size = class_sizes[i].size;
	}
	rc = follow(c, base + 16, size, &params);
	if (rc)
		return rc;
	if (top != 32) {
		rc = follow(c, base + 24, 4, &rights);
		if (rc)
			return rc;
	}
	if ((hclass == CLASS_OS_EVENT || hclass == CLASS_EVENT) && params) {
		u32 kind;

		memcpy(&kind, c->data + params + 8, 4);
		if (kind == CLASS_OS_EVENT)
			return fix_fd(c, params + 16);
	}
	if (hclass == CLASS_OS_DESCRIPTOR) {
		pr_warn("nvfwd: os descriptor through RM_ALLOC refused\n");
		return -EOPNOTSUPP;
	}
	return 0;
}

static int rm_alloc_memory(struct call *c, u32 base)
{
	u32 hclass;
	u64 limit;
	int rc;

	memcpy(&hclass, c->data + base + 12, 4);
	memcpy(&limit, c->data + base + 32, 8);
	rc = fix_fd(c, base + 48);
	if (rc)
		return rc;
	if (hclass == CLASS_OS_DESCRIPTOR)
		return pin_range(c, base + 24, limit + 1);
	return 0;
}

/* After NV_ESC_RM_MAP_MEMORY the target file remembers how the host wants it mapped. */
static void note_map(struct call *c, u32 base, s32 fd)
{
	struct file *f = fget(fd);

	if (!f)
		return;
	if (f->f_op == nv.fops)
		memcpy(&((struct nvf_file *)f->private_data)->map_flags, c->data + base + 44, 4);
	fput(f);
}

long rm_ioctl(struct nvf_file *nf, unsigned int cmd, unsigned long arg)
{
	struct call *c;
	u32 nr = _IOC_NR(cmd), size = _IOC_SIZE(cmd), base = 0;
	s32 map_fd = -1;
	long rc;
	int i;

	if (_IOC_TYPE(cmd) != 'F')
		return -ENOTTY;
	c = call_new(nf->fdid, cmd);
	if (!c)
		return -ENOMEM;

	rc = place(c, arg, size, &base);
	if (rc)
		goto out;
	if (nr == ESC_XFER) { /* nv_ioctl_xfer_t: cmd, size, pointer */
		u32 icmd, isize;

		memcpy(&icmd, c->data, 4);
		memcpy(&isize, c->data + 4, 4);
		rc = follow(c, 8, isize, &base);
		if (rc)
			goto out;
		/* The host expects the inner buffer right behind the header. */
		if (base != 16) {
			rc = -EINVAL;
			goto out;
		}
		nr = icmd & 0xff;
		size = isize;
	}
	switch (nr) {
	case ESC_RM_CONTROL:
		rc = rm_control(c, base);
		break;
	case ESC_RM_ALLOC:
		rc = rm_alloc(c, base, size);
		break;
	case ESC_RM_ALLOC_MEMORY:
		rc = rm_alloc_memory(c, base);
		break;
	case ESC_RM_MAP_MEMORY:
		memcpy(&map_fd, c->data + base + 48, 4);
		rc = fix_fd(c, base + 48);
		break;
	case ESC_REGISTER_FD:
		rc = fix_fd(c, base);
		break;
	case ESC_ALLOC_OS_EVENT:
	case ESC_FREE_OS_EVENT:
		rc = fix_fd(c, base + 8);
		break;
	case ESC_RM_VID_HEAP_CONTROL: {
		u32 function;

		memcpy(&function, c->data + base + 8, 4);
		if (function == 27) {
			pr_warn("nvfwd: os descriptor through VID_HEAP refused\n");
			rc = -EOPNOTSUPP;
		}
		break;
	}
	case ESC_RM_IDLE_CHANNELS: {
		u32 count, bytes, off;

		memcpy(&count, c->data + base + 12, 4);
		if (check_mul_overflow(count, 4u, &bytes)) {
			rc = -EINVAL;
			break;
		}
		rc = follow(c, base + 16, bytes, &off);
		if (!rc)
			rc = follow(c, base + 24, bytes, &off);
		if (!rc)
			rc = follow(c, base + 32, bytes, &off);
		break;
	}
	default:
		break;
	}
	if (rc)
		goto out;
	c->req->len = c->next;
	kick(c->slot);
	rc = c->req->ret;
	if (rc < 0)
		goto out;
	if (nr == ESC_RM_MAP_MEMORY)
		note_map(c, base, map_fd);
	for (i = c->nsaved - 1; i >= 0; i--)
		memcpy(c->data + c->saved[i].at, &c->saved[i].value, c->saved[i].size);
	for (i = 0; i < c->nout; i++)
		if (copy_to_user(u64_to_user_ptr(c->out[i].user), c->data + c->out[i].off, c->out[i].len))
			rc = -EFAULT;
out:
	call_free(c);
	return rc;
}
