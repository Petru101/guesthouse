//! A guest memory that stores only what was written to it.

use guesthouse_backend::{GuestMemory, MemoryError, PagesView, WINDOW_BASE, in_window};
use std::collections::BTreeMap;
use std::io;
use std::ops::Range;

/// Stands in for a window of many gigabytes without taking the memory.
#[derive(Debug)]
pub struct SparseMemory {
    words: BTreeMap<u64, u64>,
    window_end: u64,
    next_view: u64,
    /// Views still mapped, to catch one being lost or unmapped twice.
    pub views: Vec<PagesView>,
}

const FIRST_VIEW: u64 = 0x7000_0000_0000;
const PAGE_SIZE: u64 = 4096;
const WORD: u64 = 8;
const HALF_WORD: u64 = 4;

impl SparseMemory {
    pub fn new(window_end: u64) -> Self {
        Self { words: BTreeMap::new(), window_end, next_view: FIRST_VIEW, views: Vec::new() }
    }
}

impl SparseMemory {
    /// Refuses what the real memory refuses, so that a mistake in the core shows up here too.
    fn check(&self, address: u64, length: u64, align: u64) -> Result<(), MemoryError> {
        let in_view =
            |view: &PagesView| (view.host_address..view.host_address + view.size).contains(&address);
        let inside =
            in_window(&(WINDOW_BASE..self.window_end), address, length) || self.views.iter().any(in_view);
        if inside && address.is_multiple_of(align) { Ok(()) } else { Err(MemoryError::Outside(address)) }
    }

    fn word(&self, address: u64) -> u64 {
        self.words.get(&address).copied().unwrap_or(0)
    }
}

impl GuestMemory for SparseMemory {
    fn window(&self) -> Result<Range<u64>, MemoryError> {
        Ok(WINDOW_BASE..self.window_end)
    }

    fn read_u64(&self, address: u64) -> Result<u64, MemoryError> {
        self.check(address, WORD, WORD)?;
        Ok(self.word(address))
    }

    fn write_u64(&mut self, address: u64, value: u64) -> Result<(), MemoryError> {
        self.check(address, WORD, WORD)?;
        self.words.insert(address, value);
        Ok(())
    }

    fn write_u32(&mut self, address: u64, value: u32) -> Result<(), MemoryError> {
        self.check(address, HALF_WORD, HALF_WORD)?;
        let word = address & !(WORD - 1);
        let shift = (address & HALF_WORD) * 8;
        let old = self.word(word) & !(0xFFFF_FFFF << shift);
        self.words.insert(word, old | u64::from(value) << shift);
        Ok(())
    }

    fn read_bytes(&self, address: u64, out: &mut [u8]) -> usize {
        if self.check(address, 0, 1).is_err() {
            return 0;
        }
        // Views lie past the window, and only the window has an end to stop at.
        let room = self.window_end.checked_sub(address).unwrap_or(out.len() as u64);
        let length = out.len().min(room as usize);
        for (index, byte) in out[..length].iter_mut().enumerate() {
            let at = address + index as u64;
            *byte = self.word(at & !(WORD - 1)).to_le_bytes()[(at % WORD) as usize];
        }
        length
    }

    fn zero(&mut self, address: u64, length: u64) -> Result<(), MemoryError> {
        self.check(address, length, 1)?;
        let doomed: Vec<u64> = self.words.range(address..address + length).map(|(&at, _)| at).collect();
        doomed.iter().for_each(|at| {
            self.words.remove(at);
        });
        Ok(())
    }

    fn host_address(&self, address: u64, length: u64) -> Result<u64, MemoryError> {
        self.check(address, length, 1)?;
        Ok(address)
    }

    fn map_pages(&mut self, pages: &[u64]) -> io::Result<PagesView> {
        let view = PagesView { host_address: self.next_view, size: pages.len() as u64 * PAGE_SIZE };
        self.next_view += view.size.max(PAGE_SIZE);
        self.views.push(view);
        Ok(view)
    }

    fn unmap_pages(&mut self, view: PagesView) {
        let index = self.views.iter().position(|&held| held == view).expect("a view unmapped twice");
        self.views.swap_remove(index);
    }

    /// A view's bytes are kept at its made-up address, which is past any window.
    fn read_view(&self, view: PagesView, offset: u64, out: &mut [u8]) -> usize {
        let length = view.size.saturating_sub(offset).min(out.len() as u64) as usize;
        self.read_bytes(view.host_address + offset, &mut out[..length])
    }

    fn write_view_u32(&mut self, view: PagesView, offset: u64, value: u32) {
        if offset + 4 <= view.size {
            let _ = self.write_u32(view.host_address + offset, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_clears_only_its_range() {
        let mut memory = SparseMemory::new(WINDOW_BASE + 0x3000);
        memory.write_u64(WINDOW_BASE + 0x1000, 5).unwrap();
        memory.write_u64(WINDOW_BASE + 0x2000, 6).unwrap();
        memory.zero(WINDOW_BASE + 0x1000, 0x1000).unwrap();
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x1000), Ok(0));
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x2000), Ok(6));
    }

    #[test]
    fn host_addresses_stop_at_the_window() {
        let memory = SparseMemory::new(WINDOW_BASE + 0x1000);
        assert_eq!(memory.host_address(WINDOW_BASE, 0x1000), Ok(WINDOW_BASE));
        assert!(memory.host_address(WINDOW_BASE, 0x1001).is_err());
        assert!(memory.host_address(WINDOW_BASE - 1, 1).is_err());
    }

    #[test]
    fn accesses_outside_the_window_or_out_of_line_fail() {
        let mut memory = SparseMemory::new(WINDOW_BASE + 0x1000);
        assert_eq!(memory.read_u64(WINDOW_BASE - 8), Err(MemoryError::Outside(WINDOW_BASE - 8)));
        assert!(memory.write_u64(WINDOW_BASE + 0xFFC, 0).is_err());
        assert!(memory.write_u32(WINDOW_BASE + 2, 0).is_err());
        assert!(memory.zero(WINDOW_BASE, 0x1001).is_err());
        assert_eq!(memory.read_bytes(WINDOW_BASE - 8, &mut [0; 8]), 0);
        assert_eq!(memory.read_bytes(WINDOW_BASE + 0xFFC, &mut [0; 8]), 4);
    }
}
