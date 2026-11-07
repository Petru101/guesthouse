//! Live readings from the host's card, for the guest's performance queries.
//!
//! A reading can take many milliseconds, and the message path must never wait for one. So one thread
//! on the host reads the card on a schedule and publishes a snapshot, and the message path only
//! looks at the newest snapshot. Several daemons can run on one PC, so the snapshot lives in a
//! small shared file, and a lock on that file decides whose thread does the reading. If that daemon
//! goes, the lock passes to the next.
//!
//! The readings come through the vendor's documented management library, loaded when the daemon
//! starts. Without it there are no snapshots and everything else works as before. The snapshot's
//! file is the `guesthouse-sensors` crate's, so the app can read the same figures.

mod library;
mod sampler;

pub use guesthouse_sensors::{SharedSnapshot, Snapshot, SnapshotReader};
pub use sampler::{FastReadings, Sampler, SensorSource, SlowReadings};

use guesthouse_sensors::{milliseconds_since_epoch, snapshot_path};
use std::io;
use std::thread;
use std::time::Instant;

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
