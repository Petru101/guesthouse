//! Answers to adapter queries the vendor owns.

use super::caps::{CAPS_SIZE, caps_answer};
use super::guid::name_based_guid;
use super::nodes::{ENGINE_3D, ENGINE_COPY, ENGINE_VIDEO_ENCODE, Node, node_metadata};
use super::queries::fixed_answers;
use super::registry::{AdapterRegistry, DRIVER_STORE};
use crate::profile::RecordLayout;
use guesthouse_backend::guest_text::{self, QUERY_USER_DRIVER_NAME};
use guesthouse_backend::{Status, VendorQuery};
use std::collections::HashMap;

const QUERY_RECORD: u32 = 0;
const QUERY_OPENGL: u32 = 2;
const QUERY_NODE: u32 = 25;
const QUERY_GPU_MMU_CAPS: u32 = 34;
const QUERY_WDDM_3_0_CAPS: u32 = 77;

/// The answers to the two queries above, which are in Microsoft's public headers. Direct3D 12 stops
/// without the second.
const GPU_MMU_CAPS: [u32; 3] = [0, 5, 44];
const WDDM_3_0_CAPS: u32 = 0xE;

const USER_DRIVER: &str = "nvldumdx.dll";
const USER_DRIVER_32_BIT: &str = "nvldumd.dll";
const OPENGL_NAME_BYTES: usize = 520;
const NODE_NUMBER_MASK: u32 = 0xFFFF;
const NODE_ENGINE_FIELD: usize = 4;

/// What goes into the answers. Clocks are in hertz.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub record: Vec<u8>,
    /// The RM's engine numbers on this card.
    pub engines: Vec<u32>,
    pub nodes: Vec<Node>,
    pub registry: AdapterRegistry,
    pub guid_seed: String,
    pub graphics_hertz: u64,
    pub memory_hertz: u64,
}

/// See the module documentation.
#[derive(Debug, Clone)]
pub struct AdapterAnswers {
    record: Vec<u8>,
    rm_engines: Vec<u32>,
    layout: &'static RecordLayout,
    discovery_field: usize,
    fixed: HashMap<u32, Vec<u8>>,
    nodes: Vec<Vec<u8>>,
    engines: Vec<u32>,
    /// Which nodes can run video.
    video_nodes: Vec<bool>,
    registry: AdapterRegistry,
}

impl AdapterAnswers {
    pub fn new(inputs: Inputs, layout: &'static RecordLayout) -> Self {
        let guid = name_based_guid(&inputs.guid_seed);
        let fixed = fixed_answers(&guid, inputs.graphics_hertz, inputs.memory_hertz);
        let numbered = (0u32..).zip(&inputs.nodes);
        Self {
            record: inputs.record,
            rm_engines: inputs.engines,
            layout,
            discovery_field: layout.gpu_discovery,
            fixed: fixed.into_iter().collect(),
            nodes: numbered.map(|(ordinal, node)| node_metadata(ordinal, node)).collect(),
            engines: inputs.nodes.iter().map(|node| node.engine).collect(),
            video_nodes: inputs.nodes.iter().map(|node| node.video.is_some()).collect(),
            registry: inputs.registry,
        }
    }

    /// The record as built, before anything a query adds.
    pub fn record(&self) -> &[u8] {
        &self.record
    }

    /// The RM's engine numbers on this card.
    pub fn rm_engines(&self) -> &[u32] {
        &self.rm_engines
    }

    pub fn registry(&self) -> &AdapterRegistry {
        &self.registry
    }

    /// Graphics and copy engines. Video engines have channels too, of their own kind.
    pub fn node_has_channel(&self, node: u32) -> bool {
        let engine = self.engines.get(node as usize);
        engine.is_some_and(|&engine| engine == ENGINE_3D || engine == ENGINE_COPY)
    }

    /// Whether a node is the card's graphics engine.
    pub fn node_is_graphics(&self, node: u32) -> bool {
        self.engines.get(node as usize) == Some(&ENGINE_3D)
    }

    /// Whether a node is one of the card's video encoders.
    pub fn node_is_encoder(&self, node: u32) -> bool {
        self.engines.get(node as usize) == Some(&ENGINE_VIDEO_ENCODE)
    }

    /// Whether a node is one of the card's video decoders or encoders that can run.
    pub fn node_is_video(&self, node: u32) -> bool {
        self.video_nodes.get(node as usize) == Some(&true)
    }

    fn record_answer(&self, size: usize, out: &mut Vec<u8>) {
        let start = out.len();
        let copied = size.min(self.record.len());
        out.extend_from_slice(&self.record[..copied]);
        out.resize(start + size, 0);
        let field = start + self.discovery_field;
        if let Some(flag) = out.get_mut(field..field + 4) {
            flag.copy_from_slice(&1u32.to_le_bytes());
        }
    }

    fn user_driver_name(query: &VendorQuery<'_>, out: &mut Vec<u8>) {
        let name = if query.process_is_32_bit { USER_DRIVER_32_BIT } else { USER_DRIVER };
        guest_text::user_driver_name(query, &format!("{DRIVER_STORE}\\{name}"), out);
    }

