/*
 * The driver's record. It also holds things that identify the card and the machine, so this file
 * lists only the fields the report needs, and a record it can't vouch for isn't read at all.
 */
#include "card.h"

#include <string.h>

#define HEADER_MAGIC "ADVN"
#define BLOCK_MAGIC "MKVN"
#define MAGIC_SIZE 4
#define FORMAT_VERSION_FIELD 0x04
#define FORMAT_REVISION_FIELD 0x06
#define SIZE_FIELD 0x08
#define BLOCK_MAGIC_FIELD 0x0C
#define HEADER_SIZE 0x10

/* Bytes in one (index, value) entry of the record's tables, and where the value sits in it. */
#define TABLE_ENTRY_SIZE 8
#define TABLE_VALUE_FIELD 4
#define GRAPHICS_SM_VERSION 0x0C

#define NODE_ENTRY_SIZE 40
#define NODE_SCHEDULED_FIELD 0x08
#define NODE_IN_FLIGHT_FIELD 0x14
#define NODE_RECORD_SIZE 0x110
#define NODE_COPY_COUNT_FIELD 0x08
#define NODE_COPY_ENGINE_FIELD 0x0C

#define ARCHITECTURE_IMPLEMENTATION_FIELD 4
#define ARCHITECTURE_REVISION_FIELD 8
#define MEMORY_ENTRY_SIZE 8
#define MEMORY_USABLE 0
#define MEMORY_HEAP 1
#define MEMORY_TOTAL 3

#define PCI_VENDOR_MASK 0xFFFF
#define PCI_DEVICE_SHIFT 16
#define NAME_CHARS 32
#define HIGHEST_CLASS 0xFFFF

/* The fields this program reads, for one driver branch. Each matches RecordLayout in
 * crates/backend-nvidia/src/profile/guest.rs. */
typedef struct {
    const char *name;
    uint32_t size;
    uint32_t pci_id;
    uint32_t architecture;
    uint32_t graphics_table;
    uint32_t shader_set;
    uint32_t node_records;
    uint32_t node_entries;
    uint32_t node_count;
    uint32_t memory;
    uint32_t classes;
    uint32_t name_field;
} Layout;

static const Layout layouts[] = {
    {
        .name = "617",
        .size = 0xC5B8,
        .pci_id = 0x3C,
        .architecture = 0xEC,
        .graphics_table = 0x1EB,
        .shader_set = 0x1893,
        .node_records = 0x1D02,
        .node_entries = 0x6AE7,
        .node_count = 0x6FBF,
        .memory = 0xB128,
        .classes = 0xB158,
        .name_field = 0xC4B8,
    },
};

#define LAYOUT_COUNT (sizeof(layouts) / sizeof(layouts[0]))

static uint16_t read16(const uint8_t *data, size_t at)
{
    return (uint16_t)(data[at] | data[at + 1] << 8);
}

static uint32_t read32(const uint8_t *data, size_t at)
{
    return read16(data, at) | (uint32_t)read16(data, at + 2) << 16;
}

static uint64_t read64(const uint8_t *data, size_t at)
{
    return read32(data, at) | (uint64_t)read32(data, at + 4) << 32;
}

uint32_t record_known_size(size_t index)
{
    return index < LAYOUT_COUNT ? layouts[index].size : 0;
}

void plain_text(char *to, size_t room, const uint16_t *from, size_t length)
{
    size_t used = 0;

    for (size_t i = 0; i < length && from[i] && used + 1 < room; i++) {
        to[used++] = from[i] >= 0x20 && from[i] < 0x7F ? (char)from[i] : '?';
    }
    to[used] = 0;
}

static void read_name(const uint8_t *data, const Layout *layout, RecordFacts *facts)
{
    uint16_t name[NAME_CHARS];

    for (size_t i = 0; i < NAME_CHARS; i++) {
        name[i] = read16(data, layout->name_field + i * 2);
    }
    plain_text(facts->name, sizeof(facts->name), name, NAME_CHARS);
}

