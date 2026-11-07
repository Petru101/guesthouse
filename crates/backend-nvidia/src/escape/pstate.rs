//! The card's performance state, which the vendor's tools ask for by a control. It is answered from
//! the host's sampler, as every other sensor is, and refused while there is no reading.

use super::put;
use crate::sensors::Snapshot;

pub(super) const CONTROL: u32 = 0x2080_2068;
pub(super) const SIZE: usize = 4;
const STATE_AT: usize = 0;
/// The answer is one bit for each state, P0 lowest.
const MOST_STATES: u32 = 16;

/// Whether a reading was there to give.
pub(super) fn answer(live: Option<&Snapshot>, params: &mut [u8]) -> bool {
    let Some(state) = live.and_then(|live| live.performance_state) else { return false };
    if state >= MOST_STATES {
        return false;
    }
    put(params, STATE_AT, 1 << state);
    true
}

#[cfg(test)]
mod tests {
    use super::super::get;
    use super::*;

    #[test]
    fn the_state_is_given_as_its_bit() {
        let live = Snapshot { performance_state: Some(8), ..Snapshot::default() };
        let mut params = [0; SIZE];
        assert!(answer(Some(&live), &mut params));
        assert_eq!(get(&params, STATE_AT), Some(0x100));
    }

    #[test]
    fn no_reading_is_no_answer() {
        let mut params = [0; SIZE];
        assert!(!answer(None, &mut params));
        assert!(!answer(Some(&Snapshot::default()), &mut params));
        let unknown = Snapshot { performance_state: Some(32), ..Snapshot::default() };
        assert!(!answer(Some(&unknown), &mut params));
        assert_eq!(get(&params, STATE_AT), Some(0));
    }
}
