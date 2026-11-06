/* Checks Desktop Duplication against d12hdr's bars, on every pixel of every frame, with nothing else in between. Run d12hdr first.
 * Usage: dupcheck sdr|hdr [SECONDS]
 *
 * It takes frames for that long, recreating the duplication whenever Windows says access was lost. In HDR it asks for the
 * floating point desktop. No pixel may be wrong, but for where the cursor is, which it asks Windows about for each frame.
 * It says PASSED or FAILED, and how many frames it judged. Judging takes time, so this is no measure of speed.
 */
#define COBJMACROS
#include <windows.h>
#include <d3d11.h>
#include <dxgi1_6.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BAR_COUNT 6
#define WAIT_MS 200
#define DEFAULT_SECONDS 5
/* Around the cursor's picture, as it may have moved a little since the frame. */
#define CURSOR_MARGIN 2
/* A column to leave out, for the edges below. */
#define NO_BAR (-1)
/* One step of 8 bits, and what half a percent of the brightest bar is in floats. */
#define SDR_TOLERANCE 1
#define HDR_TOLERANCE 0.03f
#define MOST_RESULTS 8

/* Keep these in step with SDR_BARS and HDR_BARS in d12hdr.c. SDR's are as the 8-bit desktop has them, blue first. */
static const unsigned char SDR_WANTED[BAR_COUNT][3] = {
    {0, 0, 255}, {0, 255, 0}, {255, 0, 0}, {255, 255, 255}, {188, 188, 188}, {0, 0, 0},
};
static const float HDR_WANTED[BAR_COUNT][3] = {
    {1, 1, 1}, {2.5f, 2.5f, 2.5f}, {5, 5, 5}, {5, 0, 0}, {0, 5, 0}, {0, 0, 0},
};

typedef struct {
    HRESULT result;
    unsigned count;
} Tally;

static Tally results[MOST_RESULTS];

static void count_result(HRESULT result) {
    for (int i = 0; i < MOST_RESULTS; i++) {
        if (results[i].count == 0 || results[i].result == result) {
            results[i].result = result;
            results[i].count++;
            return;
        }
    }
}

static float half_to_float(unsigned short half) {
    int sign = half >> 15, exponent = (half >> 10) & 31, mantissa = half & 1023;
    float value = exponent ? ldexpf((float)(mantissa | 1024), exponent - 25) : ldexpf((float)mantissa, -24);
    return sign ? -value : value;
}

static float half_values[1 << 16];

static int sdr_pixel_is_right(const unsigned char *pixel, int bar) {
    for (int channel = 0; channel < 3; channel++) {
        if (abs(pixel[channel] - SDR_WANTED[bar][channel]) > SDR_TOLERANCE) return 0;
    }
    return 1;
}

static int hdr_pixel_is_right(const unsigned short *pixel, int bar) {
    for (int channel = 0; channel < 3; channel++) {
        if (fabsf(half_values[pixel[channel]] - HDR_WANTED[bar][channel]) > HDR_TOLERANCE) return 0;
    }
    return 1;
}

/* d12hdr's rectangles start at width * i / BAR_COUNT, rounded down, so this has to round the same way. */
static int bar_at(UINT x, UINT width) {
    int bar = BAR_COUNT - 1;
    while (x < width * (unsigned long long)bar / BAR_COUNT) bar--;
    return bar;
}

/* Which bar each column should show. d12hdr isn't DPI aware, so at a width the bars don't divide Windows may stretch it,
 * and the column on each side of an edge is then a blend. Only there are those columns left out. */
static int *columns_for(UINT width) {
    int *columns = malloc(width * sizeof *columns);
    for (UINT x = 0; x < width; x++) columns[x] = bar_at(x, width);
    if (width % BAR_COUNT == 0) return columns;
    int *kept = malloc(width * sizeof *kept);
    for (UINT x = 0; x < width; x++) {
        int edge = (x > 0 && columns[x - 1] != columns[x]) || (x + 1 < width && columns[x + 1] != columns[x]);
        kept[x] = edge ? NO_BAR : columns[x];
    }
    free(columns);
    return kept;
}

