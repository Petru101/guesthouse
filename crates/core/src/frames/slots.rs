//! Who has each slot of a frame ring.
//!
//! Only this decides. A slot goes to the viewer when its frame has really left, and comes back
//! only by the viewer's release of a slot it has. So the GPU never writes a slot a viewer may be
//! reading, and nothing a viewer says can make that happen.

/// What a slot is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// Holds nothing anyone wants.
    Free,
    /// A copy is writing it.
    Writing,
    /// Holds a finished frame that hasn't left yet.
    Pending { sequence: u64 },
    /// The viewer has it.
    Viewer { sequence: u64 },
}

/// A release that doesn't match what the slot is doing. A viewer's mistake, which changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotTheViewers;

/// See the module documentation.
#[derive(Debug)]
pub struct Slots {
    slots: Vec<Slot>,
    /// Where the last claim was, so that slots take turns.
    turn: usize,
}

impl Slots {
    pub fn new(count: u32) -> Self {
        Self { slots: vec![Slot::Free; count as usize], turn: 0 }
    }

    /// Takes the next free slot for a copy to write. `None` means this frame has to be skipped.
    pub fn claim(&mut self) -> Option<u32> {
        let count = self.slots.len();
        let mut next = (1..=count).map(|step| (self.turn + step) % count);
        let found = next.find(|&index| self.slots[index] == Slot::Free)?;
        self.turn = found;
        self.slots[found] = Slot::Writing;
        Some(found as u32)
    }

    /// The copy into a slot has finished on the GPU. A frame still waiting to leave is older, so
    /// it's replaced, and its slot is free. Says whether that happened.
    pub fn finished(&mut self, slot: u32, sequence: u64) -> bool {
        let older = self.pending();
        if let Some((older, _)) = older {
            self.slots[older as usize] = Slot::Free;
        }
        self.slots[slot as usize] = Slot::Pending { sequence };
        older.is_some()
    }

    /// The copy failed, so the slot holds nothing.
    pub fn abandoned(&mut self, slot: u32) {
        self.slots[slot as usize] = Slot::Free;
    }

    /// The frame waiting to leave, as (slot, sequence). There is one at most.
    pub fn pending(&self) -> Option<(u32, u64)> {
        self.slots.iter().enumerate().find_map(|(index, slot)| match slot {
            Slot::Pending { sequence } => Some((index as u32, *sequence)),
            _ => None,
        })
    }

    /// The frame in this slot has left, so the slot is the viewer's.
    pub fn sent(&mut self, slot: u32) {
        let state = &mut self.slots[slot as usize];
        if let Slot::Pending { sequence } = *state {
            *state = Slot::Viewer { sequence };
        }
    }

    /// The viewer has gone, and the slots outlive it. Only a producer that keeps its memory needs this.
    /// The newest picture goes back to waiting, as the next viewer may have nothing else to show: a
    /// still desktop sends no more frames.
    pub fn viewer_gone(&mut self) {
        let shown = |slot: &Slot| match slot {
            Slot::Viewer { sequence } => Some(*sequence),
            _ => None,
        };
        let newest = self.slots.iter().filter_map(shown).max();
        let waiting = self.pending().is_some();
        for slot in &mut self.slots {
            match shown(slot) {
                Some(sequence) if !waiting && Some(sequence) == newest => *slot = Slot::Pending { sequence },
                Some(_) => *slot = Slot::Free,
                None => {}
            }
        }
    }

    /// The viewer gives a slot back.
    pub fn released(&mut self, slot: u32) -> Result<(), NotTheViewers> {
        match self.slots.get_mut(slot as usize) {
            Some(state @ Slot::Viewer { .. }) => {
                *state = Slot::Free;
                Ok(())
            }
            _ => Err(NotTheViewers),
        }
    }

    /// Whether a copy is writing a slot, which means the memory can't go yet.
    pub fn is_writing(&self) -> bool {
        self.slots.contains(&Slot::Writing)
    }

    pub fn has_free(&self) -> bool {
        self.slots.contains(&Slot::Free)
    }

