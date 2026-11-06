//! When each reading is taken. What takes them is a [`SensorSource`].

use super::Snapshot;
use std::time::{Duration, Instant};

/// The card refreshes these itself at about this rate, so faster would only repeat them.
const FAST_INTERVAL: Duration = Duration::from_millis(500);
/// These cost much more: one has been seen to take tens of milliseconds.
const SLOW_INTERVAL: Duration = Duration::from_secs(1);

/// Cheap readings. `None` is one that failed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FastReadings {
    pub temperature_celsius: Option<u32>,
    pub power_milliwatts: Option<u32>,
    pub graphics_megahertz: Option<u32>,
    pub memory_megahertz: Option<u32>,
}

/// Costly readings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlowReadings {
    pub fan_percent: Option<u32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub power_limit_milliwatts: Option<u32>,
    pub graphics_most_megahertz: Option<u32>,
    pub memory_most_megahertz: Option<u32>,
}

/// Something that can read the card.
pub trait SensorSource {
    fn read_fast(&mut self) -> FastReadings;
    fn read_slow(&mut self) -> SlowReadings;
}

/// Keeps the schedule, and the snapshot so far.
#[derive(Debug)]
pub struct Sampler<S> {
    source: S,
    snapshot: Snapshot,
    next_fast: Instant,
    next_slow: Instant,
}

impl<S: SensorSource> Sampler<S> {
    /// Everything is due at once, so a first snapshot is complete.
    pub fn new(source: S, now: Instant) -> Self {
        Self { source, snapshot: Snapshot::default(), next_fast: now, next_slow: now }
    }

    /// Takes whatever is due. Returns the new snapshot, or `None` if nothing was due.
    pub fn step(&mut self, now: Instant, milliseconds: u64) -> Option<Snapshot> {
        let (fast_due, slow_due) = (now >= self.next_fast, now >= self.next_slow);
        if fast_due {
            let read = self.source.read_fast();
            self.snapshot.temperature_celsius = read.temperature_celsius;
            self.snapshot.power_milliwatts = read.power_milliwatts;
            self.snapshot.graphics_megahertz = read.graphics_megahertz;
            self.snapshot.memory_megahertz = read.memory_megahertz;
            self.next_fast = now + FAST_INTERVAL;
        }
        if slow_due {
            let read = self.source.read_slow();
            self.snapshot.fan_percent = read.fan_percent;
            self.snapshot.memory_used_bytes = read.memory_used_bytes;
            self.snapshot.memory_total_bytes = read.memory_total_bytes;
            self.snapshot.power_limit_milliwatts = read.power_limit_milliwatts;
            self.snapshot.graphics_most_megahertz = read.graphics_most_megahertz;
            self.snapshot.memory_most_megahertz = read.memory_most_megahertz;
            self.next_slow = now + SLOW_INTERVAL;
        }
        (fast_due || slow_due).then(|| {
            self.snapshot.taken_at = milliseconds;
            self.snapshot
        })
    }

    /// How long until something is due.
    pub fn time_to_next(&self, now: Instant) -> Duration {
        self.next_fast.min(self.next_slow).saturating_duration_since(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Counting {
        fast: u32,
        slow: u32,
    }

    impl SensorSource for Counting {
        fn read_fast(&mut self) -> FastReadings {
            self.fast += 1;
            FastReadings { temperature_celsius: Some(self.fast), ..FastReadings::default() }
        }

        fn read_slow(&mut self) -> SlowReadings {
            self.slow += 1;
            SlowReadings { fan_percent: Some(self.slow), ..SlowReadings::default() }
        }
    }

    #[test]
    fn the_first_step_reads_everything() {
        let start = Instant::now();
        let mut sampler = Sampler::new(Counting::default(), start);
        let first = sampler.step(start, 7).unwrap();
        assert_eq!((first.temperature_celsius, first.fan_percent, first.taken_at), (Some(1), Some(1), 7));
    }

    #[test]
    fn fast_readings_come_twice_as_often_as_slow_ones() {
        let start = Instant::now();
        let mut sampler = Sampler::new(Counting::default(), start);
        for half_second in 0..=4 {
            sampler.step(start + FAST_INTERVAL * half_second, 0);
        }
        assert_eq!((sampler.source.fast, sampler.source.slow), (5, 3));
    }

    #[test]
    fn nothing_due_reads_nothing_and_publishes_nothing() {
        let start = Instant::now();
        let mut sampler = Sampler::new(Counting::default(), start);
        sampler.step(start, 0);
        assert_eq!(sampler.step(start + Duration::from_millis(1), 0), None);
        assert_eq!(sampler.source.fast, 1);
    }

    #[test]
    fn slow_values_stay_in_the_snapshot_between_their_readings() {
        let start = Instant::now();
        let mut sampler = Sampler::new(Counting::default(), start);
        sampler.step(start, 0);
        let second = sampler.step(start + FAST_INTERVAL, 0).unwrap();
        assert_eq!((second.temperature_celsius, second.fan_percent), (Some(2), Some(1)));
    }

    #[test]
    fn a_reading_that_fails_clears_the_old_value() {
        struct Failing;
        impl SensorSource for Failing {
            fn read_fast(&mut self) -> FastReadings {
                FastReadings::default()
            }
            fn read_slow(&mut self) -> SlowReadings {
                SlowReadings::default()
            }
        }
        let start = Instant::now();
        let mut sampler = Sampler::new(Failing, start);
        sampler.snapshot.temperature_celsius = Some(50);
        assert_eq!(sampler.step(start, 0).unwrap().temperature_celsius, None);
    }

    #[test]
    fn the_wait_is_until_the_soonest_reading() {
        let start = Instant::now();
        let mut sampler = Sampler::new(Counting::default(), start);
        sampler.step(start, 0);
        assert_eq!(sampler.time_to_next(start), FAST_INTERVAL);
    }
}
