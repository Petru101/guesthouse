//! `fake_channels`: makes each kind of channel against the pretend RM, and prints every RM call and
//! every word left in the channels' memory, to compare with another implementation of the same rules.

use guesthouse_backend_nvidia::channel::copy::{RING_ENTRIES, Rows, Tiled};
use guesthouse_backend_nvidia::channel::{
    AREA_SIZE, Area, Channel, ChannelPlace, CopyChannel, Engine, LIST_AREA, ListChannel,
};
use guesthouse_backend_nvidia::profile::{BLACKWELL, HOST_615};
use guesthouse_backend_nvidia::rm::device::{AddressSpace, Device};
use guesthouse_backend_nvidia::rm::fake::{Call, FakeRm};

const RANGE: (u64, u64) = (0x400_0000, 0x1_0000_0000);
const LIVE_AREA: u64 = 0x800_0000;
const LIST_AREA_ADDRESS: u64 = 0x900_0000;
const COPY_AREA: u64 = 0xC00_0000;
const FENCE: u64 = 0x4_0000;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn print_call(call: &Call) {
    match call {
        Call::Alloc { parent, handle, class, params } => {
            println!("alloc {parent:#x} {handle:#x} {class:#x} {}", hex(params));
        }
        Call::Control { object, command, params } => {
            println!("control {object:#x} {command:#x} {}", hex(params))
        }
        Call::HostPages { handle, size } => println!("pages {handle:#x} {size:#x}"),
        Call::MapDma(map) => println!(
            "map {:#x} {:#x} {:#x} {:#x} {:#x} {} {}",
            map.range, map.memory, map.offset, map.length, map.address, map.small_pages, map.gpu_cached
        ),
        Call::Free { handle, .. } => println!("free {handle:#x}"),
        Call::UnmapDma { range, memory, address, length } => {
            println!("unmap {range:#x} {memory:#x} {address:#x} {length:#x}");
        }
        Call::ExportDmabuf { memory, size } => println!("export {memory:#x} {size:#x}"),
        Call::MapDoorbell => println!("doorbell-map"),
        Call::Doorbell { .. } => println!("doorbell"),
    }
}

fn print_area(name: &str, area: &Area) {
    for at in (0..area.size()).step_by(4) {
        let word = area.load(at);
        if word != 0 {
            println!("{name} {at:#x} {word:#x}");
        }
    }
}

fn channel(
    device: &mut Device<FakeRm>,
    space: &AddressSpace,
    place: ChannelPlace,
    engine: Engine,
) -> Channel {
    Channel::new(device, &BLACKWELL, &HOST_615, space, place, engine).unwrap()
}

fn main() {
    let mut device = Device::open(FakeRm::new(), &HOST_615).unwrap();
    device.rm.calls.clear();
    let space = device.create_address_space(&[RANGE]).unwrap();

    let place = ChannelPlace {
        ring_address: 0xA00_0000,
        ring_entries: 0x2000,
        area_address: LIVE_AREA,
        area_size: AREA_SIZE,
    };
    let live = channel(&mut device, &space, place, Engine::Graphics);
    live.advance(&mut device, 0x2005);
    println!("prelude {:#x}", live.prelude_entry());

    for (engine, area_address) in
        [(Engine::Graphics, LIST_AREA_ADDRESS), (Engine::Copy, LIST_AREA_ADDRESS + (8 << 20))]
    {
        let place = ChannelPlace {
            ring_address: ListChannel::ring_address(area_address),
            ring_entries: ListChannel::RING_ENTRIES,
            area_address,
            area_size: LIST_AREA,
        };
        let mut list = ListChannel::new(channel(&mut device, &space, place, engine));
        let end = list.add(&[0x11; 16], FENCE, 7, 0).unwrap();
        let end = list.add(&[0x22; 8], FENCE, 8, 0).unwrap().max(end);
        list.channel.advance(&mut device, end);
        println!("end {end}");
        print_area("list", list.channel.area());
    }

    let place = ChannelPlace {
        ring_address: COPY_AREA,
        ring_entries: RING_ENTRIES,
        area_address: COPY_AREA,
        area_size: AREA_SIZE,
    };
    let mut copier = CopyChannel::new(channel(&mut device, &space, place, Engine::Copy));
    let rows = Rows {
        source: 0x1_2345_0000,
        target: 0x2_0000,
        source_pitch: 256,
        target_pitch: 512,
        width: 64,
        count: 8,
    };
    let batch = copier.rows(&rows);
    copier.start(&mut device, batch);
    let tiled = Tiled {
        rows: Rows { source_pitch: 0, ..rows },
        x: 16,
        y: 4,
        surface_width: 4096,
        surface_height: 768,
        block: 3,
    };
    let batch = copier.tiled(&tiled);
    copier.start(&mut device, batch);
    let batch = copier.bytes(0x3_0000, 0x4_0000, 0x1000);
    copier.start(&mut device, batch);

    print_area("live", live.area());
    print_area("copy", copier.channel.area());
    live.close(&mut device, &space).unwrap();
    device.rm.calls.iter().for_each(print_call);
}
