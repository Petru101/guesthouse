// SPDX-License-Identifier: GPL-2.0-or-later
/* The object tree and its charges. */
#include "objects.h"

#include <assert.h>
#include <stdio.h>

#define MIB (1ull << 20)

static int gone_calls;
static uint64_t gone_bytes;

static void gone(void *opaque, void *range, uint64_t bytes)
{
    assert(opaque == &gone_calls && range);
    gone_calls++;
    gone_bytes += bytes;
}

static void add(Objects *o, uint32_t client, uint32_t parent, uint32_t object, MemoryKind kind,
                uint64_t bytes, uint32_t file)
{
    ObjectClaim claim = { kind, bytes };
    ObjectInfo info = { .client = client, .parent = parent, .object = object, .file = file };

    assert(objects_claim(o, &claim));
    objects_add(o, &claim, bytes, &info);
}

static void test_objects(void)
{
    ObjectLimits limits = { .max_objects = 8, .max_bytes = { 0, 100 * MIB, 10 * MIB } };
    Objects *o = objects_new(&limits, gone, &gone_calls);
    ObjectClaim big = { MEMORY_VIDEO, 60 * MIB }, huge = { MEMORY_VIDEO, UINT64_MAX };

    add(o, 1, 1, 1, MEMORY_NONE, 0, 7); /* client */
    add(o, 1, 1, 2, MEMORY_NONE, 0, 7); /* device */
    add(o, 1, 2, 3, MEMORY_VIDEO, 40 * MIB, 7);
    assert(objects_has_client(o, 1) && !objects_has_client(o, 2) && !objects_has_client(o, 3));
    assert(objects_bytes(o, MEMORY_VIDEO) == 40 * MIB);

    /* Two claims in flight cannot both fit, and a size that would wrap fits nothing. */
    assert(objects_claim(o, &big));
    assert(!objects_claim(o, &big));
    assert(!objects_claim(o, &huge));
    objects_drop_claim(o, &big);
    assert(objects_bytes(o, MEMORY_VIDEO) == 40 * MIB);

    /* The driver's size wins over the size asked for. */
    {
        ObjectClaim c = { MEMORY_VIDEO, MIB };
        ObjectInfo info = { .client = 1, .parent = 2, .object = 4, .file = 7 };

        assert(objects_claim(o, &c));
        objects_add(o, &c, 2 * MIB, &info);
        assert(objects_bytes(o, MEMORY_VIDEO) == 42 * MIB);
    }

    /* Freeing the device takes the memory under it, with no free for each. */
    objects_remove(o, 1, 2);
    assert(objects_bytes(o, MEMORY_VIDEO) == 0 && objects_count(o) == 1);

    /* Host ranges go with their object, by whichever road. */
    {
        ObjectClaim c = { MEMORY_NONE, 0 };
        ObjectInfo info = { .client = 1, .parent = 1, .object = 5, .file = 7,
                            .host_range = &limits, .host_bytes = 4096 };

        assert(objects_claim(o, &c));
        objects_add(o, &c, 0, &info);
    }
    objects_file_closed(o, 9); /* another file: nothing */
    assert(objects_count(o) == 2 && gone_calls == 0);
    objects_file_closed(o, 7);
    assert(objects_count(o) == 0 && gone_calls == 1 && gone_bytes == 4096);
    assert(!objects_has_client(o, 1));

    /* The count is a hard stop, and space comes back. */
    add(o, 2, 2, 2, MEMORY_NONE, 0, 1);
    for (uint32_t i = 10; i < 17; i++) {
        add(o, 2, 2, i, MEMORY_NONE, 0, 1);
    }
    {
        ObjectClaim c = { MEMORY_NONE, 0 };

        assert(!objects_claim(o, &c));
        objects_remove(o, 2, 10);
        assert(objects_claim(o, &c));
        objects_drop_claim(o, &c);
    }

    /* A client gone takes all, an object of an unknown parent included. */
    add(o, 2, 99, 50, MEMORY_SYSTEM, MIB, 1);
    objects_remove(o, 2, 2);
    assert(objects_count(o) == 0 && objects_bytes(o, MEMORY_SYSTEM) == 0);

    /* Local objects are told apart, per client. */
    {
        ObjectClaim c = { MEMORY_NONE, 0 };
        ObjectInfo info = { .client = 3, .parent = 3, .object = 6, .local = true };

        assert(objects_claim(o, &c));
        objects_add(o, &c, 0, &info);
        assert(objects_is_local(o, 3, 6) && !objects_is_local(o, 4, 6));
    }

    /* A handle made again replaces the old object and its charge. */
    add(o, 5, 5, 5, MEMORY_NONE, 0, 1);
    add(o, 5, 5, 6, MEMORY_VIDEO, MIB, 1);
    add(o, 5, 5, 6, MEMORY_VIDEO, 2 * MIB, 1);
    assert(objects_bytes(o, MEMORY_VIDEO) == 2 * MIB);
}

int main(void)
{
    test_objects();
    puts("ok");
    return 0;
}
