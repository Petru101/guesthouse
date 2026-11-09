//! Who holds a slot's guest memory, and how much of the PC's RAM each slot's memory takes, read from
//! `/proc`. The memory has no file now, so it's found by its memfd's name, which is the same for
//! every install: a clone or a test run can hold a slot's while this install's VM is off. A file in
//! `/dev/shm` of the same name, from a version before the memfds, counts as well.

use crate::context::Context;
use crate::machine::sorted_names;
use crate::slots;
use guesthouse_daemon::handover::{Part, shown_file_name};
use guesthouse_daemon::slot::Slot;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::MetadataExt;

/// The size of the blocks `st_blocks` counts.
const FILE_BLOCK: u64 = 512;
/// What follows a deleted file's path in `/proc/PID/maps`.
const DELETED: &str = " (deleted)";
/// The fields of a `maps` line before its path: addresses, permissions, offset, device and inode.
const MAPS_FIELDS: usize = 5;
const INODE_FIELD: usize = 4;

fn pids(context: &Context) -> Vec<u32> {
    sorted_names(&context.processes).iter().filter_map(|name| name.parse().ok()).collect()
}

/// What a `maps` line ends with when it maps this slot's memory, now or from an older version.
fn shown_names(context: &Context, slot: Slot) -> Vec<String> {
    let mut names = Vec::new();
    for part in Part::ALL {
        names.push(part.shown_as(slot));
        let old = context.paths.shared_file(part.word(), slot).display().to_string();
        names.push(format!("{old}{DELETED}"));
        names.push(old);
    }
    names
}

/// The inode of what a `maps` line maps, if its path is one of these.
fn mapped_inode(line: &str, names: &[String]) -> Option<u64> {
    let mut fields = line.splitn(MAPS_FIELDS + 1, ' ');
    let inode = fields.clone().nth(INODE_FIELD)?.parse().ok()?;
    let path = fields.nth(MAPS_FIELDS)?.trim_start();
    names.iter().any(|name| name == path).then_some(inode)
}

/// Each process with some of this slot's memory mapped, with the inode of what it maps.
pub fn holders(context: &Context, slot: Slot) -> Vec<(u32, u64)> {
    let names = shown_names(context, slot);
    let mut found = Vec::new();
    for pid in pids(context) {
        // It has gone, or belongs to another user.
        let Ok(maps) = fs::read(context.processes.join(pid.to_string()).join("maps")) else { continue };
        let maps = String::from_utf8_lossy(&maps);
        if let Some(inode) = maps.lines().find_map(|line| mapped_inode(line, &names)) {
            found.push((pid, inode));
        }
    }
    found
}

/// The slot a memfd of ours is for, from the last part of its link.
fn slot_of_memfd(file_name: &str) -> Option<Slot> {
    let is_named = |slot: &Slot| Part::ALL.iter().any(|part| shown_file_name(&part.name(*slot)) == file_name);
    slots::all().find(is_named)
}

/// How much RAM each slot's memory takes now, in bytes, by slot number: what has been written to it
/// or claimed, and not its size. A memfd counts once, however many processes have it open.
pub fn memory_used(context: &Context) -> BTreeMap<u8, u64> {
    let mut used = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for pid in pids(context) {
        let folder = context.processes.join(pid.to_string()).join("fd");
        for name in sorted_names(&folder) {
            let link = folder.join(name);
            let Ok(target) = fs::read_link(&link) else { continue };
            let file_name = target.file_name().unwrap_or_default().to_string_lossy();
            let Some(slot) = slot_of_memfd(&file_name) else { continue };
            // The link leads to the memfd itself, whatever its target says.
            let Ok(found) = fs::metadata(&link) else { continue };
            if seen.insert(found.ino()) {
                *used.entry(slot.number()).or_default() += found.blocks() * FILE_BLOCK;
            }
        }
    }
    for slot in slots::all() {
        for part in Part::ALL {
            if let Ok(found) = fs::metadata(context.paths.shared_file(part.word(), slot))
                && seen.insert(found.ino())
            {
                *used.entry(slot.number()).or_default() += found.blocks() * FILE_BLOCK;
            }
        }
    }
    used
}

