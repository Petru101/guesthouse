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

mod library;
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

/// Reads and publishes for as long as this process holds the lock, which is until it exits.
fn read_forever(snapshot: SharedSnapshot, card_uuid: String) {
    if let Err(error) = snapshot.wait_for_lock() {
        log::warn!("no host sensors: {error}");
        return;
    }
    let Some(source) = library::Library::open(library::LIBRARY_NAME, &card_uuid) else {
        log::info!("no host sensors: the management library isn't usable here");
        return;
    };
    let mut sampler = Sampler::new(source, Instant::now());
    loop {
        let reading = sampler.step(Instant::now(), milliseconds_since_epoch());
        if let Some(reading) = reading {
            snapshot.publish(&reading);
        }
        thread::sleep(sampler.time_to_next(Instant::now()));
    }
}

/// Opens the shared snapshot and starts this daemon's thread, which waits its turn to read.
pub fn start(card_uuid: &[u8; 16]) -> io::Result<SharedSnapshot> {
    let snapshot = SharedSnapshot::open(&snapshot_path())?;
    let for_thread = snapshot.try_clone()?;
    let card_uuid = library::uuid_text(card_uuid);
    thread::Builder::new().name("host-sensors".to_owned()).spawn(|| read_forever(for_thread, card_uuid))?;
    Ok(snapshot)
}
