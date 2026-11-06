//! Sync objects. One object can have many handles, and goes away with the last.

use std::collections::HashMap;

/// A sync object's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncObject {
    /// Guest physical address of its counter, or zero if it has none.
    pub fence: u64,
    pub gpu_address: u64,
    /// The guest event to wake instead of counting.
    pub event: Option<u64>,
    references: u32,
}

/// Handles to sync objects.
#[derive(Debug, Default)]
pub struct SyncTable {
    objects: Vec<SyncObject>,
    by_handle: HashMap<u32, usize>,
}

impl SyncTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, handle: u32, fence: u64, gpu_address: u64, event: Option<u64>) {
        self.objects.push(SyncObject { fence, gpu_address, event, references: 1 });
        self.by_handle.insert(handle, self.objects.len() - 1);
    }

    pub fn get(&self, handle: u32) -> Option<&SyncObject> {
        self.by_handle.get(&handle).map(|&index| &self.objects[index])
    }

    /// Gives an existing object one more handle. Returns the object, or `None` if `existing` is
    /// unknown.
    pub fn open(&mut self, existing: u32, handle: u32) -> Option<SyncObject> {
        let index = *self.by_handle.get(&existing)?;
        let object = &mut self.objects[index];
        object.references = object.references.saturating_add(1);
        self.by_handle.insert(handle, index);
        Some(self.objects[index])
    }

    /// Removes a handle. Returns the object if that was its last.
    pub fn close(&mut self, handle: u32) -> Option<SyncObject> {
        let index = self.by_handle.remove(&handle)?;
        let object = &mut self.objects[index];
        object.references = object.references.saturating_sub(1);
        (object.references == 0).then_some(*object)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_lasts_until_its_last_handle() {
        let mut table = SyncTable::new();
        table.insert(1, 0x1000, 0x2000, None);
        assert_eq!(table.open(1, 2).map(|object| object.fence), Some(0x1000));
        assert_eq!(table.close(1), None);
        assert_eq!(table.get(2).map(|object| object.gpu_address), Some(0x2000));
        assert_eq!(table.close(2).map(|object| object.fence), Some(0x1000));
        assert_eq!(table.get(2), None);
    }

    #[test]
    fn the_count_of_handles_stops_at_its_limit() {
        let mut table = SyncTable::new();
        table.insert(1, 0x1000, 0x2000, None);
        table.objects[0].references = u32::MAX;
        assert!(table.open(1, 2).is_some());
        assert_eq!(table.objects[0].references, u32::MAX);
    }

    #[test]
    fn unknown_handles_are_harmless() {
        let mut table = SyncTable::new();
        assert_eq!(table.open(9, 10), None);
        assert_eq!(table.close(9), None);
    }
}
