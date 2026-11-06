// SPDX-License-Identifier: GPL-2.0-or-later
/* Swapchains, and frames for the viewer on the host. */
#include "nvfwd_wsi.h"

uint32_t memory_type(uint32_t bits, VkMemoryPropertyFlags want)
{
    VkPhysicalDeviceMemoryProperties m;
    INST(vkGetPhysicalDeviceMemoryProperties)(the_gpu, &m);
    for (uint32_t i = 0; i < m.memoryTypeCount; i++) {
        if ((bits >> i & 1) && (m.memoryTypes[i].propertyFlags & want) == want) {
            return i;
        }
    }
    return 0;
}

static void show_publish(struct swapchain *s, VkDevice d);

VKAPI_ATTR void VKAPI_CALL destroy_swapchain(VkDevice d, VkSwapchainKHR handle, const VkAllocationCallbacks *a)
{
    struct swapchain *s = (void *)(uintptr_t)handle;
    if (!s) {
        return;
    }
    for (uint32_t i = 0; i < s->count; i++) {
        if (s->pending[i]) {
            DEV(vkWaitForFences)(d, 1, &s->fence[i], VK_TRUE, UINT64_MAX);
        }
    }
    if (s->show) {
        show_publish(s, d);
    }
    for (uint32_t i = 0; i < s->count; i++) {
        DEV(vkDestroyFence)(d, s->fence[i], NULL);
        DEV(vkDestroyImage)(d, s->image[i], NULL);
        DEV(vkFreeMemory)(d, s->memory[i], NULL);
    }
    if (s->readback) {
        size_t one = (size_t)s->width * 4 * s->height;
        DEV(vkDestroyCommandPool)(d, s->pool, NULL);
        DEV(vkDestroyBuffer)(d, s->readback, NULL);
        DEV(vkFreeMemory)(d, s->readback_memory, NULL);
        for (int i = 0; i < WL_BUFFERS; i++) {
            wl_buffer_destroy(s->wl_buffer[i]);
        }
        wl_shm_pool_destroy(s->wl_pool);
        munmap(s->wl_pixels, one * WL_BUFFERS);
        /* The queue stays: a frame callback may still be on its way to it. One queue per swapchain stays. */
        wl_display_flush(s->wl_display);
        fprintf(stderr, "nvfwd wsi: %llu presents, %llu to Wayland, %llu dropped\n", (unsigned long long)s->presents,
                (unsigned long long)s->shown, (unsigned long long)s->dropped);
    }
    if (s->show) {
        DEV(vkDestroyCommandPool)(d, s->pool, NULL);
        for (uint32_t i = 0; i < NVFWD_SHOW_SLOTS; i++) {
            DEV(vkDestroyBuffer)(d, s->buffer[i], NULL);
            DEV(vkFreeMemory)(d, s->buffer_memory[i], NULL);
        }
        fprintf(stderr, "nvfwd wsi: %llu presents, %llu sent to the host, %llu dropped\n", (unsigned long long)s->presents,
                (unsigned long long)s->shown, (unsigned long long)s->dropped);
    }
    free(s);
}

