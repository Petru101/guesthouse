//! How long the daemon waits for a run on the card, and what it makes of a long one.
//!
//! A slow run isn't a failed one: a heavy frame or a big upload can take seconds on a small card,
//! and failing its queue would cost the guest its device for nothing. A hung one is the kernel's to
//! deal with. amdgpu gives up on a job after its own timeout, resets that queue, and signals the job's
//! sync object, so the wait ends then either way. That timeout is 2 seconds by default on current
//! kernels, and was 10 seconds, or a minute on a compute queue, on older ones. The daemon only gives
//! up by itself if the kernel never answers, so a broken card can't hold it for ever.

use crate::submit::wait_eventfd;
use std::io;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

/// When a run counts as slow and is noted in the log. A frame's work takes a few milliseconds.
const SLOW_RUN: Duration = Duration::from_secs(2);
/// When the daemon stops waiting for a run the kernel hasn't finished or reset: past the longest
/// default timeout amdgpu has had, a minute, with time for its reset.
const HUNG_RUN: Duration = Duration::from_secs(75);

/// How long to wait before a run is noted as slow, and before it's given up on.
#[derive(Debug, Clone, Copy)]
pub struct Patience {
    pub slow: Duration,
    pub hung: Duration,
}

impl Patience {
    pub const RUN: Self = Self { slow: SLOW_RUN, hung: HUNG_RUN };
}

/// How a wait for a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// It retired, after this long if that was slow.
    Retired { slow: Option<Duration> },
    /// The kernel neither finished nor reset it in time.
    Hung,
}

/// Waits for `event`, armed on a run's sync object, to say the run retired.
pub fn wait_for_run(event: &impl AsRawFd, patience: Patience, engine: u32) -> io::Result<Waited> {
    let started = Instant::now();
    if wait_eventfd(event, millis(patience.slow))? {
        return Ok(Waited::Retired { slow: None });
    }
    log::warn!(
        "work on engine family {engine} still running after {} ms, so it's waited for until it \
         finishes or the kernel resets it",
        patience.slow.as_millis()
    );
    if wait_eventfd(event, millis(patience.hung.saturating_sub(patience.slow)))? {
        let took = started.elapsed();
        log::warn!("work on engine family {engine} finished after {} ms", took.as_millis());
        return Ok(Waited::Retired { slow: Some(took) });
    }
    Ok(Waited::Hung)
}

fn millis(time: Duration) -> i32 {
    i32::try_from(time.as_millis()).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::thread;

    fn eventfd() -> OwnedFd {
        // SAFETY: eventfd returns a fresh descriptor or -1, checked here.
        let raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
        assert!(raw >= 0);
        // SAFETY: the descriptor is fresh and owned here.
        unsafe { OwnedFd::from_raw_fd(raw) }
    }

    fn signal(event: &OwnedFd) {
        let one = 1u64.to_ne_bytes();
        // SAFETY: an eventfd takes an 8-byte counter.
        let wrote = unsafe { libc::write(event.as_raw_fd(), one.as_ptr().cast(), one.len()) };
        assert_eq!(wrote, 8);
    }

    const SHORT: Patience = Patience { slow: Duration::from_millis(30), hung: Duration::from_millis(600) };

    #[test]
    fn a_run_that_retires_in_time_is_not_slow() {
        let event = eventfd();
        signal(&event);
        assert_eq!(wait_for_run(&event, SHORT, 0).unwrap(), Waited::Retired { slow: None });
    }

    #[test]
    fn a_slow_run_is_waited_for_and_not_given_up_on() {
        let event = eventfd();
        let raw = event.try_clone().unwrap();
        let late = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            signal(&raw);
        });
        let waited = wait_for_run(&event, SHORT, 0).unwrap();
        late.join().unwrap();
        let Waited::Retired { slow: Some(took) } = waited else { panic!("{waited:?}") };
        assert!(took >= Duration::from_millis(100));
    }

    #[test]
    fn a_run_nobody_finishes_is_hung_after_the_patience() {
        let event = eventfd();
        let patience = Patience { slow: Duration::from_millis(10), hung: Duration::from_millis(40) };
        let started = Instant::now();
        assert_eq!(wait_for_run(&event, patience, 0).unwrap(), Waited::Hung);
        assert!(started.elapsed() >= Duration::from_millis(40));
    }
}
