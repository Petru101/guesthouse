// SPDX-License-Identifier: GPL-2.0-or-later
/* cuInit, then a vector add from PTX through the driver API, with every value checked.
 * Build: gcc -std=gnu17 -O2 -o cudavec cudavec.c -ldl */
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>

static const char ptx[] =
    ".version 8.0\n.target sm_80\n.address_size 64\n"
    ".visible .entry add(.param .u64 a, .param .u64 b, .param .u64 c, .param .u32 n) {\n"
    " .reg .pred %p; .reg .f32 %f<4>; .reg .b32 %r<6>; .reg .b64 %d<10>;\n"
    " ld.param.u64 %d1,[a]; ld.param.u64 %d2,[b]; ld.param.u64 %d3,[c]; ld.param.u32 %r1,[n];\n"
    " mov.u32 %r2,%ctaid.x; mov.u32 %r3,%ntid.x; mov.u32 %r4,%tid.x; mad.lo.s32 %r5,%r2,%r3,%r4;\n"
    " setp.ge.u32 %p,%r5,%r1; @%p bra DONE;\n"
    " mul.wide.u32 %d4,%r5,4; add.s64 %d5,%d1,%d4; add.s64 %d6,%d2,%d4; add.s64 %d7,%d3,%d4;\n"
    " ld.global.f32 %f1,[%d5]; ld.global.f32 %f2,[%d6]; add.f32 %f3,%f1,%f2; st.global.f32 [%d7],%f3;\n"
    "DONE: ret;\n}\n";

typedef int (*F)();
#define LOAD(n) F n = (F)dlsym(h, #n "_v2"); if (!n) { n = (F)dlsym(h, #n); }
#define C(x) do { int r_ = (x); printf("%s = %d\n", #x, r_); if (r_) return 1; } while (0)

int main(void)
{
    void *h = dlopen("libcuda.so.1", RTLD_NOW);
    if (!h) { printf("%s\n", dlerror()); return 1; }
    LOAD(cuInit) LOAD(cuDeviceGet) LOAD(cuDeviceGetName) LOAD(cuCtxCreate) LOAD(cuModuleLoadData)
    LOAD(cuModuleGetFunction) LOAD(cuMemAlloc) LOAD(cuMemcpyHtoD) LOAD(cuMemcpyDtoH)
    LOAD(cuLaunchKernel) F cuCtxSynchronize = (F)dlsym(h, "cuCtxSynchronize"); LOAD(cuDriverGetVersion) LOAD(cuDeviceGetCount)
    int dev, ver, count;
    char name[128];
    void *ctx, *mod, *fn;
    uint64_t a, b, c;
    unsigned n = 1 << 22;
    size_t bytes = n * 4ull;
    float *x = malloc(bytes), *y = malloc(bytes), *z = malloc(bytes);

    C(cuInit(0));
    cuDriverGetVersion(&ver);
    C(cuDeviceGetCount(&count));
    printf("driver %d, %d devices\n", ver, count);
    C(cuDeviceGet(&dev, 0));
    C(cuDeviceGetName(name, sizeof(name), dev));
    printf("name: %s\n", name);
    C(cuCtxCreate(&ctx, 0, dev));
    /* For processes alive at once: HOLD seconds before and after the work, and SEED so each has its own answer. */
    unsigned hold = getenv("HOLD") ? atoi(getenv("HOLD")) : 0;
    float seed = getenv("SEED") ? atof(getenv("SEED")) : 0;
    sleep(hold);
    C(cuModuleLoadData(&mod, ptx));
    C(cuModuleGetFunction(&fn, mod, "add"));
    C(cuMemAlloc(&a, bytes));
    C(cuMemAlloc(&b, bytes));
    C(cuMemAlloc(&c, bytes));
    for (unsigned i = 0; i < n; i++) { x[i] = i * 0.5f + seed; y[i] = 1000.0f - i * seed; }
    C(cuMemcpyHtoD(a, x, bytes));
    C(cuMemcpyHtoD(b, y, bytes));
    void *args[] = { &a, &b, &c, &n };
    C(cuLaunchKernel(fn, n / 256, 1, 1, 256, 1, 1, 0, NULL, args, NULL));
    C(cuCtxSynchronize());
    C(cuMemcpyDtoH(z, c, bytes));
    unsigned bad = 0;
    for (unsigned i = 0; i < n; i++) {
        if (z[i] != x[i] + y[i]) {
            if (bad++ < 5) printf(" bad at %u: %f, expected %f\n", i, z[i], x[i] + y[i]);
        }
    }
    printf("seed %g: checked %u values, %u wrong\n", seed, n, bad);
    fflush(stdout);
    sleep(hold);
    if (getenv("HOLD")) sleep(atoi(getenv("HOLD")));
    return !!bad;
}