/* Where the cursor is drawn, or an empty rectangle if it isn't showing. */
static RECT cursor_rectangle(void) {
    RECT empty = {0, 0, 0, 0}, place;
    CURSORINFO cursor = {sizeof cursor};
    ICONINFO icon;
    BITMAP picture;
    if (!GetCursorInfo(&cursor) || !(cursor.flags & CURSOR_SHOWING) || !GetIconInfo(cursor.hCursor, &icon)) return empty;
    GetObject(icon.hbmColor ? icon.hbmColor : icon.hbmMask, sizeof picture, &picture);
    int height = icon.hbmColor ? picture.bmHeight : picture.bmHeight / 2;
    place.left = cursor.ptScreenPos.x - (LONG)icon.xHotspot - CURSOR_MARGIN;
    place.top = cursor.ptScreenPos.y - (LONG)icon.yHotspot - CURSOR_MARGIN;
    place.right = place.left + picture.bmWidth + 2 * CURSOR_MARGIN;
    place.bottom = place.top + height + 2 * CURSOR_MARGIN;
    if (icon.hbmColor) DeleteObject(icon.hbmColor);
    if (icon.hbmMask) DeleteObject(icon.hbmMask);
    return place;
}

static long count_wrong_pixels(const unsigned char *data, UINT pitch, UINT width, UINT height, int hdr, const int *columns, const RECT *cursor) {
    long wrong = 0;
    for (UINT y = 0; y < height; y++) {
        const unsigned char *row = data + (size_t)y * pitch;
        int cursor_row = (LONG)y >= cursor->top && (LONG)y < cursor->bottom;
        for (UINT x = 0; x < width; x++) {
            if (columns[x] == NO_BAR || (cursor_row && (LONG)x >= cursor->left && (LONG)x < cursor->right)) continue;
            int right = hdr ? hdr_pixel_is_right((const unsigned short *)(row + x * 8), columns[x]) : sdr_pixel_is_right(row + x * 4, columns[x]);
            wrong += !right;
        }
    }
    return wrong;
}

static IDXGIAdapter1 *adapter_with_output(IDXGIFactory1 *factory, IDXGIOutput **output) {
    IDXGIAdapter1 *adapter;
    for (UINT i = 0; IDXGIFactory1_EnumAdapters1(factory, i, &adapter) == S_OK; i++) {
        if (IDXGIAdapter1_EnumOutputs(adapter, 0, output) == S_OK) return adapter;
    }
    return NULL;
}

static HRESULT duplicate(IDXGIOutput *output, ID3D11Device *device, int hdr, IDXGIOutputDuplication **out) {
    IDXGIOutput5 *output5;
    HRESULT result = IDXGIOutput_QueryInterface(output, &IID_IDXGIOutput5, (void **)&output5);
    if (FAILED(result)) return result;
    DXGI_FORMAT formats[] = {DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM};
    return IDXGIOutput5_DuplicateOutput1(output5, (IUnknown *)device, 0, hdr ? 2 : 1, hdr ? formats : formats + 1, out);
}

