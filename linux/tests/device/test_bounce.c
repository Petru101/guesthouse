// SPDX-License-Identifier: GPL-2.0-or-later
/* The private copy of a call. */
#include "bounce.h"

#include <assert.h>
#include <errno.h>
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static sigjmp_buf caught;

static void on_fault(int sig)
{
    (void)sig;
    siglongjmp(caught, 1);
}

static bool faults(volatile uint8_t *at)
{
    signal(SIGSEGV, on_fault);
    if (sigsetjmp(caught, 1)) {
        return true;
    }
    (void)*at;
    return false;
}

static void test_bounce(void)
{
    static uint8_t slot[NVFWD_SLOT_SIZE];
    struct nvfwd_req *req = (struct nvfwd_req *)slot;
    uint8_t *guest_data = slot + NVFWD_HDR_SIZE;
    uint64_t host_value = 0x7f0012345678, guest_value = 0x1111222233334444;
    Bounce *b = malloc(sizeof(*b));

    assert(bounce_init(b));
    assert(!faults(b->data) && !faults(b->data + NVFWD_DATA_SIZE - 1));
    assert(faults(b->data - 1) && faults(b->data + NVFWD_DATA_SIZE));

    assert(bounce_enter(b) && !bounce_enter(b));
    bounce_leave(b);
    assert(bounce_enter(b));

    req->op = NVFWD_OP_IOCTL;
    req->nfix = 1;
    req->len = 64;
    req->fix[0] = (struct nvfwd_fix){ NVFWD_FIX_PTR, 8, 16, 0 };
    memcpy(guest_data + 8, &guest_value, 8);
    bounce_take_request(b, slot);
    assert(bounce_take_call(b, slot));

    /* A change in the slot after the copy reaches nothing the device will look at. */
    req->cmd = 0xbad;
    req->len = 0;
    req->fix[0].target = 999;
    memset(guest_data, 0xff, 64);
    assert(b->req.cmd != 0xbad && b->req.len == 64 && b->req.fix[0].target == 16);
    assert(b->data[0] == 0);

    /* Host values go in, and none come home, through the data or anywhere else. */
    bounce_patch(b, 8, &host_value, 8);
    assert(!memcmp(b->data + 8, &host_value, 8));
    b->data[0] = 0x42; /* an answer from the driver */
    b->req.ret = 0;
    b->req.a3 = 77;
    bounce_give_call(b, slot);
    assert(guest_data[0] == 0x42 && !memcmp(guest_data + 8, &guest_value, 8));
    assert(req->ret == 0 && req->a3 == 77);
    assert(!memmem(slot, sizeof(slot), &host_value, 8));
    assert(!memmem(b->data, 64, &host_value, 8));

    /* A failed call hands back no data. */
    memset(guest_data, 0xee, 64);
    b->req.ret = -EINVAL;
    bounce_give_call(b, slot);
    assert(guest_data[0] == 0xee && req->ret == -EINVAL);

    /* Counts past the limits take nothing. */
    req->nfix = NVFWD_MAX_FIX + 1;
    bounce_take_request(b, slot);
    assert(!bounce_take_call(b, slot));
    req->nfix = 0;
    req->len = NVFWD_DATA_SIZE + 1;
    bounce_take_request(b, slot);
    assert(!bounce_take_call(b, slot));
    req->len = NVFWD_DATA_SIZE;
    bounce_take_request(b, slot);
    assert(bounce_take_call(b, slot));
    free(b);
}

int main(void)
{
    test_bounce();
    puts("ok");
    return 0;
}
