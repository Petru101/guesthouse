//! Opening the adapter, and what the guest asks about it.

use crate::wire::{
    Malformed, clamped, declared_size, pad_to, put_u32, put_u64, put_utf16_field, set_u32, set_u64, u32_at,
    u64_at,
};
use crate::{HEADER_SIZE, Status};

// The host's side of the interface: the handle it gives the adapter and the identifiers the guest
// keeps. Any value would do for the handle and the adapter id, as only this host reads them back.
const HOST_ADAPTER_HANDLE: u32 = 0x4000_0000;
const INTERFACE_VERSION: u32 = 40;
const DRIVER_INTERFACE_VERSION: u32 = 0xF003;
const ADAPTER_ID: (u32, u32) = (0x0000_0ABC, 0x0000_0001);

/// Set in the identity answer to let the guest send submissions, waits and signals without
/// waiting for each reply. Needs interface version 40.
pub const ASYNC_MESSAGES: u32 = 0x40;

const DESCRIPTION_CHARS: usize = 80;
const HARDWARE_ID_CHARS: usize = 260;

/// Answers the guest's offer of an interface version.
pub fn open_adapter(message: &[u8], out: &mut Vec<u8>) -> Result<(), Malformed> {
    let last_compatible = u32_at(message, HEADER_SIZE + 4)?;
    put_u32(out, HOST_ADAPTER_HANDLE);
    put_u32(out, Status::SUCCESS.0);
    put_u32(out, INTERFACE_VERSION);
    put_u32(out, last_compatible);
    Ok(())
}

/// What the guest is told the adapter is.
pub struct IdentityAnswer<'a> {
    pub description: &'a str,
    pub vendor: u16,
    pub device: u16,
    pub flags: u32,
}

pub fn adapter_identity(answer: &IdentityAnswer<'_>, out: &mut Vec<u8>) {
    put_u32(out, 0);
    put_u32(out, 0);
    put_u32(out, DRIVER_INTERFACE_VERSION);
    put_u32(out, answer.flags);
    put_adapter_id(out);
    put_utf16_field(out, answer.description, DESCRIPTION_CHARS);
    let hardware_id = format!("PCI\\VEN_{:04X}&DEV_{:04X}", answer.vendor, answer.device);
    put_utf16_field(out, &hardware_id, HARDWARE_ID_CHARS);
    put_adapter_id(out);
}

fn put_adapter_id(out: &mut Vec<u8>) {
    put_u32(out, ADAPTER_ID.0);
    put_u32(out, ADAPTER_ID.1);
}

/// Where the guest placed the shared IO space.
pub struct SetIoSpace {
    pub start: u64,
    pub length: u64,
}

impl SetIoSpace {
    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self { start: u64_at(message, HEADER_SIZE)?, length: u64_at(message, HEADER_SIZE + 8)? })
    }
}

/// The driver capability queries the guest's scheduler and memory manager make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapsQuery {
    pub kind: u32,
    pub out_size: usize,
}

impl CapsQuery {
    pub const GENERAL: u32 = 1;
    pub const GPU_PAGING: u32 = 13;
    pub const PAGE_TABLE_LEVEL: u32 = 14;
    pub const PHYSICAL_ADAPTER: u32 = 15;

    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self { kind: u32_at(message, HEADER_SIZE)?, out_size: declared_size(message, HEADER_SIZE + 8)? })
    }
}

/// The host's page tables as the guest should picture them. The guest never builds real ones here,
/// but it refuses an adapter that describes none.
pub const GPU_ADDRESS_BITS: u32 = 48;
const PAGE_TABLE_LEVELS: u32 = 4;
const PAGE_TABLE_INDEX_BITS: u32 = 9;
const PAGE_TABLE_BYTES: u32 = 0x1000;
const READ_ONLY_AND_NO_EXECUTE: u32 = 0x3;

