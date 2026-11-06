//! Driver escapes: the guest's user-mode drivers talking to their kernel driver.
//!
//! A few of them are real work, and most only need the answer their kernel driver would give. Some
//! carry a control for the RM, which runs on the host if it is on the list of what only reads.

mod client_alloc;
mod control;
mod gpu_groups;
mod gpu_range;
mod node_list;

pub use gpu_range::{RUNTIME_GPU_RANGE, RUNTIME_PLACED_RANGE};

use crate::answers::nodes::Node;
use crate::rm::device::Device;
use crate::rm::{Rm, RmError};

// The escape's fixed fields.
const HEADER_SIZE_FIELD: usize = 0x08;
const FUNCTION: usize = 0x10;
const ANSWERED: usize = 0x14;
const RESULT: usize = 0x2C;
const HEADER_SIZE: u32 = 0x30;
const RESULT_OK: i32 = 1;
const RESULT_UNSUPPORTED: i32 = -23;

const FUNCTION_UNSUPPORTED: u32 = 0x0100_0152;
const FUNCTION_RM_CONTROL: u32 = 0x0500_002B;
/// Asked of each display. It needs its own size back in the reply.
const FUNCTION_CHECK_DISPLAY: u32 = 0x0100_0001;
/// The first two questions the vendor's own API asks: which GPUs there are.
const FUNCTION_LIST_GPUS: u32 = 0x0700_0002;
const FUNCTION_LIST_COMPUTE_GPUS: u32 = 0x0700_0003;
/// The third question: how those GPUs are grouped.
const FUNCTION_LIST_GPU_GROUPS: u32 = 0x0700_0004;
/// Which GPU addresses the compute runtime may treat as its own.
const FUNCTION_RUNTIME_GPU_RANGE: u32 = 0x0100_0113;
const FUNCTION_RM_ALLOC: u32 = 0x0500_002A;
/// Which node is which engine, for the video runtime.
const FUNCTION_LIST_NODES: u32 = 0x0100_000F;
/// Asked before an encoding session opens, and the session is refused unless the answer is yes.
const FUNCTION_ENCODE_ASK: u32 = 0x0100_0128;
const ENCODE_ANSWER: usize = 0x30;
const ENCODE_YES: u8 = 1;

const GPU_LIST_COUNT: usize = 0x34;
const GPU_LIST_HANDLES: usize = 0x38;
const GPU_LIST_KINDS: usize = 0x138;
const GPU_KIND_DISPLAY_DRIVER: u32 = 1;

fn get(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4).map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
}

/// Writes a word, for a guest whose buffer may be too short for it.
fn put(data: &mut [u8], at: usize, value: u32) {
    if let Some(field) = data.get_mut(at..at + 4) {
        field.copy_from_slice(&value.to_le_bytes());
    }
}

/// What an escape's answer can draw on, besides the card.
#[derive(Debug, Clone, Copy)]
pub struct EscapeFacts<'a> {
    /// The class behind each of the guest's handles.
    pub channel_classes: &'a [(u32, u32)],
    pub nodes: &'a [Node],
}

