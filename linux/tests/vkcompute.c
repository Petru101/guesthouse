// SPDX-License-Identifier: GPL-2.0-or-later
/* Real work on the guest's GPU: a compute shader over N floats, every result checked on the CPU,
 * then the time of an empty and a one-buffer submission with a fence.
 * Build: glslangValidator -V --vn shader_spv -o shader.h shader.comp; gcc -O2 -o vkcompute vkcompute.c -lvulkan
 */
#include <vulkan/vulkan.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "shader.h"

#define CHECK(c) do { VkResult r_ = (c); if (r_) { printf("FAILED %s: %d\n", #c, r_); exit(1); } } while (0)
#define N (1u << 24)

static double now_us(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e6 + t.tv_nsec / 1e3;
}

static int cmp(const void *a, const void *b)
{
    double x = *(double *)a, y = *(double *)b;
    return x < y ? -1 : x > y;
}

int main(void)
{
    VkInstanceCreateInfo ii = { VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO };
    VkApplicationInfo ai = { VK_STRUCTURE_TYPE_APPLICATION_INFO, .apiVersion = VK_API_VERSION_1_2 };
    VkInstance inst;
    ii.pApplicationInfo = &ai;
    CHECK(vkCreateInstance(&ii, NULL, &inst));
    uint32_t n = 8;
    VkPhysicalDevice pds[8], pd = 0;
    CHECK(vkEnumeratePhysicalDevices(inst, &n, pds));
    VkPhysicalDeviceProperties pp;
    for (uint32_t i = 0; i < n; i++) {
        vkGetPhysicalDeviceProperties(pds[i], &pp);
        printf("device %u: %s vendor %04x driver %u.%u api %u.%u\n", i, pp.deviceName, pp.vendorID,
               pp.driverVersion >> 22, (pp.driverVersion >> 14) & 255, VK_API_VERSION_MAJOR(pp.apiVersion),
               VK_API_VERSION_MINOR(pp.apiVersion));
        if (pp.vendorID == 0x10de) {
            pd = pds[i];
        }
    }
    if (!pd) {
        printf("no NVIDIA device\n");
        return 1;
    }
    uint32_t qf = 0, qn = 16;
    VkQueueFamilyProperties qp[16];
    vkGetPhysicalDeviceQueueFamilyProperties(pd, &qn, qp);
    for (qf = 0; qf < qn && !(qp[qf].queueFlags & VK_QUEUE_COMPUTE_BIT); qf++) {
    }
    float prio = 1;
    VkDeviceQueueCreateInfo qi = { VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO, .queueFamilyIndex = qf,
                                   .queueCount = 1, .pQueuePriorities = &prio };
    VkDeviceCreateInfo di = { VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO, .queueCreateInfoCount = 1, .pQueueCreateInfos = &qi };
    VkDevice dev;
    CHECK(vkCreateDevice(pd, &di, NULL, &dev));
    printf("device created\n");
    VkQueue q;
    vkGetDeviceQueue(dev, qf, 0, &q);

    VkBufferCreateInfo bi = { VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO, .size = N * 4ull,
                              .usage = VK_BUFFER_USAGE_STORAGE_BUFFER_BIT };
    VkBuffer buf;
    CHECK(vkCreateBuffer(dev, &bi, NULL, &buf));
    VkMemoryRequirements mr;
    vkGetBufferMemoryRequirements(dev, buf, &mr);
    VkPhysicalDeviceMemoryProperties mp;
    vkGetPhysicalDeviceMemoryProperties(pd, &mp);
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        printf("memory type %u: flags %#x heap %u (%llu MiB)\n", i, mp.memoryTypes[i].propertyFlags,
               mp.memoryTypes[i].heapIndex,
               (unsigned long long)mp.memoryHeaps[mp.memoryTypes[i].heapIndex].size >> 20);
    }
    const char *want = getenv("MEMFLAGS");
    uint32_t need = want ? strtoul(want, 0, 0) : VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT;
    uint32_t mt = ~0u;
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        if ((mr.memoryTypeBits >> i & 1) && (mp.memoryTypes[i].propertyFlags & need) == need) {
            mt = i;
            break;
        }
    }
    printf("using memory type %u\n", mt);
    VkMemoryAllocateInfo ma = { VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .allocationSize = mr.size, .memoryTypeIndex = mt };
    VkDeviceMemory mem;
    CHECK(vkAllocateMemory(dev, &ma, NULL, &mem));
    CHECK(vkBindBufferMemory(dev, buf, mem, 0));
    float *p;
    CHECK(vkMapMemory(dev, mem, 0, VK_WHOLE_SIZE, 0, (void **)&p));
    double t0 = now_us();
    for (uint32_t i = 0; i < N; i++) {
        p[i] = (float)(i & 0xffff);
    }
    printf("CPU write %.0f MiB: %.2f GB/s\n", N * 4.0 / 1048576, N * 4.0 / (now_us() - t0) / 1e3);
    t0 = now_us();
    for (uint32_t i = 0; i < N; i++) {
        p[i] = (float)(i & 0xffff);
    }
    printf("CPU write again: %.2f GB/s\n", N * 4.0 / (now_us() - t0) / 1e3);

    VkShaderModuleCreateInfo si = { VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO, .codeSize = sizeof(shader_spv), .pCode = shader_spv };
    VkShaderModule sm;
    CHECK(vkCreateShaderModule(dev, &si, NULL, &sm));
    VkDescriptorSetLayoutBinding lb = { 0, VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, 1, VK_SHADER_STAGE_COMPUTE_BIT, NULL };
    VkDescriptorSetLayoutCreateInfo li = { VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO, .bindingCount = 1, .pBindings = &lb };
    VkDescriptorSetLayout dl;
    CHECK(vkCreateDescriptorSetLayout(dev, &li, NULL, &dl));
    VkPipelineLayoutCreateInfo pli = { VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO, .setLayoutCount = 1, .pSetLayouts = &dl };
    VkPipelineLayout pl;
    CHECK(vkCreatePipelineLayout(dev, &pli, NULL, &pl));
    VkComputePipelineCreateInfo ci = { VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO, .layout = pl };
    ci.stage = (VkPipelineShaderStageCreateInfo){ VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                                                  .stage = VK_SHADER_STAGE_COMPUTE_BIT, .module = sm, .pName = "main" };
    VkPipeline pipe;
    CHECK(vkCreateComputePipelines(dev, VK_NULL_HANDLE, 1, &ci, NULL, &pipe));
    printf("pipeline created\n");
    VkDescriptorPoolSize ps = { VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, 1 };
    VkDescriptorPoolCreateInfo dpi = { VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO, .maxSets = 1, .poolSizeCount = 1, .pPoolSizes = &ps };
    VkDescriptorPool dp;
    CHECK(vkCreateDescriptorPool(dev, &dpi, NULL, &dp));
    VkDescriptorSetAllocateInfo dai = { VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO, .descriptorPool = dp, .descriptorSetCount = 1, .pSetLayouts = &dl };
    VkDescriptorSet ds;
    CHECK(vkAllocateDescriptorSets(dev, &dai, &ds));
    VkDescriptorBufferInfo dbi = { buf, 0, VK_WHOLE_SIZE };
    VkWriteDescriptorSet w = { VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET, .dstSet = ds, .descriptorCount = 1,
                               .descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, .pBufferInfo = &dbi };
    vkUpdateDescriptorSets(dev, 1, &w, 0, NULL);
    VkCommandPoolCreateInfo cpi = { VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, .queueFamilyIndex = qf };
    VkCommandPool cp;
    CHECK(vkCreateCommandPool(dev, &cpi, NULL, &cp));
    VkCommandBufferAllocateInfo cai = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, .commandPool = cp, .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY, .commandBufferCount = 2 };
    VkCommandBuffer cb[2];
    CHECK(vkAllocateCommandBuffers(dev, &cai, cb));
    VkCommandBufferBeginInfo cbi = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO };
    CHECK(vkBeginCommandBuffer(cb[0], &cbi));
    vkCmdBindPipeline(cb[0], VK_PIPELINE_BIND_POINT_COMPUTE, pipe);
    vkCmdBindDescriptorSets(cb[0], VK_PIPELINE_BIND_POINT_COMPUTE, pl, 0, 1, &ds, 0, NULL);
    vkCmdDispatch(cb[0], N / 256, 1, 1);
    CHECK(vkEndCommandBuffer(cb[0]));
    CHECK(vkBeginCommandBuffer(cb[1], &cbi));
    CHECK(vkEndCommandBuffer(cb[1]));
    VkFenceCreateInfo fi = { VK_STRUCTURE_TYPE_FENCE_CREATE_INFO };
    VkFence f;
    CHECK(vkCreateFence(dev, &fi, NULL, &f));
    VkSubmitInfo sb = { VK_STRUCTURE_TYPE_SUBMIT_INFO, .commandBufferCount = 1, .pCommandBuffers = &cb[0] };
    t0 = now_us();
    CHECK(vkQueueSubmit(q, 1, &sb, f));
    VkResult fr = vkWaitForFences(dev, 1, &f, VK_TRUE, 5000000000ull);
    printf("dispatch and fence: %d, %.0f us\n", fr, now_us() - t0);
    if (fr) {
        return 1;
    }
    uint32_t bad = 0;
    t0 = now_us();
    for (uint32_t i = 0; i < N; i++) {
        float x = (float)(i & 0xffff), e = x * 2.0f + 1.0f;
        if (p[i] != e) {
            if (bad++ < 5) {
                printf("  bad at %u: %f expected %f\n", i, p[i], e);
            }
        }
    }
    printf("CPU check read: %.2f GB/s\n", N * 4.0 / (now_us() - t0) / 1e3);
    printf("checked %u values, %u wrong\n", N, bad);
    if (getenv("NO_BENCH")) {
        return !!bad;
    }

    enum { RUNS = 2000 };
    static double d[RUNS];
    for (int kind = 0; kind < 2; kind++) {
        VkSubmitInfo s2 = { VK_STRUCTURE_TYPE_SUBMIT_INFO, .commandBufferCount = (uint32_t)kind, .pCommandBuffers = &cb[1] };
        for (int i = 0; i < RUNS; i++) {
            CHECK(vkResetFences(dev, 1, &f));
            double a = now_us();
            CHECK(vkQueueSubmit(q, 1, &s2, f));
            CHECK(vkWaitForFences(dev, 1, &f, VK_TRUE, 5000000000ull));
            d[i] = now_us() - a;
        }
        qsort(d, RUNS, sizeof(double), cmp);
        printf("submit %s + fence: median %.1f us, p99 %.1f us\n", kind ? "one empty buffer" : "no buffers", d[RUNS / 2], d[RUNS * 99 / 100]);
    }
    vkDeviceWaitIdle(dev);
    return !!bad;
}
