// SPDX-License-Identifier: MIT
// Copyright (C) 2026 xnixy
/*
 * A Vulkan layer for a compositor running on Zink over NVIDIA's driver. It corrects the calls below and does nothing else.
 * They are small assumptions in Zink that hold on Mesa's own drivers, and each could equally be a one-line change there,
 * at which point this file goes away. They happen on bare metal too, with no virtual machine involved.
 *
 * Build: gcc -O2 -Wall -shared -fPIC -o libVkLayer_zink_on_nvidia.so zink_on_nvidia.c
 * It is an implicit layer, so it loads everywhere: an environment variable would be inherited by every application started
 * from the desktop. The first two corrections are for the compositor and Xwayland alone, the third is for any Zink client.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <vulkan/vulkan.h>
#include <vulkan/vk_layer.h>

#define EXPORT __attribute__((visibility("default")))

static PFN_vkGetInstanceProcAddr next_instance_proc;
static PFN_vkGetDeviceProcAddr next_device_proc;

static int is_compositor(void)
{
    static const char *const names[] = { "gnome-shell", "mutter", "Xwayland", "Xwayland.real" };

    for (size_t i = 0; i < sizeof(names) / sizeof(names[0]); i++) {
        if (!strcmp(program_invocation_short_name, names[i])) {
            return 1;
        }
    }
    return 0;
}

/* The loader's link in a create info's chain, for this layer's place in the call order. */
static void *loader_link(const void *chain, VkStructureType type)
{
    for (VkLayerInstanceCreateInfo *link = (void *)chain; link; link = (void *)link->pNext) {
        if (link->sType == type && link->function == VK_LAYER_LINK_INFO) {
            return link;
        }
    }
    return NULL;
}

/*
 * Memory and its one dmabuf. Asked again, NVIDIA's driver makes a new dmabuf for the same memory, and two things need it to
 * stay one. A compositor scanning out a client's buffer asks every frame. And Zink attaches its fence for a frame to a dmabuf
 * it has just asked for, which is no use if that is not the dmabuf the compositor has: the compositor sees nothing to wait
 * for and reads the buffer while it is being drawn.
 */
struct imported {
    struct imported *next;
    VkDeviceMemory memory;
    int dmabuf;
};
static struct imported *imports;
static pthread_mutex_t imports_lock = PTHREAD_MUTEX_INITIALIZER;

static void remember_import(VkDeviceMemory memory, int dmabuf)
{
    struct imported *i = malloc(sizeof(*i));

    if (!i) {
        close(dmabuf);
        return;
    }
    i->memory = memory;
    i->dmabuf = dmabuf;
    pthread_mutex_lock(&imports_lock);
    i->next = imports;
    imports = i;
    pthread_mutex_unlock(&imports_lock);
}

static int dmabuf_of_import(VkDeviceMemory memory)
{
    int fd = -1;

    pthread_mutex_lock(&imports_lock);
    for (struct imported *i = imports; i; i = i->next) {
        if (i->memory == memory) {
            fd = fcntl(i->dmabuf, F_DUPFD_CLOEXEC, 0);
            break;
        }
    }
    pthread_mutex_unlock(&imports_lock);
    return fd;
}

static VKAPI_ATTR void VKAPI_CALL free_memory(VkDevice device, VkDeviceMemory memory, const VkAllocationCallbacks *allocator)
{
    pthread_mutex_lock(&imports_lock);
    for (struct imported **i = &imports; *i; i = &(*i)->next) {
        if ((*i)->memory == memory) {
            struct imported *found = *i;

            *i = found->next;
            close(found->dmabuf);
            free(found);
            break;
        }
    }
    pthread_mutex_unlock(&imports_lock);
    ((PFN_vkFreeMemory)next_device_proc(device, "vkFreeMemory"))(device, memory, allocator);
}

/*
 * Zink turns a buffer into a KMS handle by asking for an opaque fd and giving it to drmPrimeFDToHandle, as if opaque meant
 * dmabuf. In NVIDIA's driver it does not: the fd is /dev/nvidiactl and the import fails. So that request gets a dmabuf.
 */
static VKAPI_ATTR VkResult VKAPI_CALL get_memory_fd(VkDevice device, const VkMemoryGetFdInfoKHR *info, int *fd)
{
    PFN_vkGetMemoryFdKHR next = (PFN_vkGetMemoryFdKHR)next_device_proc(device, "vkGetMemoryFdKHR");

    VkMemoryGetFdInfoKHR request = *info;

    if (info->handleType == VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT && is_compositor()) {
        request.handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT;
    }
    if (request.handleType != VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT) {
        return next(device, info, fd);
    }
    *fd = dmabuf_of_import(info->memory);
    if (*fd >= 0) {
        return VK_SUCCESS;
    }
    VkResult result = next(device, &request, fd);
    if (result != VK_SUCCESS) {
        return request.handleType == info->handleType ? result : next(device, info, fd);
    }
    int kept = fcntl(*fd, F_DUPFD_CLOEXEC, 0);
    if (kept >= 0) {
        remember_import(info->memory, kept);
    }
    return VK_SUCCESS;
}

