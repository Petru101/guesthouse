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
#define NODE_ALWAYS_ONE_FIELD 0x04
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
#define NAME_START "NVIDIA "
#define HIGHEST_CLASS 0xFFFF
#define CHANNEL_CLASS_LOW_BYTE 0x6F

/* The Windows version block in the record: its own size, then the major version. */
#define WINDOWS_INFO_SIZE 284
#define WINDOWS_MAJOR_FIELD 4
#define WINDOWS_MAJOR 10

/* The most these can be, for a number to be believed. */
#define HIGHEST_ARCHITECTURE 0xFFF
#define HIGHEST_BYTE 0xFF
#define HIGHEST_SM_VERSION 0xFFFF
#define HIGHEST_SHADER_SET 0xFFFF
#define MOST_COPY_ENGINES 32
#define MOST_VIDEO_MEMORY (1ULL << 40)

/* The fields this program reads, for one driver branch. 617 matches RecordLayout in
 * crates/backend-nvidia/src/profile/guest.rs. An unproven row hasn't met a real record yet, so the
 * fields that moved on that branch are only read if they all pass moved_fields_hold. */
typedef struct {
    const char *name;
    bool unproven;
    uint32_t size;
    uint32_t pci_id;
    uint32_t architecture;
    uint32_t graphics_table;
    uint32_t windows_version;
    uint32_t shader_set;
    uint32_t node_records;
    uint32_t node_entries;
    uint32_t node_count;
    uint32_t memory;
    uint32_t classes;
    uint32_t name_field;
} Layout;

