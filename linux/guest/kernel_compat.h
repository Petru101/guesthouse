/* SPDX-License-Identifier: GPL-2.0-or-later */
/* What a kernel older than the newest lacks, for a guest on a long-term kernel such as Debian 13's 6.12.
 * The guest module and the stand-in both include this. */
#ifndef NVFWD_KERNEL_COMPAT_H
#define NVFWD_KERNEL_COMPAT_H

#include <linux/hrtimer.h>
#include <linux/version.h>

/* hrtimer_setup() came in 6.13. */
#if LINUX_VERSION_CODE < KERNEL_VERSION(6, 13, 0)
static inline void hrtimer_setup(struct hrtimer *timer, enum hrtimer_restart (*function)(struct hrtimer *),
				 clockid_t clock, enum hrtimer_mode mode)
{
	hrtimer_init(timer, clock, mode);
	timer->function = function;
}
#endif

#endif
