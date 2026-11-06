//! The engine nodes the guest sees, from the list of engines the RM reports.
//!
//! Each engine the card has gets a node, and names go by the engine's real index, so a card with only
//! the fourth JPEG engine says so. JPEG and optical flow engines have no channels here and the guest
//! doesn't mind which numbers their nodes have, so only engines that have been run get a node. A card
//! with others shows fewer nodes.

// The guest's engine kinds.
pub const ENGINE_OTHER: u32 = 0;
pub const ENGINE_3D: u32 = 1;
pub const ENGINE_VIDEO_DECODE: u32 = 2;
pub const ENGINE_VIDEO_ENCODE: u32 = 3;
pub const ENGINE_COPY: u32 = 6;

const CONTEXT_SCHEDULING_SUPPORTED: u32 = 1 << 0;
const MAX_IN_FLIGHT_SHIFT: u32 = 16;
const GRAPHICS_IN_FLIGHT: u32 = 12;
const COPY_IN_FLIGHT: u32 = 32;
const DEFAULT_IN_FLIGHT: u32 = 4;
const NAME_CHARS: usize = 32;

// The node types the guest's driver numbers its nodes by. The lists of pairs are by the engine's index
// in the RM's list.
const NODE_GRAPHICS: u32 = 0x01;
const NODE_SECURITY: u32 = 0x0A;
const NODE_VR: u32 = 0x1C;
const NODE_SECURITY_VIRTUAL: u32 = 0x3B;
const NODE_COPY: [u32; 3] = [0x0B, 0x0C, 0x0D];
const NODE_COPY_SCHEDULED: [u32; 2] = [0x0C, 0x0D];
const NODE_VIRTUAL_COPY: [u32; 4] = [0x1D, 0x0E, 0x0F, 0x10];
const NODE_DECODE: [(usize, u32); 2] = [(0, 0x1A), (3, 0x22)];
const NODE_ENCODE: [(usize, u32); 3] = [(1, 0x14), (2, 0x15), (3, 0x36)];
const NODE_OPTICAL_FLOW: [(usize, u32); 1] = [(0, 0x21)];
const NODE_JPEG: [(usize, u32); 1] = [(3, 0x27)];

// The RM's engine numbers, from its public headers.
const RM_COPY: [u32; 20] = [
    0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A,
    0x3B, 0x3C, 0x3D,
];
const RM_DECODE: [u32; 8] = [0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A];
const RM_ENCODE: [u32; 4] = [0x1B, 0x1C, 0x1D, 0x3F];
const RM_JPEG: [u32; 8] = [0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x32];
const RM_OPTICAL_FLOW: [u32; 2] = [0x33, 0x3E];
const RM_SECURITY: u32 = 0x26;

const SECURITY_NAME: &str = "Security";
const SECURITY_VIRTUAL_NAME: &str = "Security_1";
const VR_NAME: &str = "VR";

/// What every copy node's record says on the tested card. Each bit can go alone, but a guest with
/// none of them failed its Vulkan test.
const COPY_FLAGS: u32 = 0x1A0;
const COPY_FLAG_SHARED_NODE: u32 = 0x02;
const COPY_FLAG_GRAPHICS: u32 = 0x04;
const COPY_FLAG_OWN_ENGINE: u32 = 0x10;

/// One node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: u32,
    pub engine: u32,
    pub name: String,
    pub scheduled: bool,
    pub in_flight: u32,
    /// The RM's copy engine, for nodes that copy.
    pub copy_engine: u32,
    /// The RM's engine and which of its kind it is, for nodes that decode or encode.
    pub video: Option<(u32, u32)>,
}

impl Node {
    fn new(kind: u32, engine: u32) -> Self {
        Self {
            kind,
            engine,
            name: String::new(),
            scheduled: false,
            in_flight: DEFAULT_IN_FLIGHT,
            copy_engine: 0,
            video: None,
        }
    }

    fn video(kind: u32, engine: u32, rm_engine: u32, instance: usize) -> Self {
        Self { video: Some((rm_engine, instance as u32)), ..Self::new(kind, engine) }
    }

