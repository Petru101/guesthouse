//! Ring entries and method headers: the words the GPU's command processor reads.
//!
//! The formats are in the open kernel module's class headers for channels.

/// Bytes in a ring entry.
pub const ENTRY_SIZE: usize = 8;

const ENTRY_ADDRESS_HIGH_MASK: u32 = 0xFF;
const ENTRY_LENGTH_SHIFT: u32 = 10;
const ENTRY_LENGTH_MASK: u32 = 0x1F_FFFF;
const ENTRY_NO_PREFETCH_OR_FETCH: u64 = 1 << 41;
const ENTRY_LENGTH_SHIFT_IN_U64: u32 = 42;

const HEADER_INCREASING: u32 = 0x2000_0000;
const HEADER_COUNT_SHIFT: u32 = 16;
const HEADER_SUBCHANNEL_SHIFT: u32 = 13;
const HEADER_MODE_SHIFT: u32 = 29;
const HEADER_COUNT_MASK: u32 = 0x1FFF;
const HEADER_METHOD_MASK: u32 = 0xFFF;
const HEADER_SUBCHANNEL_MASK: u32 = 7;
/// Modes whose header is followed by that many data words: increasing, non-increasing, increase-once.
const MODES_WITH_DATA: [u32; 3] = [1, 3, 5];

/// What a ring entry points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingEntry {
    pub address: u64,
    /// How many words.
    pub length: u32,
}

impl RingEntry {
    pub fn parse(entry: [u8; 8]) -> Self {
        let low = u32::from_le_bytes(entry[..4].try_into().expect("four bytes"));
        let high = u32::from_le_bytes(entry[4..].try_into().expect("four bytes"));
        Self {
            address: u64::from(low & !3) | u64::from(high & ENTRY_ADDRESS_HIGH_MASK) << 32,
            length: high >> ENTRY_LENGTH_SHIFT & ENTRY_LENGTH_MASK,
        }
    }
}

/// Builds a ring entry for `words` commands at a GPU address.
pub fn ring_entry(address: u64, words: u32) -> u64 {
    (address & !3) | u64::from(words) << ENTRY_LENGTH_SHIFT_IN_U64 | ENTRY_NO_PREFETCH_OR_FETCH
}

/// A header for `count` words to consecutive methods.
pub fn increasing(subchannel: u32, method: u32, count: u32) -> u32 {
    HEADER_INCREASING | count << HEADER_COUNT_SHIFT | subchannel << HEADER_SUBCHANNEL_SHIFT | method >> 2
}

/// What a header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub mode: u32,
    pub count: u32,
    pub method: u32,
    pub subchannel: u32,
}

impl Header {
    pub fn parse(word: u32) -> Self {
        Self {
            mode: word >> HEADER_MODE_SHIFT,
            count: word >> HEADER_COUNT_SHIFT & HEADER_COUNT_MASK,
            method: (word & HEADER_METHOD_MASK) << 2,
            subchannel: word >> HEADER_SUBCHANNEL_SHIFT & HEADER_SUBCHANNEL_MASK,
        }
    }

    pub fn has_data(&self) -> bool {
        MODES_WITH_DATA.contains(&self.mode)
    }

    /// Words from this header to the next.
    pub fn stride(&self) -> usize {
        1 + if self.has_data() { self.count as usize } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_reads_back() {
        let entry = RingEntry::parse(ring_entry(0x12_3456_7890, 7).to_le_bytes());
        assert_eq!(entry, RingEntry { address: 0x12_3456_7890, length: 7 });
    }

    #[test]
    fn a_header_reads_back() {
        let header = Header::parse(increasing(4, 0x304, 1));
        assert_eq!(header, Header { mode: 1, count: 1, method: 0x304, subchannel: 4 });
        assert_eq!(header.stride(), 2);
    }

    #[test]
    fn a_header_with_immediate_data_has_nothing_after_it() {
        assert_eq!(Header::parse(4 << 29 | 3 << 16).stride(), 1);
    }
}
