//! Sync objects, fences and waits.

use crate::Status;
use crate::wire::{Malformed, U32List, U64List, put_u32, put_u64, u8_at, u32_at, u64_at};

/// A fence with a counter in shared memory, which guest and GPU both read.
pub const KIND_MONITORED_FENCE: u32 = 5;
/// An object that wakes a guest event instead of counting.
pub const KIND_CPU_NOTIFICATION: u32 = 4;

/// A request for a new sync object.
#[derive(Debug, Clone, Copy)]
pub struct CreateSyncObject {
    pub kind: u32,
    /// A fence's first value, or a notification's guest event. Other kinds need not send it.
    pub initial: Option<u64>,
}

impl CreateSyncObject {
    const KIND: usize = 0x20;
    const INITIAL: usize = 0x28;

    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self { kind: u32_at(message, Self::KIND)?, initial: u64_at(message, Self::INITIAL).ok() })
    }

    /// With no device, the guest reads the second handle, so both are set.
    pub fn answer(handle: u32, gpu_address: u64, fence: u64, out: &mut Vec<u8>) {
        put_u32(out, handle);
        put_u32(out, handle);
        put_u64(out, gpu_address);
        put_u64(out, fence);
        put_u64(out, 0);
    }
}

/// The global handle in a request to open a shared sync object.
pub fn opened_sync_object(message: &[u8]) -> Result<u32, Malformed> {
    u32_at(message, 0x1C)
}

pub fn open_answer(handle: u32, status: Status, gpu_address: u64, fence: u64, out: &mut Vec<u8>) {
    put_u32(out, handle);
    put_u32(out, status.0);
    put_u64(out, gpu_address);
    put_u64(out, fence);
}

/// The answer to a paging queue: its handle, its sync object, and its fence.
pub fn paging_queue_answer(queue: u32, sync: u32, fence: u64, out: &mut Vec<u8>) {
    put_u32(out, queue);
    put_u32(out, sync);
    put_u64(out, fence);
    put_u64(out, 0);
}

/// The first field after the header, which many requests use for one handle.
pub fn first_handle(message: &[u8]) -> Result<u32, Malformed> {
    u32_at(message, crate::HEADER_SIZE)
}

/// A fence to raise now, with no queue to wait for.
#[derive(Debug, Clone, Copy)]
pub struct SignalFence {
    pub handle: u32,
    pub value: u64,
}

impl SignalFence {
    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self { handle: u32_at(message, 0x18)?, value: u64_at(message, 0x20)? })
    }
}

/// A shared fence to signal when a context reaches this point.
#[derive(Debug, Clone, Copy)]
pub struct SignalSharedFence {
    pub handle: u32,
    pub context: u32,
    pub value: u64,
}

impl SignalSharedFence {
    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self {
            handle: u32_at(message, 0x18)?,
            context: u32_at(message, 0x1C)?,
            value: u64_at(message, 0x20)?,
        })
    }
}

/// Objects to signal when some contexts reach this point.
#[derive(Debug, Clone, Copy)]
pub struct SignalSyncObject<'a> {
    pub objects: U32List<'a>,
    pub contexts: U32List<'a>,
    /// With this set, the guest wants this event woken instead of the objects changed.
    pub cpu_event: Option<u64>,
    /// Only monitored fences have values, so a message may stop before them.
    pub values: Option<U64List<'a>>,
}

impl<'a> SignalSyncObject<'a> {
    const OBJECTS: usize = 0x18;
    const FLAGS: usize = 0x1C;
    const CONTEXTS: usize = 0x20;
    const CPU_EVENT: usize = 0x30;
    const LISTS: usize = 0x38;
    const FLAG_CPU_EVENT: u32 = 2;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let object_count = u32_at(message, Self::OBJECTS)? as usize;
        let context_count = u32_at(message, Self::CONTEXTS)? as usize;
        let flags = u32_at(message, Self::FLAGS)?;
        let objects = U32List::at(message, Self::LISTS, object_count)?;
        let contexts = U32List::at(message, Self::LISTS + 4 * object_count, context_count)?;
        let cpu_event =
            if flags & Self::FLAG_CPU_EVENT != 0 { Some(u64_at(message, Self::CPU_EVENT)?) } else { None };
        let values_at = Self::LISTS + 4 * (object_count + context_count);
        Ok(Self { objects, contexts, cpu_event, values: U64List::at(message, values_at, object_count).ok() })
    }
}

/// The guest sleeps until these objects reach these values, and we wake its event.
#[derive(Debug, Clone, Copy)]
pub struct WaitOnCpu<'a> {
    pub objects: U32List<'a>,
    pub values: U64List<'a>,
    pub wake_on_any: bool,
    pub event: u64,
    pub release_event: bool,
}

