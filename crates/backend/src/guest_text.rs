//! Text in the form a Windows guest reads it, and the one adapter query every vendor answers the
//! same way: where the user-mode driver is.

use crate::VendorQuery;

/// Adapter query type 1, the WDK's KMTQAITYPE_UMDRIVERNAME.
pub const QUERY_USER_DRIVER_NAME: u32 = 1;

/// The version dword the guest puts in front of the path, which the answer keeps.
const VERSION_FIELD: usize = 4;

/// UTF-16, little-endian, with a closing zero.
pub fn utf16_zero(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// A REG_MULTI_SZ: each string closed with a zero, then one more zero.
pub fn utf16_list<S: AsRef<str>>(values: &[S]) -> Vec<u8> {
    let mut bytes: Vec<u8> = values.iter().flat_map(|text| utf16_zero(text.as_ref())).collect();
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

/// Answers the user-mode driver query with `path`, keeping the guest's version dword and filling
/// the rest of the guest's buffer with zeros.
pub fn user_driver_name(query: &VendorQuery<'_>, path: &str, out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(query.data.get(..VERSION_FIELD).unwrap_or(&[0; VERSION_FIELD]));
    out.extend(utf16_zero(path));
    out.resize((start + query.data.len()).max(out.len()), 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_after_version(out: &[u8]) -> String {
        let units = out[VERSION_FIELD..].as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit));
        String::from_utf16_lossy(&units.take_while(|&unit| unit != 0).collect::<Vec<_>>())
    }

    #[test]
    fn the_driver_name_keeps_the_version_and_fills_the_buffer() {
        let data = [9, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];
        let query = VendorQuery {
            kind: QUERY_USER_DRIVER_NAME,
            size: data.len(),
            data: &data,
            process_is_32_bit: false,
        };
        let mut out = Vec::new();
        user_driver_name(&query, "C:\\d.dll", &mut out);
        assert_eq!(out[..VERSION_FIELD], [9, 0, 0, 0]);
        assert_eq!(text_after_version(&out), "C:\\d.dll");
        assert_eq!(out.len(), data.len());
    }

    #[test]
    fn a_list_ends_with_two_zeros() {
        assert_eq!(utf16_list(&["a", "b"]), [b'a', 0, 0, 0, b'b', 0, 0, 0, 0, 0]);
    }
}
