//! The channel test: a graphics channel, a copy channel and, where this generation runs video, one
//! on each video engine. Each releases a semaphore into host memory, and is freed. Nothing from any
//! guest is involved. This is the first thing a VM would do on the card, and the first thing that
//! could go wrong on a card nobody has tried.

use guesthouse_backend_nvidia::answers::nodes::{ENGINE_VIDEO_ENCODE, Node};
use guesthouse_backend_nvidia::channel::{AREA_SIZE, Channel, ChannelPlace, Engine, VideoEngine};
use guesthouse_backend_nvidia::profile::{Generation, HostBranch};
use guesthouse_backend_nvidia::pushbuffer::{increasing, ring_entry};
use guesthouse_backend_nvidia::rm::Rm;
use guesthouse_backend_nvidia::rm::RmError;
use guesthouse_backend_nvidia::rm::device::{AddressSpace, Device};
use std::time::{Duration, Instant};

/// The GPU addresses this program's own address space covers.
const RANGE: (u64, u64) = (0x400_0000, 0x1_0000_0000);
const RING_ENTRIES: u32 = 1024;
/// Where in each channel's area its commands and their result go.
const PUSH: usize = 0x12000;
const RESULT: usize = 0x14000;
/// Each channel gets an area of its own, this far from the last.
const FIRST_AREA: u64 = 0x800_0000;
const AREA_STEP: u64 = 0x100_0000;
/// The channel's own semaphore methods, which every engine here takes.
const SEMAPHORE_ADDRESS_LOW: u32 = 0x5C;
const SEMAPHORE_RELEASE_32: u32 = 1;
const VALUE: u32 = 0xCAFE_1234;
/// Two ring entries: the prelude that binds the classes, then the release.
const ENTRIES_USED: u32 = 2;

/// How long to wait for one channel before calling it stuck.
pub const WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Released(Duration),
    NotReleased,
    NoChannel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelResult {
    pub label: String,
    pub outcome: Outcome,
}

impl ChannelResult {
    pub fn released(&self) -> bool {
        matches!(self.outcome, Outcome::Released(_))
    }
}

fn label(engine: Engine) -> String {
    match engine {
        Engine::Graphics => "graphics".to_owned(),
        Engine::Copy => "copy".to_owned(),
        Engine::Video(video) => format!("video engine {:#x} (class {:#X})", video.rm_engine, video.class),
    }
}

fn run_one<R: Rm>(
    device: &mut Device<R>,
    generation: &Generation,
    sizes: &HostBranch,
    space: &AddressSpace,
    area_address: u64,
    engine: Engine,
    wait: Duration,
) -> Outcome {
    let place = ChannelPlace {
        ring_address: area_address,
        ring_entries: RING_ENTRIES,
        area_address,
        area_size: AREA_SIZE,
    };
    let channel = match Channel::new(device, generation, sizes, space, place, engine) {
        Ok(channel) => channel,
        Err(error) => return Outcome::NoChannel(error.to_string()),
    };
    let target = area_address + RESULT as u64;
    let words = [
        increasing(0, SEMAPHORE_ADDRESS_LOW, 5),
        target as u32,
        (target >> 32) as u32,
        VALUE,
        0,
        SEMAPHORE_RELEASE_32,
    ];
    let area = channel.area();
    area.store_words(PUSH, &words);
    area.store_u64(0, channel.prelude_entry());
    area.store_u64(8, ring_entry(area_address + PUSH as u64, words.len() as u32));
    channel.advance(device, ENTRIES_USED);
    let started = Instant::now();
    while area.load(RESULT) != VALUE && started.elapsed() < wait {
        std::hint::spin_loop();
    }
    let outcome =
        if area.load(RESULT) == VALUE { Outcome::Released(started.elapsed()) } else { Outcome::NotReleased };
    match channel.close(device, space) {
        Ok(()) => outcome,
        Err(error) => Outcome::NoChannel(format!("ran, but closing it failed: {error}")),
    }
}

/// The video engines the guest's nodes would run on, which only a generation whose video runs has.
fn video_engines(generation: &Generation, nodes: &[Node]) -> Vec<Engine> {
    nodes
        .iter()
        .filter_map(|node| {
            let (rm_engine, instance) = node.video?;
            let encodes = node.engine == ENGINE_VIDEO_ENCODE;
            let class = if encodes { generation.video_encode_class } else { generation.video_decode_class };
            Some(Engine::Video(VideoEngine { rm_engine, class, instance }))
        })
        .collect()
}

/// Runs every channel in turn, graphics first. `wait` is how long each may take.
pub fn run_all<R: Rm>(
    device: &mut Device<R>,
    generation: &Generation,
    sizes: &HostBranch,
    nodes: &[Node],
    wait: Duration,
) -> Result<Vec<ChannelResult>, RmError> {
    let space = device.create_address_space(&[RANGE])?;
    let mut engines = vec![Engine::Graphics, Engine::Copy];
    engines.extend(video_engines(generation, nodes));
    let mut results = Vec::with_capacity(engines.len());
    for (index, engine) in engines.into_iter().enumerate() {
        let area_address = FIRST_AREA + index as u64 * AREA_STEP;
        let outcome = run_one(device, generation, sizes, &space, area_address, engine, wait);
        results.push(ChannelResult { label: label(engine), outcome });
    }
    device.destroy_address_space(&space)?;
    Ok(results)
}