/* Linear buffers the host can read, and a copy from every image to every buffer, recorded once. */
static int show_setup(struct swapchain *s, const VkSwapchainCreateInfoKHR *c)
{
    VkDevice d = s->device;
    uint32_t pitch = (s->width * 4 + 255) & ~255u;
    PFN_vkGetMemoryFdKHR get_fd = DEV(vkGetMemoryFdKHR);

    if (c->imageFormat != VK_FORMAT_B8G8R8A8_UNORM || !get_fd) {
        fprintf(stderr, "nvfwd wsi: format %d is not shown\n", c->imageFormat);
        return 0;
    }
    if (show_fd < 0) {
        show_fd = open("/dev/nvidiactl", O_RDWR | O_CLOEXEC);
    }
    for (uint32_t i = 0; i < NVFWD_SHOW_SLOTS; i++) {
        VkExternalMemoryBufferCreateInfo eb = { VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_BUFFER_CREATE_INFO,
                                                .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT };
        VkBufferCreateInfo bi = { VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO, .pNext = &eb, .size = (uint64_t)pitch * s->height,
                                  .usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT, .sharingMode = VK_SHARING_MODE_EXCLUSIVE };
        VkResult r = DEV(vkCreateBuffer)(d, &bi, NULL, &s->buffer[i]);
        if (r) {
            return 0;
        }
        VkMemoryRequirements m;
        DEV(vkGetBufferMemoryRequirements)(d, s->buffer[i], &m);
        VkMemoryDedicatedAllocateInfo ded = { VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO, .buffer = s->buffer[i] };
        VkExportMemoryAllocateInfo ex = { VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO, .pNext = &ded,
                                          .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT };
        VkMemoryAllocateInfo mi = { VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .pNext = &ex, .allocationSize = m.size,
                                    .memoryTypeIndex = memory_type(m.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) };
        r = DEV(vkAllocateMemory)(d, &mi, NULL, &s->buffer_memory[i]);
        if (r) {
            fprintf(stderr, "nvfwd wsi: export allocation failed: %d\n", r);
            return 0;
        }
        DEV(vkBindBufferMemory)(d, s->buffer[i], s->buffer_memory[i], 0);
        VkMemoryGetFdInfoKHR gf = { VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR, .memory = s->buffer_memory[i],
                                    .handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT };
        int fd = -1;
        r = get_fd(d, &gf, &fd);
        if (r) {
            fprintf(stderr, "nvfwd wsi: vkGetMemoryFdKHR failed: %d\n", r);
            return 0;
        }
        struct nvfwd_show_buffer b = { fd, i, s->width, s->height, pitch, 0x34325258, (m.size + 4095) & ~4095ull };
        int rc = ioctl(show_fd, NVFWD_SHOW_IOC_BUFFER, &b);
        close(fd);
        if (rc) {
            perror("nvfwd wsi: show buffer");
            return 0;
        }
    }
    VkCommandPoolCreateInfo pi = { VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, .queueFamilyIndex = the_family };
    DEV(vkCreateCommandPool)(d, &pi, NULL, &s->pool);
    for (uint32_t i = 0; i < s->count; i++) {
        VkCommandBufferAllocateInfo ai = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, .commandPool = s->pool,
                                           .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY, .commandBufferCount = NVFWD_SHOW_SLOTS };
        DEV(vkAllocateCommandBuffers)(d, &ai, s->copy[i]);
        for (uint32_t k = 0; k < NVFWD_SHOW_SLOTS; k++) {
            VkCommandBuffer cb = s->copy[i][k];
            if (set_loader_data) {
                set_loader_data(d, cb);
            }
            VkCommandBufferBeginInfo bi = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO };
            DEV(vkBeginCommandBuffer)(cb, &bi);
            VkImageMemoryBarrier to = { VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER, .srcAccessMask = VK_ACCESS_MEMORY_WRITE_BIT,
                                        .dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT, .oldLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
                                        .newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
                                        .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED, .image = s->image[i],
                                        .subresourceRange = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 } };
            DEV(vkCmdPipelineBarrier)(cb, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, NULL, 0, NULL, 1, &to);
            VkBufferImageCopy region = { .bufferRowLength = pitch / 4, .imageSubresource = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1 },
                                         .imageExtent = { s->width, s->height, 1 } };
            DEV(vkCmdCopyImageToBuffer)(cb, s->image[i], VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, s->buffer[k], 1, &region);
            VkImageMemoryBarrier back = to;
            back.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
            back.dstAccessMask = 0;
            back.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
            back.newLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
            VkMemoryBarrier done = { VK_STRUCTURE_TYPE_MEMORY_BARRIER, .srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT, .dstAccessMask = VK_ACCESS_MEMORY_READ_BIT };
            DEV(vkCmdPipelineBarrier)(cb, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, 0, 1, &done, 0, NULL, 1, &back);
            DEV(vkEndCommandBuffer)(cb);
        }
    }
    fprintf(stderr, "nvfwd wsi: %u buffers exported to the host, pitch %u\n", NVFWD_SHOW_SLOTS, pitch);
    return 1;
}

