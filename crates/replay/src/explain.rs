//! Saying why two replies differ, as far as bytes can say.

use guesthouse_messages::Command;
use guesthouse_messages::wire::{u32_at, u64_at};

const HANDLE_STEP: u32 = 0x40;
const GPU_ADDRESS_GRAIN: u64 = 64 << 10;
const FENCE_PAGE: u64 = 4096;
const WORD_ALIGN: usize = 4;
const FIRST_HANDLE: u32 = 0x4000_0000;
const LAST_HANDLE: u32 = 0x5000_0000;

/// Where two replies first differ.
pub fn first_difference(expected: &[u8], actual: &[u8]) -> Option<usize> {
    let common = expected.len().min(actual.len());
    let found = expected.iter().zip(actual).position(|(left, right)| left != right);
    found.or((expected.len() != actual.len()).then_some(common))
}

fn looks_like_handle(value: u32) -> bool {
    (FIRST_HANDLE..LAST_HANDLE).contains(&value) && value.is_multiple_of(HANDLE_STEP)
}

/// The field a reply offset is in, where it is worth naming.
fn field_name(command: Command, offset: usize) -> Option<&'static str> {
    use Command as C;
    Some(match (command, offset) {
        (C::CreateAllocation, 0..4) => "status",
        (C::CreateAllocation, 4..8) => "resource handle",
        (C::CreateHardwareQueue, 0x18..0x1C) => "status",
        (C::CreateHardwareQueue, 0x1C..0x20) => "queue handle",
        (C::CreateHardwareQueue, 0x20..0x24) => "progress fence handle",
        (C::CreateHardwareQueue, 0x28..0x30) => "fence storage",
        (C::CreateHardwareQueue, 0x30..0x38) => "fence GPU address",
        (C::CreateHardwareQueue, 0x44..) => "vendor's private data",
        (C::CreateContext, 0x18..0x1C) => "context handle",
        (C::CreateSyncObject, 0..8) => "sync object handle",
        (C::CreateSyncObject, 8..16) => "fence GPU address",
        (C::CreateSyncObject, 16..24) => "fence storage",
        (C::CreatePagingQueue, 0..4) => "queue handle",
        (C::CreatePagingQueue, 4..8) => "sync object handle",
        (C::CreatePagingQueue, 8..16) => "fence storage",
        (C::MapGpuAddresses, 0..8) => "GPU address",
        (C::MapGpuAddresses, 16..20) => "status",
        (C::ReserveGpuAddresses, 0..8) => "GPU address",
        (C::LockForCpu, 0..4) => "status",
        (C::LockForCpu, 8..16) => "guest physical address",
        (C::QueryVideoMemory, 0..8) => "budget",
        (C::QueryVideoMemory, 8..16) => "usage",
        _ => return None,
    })
}

/// A sentence on the likely cause, for the first difference between two replies.
pub fn explain(command: Command, offset: usize, expected: &[u8], actual: &[u8]) -> String {
    if expected.len() != actual.len() && offset == expected.len().min(actual.len()) {
        return format!(
            "The replies match as far as they go, but one is {} bytes and the other {}. A handler is \
             writing a different shape of answer.",
            expected.len(),
            actual.len()
        );
    }
    let field = field_name(command, offset).map_or_else(String::new, |name| format!(" ({name})"));
    let at = offset / WORD_ALIGN * WORD_ALIGN;
    let words = u32_at(expected, at).ok().zip(u32_at(actual, at).ok());
    if let Some((left, right)) = words
        && looks_like_handle(left)
        && looks_like_handle(right)
    {
        let apart = left.abs_diff(right) / HANDLE_STEP;
        return format!(
            "Byte {offset:#x}{field} is a handle, {apart} away from the recorded one. An earlier message \
             made a different number of objects."
        );
    }
    let longs = u64_at(expected, at).ok().zip(u64_at(actual, at).ok());
    if let Some((left, right)) = longs {
        let apart = left.abs_diff(right);
        if apart != 0 && apart % GPU_ADDRESS_GRAIN == 0 {
            return format!(
                "Byte {offset:#x}{field} is {apart:#x} from the recorded value, a whole number of \
                 allocation units. If this is a GPU or shared-memory address, an earlier allocation \
                 differed, or something freed at a different time."
            );
        }
        if apart != 0 && apart % FENCE_PAGE == 0 {
            return format!(
                "Byte {offset:#x}{field} is {apart:#x} from the recorded value, a whole number of pages. \
                 If this is a fence, a fence came back to the pool at a different moment."
            );
        }
    }
    format!(
        "Byte {offset:#x}{field} is {:#04x} and the recording has {:#04x}.",
        actual.get(offset).copied().unwrap_or(0),
        expected.get(offset).copied().unwrap_or(0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_replies_have_no_difference() {
        assert_eq!(first_difference(b"same", b"same"), None);
    }

    #[test]
    fn the_first_changed_byte_counts() {
        assert_eq!(first_difference(b"abcd", b"abXY"), Some(2));
    }

    #[test]
    fn a_shorter_reply_differs_where_it_ends() {
        assert_eq!(first_difference(b"abc", b"ab"), Some(2));
    }

    #[test]
    fn a_length_difference_is_named() {
        assert!(explain(Command::CreateDevice, 2, b"abc", b"ab").contains("3 bytes"));
    }

    #[test]
    fn handles_that_drifted_are_recognised() {
        let (expected, actual) = (0x4000_1040u32.to_le_bytes(), 0x4000_10C0u32.to_le_bytes());
        let reason = explain(Command::CreateDevice, 0, &expected, &actual);
        assert!(reason.contains("2 away"), "{reason}");
    }

    #[test]
    fn addresses_that_drifted_are_recognised() {
        let (expected, actual) = (0x500_0000u64.to_le_bytes(), 0x501_0000u64.to_le_bytes());
        let reason = explain(Command::MapGpuAddresses, 2, &expected, &actual);
        assert!(reason.contains("GPU address") && reason.contains("allocation units"), "{reason}");
    }

    #[test]
    fn anything_else_gives_the_bytes() {
        let reason = explain(Command::Evict, 1, &[0, 1], &[0, 2]);
        assert!(reason.contains("0x02") && reason.contains("0x01"), "{reason}");
    }
}
