// SPDX-License-Identifier: GPL-2.0-or-later
/* Surfaces: the handle is ours, and nothing below ever sees one. */
#include "nvfwd_wsi.h"

static VkSurfaceKHR new_surface(uint32_t kind, void *display, uint64_t window)
{
    struct surface *s = calloc(1, sizeof(*s));
    s->kind = kind;
    s->display = display;
    s->window = window;
    return (VkSurfaceKHR)(uintptr_t)s;
}

struct xlib_info { VkStructureType sType; const void *pNext; VkFlags flags; void *dpy; unsigned long window; };
struct xcb_info { VkStructureType sType; const void *pNext; VkFlags flags; void *connection; uint32_t window; };

VKAPI_ATTR VkResult VKAPI_CALL create_xlib(VkInstance i, const void *info, const VkAllocationCallbacks *a, VkSurfaceKHR *out)
{
    const struct xlib_info *x = info;
    *out = new_surface(1, x->dpy, x->window);
    return VK_SUCCESS;
}

VKAPI_ATTR VkResult VKAPI_CALL create_xcb(VkInstance i, const void *info, const VkAllocationCallbacks *a, VkSurfaceKHR *out)
{
    const struct xcb_info *x = info;
    *out = new_surface(2, x->connection, x->window);
    return VK_SUCCESS;
}

struct wl_info { VkStructureType sType; const void *pNext; VkFlags flags; struct wl_display *display; struct wl_surface *surface; };

VKAPI_ATTR VkResult VKAPI_CALL create_wayland(VkInstance i, const void *info, const VkAllocationCallbacks *a, VkSurfaceKHR *out)
{
    const struct wl_info *x = info;
    *out = new_surface(3, x->display, (uint64_t)(uintptr_t)x->surface);
    return VK_SUCCESS;
}

VKAPI_ATTR void VKAPI_CALL destroy_surface(VkInstance i, VkSurfaceKHR s, const VkAllocationCallbacks *a)
{
    free((void *)(uintptr_t)s);
}

static void window_size(VkSurfaceKHR handle, uint32_t *w, uint32_t *h)
{
    struct surface *s = (void *)(uintptr_t)handle;
    typedef int (*get_geometry)(void *, unsigned long, unsigned long *, int *, int *, unsigned *, unsigned *, unsigned *, unsigned *);
    get_geometry g = (get_geometry)dlsym(RTLD_DEFAULT, "XGetGeometry");
    *w = 1280;
    *h = 720;
    if (s->kind == 1 && g) {
        unsigned long root;
        int x, y;
        unsigned width, height, border, depth;
        if (g(s->display, s->window, &root, &x, &y, &width, &height, &border, &depth)) {
            *w = width;
            *h = height;
        }
    }
}

VKAPI_ATTR VkResult VKAPI_CALL surface_support(VkPhysicalDevice p, uint32_t family, VkSurfaceKHR s, VkBool32 *out)
{
    *out = VK_TRUE;
    return VK_SUCCESS;
}

VKAPI_ATTR VkBool32 VKAPI_CALL presentation_support(VkPhysicalDevice p, uint32_t family, void *a, uint64_t b)
{
    return VK_TRUE;
}

VKAPI_ATTR VkResult VKAPI_CALL surface_caps(VkPhysicalDevice p, VkSurfaceKHR s, VkSurfaceCapabilitiesKHR *c)
{
    uint32_t w, h;
    window_size(s, &w, &h);
    memset(c, 0, sizeof(*c));
    c->minImageCount = 2;
    c->maxImageCount = MAX_IMAGES;
    c->currentExtent = (VkExtent2D){ w, h };
    if (((struct surface *)(uintptr_t)s)->kind == 3) {
        c->currentExtent = (VkExtent2D){ 0xFFFFFFFF, 0xFFFFFFFF }; /* on Wayland the app decides */
    }
    c->minImageExtent = (VkExtent2D){ 1, 1 };
    c->maxImageExtent = (VkExtent2D){ 16384, 16384 };
    c->maxImageArrayLayers = 1;
    c->supportedTransforms = VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR;
    c->currentTransform = VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR;
    c->supportedCompositeAlpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
    if (((struct surface *)(uintptr_t)s)->kind == 3) {
        c->supportedCompositeAlpha |= VK_COMPOSITE_ALPHA_PRE_MULTIPLIED_BIT_KHR | VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR;
    }
    c->supportedUsageFlags = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT |
                             VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_STORAGE_BIT;
    return VK_SUCCESS;
}

VKAPI_ATTR VkResult VKAPI_CALL surface_caps2(VkPhysicalDevice p, const VkPhysicalDeviceSurfaceInfo2KHR *i, VkSurfaceCapabilities2KHR *c)
{
    return surface_caps(p, i->surface, &c->surfaceCapabilities);
}

static const VkSurfaceFormatKHR formats[] = {
    { VK_FORMAT_B8G8R8A8_UNORM, VK_COLOR_SPACE_SRGB_NONLINEAR_KHR },
    { VK_FORMAT_R8G8B8A8_UNORM, VK_COLOR_SPACE_SRGB_NONLINEAR_KHR },
    { VK_FORMAT_B8G8R8A8_SRGB, VK_COLOR_SPACE_SRGB_NONLINEAR_KHR },
};
#define NFORMATS (sizeof(formats) / sizeof(formats[0]))

VKAPI_ATTR VkResult VKAPI_CALL surface_formats(VkPhysicalDevice p, VkSurfaceKHR s, uint32_t *n, VkSurfaceFormatKHR *out)
{
    if (out) {
        uint32_t c = *n < NFORMATS ? *n : NFORMATS;
        memcpy(out, formats, c * sizeof(*out));
        *n = c;
        return c < NFORMATS ? VK_INCOMPLETE : VK_SUCCESS;
    }
    *n = NFORMATS;
    return VK_SUCCESS;
}

VKAPI_ATTR VkResult VKAPI_CALL surface_formats2(VkPhysicalDevice p, const VkPhysicalDeviceSurfaceInfo2KHR *i, uint32_t *n, VkSurfaceFormat2KHR *out)
{
    if (out) {
        uint32_t c = *n < NFORMATS ? *n : NFORMATS;
        for (uint32_t k = 0; k < c; k++) {
            out[k].surfaceFormat = formats[k];
        }
        *n = c;
        return c < NFORMATS ? VK_INCOMPLETE : VK_SUCCESS;
    }
    *n = NFORMATS;
    return VK_SUCCESS;
}

VKAPI_ATTR VkResult VKAPI_CALL surface_modes(VkPhysicalDevice p, VkSurfaceKHR s, uint32_t *n, VkPresentModeKHR *out)
{
    static const VkPresentModeKHR modes[] = { VK_PRESENT_MODE_IMMEDIATE_KHR, VK_PRESENT_MODE_MAILBOX_KHR, VK_PRESENT_MODE_FIFO_KHR };
    if (out) {
        uint32_t c = *n < 3 ? *n : 3;
        memcpy(out, modes, c * sizeof(*out));
        *n = c;
        return c < 3 ? VK_INCOMPLETE : VK_SUCCESS;
    }
    *n = 3;
    return VK_SUCCESS;
}
