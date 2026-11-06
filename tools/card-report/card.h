/*
 * What the report can say. Only these structs reach the report, so anything that isn't a field
 * here can't end up in the file.
 */
#ifndef CARD_H
#define CARD_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

#define NVIDIA_VENDOR 0x10DE
#define MOST_CARDS 4
#define MOST_NODES 31
#define MOST_CLASSES 200
#define NAME_SIZE 64
#define NODE_NAME_SIZE 33

/* One engine node, as Windows lists it. */
typedef struct {
    uint32_t engine_type;
    char name[NODE_NAME_SIZE];
} WindowsNode;

/* What the ordinary adapter queries say. A field nobody could read stays false or empty. */
typedef struct {
    bool have_ids;
    uint16_t device_id;
    uint8_t revision;
    char name[NAME_SIZE];
    bool have_driver_version;
    uint16_t driver_version[4];
    uint32_t driver_model;
    size_t node_count;
    WindowsNode nodes[MOST_NODES];
} AdapterFacts;

/* One engine node, as the driver's record lists it. */
typedef struct {
    uint32_t node_type;
    uint32_t scheduled;
    uint32_t in_flight;
    uint32_t copy_flags;
    uint32_t copy_engine_count;
    uint32_t copy_engine;
} RecordNode;

typedef enum {
    RECORD_REFUSED,      /* the driver wouldn't hand it over */
    RECORD_NOT_ONE,      /* something came back, but it doesn't start like the record */
    RECORD_OTHER_LAYOUT, /* its size isn't one this program has a layout for */
    RECORD_FAILED_CHECK, /* the size fits, but what's inside doesn't */
    RECORD_READ,
} RecordState;

/* What a search of the record looks for. */
typedef enum {
    LANDMARK_MARKER,
    LANDMARK_PCI_ID,
    LANDMARK_ARCHITECTURE,
    LANDMARK_GRAPHICS_TABLE,
    LANDMARK_WINDOWS_VERSION,
    LANDMARK_NODES,
    LANDMARK_CLASSES,
    LANDMARK_NAME,
    LANDMARK_COUNT,
} LandmarkKind;

/* Where a search found one thing. Offsets only: nothing from the record itself. */
typedef struct {
    size_t count;
    uint32_t found_at;
    uint32_t known_at;
} Landmark;

/* The named fields of the driver's record. Zero means the record had nothing there. */
typedef struct {
    RecordState state;
    uint32_t size;
    uint16_t format_version;
    uint16_t format_revision;
    const char *layout;
    bool layout_unproven;
    bool moved_fields_unread;
    char name[NAME_SIZE];
    uint32_t architecture;
    uint32_t implementation;
    uint32_t chip_revision;
    uint32_t sm_version;
    uint32_t shader_set;
    uint64_t memory_total;
    uint64_t memory_heap;
    uint64_t memory_usable;
    bool nodes_readable;
    size_t node_count;
    RecordNode nodes[MOST_NODES];
    bool classes_readable;
    size_t class_count;
    uint32_t classes[MOST_CLASSES];
    bool searched;
    Landmark landmarks[LANDMARK_COUNT];
    bool architecture_searched;
    bool sm_version_searched;
    bool shader_set_guessed;
    bool nodes_searched;
    bool classes_searched;
} RecordFacts;

/* What Windows already said about the card, to tell the record's fields by. */
typedef struct {
    uint16_t device_id;
    uint8_t revision;
    size_t windows_nodes;
    const char *name;
} RecordClues;

typedef struct {
    AdapterFacts adapter;
    RecordFacts record;
} Card;

typedef struct {
    bool have_windows_version;
    uint32_t windows_version[3];
    bool listed_adapters;
    size_t adapters_seen;
    size_t card_count;
    Card cards[MOST_CARDS];
} Report;

/* The record sizes this program has a layout for, largest first, or zero past the last one. */
uint32_t record_known_size(size_t index);

/* Reads the named fields out of `size` bytes of record, and writes down no other byte of it. */
void record_read(const uint8_t *data, size_t size, const RecordClues *clues, RecordFacts *facts);

/* Copies text, keeping plain printable characters and cutting it to fit. */
void plain_text(char *to, size_t room, const uint16_t *from, size_t length);

void report_write(FILE *out, const Report *report);

#endif
