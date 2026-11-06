/*
 * Finding fields in a record that fits no layout. Each thing here is told by its own shape, or by
 * what Windows already said, so a field is only taken from a place nothing else could be. Two
 * matches count as none. Only offsets and the named fields leave this file.
 */
#include "layout.h"

#include <string.h>

#define GRAPHICS_ENTRIES 63
#define WINDOWS_PLATFORM_NT 2
#define WINDOWS_PLATFORM_FIELD 16
#define WINDOWS_MINOR_FIELD 8
#define WINDOWS_BLOCK_SIZE 20
#define ARCHITECTURE_SIZE 12
#define HIGHEST_IMPLEMENTATION 0xF
#define NODE_GRAPHICS 0x01
#define FEWEST_CLASSES 16

/* From NVIDIA's public headers: Turing to Blackwell, and classes every card has. */
static const uint32_t architectures[] = { 0x160, 0x170, 0x180, 0x190, 0x1A0, 0x1B0 };
static const uint32_t common_classes[] = { 0x0080, 0x2080, 0x902D, 0x906F };

#define COUNT_OF(list) (sizeof(list) / sizeof((list)[0]))

typedef bool Matcher(const uint8_t *data, size_t at, size_t size, const RecordClues *clues);

static bool is_marker(const uint8_t *data, size_t at, size_t size, const RecordClues *clues)
{
    (void)clues;
    return at + MAGIC_SIZE <= size && !memcmp(data + at, BLOCK_MAGIC, MAGIC_SIZE);
}

static bool is_pci_id(const uint8_t *data, size_t at, size_t size, const RecordClues *clues)
{
    return at + 4 <= size && read32(data, at) == ((uint32_t)clues->device_id << PCI_DEVICE_SHIFT | NVIDIA_VENDOR);
}

static bool is_architecture(const uint8_t *data, size_t at, size_t size, const RecordClues *clues)
{
    if (at + ARCHITECTURE_SIZE > size || !clues->revision ||
        read32(data, at + ARCHITECTURE_REVISION_FIELD) != clues->revision ||
        read32(data, at + ARCHITECTURE_IMPLEMENTATION_FIELD) > HIGHEST_IMPLEMENTATION) {
        return false;
    }
    for (size_t i = 0; i < COUNT_OF(architectures); i++) {
        if (read32(data, at) == architectures[i]) {
            return true;
        }
    }
    return false;
}

/* Other tables in the record count up the same way, but not to this length. */
static bool is_graphics_table(const uint8_t *data, size_t at, size_t size, const RecordClues *clues)
{
    (void)clues;
    if (at + (GRAPHICS_ENTRIES + 1) * TABLE_ENTRY_SIZE > size) {
        return false;
    }
    for (uint32_t index = 0; index < GRAPHICS_ENTRIES; index++) {
        if (read32(data, at + index * TABLE_ENTRY_SIZE) != index) {
            return false;
        }
    }
    return read32(data, at + GRAPHICS_ENTRIES * TABLE_ENTRY_SIZE) != GRAPHICS_ENTRIES;
}

static bool is_windows_block(const uint8_t *data, size_t at, size_t size, const RecordClues *clues)
{
    (void)clues;
    return at + WINDOWS_BLOCK_SIZE <= size && read32(data, at) == WINDOWS_INFO_SIZE &&
           read32(data, at + WINDOWS_MAJOR_FIELD) == WINDOWS_MAJOR &&
           read32(data, at + WINDOWS_MINOR_FIELD) == 0 &&
           read32(data, at + WINDOWS_PLATFORM_FIELD) == WINDOWS_PLATFORM_NT;
}

static bool is_node_list(const uint8_t *data, size_t at, size_t size, const RecordClues *clues)
{
    size_t count = clues->windows_nodes;

    if (!count || at + count * NODE_ENTRY_SIZE > size || read32(data, at) != NODE_GRAPHICS) {
        return false;
    }
    for (size_t i = 0; i < count; i++) {
        if (!node_entry_holds(data, at + i * NODE_ENTRY_SIZE)) {
            return false;
        }
    }
    return true;
}

static bool is_class_list(const uint8_t *data, size_t at, size_t size, const RecordClues *clues)
{
    size_t common = 0;
    uint32_t count;

    (void)clues;
    if (at + 4 > size || (count = read32(data, at)) < FEWEST_CLASSES || count > MOST_CLASSES ||
        at + 4 + count * 4 > size) {
        return false;
    }
    for (size_t i = 0; i < count; i++) {
        uint32_t class_number = read32(data, at + 4 + i * 4);

        if (class_number > HIGHEST_CLASS) {
            return false;
        }
        for (size_t known = 0; known < COUNT_OF(common_classes); known++) {
            common += class_number == common_classes[known];
        }
    }
    return common == COUNT_OF(common_classes);
}

static bool is_name(const uint8_t *data, size_t at, size_t size, const RecordClues *clues)
{
    size_t length = strlen(clues->name);

    if (!length || at + (length + 1) * 2 > size) {
        return false;
    }
    for (size_t i = 0; i < length; i++) {
        if (read16(data, at + i * 2) != (uint8_t)clues->name[i]) {
            return false;
        }
    }
    return read16(data, at + length * 2) == 0;
}

