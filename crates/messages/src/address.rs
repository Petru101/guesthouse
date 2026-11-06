//! GPU address requests: mapping, reserving and updating, and the memory query.

use crate::Status;
use crate::wire::{Malformed, clamped, put_u32, put_u64, u32_at, u64_at};

/// Bytes in the guest's GPU pages.
pub const PAGE_SIZE: u64 = 4096;

/// A request to map an allocation.
#[derive(Debug, Clone, Copy)]
pub struct MapRequest {
    /// Where the guest wants it, or zero for wherever the host likes.
    pub address: u64,
    /// Zero means the guest is only setting a range aside.
    pub allocation: u32,
    pub first_page: u64,
    pub pages: u64,
}

impl MapRequest {
    const ADDRESS: usize = 0x20;
    const ALLOCATION: usize = 0x38;
    const FIRST_PAGE: usize = 0x40;
    const PAGES: usize = 0x48;

    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self {
            address: u64_at(message, Self::ADDRESS)?,
            allocation: u32_at(message, Self::ALLOCATION)?,
            first_page: u64_at(message, Self::FIRST_PAGE)?,
            pages: u64_at(message, Self::PAGES)?,
        })
    }
}

/// Writes the answer shared by mapping, residency and eviction: two values and a status.
pub fn two_values_and_status(first: u64, second: u64, status: Status, out: &mut Vec<u8>) {
    put_u64(out, first);
    put_u64(out, second);
    put_u32(out, status.0);
    put_u32(out, 0);
}

/// How much address space the guest wants set aside.
pub fn reserve_size(message: &[u8]) -> Result<u64, Malformed> {
    u64_at(message, 0x38)
}

/// The start of a range the guest gives back.
pub fn freed_address(message: &[u8]) -> Result<u64, Malformed> {
    u64_at(message, 0x28)?;
    u64_at(message, 0x20)
}

/// A batch of address changes, which waits for a fence to reach a value.
#[derive(Debug, Clone, Copy)]
pub struct UpdateRequest<'a> {
    pub fence_value: u64,
    pub fence: u32,
    pub count: usize,
    operations: &'a [u8],
}

impl<'a> UpdateRequest<'a> {
    const FENCE_VALUE: usize = 0x18;
    const FENCE: usize = 0x28;
    const COUNT: usize = 0x2C;
    const OPERATIONS: usize = 0x38;
    pub const OPERATION_SIZE: usize = 64;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let count = u32_at(message, Self::COUNT)? as usize;
        Ok(Self {
            fence_value: u64_at(message, Self::FENCE_VALUE)?,
            fence: u32_at(message, Self::FENCE)?,
            count,
            operations: clamped(message, Self::OPERATIONS, count.saturating_mul(Self::OPERATION_SIZE)),
        })
    }

    /// Whether the message really holds as many operations as it says.
    pub fn is_complete(&self) -> bool {
        self.operations.len() == self.count.saturating_mul(Self::OPERATION_SIZE)
    }

    /// The raw operations, to keep when the update has to wait.
    pub fn raw_operations(&self) -> &'a [u8] {
        self.operations
    }
}

/// One change of a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOperation {
    Map { address: u64, size: u64, allocation: u32, offset: u64 },
    Unmap { address: u64, size: u64 },
    Copy { source: u64, size: u64, destination: u64 },
    Unknown(u32),
}

impl UpdateOperation {
    const KIND_MAP: u32 = 0;
    const KIND_UNMAP: u32 = 1;
    const KIND_COPY: u32 = 2;
    const KIND_MAP_PROTECTED: u32 = 3;
    const ADDRESS: usize = 0x08;
    const SIZE: usize = 0x10;
    const THIRD: usize = 0x18;
    const OFFSET: usize = 0x20;
    /// Reads past here would pass the end of an operation cut short by the message.
    const FULL_SIZE: usize = 0x40;

    pub fn parse(operation: &[u8]) -> Result<Self, Malformed> {
        let kind = u32_at(operation, 0)?;
        let address = u64_at(operation, Self::ADDRESS)?;
        let size = u64_at(operation, Self::SIZE)?;
        let third = u64_at(operation, Self::THIRD)?;
        Ok(match kind {
            Self::KIND_MAP | Self::KIND_MAP_PROTECTED => {
                if operation.len() < Self::FULL_SIZE {
                    return Err(Malformed);
                }
                let offset = u64_at(operation, Self::OFFSET)?;
                Self::Map { address, size, allocation: third as u32, offset }
            }
            Self::KIND_UNMAP => Self::Unmap { address, size },
            Self::KIND_COPY => Self::Copy { source: address, size, destination: third },
            other => Self::Unknown(other),
        })
    }
}

/// Which pool of memory a usage query is about: zero is video memory.
pub fn memory_group(message: &[u8]) -> Result<u32, Malformed> {
    u32_at(message, 0x1C)
}

pub fn memory_answer(budget: u64, usage: u64, out: &mut Vec<u8>) {
    put_u64(out, budget);
    put_u64(out, usage);
    put_u64(out, 0);
    put_u64(out, budget / 2);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::set_u64;

    #[test]
    fn map_operation_reads_its_allocation_from_the_low_half() {
        let mut bytes = [0; 64];
        bytes[0] = 3;
        set_u64(&mut bytes, 8, 0x1000);
        set_u64(&mut bytes, 16, 0x2000);
        set_u64(&mut bytes, 24, 0xAAAA_0000_0040);
        set_u64(&mut bytes, 32, 0x10);
        let expected = UpdateOperation::Map { address: 0x1000, size: 0x2000, allocation: 0x40, offset: 0x10 };
        assert_eq!(UpdateOperation::parse(&bytes), Ok(expected));
    }

    #[test]
    fn a_cut_short_map_is_malformed_but_an_unmap_is_not() {
        let mut bytes = [0; 32];
        assert!(UpdateOperation::parse(&bytes).is_err());
        bytes[0] = 1;
        assert!(UpdateOperation::parse(&bytes).is_ok());
    }

    #[test]
    fn unknown_operations_are_kept_for_the_log() {
        let mut bytes = [0; 64];
        bytes[0] = 9;
        assert_eq!(UpdateOperation::parse(&bytes), Ok(UpdateOperation::Unknown(9)));
    }

    #[test]
    fn answer_shapes() {
        let mut out = Vec::new();
        two_values_and_status(1, 2, Status::SUCCESS, &mut out);
        assert_eq!(out.len(), 24);
        let mut out = Vec::new();
        memory_answer(100, 7, &mut out);
        assert_eq!(u64_at(&out, 24), Ok(50));
    }
}
