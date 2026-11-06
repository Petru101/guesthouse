// SPDX-License-Identifier: GPL-2.0-or-later
/* Present on Wayland: NVIDIA's own Wayland WSI offers no device here, so frames go to the compositor as shared memory. */
#include "nvfwd_wsi.h"

static void wl_release(void *data, struct wl_buffer *b)
{
    *(uint8_t *)data = 0;
}

static const struct wl_buffer_listener wl_buffer_events = { wl_release };

static struct wl_shm *wl_found_shm;

static void wl_global(void *data, struct wl_registry *r, uint32_t name, const char *iface, uint32_t version)
{
    if (!strcmp(iface, "wl_shm")) {
        wl_found_shm = wl_registry_bind(r, name, &wl_shm_interface, 1);
    }
}

static void wl_global_gone(void *data, struct wl_registry *r, uint32_t name) { }

static const struct wl_registry_listener wl_registry_events = { wl_global, wl_global_gone };

static void wl_frame_done(void *data, struct wl_callback *cb, uint32_t time)
{
    *(int *)data = 0;
    wl_callback_destroy(cb);
}

static const struct wl_callback_listener wl_frame_events = { wl_frame_done };

/* A readback buffer, a copy per image, and shared memory buffers the compositor can read. */
int wl_setup(struct swapchain *s, struct surface *surface, const VkSwapchainCreateInfoKHR *c)
{
    VkDevice d = s->device;
    uint32_t stride = s->width * 4;
    size_t one = (size_t)stride * s->height;
    s->wl_display = surface->display;
    s->wl_surface = (void *)(uintptr_t)surface->window;
    s->fifo = c->presentMode == VK_PRESENT_MODE_FIFO_KHR;
    s->wl_queue = wl_display_create_queue(s->wl_display);
    struct wl_display *wrapper = wl_proxy_create_wrapper(s->wl_display);
    wl_proxy_set_queue((struct wl_proxy *)wrapper, s->wl_queue);
    struct wl_registry *reg = wl_display_get_registry(wrapper);
    wl_found_shm = NULL;
    wl_registry_add_listener(reg, &wl_registry_events, NULL);
    wl_display_roundtrip_queue(s->wl_display, s->wl_queue);
    wl_proxy_wrapper_destroy(wrapper);
    if (!wl_found_shm) {
        return 0;
    }
    s->wl_wrapper = wl_proxy_create_wrapper(s->wl_surface);
    wl_proxy_set_queue((struct wl_proxy *)s->wl_wrapper, s->wl_queue);
    int fd = memfd_create("nvfwd-wl", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, one * WL_BUFFERS)) {
        return 0;
    }
    s->wl_pixels = mmap(NULL, one * WL_BUFFERS, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    s->wl_pool = wl_shm_create_pool(wl_found_shm, fd, one * WL_BUFFERS);
    close(fd);
    for (int i = 0; i < WL_BUFFERS; i++) {
        int opaque = c->compositeAlpha == VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
        uint32_t format = opaque ? WL_SHM_FORMAT_XRGB8888 : WL_SHM_FORMAT_ARGB8888;
        if (s->format == VK_FORMAT_A2R10G10B10_UNORM_PACK32) { /* Zink asks for this one on a 10-bit config */
            format = opaque ? WL_SHM_FORMAT_XRGB2101010 : WL_SHM_FORMAT_ARGB2101010;
        } else if (s->format == VK_FORMAT_A2B10G10R10_UNORM_PACK32) {
            format = opaque ? WL_SHM_FORMAT_XBGR2101010 : WL_SHM_FORMAT_ABGR2101010;
        } else if (s->format == VK_FORMAT_R8G8B8A8_UNORM || s->format == VK_FORMAT_R8G8B8A8_SRGB) {
            format = opaque ? WL_SHM_FORMAT_XBGR8888 : WL_SHM_FORMAT_ABGR8888;
        }
        s->wl_buffer[i] = wl_shm_pool_create_buffer(s->wl_pool, one * i, s->width, s->height, stride, format);
        wl_buffer_add_listener(s->wl_buffer[i], &wl_buffer_events, &s->wl_busy[i]);
    }
    VkBufferCreateInfo bi = { VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO, .size = one, .usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT };
    DEV(vkCreateBuffer)(d, &bi, NULL, &s->readback);
    VkMemoryRequirements m;
    DEV(vkGetBufferMemoryRequirements)(d, s->readback, &m);
    VkMemoryAllocateInfo mi = { VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .allocationSize = m.size,
                                .memoryTypeIndex = memory_type(m.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_CACHED_BIT) };
    if (DEV(vkAllocateMemory)(d, &mi, NULL, &s->readback_memory)) {
        return 0;
    }
    DEV(vkBindBufferMemory)(d, s->readback, s->readback_memory, 0);
    DEV(vkMapMemory)(d, s->readback_memory, 0, VK_WHOLE_SIZE, 0, (void **)&s->readback_map);
    VkCommandPoolCreateInfo pi = { VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, .queueFamilyIndex = the_family };
    DEV(vkCreateCommandPool)(d, &pi, NULL, &s->pool);
    VkCommandBufferAllocateInfo ai = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, .commandPool = s->pool,
                                       .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY, .commandBufferCount = s->count };
    DEV(vkAllocateCommandBuffers)(d, &ai, s->wl_copy);
    for (uint32_t i = 0; i < s->count; i++) {
        VkCommandBuffer cb = s->wl_copy[i];
        if (set_loader_data) {
            set_loader_data(d, cb);
        }
        VkCommandBufferBeginInfo bg = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO };
        DEV(vkBeginCommandBuffer)(cb, &bg);
        VkImageMemoryBarrier to = { VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER, .srcAccessMask = VK_ACCESS_MEMORY_WRITE_BIT,
                                    .dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT, .oldLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
                                    .newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
                                    .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED, .image = s->image[i],
                                    .subresourceRange = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 } };
        DEV(vkCmdPipelineBarrier)(cb, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, NULL, 0, NULL, 1, &to);
        VkBufferImageCopy region = { .bufferRowLength = s->width, .imageSubresource = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1 },
                                     .imageExtent = { s->width, s->height, 1 } };
        DEV(vkCmdCopyImageToBuffer)(cb, s->image[i], VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, s->readback, 1, &region);
        VkImageMemoryBarrier back = to;
        back.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
        back.dstAccessMask = 0;
        back.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
        back.newLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
        VkMemoryBarrier done = { VK_STRUCTURE_TYPE_MEMORY_BARRIER, .srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT, .dstAccessMask = VK_ACCESS_HOST_READ_BIT };
        DEV(vkCmdPipelineBarrier)(cb, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT | VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, 0, 1, &done, 0, NULL, 1, &back);
        DEV(vkEndCommandBuffer)(cb);
    }
    return 1;
}

