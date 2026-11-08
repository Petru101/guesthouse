// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * The one card a guest gets, on a pretend host with two: the 4080 and 1660 SUPER of a tester's PC,
 * where the RM's device instances run the other way from the device files.
 */
#include "card.h"
#include "rm_layout.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

#define ESC_CARD_INFO 0xc8
#define ESC_ATTACH_GPUS_TO_FD 0xd4
#define CLASS_DEVICE 0x80
#define NO_GPU 0xffffffffu
#define ALL_GPUS 0xffffu
#define CARD_INFO_BYTES 72

typedef struct FakeGpu {
    uint32_t id, domain, bus, slot, instance;
} FakeGpu;

typedef struct FakeRm {
    const FakeGpu *gpus;
    uint32_t count;
} FakeRm;

static const FakeGpu tester[] = {
    { .id = 0x100, .domain = 0, .bus = 1, .slot = 0, .instance = 1 },
    { .id = 0x500, .domain = 0, .bus = 5, .slot = 0, .instance = 0 },
};
static const FakeGpu only[] = { { .id = 0x200, .domain = 0, .bus = 2, .slot = 0, .instance = 0 } };

static uint32_t get32(const uint8_t *at)
{
    uint32_t v;
    memcpy(&v, at, 4);
    return v;
}

static void put32(uint8_t *at, uint32_t v)
{
    memcpy(at, &v, 4);
}

static int64_t fake_control(void *opaque, uint32_t cmd, void *params, uint32_t size)
{
    const FakeRm *rm = opaque;
    uint8_t *p = params;

    if (cmd == 0x201) {
        assert(size == 128);
        memset(p, 0xff, size);
        for (uint32_t i = 0; i < rm->count; i++) {
            put32(p + i * 4, rm->gpus[i].id);
        }
        return STATUS_OK;
    }
    for (uint32_t i = 0; i < rm->count; i++) {
        const FakeGpu *g = &rm->gpus[i];
        if (get32(p) != g->id) {
            continue;
        }
        if (cmd == 0x21b) {
            uint16_t bus = g->bus, slot = g->slot;
            assert(size == 12);
            put32(p + 4, g->domain);
            memcpy(p + 8, &bus, 2);
            memcpy(p + 10, &slot, 2);
            return STATUS_OK;
        }
        if (cmd == 0x205) {
            assert(size == 32);
            put32(p + 8, g->instance);
            return STATUS_OK;
        }
    }
    return STATUS_INVALID_ARGUMENT;
}

static const char *find(const FakeGpu *gpus, uint32_t count, const char *text, NvfwdCard *card)
{
    FakeRm rm = { gpus, count };
    CardAddress address;

    if (!text) {
        return card_find(fake_control, &rm, NULL, card);
    }
    assert(card_parse_address(text, &address));
    return card_find(fake_control, &rm, &address, card);
}

static void test_addresses(void)
{
    CardAddress a;
    char text[CARD_ADDRESS_TEXT];

    assert(card_parse_address("0000:02:00.0", &a) && a.domain == 0 && a.bus == 2 && a.device == 0);
    assert(card_parse_address("00000001:65:1f.0", &a) && a.domain == 1 && a.bus == 0x65 && a.device == 0x1f);
    card_print_address(&a, text, sizeof(text));
    assert(!strcmp(text, "0001:65:1f.0"));
    assert(!card_parse_address("02:00.0", &a));
    assert(!card_parse_address("0000:02:00.0 ", &a));
    assert(!card_parse_address("0000:02:00.8", &a));
    assert(!card_parse_address("0000:100:00.0", &a));
    assert(!card_parse_address("nvidia0", &a));
    assert(!card_parse_address(NULL, &a));
}

static void test_find(void)
{
    NvfwdCard card;

    assert(!find(tester, 2, "0000:01:00.0", &card) && card.gpu_id == 0x100 && card.instance == 1);
    assert(!find(tester, 2, "0000:05:00.0", &card) && card.gpu_id == 0x500 && card.instance == 0);
    assert(find(tester, 2, "0000:03:00.0", &card));
    assert(find(tester, 2, "0000:01:00.1", &card)); /* a GPU is function 0 */
    assert(find(tester, 2, NULL, &card));          /* two cards, and nothing says which */
    /* One card: the same as before there was a choice. */
    assert(!find(only, 1, NULL, &card) && card.gpu_id == 0x200 && card.instance == 0);
    assert(!find(only, 1, "0000:02:00.0", &card) && card.instance == 0);
    assert(find(only, 0, NULL, &card));
}

/* An RM call's arguments, with its parameters pointed at. */
typedef struct Call {
    uint8_t arg[48];
    uint8_t params[2048];
} Call;

