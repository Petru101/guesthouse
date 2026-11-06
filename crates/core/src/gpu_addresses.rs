//! Handing out GPU addresses, and taking them back.

use guesthouse_backend::{BackendError, GpuRange};
use guesthouse_messages::wire::align_up;
use std::collections::HashMap;

/// The ranges every address space covers. There is a hole around 4 GiB, where real drivers keep
/// something of their own, and the ranges are big because one process can reserve gigabytes.
pub const GPU_RANGES: [GpuRange; 2] = [
    GpuRange { start: 0x400_0000, end: 0x1_0000_0000 },
    GpuRange { start: 0x1_4000_0000, end: 0x80_0000_0000 },
];

/// Past the last address any mapping can have, which also makes it more than any size can be.
pub const ADDRESSES_END: u64 = GPU_RANGES[1].end;

/// The alignment of ordinary mappings.
pub const ALLOCATION_ALIGN: u64 = 64 << 10;

/// The alignment of mappings big enough for 2 MiB pages, which must start on a 2 MiB boundary.
pub const LARGE_ALIGN: u64 = 2 << 20;

/// What a range of addresses is for. A range only ever goes back to the same use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressUse {
    /// The host's own, for channels and copies. These are never freed.
    Host,
    /// A slot of the frame ring, which comes and goes with viewers and screen modes.
    FrameRing,
    /// A whole allocation, mapped where the host chose.
    Mapped,
    /// Set aside by the guest, for mappings it places itself.
    Reserved,
}

/// A bump allocator with a free list per shape of range. Addresses stay unique across every address
/// space at any one time, so an address always names one allocation.
///
/// A program that makes and frees memory all day would run out of new addresses in the end. Freed
/// ranges are not split or joined, and go back only to the same use, size and alignment, because
/// handing one to a different use has made the vendor's driver refuse mappings.
#[derive(Debug)]
pub struct GpuAddresses {
    next: u64,
    free: HashMap<(AddressUse, u64, u64), Vec<u64>>,
    /// Where new addresses come from: the ranges, less what a backend keeps for the guest.
    hand_out: Vec<GpuRange>,
}

/// `ranges` without `kept`, in order.
fn without(ranges: &[GpuRange], kept: &GpuRange) -> Vec<GpuRange> {
    let mut left = Vec::with_capacity(ranges.len() + 1);
    for range in ranges {
        let before = GpuRange { start: range.start, end: range.end.min(kept.start) };
        let after = GpuRange { start: range.start.max(kept.end), end: range.end };
        left.extend([before, after].into_iter().filter(|part| part.start < part.end));
    }
    left
}

impl GpuAddresses {
    pub fn new() -> Self {
        Self::keeping_clear(&[])
    }

    /// Never hands out anything in `kept`. Those addresses still work, as the guest may map there.
    pub fn keeping_clear(kept: &[GpuRange]) -> Self {
        let hand_out = kept.iter().fold(GPU_RANGES.to_vec(), |left, range| without(&left, range));
        Self { next: GPU_RANGES[0].start, free: HashMap::new(), hand_out }
    }

    /// Takes `size` bytes: what was given back for this use, or new addresses, moving to the next
    /// range if this one is too full.
    pub fn take(&mut self, size: u64, align: u64, usage: AddressUse) -> Result<u64, BackendError> {
        if let Some(spare) = self.free.get_mut(&(usage, size, align)).and_then(Vec::pop) {
            return Ok(spare);
        }
        for range in &self.hand_out {
            let base = align_up(self.next.max(range.start), align);
            if let Some(end) = base.checked_add(size).filter(|&end| end <= range.end) {
                self.next = end;
                return Ok(base);
            }
        }
        Err(BackendError::new("no room left in the GPU address ranges"))
    }

    /// Makes a range free for the same use again. Nothing may be mapped there now.
    pub fn give_back(&mut self, base: u64, size: u64, align: u64, usage: AddressUse) {
        self.free.entry((usage, size, align)).or_default().push(base);
    }

    pub fn in_a_range(address: u64) -> bool {
        GPU_RANGES.iter().any(|range| range.contains(address))
    }
}

impl Default for GpuAddresses {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: AddressUse = AddressUse::Host;

    #[test]
    fn addresses_follow_one_another() {
        let mut addresses = GpuAddresses::new();
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(0x400_0000));
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(0x401_0000));
    }

    #[test]
    fn alignment_can_leave_a_gap() {
        let mut addresses = GpuAddresses::new();
        addresses.take(0x1000, ALLOCATION_ALIGN, HOST).unwrap();
        assert_eq!(addresses.take(0x1000, LARGE_ALIGN, HOST), Ok(0x420_0000));
    }

    #[test]
    fn what_does_not_fit_moves_to_the_next_range() {
        let mut addresses = GpuAddresses::new();
        assert_eq!(addresses.take(0x1_0000_0000, ALLOCATION_ALIGN, HOST), Ok(GPU_RANGES[1].start));
    }

    #[test]
    fn the_hole_is_not_in_a_range() {
        assert!(!GpuAddresses::in_a_range(0x1_0000_0000));
        assert!(GpuAddresses::in_a_range(0x1_4000_0000));
    }

    #[test]
    fn a_kept_range_is_jumped_over_and_still_mappable() {
        let kept = GpuRange { start: 0x800_0000, end: GPU_RANGES[0].end };
        let mut addresses = GpuAddresses::keeping_clear(&[kept]);
        assert_eq!(addresses.take(0x400_0000, ALLOCATION_ALIGN, HOST), Ok(0x400_0000));
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(GPU_RANGES[1].start));
        assert!(GpuAddresses::in_a_range(kept.start));
    }

    #[test]
    fn a_kept_range_in_the_middle_leaves_both_sides() {
        let kept = GpuRange { start: 0x500_0000, end: 0x600_0000 };
        let mut addresses = GpuAddresses::keeping_clear(&[kept]);
        assert_eq!(addresses.take(0x100_0000, ALLOCATION_ALIGN, HOST), Ok(0x400_0000));
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN, HOST), Ok(0x600_0000));
    }

    #[test]
    fn running_out_is_an_error() {
        assert!(GpuAddresses::new().take(1 << 50, ALLOCATION_ALIGN, HOST).is_err());
        assert!(GpuAddresses::new().take(u64::MAX, ALLOCATION_ALIGN, HOST).is_err());
    }

    #[test]
    fn a_freed_range_comes_back_for_the_same_use_only() {
        let mut addresses = GpuAddresses::new();
        let usage = AddressUse::Mapped;
        let first = addresses.take(0x10000, ALLOCATION_ALIGN, usage).unwrap();
        addresses.give_back(first, 0x10000, ALLOCATION_ALIGN, usage);
        assert_ne!(addresses.take(0x10000, ALLOCATION_ALIGN, AddressUse::Reserved), Ok(first));
        assert_ne!(addresses.take(0x20000, ALLOCATION_ALIGN, usage), Ok(first));
        assert_eq!(addresses.take(0x10000, ALLOCATION_ALIGN, usage), Ok(first));
    }

    #[test]
    fn the_last_range_given_back_goes_out_first() {
        let mut addresses = GpuAddresses::new();
        let usage = AddressUse::Reserved;
        addresses.give_back(0x1000, 1, 1, usage);
        addresses.give_back(0x2000, 1, 1, usage);
        assert_eq!(addresses.take(1, 1, usage), Ok(0x2000));
    }
}
