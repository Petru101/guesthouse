/* A D3D12 window that asks Windows for HDR: an FP16 swap chain tagged scRGB, cleared to colours brighter than SDR white.
 * Usage: d12hdr SECONDS OUTPUT [sdr|hdr]
 *
 * With sdr or hdr it shows still bars of known colour instead of a ramp, to look for on the host.
 *
 * It only clears, so it needs no shader compiler. What matters is what it writes to OUTPUT:
 * every output's colour space as DXGI sees it, and whether the swap chain took scRGB.
 */
#define COBJMACROS
#define WIDL_C_INLINE_WRAPPERS
#define INITGUID
#include <initguid.h>
#include <windows.h>
#include <d3d12.h>
#include <dxgi1_6.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BUFFERS 2
#define WAIT_MS 5000
/* In scRGB 1.0 is 80 nits, so this is 1,000 nits. */
#define BRIGHTEST 12.5f
#define RAMP_FRAMES 240

typedef struct {
    float red, green, blue;
} Bar;

#define BAR_COUNT 6
/* In scRGB. SDR stays within 0 to 1. For HDR, these stop at 400 nits, under the display's peak, so that Windows has no reason to tone map. */
static const Bar SDR_BARS[BAR_COUNT] = {
    { 1.0f, 0.0f, 0.0f }, { 0.0f, 1.0f, 0.0f }, { 0.0f, 0.0f, 1.0f },
    { 1.0f, 1.0f, 1.0f }, { 0.5f, 0.5f, 0.5f }, { 0.0f, 0.0f, 0.0f },
};
static const Bar HDR_BARS[BAR_COUNT] = {
    { 1.0f, 1.0f, 1.0f }, { 2.5f, 2.5f, 2.5f }, { 5.0f, 5.0f, 5.0f },
    { 5.0f, 0.0f, 0.0f }, { 0.0f, 5.0f, 0.0f }, { 0.0f, 0.0f, 0.0f },
};

static FILE *out;

static void say(const char *format, ...)
{
    va_list args;
    va_start(args, format);
    vfprintf(out, format, args);
    va_end(args);
    fputc('\n', out);
    fflush(out);
}

#define CHECK(call) do { HRESULT hr_ = (call); if (FAILED(hr_)) { say("failed: %s: 0x%08lx", #call, hr_); exit(1); } } while (0)

static void describe_outputs(IDXGIFactory4 *factory)
{
    IDXGIAdapter1 *adapter;
    UINT a, o;

    for (a = 0; IDXGIFactory4_EnumAdapters1(factory, a, &adapter) == S_OK; a++) {
        DXGI_ADAPTER_DESC1 adapter_desc;
        IDXGIOutput *output;

        IDXGIAdapter1_GetDesc1(adapter, &adapter_desc);
        say("adapter %u: %ls", a, adapter_desc.Description);
        for (o = 0; IDXGIAdapter1_EnumOutputs(adapter, o, &output) == S_OK; o++) {
            IDXGIOutput6 *output6;
            DXGI_OUTPUT_DESC1 desc;

            if (FAILED(IDXGIOutput_QueryInterface(output, &IID_IDXGIOutput6, (void **)&output6))) {
                say("  output %u: no IDXGIOutput6", o);
                continue;
            }
            IDXGIOutput6_GetDesc1(output6, &desc);
            say("  output %u: %ls colour space %d, %u bits, %.0f nits peak, %.0f full frame", o, desc.DeviceName,
                desc.ColorSpace, desc.BitsPerColor, desc.MaxLuminance, desc.MaxFullFrameLuminance);
            IDXGIOutput6_Release(output6);
            IDXGIOutput_Release(output);
        }
        IDXGIAdapter1_Release(adapter);
    }
}

static IDXGIAdapter1 *render_adapter(IDXGIFactory4 *factory)
{
    IDXGIAdapter1 *adapter;
    UINT i;

    for (i = 0; IDXGIFactory4_EnumAdapters1(factory, i, &adapter) == S_OK; i++) {
        DXGI_ADAPTER_DESC1 desc;
        IDXGIAdapter1_GetDesc1(adapter, &desc);
        if (!(desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) && wcsstr(desc.Description, L"NVIDIA"))
            return adapter;
        IDXGIAdapter1_Release(adapter);
    }
    say("failed: no NVIDIA adapter");
    exit(1);
}

