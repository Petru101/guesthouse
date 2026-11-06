// SPDX-License-Identifier: MIT
// Copyright (C) 2026 xnixy
/*
 * A GLX vendor for libglvnd that is Mesa, with one difference: Zink presents over DRI3 itself. On X11 its usual swapchain
 * goes to NVIDIA's X11 present, which needs a display object a guest does not have. This has to be for GLX alone. On
 * Wayland that swapchain is what synchronises a client with the compositor, and without it frames are shown unfinished.
 *
 * Build: gcc -O2 -Wall -shared -fPIC -o libGLX_dri3.so.0 glx_dri3.c -l:libGLX_mesa.so.0
 * Use: __GLX_VENDOR_LIBRARY_NAME=dri3
 */
#include <stdlib.h>

__attribute__((constructor)) static void present_over_dri3(void)
{
    setenv("LIBGL_KOPPER_DISABLE", "1", 1);
}
