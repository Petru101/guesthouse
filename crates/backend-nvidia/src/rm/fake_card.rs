//! A pretend card for [`FakeRm`]: answers that look like a card's, made up from each control's number
//! so that nothing about a real card is in them.

use super::Rm;
use super::fake::FakeRm;
use crate::record::card::{
    ARCHITECTURE_INFO, BUS_INFO, CHIPSET_INFO, CLASS_LIST, COPY_ENGINE_MASK, CPU_INFO, Control, DMA_CAPS,
    ENGINE_PARTNERS, ENGINES, FIFO_CAPS, FRAMEBUFFER_CAPS, FRAMEBUFFER_INFO, GPC_MASK, GPU_INFO,
    GRAPHICS_CAPS, GRAPHICS_INFO, JPEG_CAPS, NAME, PCI_INFO, SHORT_NAME, TPC_MASK, UUID, VIDEO_DECODE_CAPS,
    ZCULL_INFO,
};
use crate::rm::classes::{CONTROL_GET_GPU_ID, CONTROL_GET_TIME};

pub const FAKE_NAME: &str = "Fake Card 1";
pub const FAKE_SHORT_NAME: &str = "FK1";
pub const FAKE_DEVICE: u32 = 0x1234;
pub const FAKE_VENDOR: u32 = 0x10DE;
pub const FAKE_GPU_ID: u32 = 0x300;
/// A card's memory, in KiB.
pub const FAKE_RAM_KIB: u32 = 20 << 20;
pub const FAKE_ENGINES: [u32; 9] = [0x01, 0x0B, 0x0C, 0x0F, 0x13, 0x1C, 0x1D, 0x26, 0x2E];
pub const FAKE_CLASSES: [u32; 4] = [0xCA6F, 0xC96F, 0xCE97, 0xCAB5];
pub const FAKE_GRAPHICS_COPY: u32 = 0x0C;
const FAKE_GPC_COUNT: u32 = 4;
const JPEG_ENGINES_PRESENT: u32 = 2;

const FB_RAM: u32 = 7;
const FB_BAR1: u32 = 5;
const GRAPHICS_GPC_COUNT: u32 = 0x14;
const GRAPHICS_SM_VERSION: u32 = 0x0C;
const STATUS_NO_SUCH_ENGINE: u32 = 0x1F;
const ARCHITECTURE_IMPLEMENTATION_FIELD: usize = 4;
/// Classes past the named ones start here, below any class this backend looks for.
const FIRST_FILLER_CLASS: u32 = 0x1000;

/// What makes one pretend card differ from another.
#[derive(Debug, Clone, Copy)]
pub struct FakeCard {
    pub name: &'static str,
    pub device: u32,
    pub architecture: u32,
    /// `None` leaves it as made-up bytes.
    pub implementation: Option<u32>,
    pub sm_version: u32,
    pub engines: &'static [u32],
    pub graphics_copy: u32,
    pub classes: &'static [u32],
    /// How many classes the card lists. Filler classes make up what `classes` doesn't name.
    pub class_count: usize,
}

/// The pretend card most tests use.
pub const FAKE_BLACKWELL: FakeCard = FakeCard {
    name: FAKE_NAME,
    device: FAKE_DEVICE,
    architecture: 0x1B0,
    implementation: None,
    sm_version: 0xA04,
    engines: &FAKE_ENGINES,
    graphics_copy: FAKE_GRAPHICS_COPY,
    classes: &FAKE_CLASSES,
    class_count: FAKE_CLASSES.len(),
};

/// A pretend 40-series card. Its engines are only there to make the nodes come out.
pub const FAKE_ADA: FakeCard = FakeCard {
    name: "Fake Ada Card",
    device: 0x2704,
    architecture: 0x190,
    implementation: Some(0x3),
    sm_version: 0x809,
    engines: &[0x01, 0x09, 0x0D, 0x0E, 0x13, 0x1B, 0x1C, 0x26, 0x33],
    graphics_copy: 0x09,
    classes: &[
        0x902D, 0xA140, 0xC36F, 0xC361, 0xC46F, 0xC461, 0xC56F, 0xC561, 0xC7B5, 0xC997, 0xC9B0, 0xC9B7,
        0xC9C0,
    ],
    class_count: 127,
};

/// A pretend 16-series card, as the one above.
pub const FAKE_TURING: FakeCard = FakeCard {
    name: "Fake Turing Card",
    device: 0x21C4,
    architecture: 0x160,
    implementation: Some(0x8),
    sm_version: 0x703,
    engines: &[0x01, 0x09, 0x0A, 0x0B, 0x0C, 0x13, 0x1B, 0x26],
    graphics_copy: 0x09,
    classes: &[0x902D, 0xA140, 0xC36F, 0xC361, 0xC46F, 0xC461, 0xC4B0, 0xC4B7, 0xC597, 0xC5B5, 0xC5C0],
    class_count: 110,
};

// A real RM copes with parameters of the wrong size, so these do too.
fn word(params: &[u8], at: usize) -> u32 {
    params.get(at..at + 4).map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
}

