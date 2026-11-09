//! The card's one GPU address space, shared by every space the core makes.
//!
//! amdgpu gives each open render node a single VM, and the backend has one open node, so every
//! space the core asks for lands in that VM. The core maps some memory into every space at the same
//! address, its fences for one, and a guest process that comes back maps again where it was before.
//! amdgpu refuses a second mapping over addresses already mapped, so a mapping is made once here and
//! counted for each space that holds it. It is taken out of the VM when the last space lets go.

use crate::memory::GemHandle;
use guesthouse_backend::SpaceId;
use std::collections::BTreeMap;

/// One mapping in the VM, and the spaces holding it.
#[derive(Debug)]
struct Held {
    handle: GemHandle,
    offset: u64,
    length: u64,
    spaces: Vec<SpaceId>,
}

/// What a space asking to map can do.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// Nothing is there: map it, then call [`SharedVm::add`].
    Free,
    /// The same memory is already mapped there, and the space now holds it too.
    Shared,
    /// Something else holds some of those addresses.
    Taken,
}

/// A mapping the last space let go of, which must now leave the VM.
#[derive(Debug, PartialEq, Eq)]
pub struct Unmap {
    pub handle: GemHandle,
    pub address: u64,
    pub length: u64,
}

#[derive(Debug, Default)]
pub struct SharedVm {
    held: BTreeMap<u64, Held>,
}

impl SharedVm {
    /// Whether `space` may have `length` bytes of `handle`, from `offset`, at `address`.
    pub fn claim(
        &mut self,
        space: SpaceId,
        handle: GemHandle,
        address: u64,
        offset: u64,
        length: u64,
    ) -> Claim {
        if let Some(held) = self.held.get_mut(&address)
            && held.handle == handle
            && held.offset == offset
            && held.length == length
        {
            if !held.spaces.contains(&space) {
                held.spaces.push(space);
            }
            return Claim::Shared;
        }
        let end = address.saturating_add(length);
        let before = self.held.range(..end).next_back();
        match before {
            Some((&start, held)) if start.saturating_add(held.length) > address => Claim::Taken,
            _ => Claim::Free,
        }
    }

    /// Notes a mapping just made for `space`.
    pub fn add(&mut self, space: SpaceId, handle: GemHandle, address: u64, offset: u64, length: u64) {
        self.held.insert(address, Held { handle, offset, length, spaces: vec![space] });
    }

    /// `space` lets go of the mapping at `address`. Returns it if no space holds it any more.
    pub fn release(&mut self, space: SpaceId, address: u64) -> Option<Unmap> {
        let held = self.held.get_mut(&address)?;
        held.spaces.retain(|&holder| holder != space);
        if !held.spaces.is_empty() {
            return None;
        }
        let held = self.held.remove(&address)?;
        Some(Unmap { handle: held.handle, address, length: held.length })
    }

    /// `space` lets go of everything it holds. Returns what no space holds any more.
    pub fn release_space(&mut self, space: SpaceId) -> Vec<Unmap> {
        let addresses: Vec<u64> = self
            .held
            .iter()
            .filter(|(_, held)| held.spaces.contains(&space))
            .map(|(&address, _)| address)
            .collect();
        addresses.into_iter().filter_map(|address| self.release(space, address)).collect()
    }

    /// Every space lets go of what `handle` has mapped, as when the memory is freed.
    pub fn release_memory(&mut self, handle: GemHandle) -> Vec<Unmap> {
        let addresses: Vec<u64> =
            self.held.iter().filter(|(_, held)| held.handle == handle).map(|(&address, _)| address).collect();
        addresses
            .into_iter()
            .filter_map(|address| {
                let held = self.held.remove(&address)?;
                Some(Unmap { handle: held.handle, address, length: held.length })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FENCES: GemHandle = GemHandle(7);
    const OTHER: GemHandle = GemHandle(8);
    const FIRST: SpaceId = SpaceId(1);
    const SECOND: SpaceId = SpaceId(2);
    const AT: u64 = 0x40_0000;
    const SIZE: u64 = 0x1_0000;

    fn with_fences() -> SharedVm {
        let mut vm = SharedVm::default();
        assert_eq!(vm.claim(FIRST, FENCES, AT, 0, SIZE), Claim::Free);
        vm.add(FIRST, FENCES, AT, 0, SIZE);
        vm
    }

    #[test]
    fn the_same_memory_at_the_same_place_is_shared_by_a_second_space() {
        let mut vm = with_fences();
        assert_eq!(vm.claim(SECOND, FENCES, AT, 0, SIZE), Claim::Shared);
    }

    #[test]
    fn other_memory_over_the_same_addresses_is_refused() {
        let mut vm = with_fences();
        assert_eq!(vm.claim(SECOND, OTHER, AT, 0, SIZE), Claim::Taken);
        assert_eq!(vm.claim(SECOND, OTHER, AT + SIZE / 2, 0, SIZE), Claim::Taken);
        assert_eq!(vm.claim(SECOND, FENCES, AT, 0, SIZE / 2), Claim::Taken);
        assert_eq!(vm.claim(SECOND, OTHER, AT + SIZE, 0, SIZE), Claim::Free);
    }

    #[test]
    fn a_mapping_leaves_the_vm_only_when_the_last_space_lets_go() {
        let mut vm = with_fences();
        vm.claim(SECOND, FENCES, AT, 0, SIZE);
        assert_eq!(vm.release(FIRST, AT), None);
        assert_eq!(vm.release(SECOND, AT), Some(Unmap { handle: FENCES, address: AT, length: SIZE }));
        assert_eq!(vm.claim(FIRST, OTHER, AT, 0, SIZE), Claim::Free);
    }

    #[test]
    fn a_closed_space_lets_go_of_all_it_held() {
        let mut vm = with_fences();
        vm.claim(SECOND, FENCES, AT, 0, SIZE);
        vm.add(SECOND, OTHER, AT + SIZE, 0, SIZE);
        let unmapped = vm.release_space(SECOND);
        assert_eq!(unmapped, vec![Unmap { handle: OTHER, address: AT + SIZE, length: SIZE }]);
        assert_eq!(vm.release_space(FIRST), vec![Unmap { handle: FENCES, address: AT, length: SIZE }]);
    }

    #[test]
    fn freed_memory_leaves_the_vm_whoever_held_it() {
        let mut vm = with_fences();
        vm.claim(SECOND, FENCES, AT, 0, SIZE);
        assert_eq!(vm.release_memory(FENCES), vec![Unmap { handle: FENCES, address: AT, length: SIZE }]);
        assert_eq!(vm.release(FIRST, AT), None);
    }
}