static void read_nodes(const uint8_t *data, const Layout *layout, RecordFacts *facts)
{
    uint32_t count = read32(data, layout->node_count);

    if (count > MOST_NODES) {
        return;
    }
    facts->nodes_readable = true;
    facts->node_count = count;
    for (size_t i = 0; i < count; i++) {
        size_t entry = layout->node_entries + i * NODE_ENTRY_SIZE;
        size_t record = layout->node_records + i * NODE_RECORD_SIZE;

        facts->nodes[i] = (RecordNode){
            .node_type = read32(data, entry),
            .scheduled = read32(data, entry + NODE_SCHEDULED_FIELD),
            .in_flight = read32(data, entry + NODE_IN_FLIGHT_FIELD),
            .copy_flags = read32(data, record),
            .copy_engine_count = read32(data, record + NODE_COPY_COUNT_FIELD),
            .copy_engine = read32(data, record + NODE_COPY_ENGINE_FIELD),
        };
    }
}

static void read_classes(const uint8_t *data, const Layout *layout, RecordFacts *facts)
{
    uint32_t count = read32(data, layout->classes);

    if (count == 0 || count > MOST_CLASSES) {
        return;
    }
    for (size_t i = 0; i < count; i++) {
        facts->classes[i] = read32(data, layout->classes + 4 + i * 4);
        if (facts->classes[i] > HIGHEST_CLASS) {
            return;
        }
    }
    facts->classes_readable = true;
    facts->class_count = count;
}

static void read_fields(const uint8_t *data, const Layout *layout, RecordFacts *facts)
{
    size_t sm_version = layout->graphics_table + GRAPHICS_SM_VERSION * TABLE_ENTRY_SIZE + TABLE_VALUE_FIELD;

    facts->layout = layout->name;
    read_name(data, layout, facts);
    facts->architecture = read32(data, layout->architecture);
    facts->implementation = read32(data, layout->architecture + ARCHITECTURE_IMPLEMENTATION_FIELD);
    facts->chip_revision = read32(data, layout->architecture + ARCHITECTURE_REVISION_FIELD);
    facts->sm_version = read32(data, sm_version);
    facts->shader_set = read32(data, layout->shader_set);
    facts->memory_usable = read64(data, layout->memory + MEMORY_USABLE * MEMORY_ENTRY_SIZE);
    facts->memory_heap = read64(data, layout->memory + MEMORY_HEAP * MEMORY_ENTRY_SIZE);
    facts->memory_total = read64(data, layout->memory + MEMORY_TOTAL * MEMORY_ENTRY_SIZE);
    read_nodes(data, layout, facts);
    read_classes(data, layout, facts);
}

static const Layout *layout_of_size(uint32_t size)
{
    for (size_t i = 0; i < LAYOUT_COUNT; i++) {
        if (layouts[i].size == size) {
            return &layouts[i];
        }
    }
    return NULL;
}

/* A record of the right size could still be laid out differently, and then a field read by its
 * position could land on something private. The block marker and the card's own ID have to be
 * where this layout says first. */
static bool layout_holds(const uint8_t *data, const Layout *layout, uint16_t device_id)
{
    uint32_t pci_id = read32(data, layout->pci_id);

    return !memcmp(data + BLOCK_MAGIC_FIELD, BLOCK_MAGIC, MAGIC_SIZE) &&
           (pci_id & PCI_VENDOR_MASK) == NVIDIA_VENDOR && pci_id >> PCI_DEVICE_SHIFT == device_id;
}

void record_read(const uint8_t *data, size_t size, uint16_t device_id, RecordFacts *facts)
{
    const Layout *layout;

    memset(facts, 0, sizeof(*facts));
    if (size < HEADER_SIZE || memcmp(data, HEADER_MAGIC, MAGIC_SIZE)) {
        facts->state = RECORD_NOT_ONE;
        return;
    }
    facts->size = read32(data, SIZE_FIELD);
    facts->format_version = read16(data, FORMAT_VERSION_FIELD);
    facts->format_revision = read16(data, FORMAT_REVISION_FIELD);

    layout = layout_of_size(facts->size);
    if (!layout) {
        facts->state = RECORD_OTHER_LAYOUT;
    } else if (size < layout->size || !layout_holds(data, layout, device_id)) {
        facts->state = RECORD_FAILED_CHECK;
    } else {
        facts->state = RECORD_READ;
        read_fields(data, layout, facts);
    }
}
