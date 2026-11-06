// SPDX-License-Identifier: GPL-2.0-or-later
/* Which clients a call names. */
#include "clients.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

#define MINE 0xc1d00001u
#define THEIRS 0xc1d00099u
#define RM_CMD(nr, size) (0xc0000000u | (uint32_t)(size) << 16 | 'F' << 8 | (nr))
#define PARAMS 64

static uint8_t data[16384];

static bool only_mine(void *opaque, uint32_t client)
{
    (void)opaque;
    return client == MINE;
}

static void put32(uint32_t at, uint32_t v)
{
    memcpy(data + at, &v, sizeof(v));
}

static bool known(uint32_t nr, uint32_t size, const struct nvfwd_fix *fix, uint32_t nfix)
{
    RmCall c;

    assert(rmargs_parse(&c, RM_CMD(nr, size), data, sizeof(data), fix, nfix));
    return clients_rm_known(&c, only_mine, NULL);
}

static void test_first_field(void)
{
    static const uint32_t escapes[] = { ESC_RM_FREE, ESC_RM_CONTROL, ESC_RM_ALLOC_MEMORY, ESC_RM_MAP_MEMORY,
                                        ESC_RM_UNMAP_MEMORY, ESC_RM_VID_HEAP_CONTROL, ESC_RM_SHARE, 0x57,
                                        0x58, 0x59, 0x5e, ESC_ALLOC_OS_EVENT, ESC_FREE_OS_EVENT };

    for (size_t i = 0; i < sizeof(escapes) / sizeof(escapes[0]); i++) {
        memset(data, 0, sizeof(data));
        put32(0, MINE);
        assert(known(escapes[i], 32, NULL, 0));
        put32(0, THEIRS);
        assert(!known(escapes[i], 32, NULL, 0));
        put32(0, 0);
        assert(!known(escapes[i], 32, NULL, 0));
    }
    /* A call that names no client at all. */
    memset(data, 0, sizeof(data));
    assert(known(ESC_REGISTER_FD, 4, NULL, 0));
}

static void test_dup(void)
{
    memset(data, 0, sizeof(data));
    put32(RM_DUP_CLIENT, MINE);
    put32(RM_DUP_SOURCE_CLIENT, MINE);
    assert(known(ESC_RM_DUP_OBJECT, RM_DUP_BYTES, NULL, 0));
    put32(RM_DUP_SOURCE_CLIENT, THEIRS);
    assert(!known(ESC_RM_DUP_OBJECT, RM_DUP_BYTES, NULL, 0));
}

static void test_alloc(void)
{
    struct nvfwd_fix params = { NVFWD_FIX_PTR, RM_ALLOC_PARAMS, PARAMS, 0 };

    memset(data, 0, sizeof(data));
    put32(RM_ALLOC_CLASS, 0x41); /* making a client names none */
    assert(known(ESC_RM_ALLOC, 48, NULL, 0));

    put32(RM_ALLOC_ROOT, MINE);
    put32(RM_ALLOC_CLASS, 0x80);
    assert(known(ESC_RM_ALLOC, 48, &params, 1));
    put32(PARAMS + 4, THEIRS); /* hClientShare */
    assert(!known(ESC_RM_ALLOC, 48, &params, 1));
    put32(PARAMS + 4, MINE);
    put32(PARAMS + 8, THEIRS); /* hTargetClient */
    assert(!known(ESC_RM_ALLOC, 48, &params, 1));
    put32(PARAMS + 8, 0);
    assert(known(ESC_RM_ALLOC, 48, &params, 1));

    memset(data + PARAMS, 0, 64);
    put32(RM_ALLOC_CLASS, 0x83de);
    put32(PARAMS + 4, THEIRS); /* hAppClient */
    assert(!known(ESC_RM_ALLOC, 48, &params, 1));

    memset(data + PARAMS, 0, 64);
    put32(RM_ALLOC_CLASS, CLASS_EVENT);
    put32(PARAMS, THEIRS); /* hParentClient */
    assert(!known(ESC_RM_ALLOC, 48, &params, 1));

    put32(RM_ALLOC_ROOT, THEIRS);
    put32(RM_ALLOC_CLASS, 0x80);
    assert(!known(ESC_RM_ALLOC, 48, NULL, 0));
}

static void test_idle_and_control(void)
{
    struct nvfwd_fix list = { NVFWD_FIX_PTR, RM_IDLE_CLIENTS, PARAMS, 0 };
    struct nvfwd_fix params = { NVFWD_FIX_PTR, RM_CONTROL_PARAMS, PARAMS, 0 };

    memset(data, 0, sizeof(data));
    put32(RM_IDLE_COUNT, 2);
    put32(PARAMS, MINE);
    put32(PARAMS + 4, MINE);
    assert(known(ESC_RM_IDLE_CHANNELS, 56, &list, 1));
    put32(PARAMS + 4, THEIRS); /* the last of the list */
    assert(!known(ESC_RM_IDLE_CHANNELS, 56, &list, 1));

    memset(data, 0, sizeof(data));
    put32(RM_CONTROL_CLIENT, MINE);
    put32(RM_CONTROL_CMD, 0x20801208);
    put32(RM_CONTROL_PARAMS_SIZE, 24);
    put32(PARAMS, THEIRS);
    assert(!known(ESC_RM_CONTROL, 32, &params, 1));
    put32(PARAMS, MINE);
    assert(known(ESC_RM_CONTROL, 32, &params, 1));
}

static void test_uvm(void)
{
    memset(data, 0, sizeof(data));
    assert(clients_uvm_known(37, data, 40, only_mine, NULL));
    put32(28, THEIRS);
    assert(!clients_uvm_known(37, data, 40, only_mine, NULL));
    put32(28, MINE);
    assert(clients_uvm_known(37, data, 40, only_mine, NULL));
    put32(9252, THEIRS);
    assert(!clients_uvm_known(33, data, 9264, only_mine, NULL));
    assert(clients_uvm_known(39, data, 8, only_mine, NULL)); /* names none */
}

int main(void)
{
    test_first_field();
    test_dup();
    test_alloc();
    test_idle_and_control();
    test_uvm();
    puts("ok");
    return 0;
}
