// SPDX-License-Identifier: GPL-2.0-or-later
/* The call rate: the burst, the steady rate, and the burst coming back. */
#include "callrate.h"

#include <assert.h>
#include <stdio.h>

#define SECOND 1000000000ull

static void test_rate(void)
{
    CallRate r;
    uint64_t now = SECOND, waited = 0;
    int calls = 0;

    callrate_init(&r, 0, 0);
    assert(callrate_wait_ns(&r, now) == 0);

    callrate_init(&r, 1000, 100);
    for (int i = 0; i < 100; i++) {
        assert(callrate_wait_ns(&r, now) == 0); /* the burst goes straight through */
    }
    assert(callrate_wait_ns(&r, now) > 0);

    /* A caller flat out, sleeping as told, for ten seconds, gets the rate and no more. */
    callrate_init(&r, 1000, 100);
    for (now = SECOND; now < 11 * SECOND; calls++) {
        uint64_t wait = callrate_wait_ns(&r, now);

        waited += wait;
        now += wait + 1000;
    }
    assert(calls >= 10000 && calls <= 10100 + 10);
    assert(waited > 9 * SECOND);

    /* After a quiet spell the burst is back, whole. */
    now += 100 * SECOND;
    for (int i = 0; i < 100; i++) {
        assert(callrate_wait_ns(&r, now) == 0);
    }
}

int main(void)
{
    test_rate();
    puts("ok");
    return 0;
}
