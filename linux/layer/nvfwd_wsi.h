// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * An implicit Vulkan layer for a guest with no NVIDIA X driver or display. NVIDIA's own WSI wants its
 * X driver, so this layer answers every surface call and owns the swapchain: plain device-local
 * images, an acquire that only signals, and a present that only waits. One instance and one device
 * at a time.
 *
 * This header is what its parts share.
 */
#ifndef NVFWD_WSI_H
#define NVFWD_WSI_H

#define _GNU_SOURCE
#include <vulkan/vulkan.h>
#include <vulkan/vk_layer.h>
#include <dlfcn.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <poll.h>
#include <wayland-client.h>
#include "nvfwd_proto.h"

#define EXPORT __attribute__((visibility("default")))
#define MAX_IMAGES 8
#define WL_BUFFERS 3

struct surface { uint32_t kind; void *display; uint64_t window; };

struct swapchain {
    VkDevice device;
    uint32_t count, next, width, height;
    VkFormat format;
    VkImage image[MAX_IMAGES];
    VkDeviceMemory memory[MAX_IMAGES];
    VkFence fence[MAX_IMAGES];
    uint8_t pending[MAX_IMAGES];
    uint64_t presents;
    /* Frames for the host: linear buffers in video memory, exported once. */
    int show;
    VkBuffer buffer[NVFWD_SHOW_SLOTS];
    VkDeviceMemory buffer_memory[NVFWD_SHOW_SLOTS];
    VkCommandPool pool;
    VkCommandBuffer copy[MAX_IMAGES][NVFWD_SHOW_SLOTS];
    int8_t slot_of[MAX_IMAGES]; /* the slot this image's fence will fill, or -1 */
    uint64_t present_of[MAX_IMAGES];
    uint64_t shown, dropped;
    /* Wayland: no dmabuf here, so frames go to the compositor as shared memory. */
    struct wl_surface *wl_surface, *wl_wrapper;
    struct wl_display *wl_display;
    struct wl_event_queue *wl_queue;
    struct wl_shm_pool *wl_pool;
    struct wl_buffer *wl_buffer[WL_BUFFERS];
    uint8_t wl_busy[WL_BUFFERS];
    uint8_t *wl_pixels;
    VkBuffer readback;
    VkDeviceMemory readback_memory;
    uint8_t *readback_map;
    VkCommandBuffer wl_copy[MAX_IMAGES];
    int fifo, *frame_flag;
};

extern PFN_vkGetInstanceProcAddr next_gipa;
extern PFN_vkGetDeviceProcAddr next_gdpa;
extern VkInstance the_instance;
extern VkPhysicalDevice the_gpu;
extern VkDevice the_device;
extern VkQueue the_queue; /* the layer's own, for acquires: apps may submit on theirs from another thread */
extern pthread_mutex_t lock;
extern uint32_t the_family;
extern int show_fd;
extern PFN_vkSetDeviceLoaderData set_loader_data;

#define DEV(name) ((PFN_##name)next_gdpa(the_device, #name))
#define INST(name) ((PFN_##name)next_gipa(the_instance, #name))

/* wsi_surfaces.c */
VKAPI_ATTR VkResult VKAPI_CALL create_wayland(VkInstance i, const void *info, const VkAllocationCallbacks *a, VkSurfaceKHR *out);
VKAPI_ATTR VkResult VKAPI_CALL create_xcb(VkInstance i, const void *info, const VkAllocationCallbacks *a, VkSurfaceKHR *out);
VKAPI_ATTR VkResult VKAPI_CALL create_xlib(VkInstance i, const void *info, const VkAllocationCallbacks *a, VkSurfaceKHR *out);
VKAPI_ATTR void VKAPI_CALL destroy_surface(VkInstance i, VkSurfaceKHR s, const VkAllocationCallbacks *a);
VKAPI_ATTR VkBool32 VKAPI_CALL presentation_support(VkPhysicalDevice p, uint32_t family, void *a, uint64_t b);
VKAPI_ATTR VkResult VKAPI_CALL surface_caps(VkPhysicalDevice p, VkSurfaceKHR s, VkSurfaceCapabilitiesKHR *c);
VKAPI_ATTR VkResult VKAPI_CALL surface_caps2(VkPhysicalDevice p, const VkPhysicalDeviceSurfaceInfo2KHR *i, VkSurfaceCapabilities2KHR *c);
VKAPI_ATTR VkResult VKAPI_CALL surface_formats(VkPhysicalDevice p, VkSurfaceKHR s, uint32_t *n, VkSurfaceFormatKHR *out);
VKAPI_ATTR VkResult VKAPI_CALL surface_formats2(VkPhysicalDevice p, const VkPhysicalDeviceSurfaceInfo2KHR *i, uint32_t *n, VkSurfaceFormat2KHR *out);
VKAPI_ATTR VkResult VKAPI_CALL surface_modes(VkPhysicalDevice p, VkSurfaceKHR s, uint32_t *n, VkPresentModeKHR *out);
VKAPI_ATTR VkResult VKAPI_CALL surface_support(VkPhysicalDevice p, uint32_t family, VkSurfaceKHR s, VkBool32 *out);

/* wsi_wayland.c */
void wl_present(struct swapchain *s);
int wl_setup(struct swapchain *s, struct surface *surface, const VkSwapchainCreateInfoKHR *c);

/* wsi_swapchain.c */
VKAPI_ATTR VkResult VKAPI_CALL acquire(VkDevice d, VkSwapchainKHR handle, uint64_t timeout, VkSemaphore semaphore, VkFence fence, uint32_t *index);
VKAPI_ATTR VkResult VKAPI_CALL acquire2(VkDevice d, const VkAcquireNextImageInfoKHR *i, uint32_t *index);
VKAPI_ATTR VkResult VKAPI_CALL create_swapchain(VkDevice d, const VkSwapchainCreateInfoKHR *c, const VkAllocationCallbacks *a, VkSwapchainKHR *out);
VKAPI_ATTR void VKAPI_CALL destroy_swapchain(VkDevice d, VkSwapchainKHR handle, const VkAllocationCallbacks *a);
uint32_t memory_type(uint32_t bits, VkMemoryPropertyFlags want);
VKAPI_ATTR VkResult VKAPI_CALL present(VkQueue q, const VkPresentInfoKHR *info);
VKAPI_ATTR VkResult VKAPI_CALL swapchain_images(VkDevice d, VkSwapchainKHR handle, uint32_t *n, VkImage *out);

#endif
