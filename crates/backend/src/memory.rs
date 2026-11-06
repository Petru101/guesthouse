//! How the host reaches memory it shares with the guest.

use crate::BackendError;
use std::fmt;
use std::io;
use std::ops::Range;

/// Where QEMU places the shared window in guest physical memory.
pub const WINDOW_BASE: u64 = 1 << 40;

/// Why shared memory couldn't be reached. Nothing panics over these, as the address may have come
/// from a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// This address isn't in the window, or isn't aligned for the access.
    Outside(u64),
    /// The window's file can't be opened, which the log says more about.
    Unavailable,
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Outside(address) => write!(f, "{address:#x} is not usable in the shared window"),
            Self::Unavailable => f.write_str("the shared window is not available"),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<MemoryError> for BackendError {
    fn from(error: MemoryError) -> Self {
        Self::new(error.to_string())
    }
}

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
    fn window(&self) -> Result<Range<u64>, MemoryError>;

    fn read_u64(&self, address: u64) -> Result<u64, MemoryError>;

    fn write_u64(&mut self, address: u64, value: u64) -> Result<(), MemoryError>;

    /// A 32-bit field, which the GPU may read while it changes, so this is a single store too.
    fn write_u32(&mut self, address: u64, value: u32) -> Result<(), MemoryError>;

    /// Copies bytes out of the window, as far as `out` goes or the window does. Returns how many,
    /// which is none for an address outside it.
    fn read_bytes(&self, address: u64, out: &mut [u8]) -> usize;

    fn zero(&mut self, address: u64, length: u64) -> Result<(), MemoryError>;

    /// Where `length` bytes at a window address are in this process. All of them must be in the
    /// window.
    fn host_address(&self, address: u64, length: u64) -> Result<u64, MemoryError>;

    /// Maps scattered guest pages, by page number, at consecutive addresses here.
    fn map_pages(&mut self, pages: &[u64]) -> io::Result<PagesView>;

    fn unmap_pages(&mut self, view: PagesView);
}
