//! A backend on the pretend RM, and the messages a guest's screen is made of, for tests and examples
//! in other crates. Everything in it is made up, and nothing goes near a card.

use crate::NvidiaBackend;
use crate::profile::{GUEST_617, HOST_615};
use crate::record::HostFacts;
use crate::rm::device::Device;
use crate::rm::fake::FakeRm;
use crate::rm::fake_card::{FAKE_BLACKWELL, FakeCard, install_card};
use guesthouse_backend::{DriverVersion, PciCard, Refusal, Setup};
use guesthouse_messages::allocation::PRIMARY;
use guesthouse_messages::wire::{set_u32, set_u64, u32_at};
use guesthouse_messages::{Command, HEADER_SIZE};

/// The guest process these messages come from.
pub const PROCESS: u32 = 0x4000_2000;
const REPLY_FIRST_HANDLE: usize = 0x10;
/// A reply's records follow one another at this distance.
const REPLY_HANDLE_STEP: usize = 0x48;
/// Any length with room for every field does.
const REQUEST_LENGTH: usize = 0x200;

fn inf(device: u32) -> String {
    let names = [
        "DXCoreAttributes",
        "VulkanDriverName",
        "VulkanDriverNameWoW",
        "VulkanImplicitLayers",
        "VulkanImplicitLayersWow",
        "OpenGLDriverName",
        "OpenGLDriverNameWoW",
        "OpenGLVersion",
        "OpenGLVersionWoW",
        "OpenGLFlags",
        "OpenGLFlagsWoW",
        "OpenCLDriverName",
        "OpenCLDriverNameWow",
    ];
    let mut text = format!(
        "[NVIDIA_Devices.NTamd64.10.0]\n%A% = Sec, PCI\\VEN_10DE&DEV_{device:04X}\n[sec]\nAddReg = vals\n[vals]\n"
    );
    for name in names {
        text.push_str(&format!("HKR,,{name},%REG_SZ%,\"%13%\\file\"\n"));
    }
    text.push_str("[services]\nServiceBinary = %13%\\fake.sys\n");
    text
}

/// A started backend on a pretend card.
pub fn fake_backend(rm: FakeRm) -> NvidiaBackend<FakeRm> {
    let started = start_on_fake_card(rm, FAKE_BLACKWELL.device as u16);
    started.map_err(|refusal| refusal.0).expect("the pretend card passes")
}

/// Starts a backend with setup having chosen this card, which may not be the pretend one.
pub fn start_on_fake_card(rm: FakeRm, chosen: u16) -> Result<NvidiaBackend<FakeRm>, Refusal> {
    start_on_card(rm, &FAKE_BLACKWELL, chosen)
}

/// Starts a backend on this pretend card, with setup having chosen `chosen`.
pub fn start_on_card(mut rm: FakeRm, card: &FakeCard, chosen: u16) -> Result<NvidiaBackend<FakeRm>, Refusal> {
    install_card(&mut rm, card);
    let device = Device::open(rm, &HOST_615).expect("the pretend RM opens");
    let setup = Setup {
        guest_driver_version: DriverVersion::parse("32.0.16.1742").expect("a version"),
        guest_branch: "617".into(),
        driver_folder: Default::default(),
        card: Some(PciCard { vendor: 0x10DE, device: chosen, revision: 0xA1 }),
        card_address: None,
    };
    let host = HostFacts {
        total_ram_kib: 64 << 20,
        has_avx: true,
        cpu_megahertz: 4300,
        rated_clocks: (3_000_000_000, 14_000_000_000),
    };
    NvidiaBackend::start(device, &HOST_615, &GUEST_617, &setup, &host, &[inf(card.device).into_bytes()])
}

/// A message with room for `size` bytes, and its command and process filled in.
pub fn message(command: Command, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size.max(HEADER_SIZE)];
    set_u32(&mut bytes, 8, PROCESS);
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

pub fn set_io_space(window: u64) -> Vec<u8> {
    let mut request = message(Command::SetIoSpace, HEADER_SIZE + 16);
    set_u64(&mut request, HEADER_SIZE, window);
    request
}

/// An allocation request with nothing in it yet, in the driver's own kind of private data.
pub fn blank_request() -> Vec<u8> {
    let layout = &GUEST_617.allocation;
    let mut request = vec![0; REQUEST_LENGTH];
    request[..4].copy_from_slice(layout.magic);
    set_u32(&mut request, layout.own_length, REQUEST_LENGTH as u32);
    request
}

/// Makes two surfaces of one size, in the driver's own kind of private data, as the screen's image
/// and its staging surface are. The image is a primary, as DWM's are.
pub fn create_surfaces(width: u32, height: u32) -> Vec<u8> {
    create_coloured_surfaces(width, height, 0)
}

/// The same, with the guest driver's word for their colour format.
pub fn create_coloured_surfaces(width: u32, height: u32, colour: u8) -> Vec<u8> {
    let layout = &GUEST_617.allocation;
    let mut private = blank_request();
    private[layout.colour] = colour;
    set_u32(&mut private, layout.size, width * height * 4);
    set_u32(&mut private, layout.width, width);
    set_u32(&mut private, layout.height, height);
    set_u32(&mut private, layout.pitch, width * 4);
    let mut create = message(Command::CreateAllocation, 0x40);
    set_u32(&mut create, 0x28, 2);
    for flags in [PRIMARY, 0] {
        create.extend_from_slice(&flags.to_le_bytes());
        create.extend_from_slice(&(private.len() as u32).to_le_bytes());
        create.extend_from_slice(&[0; 4]);
    }
    create.extend_from_slice(&private);
    create.extend_from_slice(&private);
    create
}

/// The two handles from the answer to [`create_surfaces`].
pub fn surface_handles(reply: &[u8]) -> (u32, u32) {
    let handle = |at| u32_at(reply, at).expect("a reply with two allocations");
    (handle(REPLY_FIRST_HANDLE), handle(REPLY_FIRST_HANDLE + REPLY_HANDLE_STEP))
}

/// What puts a surface in the shared window, which makes it a screen's staging surface.
pub fn lock_surface(handle: u32) -> Vec<u8> {
    let mut lock = message(Command::LockForCpu, 0x20);
    set_u32(&mut lock, 0x1C, handle);
    lock
}

/// A present: a whole image to the staging surface.
pub fn copy_to_staging(source: u32, target: u32, width: u32, height: u32) -> Vec<u8> {
    let mut copy = message(Command::CopyToStaging, 0x50);
    set_u32(&mut copy, 0x20, source);
    set_u32(&mut copy, 0x24, target);
    set_u32(&mut copy, 0x30, width);
    set_u32(&mut copy, 0x34, height);
    copy
}
