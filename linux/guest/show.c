// SPDX-License-Identifier: GPL-2.0-or-later
#include <linux/file.h>
#include <linux/uaccess.h>
#include "nvfwd_guest.h"

/* The "show this surface" calls. These go to QEMU's own code, never to the host's driver. */
long show_ioctl(unsigned int cmd, unsigned long arg)
{
	struct nvfwd_show_buffer b;
	struct nvfwd_req *r;
	u64 value = 0;
	long rc;
	int slot;

	if (cmd == NVFWD_SHOW_IOC_BUFFER) {
		struct file *f;

		if (copy_from_user(&b, (void __user *)arg, sizeof(b)))
			return -EFAULT;
		f = fget(b.fd);
		if (!f)
			return -EBADF;
		if (f->f_op != nv.fops) {
			fput(f);
			return -EBADF;
		}
		slot = slot_get();
		r = slot_req(slot);
		r->op = NVFWD_OP_SHOW_BUFFER;
		r->fdid = ((struct nvf_file *)f->private_data)->fdid;
		r->a0 = b.slot;
		r->a1 = b.width | (u64)b.height << 32;
		r->a2 = b.pitch | (u64)b.format << 32;
		r->a3 = b.size;
		kick(slot);
		rc = r->ret;
		slot_put(slot);
		fput(f);
		return rc;
	}
	if (cmd == NVFWD_SHOW_IOC_FRAME && copy_from_user(&value, (void __user *)arg, 8))
		return -EFAULT;
	slot = slot_get();
	r = slot_req(slot);
	r->op = cmd == NVFWD_SHOW_IOC_NEXT ? NVFWD_OP_SHOW_NEXT : NVFWD_OP_SHOW_FRAME;
	r->a0 = value;
	kick(slot);
	rc = r->ret;
	slot_put(slot);
	if (cmd == NVFWD_SHOW_IOC_NEXT && rc >= 0) {
		value = rc;
		if (copy_to_user((void __user *)arg, &value, 4))
			return -EFAULT;
		return 0;
	}
	return rc;
}
