//! The range of GPU addresses the guest's compute runtime expects to have for itself.
//!
//! The runtime treats it as set aside already and places things there without asking, so the host
//! must keep its own hand-outs clear of it.

use super::put;
use guesthouse_backend::GpuRange;

/// Everything from 512 MiB up to the 4 GiB line.
pub const RUNTIME_GPU_RANGE: GpuRange = GpuRange { start: 0x2000_0000, end: 0x1_0000_0000 };

// The answer is packed, so these fields aren't aligned.
const PRESENT: usize = 0x30;
const UNEXPLAINED: usize = 0x31;
const BASE: usize = 0x35;
const LAST: usize = 0x3D;

const RANGE_PRESENT: u8 = 1;
/// What this word means isn't known. It goes back as a real install sends it.
const UNEXPLAINED_VALUE: u32 = 2 << 20;

/// Fills in the answer. A zero in the first field stops the runtime from starting at all.
pub(super) fn answer(data: &mut [u8]) {
    if let Some(present) = data.get_mut(PRESENT) {
        *present = RANGE_PRESENT;
    }
    put(data, UNEXPLAINED, UNEXPLAINED_VALUE);
    put_long(data, BASE, RUNTIME_GPU_RANGE.start);
    put_long(data, LAST, RUNTIME_GPU_RANGE.end - 1);
}

fn put_long(data: &mut [u8], at: usize, value: u64) {
    if let Some(field) = data.get_mut(at..at + 8) {
        field.copy_from_slice(&value.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{device, escape};
    use super::super::{FUNCTION_RUNTIME_GPU_RANGE, RESULT, answer as answer_escape, get};
    use super::*;

    /// The size the runtime really asks with.
    const WHOLE_ANSWER: usize = 0x45;

    fn long(data: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn the_four_fields_land_at_their_packed_offsets() {
        let mut data = escape(FUNCTION_RUNTIME_GPU_RANGE, WHOLE_ANSWER);
        answer_escape(&mut device(), &[], &mut data).unwrap();
        assert_eq!(get(&data, RESULT), Some(1));
        assert_eq!(data[PRESENT], 1);
        assert_eq!(get(&data, UNEXPLAINED), Some(0x20_0000));
        assert_eq!(long(&data, BASE), 0x2000_0000);
        assert_eq!(long(&data, LAST), 0xFFFF_FFFF);
    }

    #[test]
    fn the_range_promised_is_the_range_the_host_keeps_clear() {
        let mut data = escape(FUNCTION_RUNTIME_GPU_RANGE, WHOLE_ANSWER);
        answer_escape(&mut device(), &[], &mut data).unwrap();
        assert!(RUNTIME_GPU_RANGE.contains(long(&data, BASE)));
        assert!(RUNTIME_GPU_RANGE.contains(long(&data, LAST)));
    }

    #[test]
    fn a_short_buffer_gets_what_fits() {
        let mut data = escape(FUNCTION_RUNTIME_GPU_RANGE, BASE + 4);
        answer_escape(&mut device(), &[], &mut data).unwrap();
        assert_eq!(data[PRESENT], 1);
    }
}
