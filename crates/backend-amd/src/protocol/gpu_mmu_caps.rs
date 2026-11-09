//! The GPU MMU query, KMTQAITYPE_GPUMMU_CAPS, which AMD's OpenGL driver asks as it makes a context.
//!
//! The answer is the WDK's D3DKMT_QUERY_GPUMMU_CAPS: the physical adapter, then
//! D3DKMT_GPUMMU_CAPS, its flags and how many bits a GPU address has. The NVIDIA backend gives the
//! same shape, with its own card's numbers.

use guesthouse_backend::{GPU_RANGES, Status, VendorQuery};

pub const QUERY_GPU_MMU_CAPS: u32 = 34;

/// There is one physical adapter behind the guest's.
const PHYSICAL_ADAPTER: u32 = 0;
/// A mapping can be made read-only, as the guest asks.
const READ_ONLY_MEMORY_SUPPORTED: u32 = 1 << 0;
/// Memory the guest asks to be coherent with the CPU's caches is mapped so.
const CACHE_COHERENT_MEMORY_SUPPORTED: u32 = 1 << 2;
const FLAGS: u32 = READ_ONLY_MEMORY_SUPPORTED | CACHE_COHERENT_MEMORY_SUPPORTED;

/// Enough bits to reach the end of the last range the guest is given.
fn address_bits() -> u32 {
    let end = GPU_RANGES.iter().map(|range| range.end).max().unwrap_or(0);
    u64::BITS - end.saturating_sub(1).leading_zeros()
}

/// Answers the query. Any other is left alone.
pub fn answer(query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Option<Status> {
    if query.kind != QUERY_GPU_MMU_CAPS {
        return None;
    }
    let answer = [PHYSICAL_ADAPTER, FLAGS, address_bits()];
    if query.size < size_of_val(&answer) {
        return Some(Status::INVALID_PARAMETER);
    }
    let start = out.len();
    answer.iter().for_each(|word| out.extend_from_slice(&word.to_le_bytes()));
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
    fn the_ranges_reach_to_two_to_the_forty() {
        assert_eq!(address_bits(), 40);
    }

    #[test]
    fn the_answer_is_the_adapter_the_flags_and_the_address_bits() {
        let (status, out) = ask(QUERY_GPU_MMU_CAPS, 12);
        assert_eq!(status, Some(Status::SUCCESS));
        assert_eq!(out, [0, 0, 0, 0, 5, 0, 0, 0, 40, 0, 0, 0]);
    }

    #[test]
    fn a_short_buffer_is_refused_and_other_queries_are_left_alone() {
        assert_eq!(ask(QUERY_GPU_MMU_CAPS, 8).0, Some(Status::INVALID_PARAMETER));
        assert_eq!(ask(QUERY_GPU_MMU_CAPS + 1, 12), (None, Vec::new()));
    }
}
