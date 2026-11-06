/*
 * Tests for what reaches the report, on a made-up record with made-up private values.
 * Run by test.sh, which needs nothing but gcc.
 */
#define _GNU_SOURCE
#include "card.h"

#include <stdlib.h>
#include <string.h>

#define RECORD_SIZE 0xC5B8
#define RECORD_SIZE_621 0xCDF8
#define DEVICE_ID 0x2704

/* On the 621 layout, everything from here on sits further along. */
#define MOVED_FROM 0x6777
#define MOVED_BY_621 0x840
#define NO_MOVE 0

/* Where the private things sit in the record. These match RecordLayout in guest.rs. */
#define UUID_FIELD 0x15CB
#define UUID_SIZE 16
#define ECID_FIELD 0x6ADF
#define ECID_SIZE 8

#define PCI_ID_FIELD 0x3C
#define ARCHITECTURE_FIELD 0xEC
#define GRAPHICS_TABLE_FIELD 0x1EB
#define SM_INDEX_FIELD (GRAPHICS_TABLE_FIELD + 0x0C * 8)
#define SM_VERSION_FIELD (SM_INDEX_FIELD + 4)
#define WINDOWS_VERSION_FIELD 0x1767
#define SHADER_SET_FIELD 0x1893
#define NODE_RECORDS_FIELD 0x1D02
#define NODE_ENTRIES_FIELD 0x6AE7
#define NODE_COUNT_FIELD 0x6FBF
#define MEMORY_FIELD 0xB128
#define CLASSES_FIELD 0xB158
#define NAME_FIELD 0xC4B8

#define CLASS_LINES "Classes: 16\n  000F 0080 2080 902D 906F C96F C9B5 0001\n  0002 0005 0041 0071 00DE 0090 0092 00B1\n"
#define NODE_ENTRY_SIZE 40
#define NODE_RECORD_SIZE 0x110
#define TEST_NODES 2
#define TEST_CLASSES 16
#define GRAPHICS_ENTRIES 63
#define FRONT_MOVES_FROM 0x100
#define TEST_REVISION 0xA1

static const uint8_t uuid[UUID_SIZE] = {
    0xFA, 0xCE, 0xB0, 0x0C, 0x13, 0x57, 0x9B, 0xDF, 0x24, 0x68, 0xAC, 0xE0, 0x11, 0x22, 0x33, 0x44,
};
static const uint8_t ecid[ECID_SIZE] = { 0xDE, 0xAD, 0xBE, 0xEF, 0x55, 0x66, 0x77, 0x88 };

static int failures;

