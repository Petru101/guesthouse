//! Live readings from the host's card, for the guest's performance queries.
//!
//! A reading can take many milliseconds, and the message path must never wait for one. So one thread
//! on the host reads the card on a schedule and publishes a snapshot, and the message path only
//! looks at the newest snapshot. Several daemons can run on one PC, so the snapshot lives in a
//! small shared file, and a lock on that file decides whose thread does the reading. If that daemon
//! goes, the lock passes to the next.
//!
//! The readings come through the vendor's documented management library, loaded when the daemon
//! starts. Without it there are no snapshots and everything else works as before.

mod sampler;
mod shared;

pub use sampler::{FastReadings, Sampler, SensorSource, SlowReadings};
pub use shared::SharedSnapshot;

use std::io;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A snapshot this old means the reading thread has stopped, so it is not used.
const OLDEST_USABLE: Duration = Duration::from_secs(5);

/// What was last read from the card. `None` is a value that couldn't be read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// When it was taken, in milliseconds since the Unix epoch.
    pub taken_at: u64,
    pub temperature_celsius: Option<u32>,
    pub power_milliwatts: Option<u32>,
    pub power_limit_milliwatts: Option<u32>,
    pub graphics_megahertz: Option<u32>,
    pub graphics_most_megahertz: Option<u32>,
    pub memory_megahertz: Option<u32>,
    pub memory_most_megahertz: Option<u32>,
    pub fan_percent: Option<u32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
}

/// Where the newest snapshot can be found. This runs on the message path, so it must not wait.
pub trait SnapshotReader {
    /// The newest snapshot, if there is one recent enough to be true.
    fn latest(&self) -> Option<Snapshot>;
}

fn milliseconds_since_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}

fn is_fresh(snapshot: &Snapshot, now: u64) -> bool {
    now.saturating_sub(snapshot.taken_at) <= OLDEST_USABLE.as_millis() as u64
}

fn snapshot_path() -> PathBuf {
    // SAFETY: this asks for a number and can't fail.
    let user = unsafe { libc::getuid() };
    PathBuf::from(format!("/dev/shm/gpupv-sensors-{user}"))
}
