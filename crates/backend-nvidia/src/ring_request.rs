//! A request for a command ring, and the kernel side's answer to it.
//!
//! The guest's driver keeps a ring of entries for each of its queues. When the ring looks full for
//! too long it asks for a bigger one, with the same private data it uses for a ring at start, and
//! the kernel side answers with a handle, one word a node. The driver puts that handle in a method
//! at the end of the old ring, and the GPU moves the channel to the new ring when it gets there.
//! With the word left at zero the driver gives up on the move, and the device with it.

use crate::profile::AllocationLayout;
use crate::pushbuffer::ENTRY_SIZE;
use crate::rm::Handle;
use guesthouse_backend::MemoryId;
use std::collections::HashMap;

/// Every ring request has had this length.
const RING_REQUEST_LENGTH: u32 = 0x24A;
/// Two words every ring request has had, which nothing else has.
const KIND: usize = 0x20;
const KIND_RING: u32 = 0x2_0000;
const USAGE: usize = 0x8A;
const USAGE_RING: u32 = 0x7810_0000;
/// Where the handle goes, one word a node. This host has one node.
const MOVE_HANDLE: usize = 0x22A;
/// The smallest ring. The same request comes for a ring's semaphores, which are a page each.
const SMALLEST_RING: u32 = 0x1_0000;

fn word(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4).map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
}

/// Whether this private data asks for a ring, rather than for one of a ring's semaphores.
pub(crate) fn is_ring_request(layout: &AllocationLayout, data: &[u8]) -> bool {
    let size = word(data, layout.size);
    data.first_chunk::<4>() == Some(layout.magic)
        && word(data, layout.own_length) == RING_REQUEST_LENGTH
        && data.len() >= RING_REQUEST_LENGTH as usize
        && word(data, KIND) == KIND_RING
        && word(data, USAGE) == USAGE_RING
        && size >= SMALLEST_RING
        && size.is_power_of_two()
}

/// Puts the handle where the guest's driver reads it.
pub(crate) fn answer_with_handle(data: &mut [u8], handle: Handle) {
    if let Some(field) = data.get_mut(MOVE_HANDLE..MOVE_HANDLE + 4) {
        field.copy_from_slice(&handle.to_le_bytes());
    }
}

/// A ring a queue may move to, as its request was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RingMove {
    pub handle: Handle,
    pub entries: u32,
}

/// The rings asked for and not yet moved to, by the GPU address each got. A ring nobody moves to is
/// forgotten when the memory at its address is freed.
#[derive(Debug, Default)]
pub(crate) struct RingMoves {
    answered: HashMap<MemoryId, RingMove>,
    mapped: HashMap<u64, (MemoryId, RingMove)>,
}

impl RingMoves {
    /// A ring request was answered with `handle`, for memory of `size` bytes.
    pub fn answered(&mut self, memory: MemoryId, handle: Handle, size: u64) {
        let entries = u32::try_from(size / ENTRY_SIZE as u64).unwrap_or(u32::MAX);
        self.answered.insert(memory, RingMove { handle, entries });
    }

    /// Memory got a GPU address. A ring is found again by that address when a queue moves to it.
    /// Its first lock puts shared pages where the ring's memory was, so other memory mapped at a
    /// ring's address becomes the ring.
    pub fn mapped(&mut self, memory: MemoryId, address: u64) {
        if let Some(ring) = self.answered.remove(&memory) {
            self.mapped.insert(address, (memory, ring));
        } else if let Some((owner, _)) = self.mapped.get_mut(&address) {
            *owner = memory;
        }
    }

    /// Memory was freed, whether or not a queue moved to it.
    pub fn released(&mut self, memory: MemoryId) {
        self.answered.remove(&memory);
        self.mapped.retain(|_, (owner, _)| *owner != memory);
    }