    pub fn count(&self) -> u32 {
        self.slots.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot taken all the way to the viewer.
    fn given_to_viewer(slots: &mut Slots, sequence: u64) -> u32 {
        let slot = slots.claim().unwrap();
        slots.finished(slot, sequence);
        slots.sent(slot);
        slot
    }

    #[test]
    fn the_newest_picture_outlives_its_viewer() {
        let mut slots = Slots::new(3);
        let older = given_to_viewer(&mut slots, 1);
        let newer = given_to_viewer(&mut slots, 2);
        slots.viewer_gone();
        assert_eq!(slots.pending(), Some((newer, 2)));
        assert_eq!(slots.released(older), Err(NotTheViewers));
        assert!(slots.has_free());
    }

    #[test]
    fn a_frame_still_waiting_beats_one_a_viewer_saw() {
        let mut slots = Slots::new(3);
        given_to_viewer(&mut slots, 1);
        let waiting = slots.claim().unwrap();
        slots.finished(waiting, 2);
        slots.viewer_gone();
        assert_eq!(slots.pending(), Some((waiting, 2)));
    }

    #[test]
    fn slots_take_turns() {
        let mut slots = Slots::new(3);
        let first = given_to_viewer(&mut slots, 1);
        slots.released(first).unwrap();
        assert_ne!(slots.claim(), Some(first), "the newest picture stays readable a little longer");
    }

    #[test]
    fn with_every_slot_taken_there_is_nothing_to_claim() {
        let mut slots = Slots::new(2);
        given_to_viewer(&mut slots, 1);
        let second = given_to_viewer(&mut slots, 2);
        assert_eq!(slots.claim(), None);
        assert!(!slots.has_free());
        slots.released(second).unwrap();
        assert_eq!(slots.claim(), Some(second));
    }

    #[test]
    fn a_slot_being_written_or_waiting_is_never_claimed() {
        let mut slots = Slots::new(2);
        let writing = slots.claim().unwrap();
        let waiting = slots.claim().unwrap();
        slots.finished(waiting, 1);
        assert_eq!(slots.claim(), None);
        assert_ne!(writing, waiting);
    }

    #[test]
    fn a_newer_frame_replaces_one_that_never_left() {
        let mut slots = Slots::new(3);
        let older = slots.claim().unwrap();
        assert!(!slots.finished(older, 1));
        let newer = slots.claim().unwrap();
        assert!(slots.finished(newer, 2));
        assert_eq!(slots.pending(), Some((newer, 2)));
        assert_eq!(slots.released(older), Err(NotTheViewers), "it never reached the viewer");
        assert!(slots.has_free());
        slots.claim().unwrap();
        assert_eq!(slots.claim(), Some(older), "it is free again");
    }

    #[test]
    fn a_frame_that_left_is_not_replaced() {
        let mut slots = Slots::new(3);
        let shown = given_to_viewer(&mut slots, 1);
        let next = slots.claim().unwrap();
        assert!(!slots.finished(next, 2));
        slots.released(shown).unwrap();
    }

    #[test]
    fn only_a_slot_the_viewer_has_can_be_released() {
        let mut slots = Slots::new(2);
        assert_eq!(slots.released(0), Err(NotTheViewers), "free");
        assert_eq!(slots.released(9), Err(NotTheViewers), "no such slot");
        let writing = slots.claim().unwrap();
        assert_eq!(slots.released(writing), Err(NotTheViewers), "the GPU is writing it");
        slots.finished(writing, 1);
        assert_eq!(slots.released(writing), Err(NotTheViewers), "it hasn't been sent");
        slots.sent(writing);
        assert_eq!(slots.released(writing), Ok(()));
        assert_eq!(slots.released(writing), Err(NotTheViewers), "released twice");
    }

    #[test]
    fn a_failed_copy_frees_its_slot() {
        let mut slots = Slots::new(1);
        let slot = slots.claim().unwrap();
        assert!(slots.is_writing());
        slots.abandoned(slot);
        assert!(!slots.is_writing());
        assert_eq!(slots.claim(), Some(slot));
    }
}
