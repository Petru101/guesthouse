//! How the host reaches memory it shares with the guest.

use std::io;
use std::ops::Range;

/// Where QEMU places the shared window in guest physical memory.
pub const WINDOW_BASE: u64 = 1 << 40;

/// Guest pages mapped side by side in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagesView {
    pub host_address: u64,
    pub size: u64,
}

/// Whether all of `length` bytes at `address` are in `window`.
pub fn in_window(window: &Range<u64>, address: u64, length: u64) -> bool {
    address >= window.start && address.checked_add(length).is_some_and(|end| end <= window.end)
}

/// The shared window and the guest's RAM. Addresses are guest physical.
///
/// Fences live in the window, where guest and GPU read them while the host writes, so the 64-bit
/// accesses must each be a single one.
pub trait GuestMemory {
    /// The guest physical addresses the window covers.
    fn window(&self) -> Range<u64>;

    fn read_u64(&self, address: u64) -> u64;

    fn write_u64(&mut self, address: u64, value: u64);

    /// A 32-bit field, which the GPU may read while it changes, so this is a single store too.
    fn write_u32(&mut self, address: u64, value: u32);

    /// Copies bytes out of the window, as far as `out` goes or the window does. Returns how many.
    fn read_bytes(&self, address: u64, out: &mut [u8]) -> usize;

    fn zero(&mut self, address: u64, length: u64);

    /// Where `length` bytes at a window address are in this process, or `None` unless all of them
    /// are in the window.
    fn host_address(&self, address: u64, length: u64) -> Option<u64>;

    /// Maps scattered guest pages, by page number, at consecutive addresses here.
    fn map_pages(&mut self, pages: &[u64]) -> io::Result<PagesView>;

    fn unmap_pages(&mut self, view: PagesView);
}