const CAPS_HIGHEST_ADDRESS: usize = 0x00;
const CAPS_ALLOCATION_SLOTS: usize = 0x08;
const CAPS_POINTER_SIZE: usize = 0x18;
const CAPS_QUEUED_FLIPS: usize = 0x38;
const CAPS_SCHEDULER: usize = 0x40;
const CAPS_MEMORY_MANAGER: usize = 0x44;
const CAPS_ASYMMETRIC_NODES: usize = 0x48;
const CAPS_DRIVER_MODEL: usize = 0x150;

const SCHEDULER_MULTIPLE_ENGINES: u32 = 1 << 0;
const SCHEDULER_PREEMPTION: u32 = 1 << 2;
const SCHEDULER_NO_COMMAND_PATCHING: u32 = 1 << 3;
const MEMORY_VIRTUAL_ADDRESSING: u32 = 1 << 5;
const MEMORY_GPU_PAGING: u32 = 1 << 6;
const MEMORY_PARAVIRTUAL: u32 = 1 << 10;

const HIGHEST_ACCEPTABLE_ADDRESS: u64 = (1 << 48) - 1;
const ALLOCATION_SLOTS: u32 = 0x10_0000;
const LARGEST_POINTER: u32 = 256;
/// The driver model version as this query spells it.
const DRIVER_MODEL_CAPS: u32 = 0x3000;

/// Writes the answer to `query`. An answer needs room for its fields, so a guest that asks for
/// too few bytes gets `Malformed`.
pub fn driver_caps(query: CapsQuery, node_count: u32, out: &mut Vec<u8>) -> Result<(), Malformed> {
    let start = out.len();
    out.resize(start + query.out_size, 0);
    let answer = &mut out[start..];
    let needed = match query.kind {
        CapsQuery::GENERAL => CAPS_DRIVER_MODEL + 4,
        CapsQuery::GPU_PAGING | CapsQuery::PAGE_TABLE_LEVEL => 20,
        CapsQuery::PHYSICAL_ADAPTER => 8,
        _ => 0,
    };
    if answer.len() < needed {
        return Err(Malformed);
    }
    match query.kind {
        CapsQuery::GENERAL => {
            set_u64(answer, CAPS_HIGHEST_ADDRESS, HIGHEST_ACCEPTABLE_ADDRESS);
            set_u32(answer, CAPS_ALLOCATION_SLOTS, ALLOCATION_SLOTS);
            set_u32(answer, CAPS_POINTER_SIZE, LARGEST_POINTER);
            set_u32(answer, CAPS_POINTER_SIZE + 4, LARGEST_POINTER);
            set_u32(answer, CAPS_QUEUED_FLIPS, 1);
            let scheduler = SCHEDULER_MULTIPLE_ENGINES | SCHEDULER_PREEMPTION | SCHEDULER_NO_COMMAND_PATCHING;
            set_u32(answer, CAPS_SCHEDULER, scheduler);
            let memory = MEMORY_VIRTUAL_ADDRESSING | MEMORY_GPU_PAGING | MEMORY_PARAVIRTUAL;
            set_u32(answer, CAPS_MEMORY_MANAGER, memory);
            set_u32(answer, CAPS_ASYMMETRIC_NODES, 1);
            set_u32(answer, CAPS_DRIVER_MODEL, DRIVER_MODEL_CAPS);
        }
        CapsQuery::GPU_PAGING => {
            set_u32(answer, 0, READ_ONLY_AND_NO_EXECUTE);
            set_u32(answer, 8, GPU_ADDRESS_BITS);
            set_u32(answer, 12, PAGE_TABLE_BYTES);
            set_u32(answer, 16, PAGE_TABLE_LEVELS);
        }
        CapsQuery::PAGE_TABLE_LEVEL => {
            set_u32(answer, 0, PAGE_TABLE_INDEX_BITS);
            set_u32(answer, 12, PAGE_TABLE_BYTES);
            set_u32(answer, 16, PAGE_TABLE_BYTES);
        }
        CapsQuery::PHYSICAL_ADAPTER => set_u32(answer, 0, node_count),
        _ => {}
    }
    Ok(())
}

const NODE_ANSWER_SIZE: usize = 0x50;
const NODE_ENGINE: usize = 0x00;
const NODE_NAME: usize = 0x04;
const NODE_NAME_CHARS: usize = 32;
const NODE_GPU_PAGING: usize = 0x48;
const ENGINE_3D: u32 = 1;
const ENGINE_COPY: u32 = 6;

