//! What is running on this PC, read from `/proc`, so a VM started some other way counts too.

use crate::context::Context;
use crate::daemon::is_own_daemon;
use crate::host::QEMU_FILES;
use crate::machine::{Machine, sorted_names};
use crate::monitor::MAIN_SOCKET;
use crate::slots;
use crate::tasks::{read_task, task_alive, task_path};
use crate::text::qemu_value;
use guesthouse_daemon::slot::Slot;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;

type Words = Vec<Vec<u8>>;

fn process_words(context: &Context, pid: u32) -> Option<Words> {
    let data = fs::read(context.processes.join(pid.to_string()).join("cmdline")).ok()?;
    Some(data.split(|byte| *byte == 0).map(<[u8]>::to_vec).collect())
}

fn pids(context: &Context) -> Vec<u32> {
    sorted_names(&context.processes).iter().filter_map(|name| name.parse().ok()).collect()
}

/// Every process of this install's QEMU, as its pid and its words.
pub fn qemu_processes(context: &Context) -> Vec<(u32, Words)> {
    let program = context.paths.qemu.as_os_str().as_encoded_bytes();
    let of_pid = |pid| Some((pid, process_words(context, pid)?));
    pids(context).into_iter().filter_map(of_pid).filter(|(_, words)| words[0] == program).collect()
}

fn monitor_word(context: &Context, slot: Slot) -> Vec<u8> {
    let folder = context.paths.run_folder(slot);
    format!("unix:{}/{MAIN_SOCKET},", folder.display()).into_bytes()
}

fn names_slot(context: &Context, words: &Words, slot: Slot) -> bool {
    let wanted = monitor_word(context, slot);
    let pid_file = context.paths.run_folder(slot).join(crate::vm_args::PID_FILE);
    let pid_file = pid_file.as_os_str().as_encoded_bytes();
    words.iter().any(|word| word.starts_with(&wanted) || word == pid_file)
}

/// Finds a VM by its program and run folder.
pub fn qemu_pid(context: &Context, slot: Option<Slot>) -> Option<u32> {
    let slot = slot.unwrap_or(context.slot);
    qemu_processes(context)
        .into_iter()
        .find(|(_, words)| names_slot(context, words, slot))
        .map(|found| found.0)
}

/// Each running VM, by slot number.
pub fn running_slots(context: &Context) -> BTreeMap<u8, u32> {
    let mut found = BTreeMap::new();
    for (pid, words) in qemu_processes(context) {
        for slot in slots::all() {
            if names_slot(context, &words, slot) {
                found.insert(slot.number(), pid);
            }
        }
    }
    found
}

/// Whether one of QEMU's words opens this machine's disk. A comma after the name ends it, unless it's
/// doubled, when the name goes on and this is some other disk.
pub(crate) fn opens_disk(word: &[u8], machine: &Machine) -> bool {
    let wanted = format!("file={},", qemu_value(&machine.disk)).into_bytes();
    let found = word.windows(wanted.len()).position(|part| part == wanted);
    found.is_some_and(|at| word.get(at + wanted.len()) != Some(&b','))
}

pub fn uses_disk(context: &Context, pid: u32, machine: &Machine) -> bool {
    process_words(context, pid).is_some_and(|words| words.iter().any(|word| opens_disk(word, machine)))
}

/// Whether another VM on this PC has a slot's shared memory. The names there are the same for every
/// install, so a clone or a test run can hold them while this install's VM is off. Each line must end
/// in the name, or slot 1's file would count for slot 0 as well. A daemon of ours for this slot
/// doesn't count, as it holds the memory until it's told to stop.
pub fn memory_taken(context: &Context, slot: Option<Slot>) -> bool {
    let slot = slot.unwrap_or(context.slot);
    let path = context.paths.shared_file(QEMU_FILES[0], slot);
    if !path.exists() {
        return false;
    }
    let (plain, deleted) = (path.display().to_string(), format!("{} (deleted)", path.display()));
    pids(context).iter().any(|pid| {
        let pid = pid.to_string();
        // It has gone, or belongs to another user.
        let Ok(maps) = fs::read(context.processes.join(&pid).join("maps")) else { return false };
        let maps = String::from_utf8_lossy(&maps);
        let held = maps.lines().any(|line| line.ends_with(&plain) || line.ends_with(&deleted));
        held && !is_own_daemon(context, &pid, slot)
    })
}

/// Identifies what a VM left behind if the guest shut it down, as nothing cleaned up then. Only QEMU
/// makes this file, so without QEMU it's stale unless a stop is just getting to it. The caller waits
/// to see the same file again, and the daemon is still holding it all pinned.
pub fn leftovers(context: &Context, slot: Option<Slot>) -> Option<u64> {
    if context.second_instance() || qemu_pid(context, slot).is_some() || memory_taken(context, slot) {
        return None;
    }
    let path = context.paths.shared_file(QEMU_FILES[0], slot.unwrap_or(context.slot));
    fs::metadata(path).ok().map(|found| found.ino())
}

