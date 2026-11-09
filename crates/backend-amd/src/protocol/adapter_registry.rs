//! The four strings Windows asks for with adapter query type 8.
//!
//! The query is the WDK's `KMTQAITYPE_ADAPTERREGISTRYINFO`: the adapter name, the BIOS string, the
//! DAC type and the chip type, each one `MAX_PATH` of UTF-16. The guest drops an adapter whose chip
//! type does not contain "AMD". The BIOS and DAC strings stay empty, because nothing on the host
//! names them and the guest does not require them to keep the adapter.

use guesthouse_backend::{Status, VendorQuery};

/// Adapter query type 8, `KMTQAITYPE_ADAPTERREGISTRYINFO`.
pub const QUERY_TYPE: u32 = 8;
/// Windows' `MAX_PATH`, in UTF-16 code units, the width of each string.
const FIELD_CHARS: usize = 260;
/// The four strings, in the order the query lays them out.
const FIELD_COUNT: usize = 4;
const ADAPTER_NAME: &str = "AMD Radeon Graphics";
/// Has to contain "AMD", which is what the guest looks for.
const CHIP_TYPE: &str = "AMD Radeon Graphics";

/// Answers the registry-info query. Any other query is left alone.
pub fn answer(query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Option<Status> {
    if query.kind != QUERY_TYPE {
        return None;
    }
    let bytes = FIELD_CHARS * 2 * FIELD_COUNT;
    if query.size < bytes {
        log::info!("adapter registry refused: the guest asked for {} bytes", query.size);
        return Some(Status::INVALID_PARAMETER);
    }
    log::debug!("answered the adapter registry, chip type {CHIP_TYPE}");
    let start = out.len();
    for text in [ADAPTER_NAME, "", "", CHIP_TYPE] {
        write_field(out, text);
    }
    out.resize(start + query.size, 0);
    Some(Status::SUCCESS)
}

fn write_field(out: &mut Vec<u8>, text: &str) {
    let mut units: Vec<u16> = text.encode_utf16().take(FIELD_CHARS - 1).collect();
    units.resize(FIELD_CHARS, 0);
    out.extend(units.into_iter().flat_map(u16::to_le_bytes));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(out: &[u8], index: usize) -> String {
        let start = index * FIELD_CHARS * 2;
        let units = out[start..start + FIELD_CHARS * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|unit| u16::from_le_bytes(*unit));
        String::from_utf16_lossy(&units.take_while(|&unit| unit != 0).collect::<Vec<_>>())
    }

    #[test]
    fn the_chip_type_names_amd_and_the_other_query_is_left_alone() {
        let query = VendorQuery { kind: QUERY_TYPE, size: 2080, data: &[], process_is_32_bit: false };
        let mut out = Vec::new();
        assert_eq!(answer(&query, &mut out), Some(Status::SUCCESS));
        assert_eq!(out.len(), 2080);
        assert_eq!(field(&out, 0), ADAPTER_NAME);
        assert_eq!(field(&out, 1), "");
        assert_eq!(field(&out, 2), "");
        assert!(field(&out, 3).contains("AMD"));
        let other = VendorQuery { kind: 0, size: 2080, data: &[], process_is_32_bit: false };
        assert!(answer(&other, &mut Vec::new()).is_none());
    }

    #[test]
    fn a_short_ask_is_refused() {
        let query = VendorQuery { kind: QUERY_TYPE, size: 16, data: &[], process_is_32_bit: false };
        let mut out = Vec::new();
        assert_eq!(answer(&query, &mut out), Some(Status::INVALID_PARAMETER));
        assert!(out.is_empty());
    }
}