/// Which node the guest asks about.
pub fn node_of(message: &[u8]) -> Result<u32, Malformed> {
    u32_at(message, HEADER_SIZE)
}

/// Describes an engine node: the first draws and the rest copy. The status closes the answer.
pub fn node_description(node: u32, out: &mut Vec<u8>) {
    let start = out.len();
    out.resize(start + NODE_NAME, 0);
    let (engine, name) = if node == 0 { (ENGINE_3D, "3D") } else { (ENGINE_COPY, "Copy") };
    set_u32(&mut out[start..], NODE_ENGINE, engine);
    put_utf16_field(out, name, NODE_NAME_CHARS);
    pad_to(out, start + NODE_ANSWER_SIZE);
    out[start + NODE_GPU_PAGING] = 1;
}

/// An adapter query: its kind, and the guest's buffer, which also carries any input.
#[derive(Debug, Clone, Copy)]
pub struct AdapterQuery<'a> {
    pub kind: u32,
    pub size: usize,
    pub data: &'a [u8],
}

impl<'a> AdapterQuery<'a> {
    pub const SEGMENT_SIZES: u32 = 3;
    pub const FLIP_QUEUE: u32 = 5;
    pub const CURRENT_DISPLAY_MODE: u32 = 9;
    pub const DRIVER_MODEL: u32 = 13;
    pub const DIRECT_FLIP: u32 = 19;
    pub const CONSOLE_CAPS: u32 = 27;
    pub const PHYSICAL_ADAPTER_COUNT: u32 = 30;
    pub const PHYSICAL_ADAPTER_IDS: u32 = 31;
    pub const REGISTRY: u32 = 48;
    pub const DRIVER_VERSION: u32 = 49;
    pub const DRIVER_MODEL_FOR_RENDER: u32 = 56;
    pub const DESCRIPTION: u32 = 66;
    pub const USER_DRIVER_VERSION: u32 = 18;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let size = declared_size(message, HEADER_SIZE + 4)?;
        Ok(Self { kind: u32_at(message, HEADER_SIZE)?, size, data: clamped(message, HEADER_SIZE + 8, size) })
    }
}

/// What's on the screen, for the current display mode query. Desktop Duplication asks this of the
/// adapter that renders the screen, and a refusal there is an invalid-argument error to the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    /// In Windows' own format numbers, as the desktop has it.
    pub format: u32,
    pub refresh_millihertz: u32,
}

impl DisplayMode {
    /// Windows' format numbers for an ordinary desktop and an HDR one.
    pub const FORMAT_8_BIT: u32 = 21;
    pub const FORMAT_16_BIT_FLOAT: u32 = 113;
    /// The source, size, format, two rates, scan order, rotation, fixed output and flags.
    pub const ANSWER_SIZE: usize = 11 * size_of::<u32>();
    const PROGRESSIVE: u32 = 1;
    const NOT_ROTATED: u32 = 1;
    const MILLIHERTZ: u32 = 1000;

