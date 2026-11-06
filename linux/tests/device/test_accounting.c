// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * What a call is charged. The numbers here are from NVIDIA's source, not from accounting.c: the flag
 * and field values are written out so that a mistake there cannot also be a mistake here.
 */
#include "accounting.h"
#include "rm_layout.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

#define MIB (1ull << 20)
#define CLIENT 0xc1d00001u
#define ATTR_LOCATION_PCI (1u << 25)
#define OS02_ALLOC_NONE (1u << 16)

static uint8_t args[256], params[256];

static void put32(uint8_t *to, uint32_t at, uint32_t v)
{
    memcpy(to + at, &v, sizeof(v));
}

static void put64(uint8_t *to, uint32_t at, uint64_t v)
{
    memcpy(to + at, &v, sizeof(v));
}

static void gone(void *opaque, void *range, uint64_t bytes)
{
    (void)opaque, (void)range, (void)bytes;
}

static Objects *new_objects(void)
{
    ObjectLimits limits = { .max_objects = 100, .max_bytes = { 0, 100 * MIB, 50 * MIB } };

    return objects_new(&limits, gone, NULL);
}

static uint32_t run(Objects *o, uint32_t nr, uint32_t size, uint32_t object)
{
    AccountCall call = { .nr = nr, .arg = args, .size = size, .file = 1, .data = params,
                         .data_size = sizeof(params) };
    AccountHold hold;
    uint32_t refused = account_before(o, &call, &hold);

    if (!refused) {
        put32(args, nr == ESC_RM_VID_HEAP_CONTROL ? RM_VID_HEAP_MEMORY : RM_ALLOC_OBJECT, object);
        put32(args, account_status_at(nr, size), STATUS_OK);
        account_after(o, &call, &hold);
    }
    return refused;
}

static void heap_call(uint32_t flags, uint32_t attr, uint64_t size)
{
    memset(args, 0, sizeof(args));
    put32(args, 0, CLIENT);       /* hRoot */
    put32(args, 8, 2);            /* function: NVOS32_FUNCTION_ALLOC_SIZE */
    put32(args, 52, flags);       /* data.AllocSize.flags */
    put32(args, 56, attr);        /* data.AllocSize.attr */
    put64(args, 88, size);        /* data.AllocSize.size */
}

static void test_heap(void)
{
    Objects *o = new_objects();

    heap_call(0x00080000, 0, 80 * MIB); /* NVOS32_ALLOC_FLAGS_VIRTUAL: addresses only */
    assert(!run(o, ESC_RM_VID_HEAP_CONTROL, RM_VID_HEAP_BYTES, 1));
    assert(objects_bytes(o, MEMORY_VIDEO) == 0 && objects_bytes(o, MEMORY_SYSTEM) == 0);

    heap_call(0x00080000, 0, 1ull << 40); /* however much of them */
    assert(!run(o, ESC_RM_VID_HEAP_CONTROL, RM_VID_HEAP_BYTES, 2));

    heap_call(0, 0, 80 * MIB); /* location VIDMEM */
    assert(!run(o, ESC_RM_VID_HEAP_CONTROL, RM_VID_HEAP_BYTES, 3));
    assert(objects_bytes(o, MEMORY_VIDEO) == 80 * MIB);

    heap_call(0, 0, 80 * MIB); /* a second one passes the cap */
    assert(run(o, ESC_RM_VID_HEAP_CONTROL, RM_VID_HEAP_BYTES, 4) == STATUS_NO_MEMORY);

    heap_call(0, ATTR_LOCATION_PCI, 40 * MIB); /* system memory has a cap of its own */
    assert(!run(o, ESC_RM_VID_HEAP_CONTROL, RM_VID_HEAP_BYTES, 5));
    assert(objects_bytes(o, MEMORY_SYSTEM) == 40 * MIB && objects_bytes(o, MEMORY_VIDEO) == 80 * MIB);

    heap_call(0, 0, 0); /* a charge that cannot be known */
    assert(run(o, ESC_RM_VID_HEAP_CONTROL, RM_VID_HEAP_BYTES, 6) == STATUS_INVALID_ARGUMENT);
}

static void test_alloc_memory(void)
{
    Objects *o = new_objects();

    /* Video memory through this call is a dummy handle for a user. */
    memset(args, 0, sizeof(args));
    put32(args, RM_MEMORY_CLASS, CLASS_MEMORY_LOCAL);
    put32(args, 16, OS02_ALLOC_NONE);
    put64(args, RM_MEMORY_LIMIT, 0x7ffffffffull);
    assert(!run(o, ESC_RM_ALLOC_MEMORY, RM_MEMORY_BYTES, 1));
    assert(objects_bytes(o, MEMORY_VIDEO) == 0);

    memset(args, 0, sizeof(args));
    put32(args, RM_MEMORY_CLASS, CLASS_MEMORY_SYSTEM);
    put64(args, RM_MEMORY_LIMIT, MIB - 1);
    assert(!run(o, ESC_RM_ALLOC_MEMORY, RM_MEMORY_BYTES, 2));
    assert(objects_bytes(o, MEMORY_SYSTEM) == MIB);
}

static void test_alloc(void)
{
    Objects *o = new_objects();

    memset(args, 0, sizeof(args));
    memset(params, 0, sizeof(params));
    put32(args, RM_ALLOC_CLASS, CLASS_MEMORY_LOCAL);
    put64(args, RM_ALLOC_PARAMS, (uint64_t)(uintptr_t)params);
    put64(params, 64, 10 * MIB); /* NV_MEMORY_ALLOCATION_PARAMS.size */
    assert(!run(o, ESC_RM_ALLOC, RM_ALLOC_LONG_BYTES, 1));
    assert(objects_bytes(o, MEMORY_VIDEO) == 10 * MIB);

    put64(args, RM_ALLOC_PARAMS, 0x1000); /* parameters that are not in the call */
    assert(run(o, ESC_RM_ALLOC, RM_ALLOC_LONG_BYTES, 2) == STATUS_INVALID_ARGUMENT);
}

int main(void)
{
    test_heap();
    test_alloc_memory();
    test_alloc();
    puts("ok");
    return 0;
}
