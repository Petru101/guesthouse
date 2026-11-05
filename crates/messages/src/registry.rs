//! Registry lookups the guest forwards to the host, carried inside an adapter query.
//!
//! The guest asks by name for values under the adapter's key, or for one of two driver paths. The
//! query and its answer share a buffer: a fixed block, then room for the value.

use crate::wire::{Malformed, set_u32, u32_at, utf16_until_zero};

const QUERY_KIND: usize = 0;
const FLAGS: usize = 4;
const NAME: usize = 8;
const VALUE_TYPE: usize = 528;
const OUTPUT_SIZE: usize = 536;
const OUTPUT_STATUS: usize = 540;
const OUTPUT: usize = 544;

const KIND_DRIVER_STORE_PATH: u32 = 2;
const KIND_DRIVER_IMAGE_PATH: u32 = 3;
const FLAG_TRANSLATE_PATH: u32 = 1;
const STATUS_OVERFLOW: u32 = 1;

/// What the real host adds to the size it reports for a path the guest will rewrite, as the guest
/// grows it by a folder prefix and counts a device prefix it does not copy.
const TRANSLATED_PATH_ROOM: usize = 0x10;

/// The value types the guest distinguishes.
pub const TYPE_STRING: u32 = 1;
pub const TYPE_NUMBER: u32 = 4;
pub const TYPE_STRING_LIST: u32 = 7;

/// What is being looked up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryName {
    DriverStorePath,
    DriverImagePath,
    Value(String),
}

/// A parsed lookup.
#[derive(Debug, Clone)]
pub struct RegistryQuery<'a> {
    pub name: RegistryName,
    pub value_type: u32,
    translate: bool,
    buffer: &'a [u8],
}

impl<'a> RegistryQuery<'a> {
    pub fn parse(buffer: &'a [u8]) -> Result<Self, Malformed> {
        let kind = u32_at(buffer, QUERY_KIND)?;
        let flags = u32_at(buffer, FLAGS)?;
        let value_type = u32_at(buffer, VALUE_TYPE)?;
        let (name, is_path) = match kind {
            KIND_DRIVER_STORE_PATH => (RegistryName::DriverStorePath, true),
            KIND_DRIVER_IMAGE_PATH => (RegistryName::DriverImagePath, true),
            _ => (RegistryName::Value(utf16_until_zero(&buffer[NAME..VALUE_TYPE])), false),
        };
        Ok(Self { name, value_type, translate: is_path || flags & FLAG_TRANSLATE_PATH != 0, buffer })
    }

    /// The buffer as it came, for a lookup that has no answer.
    pub fn unchanged(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self.buffer);
    }

    /// Writes `value` after the fixed block, or says how much room it needs if it does not fit.
    pub fn answer(&self, value: &[u8], out: &mut Vec<u8>) -> Result<(), Malformed> {
        if self.buffer.len() < OUTPUT {
            return Err(Malformed);
        }
        // The guest rewrites string lists in place, and leaves the last zero to what is here, so
        // the rest of the buffer must be clear.
        let start = out.len();
        out.extend_from_slice(&self.buffer[..OUTPUT]);
        out.resize(start + self.buffer.len(), 0);
        let answer = &mut out[start..];
        if value.len() > self.buffer.len() - OUTPUT {
            let room = if self.translate { TRANSLATED_PATH_ROOM } else { 0 };
            set_u32(answer, OUTPUT_SIZE, (value.len() + room) as u32);
            set_u32(answer, OUTPUT_STATUS, STATUS_OVERFLOW);
        } else {
            set_u32(answer, OUTPUT_SIZE, value.len() as u32);
            set_u32(answer, OUTPUT_STATUS, 0);
            answer[OUTPUT..OUTPUT + value.len()].copy_from_slice(value);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::put_utf16_terminated;

    fn buffer(kind: u32, flags: u32, name: &str, room: usize) -> Vec<u8> {
        let mut bytes = vec![0; OUTPUT + room];
        set_u32(&mut bytes, QUERY_KIND, kind);
        set_u32(&mut bytes, FLAGS, flags);
        let mut text = Vec::new();
        put_utf16_terminated(&mut text, name);
        bytes[NAME..NAME + text.len()].copy_from_slice(&text);
        set_u32(&mut bytes, VALUE_TYPE, TYPE_STRING);
        bytes
    }

    #[test]
    fn a_value_is_found_by_name() {
        let bytes = buffer(0, 0, "SomeValue", 0);
        let query = RegistryQuery::parse(&bytes).unwrap();
        assert_eq!(query.name, RegistryName::Value("SomeValue".into()));
    }

    #[test]
    fn a_value_that_fits_is_copied_in() {
        let bytes = buffer(0, 0, "x", 8);
        let mut out = Vec::new();
        RegistryQuery::parse(&bytes).unwrap().answer(b"abcd", &mut out).unwrap();
        assert_eq!(out.len(), bytes.len());
        assert_eq!(u32_at(&out, OUTPUT_SIZE), Ok(4));
        assert_eq!(&out[OUTPUT..OUTPUT + 4], b"abcd");
    }

    #[test]
    fn too_little_room_asks_for_more() {
        let bytes = buffer(0, 0, "x", 2);
        let mut out = Vec::new();
        RegistryQuery::parse(&bytes).unwrap().answer(b"abcd", &mut out).unwrap();
        assert_eq!(u32_at(&out, OUTPUT_SIZE), Ok(4));
        assert_eq!(u32_at(&out, OUTPUT_STATUS), Ok(STATUS_OVERFLOW));
    }

    #[test]
    fn a_path_asks_for_room_to_be_rewritten() {
        let bytes = buffer(KIND_DRIVER_STORE_PATH, 0, "", 2);
        let mut out = Vec::new();
        RegistryQuery::parse(&bytes).unwrap().answer(b"abcd", &mut out).unwrap();
        assert_eq!(u32_at(&out, OUTPUT_SIZE), Ok(4 + TRANSLATED_PATH_ROOM as u32));
    }

    #[test]
    fn a_short_buffer_is_malformed() {
        assert!(RegistryQuery::parse(&[0; 100]).is_err());
    }
}
