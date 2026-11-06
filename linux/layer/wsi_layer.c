// SPDX-License-Identifier: GPL-2.0-or-later
/* The layer's entry points: instance, device, and looking up what it answers. */
#include "nvfwd_wsi.h"

PFN_vkGetInstanceProcAddr next_gipa;
PFN_vkGetDeviceProcAddr next_gdpa;
VkInstance the_instance;
VkPhysicalDevice the_gpu;
VkDevice the_device;
VkQueue the_queue; /* the layer's own, for acquires: apps may submit on theirs from another thread */
pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
uint32_t the_family;
int show_fd = -1;
PFN_vkSetDeviceLoaderData set_loader_data;

static void *chain_find(const void *pnext, VkStructureType type, int function)
{
    for (VkLayerInstanceCreateInfo *i = (void *)pnext; i; i = (void *)i->pNext) {
        if (i->sType == type && (int)i->function == function) {
            return i;
        }
    }
    return NULL;
}

static VKAPI_ATTR VkResult VKAPI_CALL create_instance(const VkInstanceCreateInfo *info, const VkAllocationCallbacks *alloc, VkInstance *out)
{
    VkLayerInstanceCreateInfo *link = chain_find(info->pNext, VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO, VK_LAYER_LINK_INFO);
    if (!link) {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    next_gipa = link->u.pLayerInfo->pfnNextGetInstanceProcAddr;
    link->u.pLayerInfo = link->u.pLayerInfo->pNext;
    PFN_vkCreateInstance create = (PFN_vkCreateInstance)next_gipa(NULL, "vkCreateInstance");
    VkResult r = create(info, alloc, out);
    if (r == VK_SUCCESS) {
        the_instance = *out;
    }
    return r;
}

static VKAPI_ATTR VkResult VKAPI_CALL create_device(VkPhysicalDevice gpu, const VkDeviceCreateInfo *info, const VkAllocationCallbacks *alloc, VkDevice *out)
{
    VkLayerDeviceCreateInfo *link = chain_find(info->pNext, VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO, VK_LAYER_LINK_INFO);
    if (!link) {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    PFN_vkGetInstanceProcAddr gipa = link->u.pLayerInfo->pfnNextGetInstanceProcAddr;
    next_gdpa = link->u.pLayerInfo->pfnNextGetDeviceProcAddr;
    link->u.pLayerInfo = link->u.pLayerInfo->pNext;
    VkLayerDeviceCreateInfo *data = chain_find(info->pNext, VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO, VK_LOADER_DATA_CALLBACK);
    if (data) {
        set_loader_data = data->u.pfnSetDeviceLoaderData;
    }
    PFN_vkCreateDevice create = (PFN_vkCreateDevice)gipa(the_instance, "vkCreateDevice");
    /* Sharing a frame needs an exportable allocation, whatever the app asked for. */
    static const char *const wanted[] = { VK_KHR_EXTERNAL_MEMORY_EXTENSION_NAME, VK_KHR_EXTERNAL_MEMORY_FD_EXTENSION_NAME };
    const char **names = calloc(info->enabledExtensionCount + 2, sizeof(*names));
    uint32_t count = info->enabledExtensionCount;
    memcpy(names, info->ppEnabledExtensionNames, count * sizeof(*names));
    for (int w = 0; w < 2; w++) {
        int have = 0;
        for (uint32_t i = 0; i < info->enabledExtensionCount; i++) {
            have |= !strcmp(names[i], wanted[w]);
        }
        if (!have) {
            names[count++] = wanted[w];
        }
    }
    VkDeviceCreateInfo copy = *info;
    copy.enabledExtensionCount = count;
    copy.ppEnabledExtensionNames = names;
    VkDeviceQueueCreateInfo *queues = calloc(info->queueCreateInfoCount, sizeof(*queues));
    memcpy(queues, info->pQueueCreateInfos, info->queueCreateInfoCount * sizeof(*queues));
    uint32_t own = queues[0].queueCount;
    float *priorities = calloc(own + 1, sizeof(float));
    memcpy(priorities, queues[0].pQueuePriorities, own * sizeof(float));
    priorities[own] = priorities[0];
    queues[0].queueCount = own + 1;
    queues[0].pQueuePriorities = priorities;
    copy.pQueueCreateInfos = queues;
    VkResult r = create(gpu, &copy, alloc, out);
    free(names);
    free(queues);
    free(priorities);
    if (r == VK_SUCCESS) {
        the_gpu = gpu;
        the_device = *out;
        the_family = info->pQueueCreateInfos[0].queueFamilyIndex;
        DEV(vkGetDeviceQueue)(*out, the_family, own, &the_queue);
    }
    return r;
}

static VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL gdpa(VkDevice d, const char *name);

static PFN_vkVoidFunction surface_lookup(const char *n)
{
    static const struct { const char *name; void *f; } t[] = {
        { "vkCreateXlibSurfaceKHR", create_xlib }, { "vkCreateXcbSurfaceKHR", create_xcb }, { "vkCreateWaylandSurfaceKHR", create_wayland },
        { "vkDestroySurfaceKHR", destroy_surface }, { "vkGetPhysicalDeviceSurfaceSupportKHR", surface_support },
        { "vkGetPhysicalDeviceXlibPresentationSupportKHR", presentation_support },
        { "vkGetPhysicalDeviceXcbPresentationSupportKHR", presentation_support },
        { "vkGetPhysicalDeviceWaylandPresentationSupportKHR", presentation_support },
        { "vkGetPhysicalDeviceSurfaceCapabilitiesKHR", surface_caps }, { "vkGetPhysicalDeviceSurfaceCapabilities2KHR", surface_caps2 },
        { "vkGetPhysicalDeviceSurfaceFormatsKHR", surface_formats }, { "vkGetPhysicalDeviceSurfaceFormats2KHR", surface_formats2 },
        { "vkGetPhysicalDeviceSurfacePresentModesKHR", surface_modes },
    };
    for (size_t i = 0; i < sizeof(t) / sizeof(t[0]); i++) {
        if (!strcmp(n, t[i].name)) {
            return (PFN_vkVoidFunction)t[i].f;
        }
    }
    return NULL;
}

static PFN_vkVoidFunction device_lookup(const char *n)
{
    static const struct { const char *name; void *f; } t[] = {
        { "vkCreateSwapchainKHR", create_swapchain }, { "vkDestroySwapchainKHR", destroy_swapchain },
        { "vkGetSwapchainImagesKHR", swapchain_images }, { "vkAcquireNextImageKHR", acquire },
        { "vkAcquireNextImage2KHR", acquire2 }, { "vkQueuePresentKHR", present }, { "vkGetDeviceProcAddr", gdpa },
    };
    for (size_t i = 0; i < sizeof(t) / sizeof(t[0]); i++) {
        if (!strcmp(n, t[i].name)) {
            return (PFN_vkVoidFunction)t[i].f;
        }
    }
    return NULL;
}

static VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL gdpa(VkDevice d, const char *name)
{
    PFN_vkVoidFunction f = device_lookup(name);
    return f ? f : next_gdpa(d, name);
}

static VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL gipa(VkInstance i, const char *name)
{
    PFN_vkVoidFunction f;
    if (!strcmp(name, "vkGetInstanceProcAddr")) {
        return (PFN_vkVoidFunction)gipa;
    }
    if (!strcmp(name, "vkCreateInstance")) {
        return (PFN_vkVoidFunction)create_instance;
    }
    if (!strcmp(name, "vkCreateDevice")) {
        return (PFN_vkVoidFunction)create_device;
    }
    if ((f = surface_lookup(name)) || (f = device_lookup(name))) {
        return f;
    }
    return next_gipa ? next_gipa(i, name) : NULL;
}

EXPORT VKAPI_ATTR VkResult VKAPI_CALL vkNegotiateLoaderLayerInterfaceVersion(VkNegotiateLayerInterface *v)
{
    if (v->loaderLayerInterfaceVersion > 2) {
        v->loaderLayerInterfaceVersion = 2;
    }
    v->pfnGetInstanceProcAddr = gipa;
    v->pfnGetDeviceProcAddr = gdpa;
    v->pfnGetPhysicalDeviceProcAddr = NULL;
    return VK_SUCCESS;
}
