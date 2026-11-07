//! The host card's newest readings, shared between processes.
//!
//! One process reads the card and publishes a [`Snapshot`] in a small file in shared memory. Any
//! process of the same user can read it: a daemon answering a guest's performance queries, or the
//! app showing the host's figures over a guest's screen. A reader takes no lock and never waits.
//! What reads the card, and when, is the reader's own business and lives elsewhere.

mod shared;

pub use shared::SharedSnapshot;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A snapshot this old means the reading thread has stopped, so it is not used.
pub const OLDEST_USABLE: Duration = Duration::from_secs(5);

/// What was last read from the card. `None` is a value that couldn't be read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// When it was taken, in milliseconds since the Unix epoch.
    pub taken_at: u64,
    pub temperature_celsius: Option<u32>,
    /// How much of the last moment the card spent busy.
    pub load_percent: Option<u32>,
    /// The performance state, 0 for the fastest.
    pub performance_state: Option<u32>,
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

pub fn milliseconds_since_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}

pub fn is_fresh(snapshot: &Snapshot, now: u64) -> bool {
    now.saturating_sub(snapshot.taken_at) <= OLDEST_USABLE.as_millis() as u64
}

/// The file for this user's readings.
pub fn snapshot_path() -> PathBuf {
    // SAFETY: this asks for a number and can't fail.
    let user = unsafe { libc::getuid() };
    PathBuf::from(format!("/dev/shm/gpupv-sensors-{user}"))
}