VKAPI_ATTR VkResult VKAPI_CALL create_swapchain(VkDevice d, const VkSwapchainCreateInfoKHR *c, const VkAllocationCallbacks *a, VkSwapchainKHR *out)
{
    struct swapchain *s = calloc(1, sizeof(*s));
    s->device = d;
    s->count = c->minImageCount < 3 ? 3 : c->minImageCount;
    if (s->count > MAX_IMAGES) {
        s->count = MAX_IMAGES;
    }
    s->width = c->imageExtent.width;
    s->height = c->imageExtent.height;
    s->format = c->imageFormat;
    for (uint32_t i = 0; i < s->count; i++) {
        VkImageCreateInfo ii = { VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO, .imageType = VK_IMAGE_TYPE_2D, .format = c->imageFormat,
                                 .extent = { s->width, s->height, 1 }, .mipLevels = 1, .arrayLayers = c->imageArrayLayers,
                                 .samples = VK_SAMPLE_COUNT_1_BIT, .tiling = VK_IMAGE_TILING_OPTIMAL,
                                 .usage = c->imageUsage | VK_IMAGE_USAGE_TRANSFER_SRC_BIT, .sharingMode = VK_SHARING_MODE_EXCLUSIVE,
                                 .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED };
        VkResult r = DEV(vkCreateImage)(d, &ii, NULL, &s->image[i]);
        if (r) {
            return r;
        }
        VkMemoryRequirements m;
        DEV(vkGetImageMemoryRequirements)(d, s->image[i], &m);
        VkMemoryAllocateInfo mi = { VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .allocationSize = m.size,
                                    .memoryTypeIndex = memory_type(m.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) };
        r = DEV(vkAllocateMemory)(d, &mi, NULL, &s->memory[i]);
        if (r) {
            return r;
        }
        DEV(vkBindImageMemory)(d, s->image[i], s->memory[i], 0);
        VkFenceCreateInfo fi = { VK_STRUCTURE_TYPE_FENCE_CREATE_INFO };
        DEV(vkCreateFence)(d, &fi, NULL, &s->fence[i]);
    }
    memset(s->slot_of, -1, sizeof(s->slot_of));
    struct surface *surface = (void *)(uintptr_t)c->surface;
    if (surface->kind == 3) {
        if (!wl_setup(s, surface, c)) {
            fprintf(stderr, "nvfwd wsi: no Wayland present\n");
        }
    } else if (getenv("NVFWD_SHOW")) {
        s->show = show_setup(s, c);
    }
    *out = (VkSwapchainKHR)(uintptr_t)s;
    fprintf(stderr, "nvfwd wsi: swapchain %ux%u format %d, %u images, mode %d\n", s->width, s->height, s->format, s->count, c->presentMode);
    return VK_SUCCESS;
}

VKAPI_ATTR VkResult VKAPI_CALL swapchain_images(VkDevice d, VkSwapchainKHR handle, uint32_t *n, VkImage *out)
{
    struct swapchain *s = (void *)(uintptr_t)handle;
    if (out) {
        uint32_t c = *n < s->count ? *n : s->count;
        memcpy(out, s->image, c * sizeof(*out));
        *n = c;
        return c < s->count ? VK_INCOMPLETE : VK_SUCCESS;
    }
    *n = s->count;
    return VK_SUCCESS;
}