/// Each slot's leftovers, by slot number, for this install to clear up. Another install's VM can be in
/// a slot too, so only the slots this one has run something in count. A task that's still going may
/// have just reserved the memory for a VM that's on its way.
pub fn all_leftovers(context: &Context) -> BTreeMap<u8, u64> {
    let ours = slots::all().filter(|slot| {
        task_path(context, Some(*slot)).exists() && !task_alive(context, &read_task(context, Some(*slot)))
    });
    ours.filter_map(|slot| Some((slot.number(), leftovers(context, Some(slot))?))).collect()
}

#[cfg(test)]
pub mod stand_in {
    use crate::context::Context;
    use std::fs;

    /// A process with these words, and this file mapped.
    pub fn fake_process(context: &Context, pid: u32, words: &[&str], environment: &[&str], mapped: &str) {
        let folder = context.processes.join(pid.to_string());
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("cmdline"), words.join("\0") + "\0").unwrap();
        fs::write(folder.join("environ"), environment.join("\0") + "\0").unwrap();
        fs::write(
            folder.join("maps"),
            format!("7f0000000000-7f0400000000 rw-s 00000000 00:1b 42 {mapped}\n"),
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::fake_process;
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    fn daemon_words(context: &Context) -> [String; 2] {
        [
            context.paths.binary("guesthouse-daemon").display().to_string(),
            context.paths.runtime.display().to_string(),
        ]
    }

    fn fake_daemon(context: &Context, pid: u32, slot: u8, mapped: &str) {
        let [program, base] = daemon_words(context);
        fake_process(context, pid, &[&program, &base], &[&format!("GUESTHOUSE_SLOT={slot}")], mapped);
    }

    #[test]
    fn memory_held_by_another_vm_is_not_ours_to_clean_up() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let (first, second) = (slots::first(), slots::number(1));
        let memory = context.paths.shared_file("ram", first);
        let memory_1 = context.paths.shared_file("ram", second);
        assert_eq!(leftovers(&context, Some(first)), None);
        assert!(!memory_taken(&context, Some(first)));
        fs::write(&memory, "").unwrap();
        let inode = fs::metadata(&memory).unwrap().ino();
        assert_eq!(leftovers(&context, Some(first)), Some(inode), "with no one holding it, it is a leftover");
        let text = memory.display().to_string();
        fake_daemon(&context, 100, 0, &text);
        assert_eq!(
            leftovers(&context, Some(first)),
            Some(inode),
            "our own daemon still holds it after a shutdown"
        );
        fake_process(
            &context,
            200,
            &["/mnt/other/guesthouse-daemon", "/mnt/other/run"],
            &["GUESTHOUSE_SLOT=0"],
            &text,
        );
        assert!(memory_taken(&context, Some(first)), "another install's daemon is another VM");
        assert_eq!(leftovers(&context, Some(first)), None);
        fs::remove_dir_all(context.processes.join("200")).unwrap();
        fs::write(&memory_1, "").unwrap();
        fake_daemon(&context, 201, 1, &memory_1.display().to_string());
        assert_eq!(leftovers(&context, Some(second)), Some(fs::metadata(&memory_1).unwrap().ino()));
        assert!(!memory_taken(&context, Some(first)), "slot 1's file must not count for slot 0");
        fake_daemon(&context, 202, 1, &text);
        assert!(memory_taken(&context, Some(first)), "and slot 1's daemon is no business of slot 0's memory");
        fs::remove_dir_all(context.processes.join("202")).unwrap();
        fake_process(&context, 300, &["/else/qemu", "-name", "gpupv"], &[], &format!("{text} (deleted)"));
        assert_eq!(leftovers(&context, Some(first)), None, "deleted or not, the name counts");
        assert!(all_leftovers(&context).is_empty(), "a slot this install never used is left alone");
        let task = task_path(&context, Some(second));
        fs::create_dir_all(task.parent().unwrap()).unwrap();
        fs::write(task, "{}").unwrap();
        let found = all_leftovers(&context);
        assert_eq!(
            found,
            BTreeMap::from([(1, fs::metadata(&memory_1).unwrap().ino())]),
            "each slot for itself"
        );
    }

    #[test]
    fn a_second_instance_has_no_leftovers() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        fs::write(context.paths.shared_file("ram", slots::first()), "").unwrap();
        context.paths.own_run = Some(scratch.path("second"));
        assert_eq!(leftovers(&context, None), None);
    }

    #[test]
    fn a_vm_is_found_by_program_and_run_folder() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let qemu = context.paths.qemu.display().to_string();
        let socket = format!(
            "unix:{}/qmp.sock,server=on,wait=off",
            context.paths.run_folder(slots::number(2)).display()
        );
        fake_process(
            &context,
            50,
            &[&qemu, "-qmp", &socket, "-drive", "if=none,file=/d,,e,format=qcow2"],
            &[],
            "",
        );
        fake_process(&context, 51, &["/other/qemu", "-qmp", &socket], &[], "");
        assert_eq!(running_slots(&context), BTreeMap::from([(2, 50)]));
        assert_eq!(qemu_pid(&context, Some(slots::number(2))), Some(50));
        assert_eq!(qemu_pid(&context, None), None);
        let machine = Machine::new("x", "x", "/d,e", "/v");
        assert!(uses_disk(&context, 50, &machine));
        assert!(!uses_disk(&context, 50, &Machine::new("y", "y", "/d", "/v")));
    }
}
