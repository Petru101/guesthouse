/*
 * card-report: writes down what the Windows driver reports for an NVIDIA card, in a short text
 * file for a person to read and post.
 *
 * It only asks Windows ordinary questions about the adapter, the same ones every program that
 * uses the GPU asks. It sends nothing to the driver, changes no settings and needs no admin rights.
 */
#include "card.h"

#include <windows.h>
#include <stdlib.h>
#include <string.h>

#define REPORT_NAME L"guesthouse-card-report.txt"
#define MOST_ADAPTERS 32

/* D3DKMT_QUERYADAPTERINFO's question numbers. */
#define QUERY_RECORD 0
#define QUERY_DRIVER_MODEL 13
#define QUERY_USER_DRIVER_VERSION 18
#define QUERY_NODE 25
#define QUERY_DEVICE_IDS 31
#define QUERY_KERNEL_DRIVER_VERSION 49
#define QUERY_DRIVER_MODEL_FOR_RENDER 56
#define QUERY_DESCRIPTION 65
#define QUERY_DESCRIPTION_FOR_RENDER 66

/* Room for a record of a size this program has no layout for. */
#define RECORD_LARGEST_ASK 0x10000

#define VERSION_PARTS 4
#define VERSION_PART_BITS 16
#define DESCRIPTION_CHARS 4096
#define NODE_NAME_CHARS 32

typedef LONG NTSTATUS;

typedef struct { UINT handle; LUID luid; ULONG sources; BOOL precise; } AdapterInfo;
typedef struct { ULONG count; AdapterInfo *list; } EnumAdapters;
typedef struct { UINT handle; UINT type; void *data; UINT size; } AdapterQuery;
typedef struct { UINT handle; } CloseAdapter;

typedef struct {
    UINT physical_adapter;
    UINT vendor, device, sub_vendor, sub_system, revision, bus_type;
} DeviceIds;

/* The only packed one, as Windows wants exactly this many bytes. */
#pragma pack(push, 2)
typedef struct {
    UINT ordinal;
    UINT engine_type;
    WCHAR name[NODE_NAME_CHARS];
    UINT flags;
    BOOLEAN gpu_mmu;
    BOOLEAN io_mmu;
} NodeMetadata;
#pragma pack(pop)

typedef NTSTATUS (WINAPI *EnumAdaptersFn)(EnumAdapters *);
typedef NTSTATUS (WINAPI *AdapterQueryFn)(AdapterQuery *);
typedef NTSTATUS (WINAPI *CloseAdapterFn)(CloseAdapter *);
typedef NTSTATUS (WINAPI *WindowsVersionFn)(OSVERSIONINFOW *);

static EnumAdaptersFn enum_adapters;
static AdapterQueryFn adapter_query;
static CloseAdapterFn close_adapter;

static Report report;

static bool ask(UINT adapter, UINT question, void *answer, UINT size)
{
    AdapterQuery query = { adapter, question, answer, size };

    return adapter_query(&query) >= 0;
}

static void read_driver_version(UINT adapter, AdapterFacts *facts)
{
    uint64_t version = 0;

    if (!ask(adapter, QUERY_USER_DRIVER_VERSION, &version, sizeof(version)) &&
        !ask(adapter, QUERY_KERNEL_DRIVER_VERSION, &version, sizeof(version))) {
        return;
    }
    facts->have_driver_version = true;
    for (int part = 0; part < VERSION_PARTS; part++) {
        int shift = (VERSION_PARTS - 1 - part) * VERSION_PART_BITS;

        facts->driver_version[part] = (uint16_t)(version >> shift);
    }
}

static void read_nodes(UINT adapter, AdapterFacts *facts)
{
    for (UINT ordinal = 0; ordinal < MOST_NODES; ordinal++) {
        NodeMetadata node = { .ordinal = ordinal };
        WindowsNode *named = &facts->nodes[ordinal];

        if (!ask(adapter, QUERY_NODE, &node, sizeof(node))) {
            return;
        }
        named->engine_type = node.engine_type;
        plain_text(named->name, sizeof(named->name), (const uint16_t *)node.name, NODE_NAME_CHARS);
        facts->node_count++;
    }
}

static void read_adapter(UINT adapter, const DeviceIds *ids, AdapterFacts *facts)
{
    static WCHAR description[DESCRIPTION_CHARS];

    facts->have_ids = true;
    facts->device_id = (uint16_t)ids->device;
    facts->revision = (uint8_t)ids->revision;
    if (ask(adapter, QUERY_DESCRIPTION_FOR_RENDER, description, sizeof(description)) ||
        ask(adapter, QUERY_DESCRIPTION, description, sizeof(description))) {
        plain_text(facts->name, sizeof(facts->name), (const uint16_t *)description, DESCRIPTION_CHARS);
    }
    read_driver_version(adapter, facts);
    if (!ask(adapter, QUERY_DRIVER_MODEL_FOR_RENDER, &facts->driver_model, sizeof(facts->driver_model))) {
        ask(adapter, QUERY_DRIVER_MODEL, &facts->driver_model, sizeof(facts->driver_model));
    }
    read_nodes(adapter, facts);
}

