//! The list of engine nodes the video runtime asks for.
//!
//! Each entry gives a node's type, the same types as the record's list, and whether it's scheduled.
//! Video decode and encode run with this answer. Measured: with an empty list, `cuvidCreateDecoder`
//! fails.

use super::put;
use crate::answers::nodes::Node;

const ENTRIES: usize = 0x30;
const ENTRY_SIZE: usize = 24;
const ENTRY_KIND: usize = 0;
const ENTRY_INSTANCE: usize = 4;
const ENTRY_SCHEDULED: usize = 8;
const COUNT: usize = 0x318;
const MOST_ENTRIES: usize = (COUNT - ENTRIES) / ENTRY_SIZE;

/// Fills in the answer, as far as the guest's buffer goes.
pub(super) fn answer(nodes: &[Node], data: &mut [u8]) {
    let listed = nodes.len().min(MOST_ENTRIES);
    for (index, node) in nodes[..listed].iter().enumerate() {
        let at = ENTRIES + index * ENTRY_SIZE;
        put(data, at + ENTRY_KIND, node.kind);
        put(data, at + ENTRY_INSTANCE, 1);
        put(data, at + ENTRY_SCHEDULED, u32::from(node.scheduled));
    }
    put(data, COUNT, listed as u32);
}

#[cfg(test)]
mod tests {
    use super::super::get;
    use super::*;
    use crate::answers::nodes::node_list;
    use crate::profile::BLACKWELL;

    /// The size the runtime really asks with.
    const WHOLE_ANSWER: usize = 0x31C;

    #[test]
    fn every_node_is_listed_by_its_kind_in_order() {
        let nodes = node_list(&[0x01, 0x09, 0x0D, 0x13, 0x1B], None, &BLACKWELL.nodes);
        let mut data = vec![0; WHOLE_ANSWER];
        answer(&nodes, &mut data);
        assert_eq!(get(&data, COUNT), Some(nodes.len() as u32));
        for (index, node) in nodes.iter().enumerate() {
            assert_eq!(get(&data, ENTRIES + index * ENTRY_SIZE), Some(node.kind));
        }
    }

    #[test]
    fn a_short_buffer_gets_what_fits() {
        let nodes = node_list(&[0x01, 0x09], None, &BLACKWELL.nodes);
        let mut data = vec![0; ENTRIES + 4];
        answer(&nodes, &mut data);
        assert_eq!(get(&data, ENTRIES), Some(nodes[0].kind));
    }
}
