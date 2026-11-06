// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Stands in for nvfwd-sensord in test_sensors, which starts it the same way. FAKE_SENSORD says how it
 * behaves: "good", "no-page" as on a driver it doesn't know, "silent" to exit with nothing said,
 * "stuck" to say nothing and stay, "dies" to go at the first mask it's sent, or "quits" to answer
 * that mask and then go.
 */
#include "rusd.h"

#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define CLIENT 0xc1d00007u
#define READING_AT 0x908
#define READING 0x5a
#define STUCK_SECONDS 30

static void say_client(uint32_t client)
{
    if (write(SENSORD_SOCKET_FD, &client, sizeof(client)) != sizeof(client)) {
        exit(1);
    }
}

int main(void)
{
    const char *mode = getenv("FAKE_SENSORD");
    uint8_t reading = READING;
    uint64_t mask;
    int32_t ok = 0;

    if (!strcmp(mode, "silent")) {
        return 1;
    }
    if (!strcmp(mode, "stuck")) {
        sleep(STUCK_SECONDS);
        return 1;
    }
    if (!strcmp(mode, "no-page")) {
        say_client(0);
        return 1;
    }
    if (pwrite(SENSORD_PAGE_FD, &reading, sizeof(reading), READING_AT) != sizeof(reading)) {
        return 1;
    }
    say_client(CLIENT);
    while (read(SENSORD_SOCKET_FD, &mask, sizeof(mask)) == sizeof(mask)) {
        if (!strcmp(mode, "dies") ||
            write(SENSORD_SOCKET_FD, &ok, sizeof(ok)) != sizeof(ok) || !strcmp(mode, "quits")) {
            return 1;
        }
    }
    return 0;
}