int main(int argc, char **argv) {
    int hdr = argc > 1 && !strcmp(argv[1], "hdr");
    int seconds = argc > 2 ? atoi(argv[2]) : DEFAULT_SECONDS;
    setvbuf(stdout, NULL, _IONBF, 0);
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

    IDXGIFactory1 *factory;
    CreateDXGIFactory1(&IID_IDXGIFactory1, (void **)&factory);
    IDXGIOutput *output = NULL;
    IDXGIAdapter1 *adapter = adapter_with_output(factory, &output);
    if (!adapter) { printf("FAILED: no output\n"); return 1; }
    ID3D11Device *device;
    ID3D11DeviceContext *context;
    HRESULT result = D3D11CreateDevice((IDXGIAdapter *)adapter, D3D_DRIVER_TYPE_UNKNOWN, NULL, 0, NULL, 0, D3D11_SDK_VERSION, &device, NULL, &context);
    if (FAILED(result)) { printf("FAILED: device 0x%08lx\n", result); return 1; }

    for (int half = 0; half < 1 << 16; half++) half_values[half] = half_to_float((unsigned short)half);

    IDXGIOutputDuplication *duplication = NULL;
    ID3D11Texture2D *staging = NULL;
    D3D11_TEXTURE2D_DESC description;
    int *columns = NULL;
    unsigned frames = 0, created = 0, bad_frames = 0;
    long worst = 0;
    ULONGLONG end = GetTickCount64() + seconds * 1000ull;
    while (GetTickCount64() < end) {
        if (!duplication) {
            result = duplicate(output, device, hdr, &duplication);
            if (FAILED(result)) { count_result(result); duplication = NULL; Sleep(WAIT_MS / 4); continue; }
            created++;
        }
        DXGI_OUTDUPL_FRAME_INFO info;
        IDXGIResource *resource = NULL;
        result = IDXGIOutputDuplication_AcquireNextFrame(duplication, WAIT_MS, &info, &resource);
        count_result(result);
        if (result == DXGI_ERROR_ACCESS_LOST) { IDXGIOutputDuplication_Release(duplication); duplication = NULL; continue; }
        if (FAILED(result)) continue;
        ID3D11Texture2D *texture = NULL;
        IDXGIResource_QueryInterface(resource, &IID_ID3D11Texture2D, (void **)&texture);
        D3D11_TEXTURE2D_DESC now;
        ID3D11Texture2D_GetDesc(texture, &now);
        if (staging && (now.Format != description.Format || now.Width != description.Width || now.Height != description.Height)) {
            ID3D11Texture2D_Release(staging);
            staging = NULL;
        }
        if (!staging) {
            description = now;
            description.Usage = D3D11_USAGE_STAGING;
            description.BindFlags = 0;
            description.MiscFlags = 0;
            description.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
            ID3D11Device_CreateTexture2D(device, &description, NULL, &staging);
            free(columns);
            columns = columns_for(description.Width);
        }
        ID3D11DeviceContext_CopyResource(context, (ID3D11Resource *)staging, (ID3D11Resource *)texture);
        ID3D11Texture2D_Release(texture);
        IDXGIResource_Release(resource);
        IDXGIOutputDuplication_ReleaseFrame(duplication);

        int floats = description.Format == DXGI_FORMAT_R16G16B16A16_FLOAT;
        if (floats != hdr) { printf("FAILED: format %u for %s\n", description.Format, hdr ? "hdr" : "sdr"); return 1; }
        D3D11_MAPPED_SUBRESOURCE mapped;
        result = ID3D11DeviceContext_Map(context, (ID3D11Resource *)staging, 0, D3D11_MAP_READ, 0, &mapped);
        if (FAILED(result)) { printf("FAILED: map 0x%08lx\n", result); return 1; }
        RECT cursor = cursor_rectangle();
        long wrong = count_wrong_pixels(mapped.pData, mapped.RowPitch, description.Width, description.Height, hdr, columns, &cursor);
        ID3D11DeviceContext_Unmap(context, (ID3D11Resource *)staging, 0);
        frames++;
        bad_frames += wrong != 0;
        if (wrong > worst) worst = wrong;
    }
    printf("%u frames judged in %d s, created %u times\n", frames, seconds, created);
    for (int i = 0; i < MOST_RESULTS && results[i].count; i++) printf("  0x%08lx: %u\n", results[i].result, results[i].count);
    if (!frames) { printf("FAILED: no frame\n"); return 1; }
    int passed = bad_frames == 0;
    printf("%s: %s bars at %ux%u, format %u, %u of %u frames wrong, the worst by %ld pixels\n", passed ? "PASSED" : "FAILED", hdr ? "hdr" : "sdr", description.Width,
           description.Height, description.Format, bad_frames, frames, worst);
    return passed ? 0 : 1;
}
