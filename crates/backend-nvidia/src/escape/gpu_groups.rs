//! How the vendor's API learns which GPUs work together.
//!
//! A GPU this answer leaves out fails every later call by its handle, even when the GPU list named
//! it.

use super::{GPU_KIND_DISPLAY_DRIVER, put};

const GROUP_COUNT: usize = 0x34;
const GROUP_GPU_IDS: usize = 0x3C;
/// One word per GPU, whose meaning is not known. Zero is the guess for a group of one.
const GROUP_GPU_DETAILS: usize = 0x13C;
const GROUP_GPU_COUNT: usize = 0x23C;
const GROUP_KEY: usize = 0x240;
const GROUP_FLAGS: usize = 0x244;
const GROUP_KIND: usize = 0x248;

/// The daemon's device instance, which is the RM's first.
const DEVICE_INSTANCE: u32 = 0;

/// Says there is one group, of this one card.
pub(super) fn answer(gpu_id: u32, data: &mut [u8]) {
    put(data, GROUP_COUNT, 1);
    put(data, GROUP_GPU_COUNT, 1);
    put(data, GROUP_GPU_IDS, gpu_id);
    put(data, GROUP_GPU_DETAILS, 0);
    put(data, GROUP_KEY, DEVICE_INSTANCE);
    put(data, GROUP_FLAGS, 0);
    put(data, GROUP_KIND, GPU_KIND_DISPLAY_DRIVER);
}

#[cfg(test)]
mod tests {
    use super::super::tests::{NO_FACTS, device, escape};
    use super::super::{FUNCTION_LIST_GPU_GROUPS, RESULT, answer as answer_escape, get};
    use super::*;

    /// The size the guest's API really asks with.
    const WHOLE_ANSWER: usize = 0x8538;

    #[test]
    fn the_answer_names_this_card_as_a_group_of_one() {
        let mut data = escape(FUNCTION_LIST_GPU_GROUPS, WHOLE_ANSWER);
        answer_escape(&mut device(), NO_FACTS, &mut data).unwrap();
        assert_eq!(get(&data, RESULT), Some(1));
        assert_eq!(get(&data, GROUP_COUNT), Some(1));
        assert_eq!(get(&data, GROUP_GPU_COUNT), Some(1));
        assert_eq!(get(&data, GROUP_GPU_IDS), Some(0x300));
        assert_eq!(get(&data, GROUP_KIND), Some(GPU_KIND_DISPLAY_DRIVER));
    }

    #[test]
    fn old_bytes_in_the_guests_buffer_do_not_survive() {
        let mut data = vec![0xAA; WHOLE_ANSWER];
        data[..0x14].copy_from_slice(&escape(FUNCTION_LIST_GPU_GROUPS, 0x14));
        answer_escape(&mut device(), NO_FACTS, &mut data).unwrap();
        assert_eq!(get(&data, GROUP_KEY), Some(DEVICE_INSTANCE));
        assert_eq!(get(&data, GROUP_FLAGS), Some(0));
        assert_eq!(get(&data, GROUP_GPU_DETAILS), Some(0));
    }

    #[test]
    fn a_short_buffer_gets_what_fits() {
        let mut data = escape(FUNCTION_LIST_GPU_GROUPS, GROUP_COUNT + 4);
        answer_escape(&mut device(), NO_FACTS, &mut data).unwrap();
        assert_eq!(get(&data, GROUP_COUNT), Some(1));
    }
}
