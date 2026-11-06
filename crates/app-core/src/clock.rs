//! Time, so that a test of a two-hour wait doesn't take two hours.

use std::thread;
use std::time::{Duration, Instant};

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
    fn sleep(&self, time: Duration);
}

pub struct RealClock(Instant);

impl Default for RealClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&self, time: Duration) {
        thread::sleep(time);
    }
}
