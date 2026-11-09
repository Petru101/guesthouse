//! The short adapter block AMD's Windows driver asks for when it opens a paravirtual adapter.
//!
//! The same adapter query as the full block, type 0, has two parts in the kernel driver: this
//! header of [`HEADER_SIZE`] bytes, then, if the buffer is long enough, the full block from
//! [`super::adapter_info`]. The user-mode driver asks for the header alone when it opens the adapter,
//! and stops there if the query fails.
//!
//! For a guest, the kernel fills only a few fields: the version, the card's PCI ids, which adapter
//! of the driver's this is, the WDDM version, a flag, the size asked for, and a checksum over the
//! whole header. Everything else stays zero.

use super::bytes::{index_xor_sum, put_dword, put_word};
use crate::info::DeviceInfo;
use crate::ioctl::AMD_VENDOR;

/// The header's size, which is also the shortest buffer the query takes.
pub const HEADER_SIZE: usize = 0x960;

const VERSION: usize = 0x000;
/// 2.0, major in the high half.
const VERSION_VALUE: u32 = 0x0002_0000;
/// Two dwords the kernel sets to 1 for a guest: as far as can be told, the adapter count and the
/// count of adapters linked to it.
const ADAPTER_COUNT: usize = 0x004;
const LINKED_COUNT: usize = 0x008;
const ONE_ADAPTER: u32 = 1;
/// The PCI ids, as the kernel reads them from the adapter's hardware id, one word each.
const VENDOR_ID: usize = 0x00C;
const DEVICE_ID: usize = 0x014;
const SUBSYSTEM_ID: usize = 0x01C;
const SUBSYSTEM_VENDOR_ID: usize = 0x024;
/// This adapter's place among the driver's adapters. There is one.
const ADAPTER_INDEX: usize = 0x02C;
const FIRST_ADAPTER: u32 = 0;
/// Flags. A guest's header has one bit set.
const FLAGS: usize = 0x85C;
const GUEST_FLAG: u32 = 0x200;
const CHECKSUM: usize = 0x8AC;
const WDDM_VERSION: usize = 0x8B0;
/// WDDM 3.2, the newest the driver knows and what a Windows 11 guest runs.
const WDDM_VERSION_VALUE: u32 = 0x3200;
/// The size of the buffer the guest gave, header and all.
const ASKED_SIZE: usize = 0x95C;

/// The header for a buffer of `asked` bytes.
pub fn build(info: &DeviceInfo, asked: usize) -> Vec<u8> {
    let mut header = vec![0u8; HEADER_SIZE];
    put_dword(&mut header, VERSION, VERSION_VALUE);
    put_dword(&mut header, ADAPTER_COUNT, ONE_ADAPTER);
    put_dword(&mut header, LINKED_COUNT, ONE_ADAPTER);
    let subsystem = info.details.subsystem;
    put_word(&mut header, VENDOR_ID, AMD_VENDOR);
    put_word(&mut header, DEVICE_ID, info.device_id);
    put_word(&mut header, SUBSYSTEM_ID, (subsystem >> 16) as u16);
    put_word(&mut header, SUBSYSTEM_VENDOR_ID, subsystem as u16);
    put_dword(&mut header, ADAPTER_INDEX, FIRST_ADAPTER);
    put_dword(&mut header, FLAGS, GUEST_FLAG);
    put_dword(&mut header, WDDM_VERSION, WDDM_VERSION_VALUE);
    put_dword(&mut header, ASKED_SIZE, asked as u32);
    let sum = index_xor_sum(&header, HEADER_SIZE / 4, CHECKSUM);
    put_dword(&mut header, CHECKSUM, sum);
    header
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::bytes::dword;

    fn info() -> DeviceInfo {
        let mut info = crate::protocol::adapter_info::tests::small_card();
        info.device_id = 0x13C0;
        info.details.subsystem = 0x8877_1043;
        info
    }

    #[test]
    fn the_header_carries_the_cards_ids_and_holds_its_checksum() {
        let header = build(&info(), HEADER_SIZE);
        assert_eq!(header.len(), HEADER_SIZE);
        assert_eq!(dword(&header, VERSION), Some(VERSION_VALUE));
        assert_eq!(header[VENDOR_ID..VENDOR_ID + 2], 0x1002u16.to_le_bytes());
        assert_eq!(header[DEVICE_ID..DEVICE_ID + 2], 0x13C0u16.to_le_bytes());
        assert_eq!(header[SUBSYSTEM_ID..SUBSYSTEM_ID + 2], 0x8877u16.to_le_bytes());
        assert_eq!(header[SUBSYSTEM_VENDOR_ID..SUBSYSTEM_VENDOR_ID + 2], 0x1043u16.to_le_bytes());
        assert_eq!(dword(&header, ASKED_SIZE), Some(HEADER_SIZE as u32));
        assert_eq!(dword(&header, CHECKSUM), Some(index_xor_sum(&header, HEADER_SIZE / 4, CHECKSUM)));
    }

    #[test]
    fn the_asked_size_is_the_whole_buffer() {
        let header = build(&info(), HEADER_SIZE + 0x3A00);
        assert_eq!(dword(&header, ASKED_SIZE), Some((HEADER_SIZE + 0x3A00) as u32));
    }
}
