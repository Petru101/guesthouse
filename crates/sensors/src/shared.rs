//! The snapshot in a small file in shared memory, written by one process and read by any.
//!
//! A reader takes no lock, as it is on the message path. Each write makes a counter odd, stores the
//! fields, and makes it even, and a reader that saw it change gives up rather than retrying for long.

use crate::{Snapshot, SnapshotReader, is_fresh, milliseconds_since_epoch};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering, fence};

const ONLY_THE_OWNER: u32 = 0o600;
const NOT_READ: u64 = u64::MAX;
const COUNTER: usize = 0;
const FIELDS: usize = 1;
const FIELD_COUNT: usize = 11;
const WORDS: usize = FIELDS + FIELD_COUNT;
const FILE_SIZE: usize = WORDS * size_of::<u64>();
/// How often a reader looks again if a write was under way. Writes take well under a microsecond.
const READ_TRIES: usize = 3;

struct Mapping {
    start: *mut AtomicU64,
}

// SAFETY: every access through the pointer is atomic, and the mapping lasts as long as this does.
unsafe impl Send for Mapping {}
// SAFETY: as above.
unsafe impl Sync for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this is the mapping made in `open`, and this is its last owner.
        unsafe { libc::munmap(self.start.cast(), FILE_SIZE) };
    }
}

fn encode(value: Option<u32>) -> u64 {
    value.map_or(NOT_READ, u64::from)
}

fn decode(word: u64) -> Option<u32> {
    (word != NOT_READ).then_some(word as u32)
}

fn encode_long(value: Option<u64>) -> u64 {
    value.unwrap_or(NOT_READ)
}

fn decode_long(word: u64) -> Option<u64> {
    (word != NOT_READ).then_some(word)
}

fn to_words(snapshot: &Snapshot) -> [u64; FIELD_COUNT] {
    [
        snapshot.taken_at,
        encode(snapshot.temperature_celsius),
        encode(snapshot.power_milliwatts),
        encode(snapshot.power_limit_milliwatts),
        encode(snapshot.graphics_megahertz),
        encode(snapshot.graphics_most_megahertz),
        encode(snapshot.memory_megahertz),
        encode(snapshot.memory_most_megahertz),
        encode(snapshot.fan_percent),
        encode_long(snapshot.memory_used_bytes),
        encode_long(snapshot.memory_total_bytes),
    ]
}

fn from_words(words: [u64; FIELD_COUNT]) -> Snapshot {
    Snapshot {
        taken_at: words[0],
        temperature_celsius: decode(words[1]),
        power_milliwatts: decode(words[2]),
        power_limit_milliwatts: decode(words[3]),
        graphics_megahertz: decode(words[4]),
        graphics_most_megahertz: decode(words[5]),
        memory_megahertz: decode(words[6]),
        memory_most_megahertz: decode(words[7]),
        fan_percent: decode(words[8]),
        memory_used_bytes: decode_long(words[9]),
        memory_total_bytes: decode_long(words[10]),
    }
}

/// See the module documentation.
pub struct SharedSnapshot {
    /// Each open of the file has a lock of its own, which is what lets processes take turns.
    file: File,
    mapping: Arc<Mapping>,
}

