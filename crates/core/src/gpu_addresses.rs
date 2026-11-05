//! Handing out GPU addresses.

use guesthouse_backend::{BackendError, GpuRange};
use guesthouse_messages::wire::align_up;

/// The ranges every address space covers. There is a hole around 4 GiB, where real drivers keep
/// something of their own, and the ranges are big because one process can reserve gigabytes.
pub const GPU_RANGES: [GpuRange; 2] = [
    GpuRange { start: 0x400_0000, end: 0x1_0000_0000 },
    GpuRange { start: 0x1_4000_0000, end: 0x80_0000_0000 },
];

/// The alignment of ordinary mappings.
pub const ALLOCATION_ALIGN: u64 = 64 << 10;

/// The alignment of mappings big enough for 2 MiB pages, which must start on a 2 MiB boundary.
pub const LARGE_ALIGN: u64 = 2 << 20;

/// A bump allocator. Addresses are never reused and stay unique across every address space, so an
/// address always names one allocation.
#[derive(Debug)]
pub struct GpuAddresses {
    next: u64,
}

impl GpuAddresses {
    pub fn new() -> Self {
        Self { next: GPU_RANGES[0].start }
    }

    /// Takes `size` bytes, moving to the next range if this one is too full.
    pub fn take(&mut self, size: u64, align: u64) -> Result<u64, BackendError> {
        for range in GPU_RANGES {
            let base = align_up(self.next.max(range.start), align);
            if base + size <= range.end {
                self.next = base + size;
                return Ok(base);
            }
        }
        Err(BackendError::new("no room left in the GPU address ranges"))
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

    #[test]
    fn addresses_follow_one_another() {
        let mut addresses = GpuAddresses::new();
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN), Ok(0x400_0000));
        assert_eq!(addresses.take(0x1000, ALLOCATION_ALIGN), Ok(0x401_0000));
    }

    #[test]
    fn alignment_can_leave_a_gap() {
        let mut addresses = GpuAddresses::new();
        addresses.take(0x1000, ALLOCATION_ALIGN).unwrap();
        assert_eq!(addresses.take(0x1000, LARGE_ALIGN), Ok(0x420_0000));
    }

    #[test]
    fn what_does_not_fit_moves_to_the_next_range() {
        let mut addresses = GpuAddresses::new();
        assert_eq!(addresses.take(0x1_0000_0000, ALLOCATION_ALIGN), Ok(GPU_RANGES[1].start));
    }

    #[test]
    fn the_hole_is_not_in_a_range() {
        assert!(!GpuAddresses::in_a_range(0x1_0000_0000));
        assert!(GpuAddresses::in_a_range(0x1_4000_0000));
    }

    #[test]
    fn running_out_is_an_error() {
        assert!(GpuAddresses::new().take(1 << 50, ALLOCATION_ALIGN).is_err());
    }
}
