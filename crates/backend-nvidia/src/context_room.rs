//! Room in every address space for the RM's own context buffers.
//!
//! The RM maps each channel group's context buffers into the group's address space, wherever the
//! space has room. A graphics group takes tens of MiB, some of it on large boundaries, and the gaps
//! the guest's ranges leave held only about eight groups. Past that the RM left buffers out, and a
//! few more groups and it refused the channel. Some units hold these addresses in 40 bits, so the
//! room has to be below 1 TiB too. It is the top of the guest's last range, which nothing of the
//! guest's own reaches.

use crate::escape::{RUNTIME_GPU_RANGE, RUNTIME_PLACED_RANGE};
use guesthouse_backend::GpuRange;

/// Never reserved for the guest's mappings, and never handed out.
pub const CONTEXT_BUFFER_ROOM: GpuRange = GpuRange { start: 0xF0_0000_0000, end: 0x100_0000_0000 };

const _: () = assert!(CONTEXT_BUFFER_ROOM.end <= 1 << 40, "the room must stay within 40 bits");
const _: () = assert!(RUNTIME_GPU_RANGE.end <= CONTEXT_BUFFER_ROOM.start);
const _: () = assert!(RUNTIME_PLACED_RANGE.end <= CONTEXT_BUFFER_ROOM.start);

/// `ranges` with the room taken out, as (start, end).
pub fn without_room(ranges: &[GpuRange]) -> Vec<(u64, u64)> {
    let room = CONTEXT_BUFFER_ROOM;
    let mut left = Vec::with_capacity(ranges.len() + 1);
    for range in ranges {
        let before = (range.start, range.end.min(room.start));
        let after = (range.start.max(room.end), range.end);
        left.extend([before, after].into_iter().filter(|(start, end)| start < end));
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::GPU_RANGES;

    #[test]
    fn the_room_comes_off_the_top_of_the_last_range() {
        let first = (GPU_RANGES[0].start, GPU_RANGES[0].end);
        let second = (GPU_RANGES[1].start, CONTEXT_BUFFER_ROOM.start);
        assert_eq!(without_room(&GPU_RANGES), vec![first, second]);
        assert_eq!(CONTEXT_BUFFER_ROOM.end, GPU_RANGES[1].end);
    }

    #[test]
    fn a_range_across_the_room_keeps_both_sides() {
        let range = GpuRange { start: 0xE0_0000_0000, end: 0x110_0000_0000 };
        let sides = vec![(0xE0_0000_0000, 0xF0_0000_0000), (0x100_0000_0000, 0x110_0000_0000)];
        assert_eq!(without_room(&[range]), sides);
    }
}