impl SharedSnapshot {
    /// Opens the file, making it if needed. Anyone else's file at this path is refused.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        let file = options.mode(ONLY_THE_OWNER).custom_flags(libc::O_NOFOLLOW).open(path)?;
        let details = file.metadata()?;
        // SAFETY: this asks for a number and can't fail.
        if details.uid() != unsafe { libc::getuid() } || details.mode() & 0o077 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "someone else's sensor file"));
        }
        file.set_len(FILE_SIZE as u64)?;
        let protection = libc::PROT_READ | libc::PROT_WRITE;
        // SAFETY: a fresh shared mapping of a file just sized to fit, at an address the kernel picks.
        let start = unsafe {
            libc::mmap(ptr::null_mut(), FILE_SIZE, protection, libc::MAP_SHARED, file.as_raw_fd(), 0)
        };
        if start == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file, mapping: Arc::new(Mapping { start: start.cast() }) })
    }

    /// Another handle on the same snapshot, with a lock of its own, as it is a new open of the file.
    /// Take the lock on one of these: the mapping keeps the first open alive, and a lock on that
    /// would outlast the handle.
    pub fn try_clone(&self) -> io::Result<Self> {
        let own_path = format!("/proc/self/fd/{}", self.file.as_raw_fd());
        let file = OpenOptions::new().read(true).write(true).open(own_path)?;
        Ok(Self { file, mapping: Arc::clone(&self.mapping) })
    }

    /// Waits until no other process is reading the card. The lock lasts until this process exits.
    pub fn wait_for_lock(&self) -> io::Result<()> {
        // SAFETY: a plain call on a file this owns.
        match unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_EX) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn word(&self, index: usize) -> &AtomicU64 {
        assert!(index < WORDS);
        // SAFETY: the assert keeps it inside the mapping, which lives as long as `self`.
        unsafe { &*self.mapping.start.add(index) }
    }

    /// Only the holder of the lock writes.
    pub fn publish(&self, snapshot: &Snapshot) {
        // A writer that died part way leaves the counter odd, and this just carries on from there.
        let writing = self.word(COUNTER).load(Ordering::Relaxed) | 1;
        self.word(COUNTER).store(writing, Ordering::Relaxed);
        fence(Ordering::Release);
        for (index, value) in to_words(snapshot).into_iter().enumerate() {
            self.word(FIELDS + index).store(value, Ordering::Relaxed);
        }
        self.word(COUNTER).store(writing + 1, Ordering::Release);
    }

    /// The snapshot as it stands, however old, or `None` if a write kept getting in the way.
    fn read(&self) -> Option<Snapshot> {
        (0..READ_TRIES).find_map(|_| {
            let before = self.word(COUNTER).load(Ordering::Acquire);
            let words: [u64; FIELD_COUNT] =
                std::array::from_fn(|index| self.word(FIELDS + index).load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            let steady = before.is_multiple_of(2) && self.word(COUNTER).load(Ordering::Relaxed) == before;
            steady.then(|| from_words(words))
        })
    }
}

impl SnapshotReader for SharedSnapshot {
    fn latest(&self) -> Option<Snapshot> {
        self.read().filter(|snapshot| is_fresh(snapshot, milliseconds_since_epoch()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temporary(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("guesthouse-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    fn now_snapshot() -> Snapshot {
        Snapshot {
            taken_at: milliseconds_since_epoch(),
            temperature_celsius: Some(40),
            memory_total_bytes: Some(32 << 30),
            ..Snapshot::default()
        }
    }

    #[test]
    fn an_empty_file_has_nothing_fresh() {
        let path = temporary("empty");
        assert_eq!(SharedSnapshot::open(&path).unwrap().latest(), None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_second_process_sees_what_the_first_wrote() {
        let path = temporary("two");
        let writer = SharedSnapshot::open(&path).unwrap();
        let reader = SharedSnapshot::open(&path).unwrap();
        let snapshot = now_snapshot();
        writer.publish(&snapshot);
        assert_eq!(reader.latest(), Some(snapshot));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_value_not_read_stays_not_read() {
        let path = temporary("unread");
        let shared = SharedSnapshot::open(&path).unwrap();
        shared.publish(&now_snapshot());
        assert_eq!(shared.latest().unwrap().fan_percent, None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_old_snapshot_is_not_used() {
        let path = temporary("old");
        let shared = SharedSnapshot::open(&path).unwrap();
        let old = Snapshot { taken_at: milliseconds_since_epoch() - 60_000, ..now_snapshot() };
        shared.publish(&old);
        assert_eq!(shared.latest(), None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_write_cut_short_reads_as_nothing_until_the_next_one() {
        let path = temporary("cut");
        let shared = SharedSnapshot::open(&path).unwrap();
        shared.publish(&now_snapshot());
        shared.word(COUNTER).fetch_add(1, Ordering::Relaxed);
        assert_eq!(shared.latest(), None);
        shared.publish(&now_snapshot());
        assert!(shared.latest().is_some());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_clone_has_a_lock_of_its_own() {
        let path = temporary("lock");
        let opened = SharedSnapshot::open(&path).unwrap();
        let first = opened.try_clone().unwrap();
        let second = opened.try_clone().unwrap();
        first.wait_for_lock().unwrap();
        // SAFETY: a plain call on a file this owns.
        let taken = unsafe { libc::flock(second.file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(taken, -1, "the lock is held");
        drop(first);
        second.wait_for_lock().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_file_others_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let path = temporary("open");
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(SharedSnapshot::open(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_link_is_not_followed() {
        let (target, link) = (temporary("target"), temporary("link"));
        fs::write(&target, b"kept").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(SharedSnapshot::open(&link).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"kept");
        fs::remove_file(link).unwrap();
        fs::remove_file(target).unwrap();
    }
}
