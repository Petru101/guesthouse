// SPDX-License-Identifier: GPL-2.0-or-later
/* The call slots in BAR2, and the kick in BAR0 that runs one. */
#include <linux/io.h>
#include <linux/module.h>
#include <linux/spinlock.h>
#include "nvfwd_guest.h"

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Forwards NVIDIA driver calls to the host");

struct nvf_state nv;

int slot_get(void)
{
	int i;

	down(&nv.free);
	spin_lock(&nv.lock);
	i = find_first_zero_bit(&nv.used, NVFWD_SLOTS);
	__set_bit(i, &nv.used);
	spin_unlock(&nv.lock);
	return i;
}

void slot_put(int i)
{
	spin_lock(&nv.lock);
	__clear_bit(i, &nv.used);
	spin_unlock(&nv.lock);
	up(&nv.free);
}

struct nvfwd_req *slot_req(int i)
{
	return (struct nvfwd_req *)(nv.slots + (size_t)i * NVFWD_SLOT_SIZE);
}

void kick(int i)
{
	writel(i, nv.mmio);
}
