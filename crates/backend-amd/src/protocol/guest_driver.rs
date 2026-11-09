//! Where the guest finds AMD's user-mode driver.
//!
//! The paravirtual adapter's kernel driver is Windows' own. The user-mode driver is AMD's, and the
//! guest asks for its path with adapter query type 1. It keeps the version dword it sent and reads
//! a UTF-16 path after that. The same query is how a guest is pointed at its driver today.
//!
//! The files have to be in the guest, under the host driver store, at the path given here. Nothing
//! here copies them.

use guesthouse_backend::{RegistryName, RegistryValue, VendorQuery};
use guesthouse_messages::registry::TYPE_STRING_LIST;
use std::sync::atomic::{AtomicBool, Ordering};

/// Query type 1: the user-mode driver's path.
pub const QUERY_NAME: u32 = 1;

/// The one Adrenalin release these answers were written for. Another release is refused at start.
pub const GUEST_RELEASE: &str = "32.0.32015.12012";

/// Where the guest is told the driver lives. It rewrites `DriverStore` to its host driver store.
pub const DRIVER_STORE: &str = "C:\\Windows\\System32\\DriverStore\\FileRepository\\amdxc.inf_amd64_gpupv";

const USER_DRIVER: &str = "amdxc64.dll";
const USER_DRIVER_32_BIT: &str = "amdxc32.dll";
const KERNEL_DRIVER: &str = "amdkmdag.sys";
/// The version dword the guest sent, which the answer keeps in front of the path.
const VERSION_FIELD: usize = 4;

/// What DXCore looks up, by the lower-case name the core asks with. The strings are Microsoft's
/// public attribute GUIDs: a GPU, then Direct3D 11, Direct3D 12, compute, machine learning and media.
/// Without the list, DXCore has nothing to say this adapter can draw.
const DXCORE_ATTRIBUTES: &[&str] = &[
    "{B69EB219-3DED-4464-979F-A00BD4687006}",
    "{8C47866B-7583-450D-F0F0-6BADA895AF4B}",
    "{0C9ECE4D-2F6E-4F01-8C96-E89E331B47B1}",
    "{248E2800-A793-4724-ABAA-23A6DE1BE090}",
    "{B71B0D41-1088-422F-A27C-0250B7D3A988}",
    "{8EB2C848-82F6-4B49-AA87-AECFCF0174C6}",
];

static LOGGED_DRIVER_NAME: AtomicBool = AtomicBool::new(false);

fn utf16_zero(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// Writes the user-mode driver path into `out`, keeping the guest's version dword.
pub fn write_name(query: &VendorQuery<'_>, out: &mut Vec<u8>) {
    let name = if query.process_is_32_bit { USER_DRIVER_32_BIT } else { USER_DRIVER };
    let path = format!("{DRIVER_STORE}\\{name}");
    // The guest asks again for every process it starts. The first answer is the one that matters.
    if !LOGGED_DRIVER_NAME.swap(true, Ordering::Relaxed) {
        log::info!("guest asked for the user-mode driver, answered with {name}");
    } else {
        log::debug!("guest asked for the user-mode driver, answered with {name}");
    }
    let start = out.len();
    out.extend_from_slice(query.data.get(..VERSION_FIELD).unwrap_or(&[0; VERSION_FIELD]));
    out.extend(utf16_zero(&path));
    out.resize((start + query.data.len()).max(out.len()), 0);
}

fn string_list(values: &[&str]) -> Vec<u8> {
    let mut bytes: Vec<u8> = values.iter().flat_map(|text| utf16_zero(text)).collect();
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

/// The driver paths, and the attribute list DXCore asks for. Any other registry name has no answer.
pub fn registry(name: &RegistryName) -> Option<RegistryValue> {
    match name {
        RegistryName::DriverStorePath => {
            Some(RegistryValue { value_type: None, bytes: utf16_zero(DRIVER_STORE) })
        }
        RegistryName::DriverImagePath => {
            let path = format!("{DRIVER_STORE}\\{KERNEL_DRIVER}");
            Some(RegistryValue { value_type: None, bytes: utf16_zero(&path) })
        }
        RegistryName::Value(name) if name.eq_ignore_ascii_case("dxcoreattributes") => {
            Some(RegistryValue { value_type: Some(TYPE_STRING_LIST), bytes: string_list(DXCORE_ATTRIBUTES) })
        }
        RegistryName::Value(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_of(out: &[u8]) -> String {
        let units = out[VERSION_FIELD..].as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit));
        let units: Vec<u16> = units.take_while(|&unit| unit != 0).collect();
        String::from_utf16_lossy(&units)
    }

    #[test]
    fn the_driver_name_keeps_the_guests_version_and_names_the_64_bit_driver() {
        let mut out = Vec::new();
        let data = [9, 0, 0, 0, 1, 2, 3, 4];
        let query = VendorQuery { kind: QUERY_NAME, size: data.len(), data: &data, process_is_32_bit: false };
        write_name(&query, &mut out);
        assert_eq!(out[0], 9);
        let path = name_of(&out);
        assert!(path.ends_with(USER_DRIVER), "{path}");
        assert!(path.starts_with(DRIVER_STORE), "{path}");
    }

    #[test]
    fn a_32_bit_process_gets_the_32_bit_driver() {
        let mut out = Vec::new();
        let query = VendorQuery { kind: QUERY_NAME, size: 4, data: &[0; 4], process_is_32_bit: true };
        write_name(&query, &mut out);
        assert!(name_of(&out).ends_with(USER_DRIVER_32_BIT));
    }

    #[test]
    fn the_driver_paths_are_under_the_store_and_nothing_else_is_answered() {
        let store = registry(&RegistryName::DriverStorePath).unwrap();
        let image = registry(&RegistryName::DriverImagePath).unwrap();
        assert_eq!(store.bytes, utf16_zero(DRIVER_STORE));
        assert!(name_text(&image.bytes).ends_with(KERNEL_DRIVER));
        assert!(registry(&RegistryName::Value("vulkan".into())).is_none());
        let attributes = registry(&RegistryName::Value("dxcoreattributes".into())).unwrap();
        assert_eq!(attributes.value_type, Some(TYPE_STRING_LIST));
        let units = attributes.bytes.as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit));
        let text = String::from_utf16_lossy(&units.collect::<Vec<_>>());
        assert!(text.contains("B69EB219-3DED-4464-979F-A00BD4687006"), "{text}");
        assert!(text.contains("8C47866B-7583-450D-F0F0-6BADA895AF4B"), "{text}");
    }

    fn name_text(bytes: &[u8]) -> String {
        let units = bytes.as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit));
        String::from_utf16_lossy(&units.take_while(|&unit| unit != 0).collect::<Vec<_>>())
    }
}
