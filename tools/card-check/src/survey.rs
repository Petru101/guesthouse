//! The read-only part: what the card says of itself, through controls that only read, and what the
//! app would make of it.

use guesthouse_backend_nvidia::answers::nodes::{Node, is_copy_engine, node_list};
use guesthouse_backend_nvidia::profile::{Generation, generation_for};
use guesthouse_backend_nvidia::record::card::{ARCHITECTURE_INFO, CardReader, GRAPHICS_INFO, NAME, PCI_INFO};
use guesthouse_backend_nvidia::rm::device::Device;
use guesthouse_backend_nvidia::rm::{Rm, RmError};
use guesthouse_backend_nvidia::startup::card_tested;

/// The graphics table's entry holding the SM version.
const SM_VERSION_ENTRY: u32 = 0x0C;
const IMPLEMENTATION_FIELD: usize = 4;
const PCI_REVISION_FIELD: usize = 8;
const NAME_TEXT_AT: usize = 4;

/// One of the card's copy engines, as the guest's record would describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyEngine {
    pub engine: u32,
    pub caps: u32,
    /// Which physical engines serve it.
    pub physical: u32,
}

/// What the app would do with this card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Supported,
    /// The generation has a profile. `ever_run` says whether a card of it has run a guest.
    Experimental {
        generation: &'static str,
        ever_run: bool,
    },
    Refused(String),
}

/// Everything the read-only part found. Nothing here identifies the card or the PC.
#[derive(Debug, Clone)]
pub struct Findings {
    pub name: String,
    pub device: u16,
    pub revision: u8,
    pub architecture: u32,
    pub implementation: u32,
    pub sm_version: u32,
    pub shader_set: Option<u32>,
    pub generation: Option<&'static Generation>,
    pub verdict: Verdict,
    pub engines: Vec<u32>,
    /// The copy engine the host pairs with graphics, if it names one.
    pub graphics_copy: Option<u32>,
    pub copy_engines: Vec<CopyEngine>,
    /// The engine nodes a guest would see, if the generation has a profile.
    pub nodes: Vec<Node>,
    pub classes: Vec<u32>,
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn verdict(
    generation: Option<&Generation>,
    shader_set: Option<u32>,
    device: u16,
    architecture: u32,
    sm: u32,
) -> Verdict {
    let Some(generation) = generation else {
        return Verdict::Refused(format!(
            "This build has no profile for GPU architecture {architecture:#x}."
        ));
    };
    if shader_set.is_none() {
        return Verdict::Refused(format!("SM {sm:#x} has not been tested with this project."));
    }
    if card_tested(device) {
        return Verdict::Supported;
    }
    Verdict::Experimental {
        generation: generation.name,
        ever_run: !generation.run_on_guest_branches.is_empty(),
    }
}

/// Reads the card. Every control here only reads.
pub fn survey<R: Rm>(device: &mut Device<R>) -> Result<Findings, RmError> {
    let mut card = CardReader::new(device);
    let name = card.text(NAME, NAME_TEXT_AT)?;
    let pci = card.read(PCI_INFO, &[])?;
    let device_id = (word(&pci, 0) >> 16) as u16;
    let revision = word(&pci, PCI_REVISION_FIELD) as u8;
    let architecture_info = card.read(ARCHITECTURE_INFO, &[])?;
    let architecture = word(&architecture_info, 0);
    let implementation = word(&architecture_info, IMPLEMENTATION_FIELD);
    let sm_version = card.entry(GRAPHICS_INFO, SM_VERSION_ENTRY)?;
    let generation = generation_for(architecture);
    let shader_set = generation
        .and_then(|generation| generation.shader_sets.iter().find(|set| set.0 == sm_version))
        .map(|set| set.1);
    let engines = card.engines()?;
    let classes = card.classes()?;
    let graphics_copy = card.graphics_copy()?;
    let mut copy_engines = Vec::new();
    for &engine in engines.iter().filter(|&&engine| is_copy_engine(engine)) {
        let caps = card.copy_engine_caps(engine)?;
        let physical = card.copy_engine_mask(engine)?;
        copy_engines.push(CopyEngine { engine, caps, physical });
    }
    let nodes =
        generation.map_or_else(Vec::new, |generation| node_list(&engines, graphics_copy, &generation.nodes));
    Ok(Findings {
        name,
        device: device_id,
        revision,
        architecture,
        implementation,
        sm_version,
        shader_set,
        generation,
        verdict: verdict(generation, shader_set, device_id, architecture, sm_version),
        engines,
        graphics_copy,
        copy_engines,
        nodes,
        classes,
    })
}
