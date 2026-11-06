/* Writes the report. Everything here is a fixed label and a number or a cleaned-up name. */
#include "card.h"

#define REPORT_VERSION 1
#define CLASSES_PER_LINE 8
#define BYTES_PER_MEBIBYTE (1024 * 1024)

/* The last two parts of the Windows version, read as NVIDIA numbers its drivers. */
#define VERSION_BRANCH_DIGIT 10
#define VERSION_BRANCH_STEP 10000
#define VERSION_BRANCH_DIVISOR 100

#define WDDM_MAJOR_DIVISOR 1000
#define WDDM_MINOR_DIVISOR 100

static const char *const engine_types[] = {
    "other", "3D", "video decode", "video encode", "video processing",
    "scene assembly", "copy", "overlay", "crypto",
};

#define ENGINE_TYPE_COUNT (sizeof(engine_types) / sizeof(engine_types[0]))

static const char *why_unread(const RecordFacts *record)
{
    switch (record->state) {
    case RECORD_REFUSED:
        return "the driver didn't hand over its record";
    case RECORD_NOT_ONE:
        return "what the driver handed over isn't a record this program knows";
    case RECORD_OTHER_LAYOUT:
        return "this program has no layout for a record of this size";
    case RECORD_FAILED_CHECK:
        return "the record didn't pass its checks, so its layout can't be trusted";
    default:
        return "the record had nothing there";
    }
}

static void line_unread(FILE *out, const char *label, const RecordFacts *record)
{
    fprintf(out, "%s: can't be read, %s\n", label, why_unread(record));
}

static void line_number(FILE *out, const char *label, uint32_t value, const RecordFacts *record)
{
    if (value) {
        fprintf(out, "%s: 0x%X\n", label, (unsigned)value);
    } else {
        line_unread(out, label, record);
    }
}

static void line_memory(FILE *out, const char *label, uint64_t bytes, const RecordFacts *record)
{
    if (bytes) {
        fprintf(out, "%s: %llu MiB\n", label, (unsigned long long)(bytes / BYTES_PER_MEBIBYTE));
    } else {
        line_unread(out, label, record);
    }
}

static void write_driver_version(FILE *out, const AdapterFacts *adapter)
{
    const uint16_t *version = adapter->driver_version;
    unsigned nvidia = (version[2] % VERSION_BRANCH_DIGIT) * VERSION_BRANCH_STEP + version[3];

    if (!adapter->have_driver_version) {
        fprintf(out, "Driver version: can't be read on this driver\n");
        return;
    }
    fprintf(out, "Driver version: %u.%u.%u.%u (NVIDIA %u.%02u)\n", version[0], version[1], version[2],
            version[3], nvidia / VERSION_BRANCH_DIVISOR, nvidia % VERSION_BRANCH_DIVISOR);
}

static void write_adapter(FILE *out, const AdapterFacts *adapter, const RecordFacts *record)
{
    const char *name = adapter->name[0] ? adapter->name : record->name;

    if (name[0]) {
        fprintf(out, "GPU name: %s\n", name);
    } else {
        fprintf(out, "GPU name: can't be read on this driver\n");
    }
    if (adapter->have_ids) {
        fprintf(out, "Device ID: 0x%04X\n", adapter->device_id);
        fprintf(out, "PCI revision: 0x%02X\n", adapter->revision);
    } else {
        fprintf(out, "Device ID: can't be read on this driver\n");
    }
    write_driver_version(out, adapter);
    if (adapter->driver_model) {
        fprintf(out, "WDDM version: %u.%u\n", (unsigned)adapter->driver_model / WDDM_MAJOR_DIVISOR,
                (unsigned)adapter->driver_model % WDDM_MAJOR_DIVISOR / WDDM_MINOR_DIVISOR);
    } else {
        fprintf(out, "WDDM version: can't be read on this driver\n");
    }
}

static void write_record_size(FILE *out, const RecordFacts *record)
{
    if (record->size) {
        fprintf(out, "Driver record size: %u bytes\n", (unsigned)record->size);
        fprintf(out, "Driver record format: %u.%u\n", record->format_version, record->format_revision);
    } else {
        line_unread(out, "Driver record size", record);
    }
    if (record->state == RECORD_READ) {
        fprintf(out, "Driver record layout: %s\n", record->layout);
    } else {
        line_unread(out, "Driver record layout", record);
    }
}

