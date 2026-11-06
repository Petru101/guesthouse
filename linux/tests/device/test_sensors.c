// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * A guest's sensor objects. The rule under test: a guest's calls work whether or not there's a daemon,
 * and with no daemon the page it maps has no readings. NVIDIA's user-mode driver won't start without
 * the object, so a failure here would cost a guest CUDA and Vulkan and not just sensors.
 * The numbers are written out, as in test_accounting, so a mistake in sensors.c can't hide here.
 */
#include "sensors.h"
#include "rusd.h"

#include <assert.h>
#include <glib.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define ESC_RM_FREE 0x29
#define ESC_RM_CONTROL 0x2a
#define ESC_RM_ALLOC 0x2b
#define ESC_RM_MAP_MEMORY 0x4e
#define ESC_RM_UNMAP_MEMORY 0x4f

#define CLASS_SENSOR_PAGE 0xde
#define CTRL_POLL 0xde0001
#define STATUS_OK 0
#define STATUS_NOT_SUPPORTED 0x56

#define CLIENT 0xc1d00001u
#define OBJECT 0xcafe0001u
#define FILE_ID 5
#define ALLOC_BYTES 48
#define CONTROL_BYTES 32
#define MAP_BYTES 56
#define UNMAP_BYTES 32
#define FREE_BYTES 16

#define POLL_TEMPERATURE 0x10
#define POLL_PROCESSES 0x80 /* never for a guest: see rusd.h */
#define READING_AT 0x908
#define READING 0x5a
#define MOST_SECONDS_FOR_STUCK_DAEMON 10
#define DAEMON_EXIT_MICROSECONDS 300000

static const char *fake_daemon;

static uint8_t args[64];
static uint64_t mask_param;

static void put32(uint32_t at, uint32_t v)
{
    memcpy(args + at, &v, sizeof(v));
}

static void put64(uint32_t at, uint64_t v)
{
    memcpy(args + at, &v, sizeof(v));
}

static uint32_t get32(uint32_t at)
{
    uint32_t v;

    memcpy(&v, args + at, sizeof(v));
    return v;
}

static bool answer(Sensors *s, uint32_t nr, uint32_t size)
{
    SensorCall call = {
        .nr = nr, .arg = args, .size = size, .file = FILE_ID, .map_file = FILE_ID,
        .data = (uint8_t *)&mask_param, .data_size = sizeof(mask_param),
    };

    return sensors_answer(s, &call);
}

static uint32_t alloc(Sensors *s, uint64_t mask)
{
    memset(args, 0, sizeof(args));
    mask_param = mask;
    put32(0, CLIENT);
    put32(8, OBJECT);
    put32(12, CLASS_SENSOR_PAGE);
    put64(16, (uint64_t)(uintptr_t)&mask_param);
    put32(32, sizeof(mask_param));
    assert(answer(s, ESC_RM_ALLOC, ALLOC_BYTES));
    return get32(40);
}

static uint32_t poll_for(Sensors *s, uint64_t mask)
{
    memset(args, 0, sizeof(args));
    mask_param = mask;
    put32(0, CLIENT);
    put32(4, OBJECT);
    put32(8, CTRL_POLL);
    put64(16, (uint64_t)(uintptr_t)&mask_param);
    put32(24, sizeof(mask_param));
    assert(answer(s, ESC_RM_CONTROL, CONTROL_BYTES));
    return get32(28);
}

static uint32_t map(Sensors *s)
{
    memset(args, 0, sizeof(args));
    put32(0, CLIENT);
    put32(8, OBJECT);
    put64(24, RUSD_PAGE_BYTES);
    assert(answer(s, ESC_RM_MAP_MEMORY, MAP_BYTES));
    return get32(40);
}

static uint32_t unmap(Sensors *s)
{
    memset(args, 0, sizeof(args));
    put32(0, CLIENT);
    put32(8, OBJECT);
    assert(answer(s, ESC_RM_UNMAP_MEMORY, UNMAP_BYTES));
    return get32(24);
}

static uint32_t free_object(Sensors *s)
{
    memset(args, 0, sizeof(args));
    put32(0, CLIENT);
    put32(8, OBJECT);
    assert(answer(s, ESC_RM_FREE, FREE_BYTES));
    return get32(12);
}

static bool page_is_empty(Sensors *s)
{
    uint8_t page[RUSD_MAP_BYTES];

    assert(pread(sensors_page_fd(s), page, sizeof(page), 0) == sizeof(page));
    for (uint32_t i = 0; i < sizeof(page); i++) {
        if (page[i]) {
            return false;
        }
    }
    return true;
}