static void transition(ID3D12GraphicsCommandList *list, ID3D12Resource *resource, D3D12_RESOURCE_STATES before,
                       D3D12_RESOURCE_STATES after)
{
    D3D12_RESOURCE_BARRIER b;

    memset(&b, 0, sizeof(b));
    b.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    b.Transition.pResource = resource;
    b.Transition.Subresource = D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES;
    b.Transition.StateBefore = before;
    b.Transition.StateAfter = after;
    ID3D12GraphicsCommandList_ResourceBarrier(list, 1, &b);
}

static void clear_to_bars(ID3D12GraphicsCommandList *list, D3D12_CPU_DESCRIPTOR_HANDLE view, const Bar *bars,
                          LONG width, LONG height)
{
    int i;

    for (i = 0; i < BAR_COUNT; i++) {
        float colour[4] = { bars[i].red, bars[i].green, bars[i].blue, 1.0f };
        D3D12_RECT rectangle = { width * i / BAR_COUNT, 0, width * (i + 1) / BAR_COUNT, height };
        ID3D12GraphicsCommandList_ClearRenderTargetView(list, view, colour, 1, &rectangle);
    }
}

static HWND make_window(void)
{
    WNDCLASSW window_class;

    memset(&window_class, 0, sizeof(window_class));
    window_class.lpfnWndProc = DefWindowProcW;
    window_class.hInstance = GetModuleHandleW(NULL);
    window_class.lpszClassName = L"d12hdr";
    RegisterClassW(&window_class);
    return CreateWindowExW(WS_EX_TOPMOST, L"d12hdr", L"d12hdr", WS_POPUP | WS_VISIBLE, 0, 0,
                           GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN), NULL, NULL,
                           window_class.hInstance, NULL);
}

