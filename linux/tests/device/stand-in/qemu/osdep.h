/* What sensors.c gets from QEMU's own header, for building it with no QEMU configured. */
#ifndef STAND_IN_OSDEP_H
#define STAND_IN_OSDEP_H

#include <glib.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/time.h>
#include <unistd.h>

extern char **environ;

#endif