static uint8_t reading(Sensors *s)
{
    uint8_t value = 0;

    assert(pread(sensors_page_fd(s), &value, sizeof(value), READING_AT) == sizeof(value));
    return value;
}

static Sensors *sensors_with(const char *path, const char *mode, FILE *log)
{
    setenv("FAKE_SENSORD", mode, 1);
    return sensors_new(log, path);
}

/* Everything a guest does with the object, ending with a page that has no readings. */
static void guest_works_without_readings(const char *path, const char *mode, FILE *log)
{
    Sensors *s = sensors_with(path, mode, log);

    assert(alloc(s, 0) == STATUS_OK);
    assert(poll_for(s, POLL_TEMPERATURE) == STATUS_OK);
    assert(map(s) == STATUS_OK);
    assert(sensors_take_mapping(s, FILE_ID));
    assert(sensors_page_fd(s) >= 0);
    assert(page_is_empty(s));
    assert(unmap(s) == STATUS_OK);
    assert(poll_for(s, 0) == STATUS_OK);
    assert(free_object(s) == STATUS_OK);
}

static void no_daemon_at_all(FILE *log)
{
    guest_works_without_readings(NULL, "good", log);
    guest_works_without_readings("/nonexistent/nvfwd-sensord", "good", log);
}

static void a_daemon_with_no_page(FILE *log)
{
    guest_works_without_readings(fake_daemon, "no-page", log);
    guest_works_without_readings(fake_daemon, "silent", log);
}

static void a_stuck_daemon_costs_a_stall_and_no_more(FILE *log)
{
    time_t start = time(NULL);

    guest_works_without_readings(fake_daemon, "stuck", log);
    assert(time(NULL) - start < MOST_SECONDS_FOR_STUCK_DAEMON);
}

static void a_working_daemon_fills_the_page(FILE *log)
{
    Sensors *s = sensors_with(fake_daemon, "good", log);

    assert(alloc(s, POLL_TEMPERATURE) == STATUS_OK);
    assert(reading(s) == READING);
    assert(poll_for(s, 0) == STATUS_OK);
    assert(reading(s) == READING);
    assert(free_object(s) == STATUS_OK);
}

/* Old readings would be worse than none: they look true. */
static void a_daemon_that_dies_takes_its_readings_with_it(FILE *log)
{
    Sensors *s = sensors_with(fake_daemon, "dies", log);

    assert(alloc(s, 0) == STATUS_OK);
    assert(reading(s) == READING);
    assert(poll_for(s, POLL_TEMPERATURE) == STATUS_OK);
    assert(page_is_empty(s));
    assert(poll_for(s, 0) == STATUS_OK);
    assert(map(s) == STATUS_OK);
    assert(free_object(s) == STATUS_OK);
}

/* The same when nothing a guest does would need an answer from it. */
static void a_quiet_death_is_noticed_too(FILE *log)
{
    Sensors *s = sensors_with(fake_daemon, "quits", log);

    assert(alloc(s, POLL_TEMPERATURE) == STATUS_OK);
    assert(reading(s) == READING);
    usleep(DAEMON_EXIT_MICROSECONDS);
    assert(poll_for(s, POLL_TEMPERATURE) == STATUS_OK);
    assert(page_is_empty(s));
}

static void no_daemon_does_not_loosen_the_mask(FILE *log)
{
    Sensors *s = sensors_with(NULL, "good", log);

    assert(alloc(s, POLL_PROCESSES) == STATUS_NOT_SUPPORTED);
    assert(alloc(s, 0) == STATUS_OK);
    assert(poll_for(s, POLL_PROCESSES) == STATUS_NOT_SUPPORTED);
}

int main(int argc, char **argv)
{
    FILE *log = fopen("/dev/null", "w");

    assert(argc == 2 && log);
    fake_daemon = argv[1];
    signal(SIGPIPE, SIG_IGN); /* as QEMU does */
    no_daemon_at_all(log);
    a_daemon_with_no_page(log);
    a_stuck_daemon_costs_a_stall_and_no_more(log);
    a_working_daemon_fills_the_page(log);
    a_daemon_that_dies_takes_its_readings_with_it(log);
    a_quiet_death_is_noticed_too(log);
    no_daemon_does_not_loosen_the_mask(log);
    puts("sensors: all passed");
    return 0;
}