/* Tells the host about every copy the GPU has finished, oldest first. */
static void show_publish(struct swapchain *s, VkDevice d)
{
    for (uint32_t n = 0; n < s->count; n++) {
        uint32_t i = (s->next + n) % s->count;
        if (s->pending[i] && s->slot_of[i] >= 0 && DEV(vkGetFenceStatus)(d, s->fence[i]) == VK_SUCCESS) {
            uint64_t word = (uint64_t)s->slot_of[i] | s->present_of[i] << 8;
            ioctl(show_fd, NVFWD_SHOW_IOC_FRAME, &word);
            s->slot_of[i] = -1;
            s->shown++;
        }
    }
}

VKAPI_ATTR VkResult VKAPI_CALL acquire(VkDevice d, VkSwapchainKHR handle, uint64_t timeout, VkSemaphore semaphore, VkFence fence, uint32_t *index)
{
    struct swapchain *s = (void *)(uintptr_t)handle;
    uint32_t i = s->next;
    s->next = (i + 1) % s->count;
    if (s->pending[i]) {
        DEV(vkWaitForFences)(d, 1, &s->fence[i], VK_TRUE, UINT64_MAX);
        if (s->show) {
            show_publish(s, d);
        }
        DEV(vkResetFences)(d, 1, &s->fence[i]);
        s->pending[i] = 0;
    }
    *index = i;
    if (semaphore || fence) {
        VkSubmitInfo si = { VK_STRUCTURE_TYPE_SUBMIT_INFO, .signalSemaphoreCount = semaphore ? 1 : 0, .pSignalSemaphores = &semaphore };
        pthread_mutex_lock(&lock);
        VkResult r = DEV(vkQueueSubmit)(the_queue, 1, &si, fence);
        pthread_mutex_unlock(&lock);
        return r;
    }
    return VK_SUCCESS;
}

VKAPI_ATTR VkResult VKAPI_CALL acquire2(VkDevice d, const VkAcquireNextImageInfoKHR *i, uint32_t *index)
{
    return acquire(d, i->swapchain, i->timeout, i->semaphore, i->fence, index);
}

VKAPI_ATTR VkResult VKAPI_CALL present(VkQueue q, const VkPresentInfoKHR *info)
{
    VkPipelineStageFlags stages[16];
    for (uint32_t i = 0; i < info->waitSemaphoreCount && i < 16; i++) {
        stages[i] = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
    }
    for (uint32_t i = 0; i < info->swapchainCount; i++) {
        struct swapchain *s = (void *)(uintptr_t)info->pSwapchains[i];
        uint32_t index = info->pImageIndices[i];
        VkSubmitInfo si = { VK_STRUCTURE_TYPE_SUBMIT_INFO };
        if (i == 0) {
            si.waitSemaphoreCount = info->waitSemaphoreCount;
            si.pWaitSemaphores = info->pWaitSemaphores;
            si.pWaitDstStageMask = stages;
        }
        s->slot_of[index] = -1;
        if (s->show) {
            uint32_t slot;
            show_publish(s, s->device);
            if (ioctl(show_fd, NVFWD_SHOW_IOC_NEXT, &slot) == 0) {
                s->slot_of[index] = slot;
                s->present_of[index] = s->presents + 1;
                si.commandBufferCount = 1;
                si.pCommandBuffers = &s->copy[index][slot];
            } else {
                s->dropped++;
            }
        }
        if (s->readback) {
            si.commandBufferCount = 1;
            si.pCommandBuffers = &s->wl_copy[index];
        }
        VkResult r = DEV(vkQueueSubmit)(q, 1, &si, s->fence[index]);
        s->pending[index] = 1;
        s->presents++;
        if (s->readback && r == VK_SUCCESS) {
            DEV(vkWaitForFences)(s->device, 1, &s->fence[index], VK_TRUE, UINT64_MAX);
            wl_present(s);
        }
        if (info->pResults) {
            info->pResults[i] = r;
        }
        if (r) {
            return r;
        }
    }
    return VK_SUCCESS;
}
