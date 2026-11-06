//! Submissions that say only where the guest's ring has got to.
//!
//! Some of the guest's runtimes keep a ring of their own and send a new position with each
//! submission. There's no ring address in this form and no list, and the bytes where a list would
//! be are whatever was on the sender's stack. Taking those for ring entries would put rubbish on a
//! real channel, on the card that runs the host's desktop, so this form must never reach the list
//! path. A flag in the private data marks it.
//!
//! What such a submission means is in the guest's ring, whose place is in the queue's private data.
//! The new entries are read from there and go on a list channel like any list, which brings the
//! prelude, the fence's release and the copy engine's fixes with them. Only the entries the
//! submission counts are taken, so an old lap of the ring can't run again. The bytes are the guest's
//! commands in the guest's address space, as a list's are, and nothing else in them reaches the host.

use crate::pushbuffer::ENTRY_SIZE;
use guesthouse_backend::{HostServices, SpaceId};

const QUEUE_RING_ADDRESS: usize = 0x18;
const QUEUE_RING_SIZE: usize = 0x20;

const FORM_FLAGS: usize = 0x110;
const RING_POSITION_FORM: u8 = 0x10;

/// Whether this private data carries the flag. Hardware queues with a ring address carry it too.
pub(crate) fn has_ring_position_flag(private: &[u8]) -> bool {
    private.get(FORM_FLAGS).is_some_and(|flags| flags & RING_POSITION_FORM != 0)
}

/// A queue's ring, as its private data gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GuestRing {
    address: u64,
    /// Taken as entries. If it is really bytes, the mapping is the smaller of the two, or an entry
    /// that was never written shows up the mistake.
    size: u32,
}

impl GuestRing {
    /// The ring a queue's private data names, if it names one.
    pub(crate) fn of_queue(private: &[u8]) -> Option<Self> {
        let address = private.get(QUEUE_RING_ADDRESS..QUEUE_RING_ADDRESS + 8)?;
        let size = private.get(QUEUE_RING_SIZE..QUEUE_RING_SIZE + 4)?;
        let address = u64::from_le_bytes(address.try_into().expect("eight bytes"));
        let size = u32::from_le_bytes(size.try_into().expect("four bytes"));
        (address != 0 && size != 0).then_some(Self { address, size })
    }

    /// The `count` entries before ring position `end`, in order, or why they can't be trusted.
    pub(crate) fn entries_before(
        &self,
        host: &dyn HostServices,
        space: SpaceId,
        count: u32,
        end: u32,
    ) -> Result<Vec<u8>, &'static str> {
        let span =
            host.guest_span(space, self.address).ok_or("the ring isn't in memory the host can read")?;
        let room = (span.mapping_length - span.offset_in_mapping) / ENTRY_SIZE as u64;
        let ring_entries = room.min(u64::from(self.size));
        if ring_entries == 0 || u64::from(count) > ring_entries {
            return Err("more entries than the ring holds");
        }
        let first = u64::from(end) + ring_entries - u64::from(count);
        let mut listing = Vec::with_capacity(count as usize * ENTRY_SIZE);
        for index in 0..u64::from(count) {
            let at = (first + index) % ring_entries * ENTRY_SIZE as u64;
            let mut entry = [0; ENTRY_SIZE];
            let read = host.read_guest_span(space, self.address + at, &mut entry);
            if read != Some(ENTRY_SIZE) {
                return Err("a ring entry can't be read");
            }
            if entry == [0; ENTRY_SIZE] {
                return Err("a counted ring entry is empty");
            }
            listing.extend_from_slice(&entry);
        }
        Ok(listing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::{BackendError, GuestSpan, MemoryError, SharedSpan};

    const SPACE: SpaceId = SpaceId(1);
    const RING: u64 = 0x1000;

    /// A ring in plain bytes.
    struct Ring(Vec<u8>);

    impl HostServices for Ring {
        fn take_gpu_addresses(&mut self, _size: u64, _align: u64) -> Result<u64, BackendError> {
            unimplemented!()
        }
        fn read_at_gpu_address(&self, _: SpaceId, _: u64, _: &mut [u8]) -> Option<usize> {
            unimplemented!()
        }
        fn shared_span(&self, _: SpaceId, _: u64) -> Option<SharedSpan> {
            unimplemented!()
        }
        fn guest_span(&self, _: SpaceId, address: u64) -> Option<GuestSpan> {
            (address == RING)
                .then_some(GuestSpan { offset_in_mapping: 0, mapping_length: self.0.len() as u64 })
        }
        fn read_guest_span(&self, _: SpaceId, address: u64, out: &mut [u8]) -> Option<usize> {
            let at = (address - RING) as usize;
            let bytes = self.0.get(at..at + out.len())?;
            out.copy_from_slice(bytes);
            Some(out.len())
        }
        fn write_guest_span(&mut self, _: SpaceId, _: u64, _: u32) -> bool {
            unimplemented!()
        }
        fn write_u32(&mut self, _: u64, _: u32) -> Result<(), MemoryError> {
            unimplemented!()
        }
        fn write_u64(&mut self, _: u64, _: u64) -> Result<(), MemoryError> {
            unimplemented!()
        }
        fn read_guest(&self, _: u64, _: &mut [u8]) -> usize {
            unimplemented!()
        }
        fn fence_value(&self, _: u64) -> Result<u64, MemoryError> {
            unimplemented!()
        }
    }

    /// Each entry says which it is, counting from one.
    fn ring_of(entries: u8) -> Ring {
        Ring((1..=entries).flat_map(|entry| [entry; ENTRY_SIZE]).collect())
    }

    fn firsts(listing: &[u8]) -> Vec<u8> {
        listing.iter().step_by(ENTRY_SIZE).copied().collect()
    }

    fn read(ring: &Ring, size: u32, count: u32, end: u32) -> Result<Vec<u8>, &'static str> {
        GuestRing { address: RING, size }.entries_before(ring, SPACE, count, end)
    }

