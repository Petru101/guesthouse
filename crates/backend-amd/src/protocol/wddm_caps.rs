//! The two WDDM capability queries Direct3D asks before it makes a device.
//!
//! Unanswered, Direct3D 11 fails device creation with E_FAIL, because the guest's kernel returns
//! STATUS_NOT_SUPPORTED for each. The bits are the WDK's D3DKMT_WDDM_2_0_CAPS and
//! D3DKMT_WDDM_3_0_CAPS, and each one is a promise this backend keeps.

use guesthouse_backend::{Status, VendorQuery};

/// KMTQAITYPE_WDDM_2_0_CAPS.
pub const QUERY_WDDM_2_0_CAPS: u32 = 24;
/// KMTQAITYPE_WDDM_3_0_CAPS.
pub const QUERY_WDDM_3_0_CAPS: u32 = 77;

/// 64-bit atomics in shaders, which RDNA has.
const SUPPORT_64_BIT_ATOMICS: u32 = 1 << 0;
/// The card has its own page tables for each process. The guest's addresses are mapped in a VM of
/// the process's own on the host card.
const GPU_MMU_SUPPORTED: u32 = 1 << 1;
/// A flip may replace one that has not been shown yet.
const FLIP_OVERWRITE_SUPPORTED: u32 = 1 << 3;
/// A present needs no context of its own.
const SUPPORT_CONTEXTLESS_PRESENT: u32 = 1 << 4;
const WDDM_2_0_CAPS: u32 =
    SUPPORT_64_BIT_ATOMICS | GPU_MMU_SUPPORTED | FLIP_OVERWRITE_SUPPORTED | SUPPORT_CONTEXTLESS_PRESENT;
/// The one WDDM 3.0 bit is hardware flip queues, which nothing here gives.
const WDDM_3_0_CAPS: u32 = 0;

/// Answers either query. Any other is left alone.
pub fn answer(query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Option<Status> {
    let caps = match query.kind {
        QUERY_WDDM_2_0_CAPS => WDDM_2_0_CAPS,
        QUERY_WDDM_3_0_CAPS => WDDM_3_0_CAPS,
        _ => return None,
    };
    if query.size < size_of::<u32>() {
        return Some(Status::INVALID_PARAMETER);
    }
    let start = out.len();
    out.extend_from_slice(&caps.to_le_bytes());
    out.resize(start + query.size, 0);
    Some(Status::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(kind: u32, size: usize) -> (Option<Status>, Vec<u8>) {
        let mut out = Vec::new();
        let status = answer(&VendorQuery { kind, size, data: &[], process_is_32_bit: false }, &mut out);
        (status, out)
    }

    #[test]
    fn wddm_2_0_promises_the_gpu_mmu_and_wddm_3_0_nothing() {
        let (status, out) = ask(QUERY_WDDM_2_0_CAPS, 4);
        assert_eq!(status, Some(Status::SUCCESS));
        assert_eq!(u32::from_le_bytes(out[..4].try_into().unwrap()), 0b11011);
        assert_eq!(ask(QUERY_WDDM_3_0_CAPS, 4), (Some(Status::SUCCESS), vec![0; 4]));
    }

    #[test]
    fn a_short_buffer_is_refused_and_other_queries_are_left_alone() {
        assert_eq!(ask(QUERY_WDDM_2_0_CAPS, 2).0, Some(Status::INVALID_PARAMETER));
        assert_eq!(ask(QUERY_WDDM_2_0_CAPS + 1, 4), (None, Vec::new()));
    }
}
