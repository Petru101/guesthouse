//! Contexts, hardware queues, submissions and copies to the screen.

use crate::wire::{
    Malformed, align_up, clamped, i32_at, pad_to, put_u32, put_u64, set_u32, set_u64, u32_at, u64_at,
};

const ANSWER_ALIGN: u64 = 8;
/// The guest sizes these answers a few bytes past the private data before rounding up.
const ANSWER_SLACK: u64 = 3;

fn padded(fixed_part: usize, private_size: u32) -> usize {
    let used = fixed_part as u64 + u64::from(private_size) + ANSWER_SLACK;
    align_up(used, ANSWER_ALIGN) as usize
}

/// A new context. The guest reads its own message back as the answer.
#[derive(Debug, Clone, Copy)]
pub struct CreateContext {
    pub node: u32,
    pub private_size: u32,
}

impl CreateContext {
    const HANDLE: usize = crate::HEADER_SIZE;
    const NODE: usize = 0x20;
    const PRIVATE_SIZE: usize = 0x30;
    const PRIVATE: usize = 0x34;

    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self { node: u32_at(message, Self::NODE)?, private_size: u32_at(message, Self::PRIVATE_SIZE)? })
    }

    pub fn answer(&self, message: &[u8], handle: u32, out: &mut Vec<u8>) {
        out.extend_from_slice(message);
        pad_to(out, padded(Self::PRIVATE, self.private_size));
        set_u32(out, Self::HANDLE, handle);
    }
}

/// A new hardware queue. The guest reads its own message back here too.
#[derive(Debug, Clone, Copy)]
pub struct CreateHardwareQueue {
    pub context: u32,
    pub private_size: u32,
}

impl CreateHardwareQueue {
    const STATUS: usize = 0x18;
    const QUEUE: usize = 0x1C;
    const PROGRESS_FENCE: usize = 0x20;
    const FENCE_STORAGE: usize = 0x28;
    const FENCE_GPU_ADDRESS: usize = 0x30;
    const CONTEXT: usize = 0x38;
    const PRIVATE_SIZE: usize = 0x40;
    pub const PRIVATE: usize = 0x44;

    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self {
            context: u32_at(message, Self::CONTEXT)?,
            private_size: u32_at(message, Self::PRIVATE_SIZE)?,
        })
    }

    pub fn begin_answer(&self, message: &[u8], out: &mut Vec<u8>) {
        out.extend_from_slice(message);
        pad_to(out, padded(Self::PRIVATE, self.private_size));
    }

    pub fn finish_answer(queue: u32, progress_fence: u32, storage: u64, gpu_address: u64, out: &mut [u8]) {
        set_u32(out, Self::STATUS, crate::Status::SUCCESS.0);
        set_u32(out, Self::QUEUE, queue);
        set_u32(out, Self::PROGRESS_FENCE, progress_fence);
        set_u32(out, Self::PROGRESS_FENCE + 4, 0);
        set_u64(out, Self::FENCE_STORAGE, storage);
        set_u64(out, Self::FENCE_GPU_ADDRESS, gpu_address);
    }
}

/// Work for a hardware queue.
#[derive(Debug, Clone, Copy)]
pub struct SubmitToQueue<'a> {
    pub queue: u32,
    pub fence_value: u64,
    pub commands: u64,
    pub commands_length: u32,
    /// What the vendor's driver put after the allocation list.
    pub private: &'a [u8],
}

impl<'a> SubmitToQueue<'a> {
    const QUEUE: usize = 0x18;
    const FENCE_VALUE: usize = 0x20;
    const COMMANDS: usize = 0x28;
    const COMMANDS_LENGTH: usize = 0x30;
    const ALLOCATION_COUNT: usize = 0x40;
    const ALLOCATIONS: usize = 0x50;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        // The private size field is read too, so that a message cut short there fails here.
        u32_at(message, Self::COMMANDS_LENGTH + 4)?;
        let allocations = u32_at(message, Self::ALLOCATION_COUNT)? as usize;
        let private_at = Self::ALLOCATIONS.saturating_add(allocations.saturating_mul(4));
        Ok(Self {
            queue: u32_at(message, Self::QUEUE)?,
            fence_value: u64_at(message, Self::FENCE_VALUE)?,
            commands: u64_at(message, Self::COMMANDS)?,
            commands_length: u32_at(message, Self::COMMANDS_LENGTH)?,
            private: clamped(message, private_at, usize::MAX),
        })
    }
}

/// Work for a context with no hardware queue of its own.
#[derive(Debug, Clone, Copy)]
pub struct SubmitToContext<'a> {
    pub context: u32,
    pub private: &'a [u8],
}

impl<'a> SubmitToContext<'a> {
    const CONTEXT: usize = 0x34;
    const PRIVATE_SIZE: usize = 0x140;
    const HISTORY_COUNT: usize = 0x188;
    const HISTORY: usize = 0x198;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let private_size = u32_at(message, Self::PRIVATE_SIZE)? as usize;
        let history = u32_at(message, Self::HISTORY_COUNT)? as usize;
        let private_at = Self::HISTORY.saturating_add(history.saturating_mul(4));
        Ok(Self {
            context: u32_at(message, Self::CONTEXT)?,
            private: clamped(message, private_at, private_size),
        })
    }
}

