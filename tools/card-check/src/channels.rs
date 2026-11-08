//! The channel test: a graphics channel, a copy channel and, where this generation runs video, one
//! on each video engine. Each releases a semaphore into host memory, and is freed. Nothing from any
//! guest is involved. This is the first thing a VM would do on the card, and the first thing that
//! could go wrong on a card nobody has tried.

use crate::host::{ListedCard, same_card};
use crate::survey::Findings;
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

/// Told the label of each channel just before it's tried, so that something on disk says which one
/// the card stopped on if it stops. If the telling fails, the channel isn't tried.
pub type Progress<'a> = &'a mut dyn FnMut(&str) -> Result<(), String>;

/// Runs every channel in turn, graphics first. `wait` is how long each may take.
pub fn run_all<R: Rm>(
    device: &mut Device<R>,
    generation: &Generation,
    sizes: &HostBranch,
    nodes: &[Node],
    wait: Duration,
    progress: Progress<'_>,
) -> Result<Vec<ChannelResult>, String> {
    let failed = |error: RmError| error.to_string();
    let space = device.create_address_space(&[RANGE]).map_err(failed)?;
    let mut engines = vec![Engine::Graphics, Engine::Copy];
    engines.extend(video_engines(generation, nodes));
    let mut results = Vec::with_capacity(engines.len());
    for (index, engine) in engines.into_iter().enumerate() {
        let label = label(engine);
        progress(&label)?;
        let area_address = FIRST_AREA + index as u64 * AREA_STEP;
        let outcome = run_one(device, generation, sizes, &space, area_address, engine, wait);
        results.push(ChannelResult { label, outcome });
    }
    device.destroy_address_space(&space).map_err(failed)?;
    Ok(results)
}

/// Why the channel test can't start on this card, if it can't. The card is found by its PCI address
/// in the driver's list, and the device opened there says its own name: if the two differ, the
/// channels could reach another card, so none is made.
pub fn refusal(listed: &ListedCard, findings: &Findings) -> Option<String> {
    if !same_card(&listed.model, &findings.name) {
        return Some(format!(
            "The card at {} opened as {:?}, but the driver lists {:?} there. They don't line up on this \
             PC, so the channel test was not run: it could reach another card.",
            listed.address, findings.name, listed.model
        ));
    }
    if findings.generation.is_none() {
        return Some(
            "No channel test: this build has no classes for a card it has no profile for.".to_owned(),
        );
    }
    None
}

/// The channel test on the card the listing names, and only if that is the card that was opened.
/// Nothing here touches the device before [`refusal`] has passed it.
pub fn run_on_listed_card<R: Rm>(
    device: &mut Device<R>,
    listed: &ListedCard,
    findings: &Findings,
    sizes: &HostBranch,
    wait: Duration,
    progress: Progress<'_>,
) -> Result<Vec<ChannelResult>, String> {
    if let Some(why) = refusal(listed, findings) {
        return Err(why);
    }
    let generation = findings.generation.expect("refusal checks for one");
    run_all(device, generation, sizes, &findings.nodes, wait, progress)
}
