//! The guest page ranges at the front of a GPA-direct payload.

use crate::Truncated;

const COUNT_SIZE: usize = 4;
const RANGE_SIZE: usize = 16;

/// A stretch of guest physical memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpaRange {
    pub base: u64,
    pub length: u64,
}

/// A borrowed list of ranges, read on demand so that callers that ignore it pay nothing.
#[derive(Debug, Clone, Copy)]
pub struct GpaRanges<'a> {
    raw: &'a [u8],
}

impl GpaRanges<'_> {
    pub fn len(&self) -> usize {
        self.raw.len() / RANGE_SIZE
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = GpaRange> + '_ {
        self.raw.chunks_exact(RANGE_SIZE).map(|range| {
            let (base, length) = range.split_at(RANGE_SIZE / 2);
            GpaRange {
                base: u64::from_le_bytes(base.try_into().expect("half of a range")),
                length: u64::from_le_bytes(length.try_into().expect("half of a range")),
            }
        })
    }
}

/// Splits a payload that has ranges into the ranges and the message after them.
pub fn split_ranges(payload: &[u8]) -> Result<(GpaRanges<'_>, &[u8]), Truncated> {
    let count = payload.first_chunk::<COUNT_SIZE>().ok_or(Truncated)?;
    let count = u32::from_le_bytes(*count) as usize;
    let end = count.checked_mul(RANGE_SIZE).and_then(|size| size.checked_add(COUNT_SIZE)).ok_or(Truncated)?;
    let raw = payload.get(COUNT_SIZE..end).ok_or(Truncated)?;
    Ok((GpaRanges { raw }, &payload[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(ranges: &[(u64, u64)], message: &[u8]) -> Vec<u8> {
        let mut out = (ranges.len() as u32).to_le_bytes().to_vec();
        for (base, length) in ranges {
            out.extend_from_slice(&base.to_le_bytes());
            out.extend_from_slice(&length.to_le_bytes());
        }
        out.extend_from_slice(message);
        out
    }

    #[test]
    fn message_follows_the_ranges() {
        let bytes = payload(&[(0x1000, 0x2000), (0x8000, 0x10)], b"hello");
        let (ranges, message) = split_ranges(&bytes).unwrap();
        assert_eq!(message, b"hello");
        assert_eq!(ranges.len(), 2);
        let second = ranges.iter().nth(1).unwrap();
        assert_eq!(second, GpaRange { base: 0x8000, length: 0x10 });
    }

    #[test]
    fn no_ranges_is_fine() {
        let bytes = payload(&[], b"x");
        let (ranges, message) = split_ranges(&bytes).unwrap();
        assert!(ranges.is_empty());
        assert_eq!(message, b"x");
    }

    #[test]
    fn a_count_past_the_end_is_refused() {
        let mut bytes = payload(&[(1, 2)], b"");
        bytes[0] = 2;
        assert_eq!(split_ranges(&bytes).unwrap_err(), Truncated);
        assert_eq!(split_ranges(&[1, 0]).unwrap_err(), Truncated);
        assert_eq!(split_ranges(&u32::MAX.to_le_bytes()).unwrap_err(), Truncated);
    }
}
