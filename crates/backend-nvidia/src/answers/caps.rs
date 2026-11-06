//! The short answer the guest's video runtime asks for, in place of the whole record.
//!
//! It's the record query with a smaller size. What it wants is the block of engine capabilities from
//! near the record's start, moved down, then the class list. A class count taken from the wrong
//! place here walks the runtime off the end of its buffer.

use crate::profile::RecordLayout;

/// The size this answer is asked for with.
pub const CAPS_SIZE: usize = 0x40D;

/// The guest's header, kept as it sent it, with what it's asking for in the last word.
const HEADER_SIZE: usize = 0x14;
/// The capabilities block, in the answer and in the record.
const BLOCK_AT: usize = 0x18;
const BLOCK_IN_RECORD: usize = 0x105;
const BLOCK_SIZE: usize = 0xD1;
const CLASS_COUNT_AT: usize = BLOCK_AT + BLOCK_SIZE;

/// Builds the answer from the record, which already holds all of it.
pub fn caps_answer(record: &[u8], layout: &RecordLayout, request: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    out.resize(start + CAPS_SIZE, 0);
    let answer = &mut out[start..];
    let header = request.len().min(HEADER_SIZE);
    answer[..header].copy_from_slice(&request[..header]);
    if let Some(block) = record.get(BLOCK_IN_RECORD..BLOCK_IN_RECORD + BLOCK_SIZE) {
        answer[BLOCK_AT..CLASS_COUNT_AT].copy_from_slice(block);
    }
    let room = CAPS_SIZE - CLASS_COUNT_AT;
    if let Some(classes) = record.get(layout.classes..) {
        let count = classes.first_chunk::<4>().map_or(0, |bytes| u32::from_le_bytes(*bytes)) as usize;
        let size = (4 + count * 4).min(room).min(classes.len());
        answer[CLASS_COUNT_AT..CLASS_COUNT_AT + size].copy_from_slice(&classes[..size]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::GUEST_617;

    const LAYOUT: &RecordLayout = &GUEST_617.record;

    fn record() -> Vec<u8> {
        let mut record = vec![0; LAYOUT.size];
        record[BLOCK_IN_RECORD] = 0xAB;
        record[LAYOUT.classes..LAYOUT.classes + 4].copy_from_slice(&2u32.to_le_bytes());
        record[LAYOUT.classes + 4..LAYOUT.classes + 8].copy_from_slice(&0xCFB0u32.to_le_bytes());
        record[LAYOUT.classes + 8..LAYOUT.classes + 12].copy_from_slice(&0xCFB7u32.to_le_bytes());
        record
    }

    fn word(data: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn the_answer_is_the_size_asked_and_keeps_the_guests_header() {
        let mut out = Vec::new();
        caps_answer(&record(), LAYOUT, &[9; 0x40], &mut out);
        assert_eq!(out.len(), CAPS_SIZE);
        assert_eq!(out[HEADER_SIZE - 1], 9);
        assert_eq!(out[HEADER_SIZE], 0);
    }

    #[test]
    fn the_block_and_the_classes_land_where_the_runtime_reads_them() {
        let mut out = Vec::new();
        caps_answer(&record(), LAYOUT, &[], &mut out);
        assert_eq!(out[BLOCK_AT], 0xAB);
        assert_eq!(word(&out, CLASS_COUNT_AT), 2);
        assert_eq!(word(&out, CLASS_COUNT_AT + 8), 0xCFB7);
    }

    #[test]
    fn a_list_too_long_for_the_answer_stops_at_its_end() {
        let mut record = record();
        record[LAYOUT.classes..LAYOUT.classes + 4].copy_from_slice(&0xFFFFu32.to_le_bytes());
        let mut out = Vec::new();
        caps_answer(&record, LAYOUT, &[], &mut out);
        assert_eq!(out.len(), CAPS_SIZE);
    }
}