static void write_windows_nodes(FILE *out, const AdapterFacts *adapter)
{
    if (!adapter->node_count) {
        fprintf(out, "Engine nodes from Windows: can't be read on this driver\n");
        return;
    }
    fprintf(out, "Engine nodes from Windows: %zu\n", adapter->node_count);
    for (size_t i = 0; i < adapter->node_count; i++) {
        const WindowsNode *node = &adapter->nodes[i];
        unsigned type = (unsigned)node->engine_type;

        fprintf(out, "  %2zu: %s%s%s\n", i, type < ENGINE_TYPE_COUNT ? engine_types[type] : "unknown",
                node->name[0] ? ", " : "", node->name);
    }
}

static void write_record_nodes(FILE *out, const RecordFacts *record)
{
    if (!record->nodes_readable) {
        line_unread(out, "Engine nodes from the record", record);
        return;
    }
    fprintf(out, "Engine nodes from the record: %zu\n", record->node_count);
    for (size_t i = 0; i < record->node_count; i++) {
        const RecordNode *node = &record->nodes[i];

        fprintf(out, "  %2zu: type 0x%02X, scheduled %u, in flight %u, copy flags 0x%X, copy engines %u, "
                     "copy engine 0x%X\n",
                i, (unsigned)node->node_type, (unsigned)node->scheduled, (unsigned)node->in_flight,
                (unsigned)node->copy_flags, (unsigned)node->copy_engine_count, (unsigned)node->copy_engine);
    }
}

static void write_classes(FILE *out, const RecordFacts *record)
{
    if (!record->classes_readable) {
        line_unread(out, "Classes", record);
        return;
    }
    fprintf(out, "Classes: %zu\n", record->class_count);
    for (size_t i = 0; i < record->class_count; i++) {
        bool ends_line = (i + 1) % CLASSES_PER_LINE == 0 || i + 1 == record->class_count;

        fprintf(out, "%s%04X%s", i % CLASSES_PER_LINE ? " " : "  ", (unsigned)record->classes[i],
                ends_line ? "\n" : "");
    }
}

static void write_card(FILE *out, const Card *card)
{
    const RecordFacts *record = &card->record;

    write_adapter(out, &card->adapter, record);
    write_record_size(out, record);
    line_number(out, "Architecture", record->architecture, record);
    if (record->state == RECORD_READ) {
        fprintf(out, "Implementation: 0x%X\n", (unsigned)record->implementation);
    } else {
        line_unread(out, "Implementation", record);
    }
    line_number(out, "Chip revision", record->chip_revision, record);
    line_number(out, "SM version", record->sm_version, record);
    line_number(out, "Shader set", record->shader_set, record);
    line_memory(out, "Video memory", record->memory_total, record);
    line_memory(out, "Video memory heap", record->memory_heap, record);
    line_memory(out, "Video memory usable", record->memory_usable, record);
    write_windows_nodes(out, &card->adapter);
    write_record_nodes(out, record);
    write_classes(out, record);
}

void report_write(FILE *out, const Report *report)
{
    fprintf(out, "Guesthouse card report, version %d\n\n", REPORT_VERSION);
    if (report->have_windows_version) {
        fprintf(out, "Windows version: %u.%u.%u\n", (unsigned)report->windows_version[0],
                (unsigned)report->windows_version[1], (unsigned)report->windows_version[2]);
    } else {
        fprintf(out, "Windows version: can't be read\n");
    }
    if (!report->listed_adapters) {
        fprintf(out, "Adapters: Windows wouldn't list them\n");
    } else if (!report->card_count) {
        fprintf(out, "NVIDIA adapter: none found, out of %zu adapters\n", report->adapters_seen);
    }
    for (size_t i = 0; i < report->card_count; i++) {
        fprintf(out, "\nCard %zu\n", i + 1);
        write_card(out, &report->cards[i]);
    }
}