/*
 * Zink asks to export the memory it imports: VkExportMemoryAllocateInfo beside VkImportMemoryFdInfoKHR in one chain. NVIDIA's
 * driver then ignores the import and returns new, empty memory with VK_SUCCESS, so every client's buffer reads as zeros.
 * It happens with plain Vulkan too. The export is unlinked for the call, and put back.
 */
static VKAPI_ATTR VkResult VKAPI_CALL allocate_memory(VkDevice device, const VkMemoryAllocateInfo *info,
                                                      const VkAllocationCallbacks *allocator, VkDeviceMemory *memory)
{
    PFN_vkAllocateMemory next = (PFN_vkAllocateMemory)next_device_proc(device, "vkAllocateMemory");
    VkBaseOutStructure *before_export = NULL, *export = NULL;
    int dmabuf = -1;

    for (VkBaseOutStructure *link = (VkBaseOutStructure *)info; link->pNext; link = link->pNext) {
        if (link->pNext->sType == VK_STRUCTURE_TYPE_IMPORT_MEMORY_FD_INFO_KHR) {
            const VkImportMemoryFdInfoKHR *import = (const void *)link->pNext;

            if (import->handleType == VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT) {
                dmabuf = fcntl(import->fd, F_DUPFD_CLOEXEC, 0); /* the driver takes the fd it is given */
            }
        }
        if (link->pNext->sType == VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO) {
            before_export = link;
        }
    }
    if (dmabuf >= 0 && before_export) {
        export = before_export->pNext;
        before_export->pNext = export->pNext;
    }
    VkResult result = next(device, info, allocator, memory);
    if (export) {
        before_export->pNext = export;
    }
    if (dmabuf >= 0) {
        if (result == VK_SUCCESS) {
            remember_import(*memory, dmabuf);
        } else {
            close(dmabuf);
        }
    }
    return result;
}

static VKAPI_ATTR VkResult VKAPI_CALL create_instance(const VkInstanceCreateInfo *info, const VkAllocationCallbacks *allocator,
                                                      VkInstance *instance)
{
    VkLayerInstanceCreateInfo *link = loader_link(info->pNext, VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO);

    if (!link) {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    next_instance_proc = link->u.pLayerInfo->pfnNextGetInstanceProcAddr;
    link->u.pLayerInfo = link->u.pLayerInfo->pNext;
    return ((PFN_vkCreateInstance)next_instance_proc(NULL, "vkCreateInstance"))(info, allocator, instance);
}

static VKAPI_ATTR VkResult VKAPI_CALL create_device(VkPhysicalDevice gpu, const VkDeviceCreateInfo *info,
                                                    const VkAllocationCallbacks *allocator, VkDevice *device)
{
    VkLayerDeviceCreateInfo *link = loader_link(info->pNext, VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO);

    if (!link) {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    PFN_vkGetInstanceProcAddr instance_proc = link->u.pLayerInfo->pfnNextGetInstanceProcAddr;

    next_device_proc = link->u.pLayerInfo->pfnNextGetDeviceProcAddr;
    link->u.pLayerInfo = link->u.pLayerInfo->pNext;
    return ((PFN_vkCreateDevice)instance_proc(NULL, "vkCreateDevice"))(gpu, info, allocator, device);
}

static VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL get_device_proc(VkDevice device, const char *name)
{
    if (!strcmp(name, "vkGetDeviceProcAddr")) {
        return (PFN_vkVoidFunction)get_device_proc;
    }
    if (!strcmp(name, "vkGetMemoryFdKHR")) {
        return (PFN_vkVoidFunction)get_memory_fd;
    }
    if (!strcmp(name, "vkFreeMemory")) {
        return (PFN_vkVoidFunction)free_memory;
    }
    if (!strcmp(name, "vkAllocateMemory") && is_compositor()) {
        return (PFN_vkVoidFunction)allocate_memory;
    }
    return next_device_proc(device, name);
}

static VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL get_instance_proc(VkInstance instance, const char *name)
{
    if (!strcmp(name, "vkGetInstanceProcAddr")) {
        return (PFN_vkVoidFunction)get_instance_proc;
    }
    if (!strcmp(name, "vkGetDeviceProcAddr")) {
        return (PFN_vkVoidFunction)get_device_proc;
    }
    if (!strcmp(name, "vkCreateInstance")) {
        return (PFN_vkVoidFunction)create_instance;
    }
    if (!strcmp(name, "vkCreateDevice")) {
        return (PFN_vkVoidFunction)create_device;
    }
    return next_instance_proc ? next_instance_proc(instance, name) : NULL;
}

EXPORT VKAPI_ATTR VkResult VKAPI_CALL vkNegotiateLoaderLayerInterfaceVersion(VkNegotiateLayerInterface *version)
{
    if (version->loaderLayerInterfaceVersion > 2) {
        version->loaderLayerInterfaceVersion = 2;
    }
    version->pfnGetInstanceProcAddr = get_instance_proc;
    version->pfnGetDeviceProcAddr = get_device_proc;
    version->pfnGetPhysicalDeviceProcAddr = NULL;
    return VK_SUCCESS;
}
