//! A pretend copy engine for [`FakeRm`](super::fake::FakeRm). When a copy channel's doorbell rings, it
//! reads the channel's real ring and commands, does what they say to the files standing in for
//! exported memory, and releases the semaphore. So a test can see what a viewer would, from the same
//! words a card would get. It knows nothing else about a card, and proves nothing about one.

use super::Handle;
use crate::channel::copy::{METHOD_LAUNCH, METHOD_OFFSET_IN, METHOD_SET_SEMAPHORE};
use crate::channel::{CONTROL_BLOCK, GP_PUT};
use crate::pushbuffer::{Header, RingEntry};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicU32, Ordering};

const ENTRY_SIZE: usize = 8;
/// Words after the header in each group of methods this engine reads.
const OFFSET_WORDS: usize = 8;
const SEMAPHORE_WORDS: usize = 3;

/// One copy, as the engine was told to do it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineCopy {
    pub source: u64,
    pub target: u64,
    pub source_pitch: u32,
    pub target_pitch: u32,
    pub width: u32,
    pub rows: u32,
}

impl EngineCopy {
    /// One past the last byte it touches, for a surface of `pitch`.
    fn end(address: u64, pitch: u32, width: u32, rows: u32) -> u64 {
        address + u64::from(pitch) * u64::from(rows.saturating_sub(1)) + u64::from(width)
    }
}

/// Where a copy channel's host area is.
#[derive(Debug, Clone, Copy)]
pub struct ChannelArea {
    pub host_address: u64,
    pub gpu_address: u64,
    pub ring_entries: u32,
}

impl ChannelArea {
    fn word(&self, at: usize) -> &AtomicU32 {
        // SAFETY: a test keeps the channel, and so this mapping, alive while its doorbell can ring,
        // and everything read here is inside the area and aligned.
        unsafe { AtomicU32::from_ptr((self.host_address as *mut u8).add(at).cast()) }
    }

    fn load(&self, at: usize) -> u32 {
        self.word(at).load(Ordering::Acquire)
    }

    fn entry(&self, index: u32) -> RingEntry {
        let at = index as usize * ENTRY_SIZE;
        let mut bytes = [0; ENTRY_SIZE];
        bytes[..4].copy_from_slice(&self.load(at).to_le_bytes());
        bytes[4..].copy_from_slice(&self.load(at + 4).to_le_bytes());
        RingEntry::parse(bytes)
    }

    fn words(&self, entry: &RingEntry) -> Vec<u32> {
        let start = (entry.address - self.gpu_address) as usize;
        (0..entry.length as usize).map(|index| self.load(start + index * 4)).collect()
    }
}

/// See the module documentation.
#[derive(Debug, Default)]
pub struct FakeEngine {
    /// Copies run when this is on. Off, a doorbell does nothing, as with a stuck channel.
    pub running: bool,
    pub copies: Vec<EngineCopy>,
    /// What a real card would have called a fault: touching addresses nothing is mapped at.
    pub faults: Vec<String>,
    pub channels: HashMap<u32, ChannelArea>,
    read_up_to: HashMap<u32, u32>,
}

/// What's mapped at each GPU address, as (memory, length).
pub type Mapped = BTreeMap<u64, (Handle, u64)>;

fn mapping_at(mapped: &Mapped, address: u64) -> Option<(u64, Handle, u64)> {
    let (&start, &(memory, length)) = mapped.range(..=address).next_back()?;
    (address < start + length).then_some((start, memory, length))
}

impl FakeEngine {
    /// Runs what a channel has been given since it last looked.
    pub fn run(&mut self, token: u32, mapped: &Mapped, exports: &HashMap<Handle, File>) {
        let Some(area) = self.channels.get(&token).copied() else { return };
        let put = area.load(CONTROL_BLOCK + GP_PUT);
        let mut at = self.read_up_to.get(&token).copied().unwrap_or(0);
        while at != put {
            let words = area.words(&area.entry(at));
            self.run_words(&area, &words, mapped, exports);
            at = (at + 1) % area.ring_entries;
        }
        self.read_up_to.insert(token, at);
    }

    fn run_words(
        &mut self,
        area: &ChannelArea,
        words: &[u32],
        mapped: &Mapped,
        exports: &HashMap<Handle, File>,
    ) {
        let mut offsets = [0; OFFSET_WORDS];
        let mut semaphore = [0; SEMAPHORE_WORDS];
        let mut at = 0;
        while at < words.len() {
            let header = Header::parse(words[at]);
            let data = &words[at + 1..at + header.stride()];
            match header.method {
                METHOD_OFFSET_IN => offsets.copy_from_slice(data),
                METHOD_SET_SEMAPHORE => semaphore.copy_from_slice(data),
                METHOD_LAUNCH => {
                    self.copy(&offsets, mapped, exports);
                    self.release(area, &semaphore);
                }
                _ => {}
            }
            at += header.stride();
        }
    }

    fn check(&mut self, what: &str, address: u64, end: u64, mapped: &Mapped) -> Option<(u64, Handle)> {
        match mapping_at(mapped, address) {
            Some((start, memory, length)) if end <= start + length => Some((start, memory)),
            _ => {
                self.faults.push(format!("{what} at {address:#x} to {end:#x}"));
                None
            }
        }
    }

    fn copy(&mut self, words: &[u32; OFFSET_WORDS], mapped: &Mapped, exports: &HashMap<Handle, File>) {
        let long = |high: u32, low: u32| u64::from(high) << 32 | u64::from(low);
        let copy = EngineCopy {
            source: long(words[0], words[1]),
            target: long(words[2], words[3]),
            source_pitch: words[4],
            target_pitch: words[5],
            width: words[6],
            rows: words[7],
        };
        self.copies.push(copy);
        let serial = self.copies.len();
        let read_end = EngineCopy::end(copy.source, copy.source_pitch, copy.width, copy.rows);
        self.check("a read", copy.source, read_end, mapped);
        let write_end = EngineCopy::end(copy.target, copy.target_pitch, copy.width, copy.rows);
        let Some((start, memory)) = self.check("a write", copy.target, write_end, mapped) else { return };
        let Some(file) = exports.get(&memory) else { return };
        // Each row gets its own value, so rows out of place, or a half-written picture, can be seen.
        for row in 0..copy.rows {
            let bytes = vec![Self::stamp(serial, row); copy.width as usize];
            let at = copy.target - start + u64::from(row) * u64::from(copy.target_pitch);
            file.write_all_at(&bytes, at).expect("a file in memory takes a write");
        }
    }

    /// What the copy numbered `serial` puts in a row.
    pub fn stamp(serial: usize, row: u32) -> u8 {
        (serial as u32).wrapping_add(row) as u8 | 1
    }

    fn release(&self, area: &ChannelArea, semaphore: &[u32; SEMAPHORE_WORDS]) {
        let address = u64::from(semaphore[0]) << 32 | u64::from(semaphore[1]);
        let at = (address - area.gpu_address) as usize;
        area.word(at).store(semaphore[2], Ordering::Release);
    }
}