    fn opengl_info(&self, query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Option<()> {
        let suffix = if query.process_is_32_bit { "wow" } else { "" };
        let path = self.registry.bytes(&format!("opengldrivername{suffix}"))?;
        let start = out.len();
        out.extend_from_slice(path);
        out.resize((start + OPENGL_NAME_BYTES).max(out.len()), 0);
        out.extend_from_slice(self.registry.bytes(&format!("openglversion{suffix}"))?);
        out.extend_from_slice(self.registry.bytes(&format!("openglflags{suffix}"))?);
        Some(())
    }

    fn node_answer(&self, query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Status {
        let asked = query.data.first_chunk::<4>().map_or(0, |bytes| u32::from_le_bytes(*bytes));
        let Some(record) = self.nodes.get((asked & NODE_NUMBER_MASK) as usize) else {
            return Status::NOT_SUPPORTED;
        };
        out.extend_from_slice(query.data.get(..NODE_ENGINE_FIELD).unwrap_or(&[0; NODE_ENGINE_FIELD]));
        let end = query.size.min(record.len());
        out.extend_from_slice(record.get(NODE_ENGINE_FIELD..end).unwrap_or(&[]));
        Status::SUCCESS
    }

    /// Answers a query, or returns `None` for one the core answers.
    pub fn answer(&self, query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Option<Status> {
        match query.kind {
            QUERY_RECORD if query.size == CAPS_SIZE => {
                caps_answer(&self.record, self.layout, &self.rm_engines, query.data, out)
            }
            QUERY_RECORD => self.record_answer(query.size, out),
            QUERY_USER_DRIVER_NAME => Self::user_driver_name(query, out),
            QUERY_OPENGL => self.opengl_info(query, out)?,
            QUERY_NODE => return Some(self.node_answer(query, out)),
            QUERY_GPU_MMU_CAPS => {
                GPU_MMU_CAPS.iter().for_each(|word| out.extend_from_slice(&word.to_le_bytes()))
            }
            QUERY_WDDM_3_0_CAPS => out.extend_from_slice(&WDDM_3_0_CAPS.to_le_bytes()),
            kind => {
                let answer = self.fixed.get(&kind).filter(|answer| answer.len() == query.size)?;
                out.extend_from_slice(answer);
            }
        }
        Some(Status::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::answers::nodes::node_list;
    use crate::profile::{BLACKWELL, GUEST_617};

    fn answers() -> AdapterAnswers {
        let inputs = Inputs {
            record: vec![7; 16],
            engines: Vec::new(),
            nodes: node_list(&[0x01, 0x09, 0x0D, 0x13], None, &BLACKWELL.nodes),
            registry: AdapterRegistry::default(),
            guid_seed: "seed".into(),
            graphics_hertz: 1_000,
            memory_hertz: 2_000,
        };
        AdapterAnswers::new(inputs, &GUEST_617.record)
    }

    fn query(kind: u32, size: usize, data: &[u8]) -> VendorQuery<'_> {
        VendorQuery { kind, size, data, process_is_32_bit: false }
    }

    #[test]
    fn the_record_fills_the_size_asked_and_sets_the_discovery_word() {
        let mut out = Vec::new();
        let field = GUEST_617.record.gpu_discovery;
        answers().answer(&query(0, field + 8, &[]), &mut out);
        assert_eq!(out.len(), field + 8);
        assert_eq!(out[0], 7);
        assert_eq!(out[field], 1);
    }

    #[test]
    fn the_driver_name_keeps_the_guests_version_field() {
        let mut out = Vec::new();
        let data = [9, 0, 0, 0, 0, 0, 0, 0];
        answers().answer(&query(1, 8, &data), &mut out);
        assert_eq!(out[0], 9);
        let name: Vec<u16> =
            out[4..].as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit)).collect();
        assert!(String::from_utf16_lossy(&name).contains(USER_DRIVER));
    }

    #[test]
    fn a_32_bit_process_gets_the_32_bit_driver() {
        let mut out = Vec::new();
        let query = VendorQuery { kind: 1, size: 8, data: &[0; 8], process_is_32_bit: true };
        answers().answer(&query, &mut out);
        let name: Vec<u16> =
            out[4..].as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit)).collect();
        assert!(String::from_utf16_lossy(&name).contains(USER_DRIVER_32_BIT));
    }

    #[test]
    fn a_node_answer_keeps_what_the_guest_sent_in_front() {
        let mut out = Vec::new();
        let status = answers().answer(&query(25, 78, &[1, 0, 0, 0xAA]), &mut out);
        assert_eq!(status, Some(Status::SUCCESS));
        assert_eq!(out[..4], [1, 0, 0, 0xAA]);
        assert_eq!(out.len(), 78);
    }

    #[test]
    fn a_node_that_does_not_exist_is_not_supported() {
        let mut out = Vec::new();
        assert_eq!(answers().answer(&query(25, 78, &[99, 0, 0, 0]), &mut out), Some(Status::NOT_SUPPORTED));
    }

    #[test]
    fn a_built_answer_of_the_wrong_size_is_left_to_the_core() {
        let mut out = Vec::new();
        assert_eq!(answers().answer(&query(11, 8, &[]), &mut out), None);
        assert_eq!(answers().answer(&query(11, 4, &[]), &mut out), Some(Status::SUCCESS));
    }

    #[test]
    fn video_nodes_have_no_channel() {
        let answers = answers();
        assert!(answers.node_has_channel(0));
        let decode = answers.engines.iter().position(|&engine| engine == 2).unwrap();
        assert!(!answers.node_has_channel(decode as u32));
        assert!(!answers.node_has_channel(100));
    }
}