int main(int argc, char **argv)
{
    IDXGIFactory4 *factory;
    IDXGIAdapter1 *adapter;
    ID3D12Device *device;
    ID3D12CommandQueue *queue;
    ID3D12CommandAllocator *allocator;
    ID3D12GraphicsCommandList *list;
    ID3D12DescriptorHeap *heap;
    ID3D12Fence *fence;
    ID3D12Resource *buffers[BUFFERS];
    IDXGISwapChain1 *chain1;
    IDXGISwapChain3 *chain;
    D3D12_COMMAND_QUEUE_DESC queue_desc;
    D3D12_DESCRIPTOR_HEAP_DESC heap_desc;
    DXGI_SWAP_CHAIN_DESC1 desc;
    D3D12_CPU_DESCRIPTOR_HANDLE first;
    HANDLE done;
    HWND window;
    UINT support = 0, step, i;
    UINT64 fence_value = 0, frames = 0;
    ULONGLONG started;
    DWORD seconds;
    const Bar *bars = NULL;

    if (argc < 3)
        return 2;
    seconds = (DWORD)atoi(argv[1]);
    out = fopen(argv[2], "w");
    if (!out)
        return 2;
    if (argc > 3)
        bars = strcmp(argv[3], "hdr") == 0 ? HDR_BARS : SDR_BARS;

    CHECK(CreateDXGIFactory2(0, &IID_IDXGIFactory4, (void **)&factory));
    say("before the swap chain:");
    describe_outputs(factory);
    adapter = render_adapter(factory);
    CHECK(D3D12CreateDevice((IUnknown *)adapter, D3D_FEATURE_LEVEL_11_0, &IID_ID3D12Device, (void **)&device));

    memset(&queue_desc, 0, sizeof(queue_desc));
    queue_desc.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
    CHECK(ID3D12Device_CreateCommandQueue(device, &queue_desc, &IID_ID3D12CommandQueue, (void **)&queue));
    CHECK(ID3D12Device_CreateCommandAllocator(device, D3D12_COMMAND_LIST_TYPE_DIRECT, &IID_ID3D12CommandAllocator,
                                              (void **)&allocator));
    CHECK(ID3D12Device_CreateCommandList(device, 0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator, NULL,
                                         &IID_ID3D12GraphicsCommandList, (void **)&list));
    CHECK(ID3D12GraphicsCommandList_Close(list));
    CHECK(ID3D12Device_CreateFence(device, 0, D3D12_FENCE_FLAG_NONE, &IID_ID3D12Fence, (void **)&fence));
    done = CreateEventW(NULL, FALSE, FALSE, NULL);

    window = make_window();
    memset(&desc, 0, sizeof(desc));
    desc.Format = DXGI_FORMAT_R16G16B16A16_FLOAT;
    desc.SampleDesc.Count = 1;
    desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
    desc.BufferCount = BUFFERS;
    desc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
    CHECK(IDXGIFactory4_CreateSwapChainForHwnd(factory, (IUnknown *)queue, window, &desc, NULL, NULL, &chain1));
    CHECK(IDXGISwapChain1_QueryInterface(chain1, &IID_IDXGISwapChain3, (void **)&chain));

    CHECK(IDXGISwapChain3_CheckColorSpaceSupport(chain, DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709, &support));
    say("scRGB support flags: 0x%x", support);
    CHECK(IDXGISwapChain3_SetColorSpace1(chain, DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709));
    say("swap chain set to scRGB");

    memset(&heap_desc, 0, sizeof(heap_desc));
    heap_desc.Type = D3D12_DESCRIPTOR_HEAP_TYPE_RTV;
    heap_desc.NumDescriptors = BUFFERS;
    CHECK(ID3D12Device_CreateDescriptorHeap(device, &heap_desc, &IID_ID3D12DescriptorHeap, (void **)&heap));
    step = ID3D12Device_GetDescriptorHandleIncrementSize(device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV);
    first = ID3D12DescriptorHeap_GetCPUDescriptorHandleForHeapStart(heap);
    for (i = 0; i < BUFFERS; i++) {
        D3D12_CPU_DESCRIPTOR_HANDLE view = first;
        view.ptr += (SIZE_T)i * step;
        CHECK(IDXGISwapChain3_GetBuffer(chain, i, &IID_ID3D12Resource, (void **)&buffers[i]));
        ID3D12Device_CreateRenderTargetView(device, buffers[i], NULL, view);
    }

    started = GetTickCount64();
    while (GetTickCount64() - started < (ULONGLONG)seconds * 1000) {
        UINT index = IDXGISwapChain3_GetCurrentBackBufferIndex(chain);
        D3D12_CPU_DESCRIPTOR_HANDLE view = first;
        float level = 1.0f + (BRIGHTEST - 1.0f) * (float)(frames % RAMP_FRAMES) / RAMP_FRAMES;
        float colour[4] = { level, level * 0.25f, level * 0.25f, 1.0f };

        view.ptr += (SIZE_T)index * step;
        CHECK(ID3D12CommandAllocator_Reset(allocator));
        CHECK(ID3D12GraphicsCommandList_Reset(list, allocator, NULL));
        transition(list, buffers[index], D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET);
        if (bars)
            clear_to_bars(list, view, bars, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        else
            ID3D12GraphicsCommandList_ClearRenderTargetView(list, view, colour, 0, NULL);
        transition(list, buffers[index], D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT);
        CHECK(ID3D12GraphicsCommandList_Close(list));
        ID3D12CommandQueue_ExecuteCommandLists(queue, 1, (ID3D12CommandList **)&list);
        CHECK(IDXGISwapChain3_Present(chain, 1, 0));

        fence_value++;
        CHECK(ID3D12CommandQueue_Signal(queue, fence, fence_value));
        if (ID3D12Fence_GetCompletedValue(fence) < fence_value) {
            CHECK(ID3D12Fence_SetEventOnCompletion(fence, fence_value, done));
            if (WaitForSingleObject(done, WAIT_MS) != WAIT_OBJECT_0) {
                say("failed: the GPU did not finish a frame in %d ms, after %llu frames", WAIT_MS, frames);
                return 1;
            }
        }
        frames++;
    }

    say("presented %llu frames in %lu s", frames, seconds);
    say("after the swap chain:");
    describe_outputs(factory);
    say("finished");
    return 0;
}