#define CHECK(condition) \
    do { \
        if (!(condition)) { \
            printf("%s:%d: %s\n", __func__, __LINE__, #condition); \
            failures++; \
        } \
    } while (0)

static void put32(uint8_t *record, size_t at, uint32_t value)
{
    memcpy(record + at, &value, sizeof(value));
}

static void put64(uint8_t *record, size_t at, uint64_t value)
{
    memcpy(record + at, &value, sizeof(value));
}

/* How far a test has moved everything from FRONT_MOVES_FROM on, on top of the move further along. */
static size_t front_move;

static size_t place(size_t field, size_t moved)
{
    size_t front = field >= FRONT_MOVES_FROM ? front_move : 0;

    return field + front + (field >= MOVED_FROM ? moved : 0);
}

/* A record like a driver's, with something non-zero in every byte the report has no business with. */
static uint8_t *make_moved_record(size_t size, size_t moved)
{
    static const char *name = "NVIDIA Test Card";
    static const uint32_t classes[TEST_CLASSES] = {
        0x000F, 0x0080, 0x2080, 0x902D, 0x906F, 0xC96F, 0xC9B5, 0x0001,
        0x0002, 0x0005, 0x0041, 0x0071, 0x00DE, 0x0090, 0x0092, 0x00B1,
    };
    uint8_t *record = malloc(size);

    memset(record, 0xA5, size);
    memcpy(record, "ADVN", 4);
    record[4] = 4;
    record[5] = 0;
    record[6] = 1;
    record[7] = 0;
    put32(record, 0x08, (uint32_t)size);
    memcpy(record + 0x0C, "MKVN", 4);
    memcpy(record + place(UUID_FIELD, moved), uuid, UUID_SIZE);
    memcpy(record + place(ECID_FIELD, moved), ecid, ECID_SIZE);
    put32(record, PCI_ID_FIELD, DEVICE_ID << 16 | NVIDIA_VENDOR);
    put32(record, ARCHITECTURE_FIELD, 0x190);
    put32(record, ARCHITECTURE_FIELD + 4, 3);
    put32(record, ARCHITECTURE_FIELD + 8, TEST_REVISION);
    for (uint32_t index = 0; index < GRAPHICS_ENTRIES; index++) {
        put32(record, place(GRAPHICS_TABLE_FIELD, moved) + index * 8, index);
    }
    put32(record, place(SM_VERSION_FIELD, moved), 0x904);
    put32(record, place(WINDOWS_VERSION_FIELD, moved), 284);
    put32(record, place(WINDOWS_VERSION_FIELD, moved) + 4, 10);
    put32(record, place(WINDOWS_VERSION_FIELD, moved) + 8, 0);
    put32(record, place(WINDOWS_VERSION_FIELD, moved) + 16, 2);
    put32(record, place(SHADER_SET_FIELD, moved), 0x1234);
    put64(record, place(MEMORY_FIELD, moved), 15ULL << 30);
    put64(record, place(MEMORY_FIELD, moved) + 8, 14ULL << 30);
    put64(record, place(MEMORY_FIELD, moved) + 24, 16ULL << 30);
    put32(record, place(NODE_COUNT_FIELD, moved), TEST_NODES);
    for (uint32_t node = 0; node < TEST_NODES; node++) {
        size_t entry = place(NODE_ENTRIES_FIELD, moved) + node * NODE_ENTRY_SIZE;
        size_t copy = place(NODE_RECORDS_FIELD, moved) + node * NODE_RECORD_SIZE;

        put32(record, entry, node + 1);
        put32(record, entry + 0x04, 1);
        put32(record, entry + 0x08, 0);
        put32(record, entry + 0x14, 4);
        put32(record, copy, 0x1A0);
        put32(record, copy + 0x08, 2);
        put32(record, copy + 0x0C, 9);
    }
    put32(record, place(CLASSES_FIELD, moved), TEST_CLASSES);
    memcpy(record + place(CLASSES_FIELD, moved) + 4, classes, sizeof(classes));
    memset(record + place(NAME_FIELD, moved), 0, 64);
    for (size_t i = 0; name[i]; i++) {
        record[place(NAME_FIELD, moved) + i * 2] = (uint8_t)name[i];
    }
    return record;
}

static uint8_t *make_record(size_t size)
{
    return make_moved_record(size, NO_MOVE);
}

/* The text a report gets from this record. The caller frees it. */
static char *report_with_nodes(const uint8_t *record, size_t size, uint16_t device_id, size_t windows_nodes)
{
    static Report report;
    RecordClues clues = { .revision = TEST_REVISION, .name = "NVIDIA Test Card" };
    char *text = NULL;
    size_t length = 0;
    FILE *out = open_memstream(&text, &length);

    memset(&report, 0, sizeof(report));
    report.listed_adapters = true;
    report.adapters_seen = 2;
    report.card_count = 1;
    report.cards[0].adapter.have_ids = true;
    report.cards[0].adapter.device_id = device_id;
    clues.device_id = device_id;
    clues.windows_nodes = windows_nodes;
    record_read(record, size, &clues, &report.cards[0].record);
    report_write(out, &report);
    fclose(out);
    return text;
}

static char *report_of(const uint8_t *record, size_t size, uint16_t device_id)
{
    return report_with_nodes(record, size, device_id, TEST_NODES);
}

static bool in_named_field(size_t at, size_t moved, bool searching)
{
    const struct { size_t start, size; } named[] = {
        { 0, 0x10 },
        { PCI_ID_FIELD, 4 },
        { ARCHITECTURE_FIELD, 12 },
        { place(SM_INDEX_FIELD, moved), 8 },
        { place(WINDOWS_VERSION_FIELD, moved), 8 },
        { place(SHADER_SET_FIELD, moved), 4 },
        { place(MEMORY_FIELD, moved), 16 },
        { place(MEMORY_FIELD, moved) + 24, 8 },
        { place(NODE_COUNT_FIELD, moved), 4 },
        { place(CLASSES_FIELD, moved), 4 + TEST_CLASSES * 4 },
        { place(NAME_FIELD, moved), 64 },
    };

    for (size_t i = 0; i < sizeof(named) / sizeof(named[0]); i++) {
        if (at >= named[i].start && at < named[i].start + named[i].size) {
            return true;
        }
    }
    if (searching) {
        size_t table = place(GRAPHICS_TABLE_FIELD, moved);
        size_t windows = place(WINDOWS_VERSION_FIELD, moved);

        for (size_t index = 0; index <= GRAPHICS_ENTRIES; index++) {
            if (at >= table + index * 8 && at < table + index * 8 + 4) {
                return true;
            }
        }
        if (at >= windows && at < windows + 20 && !(at >= windows + 12 && at < windows + 16)) {
            return true;
        }
    }
    for (size_t node = 0; node < TEST_NODES; node++) {
        size_t entry = place(NODE_ENTRIES_FIELD, moved) + node * NODE_ENTRY_SIZE;
        size_t record = place(NODE_RECORDS_FIELD, moved) + node * NODE_RECORD_SIZE;

        if ((at >= entry && at < entry + 12) || (at >= entry + 0x14 && at < entry + 0x18) ||
            (at >= record && at < record + 4) || (at >= record + 8 && at < record + 0x10)) {
            return true;
        }
    }
    return false;
}

static void the_named_fields_reach_the_report(void)
{
    uint8_t *record = make_record(RECORD_SIZE);
    char *text = report_of(record, RECORD_SIZE, DEVICE_ID);

    CHECK(strstr(text, "GPU name: NVIDIA Test Card\n"));
    CHECK(strstr(text, "Device ID: 0x2704\n"));
    CHECK(strstr(text, "Driver record size: 50616 bytes\n"));
    CHECK(strstr(text, "Driver record layout: 617\n"));
    CHECK(strstr(text, "Architecture: 0x190\n"));
    CHECK(strstr(text, "SM version: 0x904\n"));
    CHECK(strstr(text, "Shader set: 0x1234\n"));
    CHECK(strstr(text, "Video memory: 16384 MiB\n"));
    CHECK(strstr(text, "Engine nodes from the record: 2\n"));
    CHECK(strstr(text, CLASS_LINES));
    free(text);
    free(record);
}

static void only_named_bytes_change_the_report(size_t size, size_t moved, bool searching)
{
    uint8_t *record = make_moved_record(size, moved);
    char *expected = report_of(record, size, DEVICE_ID);

    for (size_t at = 0; at < size; at++) {
        char *text;

        record[at] ^= 0xFF;
        text = report_of(record, size, DEVICE_ID);
        if (strcmp(text, expected) && !in_named_field(at, moved, searching)) {
            printf("byte 0x%zX reached the report\n", at);
            failures++;
        }
        record[at] ^= 0xFF;
        free(text);
    }
    free(expected);
    free(record);
}

/* The real proof of what's read: change any other byte of the record and the report stays the same. */
static void no_other_byte_of_the_record_changes_the_report(void)
{
    only_named_bytes_change_the_report(RECORD_SIZE, NO_MOVE, false);
    only_named_bytes_change_the_report(RECORD_SIZE_621, MOVED_BY_621, false);
    front_move = 0x47;
    only_named_bytes_change_the_report(RECORD_SIZE + 0x123, 0x60, true);
    front_move = 0;
}

static void contains_hex(const char *text, const uint8_t *bytes, size_t size, const char *format)
{
    char hex[3 * UUID_SIZE] = "";

    for (size_t i = 0; i < size; i++) {
        snprintf(hex + i * 2, 3, format, bytes[i]);
    }
    CHECK(!strstr(text, hex));
}

static void check_no_private_values(const char *text)
{
    contains_hex(text, uuid, UUID_SIZE, "%02x");
    contains_hex(text, uuid, UUID_SIZE, "%02X");
    contains_hex(text, uuid, 4, "%02X");
    contains_hex(text, ecid, ECID_SIZE, "%02x");
    contains_hex(text, ecid, ECID_SIZE, "%02X");
    contains_hex(text, ecid, 4, "%02X");
}

static void the_uuid_and_ecid_do_not_appear(void)
{
    uint8_t *record = make_record(RECORD_SIZE);
    char *text = report_of(record, RECORD_SIZE, DEVICE_ID);

    check_no_private_values(text);
    free(text);
    free(record);
    record = make_moved_record(RECORD_SIZE_621, MOVED_BY_621);
    text = report_of(record, RECORD_SIZE_621, DEVICE_ID);
    check_no_private_values(text);
    free(text);
    free(record);
}

static void the_621_row_reads_every_field_and_says_it_is_unproven(void)
{
    uint8_t *record = make_moved_record(RECORD_SIZE_621, MOVED_BY_621);
    char *text = report_with_nodes(record, RECORD_SIZE_621, DEVICE_ID, TEST_NODES);

    CHECK(strstr(text, "Driver record size: 52728 bytes\n"));
    CHECK(strstr(text, "Driver record layout: 621, not yet checked against a real record\n"));
    CHECK(strstr(text, "Architecture: 0x190\n"));
    CHECK(strstr(text, "SM version: 0x904\n"));
    CHECK(strstr(text, "Shader set: 0x1234\n"));
    CHECK(strstr(text, "GPU name: NVIDIA Test Card\n"));
    CHECK(strstr(text, "Video memory: 16384 MiB\n"));
    CHECK(strstr(text, "Engine nodes from the record: 2\n"));
    CHECK(strstr(text, CLASS_LINES));
    free(text);
    free(record);
}

static void check_moved_fields_left_out(const char *text)
{
    CHECK(strstr(text, "Architecture: 0x190\n"));
    CHECK(strstr(text, "SM version: 0x904\n"));
    CHECK(strstr(text, "Shader set: 0x1234\n"));
    CHECK(strstr(text, "Video memory: can't be read, this field moved"));
    CHECK(!strstr(text, "NVIDIA Test Card"));
    check_no_private_values(text);
}

/* If the move isn't what the row says, the row's places aren't used, and what's found says so. */
static void any_other_move_is_left_to_the_search(void)
{
    for (size_t moved = 0; moved <= 0x1000; moved++) {
        uint8_t *record;
        char *text;

        if (moved == MOVED_BY_621) {
            continue;
        }
        record = make_moved_record(RECORD_SIZE_621 + 0x800, moved);
        put32(record, 0x08, RECORD_SIZE_621);
        text = report_of(record, RECORD_SIZE_621, DEVICE_ID);
        check_moved_fields_left_out(text);
        CHECK(strstr(text, "Classes: 16, from searching\n"));
        CHECK(strstr(text, "Engine nodes from the record: 2, from searching\n"));
        free(text);
        free(record);
    }
}

/* The ECID sits just before the node list, so a list a little out of place would start on it. */
static void a_node_list_starting_on_the_ecid_is_not_read_from_there(void)
{
    uint8_t *record = make_moved_record(RECORD_SIZE_621, MOVED_BY_621);
    size_t entries = NODE_ENTRIES_FIELD + MOVED_BY_621;
    char *text;

    memmove(record + entries + 8, record + entries, TEST_NODES * NODE_ENTRY_SIZE);
    memcpy(record + entries, ecid, ECID_SIZE);
    text = report_of(record, RECORD_SIZE_621, DEVICE_ID);
    check_moved_fields_left_out(text);
    free(text);
    free(record);
}

static void a_different_node_count_from_windows_leaves_the_nodes_out(void)
{
    uint8_t *record = make_moved_record(RECORD_SIZE_621, MOVED_BY_621);
    char *text = report_with_nodes(record, RECORD_SIZE_621, DEVICE_ID, TEST_NODES + 1);

    check_moved_fields_left_out(text);
    CHECK(strstr(text, "Engine nodes from the record: can't be read, this field moved"));
    free(text);
    free(record);
}

static void moved_fields_that_fail_one_check_are_all_left_out(void)
{
    uint8_t *record = make_moved_record(RECORD_SIZE_621, MOVED_BY_621);
    char *text;

    put64(record, MEMORY_FIELD + MOVED_BY_621 + 24, 1);
    text = report_of(record, RECORD_SIZE_621, DEVICE_ID);
    check_moved_fields_left_out(text);
    free(text);
    free(record);
}

static void a_621_record_with_its_early_fields_moved_is_not_read(void)
{
    uint8_t *record = make_moved_record(RECORD_SIZE_621, MOVED_BY_621);
    char *text;

    put32(record, SM_INDEX_FIELD, 0xA5A5A5A5);
    text = report_of(record, RECORD_SIZE_621, DEVICE_ID);
    CHECK(strstr(text, "didn't pass its checks"));
    CHECK(!strstr(text, "SM version: 0x"));
    CHECK(!strstr(text, "Shader set: 0x"));
    free(text);
    put32(record, SM_INDEX_FIELD, 0x0C);
    put32(record, WINDOWS_VERSION_FIELD, 0xA5A5A5A5);
    text = report_of(record, RECORD_SIZE_621, DEVICE_ID);
    CHECK(strstr(text, "didn't pass its checks"));
    free(text);
    free(record);
}

static void the_largest_size_is_asked_for_first(void)
{
    CHECK(record_known_size(0) == RECORD_SIZE_621);
    CHECK(record_known_size(1) == RECORD_SIZE);
    CHECK(record_known_size(2) == 0);
}

static void check_found_by_searching(const char *text)
{
    CHECK(strstr(text, "Architecture: 0x190, from searching\n"));
    CHECK(strstr(text, "SM version: 0x904, from searching\n"));
    CHECK(strstr(text, "Shader set: 0x1234, a guess"));
    CHECK(strstr(text, "Engine nodes from the record: 2, from searching\n"));
    CHECK(strstr(text, "Classes: 16, from searching\n"));
    CHECK(strstr(text, "Video memory: can't be read"));
    check_no_private_values(text);
}

static void an_unknown_size_says_so_and_finds_what_it_can(void)
{
    size_t size = 0xD000;
    uint8_t *record = make_record(size);
    char *text = report_of(record, size, DEVICE_ID);

    CHECK(strstr(text, "Driver record size: 53248 bytes\n"));
    CHECK(strstr(text, "Driver record layout: can't be read, this program has no layout"));
    CHECK(strstr(text, "Device ID: 0x2704\n"));
    CHECK(strstr(text, "  PCI ID: found at 0x3C, 617 has it at 0x3C\n"));
    check_found_by_searching(text);
    free(text);
    free(record);
}

static void a_size_in_the_header_beats_the_size_asked_for(void)
{
    uint8_t *record = make_record(RECORD_SIZE);
    char *text;

    put32(record, 0x08, RECORD_SIZE + 8);
    text = report_of(record, RECORD_SIZE, DEVICE_ID);
    CHECK(strstr(text, "this program has no layout"));
    check_found_by_searching(text);
    free(text);
    free(record);
}

/* Moves and resizes, the 621 shape among them, on a size no layout has. */
static void the_same_fields_are_found_however_the_record_moved(void)
{
    static const size_t fronts[] = { 0, 1, 4, 7, 0x40, 0x333 };
    static const size_t moves[] = { 0, 1, 8, 0x100, 0x83F, MOVED_BY_621, 0x900 };

    for (size_t f = 0; f < sizeof(fronts) / sizeof(fronts[0]); f++) {
        for (size_t m = 0; m < sizeof(moves) / sizeof(moves[0]); m++) {
            size_t size = RECORD_SIZE + fronts[f] + moves[m] + 0x10;
            char expected[64];
            uint8_t *record;
            char *text;

            front_move = fronts[f];
            record = make_moved_record(size, moves[m]);
            text = report_of(record, size, DEVICE_ID);
            check_found_by_searching(text);
            CHECK(strstr(text, CLASS_LINES + strlen("Classes: 16\n")));
            snprintf(expected, sizeof(expected), "  Class list: found at 0x%zX, 617 has it at 0x%X\n",
                     place(CLASSES_FIELD, moves[m]), CLASSES_FIELD);
            CHECK(strstr(text, expected));
            snprintf(expected, sizeof(expected), "  Node list: found at 0x%zX, 617 has it at 0x%X\n",
                     place(NODE_ENTRIES_FIELD, moves[m]), NODE_ENTRIES_FIELD);
            CHECK(strstr(text, expected));
            free(text);
            free(record);
        }
    }
    front_move = 0;
}

static void two_places_for_anything_means_it_is_not_read(void)
{
    size_t size = 0xD000;
    uint8_t *record = make_record(size);
    char *text;

    memcpy(record + 0xC800, record + CLASSES_FIELD, 4 + TEST_CLASSES * 4);
    memcpy(record + 0xCA00, record + GRAPHICS_TABLE_FIELD, GRAPHICS_ENTRIES * 8);
    text = report_of(record, size, DEVICE_ID);
    CHECK(strstr(text, "  Class list: found in 2 places, so none were read\n"));
    CHECK(strstr(text, "  Graphics table: found in 2 places, so none were read\n"));
    CHECK(strstr(text, "Classes: can't be read"));
    CHECK(strstr(text, "SM version: can't be read"));
    CHECK(strstr(text, "Shader set: can't be read"));
    CHECK(strstr(text, "Architecture: 0x190, from searching\n"));
    free(text);
    free(record);
}

static void the_card_id_in_two_places_stops_every_read(void)
{
    size_t size = 0xD000;
    uint8_t *record = make_record(size);
    char *text;

    put32(record, 0xC800, DEVICE_ID << 16 | NVIDIA_VENDOR);
    text = report_of(record, size, DEVICE_ID);
    CHECK(strstr(text, "  PCI ID: found in 2 places, so none were read\n"));
    CHECK(!strstr(text, "from searching"));
    CHECK(!strstr(text, "a guess"));
    CHECK(strstr(text, "SM version: can't be read"));
    free(text);
    free(record);
}

static void no_shader_set_guess_if_its_neighbours_moved_apart(void)
{
    size_t size = 0xD000;
    uint8_t *record = make_record(size);
    size_t block = WINDOWS_VERSION_FIELD;
    char *text;

    memmove(record + block + 4, record + block, 20);
    put32(record, SHADER_SET_FIELD + 4, 0x5678);
    text = report_of(record, size, DEVICE_ID);
    CHECK(strstr(text, "SM version: 0x904, from searching\n"));
    CHECK(strstr(text, "Shader set: can't be read"));
    free(text);
    free(record);
}

static void a_right_size_with_the_wrong_card_in_it_is_not_read(void)
{
    uint8_t *record = make_record(RECORD_SIZE);
    char *text = report_of(record, RECORD_SIZE, 0x1111);

    CHECK(strstr(text, "didn't pass its checks"));
    CHECK(!strstr(text, "SM version: 0x"));
    free(text);
    free(record);
}

static void a_record_cut_short_is_not_read(void)
{
    uint8_t *record = make_record(RECORD_SIZE);
    char *text = report_of(record, 0x100, DEVICE_ID);

    CHECK(strstr(text, "didn't pass its checks"));
    free(text);
    free(record);
}

static void something_that_is_not_a_record_says_so(void)
{
    uint8_t zeros[32] = { 0 };
    char *text = report_of(zeros, sizeof(zeros), DEVICE_ID);

    CHECK(strstr(text, "isn't a record this program knows"));
    free(text);
}

static void a_list_longer_than_the_record_has_room_for_is_not_read(void)
{
    uint8_t *record = make_record(RECORD_SIZE);
    char *text;

    put32(record, CLASSES_FIELD, MOST_CLASSES + 1);
    put32(record, NODE_COUNT_FIELD, MOST_NODES + 1);
    text = report_of(record, RECORD_SIZE, DEVICE_ID);
    CHECK(strstr(text, "Classes: can't be read"));
    CHECK(strstr(text, "Engine nodes from the record: can't be read"));
    free(text);
    free(record);
}

static void a_zero_field_says_it_cannot_be_read_unless_zero_is_a_real_answer(void)
{
    uint8_t *record = make_record(RECORD_SIZE);
    char *text;

    put32(record, SHADER_SET_FIELD, 0);
    put32(record, ARCHITECTURE_FIELD + 4, 0);
    text = report_of(record, RECORD_SIZE, DEVICE_ID);
    CHECK(strstr(text, "Shader set: can't be read, the record had nothing there\n"));
    CHECK(strstr(text, "Implementation: 0x0\n"));
    free(text);
    free(record);
}

static void no_adapter_is_a_short_report(void)
{
    Report report = { .listed_adapters = true, .adapters_seen = 2 };
    char *text = NULL;
    size_t length = 0;
    FILE *out = open_memstream(&text, &length);

    report_write(out, &report);
    fclose(out);
    CHECK(strstr(text, "NVIDIA adapter: none found, out of 2 adapters\n"));
    CHECK(!strstr(text, "Card 1"));
    free(text);
}

static void odd_characters_in_a_name_do_not_get_through(void)
{
    static const uint16_t name[] = { 'A', 0x7, 0xE9, '\n', 'B', 0 };
    char text[8];

    plain_text(text, sizeof(text), name, 6);
    CHECK(!strcmp(text, "A???B"));
    plain_text(text, 3, name, 6);
    CHECK(!strcmp(text, "A?"));
}

int main(void)
{
    the_named_fields_reach_the_report();
    no_other_byte_of_the_record_changes_the_report();
    the_uuid_and_ecid_do_not_appear();
    the_621_row_reads_every_field_and_says_it_is_unproven();
    any_other_move_is_left_to_the_search();
    a_node_list_starting_on_the_ecid_is_not_read_from_there();
    a_different_node_count_from_windows_leaves_the_nodes_out();
    moved_fields_that_fail_one_check_are_all_left_out();
    a_621_record_with_its_early_fields_moved_is_not_read();
    the_largest_size_is_asked_for_first();
    an_unknown_size_says_so_and_finds_what_it_can();
    a_size_in_the_header_beats_the_size_asked_for();
    the_same_fields_are_found_however_the_record_moved();
    two_places_for_anything_means_it_is_not_read();
    the_card_id_in_two_places_stops_every_read();
    no_shader_set_guess_if_its_neighbours_moved_apart();
    a_right_size_with_the_wrong_card_in_it_is_not_read();
    a_record_cut_short_is_not_read();
    something_that_is_not_a_record_says_so();
    a_list_longer_than_the_record_has_room_for_is_not_read();
    a_zero_field_says_it_cannot_be_read_unless_zero_is_a_real_answer();
    no_adapter_is_a_short_report();
    odd_characters_in_a_name_do_not_get_through();
    printf(failures ? "%d failed\n" : "all passed\n", failures);
    return failures != 0;
}
