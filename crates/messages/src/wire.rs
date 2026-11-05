//! Reading and writing little-endian fields, with bounds checks instead of panics.
//!
//! The guest is not trusted to send well-formed messages, so every read can fail.

/// A message too short for the field being read from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the message is too short for a field it should have")
    }
}

impl std::error::Error for Malformed {}

pub fn u8_at(bytes: &[u8], at: usize) -> Result<u8, Malformed> {
    bytes.get(at).copied().ok_or(Malformed)
}

fn array_at<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], Malformed> {
    let tail = bytes.get(at..).ok_or(Malformed)?;
    tail.first_chunk::<N>().copied().ok_or(Malformed)
}

pub fn u32_at(bytes: &[u8], at: usize) -> Result<u32, Malformed> {
    array_at(bytes, at).map(u32::from_le_bytes)
}

pub fn i32_at(bytes: &[u8], at: usize) -> Result<i32, Malformed> {
    array_at(bytes, at).map(i32::from_le_bytes)
}

pub fn u64_at(bytes: &[u8], at: usize) -> Result<u64, Malformed> {
    array_at(bytes, at).map(u64::from_le_bytes)
}

/// As much of `length` bytes from `at` as the message has, which may be nothing.
pub fn clamped(bytes: &[u8], at: usize, length: usize) -> &[u8] {
    let start = at.min(bytes.len());
    let end = at.saturating_add(length).min(bytes.len());
    &bytes[start..end]
}

/// A run of 32-bit values inside a message, checked once and then read freely.
#[derive(Debug, Clone, Copy, Default)]
pub struct U32List<'a>(&'a [u8]);

impl<'a> U32List<'a> {
    pub fn at(bytes: &'a [u8], at: usize, count: usize) -> Result<Self, Malformed> {
        Ok(Self(checked_run(bytes, at, count, 4)?))
    }

    pub fn len(&self) -> usize {
        self.0.len() / 4
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + Clone + 'a {
        self.0.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word))
    }
}

/// A run of 64-bit values inside a message, checked once and then read freely.
#[derive(Debug, Clone, Copy, Default)]
pub struct U64List<'a>(&'a [u8]);

impl<'a> U64List<'a> {
    pub fn at(bytes: &'a [u8], at: usize, count: usize) -> Result<Self, Malformed> {
        Ok(Self(checked_run(bytes, at, count, 8)?))
    }

    pub fn len(&self) -> usize {
        self.0.len() / 8
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = u64> + Clone + 'a {
        self.0.as_chunks::<8>().0.iter().map(|word| u64::from_le_bytes(*word))
    }
}

fn checked_run(bytes: &[u8], at: usize, count: usize, size: usize) -> Result<&[u8], Malformed> {
    let end = count.checked_mul(size).and_then(|length| length.checked_add(at)).ok_or(Malformed)?;
    bytes.get(at..end).ok_or(Malformed)
}

pub fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Overwrites a field already in the buffer. Answers are built by the caller at known sizes, so
/// a write past the end is a bug here and not bad input.
pub fn set_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn set_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// Rounds up to a power of two.
pub const fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

/// Pads with zeros until the buffer is at least `length` long. Never shortens it.
pub fn pad_to(out: &mut Vec<u8>, length: usize) {
    if out.len() < length {
        out.resize(length, 0);
    }
}

/// Writes text as UTF-16 into a fixed field of `chars` characters. It always ends in a zero, so
/// long text loses its tail.
pub fn put_utf16_field(out: &mut Vec<u8>, text: &str, chars: usize) {
    let start = out.len();
    for unit in text.encode_utf16().take(chars - 1) {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out.resize(start + chars * 2, 0);
}

/// Text as UTF-16 with a closing zero, the form of the paths and names the guest reads.
pub fn put_utf16_terminated(out: &mut Vec<u8>, text: &str) {
    for unit in text.encode_utf16().chain(std::iter::once(0)) {
        out.extend_from_slice(&unit.to_le_bytes());
    }
}

/// Reads UTF-16 up to the first zero, replacing what does not decode.
pub fn utf16_until_zero(bytes: &[u8]) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit));
    let units = units.take_while(|&unit| unit != 0);
    char::decode_utf16(units).map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_stop_at_the_end() {
        let bytes = [1, 0, 0, 0, 2];
        assert_eq!(u32_at(&bytes, 0), Ok(1));
        assert_eq!(u32_at(&bytes, 2), Err(Malformed));
        assert_eq!(u32_at(&bytes, 100), Err(Malformed));
        assert_eq!(u64_at(&bytes, 0), Err(Malformed));
        assert_eq!(u8_at(&bytes, 4), Ok(2));
    }

    #[test]
    fn clamped_takes_what_there_is() {
        assert_eq!(clamped(b"abcd", 2, 10), b"cd");
        assert_eq!(clamped(b"abcd", 10, 10), b"");
        assert_eq!(clamped(b"abcd", 1, usize::MAX), b"bcd");
    }

    #[test]
    fn lists_check_their_whole_length_up_front() {
        let bytes = [1, 0, 0, 0, 2, 0, 0, 0];
        let list = U32List::at(&bytes, 0, 2).unwrap();
        assert_eq!(list.iter().collect::<Vec<_>>(), [1, 2]);
        assert!(U32List::at(&bytes, 0, 3).is_err());
        assert!(U32List::at(&bytes, 4, usize::MAX).is_err());
        assert_eq!(U64List::at(&bytes, 0, 1).unwrap().iter().next(), Some(0x2_0000_0001));
    }

    #[test]
    fn a_fixed_text_field_is_cut_and_zero_ended() {
        let mut out = Vec::new();
        put_utf16_field(&mut out, "abcdef", 3);
        assert_eq!(out, [b'a', 0, b'b', 0, 0, 0]);
        let mut out = Vec::new();
        put_utf16_field(&mut out, "a", 3);
        assert_eq!(out, [b'a', 0, 0, 0, 0, 0]);
    }

    #[test]
    fn text_reads_back_up_to_the_zero() {
        let mut out = Vec::new();
        put_utf16_terminated(&mut out, "Name");
        out.extend_from_slice(b"x\0");
        assert_eq!(utf16_until_zero(&out), "Name");
    }

    #[test]
    fn padding_never_shortens() {
        let mut out = vec![1, 2, 3];
        pad_to(&mut out, 2);
        assert_eq!(out.len(), 3);
        pad_to(&mut out, 8);
        assert_eq!(out, [1, 2, 3, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn align_up_rounds_to_the_next_multiple() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(8, 8), 8);
    }
}
