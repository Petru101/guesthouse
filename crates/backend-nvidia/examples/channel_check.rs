//! `channel_check VERSION`: makes a graphics channel, a copy channel and one on each video engine the
//! card takes on the real card, has each release a semaphore into host memory, and frees them.
//! Nothing from any guest is involved. The classes are those of the card's generation, and a card with
//! no profile is refused. Only a generation with video nodes tries its video engines.

use guesthouse_backend_nvidia::card_generation;
use guesthouse_backend_nvidia::channel::{AREA_SIZE, Channel, ChannelPlace, Engine, VideoEngine};
use guesthouse_backend_nvidia::profile::{BLACKWELL, Generation, HOST_615};
use guesthouse_backend_nvidia::pushbuffer::{increasing, ring_entry};
use guesthouse_backend_nvidia::record::CardReader;
use guesthouse_backend_nvidia::rm::device::{AddressSpace, Device};
use guesthouse_backend_nvidia::rm::linux::LinuxRm;
use std::env;
use std::time::{Duration, Instant};

const RANGE: (u64, u64) = (0x400_0000, 0x1_0000_0000);
const RING_ENTRIES: u32 = 1024;
const PUSH: usize = 0x12000;
const RESULT: usize = 0x14000;
const SEMAPHORE_ADDRESS_LOW: u32 = 0x5C;
const SEMAPHORE_RELEASE_32: u32 = 1;
const VALUE: u32 = 0xCAFE_1234;
/// The tested card's engines, as the RM's number and which instance that is, from its public headers.
const RM_DECODERS: &[(u32, u32)] = &[(0x13, 0), (0x16, 3)];
const RM_ENCODERS: &[(u32, u32)] = &[(0x1C, 1), (0x1D, 2), (0x3F, 3)];
const WAIT: Duration = Duration::from_secs(3);

fn run(
    device: &mut Device<LinuxRm>,
    generation: &Generation,
    space: &AddressSpace,
    area_address: u64,
    engine: Engine,
) -> bool {
    let place = ChannelPlace {
        ring_address: area_address,
        ring_entries: RING_ENTRIES,
        area_address,
        area_size: AREA_SIZE,
    };
    let channel = match Channel::new(device, generation, &HOST_615, space, place, engine) {
        Ok(channel) => channel,
        Err(error) => {
            println!("{engine:?}: no channel: {error}");
            return false;
        }
    };
    println!("{engine:?}: token {:#x}", channel.token());
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
    channel.advance(device, 2);
    let started = Instant::now();
    while area.load(RESULT) != VALUE && started.elapsed() < WAIT {
        std::hint::spin_loop();
    }
    let done = area.load(RESULT) == VALUE;
    println!("{engine:?}: {} after {:?}", if done { "released" } else { "NOT RELEASED" }, started.elapsed());
    channel.close(device, space).unwrap();
    done
}

fn main() {
    let version = env::args().nth(1).expect("usage: channel_check VERSION");
    let rm = LinuxRm::open(&version, &HOST_615).unwrap();
    let mut device = Device::open(rm, &HOST_615).unwrap();
    let generation = card_generation(&mut CardReader::new(&mut device)).map_err(|refusal| refusal.0).unwrap();
    println!("{} card", generation.name);
    let space = device.create_address_space(&[RANGE]).unwrap();
    let graphics = run(&mut device, generation, &space, 0x800_0000, Engine::Graphics);
    let copy = run(&mut device, generation, &space, 0x900_0000, Engine::Copy);
    if generation == &BLACKWELL {
        let mut video_area = 0xA00_0000;
        for (engines, class) in
            [(RM_DECODERS, generation.video_decode_class), (RM_ENCODERS, generation.video_encode_class)]
        {
            for &(rm_engine, instance) in engines {
                let engine = Engine::Video(VideoEngine { rm_engine, class, instance });
                run(&mut device, generation, &space, video_area, engine);
                video_area += 0x100_0000;
            }
        }
    }
    device.destroy_address_space(&space).unwrap();
    std::process::exit(if graphics && copy { 0 } else { 1 });
}
