// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * How fast a guest can change the sensor poll mask. Each change reaches the host's driver through the
 * sensor daemon, so it has to wait for the device's call rate like any other call. Run it on a guest
 * started with a low max-call-rate, with nothing else using the GPU, and compare.
 * Build: gcc -O2 -o sensorrate sensorrate.c
 */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>

#define IOCTL_MAGIC 'F'
#define ESC_RM_CONTROL 0x2a
#define ESC_RM_ALLOC 0x2b
#define CLASS_ROOT_CLIENT 0x41
#define CLASS_SENSOR_PAGE 0xde
#define CTRL_POLL 0xde0001
#define POLL_TEMPERATURE 0x10
#define SENSOR_OBJECT 0xde000010u
#define DEFAULT_SECONDS 3.0

struct alloc_args {
    uint32_t root, parent, object, class_id;
    uint64_t params, rights;
    uint32_t params_size, flags, status, pad;
};

struct control_args {
    uint32_t client, object, cmd, flags;
    uint64_t params;
    uint32_t params_size, status;
};

struct client_params {
    uint32_t client, process_id;
    char process_name[104];
    uint64_t pid_info;
};

static int call(int fd, unsigned nr, void *args, size_t size)
{
    return ioctl(fd, _IOC(_IOC_READ | _IOC_WRITE, IOCTL_MAGIC, nr, size), args);
}

static double now(void)
{
    struct timespec t;

    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec * 1e-9;
}

int main(int argc, char **argv)
{
    double seconds = argc > 1 ? atof(argv[1]) : DEFAULT_SECONDS;
    int fd = open("/dev/nvidiactl", O_RDWR);
    struct client_params client = { 0 };
    struct alloc_args alloc = { .class_id = CLASS_ROOT_CLIENT, .params = (uintptr_t)&client,
                                .params_size = sizeof(client) };
    uint64_t mask = 0, calls = 0;
    double start;

    if (fd < 0 || call(fd, ESC_RM_ALLOC, &alloc, sizeof(alloc)) || alloc.status) {
        printf("no client: status %#x\n", alloc.status);
        return 1;
    }
    uint32_t root = alloc.object;
    struct alloc_args sensor = { .root = root, .parent = root, .object = SENSOR_OBJECT,
                                 .class_id = CLASS_SENSOR_PAGE, .params = (uintptr_t)&mask,
                                 .params_size = sizeof(mask) };
    if (call(fd, ESC_RM_ALLOC, &sensor, sizeof(sensor)) || sensor.status) {
        printf("no sensor object: status %#x\n", sensor.status);
        return 1;
    }
    start = now();
    while (now() - start < seconds) {
        struct control_args poll = { .client = root, .object = SENSOR_OBJECT, .cmd = CTRL_POLL,
                                     .params = (uintptr_t)&mask, .params_size = sizeof(mask) };
        mask ^= POLL_TEMPERATURE;
        if (call(fd, ESC_RM_CONTROL, &poll, sizeof(poll)) || poll.status) {
            printf("poll: status %#x\n", poll.status);
            return 1;
        }
        calls++;
    }
    printf("%llu mask changes in %.2f s, %.0f a second\n", (unsigned long long)calls, seconds, calls / seconds);
    return 0;
}
