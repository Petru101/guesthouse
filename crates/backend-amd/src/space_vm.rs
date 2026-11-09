//! A GPU address space of its own for each space the core makes.
//!
//! amdgpu gives each open render node one VM. Guest processes place their own reservations, and two
//! processes often place them at the same addresses, so each space opens the node again and gets a
//! VM of its own. Memory is made on the backend's first open and taken into a space's open with a
//! dma-buf before it is mapped there. A buffer taken in is kept while any of its mappings in the space
//! are, and let go after the last.

use crate::device::Device;
use crate::memory::{self, GemHandle, MapFlags};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io;

/// Which buffers a space has taken in, and its mappings of them. Kept apart from the files so the
/// counting can be tested without a card.
#[derive(Debug, Default)]
pub struct Books {
    /// By the buffer's handle on the first open: its handle here, and how many mappings use it.
    taken_in: HashMap<GemHandle, (GemHandle, u32)>,
    /// By address: the buffer's handle on the first open, and the mapping's length.
    mappings: BTreeMap<u64, (GemHandle, u64)>,
}

/// A mapping on its way out, and whether the buffer's handle here must be let go after it.
#[derive(Debug, PartialEq, Eq)]
pub struct Unmapping {
    pub handle: GemHandle,
    pub address: u64,
    pub length: u64,
    pub last_use: bool,
}

impl Books {
    /// The handle here for `buffer`, if it was taken in.
    pub fn handle_of(&self, buffer: GemHandle) -> Option<GemHandle> {
        self.taken_in.get(&buffer).map(|&(handle, _)| handle)
    }

    pub fn add(&mut self, buffer: GemHandle, handle: GemHandle, address: u64, length: u64) {
        self.taken_in.entry(buffer).or_insert((handle, 0)).1 += 1;
        self.mappings.insert(address, (buffer, length));
    }

    pub fn remove(&mut self, address: u64) -> Option<Unmapping> {
        let (buffer, length) = self.mappings.remove(&address)?;
        let (handle, uses) = self.taken_in.get_mut(&buffer)?;
        let handle = *handle;
        *uses -= 1;
        let last_use = *uses == 0;
        if last_use {
            self.taken_in.remove(&buffer);
        }
        Some(Unmapping { handle, address, length, last_use })
    }

    /// Every mapping of `buffer` here.
    pub fn addresses_of(&self, buffer: GemHandle) -> Vec<u64> {
        let of_buffer = self.mappings.iter().filter(|(_, (mapped, _))| *mapped == buffer);
        of_buffer.map(|(&address, _)| address).collect()
    }
}

/// One space's open of the render node, and what it holds.
#[derive(Debug)]
pub struct SpaceVm {
    file: File,
    books: Books,
}

impl SpaceVm {
    pub fn open(device: &Device) -> io::Result<Self> {
        Ok(Self { file: device.open_again()?, books: Books::default() })
    }

    /// Maps `length` bytes of `buffer`, a handle on `device`, from `offset`, at `address`.
    pub fn map(
        &mut self,
        device: &Device,
        buffer: GemHandle,
        address: u64,
        offset: u64,
        length: u64,
        flags: MapFlags,
    ) -> io::Result<()> {
        let fresh = self.books.handle_of(buffer).is_none();
        let handle = match self.books.handle_of(buffer) {
            Some(handle) => handle,
            None => memory::import(&self.file, &memory::export(device, buffer)?)?,
        };
        if let Err(error) = memory::map(&self.file, handle, address, offset, length, flags) {
            if fresh {
                memory::close(&self.file, handle);
            }
            return Err(error);
        }
        self.books.add(buffer, handle, address, length);
        Ok(())
    }

    /// Takes back the mapping at `address`, if there is one.
    pub fn unmap(&mut self, address: u64) -> io::Result<()> {
        let Some(unmapping) = self.books.remove(address) else {
            return Ok(());
        };
        let result = memory::unmap(&self.file, unmapping.handle, unmapping.address, unmapping.length);
        if unmapping.last_use {
            memory::close(&self.file, unmapping.handle);
        }
        result
    }

    /// Takes back every mapping of `buffer`, as before it is freed. Returns where they were.
    pub fn forget(&mut self, buffer: GemHandle) -> Vec<u64> {
        let addresses = self.books.addresses_of(buffer);
        for &address in &addresses {
            if let Err(error) = self.unmap(address) {
                log::warn!("GEM_VA unmap at {address:#x}: {error}");
            }
        }
        addresses
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUFFER: GemHandle = GemHandle(7);
    const HERE: GemHandle = GemHandle(3);
    const AT: u64 = 0x40_0000;
    const SIZE: u64 = 0x1_0000;

    #[test]
    fn a_buffer_mapped_twice_is_let_go_after_its_second_mapping() {
        let mut books = Books::default();
        books.add(BUFFER, HERE, AT, SIZE);
        books.add(BUFFER, HERE, AT + SIZE, SIZE);
        assert_eq!(books.handle_of(BUFFER), Some(HERE));
        let first = books.remove(AT).unwrap();
        assert_eq!(first, Unmapping { handle: HERE, address: AT, length: SIZE, last_use: false });
        assert!(books.remove(AT + SIZE).unwrap().last_use);
        assert_eq!(books.handle_of(BUFFER), None);
    }

    #[test]
    fn an_address_with_nothing_mapped_has_nothing_to_take_back() {
        let mut books = Books::default();
        assert_eq!(books.remove(AT), None);
    }

    #[test]
    fn a_buffers_mappings_are_found_by_the_buffer() {
        let mut books = Books::default();
        books.add(BUFFER, HERE, AT, SIZE);
        books.add(GemHandle(8), GemHandle(4), AT + SIZE, SIZE);
        assert_eq!(books.addresses_of(BUFFER), vec![AT]);
    }
}
