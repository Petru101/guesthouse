//! The daemon's own copy engine channel, for frames and for moving an allocation's contents.
//!
//! A copy is started and then looked at later, so that the message thread never waits for one.
//! The engine releases a semaphore in the host area when it finishes. Method numbers and values are
//! in the open kernel module's header for the copy class.

use super::{COPY_SUBCHANNEL, Channel, l2_invalidate_words};
use crate::pushbuffer::{increasing, ring_entry};
use crate::rm::Rm;
use crate::rm::device::Device;
use crate::tiling::{BLOCK_LIMIT, origin_fits};
use std::time::{Duration, Instant};

/// Ring entries, at the start of the area.
pub const RING_ENTRIES: u32 = 1024;
const ENTRY_SIZE: usize = 8;
const PUSH: usize = 0x12000;
const SEMAPHORE: usize = 0x14000;
/// How long a copy gets before the channel counts as dead.
const COPY_TIMEOUT: Duration = Duration::from_secs(2);

/// The most one window can hold, for moving an allocation's contents.
pub const COPY_WINDOW: u64 = 64 << 20;
/// The two windows, which follow a copy channel's area in GPU addresses.
pub const COPY_AREA_EXTRA: u64 = 2 * COPY_WINDOW;

pub(crate) const METHOD_SET_SEMAPHORE: u32 = 0x240;
const METHOD_SET_RENDER_ENABLE: u32 = 0x25C;
pub(crate) const METHOD_LAUNCH: u32 = 0x300;
pub(crate) const METHOD_OFFSET_IN: u32 = 0x400;
const METHOD_SET_SOURCE_BLOCK_SIZE: u32 = 0x728;
const RENDER_ALWAYS: u32 = 1;
/// Launches: non-pipelined, flushed, ending in a one-word semaphore.
const LAUNCH_PITCH_LINES: u32 = 0x38E;
const LAUNCH_BLOCK_TO_PITCH_LINES: u32 = 0x30E;
const LAUNCH_PITCH_ONE_LINE: u32 = 0x18E;
/// Flushed, with nothing to transfer, ending in a one-word semaphore.
pub(crate) const LAUNCH_SEMAPHORE_ONLY: u32 = 0xC;
const BLOCK_HEIGHT_SHIFT: u32 = 4;
const ORIGIN_Y_SHIFT: u32 = 16;
/// Blocks are one GOB wide and deep, and GOBs have eight rows.
const BLOCK_GOB_HEIGHT_8: u32 = 1 << 12;

/// A rectangle to copy, in bytes and rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rows {
    pub source: u64,
    pub target: u64,
    pub source_pitch: u32,
    pub target_pitch: u32,
    pub width: u32,
    pub count: u32,
}

/// A rectangle out of a tiled surface. `block` is the log2 of its block height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tiled {
    pub rows: Rows,
    pub x: u32,
    pub y: u32,
    pub surface_width: u32,
    pub surface_height: u32,
    pub block: u32,
}

/// See the module documentation.
#[derive(Debug)]
pub struct CopyChannel {
    pub channel: Channel,
    put: u32,
    sequence: u32,
    started: Option<Instant>,
    /// The last batch, to run again on a new channel if this one dies.
    pub last_words: Vec<u32>,
}

impl CopyChannel {
    pub fn new(channel: Channel) -> Self {
        channel.area().store_u64(0, channel.prelude_entry());
        Self { channel, put: 1, sequence: 0, started: None, last_words: Vec::new() }
    }

    /// The methods every copy shares: where from and to, and the semaphore that marks the end.
    fn head(&mut self, rows: &Rows) -> Vec<u32> {
        self.sequence = self.sequence.wrapping_add(1);
        let done = self.channel.area_address() + SEMAPHORE as u64;
        let header = |method, count| increasing(COPY_SUBCHANNEL, method, count);
        vec![
            header(METHOD_SET_RENDER_ENABLE, 1),
            RENDER_ALWAYS,
            header(METHOD_OFFSET_IN, 8),
            (rows.source >> 32) as u32,
            rows.source as u32,
            (rows.target >> 32) as u32,
            rows.target as u32,
            rows.source_pitch,
            rows.target_pitch,
            rows.width,
            rows.count,
            header(METHOD_SET_SEMAPHORE, 3),
            (done >> 32) as u32,
            done as u32,
            self.sequence,
        ]
    }