static Call *control(uint32_t cmd, uint32_t params_size)
{
    static Call c;
    uint64_t pointer = (uintptr_t)c.params;

    memset(&c, 0, sizeof(c));
    put32(c.arg + RM_CONTROL_CMD, cmd);
    memcpy(c.arg + RM_CONTROL_PARAMS, &pointer, 8);
    put32(c.arg + RM_CONTROL_PARAMS_SIZE, params_size);
    return &c;
}

static Call *device_alloc(uint32_t size, uint32_t device_id, bool with_params)
{
    static Call c;
    uint64_t pointer = with_params ? (uintptr_t)c.params : 0;

    memset(&c, 0, sizeof(c));
    put32(c.arg + RM_ALLOC_CLASS, CLASS_DEVICE);
    memcpy(c.arg + RM_ALLOC_PARAMS, &pointer, 8);
    put32(c.arg + (size == RM_ALLOC_SHORT_BYTES ? RM_ALLOC_SHORT_PARAMS_SIZE : RM_ALLOC_LONG_PARAMS_SIZE), 56);
    put32(c.params, device_id);
    return &c;
}

static void test_devices(void)
{
    const NvfwdCard card = { .gpu_id = 0x100, .instance = 1 };
    const uint32_t sizes[] = { RM_ALLOC_SHORT_BYTES, RM_ALLOC_LONG_BYTES };

    for (int i = 0; i < 2; i++) {
        Call *c = device_alloc(sizes[i], 0, true);
        assert(card_allows(&card, ESC_RM_ALLOC, c->arg, sizes[i]) && get32(c->params) == 1);
        card_fix_answer(&card, ESC_RM_ALLOC, c->arg, sizes[i]);
        assert(get32(c->params) == 0);
        /* NVIDIA's driver leaves the size at 0, and the RM goes by the class's own. */
        c = device_alloc(sizes[i], 0, true);
        put32(c->arg + (sizes[i] == RM_ALLOC_SHORT_BYTES ? RM_ALLOC_SHORT_PARAMS_SIZE : RM_ALLOC_LONG_PARAMS_SIZE), 0);
        assert(card_allows(&card, ESC_RM_ALLOC, c->arg, sizes[i]) && get32(c->params) == 1);
        card_fix_answer(&card, ESC_RM_ALLOC, c->arg, sizes[i]);
        assert(get32(c->params) == 0);
        /* The guest has one device, and no parameters would be the RM's first. */
        assert(!card_allows(&card, ESC_RM_ALLOC, device_alloc(sizes[i], 1, true)->arg, sizes[i]));
        assert(!card_allows(&card, ESC_RM_ALLOC, device_alloc(sizes[i], 0, false)->arg, sizes[i]));
    }
    {
        Call *c = device_alloc(RM_ALLOC_SHORT_BYTES, 7, true);
        put32(c->arg + RM_ALLOC_CLASS, 0x2080); /* a subdevice's number is within its device */
        assert(card_allows(&card, ESC_RM_ALLOC, c->arg, RM_ALLOC_SHORT_BYTES) && get32(c->params) == 7);
    }
    {
        const NvfwdCard one = { .gpu_id = 0x200, .instance = 0 };
        Call *c = device_alloc(RM_ALLOC_SHORT_BYTES, 0, true);
        assert(card_allows(&one, ESC_RM_ALLOC, c->arg, RM_ALLOC_SHORT_BYTES) && get32(c->params) == 0);
    }
}

static void test_gpu_named(void)
{
    const NvfwdCard card = { .gpu_id = 0x100, .instance = 1 };
    const uint32_t named[] = { 0x202, 0x205, 0x21b, 0x275, 0x279, 0x289, 0x28a };
    Call *c;

    for (size_t i = 0; i < sizeof(named) / sizeof(named[0]); i++) {
        c = control(named[i], 32);
        put32(c->params, 0x100);
        assert(card_allows(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES));
        put32(c->params, 0x500);
        assert(!card_allows(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES));
        c = control(named[i], 0);
        assert(!card_allows(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES));
    }
    c = control(0x205, 32);
    put32(c->params, 0x100);
    put32(c->params + 8, 1);
    card_fix_answer(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES);
    assert(get32(c->params + 8) == 0);

    /* Attaching every GPU is attaching the card. */
    c = control(0x215, 132);
    memset(c->params, 0xff, 128);
    put32(c->params, ALL_GPUS);
    assert(card_allows(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES));
    assert(get32(c->params) == 0x100 && get32(c->params + 4) == NO_GPU);
    put32(c->params + 4, 0x500);
    assert(!card_allows(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES));

    c = control(0x13a, 1000);
    put32(c->params, 1);
    put32(c->params + 4, 1);
    put32(c->params + 8, 0x100);
    put32(c->params + 40, 0x100);
    assert(card_allows(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES));
    put32(c->params + 40, 0x500);
    assert(!card_allows(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES));
    put32(c->params + 4, 9);
    assert(!card_allows(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES));

    {
        /* The kernel passes over a 0 and takes the rest, so a 0 is no GPU and an end mark is one. */
        uint32_t fd_gpus[4] = { 0x100, 0, 0x500, NO_GPU };
        assert(card_allows(&card, ESC_ATTACH_GPUS_TO_FD, (uint8_t *)fd_gpus, 8));
        assert(!card_allows(&card, ESC_ATTACH_GPUS_TO_FD, (uint8_t *)fd_gpus, 12));
        assert(!card_allows(&card, ESC_ATTACH_GPUS_TO_FD, (uint8_t *)&fd_gpus[3], 4));
    }
}