/// A rectangle, as left, top, right and bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rectangle {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rectangle {
    fn at(message: &[u8], at: usize) -> Result<Self, Malformed> {
        Ok(Self {
            left: i32_at(message, at)?,
            top: i32_at(message, at + 4)?,
            right: i32_at(message, at + 8)?,
            bottom: i32_at(message, at + 12)?,
        })
    }
}

/// A finished frame to copy into the staging surface that feeds the guest's display.
#[derive(Debug, Clone, Copy)]
pub struct CopyToStaging {
    pub context: u32,
    pub source: u32,
    pub target: u32,
    pub source_rectangle: Rectangle,
    pub target_left: i32,
    pub target_top: i32,
    /// Nonzero when an app's present brought driver data. The kernel's own display queue has none.
    pub private_size: u32,
}

impl CopyToStaging {
    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self {
            context: u32_at(message, 0x1C)?,
            source: u32_at(message, 0x20)?,
            target: u32_at(message, 0x24)?,
            source_rectangle: Rectangle::at(message, 0x28)?,
            target_left: i32_at(message, 0x38)?,
            target_top: i32_at(message, 0x3C)?,
            private_size: u32_at(message, 0x4C)?,
        })
    }
}

/// An app has presented. The three values go back to the guest when the GPU reaches this point.
#[derive(Debug, Clone, Copy)]
pub struct PresentToken {
    pub context: u32,
    pub token: [u64; 3],
}

impl PresentToken {
    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self {
            context: u32_at(message, 0x18)?,
            token: [u64_at(message, 0x20)?, u64_at(message, 0x28)?, u64_at(message, 0x30)?],
        })
    }
}

/// A driver escape: opaque data for the vendor's kernel driver.
#[derive(Debug, Clone, Copy)]
pub struct DriverEscape<'a> {
    pub size: usize,
    pub data: &'a [u8],
}

impl<'a> DriverEscape<'a> {
    const SIZE: usize = crate::HEADER_SIZE + 16;
    const DATA: usize = crate::HEADER_SIZE + 24;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let size = u32_at(message, Self::SIZE)? as usize;
        Ok(Self { size, data: clamped(message, Self::DATA, size) })
    }
}

/// The 16 bytes the guest expects back from a limit change, then a status.
pub fn queued_limit_answer(message: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(clamped(message, 0x18, 0x10));
    put_u32(out, crate::Status::SUCCESS.0);
}

/// The clock answer: a status, the GPU's frequency, and both clocks now.
pub fn clock_answer(gpu_hertz: u64, gpu_clock: u64, cpu_clock: u64, out: &mut Vec<u8>) {
    put_u32(out, crate::Status::SUCCESS.0);
    put_u64(out, gpu_hertz);
    put_u64(out, gpu_clock);
    put_u64(out, cpu_clock);
    put_u32(out, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_answer_is_the_message_with_a_handle() {
        let mut message = vec![0xAA; 0x34];
        set_u32(&mut message, 0x30, 4);
        message.extend_from_slice(b"priv");
        let request = CreateContext::parse(&message).unwrap();
        let mut out = Vec::new();
        request.answer(&message, 0x77, &mut out);
        assert_eq!(out.len(), 0x40);
        assert_eq!(u32_at(&out, 0x18), Ok(0x77));
        assert_eq!(&out[0x34..0x38], b"priv");
    }

    #[test]
    fn context_answer_pads_when_the_private_data_is_missing() {
        let mut message = vec![0; 0x34];
        set_u32(&mut message, 0x30, 12);
        let mut out = Vec::new();
        CreateContext::parse(&message).unwrap().answer(&message, 1, &mut out);
        assert_eq!(out.len(), 0x48);
    }

    #[test]
    fn submission_private_data_follows_the_allocation_list() {
        let mut message = vec![0; 0x50 + 8];
        set_u32(&mut message, 0x18, 0x40);
        set_u64(&mut message, 0x20, 77);
        set_u32(&mut message, 0x40, 2);
        message.extend_from_slice(b"private");
        let parsed = SubmitToQueue::parse(&message).unwrap();
        assert_eq!((parsed.queue, parsed.fence_value), (0x40, 77));
        assert_eq!(parsed.private, b"private");
    }

    #[test]
    fn escape_data_is_cut_to_the_message() {
        let mut message = vec![0; crate::HEADER_SIZE + 24];
        set_u32(&mut message, crate::HEADER_SIZE + 16, 100);
        message.extend_from_slice(b"abc");
        let parsed = DriverEscape::parse(&message).unwrap();
        assert_eq!((parsed.size, parsed.data), (100, &b"abc"[..]));
    }

    #[test]
    fn limit_answer_repeats_sixteen_bytes() {
        let message: Vec<u8> = (0..0x30).collect();
        let mut out = Vec::new();
        queued_limit_answer(&message, &mut out);
        assert_eq!(out.len(), 20);
        assert_eq!(out[0], 0x18);
    }
}
