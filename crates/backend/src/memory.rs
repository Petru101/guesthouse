//! How the host reaches memory it shares with the guest.

use std::io;

/// Guest pages mapped side by side in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagesView {
    pub host_address: u64,
    pub size: u64,
}

/// The shared window and the guest's RAM. Addresses are guest physical.
///
/// Fences live in the window, where guest and GPU read them while the host writes, so the 64-bit
/// accesses must each be a single one.
pub trait GuestMemory {
    /// The first guest physical address past the window.
    fn window_end(&self) -> u64;

    fn read_u64(&self, address: u64) -> u64;

    fn write_u64(&mut self, address: u64, value: u64);

    fn zero(&mut self, address: u64, length: u64);

    /// Where a window address is in this process.
    fn host_address(&self, address: u64) -> u64;

    /// Maps scattered guest pages, by page number, at consecutive addresses here.
    fn map_pages(&mut self, pages: &[u64]) -> io::Result<PagesView>;

    fn unmap_pages(&mut self, view: PagesView);
}