    fn named(kind: u32, name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Self::new(kind, ENGINE_OTHER) }
    }
}

/// The indices in `table` of the engines this card has, as a card with fused-off parts skips some.
fn present(engines: &[u32], table: &[u32]) -> Vec<usize> {
    (0..table.len()).filter(|&index| engines.contains(&table[index])).collect()
}

/// The (index, node type) pairs of a table whose engines this card has.
fn known(engines: &[u32], rm_table: &[u32], nodes: &[(usize, u32)]) -> Vec<(usize, u32)> {
    nodes.iter().copied().filter(|&(index, _)| engines.contains(&rm_table[index])).collect()
}

/// The nodes from the RM's engine list. `graphics_copy` is the engine the RM pairs with graphics,
/// which gets no node of its own. The stand-in copy nodes are made from the first real one.
pub fn node_list(engines: &[u32], graphics_copy: Option<u32>) -> Vec<Node> {
    let copies: Vec<u32> = present(engines, &RM_COPY).into_iter().map(|index| RM_COPY[index]).collect();
    let graphics_copy = graphics_copy.filter(|&engine| engine != 0).or(copies.first().copied()).unwrap_or(0);
    let real: Vec<u32> =
        copies.iter().copied().filter(|&engine| engine != graphics_copy).take(NODE_COPY.len()).collect();
    let mut nodes = vec![Node {
        scheduled: true,
        in_flight: GRAPHICS_IN_FLIGHT,
        copy_engine: graphics_copy,
        ..Node::new(NODE_GRAPHICS, ENGINE_3D)
    }];
    for (index, kind) in known(engines, &RM_DECODE, &NODE_DECODE) {
        nodes.push(Node::video(kind, ENGINE_VIDEO_DECODE, RM_DECODE[index], index));
    }
    for (&kind, &engine) in NODE_COPY.iter().zip(&real) {
        let scheduled = NODE_COPY_SCHEDULED.contains(&kind);
        let in_flight = if scheduled { COPY_IN_FLIGHT } else { DEFAULT_IN_FLIGHT };
        nodes.push(Node { scheduled, in_flight, copy_engine: engine, ..Node::new(kind, ENGINE_COPY) });
    }
    let security = engines.contains(&RM_SECURITY);
    if security {
        nodes.push(Node::named(NODE_SECURITY, SECURITY_NAME));
    }
    for (index, kind) in known(engines, &RM_ENCODE, &NODE_ENCODE) {
        nodes.push(Node::video(kind, ENGINE_VIDEO_ENCODE, RM_ENCODE[index], index));
    }
    for (index, kind) in known(engines, &RM_OPTICAL_FLOW, &NODE_OPTICAL_FLOW) {
        nodes.push(Node::named(kind, format!("OFA_{index}")));
    }
    for (index, kind) in known(engines, &RM_JPEG, &NODE_JPEG) {
        nodes.push(Node::named(kind, format!("JPEG_Decode_{index}")));
    }
    if RM_ENCODE[..2].iter().any(|engine| engines.contains(engine)) {
        nodes.push(Node::named(NODE_VR, VR_NAME));
    }
    if let Some(&first) = real.first() {
        for kind in NODE_VIRTUAL_COPY {
            nodes.push(Node { copy_engine: first, ..Node::new(kind, ENGINE_COPY) });
        }
    }
    if security {
        nodes.push(Node::named(NODE_SECURITY_VIRTUAL, SECURITY_VIRTUAL_NAME));
    }
    nodes
}

/// What the guest is told about a node, in answer to its query.
pub fn node_metadata(ordinal: u32, node: &Node) -> Vec<u8> {
    let flags =
        if node.scheduled { CONTEXT_SCHEDULING_SUPPORTED | node.in_flight << MAX_IN_FLIGHT_SHIFT } else { 0 };
    let mut out = ordinal.to_le_bytes().to_vec();
    out.extend_from_slice(&node.engine.to_le_bytes());
    let start = out.len();
    node.name.encode_utf16().for_each(|unit| out.extend_from_slice(&unit.to_le_bytes()));
    out.resize(start + NAME_CHARS * 2, 0);
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&[1, 0]);
    out
}