    #[test]
    fn the_ring_comes_from_the_queues_data() {
        let mut private = vec![0; 0x24];
        assert_eq!(GuestRing::of_queue(&private), None);
        private[QUEUE_RING_ADDRESS..QUEUE_RING_ADDRESS + 8].copy_from_slice(&RING.to_le_bytes());
        private[QUEUE_RING_SIZE] = 8;
        assert_eq!(GuestRing::of_queue(&private), Some(GuestRing { address: RING, size: 8 }));
        assert_eq!(GuestRing::of_queue(&private[..0x20]), None);
    }

    #[test]
    fn only_the_counted_entries_are_read() {
        assert_eq!(firsts(&read(&ring_of(8), 8, 2, 5).unwrap()), [4, 5]);
    }

    #[test]
    fn entries_wrap_at_the_end_of_the_ring() {
        assert_eq!(firsts(&read(&ring_of(8), 8, 3, 1).unwrap()), [7, 8, 1]);
    }

    #[test]
    fn a_position_past_one_lap_wraps_too() {
        assert_eq!(firsts(&read(&ring_of(8), 8, 2, 8 + 3).unwrap()), [2, 3]);
    }

    #[test]
    fn a_size_field_bigger_than_the_mapping_is_held_to_the_mapping() {
        assert_eq!(firsts(&read(&ring_of(4), 0x2000, 1, 5).unwrap()), [1]);
    }

    #[test]
    fn a_mapping_bigger_than_the_size_field_is_held_to_the_field() {
        assert_eq!(firsts(&read(&ring_of(8), 4, 1, 5).unwrap()), [1]);
    }

    #[test]
    fn more_entries_than_the_ring_holds_is_refused() {
        assert!(read(&ring_of(4), 4, 5, 5).is_err());
    }

    #[test]
    fn an_empty_entry_is_refused_and_not_passed_along() {
        let mut ring = ring_of(4);
        ring.0[ENTRY_SIZE..2 * ENTRY_SIZE].fill(0);
        assert!(read(&ring, 4, 2, 2).is_err());
    }

    #[test]
    fn a_ring_the_host_cannot_see_is_refused() {
        let found = GuestRing { address: RING + 8, size: 4 }.entries_before(&ring_of(4), SPACE, 1, 1);
        assert!(found.is_err());
    }

    #[test]
    fn data_too_short_to_hold_the_flag_does_not_have_it() {
        assert!(!has_ring_position_flag(&[0xFF; FORM_FLAGS]));
    }

    #[test]
    fn only_the_one_bit_counts() {
        let mut private = vec![!RING_POSITION_FORM; FORM_FLAGS + 1];
        assert!(!has_ring_position_flag(&private));
        private[FORM_FLAGS] = RING_POSITION_FORM;
        assert!(has_ring_position_flag(&private));
    }
}