fn put(params: &mut [u8], at: usize, value: u32) {
    if let Some(field) = params.get_mut(at..at + 4) {
        field.copy_from_slice(&value.to_le_bytes());
    }
}

/// Made-up bytes, the same each time.
fn fill(command: u32, params: &mut [u8]) {
    for (index, byte) in params.iter_mut().enumerate() {
        *byte = command.wrapping_mul(31).wrapping_add(index as u32 * 7) as u8;
    }
}

/// A made-up value for one entry of a table.
fn entry_value(command: u32, index: u32) -> u32 {
    command.wrapping_mul(2_654_435_761).wrapping_add(index.wrapping_mul(40503)) >> 4
}

fn table(rm: &mut FakeRm, control: Control, special: impl Fn(u32) -> Option<u32> + Send + 'static) {
    let command = control.0;
    rm.answer(command, move |params| {
        let index = word(params, 4);
        put(params, 8, special(index).unwrap_or_else(|| entry_value(command, index)));
        0
    });
}

fn plain(rm: &mut FakeRm, control: Control) {
    let command = control.0;
    rm.answer(command, move |params| {
        fill(command, params);
        0
    });
}

fn list(rm: &mut FakeRm, control: Control, items: Vec<u32>) {
    rm.answer(control.0, move |params| {
        put(params, 0, items.len() as u32);
        items.iter().enumerate().for_each(|(index, &item)| put(params, 4 + index * 4, item));
        0
    });
}

fn text(rm: &mut FakeRm, control: Control, at: usize, text: &'static str) {
    rm.answer(control.0, move |params| {
        if let Some(field) = params.get_mut(at..at + text.len()) {
            field.copy_from_slice(text.as_bytes());
        }
        0
    });
}

/// The card's class list, at the length it says.
fn class_list(card: &FakeCard) -> Vec<u32> {
    let fillers = (FIRST_FILLER_CLASS..).take(card.class_count.saturating_sub(card.classes.len()));
    card.classes.iter().copied().chain(fillers).collect()
}

/// Gives a fake RM the usual pretend card's answers.
pub fn install(rm: &mut FakeRm) {
    install_card(rm, &FAKE_BLACKWELL);
}

/// Gives a fake RM a card's answers.
pub fn install_card(rm: &mut FakeRm, card: &FakeCard) {
    let card = *card;
    for control in [
        CPU_INFO,
        CHIPSET_INFO,
        FRAMEBUFFER_CAPS,
        FIFO_CAPS,
        GRAPHICS_CAPS,
        UUID,
        ZCULL_INFO,
        GPC_MASK,
        TPC_MASK,
        DMA_CAPS,
        VIDEO_DECODE_CAPS,
    ] {
        plain(rm, control);
    }
    list(rm, ENGINES, card.engines.to_vec());
    list(rm, CLASS_LIST, class_list(&card));
    text(rm, NAME, 4, card.name);
    text(rm, SHORT_NAME, 0, FAKE_SHORT_NAME);
    table(rm, GPU_INFO, |_| None);
    table(rm, BUS_INFO, |_| None);
    table(rm, FRAMEBUFFER_INFO, |index| match index {
        FB_RAM => Some(FAKE_RAM_KIB),
        FB_BAR1 => Some(FAKE_RAM_KIB / 2),
        _ => None,
    });
    table(rm, GRAPHICS_INFO, move |index| match index {
        GRAPHICS_SM_VERSION => Some(card.sm_version),
        GRAPHICS_GPC_COUNT => Some(FAKE_GPC_COUNT),
        _ => None,
    });
    rm.answer(PCI_INFO.0, move |params| {
        put(params, 0, card.device << 16 | FAKE_VENDOR);
        put(params, 4, 0x5555_0001);
        put(params, 8, 0xA1);
        put(params, 12, 0x77);
        0
    });
    rm.answer(ARCHITECTURE_INFO.0, move |params| {
        fill(ARCHITECTURE_INFO.0, params);
        put(params, 0, card.architecture);
        if let Some(implementation) = card.implementation {
            put(params, ARCHITECTURE_IMPLEMENTATION_FIELD, implementation);
        }
        0
    });
    rm.answer(ENGINE_PARTNERS.0, move |params| {
        put(params, 12, 1);
        put(params, 16, card.graphics_copy);
        0
    });
    rm.answer(JPEG_CAPS.0, |params| {
        if word(params, 12) >= JPEG_ENGINES_PRESENT {
            return STATUS_NO_SUCH_ENGINE;
        }
        fill(JPEG_CAPS.0, params);
        0
    });
    rm.answer(COPY_ENGINE_MASK.0, |params| {
        let engine = word(params, 0);
        put(params, 4, engine & 0xF);
        0
    });
    rm.answer(CONTROL_GET_GPU_ID, |params| {
        put(params, 0, FAKE_GPU_ID);
        0
    });
    rm.answer(CONTROL_GET_TIME, |params| {
        if let Some(field) = params.first_chunk_mut::<8>() {
            *field = 123_456_789u64.to_le_bytes();
        }
        0
    });
    // The client exists only so that nothing above needs to know it is fake.
    let _ = rm.new_client();
}