    /// The batch for a rectangle between linear surfaces.
    pub fn rows(&mut self, rows: &Rows) -> Vec<u32> {
        let mut words = self.head(rows);
        words.extend([increasing(COPY_SUBCHANNEL, METHOD_LAUNCH, 1), LAUNCH_PITCH_LINES]);
        words
    }

    /// The batch for a rectangle out of a tiled surface, which the engine detiles by itself. `None`
    /// if a number wouldn't fit its field, as it would change the one beside it.
    pub fn tiled(&mut self, tiled: &Tiled) -> Option<Vec<u32>> {
        if tiled.block >= BLOCK_LIMIT || !origin_fits(u64::from(tiled.x), u64::from(tiled.y)) {
            return None;
        }
        let mut words = self.head(&tiled.rows);
        words.extend([
            increasing(COPY_SUBCHANNEL, METHOD_SET_SOURCE_BLOCK_SIZE, 6),
            tiled.block << BLOCK_HEIGHT_SHIFT | BLOCK_GOB_HEIGHT_8,
            tiled.surface_width,
            tiled.surface_height,
            1,
            0,
            tiled.x | tiled.y << ORIGIN_Y_SHIFT,
            increasing(COPY_SUBCHANNEL, METHOD_LAUNCH, 1),
            LAUNCH_BLOCK_TO_PITCH_LINES,
        ]);
        Some(words)
    }

    /// The batch for plain bytes between GPU addresses.
    pub fn bytes(&mut self, source: u64, target: u64, length: u32) -> Vec<u32> {
        let rows = Rows { source, target, source_pitch: 0, target_pitch: 0, width: length, count: 1 };
        let mut words = self.head(&rows);
        words.extend([increasing(COPY_SUBCHANNEL, METHOD_LAUNCH, 1), LAUNCH_PITCH_ONE_LINE]);
        words
    }

    /// The batch that drops host memory from the L2 and then says so, for when it can't go in order
    /// ahead of a submission.
    pub fn l2_invalidate(&mut self) -> Vec<u32> {
        self.sequence += 1;
        let done = self.channel.area_address() + SEMAPHORE as u64;
        let mut words = l2_invalidate_words();
        words.extend([
            increasing(COPY_SUBCHANNEL, METHOD_SET_SEMAPHORE, 3),
            (done >> 32) as u32,
            done as u32,
            self.sequence,
            increasing(COPY_SUBCHANNEL, METHOD_LAUNCH, 1),
            LAUNCH_SEMAPHORE_ONLY,
        ]);
        words
    }

    /// Sends a batch to the GPU.
    pub fn start<R: Rm>(&mut self, device: &mut Device<R>, words: Vec<u32>) {
        let area = self.channel.area();
        area.store_words(PUSH, &words);
        let entry = ring_entry(self.channel.area_address() + PUSH as u64, words.len() as u32);
        area.store_u64(self.put as usize * ENTRY_SIZE, entry);
        self.put = (self.put + 1) % RING_ENTRIES;
        self.channel.advance(device, self.put);
        self.started = Some(Instant::now());
        self.last_words = words;
    }

    /// Does what the engine's semaphore would, as a fake has no engine.
    #[cfg(any(test, feature = "fake-rm"))]
    pub fn pretend_finished(&self) {
        self.channel.area().store(SEMAPHORE, self.sequence);
    }

    pub fn is_finished(&self) -> bool {
        self.channel.area().load(SEMAPHORE) == self.sequence
    }

    /// What the channel looks like, for the log when a copy goes wrong.
    pub fn state(&self) -> String {
        let area = self.channel.area();
        format!(
            "wanted semaphore {}, it says {}, ring put {}, GPU at {}",
            self.sequence,
            area.load(SEMAPHORE),
            self.put,
            self.channel.gp_get()
        )
    }

    /// Whether the copy under way has outlived its time.
    pub fn has_timed_out(&self, now: Instant) -> bool {
        self.started.is_some_and(|started| now.saturating_duration_since(started) > COPY_TIMEOUT)
    }