/* The record stays in this buffer. Only the fields record_read picks out go any further. */
static void read_record(UINT adapter, const AdapterFacts *card, RecordFacts *facts)
{
    uint8_t *record = calloc(1, RECORD_LARGEST_ASK);
    RecordClues clues = { card->device_id, card->revision, card->node_count, card->name };
    uint32_t size;

    facts->state = RECORD_REFUSED;
    if (!record) {
        return;
    }
    for (size_t i = 0; (size = record_known_size(i)) != 0; i++) {
        if (ask(adapter, QUERY_RECORD, record, size)) {
            record_read(record, size, &clues, facts);
            free(record);
            return;
        }
    }
    if (ask(adapter, QUERY_RECORD, record, RECORD_LARGEST_ASK)) {
        record_read(record, RECORD_LARGEST_ASK, &clues, facts);
    }
    free(record);
}

static void read_cards(void)
{
    static AdapterInfo list[MOST_ADAPTERS];
    EnumAdapters adapters = { MOST_ADAPTERS, list };

    if (enum_adapters(&adapters) < 0) {
        return;
    }
    report.listed_adapters = true;
    report.adapters_seen = adapters.count;
    for (ULONG i = 0; i < adapters.count; i++) {
        UINT adapter = list[i].handle;
        DeviceIds ids = { 0 };
        CloseAdapter closing = { adapter };

        if (ask(adapter, QUERY_DEVICE_IDS, &ids, sizeof(ids)) && ids.vendor == NVIDIA_VENDOR &&
            report.card_count < MOST_CARDS) {
            Card *card = &report.cards[report.card_count++];

            read_adapter(adapter, &ids, &card->adapter);
            read_record(adapter, &card->adapter, &card->record);
        }
        close_adapter(&closing);
    }
}

static void read_windows_version(void)
{
    WindowsVersionFn get = (WindowsVersionFn)GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "RtlGetVersion");
    OSVERSIONINFOW version = { .dwOSVersionInfoSize = sizeof(version) };

    if (get && get(&version) >= 0) {
        report.have_windows_version = true;
        report.windows_version[0] = version.dwMajorVersion;
        report.windows_version[1] = version.dwMinorVersion;
        report.windows_version[2] = version.dwBuildNumber;
    }
}

static bool load_calls(void)
{
    HMODULE gdi = LoadLibraryW(L"gdi32.dll");

    if (!gdi) {
        return false;
    }
    enum_adapters = (EnumAdaptersFn)GetProcAddress(gdi, "D3DKMTEnumAdapters2");
    adapter_query = (AdapterQueryFn)GetProcAddress(gdi, "D3DKMTQueryAdapterInfo");
    close_adapter = (CloseAdapterFn)GetProcAddress(gdi, "D3DKMTCloseAdapter");
    return enum_adapters && adapter_query && close_adapter;
}

/* Tries this folder, which is where the program is if it was double-clicked, then the temp folder. */
static FILE *open_report(WCHAR *path, DWORD room)
{
    WCHAR temp[MAX_PATH];
    FILE *file;

    if (GetFullPathNameW(REPORT_NAME, room, path, NULL) && (file = _wfopen(path, L"w"))) {
        return file;
    }
    if (GetTempPathW(MAX_PATH, temp) && _snwprintf(path, room, L"%ls%ls", temp, REPORT_NAME) > 0) {
        return _wfopen(path, L"w");
    }
    return NULL;
}

/* WriteConsole so a name with accents comes out right, and plain UTF-8 if the output is a file. */
static void say_path(const WCHAR *path)
{
    static char utf8[4 * MAX_PATH];
    DWORD written;

    if (!WriteConsoleW(GetStdHandle(STD_OUTPUT_HANDLE), path, (DWORD)wcslen(path), &written, NULL) &&
        WideCharToMultiByte(CP_UTF8, 0, path, -1, utf8, sizeof(utf8), NULL, NULL)) {
        fputs(utf8, stdout);
    }
}

/* A double-clicked program's window would close before anyone read it. */
static void wait_if_window_is_ours(void)
{
    DWORD processes[2];

    if (GetConsoleProcessList(processes, 2) == 1) {
        printf("\nPress Enter to close this window.\n");
        getchar();
    }
}

static void say_what_happened(void)
{
    if (!report.listed_adapters) {
        printf("Windows wouldn't list the graphics adapters.\n");
    } else if (!report.card_count) {
        printf("No NVIDIA adapter found. If this PC has an NVIDIA card, install NVIDIA's driver and "
               "run this again.\n");
    }
    for (size_t i = 0; i < report.card_count; i++) {
        const Card *card = &report.cards[i];
        const char *name = card->adapter.name[0] ? card->adapter.name : card->record.name;

        printf("Found %s.\n", name[0] ? name : "an NVIDIA card");
        if (card->record.state != RECORD_READ || card->record.moved_fields_unread) {
            printf("Some fields can't be read on this driver, and the report says which.\n");
        }
    }
}

int main(void)
{
    static WCHAR path[2 * MAX_PATH];
    FILE *file;

    read_windows_version();
    if (load_calls()) {
        read_cards();
    }
    say_what_happened();

    file = open_report(path, sizeof(path) / sizeof(path[0]));
    if (!file) {
        printf("Couldn't write the report in this folder or the temp folder.\n");
        wait_if_window_is_ours();
        return 1;
    }
    report_write(file, &report);
    fclose(file);

    printf("\nThe report is in this file:\n  ");
    fflush(stdout);
    say_path(path);
    printf("\n\nPlease read it before you post it. It's plain text and it's short.\n");
    fflush(stdout);
    wait_if_window_is_ours();
    return 0;
}
