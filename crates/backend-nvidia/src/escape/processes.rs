//! The guest's processes on the card, for the vendor's own tools. Two controls ask: one for the
//! PIDs, one for what each holds. Both are answered from the core's books and never reach the RM,
//! so a guest hears of its own processes and no other's. The layouts are the public headers'.
//!
//! The PIDs are asked for by class, to sort processes into compute and graphics. The books don't
//! say which objects a process holds, only its memory, so the list is the same whatever class is
//! asked about, and a tool marks every process as both.

use super::{get, put};
use guesthouse_backend::ProcessMemory;

pub(super) const CONTROL_GET_PIDS: u32 = 0x2080_018D;
pub(super) const CONTROL_GET_PID_INFO: u32 = 0x2080_018E;

/// The PID list: what kind of thing the ID names, the ID, a count, and the table.
const PIDS_KIND: usize = 0x00;
const PIDS_COUNT: usize = 0x08;
const PIDS_TABLE: usize = 0x0C;
const PIDS_MOST: usize = 950;
pub(super) const PIDS_SIZE: usize = PIDS_TABLE + PIDS_MOST * 4;
/// The one kind of ID a guest may ask by: a class. The other names a VM, and a guest has none to ask about.
const PIDS_BY_CLASS: u32 = 0;

/// The per-process query: a count, then entries of a PID, an index, a result and the figures.
const INFO_COUNT: usize = 0x00;
const INFO_LIST: usize = 0x08;
const INFO_MOST: usize = 200;
const ENTRY_PID: usize = 0x00;
const ENTRY_INDEX: usize = 0x04;
const ENTRY_RESULT: usize = 0x08;
const ENTRY_PRIVATE_BYTES: usize = 0x10;
const ENTRY_SIZE: usize = 72;
pub(super) const INFO_SIZE: usize = INFO_LIST + INFO_MOST * ENTRY_SIZE;
const INDEX_VIDEO_MEMORY: u32 = 0;

const RESULT_OK: u32 = 0;
const RESULT_INVALID_ARGUMENT: u32 = 0x1D;
const RESULT_OBJECT_NOT_FOUND: u32 = 0x57;

fn put_long(params: &mut [u8], at: usize, value: u64) {
    if let Some(field) = params.get_mut(at..at + 8) {
        field.copy_from_slice(&value.to_le_bytes());
    }
}

/// Fills the table with this guest's processes. Returns whether the question was one a guest may ask.
pub(super) fn list(processes: &[ProcessMemory], params: &mut [u8]) -> bool {
    if get(params, PIDS_KIND) != Some(PIDS_BY_CLASS) {
        return false;
    }
    let listed = processes.iter().take(PIDS_MOST);
    put(params, PIDS_COUNT, listed.len() as u32);
    for (slot, process) in listed.enumerate() {
        put(params, PIDS_TABLE + slot * 4, process.pid);
    }
    true
}

/// Fills in what each named process holds. A PID that isn't one of the guest's is not found, which
/// is also what the RM says of a process that has nothing on the card.
pub(super) fn describe(processes: &[ProcessMemory], params: &mut [u8]) {
    let count = get(params, INFO_COUNT).unwrap_or(0) as usize;
    for slot in 0..count.min(INFO_MOST) {
        let entry = INFO_LIST + slot * ENTRY_SIZE;
        let pid = get(params, entry + ENTRY_PID);
        let found = processes.iter().find(|process| Some(process.pid) == pid);
        let result = match (get(params, entry + ENTRY_INDEX), found) {
            (Some(INDEX_VIDEO_MEMORY), Some(process)) => {
                put_long(params, entry + ENTRY_PRIVATE_BYTES, process.bytes);
                RESULT_OK
            }
            (Some(INDEX_VIDEO_MEMORY), None) => RESULT_OBJECT_NOT_FOUND,
            _ => RESULT_INVALID_ARGUMENT,
        };
        put(params, entry + ENTRY_RESULT, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held() -> Vec<ProcessMemory> {
        vec![ProcessMemory { pid: 1532, bytes: 3 << 20 }, ProcessMemory { pid: 6600, bytes: 1 << 30 }]
    }

    fn long(params: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(params[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn the_list_is_this_guests_processes() {
        let mut params = vec![0; PIDS_SIZE];
        assert!(list(&held(), &mut params));
        assert_eq!(get(&params, PIDS_COUNT), Some(2));
        assert_eq!((get(&params, PIDS_TABLE), get(&params, PIDS_TABLE + 4)), (Some(1532), Some(6600)));
    }

    #[test]
    fn a_question_about_vms_is_not_answered() {
        let mut params = vec![0; PIDS_SIZE];
        put(&mut params, PIDS_KIND, 1);
        assert!(!list(&held(), &mut params));
        assert_eq!(get(&params, PIDS_COUNT), Some(0));
    }

    #[test]
    fn each_process_hears_what_it_holds() {
        let mut params = vec![0; INFO_SIZE];
        put(&mut params, INFO_COUNT, 2);
        put(&mut params, INFO_LIST + ENTRY_PID, 6600);
        put(&mut params, INFO_LIST + ENTRY_SIZE + ENTRY_PID, 1532);
        describe(&held(), &mut params);
        assert_eq!(get(&params, INFO_LIST + ENTRY_RESULT), Some(RESULT_OK));
        assert_eq!(long(&params, INFO_LIST + ENTRY_PRIVATE_BYTES), 1 << 30);
        assert_eq!(long(&params, INFO_LIST + ENTRY_SIZE + ENTRY_PRIVATE_BYTES), 3 << 20);
    }

    #[test]
    fn a_pid_that_is_not_the_guests_is_not_found() {
        let mut params = vec![0; INFO_SIZE];
        put(&mut params, INFO_COUNT, 1);
        put(&mut params, INFO_LIST + ENTRY_PID, 4);
        describe(&held(), &mut params);
        assert_eq!(get(&params, INFO_LIST + ENTRY_RESULT), Some(RESULT_OBJECT_NOT_FOUND));
        assert_eq!(long(&params, INFO_LIST + ENTRY_PRIVATE_BYTES), 0);
    }

    #[test]
    fn an_index_this_does_not_keep_is_refused_in_its_entry() {
        let mut params = vec![0; INFO_SIZE];
        put(&mut params, INFO_COUNT, 1);
        put(&mut params, INFO_LIST + ENTRY_PID, 6600);
        put(&mut params, INFO_LIST + ENTRY_INDEX, 7);
        describe(&held(), &mut params);
        assert_eq!(get(&params, INFO_LIST + ENTRY_RESULT), Some(RESULT_INVALID_ARGUMENT));
    }

    #[test]
    fn a_count_past_the_table_stops_at_its_end() {
        let mut params = vec![0; INFO_SIZE];
        put(&mut params, INFO_COUNT, u32::MAX);
        describe(&held(), &mut params);
        assert_eq!(
            get(&params, INFO_LIST + (INFO_MOST - 1) * ENTRY_SIZE + ENTRY_RESULT),
            Some(RESULT_OBJECT_NOT_FOUND)
        );
    }
}