static void look_for(LandmarkKind kind, Matcher *matches, uint32_t known_at, const uint8_t *data,
                     size_t size, const RecordClues *clues, RecordFacts *facts)
{
    Landmark *mark = &facts->landmarks[kind];

    mark->known_at = known_at;
    for (size_t at = 0; at < size; at++) {
        if (matches(data, at, size, clues)) {
            mark->found_at = (uint32_t)at;
            mark->count++;
        }
    }
}

static bool one_of(const RecordFacts *facts, LandmarkKind kind)
{
    return facts->landmarks[kind].count == 1;
}

/* The shader set has no shape of its own. It's only worth a guess if the Windows block and the
 * graphics table have moved together, which says that part of the record kept its shape. */
static void guess_shader_set(const uint8_t *data, size_t size, RecordFacts *facts)
{
    const Layout *known = reference_layout;
    const Landmark *table = &facts->landmarks[LANDMARK_GRAPHICS_TABLE];
    const Landmark *windows = &facts->landmarks[LANDMARK_WINDOWS_VERSION];
    size_t at = windows->found_at + known->shader_set - known->windows_version;

    if (facts->shader_set || !one_of(facts, LANDMARK_GRAPHICS_TABLE) ||
        !one_of(facts, LANDMARK_WINDOWS_VERSION) || at + 4 > size ||
        table->found_at - table->known_at != windows->found_at - windows->known_at) {
        return;
    }
    if (read32(data, at) && read32(data, at) <= HIGHEST_SHADER_SET) {
        facts->shader_set = read32(data, at);
        facts->shader_set_guessed = true;
    }
}

static void take_nodes(const uint8_t *data, size_t count, RecordFacts *facts)
{
    size_t list = facts->landmarks[LANDMARK_NODES].found_at;

    for (size_t i = 0; i < count; i++) {
        size_t entry = list + i * NODE_ENTRY_SIZE;

        facts->nodes[i] = (RecordNode){
            .node_type = read32(data, entry),
            .scheduled = read32(data, entry + NODE_SCHEDULED_FIELD),
            .in_flight = read32(data, entry + NODE_IN_FLIGHT_FIELD),
        };
    }
    facts->node_count = count;
    facts->nodes_readable = true;
    facts->nodes_searched = true;
}

/* Without the card's own ID in exactly one place, this may not be this card's record at all. */
static void take_fields(const uint8_t *data, size_t size, const RecordClues *clues, RecordFacts *facts)
{
    if (!one_of(facts, LANDMARK_PCI_ID)) {
        return;
    }
    if (!facts->architecture && one_of(facts, LANDMARK_ARCHITECTURE)) {
        size_t at = facts->landmarks[LANDMARK_ARCHITECTURE].found_at;

        facts->architecture = read32(data, at);
        facts->implementation = read32(data, at + ARCHITECTURE_IMPLEMENTATION_FIELD);
        facts->chip_revision = read32(data, at + ARCHITECTURE_REVISION_FIELD);
        facts->architecture_searched = true;
    }
    if (!facts->sm_version && one_of(facts, LANDMARK_GRAPHICS_TABLE)) {
        size_t entry = facts->landmarks[LANDMARK_GRAPHICS_TABLE].found_at +
                       GRAPHICS_SM_VERSION * TABLE_ENTRY_SIZE;
        uint32_t value = read32(data, entry + TABLE_VALUE_FIELD);

        if (value <= HIGHEST_SM_VERSION) {
            facts->sm_version = value;
            facts->sm_version_searched = true;
        }
    }
    guess_shader_set(data, size, facts);
    if (!facts->nodes_readable && one_of(facts, LANDMARK_NODES) && clues->windows_nodes <= MOST_NODES) {
        take_nodes(data, clues->windows_nodes, facts);
    }
    if (!facts->classes_readable && one_of(facts, LANDMARK_CLASSES)) {
        read_class_list(data, facts->landmarks[LANDMARK_CLASSES].found_at, facts);
        facts->classes_searched = facts->classes_readable;
    }
}

void record_search(const uint8_t *data, size_t size, const RecordClues *clues, RecordFacts *facts)
{
    const Layout *known = reference_layout;

    facts->searched = true;
    look_for(LANDMARK_MARKER, is_marker, BLOCK_MAGIC_FIELD, data, size, clues, facts);
    look_for(LANDMARK_PCI_ID, is_pci_id, known->pci_id, data, size, clues, facts);
    look_for(LANDMARK_ARCHITECTURE, is_architecture, known->architecture, data, size, clues, facts);
    look_for(LANDMARK_GRAPHICS_TABLE, is_graphics_table, known->graphics_table, data, size, clues, facts);
    look_for(LANDMARK_WINDOWS_VERSION, is_windows_block, known->windows_version, data, size, clues, facts);
    look_for(LANDMARK_NODES, is_node_list, known->node_entries, data, size, clues, facts);
    look_for(LANDMARK_CLASSES, is_class_list, known->classes, data, size, clues, facts);
    look_for(LANDMARK_NAME, is_name, known->name_field, data, size, clues, facts);
    take_fields(data, size, clues, facts);
}
