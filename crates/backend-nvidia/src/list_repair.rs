//! Lists longer than the ring the guest's driver builds them in.
//!
//! A guest's OpenGL submission can count more entries than the 2,048 a list holds. The list that
//! reaches us then has only the newest of them, and the count says how many went before. Those first
//! entries are often the ones that set up everything after them: a test that cleared a target and
//! drew 2,100 times lost the clear and 53 draws. Run as it stands, the rest can fault the GPU.
//!
//! So this takes everything from the end of the last list to the oldest entry left as what the lost
//! entries pointed at, and one entry stands for all of it. Zero is the GPU's word for doing nothing,
//! so any zeros in that stretch cost nothing.

use crate::pushbuffer::{ENTRY_SIZE, RingEntry, ring_entry};

/// The most entries a list is taken to hold. Past this it has wrapped.
pub(crate) const LIST_RING_ENTRIES: usize = 0x800;

/// More than this between two lists is left alone, which also keeps a made-up entry to a size the
/// GPU can take.
const MOST_RECOVERED_BYTES: u64 = 4 << 20;
const WORD_SIZE: u64 = 4;
/// Asks the GPU to finish what came before. The lost entries may have held one, so this keeps it.
const ENTRY_WAIT_FOR_EARLIER_WORK: u64 = 1 << 63;

fn entry_at(listing: &[u8], index: usize) -> Option<RingEntry> {
    let at = index * ENTRY_SIZE;
    let bytes = listing.get(at..at + ENTRY_SIZE)?;
    Some(RingEntry::parse(bytes.try_into().expect("an entry's size")))
}

/// The GPU address just past a list's commands.
pub(crate) fn end_of(listing: &[u8]) -> Option<u64> {
    let last = entry_at(listing, (listing.len() / ENTRY_SIZE).checked_sub(1)?)?;
    last.address.checked_add(u64::from(last.length) * WORD_SIZE)
}

/// One entry for what lies between the last list and this one, if the two are neighbours.
fn entry_for_gap(previous_end: Option<u64>, oldest_left: u64) -> Option<u64> {
    let start = previous_end?;
    let gap = oldest_left.checked_sub(start)?;
    let usable = (1..=MOST_RECOVERED_BYTES).contains(&gap) && gap.is_multiple_of(WORD_SIZE);
    usable.then(|| ring_entry(start, (gap / WORD_SIZE) as u32) | ENTRY_WAIT_FOR_EARLIER_WORK)
}

/// Puts a list that overran its ring back in order, with what it lost. `None` if that can't be
/// found. The rest must not run then, as commands that lost their set-up can fault the GPU, and the
/// host's desktop is on that GPU. `previous_end` is where this queue's last list ended.
pub(crate) fn repaired(listing: Vec<u8>, entries: u32, previous_end: Option<u64>) -> Option<Vec<u8>> {
    let entries = entries as usize;
    if entries <= LIST_RING_ENTRIES {
        return Some(listing);
    }
    let ring = &listing[..listing.len().min(LIST_RING_ENTRIES * ENTRY_SIZE)];
    let oldest_at = (entries % LIST_RING_ENTRIES * ENTRY_SIZE).min(ring.len());
    let (newest, oldest) = ring.split_at(oldest_at);
    let oldest_left = entry_at(oldest, 0)?.address;
    let Some(recovered) = entry_for_gap(previous_end, oldest_left) else {
        log::warn!(
            "a list of {entries} entries overran its ring: it starts at {oldest_left:#x}, and the last \
             ended at {previous_end:x?}"
        );
        return None;
    };
    Some([&recovered.to_le_bytes(), oldest, newest].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: u64 = 0x2_0000_0000;
    const STRIDE: u64 = 0x80;
    const WORDS: u32 = 17;

    /// Entries for `count` commands, one every `STRIDE` bytes from `first`, as in a real list.
    fn entries_from(first: u64, count: usize) -> Vec<u64> {
        (0..count as u64).map(|index| ring_entry(first + index * STRIDE, WORDS)).collect()
    }

    /// What's left of them in the ring once the driver has gone round it.
    fn wrapped(all: &[u64]) -> Vec<u8> {
        let mut ring = vec![0; LIST_RING_ENTRIES];
        for (index, &entry) in all.iter().enumerate() {
            ring[index % LIST_RING_ENTRIES] = entry;
        }
        ring.iter().flat_map(|entry| entry.to_le_bytes()).collect()
    }

    fn parsed(listing: &[u8]) -> Vec<RingEntry> {
        (0..listing.len() / ENTRY_SIZE).filter_map(|index| entry_at(listing, index)).collect()
    }

    #[test]
    fn a_list_that_fits_is_left_alone() {
        let listing = wrapped(&entries_from(START, 10));
        assert_eq!(repaired(listing.clone(), 10, Some(START)), Some(listing));
    }

    #[test]
    fn what_a_long_list_lost_comes_back_as_one_entry_in_front() {
        let all = entries_from(START, 2105);
        let fixed = parsed(&repaired(wrapped(&all), 2105, Some(START)).unwrap());
        let lost = 2105 - LIST_RING_ENTRIES as u64;
        assert_eq!(fixed.len(), LIST_RING_ENTRIES + 1);
        assert_eq!(fixed[0].address, START);
        assert_eq!(u64::from(fixed[0].length) * WORD_SIZE, lost * STRIDE);
        assert_eq!(fixed[1].address, START + lost * STRIDE);
        assert_eq!(fixed.last().unwrap().address, START + 2104 * STRIDE);
    }

    #[test]
    fn the_entries_left_stay_in_the_order_they_were_written() {
        let all = entries_from(START, 3000);
        let fixed = parsed(&repaired(wrapped(&all), 3000, Some(START)).unwrap());
        assert!(fixed.windows(2).all(|pair| pair[0].address < pair[1].address));
    }

    #[test]
    fn the_stand_in_entry_waits_for_earlier_work() {
        let all = entries_from(START, 2100);
        let fixed = repaired(wrapped(&all), 2100, Some(START)).unwrap();
        let entry = u64::from_le_bytes(fixed[..ENTRY_SIZE].try_into().unwrap());
        assert_ne!(entry & ENTRY_WAIT_FOR_EARLIER_WORK, 0);
    }

    #[test]
    fn with_no_last_list_nothing_runs() {
        assert_eq!(repaired(wrapped(&entries_from(START, 2100)), 2100, None), None);
    }

    #[test]
    fn a_last_list_that_is_not_a_neighbour_stops_it_too() {
        let all = entries_from(START, 2100);
        let lost = 2100 - LIST_RING_ENTRIES as u64;
        let oldest = START + lost * STRIDE;
        for previous in [oldest, oldest + 8, oldest - MOST_RECOVERED_BYTES - 4, oldest - 2, 0] {
            assert_eq!(repaired(wrapped(&all), 2100, Some(previous)), None, "{previous:#x}");
        }
    }

    #[test]
    fn a_list_cut_short_is_not_a_problem() {
        assert_eq!(repaired(Vec::new(), 2100, Some(START)), None);
        let (first, words) = (START + 0x100, 4);
        let listing = ring_entry(first, words).to_le_bytes().to_vec();
        assert_eq!(repaired(listing, 2048 + 8, Some(START)), None);
    }

    #[test]
    fn a_list_ends_where_its_last_entry_does() {
        let listing = wrapped(&entries_from(START, 2))[..2 * ENTRY_SIZE].to_vec();
        assert_eq!(end_of(&listing), Some(START + STRIDE + u64::from(WORDS) * WORD_SIZE));
        assert_eq!(end_of(&[]), None);
    }
}
