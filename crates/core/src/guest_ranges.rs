//! Handing out stretches of the shared window.

use guesthouse_messages::wire::{align_up, checked_align_up};

/// Ranges are made and joined in these units.
pub const GUEST_RANGE_GRAIN: u64 = 64 << 10;

/// First-fit over a sorted free list, falling back on fresh space at the end.
#[derive(Debug, Default)]
pub struct GuestRanges {
    /// (address, size), in address order, with neighbours joined.
    free: Vec<(u64, u64)>,
    used: u64,
}

impl GuestRanges {
    pub fn new() -> Self {
        Self::default()
    }

    /// Finds room for `size` bytes. `fresh_start` is where never-used space starts and `limit`
    /// where it ends. The second value says the range held someone else's data before.
    pub fn take(&mut self, size: u64, fresh_start: u64, limit: u64) -> Option<(u64, bool)> {
        let size = checked_align_up(size, GUEST_RANGE_GRAIN)?;
        if let Some(index) = self.free.iter().position(|&(_, length)| length >= size) {
            let (start, length) = self.free[index];
            if length == size {
                self.free.remove(index);
            } else {
                self.free[index] = (start + size, length - size);
            }
            return Some((start, true));
        }
        let address = fresh_start.checked_add(self.used)?;
        address.checked_add(size).filter(|&end| end <= limit)?;
        self.used += size;
        Some((address, false))
    }

    /// Takes a range back, joining it to any free neighbours.
    pub fn release(&mut self, address: u64, size: u64) {
        let size = align_up(size, GUEST_RANGE_GRAIN);
        let index = self.free.partition_point(|&entry| entry < (address, size));
        self.free.insert(index, (address, size));
        if let Some(&(next, next_size)) = self.free.get(index + 1)
            && address + size == next
        {
            self.free[index].1 += next_size;
            self.free.remove(index + 1);
        }
        if index > 0 {
            let (before, before_size) = self.free[index - 1];
            if before + before_size == address {
                self.free[index - 1].1 += self.free[index].1;
                self.free.remove(index);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAIN: u64 = GUEST_RANGE_GRAIN;
    const START: u64 = 0x100_0000;
    const LIMIT: u64 = 0x200_0000;

    #[test]
    fn fresh_space_rounds_up_to_the_grain() {
        let mut ranges = GuestRanges::new();
        assert_eq!(ranges.take(1, START, LIMIT), Some((START, false)));
        assert_eq!(ranges.take(1, START, LIMIT), Some((START + GRAIN, false)));
    }

    #[test]
    fn freed_space_comes_back_first_and_is_marked_used() {
        let mut ranges = GuestRanges::new();
        let (first, _) = ranges.take(GRAIN, START, LIMIT).unwrap();
        ranges.take(GRAIN, START, LIMIT).unwrap();
        ranges.release(first, GRAIN);
        assert_eq!(ranges.take(GRAIN, START, LIMIT), Some((first, true)));
    }

    #[test]
    fn a_bigger_block_is_split_from_its_front() {
        let mut ranges = GuestRanges::new();
        let (first, _) = ranges.take(3 * GRAIN, START, LIMIT).unwrap();
        ranges.release(first, 3 * GRAIN);
        assert_eq!(ranges.take(GRAIN, START, LIMIT), Some((first, true)));
        assert_eq!(ranges.take(GRAIN, START, LIMIT), Some((first + GRAIN, true)));
    }

    #[test]
    fn neighbours_join_whichever_side_comes_back_first() {
        for order in [[0, 1, 2], [2, 1, 0], [0, 2, 1]] {
            let mut ranges = GuestRanges::new();
            let parts: Vec<u64> = (0..3).map(|_| ranges.take(GRAIN, START, LIMIT).unwrap().0).collect();
            order.iter().for_each(|&index| ranges.release(parts[index], GRAIN));
            assert_eq!(ranges.free, [(START, 3 * GRAIN)], "order {order:?}");
        }
    }

    #[test]
    fn nothing_past_the_limit_is_given_out() {
        let mut ranges = GuestRanges::new();
        assert_eq!(ranges.take(LIMIT, START, LIMIT), None);
        assert_eq!(ranges.take(LIMIT - START, START, LIMIT), Some((START, false)));
        assert_eq!(ranges.take(1, START, LIMIT), None);
    }

    #[test]
    fn a_size_too_large_to_round_up_is_no_room_at_all() {
        assert_eq!(GuestRanges::new().take(u64::MAX, START, LIMIT), None);
    }
}