    /// Writes the answer for the source the guest asked about.
    pub fn answer(&self, source: u32, out: &mut Vec<u8>) {
        let rate = self.refresh_millihertz;
        for word in [
            source,
            self.width,
            self.height,
            self.format,
            rate.div_ceil(Self::MILLIHERTZ),
            rate,
            Self::MILLIHERTZ,
            Self::PROGRESSIVE,
            Self::NOT_ROTATED,
            0,
            0,
        ] {
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
}

/// The driver model as adapter queries spell it.
pub const DRIVER_MODEL_NUMBER: u32 = 3000;

/// The card's PCI identity. The subsystem fields repeat the card's own numbers.
pub fn physical_adapter_ids(vendor: u16, device: u16, revision: u8, out: &mut Vec<u8>) {
    let (vendor, device) = (u32::from(vendor), u32::from(device));
    for field in [0, vendor, device, vendor, device, u32::from(revision), 0] {
        put_u32(out, field);
    }
}

/// The flip queue answer's size: hardware queue length, software queue length and flags.
pub const FLIP_QUEUE_ANSWER_SIZE: usize = 3 * size_of::<u32>();
/// The longest software flip queue Windows allows.
const SOFTWARE_FLIP_QUEUE: u32 = 31;

/// What Windows answers for an adapter whose caps claim no flip features, as the caps above don't.
/// Direct3D 9 refuses the adapter without it.
pub fn flip_queue(out: &mut Vec<u8>) {
    for field in [0, SOFTWARE_FLIP_QUEUE, 0] {
        put_u32(out, field);
    }
}

pub fn segment_sizes(dedicated: u64, shared: u64, out: &mut Vec<u8>) {
    put_u64(out, dedicated);
    put_u64(out, 0);
    put_u64(out, shared);
}

/// The adapter's name, as UTF-16 padded to the size of the guest's buffer.
pub fn description_answer(description: &str, size: usize, out: &mut Vec<u8>) {
    let start = out.len();
    for unit in description.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    pad_to(out, start + size);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_display_mode_says_its_size_and_rate() {
        let mode = DisplayMode { width: 2560, height: 1440, format: 21, refresh_millihertz: 143_856 };
        let mut out = Vec::new();
        mode.answer(2, &mut out);
        assert_eq!(out.len(), DisplayMode::ANSWER_SIZE);
        let words: Vec<u32> =
            out.chunks(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
        assert_eq!(words, [2, 2560, 1440, 21, 144, 143_856, 1000, 1, 1, 0, 0]);
    }

    #[test]
    fn open_adapter_repeats_the_oldest_version() {
        let mut message = vec![0; HEADER_SIZE + 8];
        message[HEADER_SIZE + 4] = 7;
        let mut out = Vec::new();
        open_adapter(&message, &mut out).unwrap();
        assert_eq!(out.len(), 16);
        assert_eq!(u32_at(&out, 8), Ok(INTERFACE_VERSION));
        assert_eq!(u32_at(&out, 12), Ok(7));
    }

    #[test]
    fn identity_has_fixed_text_fields() {
        let mut out = Vec::new();
        let answer = IdentityAnswer { description: "Card", vendor: 0x1234, device: 0xABCD, flags: 0 };
        adapter_identity(&answer, &mut out);
        assert_eq!(out.len(), 16 + 8 + 160 + 520 + 8);
        let id = crate::wire::utf16_until_zero(&out[184..]);
        assert_eq!(id, "PCI\\VEN_1234&DEV_ABCD");
    }

    #[test]
    fn the_flip_queue_is_software_only() {
        let mut out = Vec::new();
        flip_queue(&mut out);
        assert_eq!(out.len(), FLIP_QUEUE_ANSWER_SIZE);
        assert_eq!((u32_at(&out, 0), u32_at(&out, 4), u32_at(&out, 8)), (Ok(0), Ok(31), Ok(0)));
    }

    #[test]
    fn caps_need_room_for_their_fields() {
        let query = CapsQuery { kind: CapsQuery::GENERAL, out_size: 0x10 };
        assert_eq!(driver_caps(query, 8, &mut Vec::new()), Err(Malformed));
    }

    #[test]
    fn unknown_caps_are_zeros_of_the_size_asked() {
        let mut out = Vec::new();
        driver_caps(CapsQuery { kind: 99, out_size: 12 }, 8, &mut out).unwrap();
        assert_eq!(out, [0; 12]);
    }

    #[test]
    fn node_zero_draws_and_the_rest_copy() {
        let (mut first, mut second) = (Vec::new(), Vec::new());
        node_description(0, &mut first);
        node_description(4, &mut second);
        assert_eq!(first.len(), NODE_ANSWER_SIZE);
        assert_eq!(u32_at(&first, 0), Ok(ENGINE_3D));
        assert_eq!(u32_at(&second, 0), Ok(ENGINE_COPY));
        assert_eq!(crate::wire::utf16_until_zero(&second[NODE_NAME..]), "Copy");
        assert_eq!(second[NODE_GPU_PAGING], 1);
    }
}
