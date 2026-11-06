//! A frame sink that tells a viewer where the newest frame is, through a small file in shared memory.
//!
//! This is a stopgap for the viewer as it is today, kept small, until there is a socket to replace it.

use guesthouse_core::{StagingFrame, StagingRing, StagingSink};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

const FILE_SIZE: usize = 4096;
const MAGIC: &[u8; 8] = b"GPVDISP1";
const WIDTH: usize = 8;
const HEIGHT: usize = 12;
const PITCH: usize = 16;
const OFFSET: usize = 24;
const SEQUENCE: usize = 32;

/// See the module documentation. An odd sequence means the frame is being written.
pub struct DisplayFile {
    path: PathBuf,
    window_base: u64,
    mapped: *mut u8,
    sequence: u64,
}

impl DisplayFile {
    pub fn new(path: PathBuf, window_base: u64) -> Self {
        Self { path, window_base, mapped: ptr::null_mut(), sequence: 0 }
    }

    fn map(&mut self) -> bool {
        if !self.mapped.is_null() {
            return true;
        }
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&self.path);
        let Ok(file) = file.and_then(|file| file.set_len(FILE_SIZE as u64).map(|()| file)) else {
            log::warn!("no display file at {}", self.path.display());
            return false;
        };
        let protection = libc::PROT_READ | libc::PROT_WRITE;
        // SAFETY: the kernel picks the address, and the mapping stays until this is dropped.
        let mapped = unsafe {
            libc::mmap(ptr::null_mut(), FILE_SIZE, protection, libc::MAP_SHARED, file.as_raw_fd(), 0)
        };
        if mapped == libc::MAP_FAILED {
            return false;
        }
        self.mapped = mapped.cast();
        true
    }

    fn put(&mut self, at: usize, bytes: &[u8]) {
        // SAFETY: the offsets are all inside the mapping, and `map` has made it.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), self.mapped.add(at), bytes.len()) };
    }

    fn set_sequence(&mut self, sequence: u64) {
        // SAFETY: the field is aligned and inside the mapping, and `map` has made it.
        let field = unsafe { AtomicU64::from_ptr(self.mapped.add(SEQUENCE).cast()) };
        field.store(sequence, Ordering::Release);
    }
}

impl StagingSink for DisplayFile {
    fn ring_opened(&mut self, ring: &StagingRing) {
        if !self.map() {
            return;
        }
        self.put(0, MAGIC);
        self.put(WIDTH, &ring.surface.width.to_le_bytes());
        self.put(HEIGHT, &ring.surface.height.to_le_bytes());
        self.put(PITCH, &ring.surface.pitch.to_le_bytes());
        self.put(OFFSET, &(ring.guest_address - self.window_base).to_le_bytes());
    }

    /// A viewer can look at any moment, so the frame is marked as changing first.
    fn slot_overwriting(&mut self, _generation: u32, _slot: u32) {
        if self.map() {
            self.set_sequence(self.sequence * 2 + 1);
        }
    }

    fn frame_ready(&mut self, frame: &StagingFrame) {
        self.sequence = frame.sequence;
        if self.map() {
            self.set_sequence(frame.sequence * 2);
        }
    }

    fn ring_closed(&mut self, _generation: u32) {}
}

impl Drop for DisplayFile {
    fn drop(&mut self) {
        if !self.mapped.is_null() {
            // SAFETY: this is the mapping made in `map`, and nothing can use it now.
            unsafe { libc::munmap(self.mapped.cast(), FILE_SIZE) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::SurfaceShape;
    use std::env;
    use std::fs;
    use std::process;

    #[test]
    fn a_frame_is_odd_while_it_changes_and_even_after() {
        let path = env::temp_dir().join(format!("guesthouse-display-{}", process::id()));
        let surface = SurfaceShape {
            width: 640,
            height: 480,
            format: 21,
            pitch: 2560,
            tiling: None,
            refresh: (60, 1),
            samples: (1, 0),
            colour: 0,
        };
        let mut display = DisplayFile::new(path.clone(), 0x1000);
        display.ring_opened(&StagingRing { generation: 1, slots: 1, surface, guest_address: 0x3000 });
        display.frame_ready(&StagingFrame { generation: 1, slot: 0, sequence: 4 });
        display.slot_overwriting(1, 0);
        let sequence = |bytes: &[u8]| u64::from_le_bytes(bytes[SEQUENCE..SEQUENCE + 8].try_into().unwrap());
        let bytes = fs::read(&path).unwrap();
        assert_eq!(sequence(&bytes), 9);
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(bytes[OFFSET..OFFSET + 8], 0x2000u64.to_le_bytes());
        display.frame_ready(&StagingFrame { generation: 1, slot: 0, sequence: 5 });
        assert_eq!(sequence(&fs::read(&path).unwrap()), 10);
        fs::remove_file(path).unwrap();
    }
}
