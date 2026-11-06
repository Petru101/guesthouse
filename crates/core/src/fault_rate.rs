//! The last guard against a guest that keeps faulting.
//!
//! One process failing costs that process, and the rest of the guest goes on. But every fault can stall
//! the whole card, and a guest can start new processes for as long as it likes. So faults are counted
//! over a short time, and too many in that time stop all of the guest's GPU work. A guest that faults
//! now and then, even for weeks, never gets there.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How many processes may fault within [`FAULT_WINDOW`]. Each costs up to a queue's timeout, so a
/// program that faults in a loop on its own gets here in under a minute.
pub const MOST_FAULTS: usize = 5;
pub const FAULT_WINDOW: Duration = Duration::from_secs(60);

/// Recent faults, and whether they've been too many. Once they have, that stands until the guest
/// restarts: going back to work would only let it stall the card again.
#[derive(Debug)]
pub struct FaultRate {
    recent: VecDeque<Instant>,
    stopped: bool,
}

impl FaultRate {
    pub fn new() -> Self {
        Self { recent: VecDeque::with_capacity(MOST_FAULTS), stopped: false }
    }

    /// Counts a fault. True if this is the one that was too many.
    pub fn note(&mut self, now: Instant) -> bool {
        while self.recent.front().is_some_and(|&fault| now.saturating_duration_since(fault) > FAULT_WINDOW) {
            self.recent.pop_front();
        }
        self.recent.push_back(now);
        let too_many = !self.stopped && self.recent.len() >= MOST_FAULTS;
        self.stopped |= too_many;
        too_many
    }

    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// A guest that has restarted has none of the processes that did this.
    pub fn forget(&mut self) {
        self.recent.clear();
        self.stopped = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults_spread_out_never_stop_anything() {
        let start = Instant::now();
        let mut rate = FaultRate::new();
        for fault in 0..1000 {
            assert!(!rate.note(start + FAULT_WINDOW * fault));
        }
        assert!(!rate.stopped());
    }

    #[test]
    fn too_many_at_once_stop_work_and_say_so_once() {
        let start = Instant::now();
        let mut rate = FaultRate::new();
        let said: Vec<bool> = (0..MOST_FAULTS + 2).map(|_| rate.note(start)).collect();
        assert_eq!(said.iter().filter(|said| **said).count(), 1);
        assert!(said[MOST_FAULTS - 1]);
        assert!(rate.stopped());
    }

    #[test]
    fn being_stopped_outlasts_the_window() {
        let start = Instant::now();
        let mut rate = FaultRate::new();
        (0..MOST_FAULTS).for_each(|_| _ = rate.note(start));
        rate.note(start + FAULT_WINDOW * 10);
        assert!(rate.stopped());
    }

    #[test]
    fn a_restart_starts_the_count_again() {
        let start = Instant::now();
        let mut rate = FaultRate::new();
        (0..MOST_FAULTS).for_each(|_| _ = rate.note(start));
        rate.forget();
        assert!(!rate.stopped());
        assert!(!rate.note(start));
    }
}
