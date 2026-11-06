//! Where the time of a submission goes.
//!
//! Each stage gets a histogram of how long it took, in power-of-two buckets, so that recording a
//! sample is a clock read and an increment. The clock read is the main cost, about 25 ns, so
//! this is off unless asked for.

use std::fmt;
use std::time::{Duration, Instant};

const BUCKETS: usize = 40;

/// The points of a submission, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The frame is whole, and the daemon has started on it.
    Received,
    /// Checked and parsed, and the vendor has been handed the work.
    Started,
    /// The vendor has finished preparing it.
    Prepared,
    /// The doorbell was rung, or the gate holding it opened.
    Rung,
    /// The reply has been written to the socket.
    Replied,
}

const STAGES: [Stage; 5] = [Stage::Received, Stage::Started, Stage::Prepared, Stage::Rung, Stage::Replied];

/// How often and how long, for one stage.
#[derive(Debug, Clone, Copy)]
struct Histogram {
    counts: [u64; BUCKETS],
    total: Duration,
    longest: Duration,
}

impl Histogram {
    const EMPTY: Self = Self { counts: [0; BUCKETS], total: Duration::ZERO, longest: Duration::ZERO };

    fn record(&mut self, taken: Duration) {
        let nanos = taken.as_nanos().max(1) as u64;
        let bucket = (u64::BITS - 1 - nanos.leading_zeros()) as usize;
        self.counts[bucket.min(BUCKETS - 1)] += 1;
        self.total += taken;
        self.longest = self.longest.max(taken);
    }

    fn count(&self) -> u64 {
        self.counts.iter().sum()
    }

    /// The top of the bucket holding the middle sample, which is within a factor of two.
    fn median_nanos(&self) -> u64 {
        let middle = self.count().div_ceil(2);
        let mut seen = 0;
        for (bucket, count) in self.counts.iter().enumerate() {
            seen += count;
            if seen >= middle && *count != 0 {
                return 1 << (bucket + 1);
            }
        }
        0
    }
}

/// Timings for every stage, and for the two waits that end a submission.
#[derive(Debug)]
pub struct Timings {
    enabled: bool,
    last: Option<(Stage, Instant)>,
    stages: [Histogram; STAGES.len()],
    fence: Histogram,
    held: Histogram,
}

impl Timings {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            last: None,
            stages: [Histogram::EMPTY; STAGES.len()],
            fence: Histogram::EMPTY,
            held: Histogram::EMPTY,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Marks reaching a stage. The time since the last mark counts against this one.
    pub fn mark(&mut self, stage: Stage) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        // Most messages aren't submissions, and go straight from received to replied.
        if let Some((last, since)) = self.last
            && stage != Stage::Received
            && !(stage == Stage::Replied && last == Stage::Received)
        {
            self.stages[stage as usize].record(now - since);
        }
        self.last = (stage != Stage::Replied).then_some((stage, now));
    }

    /// Notes how long the GPU took from the doorbell to a fence being seen to move.
    pub fn fence_seen(&mut self, taken: Duration) {
        if self.enabled {
            self.fence.record(taken);
        }
    }

    /// Notes how long a reply was held.
    pub fn reply_released(&mut self, taken: Duration) {
        if self.enabled {
            self.held.record(taken);
        }
    }
}

impl fmt::Display for Timings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut line = |name: &str, histogram: &Histogram| {
            let count = histogram.count();
            let mean = histogram.total.as_nanos().checked_div(u128::from(count)).unwrap_or(0);
            writeln!(
                f,
                "{name:<12} {count:>9} times, median under {:>9} ns, mean {mean:>9} ns, longest {:>9} ns",
                histogram.median_nanos(),
                histogram.longest.as_nanos()
            )
        };
        for (stage, histogram) in STAGES.iter().zip(&self.stages).skip(1) {
            line(&format!("{stage:?}"), histogram)?;
        }
        line("Fence seen", &self.fence)?;
        line("Reply held", &self.held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_counted_when_off() {
        let mut timings = Timings::new(false);
        timings.mark(Stage::Received);
        timings.mark(Stage::Replied);
        timings.fence_seen(Duration::from_micros(1));
        assert_eq!(timings.stages[Stage::Replied as usize].count(), 0);
        assert_eq!(timings.fence.count(), 0);
    }

    #[test]
    fn each_mark_counts_the_time_since_the_last() {
        let mut timings = Timings::new(true);
        timings.mark(Stage::Received);
        timings.mark(Stage::Started);
        timings.mark(Stage::Replied);
        assert_eq!(timings.stages[Stage::Started as usize].count(), 1);
        assert_eq!(timings.stages[Stage::Replied as usize].count(), 1);
        assert_eq!(timings.stages[Stage::Received as usize].count(), 0);
    }

    #[test]
    fn a_message_that_is_no_submission_counts_nothing() {
        let mut timings = Timings::new(true);
        timings.mark(Stage::Received);
        timings.mark(Stage::Replied);
        assert_eq!(timings.stages[Stage::Replied as usize].count(), 0);
    }

    #[test]
    fn a_mark_with_nothing_before_it_counts_nothing() {
        let mut timings = Timings::new(true);
        timings.mark(Stage::Replied);
        assert_eq!(timings.stages[Stage::Replied as usize].count(), 0);
    }

    #[test]
    fn the_median_is_the_top_of_its_bucket() {
        let mut histogram = Histogram::EMPTY;
        [100, 110, 5000].iter().for_each(|&nanos| histogram.record(Duration::from_nanos(nanos)));
        assert_eq!(histogram.median_nanos(), 128);
    }
}
