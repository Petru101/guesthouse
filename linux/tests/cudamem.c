// SPDX-License-Identifier: GPL-2.0-or-later
/* The harder CUDA memory paths: pinned host memory (guest RAM reaching the GPU) and managed memory.
 * Build: gcc -std=gnu17 -O2 -o cudamem cudamem.c -ldl */
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

static const char ptx[] =
    ".version 8.0\n.target sm_80\n.address_size 64\n"
    ".visible .entry inc(.param .u64 a, .param .u32 n) {\n"
    " .reg .pred %p; .reg .b32 %r<8>; .reg .b64 %d<6>;\n"
    " ld.param.u64 %d1,[a]; ld.param.u32 %r1,[n];\n"
    " mov.u32 %r2,%ctaid.x; mov.u32 %r3,%ntid.x; mov.u32 %r4,%tid.x; mad.lo.s32 %r5,%r2,%r3,%r4;\n"
    " setp.ge.u32 %p,%r5,%r1; @%p bra DONE;\n"
    " mul.wide.u32 %d2,%r5,4; add.s64 %d3,%d1,%d2;\n"
    " ld.global.u32 %r6,[%d3]; add.u32 %r7,%r6,7; st.global.u32 [%d3],%r7;\n"
    "DONE: ret;\n}\n";

typedef int (*F)();
#define LOAD(n) F n = (F)dlsym(h, #n "_v2"); if (!n) { n = (F)dlsym(h, #n); }
#define C(x) do { int r_ = (x); printf("%s = %d\n", #x, r_); fflush(stdout); if (r_) return 1; } while (0)

static double now_ms(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec * 1e3 + t.tv_nsec / 1e6; }

static void *fn;
static F launch, sync_;

static int run(uint64_t p, unsigned n)
{
    void *args[] = { &p, &n };
    int r = launch(fn, n / 256, 1, 1, 256, 1, 1, 0, NULL, args, NULL);
    return r ? r : sync_();
}

static unsigned check(uint32_t *v, unsigned n, uint32_t add)
{
    unsigned bad = 0;
    for (unsigned i = 0; i < n; i++) {
        bad += v[i] != i + add;
    }
    return bad;
}

int main(int argc, char **argv)
{
    void *h = dlopen("libcuda.so.1", RTLD_NOW);
    LOAD(cuInit) LOAD(cuDeviceGet) LOAD(cuCtxCreate) LOAD(cuModuleLoadData) LOAD(cuModuleGetFunction)
    LOAD(cuMemHostAlloc) LOAD(cuMemHostRegister) LOAD(cuMemHostGetDevicePointer) LOAD(cuMemAllocManaged)
    LOAD(cuLaunchKernel) LOAD(cuMemcpyHtoD) LOAD(cuMemcpyDtoH) LOAD(cuMemAlloc)
    F cuCtxSynchronize = (F)dlsym(h, "cuCtxSynchronize");
    int dev;
    void *ctx, *mod;
    unsigned n = 1 << 22;
    size_t bytes = n * 4ull;
    uint64_t d;
    launch = cuLaunchKernel;
    sync_ = cuCtxSynchronize;
    C(cuInit(0));
    C(cuDeviceGet(&dev, 0));
    C(cuCtxCreate(&ctx, 0, dev));
    C(cuModuleLoadData(&mod, ptx));
    C(cuModuleGetFunction(&fn, mod, "inc"));

    if (argc < 2 || strchr(argv[1], 'h')) {
        uint32_t *p;
        C(cuMemHostAlloc(&p, bytes, 1 | 2));
        for (unsigned i = 0; i < n; i++) p[i] = i;
        C(cuMemHostGetDevicePointer(&d, p, 0));
        C(run(d, n));
        printf("cuMemHostAlloc: %u wrong of %u\n", check(p, n, 7), n);
    }
    if (argc < 2 || strchr(argv[1], 'r')) {
        uint32_t *p = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        for (unsigned i = 0; i < n; i++) p[i] = i;
        C(cuMemHostRegister(p, bytes, 2));
        C(cuMemHostGetDevicePointer(&d, p, 0));
        C(run(d, n));
        printf("cuMemHostRegister: %u wrong of %u\n", check(p, n, 7), n);
    }
    if (argc < 2 || strchr(argv[1], 'c')) {
        uint64_t a;
        uint32_t *p = malloc(bytes);
        for (unsigned i = 0; i < n; i++) p[i] = i;
        C(cuMemAlloc(&a, bytes));
        double t = now_ms();
        C(cuMemcpyHtoD(a, p, bytes));
        printf("HtoD pageable %.2f GB/s\n", bytes / (now_ms() - t) / 1e6);
        C(run(a, n));
        t = now_ms();
        C(cuMemcpyDtoH(p, a, bytes));
        printf("DtoH pageable %.2f GB/s\n", bytes / (now_ms() - t) / 1e6);
        printf("copies: %u wrong of %u\n", check(p, n, 7), n);
    }
    if (argc < 2 || strchr(argv[1], 'm')) {
        uint64_t m;
        C(cuMemAllocManaged(&m, bytes, 1));
        uint32_t *p = (uint32_t *)m;
        printf("managed at %p\n", p);
        fflush(stdout);
        double t = now_ms();
        for (unsigned i = 0; i < n; i++) p[i] = i;
        printf("CPU wrote managed in %.1f ms\n", now_ms() - t);
        t = now_ms();
        C(run(m, n));
        printf("GPU ran on managed in %.1f ms\n", now_ms() - t);
        t = now_ms();
        printf("managed: %u wrong of %u\n", check(p, n, 7), n);
        printf("CPU read back in %.1f ms\n", now_ms() - t);
        for (unsigned i = 0; i < n; i++) p[i] = p[i] + 1;
        C(run(m, n));
        printf("managed second round: %u wrong of %u\n", check(p, n, 15), n);
    }
    return 0;
}