/// Answers an escape in place.
pub fn answer<R: Rm>(device: &mut Device<R>, facts: EscapeFacts<'_>, data: &mut [u8]) -> Result<(), RmError> {
    let Some(function) = get(data, FUNCTION) else { return Ok(()) };
    match function {
        FUNCTION_RM_CONTROL => control::run(device, facts.channel_classes, data)?,
        FUNCTION_LIST_GPUS => {
            // The RM's number serves as the handle, which comes back to the host later.
            let gpu_id = device.gpu_id()?;
            put(data, GPU_LIST_COUNT, 1);
            put(data, GPU_LIST_HANDLES, gpu_id);
            put(data, GPU_LIST_KINDS, GPU_KIND_DISPLAY_DRIVER);
        }
        FUNCTION_LIST_GPU_GROUPS => gpu_groups::answer(device.gpu_id()?, data),
        // A card under the display driver isn't a compute-only one, so this list stays empty.
        FUNCTION_LIST_COMPUTE_GPUS => {}
        FUNCTION_CHECK_DISPLAY => {}
        FUNCTION_RUNTIME_GPU_RANGE => gpu_range::answer(data),
        FUNCTION_LIST_NODES => node_list::answer(facts.nodes, data),
        FUNCTION_ENCODE_ASK => {
            if let Some(answer) = data.get_mut(ENCODE_ANSWER) {
                *answer = ENCODE_YES;
            }
        }
        FUNCTION_RM_ALLOC => {
            put(data, HEADER_SIZE_FIELD, HEADER_SIZE);
            client_alloc::answer(data);
        }
        _ => put(data, HEADER_SIZE_FIELD, HEADER_SIZE),
    }
    put(data, ANSWERED, 1);
    let result = if function == FUNCTION_UNSUPPORTED { RESULT_UNSUPPORTED } else { RESULT_OK };
    put(data, RESULT, result as u32);
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::profile::HOST_615;
    use crate::rm::classes::CONTROL_GET_GPU_ID;
    use crate::rm::fake::FakeRm;

    pub(super) fn device() -> Device<FakeRm> {
        let mut rm = FakeRm::new();
        rm.answer(CONTROL_GET_GPU_ID, |params| {
            params[..4].copy_from_slice(&0x300u32.to_le_bytes());
            0
        });
        Device::open(rm, &HOST_615).unwrap()
    }

    pub(super) const NO_FACTS: EscapeFacts<'static> = EscapeFacts { channel_classes: &[], nodes: &[] };

    pub(super) fn escape(function: u32, size: usize) -> Vec<u8> {
        let mut data = vec![0; size];
        put(&mut data, FUNCTION, function);
        data
    }

    #[test]
    fn a_plain_escape_gets_the_usual_answer() {
        let mut data = escape(0x0123, 0x40);
        answer(&mut device(), NO_FACTS, &mut data).unwrap();
        assert_eq!(get(&data, HEADER_SIZE_FIELD), Some(HEADER_SIZE));
        assert_eq!(get(&data, ANSWERED), Some(1));
        assert_eq!(get(&data, RESULT), Some(1));
    }

    #[test]
    fn one_escape_says_it_is_unsupported_as_the_real_driver_does() {
        let mut data = escape(FUNCTION_UNSUPPORTED, 0x40);
        answer(&mut device(), NO_FACTS, &mut data).unwrap();
        assert_eq!(get(&data, RESULT), Some(RESULT_UNSUPPORTED as u32));
    }

    #[test]
    fn a_display_check_keeps_its_own_size() {
        let mut data = escape(FUNCTION_CHECK_DISPLAY, 0x40);
        put(&mut data, HEADER_SIZE_FIELD, 0x1234);
        answer(&mut device(), NO_FACTS, &mut data).unwrap();
        assert_eq!(get(&data, HEADER_SIZE_FIELD), Some(0x1234));
    }

    #[test]
    fn the_gpu_list_has_this_one_card() {
        let mut data = escape(FUNCTION_LIST_GPUS, GPU_LIST_KINDS + 4);
        answer(&mut device(), NO_FACTS, &mut data).unwrap();
        assert_eq!(get(&data, GPU_LIST_COUNT), Some(1));
        assert_eq!(get(&data, GPU_LIST_HANDLES), Some(0x300));
    }

    #[test]
    fn the_encoders_question_gets_a_yes() {
        let mut data = escape(FUNCTION_ENCODE_ASK, ENCODE_ANSWER + 1);
        answer(&mut device(), NO_FACTS, &mut data).unwrap();
        assert_eq!(data[ENCODE_ANSWER], ENCODE_YES);
    }

    #[test]
    fn a_short_escape_is_not_a_problem() {
        answer(&mut device(), NO_FACTS, &mut [0; 4]).unwrap();
    }
}