    /// Gives a batch a place in this channel's sequence, for one carried over from a dead channel.
    pub fn renumber(&mut self, words: &mut [u32]) {
        const SEQUENCE_WORD: usize = 14;
        self.sequence = self.sequence.wrapping_add(1);
        words[SEQUENCE_WORD] = self.sequence;
        let done = self.channel.area_address() + SEMAPHORE as u64;
        words[SEQUENCE_WORD - 2] = (done >> 32) as u32;
        words[SEQUENCE_WORD - 1] = done as u32;
    }

    /// Spins until the copy ends. Only for what can't go on without it, and only for a moment.
    pub fn wait(&self) -> bool {
        while !self.is_finished() {
            if self.has_timed_out(Instant::now()) {
                return false;
            }
            std::hint::spin_loop();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{AREA_SIZE, ChannelPlace, Engine};
    use crate::profile::{BLACKWELL, HOST_615};
    use crate::rm::fake::FakeRm;

    const AREA_ADDRESS: u64 = 0x800_0000;

    fn copier() -> (Device<FakeRm>, CopyChannel) {
        let mut device = Device::open(FakeRm::new(), &HOST_615).unwrap();
        let space = device.create_address_space(&[(0x400_0000, 0x1_0000_0000)]).unwrap();
        let place = ChannelPlace {
            ring_address: AREA_ADDRESS,
            ring_entries: RING_ENTRIES,
            area_address: AREA_ADDRESS,
            area_size: AREA_SIZE,
        };
        let channel = Channel::new(&mut device, &BLACKWELL, &HOST_615, &space, place, Engine::Copy).unwrap();
        (device, CopyChannel::new(channel))
    }

    #[test]
    fn the_sequence_comes_round_after_its_last_number() {
        let (_device, mut copier) = copier();
        copier.sequence = u32::MAX;
        assert_eq!(copier.rows(&rows())[14], 0);
    }

    fn rows() -> Rows {
        Rows {
            source: 0x1_0000_0000,
            target: 0x2000,
            source_pitch: 256,
            target_pitch: 512,
            width: 64,
            count: 8,
        }
    }

    #[test]
    fn a_linear_copy_ends_with_a_launch() {
        let (_, mut copier) = copier();
        let words = copier.rows(&rows());
        assert_eq!(words.len(), 17);
        assert_eq!(words[3..5], [1, 0]);
        assert_eq!(words[16], LAUNCH_PITCH_LINES);
    }

    #[test]
    fn a_tiled_copy_describes_the_source_block() {
        let (_, mut copier) = copier();
        let tiled = Tiled { rows: rows(), x: 4, y: 2, surface_width: 1024, surface_height: 768, block: 3 };
        let words = copier.tiled(&tiled).unwrap();
        assert_eq!(words[16], 3 << 4 | 1 << 12);
        assert_eq!(words[21], 4 | 2 << 16);
        assert_eq!(*words.last().unwrap(), LAUNCH_BLOCK_TO_PITCH_LINES);
    }

    #[test]
    fn a_number_past_its_field_makes_no_batch() {
        let (_, mut copier) = copier();
        let tiled = Tiled { rows: rows(), x: 4, y: 2, surface_width: 1024, surface_height: 768, block: 3 };
        for bad in
            [Tiled { block: 6, ..tiled }, Tiled { x: 0x1_0000, ..tiled }, Tiled { y: 0x1_0000, ..tiled }]
        {
            assert_eq!(copier.tiled(&bad), None, "{bad:?}");
        }
        assert_eq!(copier.sequence, 0, "nothing was started");
    }

    #[test]
    fn a_copy_is_finished_when_its_semaphore_says_so() {
        let (mut device, mut copier) = copier();
        let words = copier.bytes(0x1000, 0x2000, 64);
        copier.start(&mut device, words);
        assert!(!copier.is_finished());
        copier.channel.area().store(SEMAPHORE, 1);
        assert!(copier.is_finished());
        assert_eq!(device.rm.doorbells().len(), 1);
    }

    #[test]
    fn a_stuck_copy_times_out() {
        let (mut device, mut copier) = copier();
        let words = copier.bytes(0x1000, 0x2000, 64);
        copier.start(&mut device, words);
        assert!(!copier.has_timed_out(Instant::now()));
        assert!(copier.has_timed_out(Instant::now() + COPY_TIMEOUT + Duration::from_secs(1)));
    }
}
