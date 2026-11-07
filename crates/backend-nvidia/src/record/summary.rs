//! A short text of what the backend found on a card, for someone trying one to read and post. It
//! names each field it reads from the record, and leaves out the card's UUID and ECID.

use super::builder::{GRAPHICS_SM_VERSION, TABLE_ENTRY_SIZE};
use crate::answers::nodes::Node;
use crate::profile::{Generation, RecordLayout};
use std::fmt::Write;

const SUMMARY_VERSION: u32 = 1;
/// Where an entry of a table keeps its value, after its index.
const TABLE_VALUE_FIELD: usize = 4;
const IMPLEMENTATION_FIELD: usize = 4;
const PCI_DEVICE_FIELD: usize = 12;
const CLASSES_PER_LINE: usize = 8;

/// What goes into a summary.
pub struct SummaryFacts<'a> {
    pub record: &'a [u8],
    pub layout: &'a RecordLayout,
    pub generation: &'a Generation,
    pub guest_branch: &'a str,
    pub engines: &'a [u32],
    pub nodes: &'a [Node],
}

fn word(record: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(record[at..at + 4].try_into().expect("four bytes"))
}

fn class_lines(record: &[u8], at: usize) -> String {
    let count = word(record, at) as usize;
    let classes: Vec<String> =
        (0..count).map(|index| format!("{:04X}", word(record, at + 4 + index * 4))).collect();
    let mut text = format!("Classes: {count}\n");
    for line in classes.chunks(CLASSES_PER_LINE) {
        writeln!(text, "  {}", line.join(" ")).expect("a string takes a write");
    }
    text
}

/// See the module documentation.
pub fn summary(facts: &SummaryFacts<'_>) -> String {
    let (record, layout) = (facts.record, facts.layout);
    let sm_at = layout.graphics_table + GRAPHICS_SM_VERSION as usize * TABLE_ENTRY_SIZE + TABLE_VALUE_FIELD;
    let engines: Vec<String> = facts.engines.iter().map(|engine| format!("{engine:#x}")).collect();
    let mut text = format!(
        "Guesthouse card summary, version {SUMMARY_VERSION}\n\n\
         Generation: {}\nGuest branch: {}\nDevice ID: {:#06X}\nArchitecture: {:#X}\nImplementation: {:#X}\n\
         SM version: {:#X}\nShader set: {:#X}\nEngines from the RM: {}\nNodes: {}\n",
        facts.generation.name,
        facts.guest_branch,
        word(record, layout.pci + PCI_DEVICE_FIELD) >> 16,
        word(record, layout.architecture),
        word(record, layout.architecture + IMPLEMENTATION_FIELD),
        word(record, sm_at),
        word(record, layout.shader_set),
        engines.join(" "),
        facts.nodes.len(),
    );
    for (ordinal, node) in facts.nodes.iter().enumerate() {
        writeln!(
            text,
            "  {ordinal}: type {:#04X}, engine {}, scheduled {}, in flight {}, copy engine {:#x} {}",
            node.kind,
            node.engine,
            u32::from(node.scheduled),
            node.in_flight,
            node.copy_engine,
            node.name,
        )
        .expect("a string takes a write");
    }
    text + &class_lines(record, layout.classes)
}
