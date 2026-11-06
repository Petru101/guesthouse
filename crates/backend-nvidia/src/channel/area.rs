//! Host memory the GPU reads: a channel's control block and its small buffers.

use std::io;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

/// Page-aligned memory, zeroed, for the life of the channel.
#[derive(Debug)]
pub struct Area {
    start: *mut u8,
    size: usize,
}

impl Area {
    pub fn new(size: usize) -> io::Result<Self> {
        let protection = libc::PROT_READ | libc::PROT_WRITE;
        let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;
        // SAFETY: a fresh anonymous mapping at an address the kernel picks disturbs nothing.
        let start = unsafe { libc::mmap(ptr::null_mut(), size, protection, flags, -1, 0) };
        if start == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { start: start.cast(), size })
    }

    pub fn host_address(&self) -> u64 {
        self.start as u64
    }

    pub fn size(&self) -> usize {
        self.size
    }

    fn word(&self, at: usize) -> &AtomicU32 {
        assert!(at + 4 <= self.size && at.is_multiple_of(4), "{at:#x} is not a word in this area");
        // SAFETY: the assert keeps it inside the mapping and aligned, and the mapping outlives &self.
        unsafe { AtomicU32::from_ptr(self.start.add(at).cast()) }
    }

    /// One store, as the GPU can read a field at any moment.
    pub fn store(&self, at: usize, value: u32) {
        self.word(at).store(value, Ordering::Release);
    }

    pub fn load(&self, at: usize) -> u32 {
        self.word(at).load(Ordering::Acquire)
    }

    pub fn store_u64(&self, at: usize, value: u64) {
        self.store(at, value as u32);
        self.store(at + 4, (value >> 32) as u32);
    }

    pub fn store_words(&self, at: usize, words: &[u32]) {
        for (index, &word) in words.iter().enumerate() {
            self.store(at + index * 4, word);
        }
    }

    /// Reads some words, for tests that play the GPU.
    pub fn load_words(&self, at: usize, count: usize) -> Vec<u32> {
        (0..count).map(|index| self.load(at + index * 4)).collect()
    }
}

impl Drop for Area {
    fn drop(&mut self) {
        // SAFETY: this is the mapping made in `new`, and no reference outlives self.
        unsafe { libc::munmap(self.start.cast(), self.size) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_area_is_zeroed_and_keeps_what_is_stored() {
        let area = Area::new(0x2000).unwrap();
        assert_eq!(area.load(0x1000), 0);
        area.store_u64(0x1000, 0x1122_3344_5566_7788);
        assert_eq!(area.load_words(0x1000, 2), [0x5566_7788, 0x1122_3344]);
    }
}
