//! Where the guest finds AMD's user-mode driver.
//!
//! The paravirtual adapter's kernel driver is Windows' own. The user-mode driver is AMD's, and the
//! guest asks for its path with adapter query type 1. It keeps the version dword it sent and reads
//! a UTF-16 path after that. The same query is how a guest is pointed at its driver today.
//!
//! The files have to be in the guest, under the host driver store, at the path given here. Nothing
//! here copies them.

use guesthouse_backend::{RegistryName, RegistryValue, VendorQuery};

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

fn utf16_zero(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// Writes the user-mode driver path into `out`, keeping the guest's version dword.
pub fn write_name(query: &VendorQuery<'_>, out: &mut Vec<u8>) {
    let name = if query.process_is_32_bit { USER_DRIVER_32_BIT } else { USER_DRIVER };
    let path = format!("{DRIVER_STORE}\\{name}");
    log::info!("guest asked for the user-mode driver, answered with {name}");
    let start = out.len();
    out.extend_from_slice(query.data.get(..VERSION_FIELD).unwrap_or(&[0; VERSION_FIELD]));
    out.extend(utf16_zero(&path));
    out.resize((start + query.data.len()).max(out.len()), 0);
}

/// The two driver paths. Any other registry name has no answer.
pub fn registry(name: &RegistryName) -> Option<RegistryValue> {
    let path = match name {
        RegistryName::DriverStorePath => DRIVER_STORE.to_owned(),
        RegistryName::DriverImagePath => format!("{DRIVER_STORE}\\{KERNEL_DRIVER}"),
        RegistryName::Value(_) => return None,
    };
    Some(RegistryValue { value_type: None, bytes: utf16_zero(&path) })
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
    }

    fn name_text(bytes: &[u8]) -> String {
        let units = bytes.as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit));
        String::from_utf16_lossy(&units.take_while(|&unit| unit != 0).collect::<Vec<_>>())
    }
}
