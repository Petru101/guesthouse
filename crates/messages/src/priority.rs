//! Residency priorities, which the guest sets and reads for allocations it made.

use crate::wire::{Malformed, U32List, declared_count, put_u32, u32_at};
use crate::{HEADER_SIZE, Status};

/// What an allocation's priority is until the guest sets one: the middle of the guest's scale.
pub const NORMAL_PRIORITY: u32 = 0x7800_0000;

/// Which allocations a request is about. With no handles named it means everything in the
/// resource, and then it carries one priority rather than one for each.
#[derive(Debug, Clone, Copy)]
pub struct PriorityRequest<'a> {
    pub resource: u32,
    pub handles: U32List<'a>,
    message: &'a [u8],
}

impl<'a> PriorityRequest<'a> {
    const RESOURCE: usize = HEADER_SIZE + 4;
    const COUNT: usize = HEADER_SIZE + 8;
    const HANDLES: usize = HEADER_SIZE + 12;
    const HANDLE_SIZE: usize = 4;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let count = declared_count(message, Self::COUNT, Self::HANDLE_SIZE)?;
        Ok(Self {
            resource: u32_at(message, Self::RESOURCE)?,
            handles: U32List::at(message, Self::HANDLES, count)?,
            message,
        })
    }

    /// How many priorities a set carries and a get answers.
    pub fn priority_count(&self) -> usize {
        self.handles.len().max(1)
    }

    /// The priorities a set brings, which follow the handles.
    pub fn priorities(&self) -> Result<U32List<'a>, Malformed> {
        let at = Self::HANDLES + self.handles.len() * Self::HANDLE_SIZE;
        U32List::at(self.message, at, self.priority_count())
    }
}

/// A status, then the priorities.
pub fn answer(priorities: impl Iterator<Item = u32>, out: &mut Vec<u8>) {
    put_u32(out, Status::SUCCESS.0);
    priorities.for_each(|priority| put_u32(out, priority));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(resource: u32, handles: &[u32], priorities: &[u32]) -> Vec<u8> {
        let mut bytes = vec![0; HEADER_SIZE + 4];
        put_u32(&mut bytes, resource);
        put_u32(&mut bytes, handles.len() as u32);
        handles.iter().chain(priorities).for_each(|&word| put_u32(&mut bytes, word));
        bytes
    }

    #[test]
    fn handles_come_with_one_priority_each() {
        let bytes = request(7, &[1, 2], &[10, 20]);
        let parsed = PriorityRequest::parse(&bytes).unwrap();
        assert_eq!(parsed.resource, 7);
        assert_eq!(parsed.handles.iter().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(parsed.priorities().unwrap().iter().collect::<Vec<_>>(), [10, 20]);
    }

    #[test]
    fn a_resource_alone_comes_with_a_single_priority() {
        let bytes = request(7, &[], &[30]);
        let parsed = PriorityRequest::parse(&bytes).unwrap();
        assert_eq!(parsed.priority_count(), 1);
        assert_eq!(parsed.priorities().unwrap().iter().collect::<Vec<_>>(), [30]);
    }

    #[test]
    fn missing_priorities_are_malformed() {
        let bytes = request(7, &[1, 2], &[10]);
        assert!(PriorityRequest::parse(&bytes).unwrap().priorities().is_err());
    }

    #[test]
    fn an_answer_is_a_status_and_the_priorities() {
        let mut out = Vec::new();
        answer([5, 6].into_iter(), &mut out);
        assert_eq!(out, [0, 0, 0, 0, 5, 0, 0, 0, 6, 0, 0, 0]);
    }
}