impl<'a> WaitOnCpu<'a> {
    const COUNT: usize = 0x1C;
    const FLAGS: usize = 0x20;
    const EVENT: usize = 0x28;
    const RELEASE: usize = 0x30;
    const LISTS: usize = 0x38;
    const FLAG_ANY: u32 = 1;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let count = u32_at(message, Self::COUNT)? as usize;
        Ok(Self {
            objects: U32List::at(message, Self::LISTS, count)?,
            values: U64List::at(message, Self::LISTS + 4 * count, count)?,
            wake_on_any: u32_at(message, Self::FLAGS)? & Self::FLAG_ANY != 0,
            event: u64_at(message, Self::EVENT)?,
            release_event: u8_at(message, Self::RELEASE)? != 0,
        })
    }
}

/// A queue or context must not start what comes next until these are reached.
#[derive(Debug, Clone, Copy)]
pub struct WaitOnGpu<'a> {
    pub queue_or_context: u32,
    pub values: U64List<'a>,
    pub objects: U32List<'a>,
}

impl<'a> WaitOnGpu<'a> {
    const HANDLE: usize = 0x18;
    const COUNT: usize = 0x1C;
    const VALUES: usize = 0x28;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let count = u32_at(message, Self::COUNT)? as usize;
        Ok(Self {
            queue_or_context: u32_at(message, Self::HANDLE)?,
            values: U64List::at(message, Self::VALUES, count)?,
            objects: U32List::at(message, Self::VALUES + 8 * count, count)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{set_u32, set_u64};

    fn signal(objects: &[u32], contexts: &[u32], values: &[u64], event: Option<u64>) -> Vec<u8> {
        let mut message = vec![0; 0x38];
        set_u32(&mut message, 0x18, objects.len() as u32);
        set_u32(&mut message, 0x20, contexts.len() as u32);
        if let Some(event) = event {
            set_u32(&mut message, 0x1C, 2);
            set_u64(&mut message, 0x30, event);
        }
        objects.iter().chain(contexts).for_each(|word| message.extend_from_slice(&word.to_le_bytes()));
        values.iter().for_each(|value| message.extend_from_slice(&value.to_le_bytes()));
        message
    }

    #[test]
    fn lists_follow_one_another() {
        let message = signal(&[0x10, 0x20], &[0x30], &[5, 6], None);
        let parsed = SignalSyncObject::parse(&message).unwrap();
        assert_eq!(parsed.objects.iter().collect::<Vec<_>>(), [0x10, 0x20]);
        assert_eq!(parsed.contexts.iter().collect::<Vec<_>>(), [0x30]);
        assert_eq!(parsed.values.unwrap().iter().collect::<Vec<_>>(), [5, 6]);
        assert_eq!(parsed.cpu_event, None);
    }

    #[test]
    fn a_signal_may_have_no_values() {
        let message = signal(&[1], &[], &[], None);
        let parsed = SignalSyncObject::parse(&message).unwrap();
        assert!(parsed.values.is_none());
    }

    #[test]
    fn the_cpu_event_comes_from_its_field() {
        let message = signal(&[], &[2], &[], Some(0xE));
        let parsed = SignalSyncObject::parse(&message).unwrap();
        assert_eq!(parsed.cpu_event, Some(0xE));
    }

    #[test]
    fn an_object_list_past_the_end_is_refused() {
        let mut message = signal(&[1], &[], &[], None);
        set_u32(&mut message, 0x18, 100);
        assert!(SignalSyncObject::parse(&message).is_err());
    }

    #[test]
    fn wait_on_cpu_reads_objects_then_values() {
        let mut message = vec![0; 0x38];
        set_u32(&mut message, 0x1C, 1);
        set_u32(&mut message, 0x20, 1);
        set_u64(&mut message, 0x28, 0xEE);
        message[0x30] = 1;
        message.extend_from_slice(&0x40u32.to_le_bytes());
        message.extend_from_slice(&9u64.to_le_bytes());
        let parsed = WaitOnCpu::parse(&message).unwrap();
        assert!(parsed.wake_on_any && parsed.release_event);
        assert_eq!(parsed.objects.iter().next(), Some(0x40));
        assert_eq!(parsed.values.iter().next(), Some(9));
    }

    #[test]
    fn wait_on_gpu_reads_values_then_objects() {
        let mut message = vec![0; 0x28];
        set_u32(&mut message, 0x18, 0x80);
        set_u32(&mut message, 0x1C, 1);
        message.extend_from_slice(&9u64.to_le_bytes());
        message.extend_from_slice(&0x40u32.to_le_bytes());
        let parsed = WaitOnGpu::parse(&message).unwrap();
        assert_eq!(parsed.queue_or_context, 0x80);
        assert_eq!(parsed.objects.iter().next(), Some(0x40));
        assert_eq!(parsed.values.iter().next(), Some(9));
    }
}