/* Largest first, as a driver may hand a short buffer the start of a longer record. */
static const Layout layouts[] = {
    {
        .name = "621",
        .unproven = true,
        .size = 0xCDF8,
        .pci_id = 0x3C,
        .architecture = 0xEC,
        .graphics_table = 0x1EB,
        .windows_version = 0x1767,
        .shader_set = 0x1893,
        .node_records = 0x1D02,
        .node_entries = 0x7327,
        .node_count = 0x77FF,
        .memory = 0xB968,
        .classes = 0xB998,
        .name_field = 0xCCF8,
    },
    {
        .name = "617",
        .size = 0xC5B8,
        .pci_id = 0x3C,
        .architecture = 0xEC,
        .graphics_table = 0x1EB,
        .windows_version = 0x1767,
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

static size_t sm_version_entry(const Layout *layout)
{
    return layout->graphics_table + GRAPHICS_SM_VERSION * TABLE_ENTRY_SIZE;
}

static void read_memory(const uint8_t *data, const Layout *layout, RecordFacts *facts)
{
    facts->memory_usable = read64(data, layout->memory + MEMORY_USABLE * MEMORY_ENTRY_SIZE);
    facts->memory_heap = read64(data, layout->memory + MEMORY_HEAP * MEMORY_ENTRY_SIZE);
    facts->memory_total = read64(data, layout->memory + MEMORY_TOTAL * MEMORY_ENTRY_SIZE);
}

static void read_moved_fields(const uint8_t *data, const Layout *layout, RecordFacts *facts)
{
    read_name(data, layout, facts);
    read_memory(data, layout, facts);
    read_nodes(data, layout, facts);
    read_classes(data, layout, facts);
}

static bool name_holds(const RecordFacts *facts)
{
    return !strncmp(facts->name, NAME_START, strlen(NAME_START)) && !strchr(facts->name, '?');
}

static bool nodes_hold(const uint8_t *data, const Layout *layout, const RecordFacts *facts,
                       size_t windows_nodes)
{
    if (!facts->nodes_readable || !facts->node_count ||
        (windows_nodes && facts->node_count != windows_nodes)) {
        return false;
    }
    for (size_t i = 0; i < facts->node_count; i++) {
        const RecordNode *node = &facts->nodes[i];
        size_t entry = layout->node_entries + i * NODE_ENTRY_SIZE;

        if (read32(data, entry + NODE_ALWAYS_ONE_FIELD) != 1 || node->node_type > HIGHEST_BYTE ||
            node->scheduled > 1 || !node->in_flight || node->in_flight > HIGHEST_BYTE ||
            node->copy_engine_count > MOST_COPY_ENGINES || node->copy_engine > HIGHEST_BYTE) {
            return false;
        }
    }
    return true;
}

static bool classes_hold(const RecordFacts *facts)
{
    for (size_t i = 0; facts->classes_readable && i < facts->class_count; i++) {
        if ((facts->classes[i] & 0xFF) == CHANNEL_CLASS_LOW_BYTE) {
            return true;
        }
    }
    return false;
}

static bool memory_holds(const RecordFacts *facts)
{
    return facts->memory_total && facts->memory_total < MOST_VIDEO_MEMORY &&
           facts->memory_usable <= facts->memory_total && facts->memory_heap <= facts->memory_total;
}

/* The node list sits right after the ECID, so a guess that's a few bytes out could land on it.
 * Every moved field has to look exactly as it should, or none of them are kept. */
static bool moved_fields_hold(const uint8_t *data, const Layout *layout, const RecordFacts *facts,
                              size_t windows_nodes)
{
    return name_holds(facts) && nodes_hold(data, layout, facts, windows_nodes) &&
           classes_hold(facts) && memory_holds(facts);
}

static void forget_moved_fields(RecordFacts *facts)
{
    facts->name[0] = 0;
    facts->memory_usable = facts->memory_heap = facts->memory_total = 0;
    facts->nodes_readable = facts->classes_readable = false;
    facts->node_count = facts->class_count = 0;
    memset(facts->nodes, 0, sizeof(facts->nodes));
    memset(facts->classes, 0, sizeof(facts->classes));
    facts->moved_fields_unread = true;
}

static void read_fields(const uint8_t *data, const Layout *layout, size_t windows_nodes,
                        RecordFacts *facts)
{
    facts->layout = layout->name;
    facts->layout_unproven = layout->unproven;
    facts->architecture = read32(data, layout->architecture);
    facts->implementation = read32(data, layout->architecture + ARCHITECTURE_IMPLEMENTATION_FIELD);
    facts->chip_revision = read32(data, layout->architecture + ARCHITECTURE_REVISION_FIELD);
    facts->sm_version = read32(data, sm_version_entry(layout) + TABLE_VALUE_FIELD);
    facts->shader_set = read32(data, layout->shader_set);
    read_moved_fields(data, layout, facts);
    if (layout->unproven && !moved_fields_hold(data, layout, facts, windows_nodes)) {
        forget_moved_fields(facts);
    }
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
 * position could be a wrong number or land on something private. So the block marker, the card's
 * own ID, the table's own index and the Windows version block have to be where this layout says,
 * and the numbers have to be ones that can be real. */
static bool layout_holds(const uint8_t *data, const Layout *layout, uint16_t device_id)
{
    uint32_t pci_id = read32(data, layout->pci_id);
    size_t sm_version = sm_version_entry(layout);

    return !memcmp(data + BLOCK_MAGIC_FIELD, BLOCK_MAGIC, MAGIC_SIZE) &&
           (pci_id & PCI_VENDOR_MASK) == NVIDIA_VENDOR && pci_id >> PCI_DEVICE_SHIFT == device_id &&
           read32(data, sm_version) == GRAPHICS_SM_VERSION &&
           read32(data, sm_version + TABLE_VALUE_FIELD) <= HIGHEST_SM_VERSION &&
           read32(data, layout->windows_version) == WINDOWS_INFO_SIZE &&
           read32(data, layout->windows_version + WINDOWS_MAJOR_FIELD) == WINDOWS_MAJOR &&
           read32(data, layout->architecture) <= HIGHEST_ARCHITECTURE &&
           read32(data, layout->architecture + ARCHITECTURE_IMPLEMENTATION_FIELD) <= HIGHEST_BYTE &&
           read32(data, layout->architecture + ARCHITECTURE_REVISION_FIELD) <= HIGHEST_BYTE &&
           read32(data, layout->shader_set) <= HIGHEST_SHADER_SET;
}

void record_read(const uint8_t *data, size_t size, uint16_t device_id, size_t windows_nodes,
                 RecordFacts *facts)
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
        read_fields(data, layout, windows_nodes, facts);
    }
}