/// What the other slots' memory takes, in bytes: what this slot's machine can't have.
pub fn used_by_others(context: &Context, slot: Slot) -> u64 {
    memory_used(context).iter().filter(|(number, _)| **number != slot.number()).map(|(_, bytes)| bytes).sum()
}

#[cfg(test)]
pub mod stand_in {
    use crate::context::Context;
    use crate::testing::Scratch;
    use std::fs;
    use std::os::unix::fs::symlink;

    /// A process holding a memfd of this name with this many bytes in it. The link goes to a file of
    /// the memfd's shown name, as the kernel's does.
    pub fn holds_memfd(context: &Context, scratch: &Scratch, pid: u32, shown: &str, bytes: usize) {
        let file = scratch.path(&format!("memfds/{pid}/{}", shown.trim_start_matches('/')));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, vec![1; bytes]).unwrap();
        let folder = context.processes.join(pid.to_string()).join("fd");
        fs::create_dir_all(&folder).unwrap();
        let number = fs::read_dir(&folder).unwrap().count() + 3;
        symlink(&file, folder.join(number.to_string())).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::holds_memfd;
    use super::*;
    use crate::processes::stand_in::fake_process;
    use crate::testing::{Scratch, scratch_context};
    use std::os::unix::fs::symlink;

    const MIB: usize = 1 << 20;

    #[test]
    fn a_maps_line_counts_only_for_its_own_names() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let names = shown_names(&context, slots::number(1));
        let line = |path: &str| format!("7f0000000000-7f0400000000 rw-s 00000000 00:01 4242     {path}");
        assert_eq!(mapped_inode(&line("/memfd:gpupv-iomem-1 (deleted)"), &names), Some(4242));
        assert_eq!(mapped_inode(&line("/memfd:gpupv-iomem (deleted)"), &names), None, "slot 0's");
        assert_eq!(mapped_inode(&line("/memfd:gpupv-iomem-12 (deleted)"), &names), None);
        let old = context.paths.shared_file("ram", slots::number(1)).display().to_string();
        assert_eq!(mapped_inode(&line(&format!("{old} (deleted)")), &names), Some(4242));
        assert_eq!(mapped_inode("7f00-7f01 r--p 00000000 00:00 0", &names), None);
    }

    #[test]
    fn a_memfd_counts_once_for_its_slot_by_what_it_holds() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        holds_memfd(&context, &scratch, 100, "/memfd:gpupv-ram-2 (deleted)", MIB);
        holds_memfd(&context, &scratch, 100, "/memfd:gpupv-iomem-2 (deleted)", MIB);
        holds_memfd(&context, &scratch, 101, "/memfd:gpupv-ram (deleted)", 2 * MIB);
        holds_memfd(&context, &scratch, 102, "/memfd:something-else (deleted)", 4 * MIB);
        // The daemon has QEMU's memfd too, which must not count twice.
        let fd = context.processes.join("103/fd");
        fs::create_dir_all(&fd).unwrap();
        symlink(fs::read_link(context.processes.join("100/fd/3")).unwrap(), fd.join("12")).unwrap();
        let used = memory_used(&context);
        let at_least = |slot: u8, bytes: usize| used.get(&slot).is_some_and(|found| *found >= bytes as u64);
        assert!(at_least(2, 2 * MIB) && used[&2] < 3 * MIB as u64, "{used:?}");
        assert!(at_least(0, 2 * MIB), "{used:?}");
        assert_eq!(used.len(), 2, "nothing of anyone else's: {used:?}");
        assert_eq!(used_by_others(&context, slots::number(2)), used[&0]);
    }

    #[test]
    fn an_old_versions_file_in_shared_memory_still_takes_ram() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fs::write(context.paths.shared_file("ram", slots::number(3)), vec![1; MIB]).unwrap();
        assert!(memory_used(&context)[&3] >= MIB as u64);
    }

    #[test]
    fn holders_are_found_by_what_they_map() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fake_process(&context, 300, &["/qemu"], &[], "/memfd:gpupv-ram-1 (deleted)");
        fake_process(&context, 301, &["/qemu"], &[], "/memfd:gpupv-ram (deleted)");
        assert_eq!(holders(&context, slots::number(1)), [(300, 42)]);
        assert_eq!(holders(&context, slots::first()), [(301, 42)]);
    }
}