    /// The ring at an address, once.
    pub fn take(&mut self, address: u64) -> Option<RingMove> {
        self.mapped.remove(&address).map(|(_, ring)| ring)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_guest::blank_request;
    use crate::profile::GUEST_617;

    const LAYOUT: &AllocationLayout = &GUEST_617.allocation;

    fn set_word(data: &mut [u8], at: usize, value: u32) {
        data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn ring_request() -> Vec<u8> {
        let mut data = blank_request();
        data.resize(RING_REQUEST_LENGTH as usize, 0);
        set_word(&mut data, LAYOUT.own_length, RING_REQUEST_LENGTH);
        set_word(&mut data, KIND, KIND_RING);
        set_word(&mut data, USAGE, USAGE_RING);
        set_word(&mut data, LAYOUT.size, SMALLEST_RING);
        data
    }

    #[test]
    fn a_ring_request_is_known_by_its_length_and_two_words() {
        assert!(is_ring_request(LAYOUT, &ring_request()));
        assert!(!is_ring_request(LAYOUT, &blank_request()));
        let mut other_usage = ring_request();
        set_word(&mut other_usage, USAGE, 0);
        assert!(!is_ring_request(LAYOUT, &other_usage));
    }

    #[test]
    fn a_semaphore_from_the_same_builder_is_not_a_ring() {
        let mut semaphore = ring_request();
        set_word(&mut semaphore, LAYOUT.size, 0x1000);
        assert!(!is_ring_request(LAYOUT, &semaphore));
    }

    #[test]
    fn the_handle_goes_in_the_first_node_word() {
        let mut data = ring_request();
        answer_with_handle(&mut data, 0xC1D0_0042);
        assert_eq!(word(&data, MOVE_HANDLE), 0xC1D0_0042);
        assert_eq!(word(&data, MOVE_HANDLE + 4), 0);
    }

    #[test]
    fn a_ring_is_found_by_its_address_once_it_has_one() {
        let mut moves = RingMoves::default();
        moves.answered(MemoryId(7), 0xC1D0_0001, 0x8_0000);
        assert_eq!(moves.take(0x4000), None);
        moves.mapped(MemoryId(8), 0x4000);
        moves.mapped(MemoryId(7), 0x4000);
        assert_eq!(moves.take(0x4000), Some(RingMove { handle: 0xC1D0_0001, entries: 0x1_0000 }));
        assert_eq!(moves.take(0x4000), None);
    }

    #[test]
    fn a_freed_ring_is_forgotten_whether_or_not_it_was_mapped() {
        let mut moves = RingMoves::default();
        moves.answered(MemoryId(7), 0xC1D0_0001, 0x8_0000);
        moves.answered(MemoryId(8), 0xC1D0_0002, 0x8_0000);
        moves.mapped(MemoryId(8), 0x4000);
        moves.released(MemoryId(7));
        moves.released(MemoryId(8));
        moves.mapped(MemoryId(7), 0x8000);
        assert_eq!(moves.take(0x4000), None);
        assert_eq!(moves.take(0x8000), None);
    }

    #[test]
    fn a_ring_stays_when_its_first_lock_puts_shared_pages_in_its_place() {
        let mut moves = RingMoves::default();
        moves.answered(MemoryId(7), 0xC1D0_0001, 0x8_0000);
        moves.mapped(MemoryId(7), 0x4000);
        // The lock unmaps the ring's own memory, maps shared pages there, then frees the memory.
        moves.mapped(MemoryId(9), 0x4000);
        moves.released(MemoryId(7));
        assert!(moves.take(0x4000).is_some());
    }

    #[test]
    fn a_ring_in_shared_pages_is_forgotten_when_they_are_freed() {
        let mut moves = RingMoves::default();
        moves.answered(MemoryId(7), 0xC1D0_0001, 0x8_0000);
        moves.mapped(MemoryId(7), 0x4000);
        moves.mapped(MemoryId(9), 0x4000);
        moves.released(MemoryId(7));
        moves.released(MemoryId(9));
        assert_eq!(moves.take(0x4000), None);
    }

    #[test]
    fn short_data_is_left_alone() {
        let mut data = vec![0; 8];
        answer_with_handle(&mut data, 1);
        assert_eq!(data, vec![0; 8]);
    }
}
