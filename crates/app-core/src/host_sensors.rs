//! The host card's readings, as the daemon's sensor thread publishes them. The viewer shows the
//! same figures a guest is told, and nothing in the app has to load the vendor's library.

use crate::gpu_sample::{CardReader, Sample};
use guesthouse_sensors::{SharedSnapshot, Snapshot, SnapshotReader, snapshot_path};

const BYTES_PER_MIB: u64 = 1 << 20;
const MILLIWATTS_PER_WATT: f64 = 1000.0;

pub struct HostSensors(SharedSnapshot);

impl HostSensors {
    /// Opens the shared file. `None` means nothing can be read, which the overlay says as much.
    pub fn open() -> Option<Box<dyn CardReader>> {
        let shared = SharedSnapshot::open(&snapshot_path()).ok()?;
        Some(Box::new(Self(shared)))
    }
}

/// The figures the overlay shows, out of everything the host read.
pub fn sample_of(snapshot: &Snapshot) -> Sample {
    Sample {
        taken: std::time::Duration::ZERO,
        load_percent: snapshot.load_percent,
        power_watts: snapshot.power_milliwatts.map(|milliwatts| f64::from(milliwatts) / MILLIWATTS_PER_WATT),
        clock_mhz: snapshot.graphics_megahertz,
        memory_used_mib: snapshot.memory_used_bytes.map(|bytes| bytes / BYTES_PER_MIB),
        memory_total_mib: snapshot.memory_total_bytes.map(|bytes| bytes / BYTES_PER_MIB),
    }
}

impl CardReader for HostSensors {
    /// A snapshot too old to be true reads as nothing, as the sampler's own check does.
    fn sample(&mut self) -> Sample {
        self.0.latest().as_ref().map_or_else(Sample::default, sample_of)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlays_units_come_from_the_snapshots() {
        let snapshot = Snapshot {
            load_percent: Some(42),
            power_milliwatts: Some(129_600),
            graphics_megahertz: Some(2400),
            memory_used_bytes: Some(5 << 30),
            memory_total_bytes: Some(32 << 30),
            ..Snapshot::default()
        };
        let sample = sample_of(&snapshot);
        assert_eq!(sample.load_percent, Some(42));
        assert_eq!(sample.power_watts, Some(129.6));
        assert_eq!(sample.clock_mhz, Some(2400));
        assert_eq!((sample.memory_used_mib, sample.memory_total_mib), (Some(5120), Some(32768)));
    }

    #[test]
    fn a_reading_the_host_has_not_got_stays_unknown() {
        let sample = sample_of(&Snapshot::default());
        assert_eq!(sample, Sample::default());
    }
}
