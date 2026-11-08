// SPDX-License-Identifier: MIT
// Copyright (C) 2026 xnixy
/*
 * An EGL vendor for libglvnd that is no vendor at all. It is listed first, so libglvnd loads it before Mesa, and in Xwayland
 * it takes DISPLAY away before Mesa's Zink loads NVIDIA's Vulkan driver, which opens DISPLAY as it loads. In Xwayland that
 * display is the server itself, which can't answer yet, so it would wait on itself. Ubuntu's own install wraps Xwayland for
 * this, which a distribution's package manager would undo on a guest installed by hand. Then it declines, and libglvnd goes
 * on to the next vendor.
 *
 * Build: gcc -O2 -Wall -shared -fPIC -o libEGL_nvfwd_xwayland.so.0 xwayland_display.c
 * Use: first in __EGL_VENDOR_LIBRARY_FILENAMES, through EGL_nvfwd_xwayland.json.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <stdlib.h>
#include <string.h>

__attribute__((constructor)) static void no_display_in_xwayland(void)
{
    if (!strcmp(program_invocation_short_name, "Xwayland"))
        unsetenv("DISPLAY");
}

/* libglvnd's entry point for a vendor. Returning false leaves this one out. */
__attribute__((visibility("default"))) unsigned int __egl_Main(unsigned int version, const void *exports, void *vendor, void *imports)
{
    return 0;
}
