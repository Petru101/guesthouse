//! Two smaller answers to the adapter block's query, told apart from it by the size asked for.
//!
//! The user-mode driver asks for a per-adapter record of [`ADAPTER_RECORD_SIZE`] bytes and for a
//! directory of [`DIRECTORY_SIZE`] bytes, each after the usual header.

use super::bytes::{index_xor_sum, put_dword};

/// The per-adapter record.
pub const ADAPTER_RECORD_SIZE: usize = 0x260;
/// The record's own size, at its start.
const RECORD_SIZE: usize = 0x00;
/// The sum the guest checks the record against. See [`index_xor_sum`].
const RECORD_CHECKSUM: usize = 0x0C;
/// The adapter's registry key in the guest, as ASCII. The user-mode driver opens it, and makes no
/// device if it can't. The host can't know which instance key Windows gave the device, so this is
/// the display class's own key, which every Windows has. The driver's settings come through the
/// host's registry answers, so the values under it aren't needed.
const RECORD_REGISTRY_KEY: usize = 0xE8;
const DISPLAY_CLASS_KEY: &[u8] =
    br"\REGISTRY\MACHINE\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
const RECORD_FLAGS: usize = 0x1F4;
/// Without this bit the user-mode driver stops before it looks the chip up, and makes no device.
const RECORD_ADAPTER_USABLE: u32 = 1 << 7;

/// The directory of the blocks the kernel driver offers.
pub const DIRECTORY_SIZE: usize = 0x200;
/// The two entries the user-mode driver requires. The second reads as the adapter block's size,
/// [`super::adapter_info::BLOCK_SIZE`], with the low bit set; what the first stands for wasn't
/// found. The two entries after them are optional and left out.
const DIRECTORY_ENTRIES: [u32; 2] = [0x302, 0x3A01];

pub fn adapter_record() -> Vec<u8> {
    let mut record = vec![0u8; ADAPTER_RECORD_SIZE];
    put_dword(&mut record, RECORD_SIZE, ADAPTER_RECORD_SIZE as u32);
    put_dword(&mut record, RECORD_FLAGS, RECORD_ADAPTER_USABLE);
    record[RECORD_REGISTRY_KEY..RECORD_REGISTRY_KEY + DISPLAY_CLASS_KEY.len()]
        .copy_from_slice(DISPLAY_CLASS_KEY);
    let sum = index_xor_sum(&record, ADAPTER_RECORD_SIZE / 4, RECORD_CHECKSUM);
    put_dword(&mut record, RECORD_CHECKSUM, sum);
    record
}

pub fn directory() -> Vec<u8> {
    let mut directory = vec![0u8; DIRECTORY_SIZE];
    for (index, entry) in DIRECTORY_ENTRIES.into_iter().enumerate() {
        put_dword(&mut directory, index * 4, entry);
    }
    directory
}

#[cfg(test)]
mod tests {
    use super::super::bytes::dword;
    use super::*;

    #[test]
    fn the_record_is_marked_usable_and_its_sum_holds() {
        let record = adapter_record();
        assert_eq!(dword(&record, RECORD_SIZE), Some(0x260));
        assert_eq!(dword(&record, RECORD_FLAGS), Some(0x80));
        let sum = index_xor_sum(&record, ADAPTER_RECORD_SIZE / 4, RECORD_CHECKSUM);
        assert_eq!(dword(&record, RECORD_CHECKSUM), Some(sum));
        let key = &record[RECORD_REGISTRY_KEY..];
        let key = &key[..key.iter().position(|&byte| byte == 0).unwrap()];
        assert!(key.starts_with(br"\REGISTRY\MACHINE\SYSTEM\"), "{}", String::from_utf8_lossy(key));
    }

    #[test]
    fn the_directory_names_the_adapter_block() {
        let directory = directory();
        assert_eq!(dword(&directory, 4), Some(super::super::adapter_info::BLOCK_SIZE as u32 | 1));
        assert!(directory[8..].iter().all(|&byte| byte == 0));
    }
}
