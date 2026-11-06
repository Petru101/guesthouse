/*
 * The messages between the display driver in the guest and the daemon on the host.
 *
 * They travel as the private data of D3DKMTEscape, so the daemon's reply lands in the same buffer. Everything is little endian
 * with no padding the compiler chose. notes/idd-escape-protocol.md has the same layout as a table for the daemon's side.
 *
 * Who owns what: the daemon alone decides which slot is free. The driver only ever writes the slot the last reply named.
 */
#pragma once

#include <stdint.h>

/* "GHOUSEDP" and "GHOUSERP". A reply has its own magic because an escape nobody claims comes back with the request's. */
#define GH_REQUEST_MAGIC 0x50444553554F4847ull
#define GH_REPLY_MAGIC 0x50524553554F4847ull
/* "GHOUSEST", at the start of each slot until the first frame overwrites it. */
#define GH_STAMP_MAGIC 0x54534553554F4847ull

#define GH_PROTOCOL_VERSION 1u

#define GH_KIND_REGISTER 1u
#define GH_KIND_FRAME 2u
#define GH_KIND_CLOSE 3u

/* In a FRAME, nothing was written and the driver only wants a slot. In a reply, nothing is free yet. */
#define GH_NO_SLOT 0xFFFFFFFFu
#define GH_MAX_SLOTS 8u

#define GH_STATUS_OK 0u
/* The generation isn't the daemon's current one. The driver should REGISTER again. */
#define GH_STATUS_STALE_GENERATION 1u
/* The daemon can't use this ring. The driver shouldn't try again until the mode changes. */
#define GH_STATUS_REFUSED 2u

/* DRM fourcc codes, so that neither side needs a table. */
#define GH_FOURCC_XRGB8888 0x34325258u
#define GH_FOURCC_XBGR2101010 0x30334258u

/* H.273 code points. */
#define GH_CICP_PRIMARIES_BT709 1u
#define GH_CICP_PRIMARIES_BT2020 9u
#define GH_CICP_TRANSFER_SRGB 13u
#define GH_CICP_TRANSFER_PQ 16u
#define GH_CICP_MATRIX_IDENTITY 0u

#define GH_FRAME_HAS_HDR_METADATA 0x1u

typedef struct gh_header {
	uint64_t magic;
	uint16_t version;
	uint16_t kind;
	uint32_t size; /* Of the whole message, this header included. */
	uint32_t generation;
	uint32_t reserved;
} gh_header;

/* Sent once for each ring, after the slots exist and are stamped. Slots are all the same shape. */
typedef struct gh_register {
	gh_header header;
	uint32_t slot_count;
	uint32_t width;
	uint32_t height;
	uint32_t pitch; /* Bytes from one row to the next. */
	uint64_t slot_size; /* What the buffer was created with, for finding it among the guest's allocations. */
	uint32_t fourcc;
	uint32_t monitor;
	uint8_t cicp_primaries;
	uint8_t cicp_transfer;
	uint8_t cicp_matrix;
	uint8_t full_range;
	uint32_t reserved;
} gh_register;

/* Mastering data as IddCx gives it. Primaries are in units of 0.00002, minimum luminance in units of 0.0001 nits. */
typedef struct gh_hdr_metadata {
	uint16_t red[2];
	uint16_t green[2];
	uint16_t blue[2];
	uint16_t white[2];
	uint16_t max_mastering_nits;
	uint16_t min_mastering;
	uint16_t max_content_nits;
	uint16_t max_frame_average_nits;
} gh_hdr_metadata;

/* Sent after the pass has been flushed. The GPU may still be writing: the daemon waits on the sender's queues. */
typedef struct gh_frame {
	gh_header header;
	uint32_t slot;
	uint32_t flags;
	uint64_t sequence;
	uint64_t present_qpc; /* When Windows wants it shown, in the guest's clock. */
	uint32_t windows_frame;
	uint32_t sdr_white_nits;
	gh_hdr_metadata hdr;
} gh_frame;

/* The ring is going away, because the swap chain did. */
typedef struct gh_close {
	gh_header header;
} gh_close;

/* The answer to every request. The largest request is what the buffer is sized for, so this always fits. */
typedef struct gh_reply {
	gh_header header;
	uint32_t status;
	uint32_t next_slot;
	uint32_t monitor_millihertz; /* The host monitor's rate, 0 if not known. */
	uint32_t peak_nits; /* The host monitor's peak, 0 if not known. */
} gh_reply;

/* Written over the first bytes of every slot before REGISTER, so the daemon can tell slots apart for certain. */
typedef struct gh_slot_stamp {
	uint64_t magic;
	uint32_t generation;
	uint32_t slot;
} gh_slot_stamp;

#ifdef __cplusplus
#define GH_STATIC_ASSERT static_assert
#else
#define GH_STATIC_ASSERT _Static_assert
#endif

GH_STATIC_ASSERT(sizeof(gh_header) == 24, "gh_header is part of the wire format");
GH_STATIC_ASSERT(sizeof(gh_register) == 64, "gh_register is part of the wire format");
GH_STATIC_ASSERT(sizeof(gh_hdr_metadata) == 24, "gh_hdr_metadata is part of the wire format");
GH_STATIC_ASSERT(sizeof(gh_frame) == 80, "gh_frame is part of the wire format");
GH_STATIC_ASSERT(sizeof(gh_close) == 24, "gh_close is part of the wire format");
GH_STATIC_ASSERT(sizeof(gh_reply) == 40, "gh_reply is part of the wire format");
GH_STATIC_ASSERT(sizeof(gh_slot_stamp) == 16, "gh_slot_stamp is part of the wire format");
