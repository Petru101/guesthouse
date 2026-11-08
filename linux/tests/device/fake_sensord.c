// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Stands in for nvfwd-sensord in test_sensors, which starts it the same way. FAKE_SENSORD says how it
 * behaves: "good", "no-page" as on a driver it doesn't know, "silent" to exit with nothing said,
 * "stuck" to say nothing and stay, "dies" to go at the first mask it's sent, or "quits" to answer
 * that mask and then go. Given another card than test_sensors gives, it has no page, as
 * nvfwd-sensord would have none of the right card's.
 */
#include "rusd.h"

#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define CLIENT 0xc1d00007u
#define READING_AT 0x908
#define READING 0x5a
#define STUCK_SECONDS 30
/* The card test_sensors starts it for. */
#define CARD_MINOR 1
#define CARD_GPU 0x300
#define CARD_INSTANCE 1

static void say_client(uint32_t client)
{
    if (write(SENSORD_SOCKET_FD, &client, sizeof(client)) != sizeof(client)) {
        exit(1);
    }
}

static bool given_the_card(int argc, char *argv[])
{
    RusdCard card;

    return rusd_card_parse(argc, argv, &card) && card.minor == CARD_MINOR && card.gpu_id == CARD_GPU &&
           card.instance == CARD_INSTANCE;
}

int main(int argc, char *argv[])
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
    if (!strcmp(mode, "no-page") || !given_the_card(argc, argv)) {
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
