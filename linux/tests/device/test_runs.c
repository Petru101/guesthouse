// SPDX-License-Identifier: GPL-2.0-or-later
/* Run lists: fixed cases, then random lists against a plain model of the rules. */
#include "runs.h"

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PAGE RUNS_PAGE_BYTES
#define RAM (8ull << 30)
#define MAX_TEST_RUNS 64
#define FUZZ_ROUNDS 2000000

typedef struct {
    struct nvfwd_runs head;
    struct nvfwd_run run[MAX_TEST_RUNS];
} List;

static bool check(const List *list, uint64_t needed, uint64_t *total)
{
    return runs_check(&list->head, sizeof(*list), RAM, needed, total);
}

static void fixed_cases(void)
{
    List l = { .head.count = 2, .run = { { 0x1000, 0x2000 }, { 0x8000, 0x1000 } } };
    uint64_t total = 0;

    assert(check(&l, 3 * PAGE, &total) && total == 3 * PAGE);

    l.head.first_offset = 0x10;
    assert(!check(&l, 3 * PAGE, &total)); /* the driver would run off the end */
    assert(check(&l, 3 * PAGE - 0x10, &total));

    l.head.first_offset = 3 * PAGE;
    assert(!check(&l, 1, &total));
    l.head.first_offset = UINT64_MAX;
    assert(!check(&l, 1, &total));
    l.head.first_offset = 0;
    assert(!check(&l, 0, &total));

    l.run[1].len = UINT64_MAX - 0xfff; /* with the first run, the sum wraps to a small number */
    assert(!check(&l, 1, &total));
    l.run[0].len = 1ull << 63;
    l.run[1].len = 1ull << 63;
    assert(!check(&l, 1, &total));

    l.run[0] = (struct nvfwd_run){ UINT64_MAX & ~0xfffull, PAGE * 2 }; /* the address wraps */
    l.head.count = 1;
    assert(!check(&l, 1, &total));

    l.run[0] = (struct nvfwd_run){ 0x1000, 0 };
    assert(!check(&l, 1, &total));
    l.run[0] = (struct nvfwd_run){ 0x1001, PAGE };
    assert(!check(&l, 1, &total));
    l.run[0] = (struct nvfwd_run){ 0x1000, PAGE + 1 };
    assert(!check(&l, 1, &total));
    l.run[0] = (struct nvfwd_run){ 0x1000, RAM + PAGE };
    assert(!check(&l, 1, &total));

    l.run[0] = (struct nvfwd_run){ 0x1000, PAGE };
    l.head.count = 0;
    assert(!check(&l, 1, &total));
    l.head.count = MAX_TEST_RUNS + 1; /* more than the call holds: must not read past it */
    assert(!check(&l, 1, &total));
    l.head.count = UINT32_MAX;
    assert(!check(&l, 1, &total));
    l.head.count = 1;
    assert(!runs_check(&l.head, sizeof(l.head) - 1, RAM, 1, &total));
    assert(!runs_check(&l.head, sizeof(l.head), RAM, 1, &total));
}

static uint64_t random64(void)
{
    return (uint64_t)random() << 42 ^ (uint64_t)random() << 21 ^ (uint64_t)random();
}

/* Mostly sane values, with a few that sit on an edge. */
static uint64_t pick(uint64_t sane)
{
    static const uint64_t edges[] = { 0, 1, PAGE - 1, PAGE, RAM, RAM + PAGE, 1ull << 63,
                                      UINT64_MAX, UINT64_MAX - PAGE + 1 };

    switch (random() % 4) {
    case 0:
        return edges[random() % (sizeof(edges) / sizeof(edges[0]))];
    case 1:
        return random64();
    default:
        return sane;
    }
}

static bool model(const List *list, uint64_t room, uint64_t needed, uint64_t *total)
{
    unsigned __int128 sum = 0;
    uint32_t count = list->head.count;

    if (room < sizeof(list->head) || !count || count > RUNS_MAX_COUNT ||
        sizeof(list->head) + (unsigned __int128)count * sizeof(struct nvfwd_run) > room) {
        return false;
    }
    for (uint32_t i = 0; i < count; i++) {
        unsigned __int128 end = (unsigned __int128)list->run[i].gpa + list->run[i].len;

        if (!list->run[i].len || list->run[i].gpa % PAGE || list->run[i].len % PAGE ||
            end > (unsigned __int128)UINT64_MAX + 1) {
            return false;
        }
        sum += list->run[i].len;
    }
    if (sum > RAM || !needed || (unsigned __int128)list->head.first_offset + needed > sum) {
        return false;
    }
    *total = (uint64_t)sum;
    return true;
}

static void fuzz(void)
{
    uint64_t accepted = 0;

    for (int round = 0; round < FUZZ_ROUNDS; round++) {
        /* On the heap and exactly as long as claimed, so ASan sees any read past the list. */
        uint32_t room_runs = random() % (MAX_TEST_RUNS + 1);
        uint64_t room = sizeof(struct nvfwd_runs) + room_runs * sizeof(struct nvfwd_run);
        List *list = malloc(sizeof(*list));
        struct nvfwd_runs *exact = malloc(room);
        uint64_t got = 0, want = 0, needed;
        bool ok, expected;

        memset(list, 0, sizeof(*list));
        list->head.count = random() % 8 ? random() % (room_runs + 1) : (uint32_t)pick(room_runs);
        list->head.first_offset = pick(random() % PAGE);
        for (uint32_t i = 0; i < room_runs; i++) {
            list->run[i].gpa = pick((random() % 1024) * PAGE);
            list->run[i].len = pick((1 + random() % 64) * PAGE);
        }
        needed = pick(1 + random() % PAGE);
        memcpy(exact, list, room);
        ok = runs_check(exact, room, RAM, needed, &got);
        expected = model(list, room, needed, &want);
        assert(ok == expected);
        if (ok) {
            assert(got == want && got <= RAM && exact->first_offset < got);
            accepted++;
        }
        free(exact);
        free(list);
    }
    assert(accepted > 1000); /* a model that refused everything would prove nothing */
    printf("fuzz: %d lists, %llu accepted, all as the model said\n", FUZZ_ROUNDS,
           (unsigned long long)accepted);
}

int main(void)
{
    fixed_cases();
    fuzz();
    puts("ok");
    return 0;
}
