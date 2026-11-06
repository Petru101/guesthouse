// SPDX-License-Identifier: GPL-2.0-or-later
/* UVM calls, and keeping each process's UVM addresses apart in QEMU's one address space. */
#include <linux/mman.h>
#include <linux/mm.h>
#include <linux/mutex.h>
#include <linux/sched.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <uapi/asm-generic/mman-common.h>
#include "nvfwd_guest.h"

struct uvm_call {
	u32 cmd, size;
	int fd_off;
};
static const struct uvm_call uvm_calls[] = {
#include "uvm_tables.h"
	{ 0x30000002, 0, -1 }, /* UVM_DEINITIALIZE */
};

long uvm_ioctl(struct nvf_file *nf, unsigned int cmd, unsigned long arg)
{
	const struct uvm_call *u = NULL;
	struct call *c;
	u32 base;
	size_t i;
	long rc;
	int j;

	for (i = 0; i < ARRAY_SIZE(uvm_calls); i++)
		if (uvm_calls[i].cmd == cmd)
			u = &uvm_calls[i];
	if (!u) {
		pr_warn("nvfwd: unknown uvm call %x\n", cmd);
		return -ENOTTY;
	}
	c = call_new(nf->fdid, cmd);
	if (!c)
		return -ENOMEM;
	rc = u->size ? place(c, arg, u->size, &base) : 0;
	if (!rc && u->fd_off >= 0)
		rc = fix_fd(c, u->fd_off);
	if (rc)
		goto out;
	c->req->len = c->next;
	kick(c->slot);
	rc = c->req->ret;
	if (rc < 0)
		goto out;
	for (j = c->nsaved - 1; j >= 0; j--)
		memcpy(c->data + c->saved[j].at, &c->saved[j].value, c->saved[j].size);
	if (u->size && copy_to_user((void __user *)arg, c->data, u->size))
		rc = -EFAULT;
out:
	call_free(c);
	return rc;
}

/*
 * UVM needs a mapping's address on the host to equal its address here, and every process here shares QEMU's one address
 * space there, so no two can use the same address. CUDA always starts from the same one, but goes elsewhere when it is
 * taken. So each process after the first has the addresses below its own place held, before CUDA looks.
 */
#define UVM_FIRST_ADDRESS 0x200000000ull
#define UVM_ADDRESSES_EACH (64ull << 30)
#define UVM_MAX_PROCESSES 16

static struct {
	struct mm_struct *mm;
	int files;
} uvm_processes[UVM_MAX_PROCESSES];
static DEFINE_MUTEX(uvm_processes_lock);

int uvm_process_join(void)
{
	int place, free_place = -1;

	mutex_lock(&uvm_processes_lock);
	for (place = 0; place < UVM_MAX_PROCESSES; place++) {
		if (uvm_processes[place].mm == current->mm) {
			uvm_processes[place].files++;
			goto out;
		}
		if (free_place < 0 && !uvm_processes[place].mm)
			free_place = place;
	}
	place = free_place;
	if (place < 0) {
		place = -EBUSY;
		goto out;
	}
	uvm_processes[place].mm = current->mm;
	uvm_processes[place].files = 1;
	if (place) { /* an mm is only compared with, never followed: a file holds its place until it closes */
		unsigned long held = vm_mmap(NULL, UVM_FIRST_ADDRESS, place * UVM_ADDRESSES_EACH, PROT_NONE,
					     MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE | MAP_FIXED_NOREPLACE, 0);

		/* Something of the program's is there already. CUDA then looks elsewhere by itself. */
		if (IS_ERR_VALUE(held))
			pr_info("nvfwd: addresses below place %d not held\n", place);
	}
out:
	mutex_unlock(&uvm_processes_lock);
	return place;
}

void uvm_process_leave(int place)
{
	mutex_lock(&uvm_processes_lock);
	if (!--uvm_processes[place].files)
		uvm_processes[place].mm = NULL;
	mutex_unlock(&uvm_processes_lock);
}
