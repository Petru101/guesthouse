//! A guest memory that stores only what was written to it.

use guesthouse_backend::{GuestMemory, PagesView, WINDOW_BASE, in_window};
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

impl SparseMemory {
    pub fn new(window_end: u64) -> Self {
        Self { words: BTreeMap::new(), window_end, next_view: FIRST_VIEW, views: Vec::new() }
    }
}

impl GuestMemory for SparseMemory {
    fn window(&self) -> Range<u64> {
        WINDOW_BASE..self.window_end
    }

    fn read_u64(&self, address: u64) -> u64 {
        self.words.get(&address).copied().unwrap_or(0)
    }

    fn write_u64(&mut self, address: u64, value: u64) {
        self.words.insert(address, value);
    }

    fn write_u32(&mut self, address: u64, value: u32) {
        let word = address & !7;
        let shift = (address & 4) * 8;
        let old = self.read_u64(word) & !(0xFFFF_FFFF << shift);
        self.write_u64(word, old | u64::from(value) << shift);
    }

    fn read_bytes(&self, address: u64, out: &mut [u8]) -> usize {
        for (index, byte) in out.iter_mut().enumerate() {
            let at = address + index as u64;
            *byte = self.read_u64(at & !7).to_le_bytes()[(at & 7) as usize];
        }
        out.len()
    }

    fn zero(&mut self, address: u64, length: u64) {
        let doomed: Vec<u64> = self.words.range(address..address + length).map(|(&at, _)| at).collect();
        doomed.iter().for_each(|at| {
            self.words.remove(at);
        });
    }

    fn host_address(&self, address: u64, length: u64) -> Option<u64> {
        in_window(&self.window(), address, length).then_some(address)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_clears_only_its_range() {
        let mut memory = SparseMemory::new(WINDOW_BASE + 0x3000);
        memory.write_u64(WINDOW_BASE + 0x1000, 5);
        memory.write_u64(WINDOW_BASE + 0x2000, 6);
        memory.zero(WINDOW_BASE + 0x1000, 0x1000);
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x1000), 0);
        assert_eq!(memory.read_u64(WINDOW_BASE + 0x2000), 6);
    }

    #[test]
    fn host_addresses_stop_at_the_window() {
        let memory = SparseMemory::new(WINDOW_BASE + 0x1000);
        assert_eq!(memory.host_address(WINDOW_BASE, 0x1000), Some(WINDOW_BASE));
        assert_eq!(memory.host_address(WINDOW_BASE, 0x1001), None);
        assert_eq!(memory.host_address(WINDOW_BASE - 1, 1), None);
    }
}
