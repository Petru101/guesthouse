//! Where the guest put its IO space: the part of the shared window the host hands out.
//!
//! The guest reserves this range among its own devices, and the answer to its message is only a
//! status, so the host can't choose a place and tell it. What the host can do is refuse: the fence
//! area at the front of the range goes to the GPU, so all of it must be memory shared with the guest.

use crate::fences::{FENCE_AREA, FENCE_PAGE};
use std::ops::Range;

/// A place for the IO space that passed [`IoSpace::place`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoSpace {
    start: u64,
}

impl IoSpace {
    /// `None` unless the fence area starts on a page and fits in the window.
    pub fn place(start: u64, window: &Range<u64>) -> Option<Self> {
        let end = start.checked_add(FENCE_AREA)?;
        let fits = start >= window.start && end <= window.end;
        (fits && start.is_multiple_of(FENCE_PAGE)).then_some(Self { start })
    }

    pub fn start(self) -> u64 {
        self.start
    }

    /// Where memory after the fence pages starts. `place` has seen that this doesn't overflow.
    pub fn after_fences(self) -> u64 {
        self.start + FENCE_AREA
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Range<u64> = 0x1000_0000..0x1000_0000 + 2 * FENCE_AREA;

    #[test]
    fn the_whole_fence_area_has_to_fit() {
        assert!(IoSpace::place(WINDOW.start, &WINDOW).is_some());
        assert!(IoSpace::place(WINDOW.end - FENCE_AREA, &WINDOW).is_some());
        assert!(IoSpace::place(WINDOW.end - FENCE_AREA + FENCE_PAGE, &WINDOW).is_none());
        assert!(IoSpace::place(WINDOW.start - FENCE_PAGE, &WINDOW).is_none());
    }

    #[test]
    fn a_start_inside_a_page_is_refused() {
        assert!(IoSpace::place(WINDOW.start + 8, &WINDOW).is_none());
    }

    #[test]
    fn an_end_past_the_last_address_is_refused() {
        assert!(IoSpace::place(u64::MAX - FENCE_PAGE + 1, &(0..u64::MAX)).is_none());
    }
}
