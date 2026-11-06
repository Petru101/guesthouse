// SPDX-License-Identifier: GPL-2.0-or-later
/* The list of live mappings. */
#include "maps.h"

#include <assert.h>
#include <stdio.h>

#define MIB (1ull << 20)

static void test_maps(void)
{
    Maps *m = maps_new(2, 4 * MIB);
    WindowMap a = { .window_offset = 0x1000, .len = 0x1000 };
    WindowMap u = { .window_offset = 0x2000, .len = 3 * MIB, .address = 0x700000, .uvm = true,
                   .region = &a };
    WindowMap too_big = { .window_offset = 0x9000, .len = 2 * MIB, .address = 0x900000, .uvm = true };
    WindowMap third = { .window_offset = 0xa000, .len = 0x1000 };
    WindowMap out, want;

    assert(maps_add(m, &a) && maps_add(m, &u));
    assert(!maps_add(m, &third));
    assert(maps_count(m) == 2);

    want = a;
    want.len = 0x2000;
    assert(!maps_take(m, &want, &out));
    want = a;
    want.window_offset = 0x1800;
    assert(!maps_take(m, &want, &out));
    want = u;
    want.address = 0x123000; /* the right mapping, and a different address to unmap */
    assert(!maps_take(m, &want, &out));
    want = u;
    want.uvm = false;
    assert(!maps_take(m, &want, &out));

    assert(maps_take(m, &u, &out) && out.region == &a);
    assert(!maps_take(m, &u, &out)); /* twice is once too many */
    assert(maps_add(m, &too_big) && maps_take(m, &too_big, &out));
    too_big.len = 5 * MIB;
    assert(!maps_add(m, &too_big));
}

int main(void)
{
    test_maps();
    puts("ok");
    return 0;
}