static void test_lists(void)
{
    const NvfwdCard card = { .gpu_id = 0x100, .instance = 1 };
    Call *c = control(0x201, 128);

    memset(c->params, 0xff, 128);
    put32(c->params, 0x500);
    put32(c->params + 4, 0x100);
    card_fix_answer(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES);
    assert(get32(c->params) == 0x100 && get32(c->params + 4) == NO_GPU && get32(c->params + 124) == NO_GPU);

    c = control(0x214, 384);
    memset(c->params, 0xff, 256);
    put32(c->params, 0x500);
    put32(c->params + 4, 0x100);
    put32(c->params + 128, 0x700); /* excluded */
    put32(c->params + 256 + 4, 1);
    card_fix_answer(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES);
    assert(get32(c->params) == 0x100 && get32(c->params + 4) == NO_GPU);
    assert(get32(c->params + 128) == NO_GPU && get32(c->params + 256) == 1 && get32(c->params + 260) == 0);

    c = control(0x204, 4);
    put32(c->params, 3);
    card_fix_answer(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES);
    assert(get32(c->params) == 1);
    put32(c->params, 1); /* only the other card's device */
    card_fix_answer(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES);
    assert(get32(c->params) == 0);

    c = control(0x288, 4 + 12 * 256);
    put32(c->params, 3);
    put32(c->params + 4, 0x500);
    put32(c->params + 16, 0x100);
    put32(c->params + 20, 7);
    put32(c->params + 28, 0x500);
    card_fix_answer(&card, ESC_RM_CONTROL, c->arg, RM_CONTROL_BYTES);
    assert(get32(c->params) == 1 && get32(c->params + 4) == 0x100 && get32(c->params + 8) == 7);
    assert(get32(c->params + 16) == 0 && get32(c->params + 28) == 0);
}

static void test_card_info(void)
{
    const NvfwdCard card = { .gpu_id = 0x100, .instance = 1 };
    uint8_t info[4 * CARD_INFO_BYTES] = { 0 };

    info[0] = info[CARD_INFO_BYTES] = 1;
    put32(info + 16, 0x500);
    put32(info + 56, 0);
    put32(info + CARD_INFO_BYTES + 16, 0x100);
    put32(info + CARD_INFO_BYTES + 56, 1);
    card_fix_answer(&card, ESC_CARD_INFO, info, sizeof(info));
    assert(info[0] == 1 && get32(info + 16) == 0x100 && get32(info + 56) == 0);
    assert(info[CARD_INFO_BYTES] == 0 && get32(info + CARD_INFO_BYTES + 16) == 0);
}

/* A GPU control added to the allow list has to be looked at here first. */
static void test_allow_list(void)
{
    FILE *f = fopen(ALLOW_LIST, "r");
    char kind[16], line[512];
    unsigned cmd, seen = 0;

    assert(f);
    while (fgets(line, sizeof(line), f)) {
        if (sscanf(line, "%15s %x", kind, &cmd) == 2 && !strcmp(kind, "ctrl")) {
            if (!card_knows_control(cmd)) {
                fprintf(stderr, "card.c doesn't know ctrl %#x\n", cmd);
                assert(0);
            }
            seen += (cmd & 0xffffff00u) == 0x200;
        }
    }
    fclose(f);
    assert(seen > 10);
    assert(!card_knows_control(0x2ff));
}

int main(void)
{
    test_addresses();
    test_find();
    test_devices();
    test_gpu_named();
    test_lists();
    test_card_info();
    test_allow_list();
    printf("card ok\n");
    return 0;
}
