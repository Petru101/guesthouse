/* What record.c and search.c share: where fields sit, and how to read them. */
#ifndef LAYOUT_H
#define LAYOUT_H

#include "card.h"

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

/* The layout other places are compared with, in what the report says about a search. */
extern const Layout *const reference_layout;

static inline uint16_t read16(const uint8_t *data, size_t at)
{
    return (uint16_t)(data[at] | data[at + 1] << 8);
}

static inline uint32_t read32(const uint8_t *data, size_t at)
{
    return read16(data, at) | (uint32_t)read16(data, at + 2) << 16;
}

static inline uint64_t read64(const uint8_t *data, size_t at)
{
    return read32(data, at) | (uint64_t)read32(data, at + 4) << 32;
}

/* Whether this looks like one engine node's entry. */
bool node_entry_holds(const uint8_t *data, size_t entry);

/* Reads a list with its length in front into facts, if it's a list of class numbers. */
void read_class_list(const uint8_t *data, size_t at, RecordFacts *facts);

/* Looks through a record that fits no layout, or only part of one, for what can be told by its
 * own shape. Fills in what it finds and leaves everything else alone. */
void record_search(const uint8_t *data, size_t size, const RecordClues *clues, RecordFacts *facts);

#endif