/// A node's entry in the record's list.
pub fn node_entry(node: &Node) -> [u8; 40] {
    let full_copy = NODE_COPY.contains(&node.kind) || NODE_VIRTUAL_COPY[1..].contains(&node.kind);
    let engine_context = node.kind == NODE_GRAPHICS || full_copy;
    let fields = [
        node.kind,
        1,
        u32::from(node.scheduled),
        u32::from(node.kind == NODE_GRAPHICS || node.kind == NODE_VR),
        0,
        node.in_flight,
        u32::from(engine_context),
        u32::from(node.scheduled),
        0,
        0,
    ];
    let mut out = [0; 40];
    for (bytes, field) in out.as_chunks_mut::<4>().0.iter_mut().zip(fields) {
        bytes.copy_from_slice(&field.to_le_bytes());
    }
    out
}

/// The flags at the head of a copy node's record.
pub fn copy_record_flags(node: &Node) -> u32 {
    let mut flags = COPY_FLAGS;
    if node.kind == NODE_GRAPHICS {
        flags |= COPY_FLAG_GRAPHICS;
    }
    if node.kind == NODE_VIRTUAL_COPY[0] {
        return flags | COPY_FLAG_SHARED_NODE;
    }
    flags | COPY_FLAG_OWN_ENGINE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(engines: &[u32], graphics_copy: Option<u32>) -> Vec<(u32, bool)> {
        let nodes = node_list(engines, graphics_copy);
        nodes.iter().map(|node| (node.kind, node.scheduled)).collect()
    }

    #[test]
    fn the_second_and_third_copy_nodes_are_scheduled() {
        let found = kinds(&[0x01, 0x09, 0x0D, 0x0E, 0x0F, 0x26, 0x1B], None);
        let copies: Vec<_> = found.iter().filter(|(kind, _)| NODE_COPY.contains(kind)).collect();
        assert_eq!(copies, [&(0x0B, false), &(0x0C, true), &(0x0D, true)]);
    }

    #[test]
    fn graphics_copy_gets_no_node() {
        let nodes = node_list(&[0x01, 0x09, 0x0D], Some(0x0D));
        assert_eq!(nodes[0].copy_engine, 0x0D);
        assert_eq!(nodes[1].copy_engine, 0x09);
    }

    #[test]
    fn names_go_by_the_engines_real_index() {
        let nodes = node_list(&[0x01, 0x09, 0x2E], None);
        assert!(nodes.iter().any(|node| node.name == "JPEG_Decode_3"));
    }

    #[test]
    fn an_engine_that_has_never_been_run_gets_no_node() {
        let plain = node_list(&[0x01, 0x09], None).len();
        assert_eq!(node_list(&[0x01, 0x09, 0x14, 0x2B], None).len(), plain);
        assert_eq!(node_list(&[0x01, 0x09, 0x13], None).len(), plain + 1);
    }

    #[test]
    fn with_no_other_copy_engine_there_are_no_stand_ins() {
        assert!(node_list(&[0x01, 0x09], None).iter().all(|node| node.kind != NODE_VIRTUAL_COPY[0]));
    }

    #[test]
    fn metadata_has_its_ordinal_engine_and_flags() {
        let nodes = node_list(&[0x01, 0x09], None);
        let bytes = node_metadata(0, &nodes[0]);
        assert_eq!(bytes.len(), 8 + 64 + 4 + 2);
        assert_eq!(bytes[4], ENGINE_3D as u8);
        assert_eq!(bytes[72..76], (1u32 | 12 << 16).to_le_bytes());
    }

    #[test]
    fn a_stand_in_copy_node_says_it_shares_an_engine() {
        let node = Node::new(NODE_COPY[0], ENGINE_COPY);
        assert_eq!(copy_record_flags(&node), COPY_FLAGS | COPY_FLAG_OWN_ENGINE);
        let stand_in = Node::new(NODE_VIRTUAL_COPY[0], ENGINE_COPY);
        assert_eq!(copy_record_flags(&stand_in), COPY_FLAGS | COPY_FLAG_SHARED_NODE);
    }
}
