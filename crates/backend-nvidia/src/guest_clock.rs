//! The time as a guest's clock keeps it.
//!
//! A guest's clock is set from the host's when it boots. After that it runs at the raw speed of the
//! CPU's counter, with none of the corrections NTP makes to the host's clock.

use std::mem::MaybeUninit;
use std::time::{SystemTime, UNIX_EPOCH};

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const NANOS_PER_MICRO: i128 = 1_000;

/// The host's clock with NTP's corrections left out, in nanoseconds from some point at its boot.
fn raw_nanos() -> Option<i128> {
    let mut time = MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: the call fills in the structure it's given, and says so by returning 0.
    let done = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, time.as_mut_ptr()) };
    (done == 0).then(|| {
        // SAFETY: the call succeeded, so it wrote the whole structure.
        let time = unsafe { time.assume_init() };
        i128::from(time.tv_sec) * NANOS_PER_SECOND + i128::from(time.tv_nsec)
    })
}

/// Time as the guest's clock keeps it, in microseconds since 1970.
#[derive(Debug, Clone, Copy, Default)]
pub struct GuestClock {
    /// What to add to the raw clock.
    offset_nanos: Option<i128>,
}

impl GuestClock {
    /// Lines up with the host's wall clock, as the guest does when it starts.
    pub fn restart(&mut self) {
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|since| since.as_nanos() as i128);
        self.offset_nanos = wall.zip(raw_nanos()).map(|(wall, raw)| wall - raw);
    }

    pub(crate) fn now_micros(&mut self) -> Option<u64> {
        if self.offset_nanos.is_none() {
            self.restart();
        }
        u64::try_from((raw_nanos()? + self.offset_nanos?) / NANOS_PER_MICRO).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guest_clock_only_goes_forward() {
        let mut clock = GuestClock::default();
        let first = clock.now_micros().unwrap();
        assert!(clock.now_micros().unwrap() >= first);
    }

    #[test]
    fn a_restart_lines_up_with_the_wall_clock_again() {
        let mut clock = GuestClock { offset_nanos: Some(0) };
        let since_boot = clock.now_micros().unwrap();
        clock.restart();
        assert!(clock.now_micros().unwrap() > since_boot);
    }
}