/* After the copy's fence: into a free shared buffer, then to the compositor. */
void wl_present(struct swapchain *s)
{
    wl_display_dispatch_queue_pending(s->wl_display, s->wl_queue);
    if (s->fifo) {
        /* The compositor's frame callback paces FIFO. A hidden window gets none, so the wait has an end. */
        for (int tries = 0; s->frame_flag && *s->frame_flag && tries < 5; tries++) {
            struct pollfd p = { wl_display_get_fd(s->wl_display), POLLIN, 0 };
            while (wl_display_prepare_read_queue(s->wl_display, s->wl_queue) != 0) {
                wl_display_dispatch_queue_pending(s->wl_display, s->wl_queue);
            }
            wl_display_flush(s->wl_display);
            if (poll(&p, 1, 20) > 0) {
                wl_display_read_events(s->wl_display);
            } else {
                wl_display_cancel_read(s->wl_display);
            }
            wl_display_dispatch_queue_pending(s->wl_display, s->wl_queue);
        }
    }
    for (int i = 0; i < WL_BUFFERS; i++) {
        if (!s->wl_busy[i]) {
            size_t one = (size_t)s->width * 4 * s->height;
            memcpy(s->wl_pixels + one * i, s->readback_map, one);
            s->wl_busy[i] = 1;
            wl_surface_attach(s->wl_wrapper, s->wl_buffer[i], 0, 0);
            wl_surface_damage_buffer(s->wl_wrapper, 0, 0, s->width, s->height);
            if (s->fifo) {
                s->frame_flag = calloc(1, sizeof(int)); /* outlives the swapchain if the callback comes late */
                *s->frame_flag = 1;
                wl_callback_add_listener(wl_surface_frame(s->wl_wrapper), &wl_frame_events, s->frame_flag);
            }
            wl_surface_commit(s->wl_wrapper);
            wl_display_flush(s->wl_display);
            s->shown++;
            return;
        }
    }
    s->dropped++;
}
