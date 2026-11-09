//! What is running on this PC, read from `/proc`, so a VM started some other way counts too.

use crate::context::Context;
use crate::daemon::is_own_daemon;
use crate::held_memory::holders;
use crate::machine::{Machine, sorted_names};
use crate::monitor::MAIN_SOCKET;
use crate::slots;
use crate::tasks::{read_task, task_alive, task_path};
use crate::text::qemu_value;
use guesthouse_daemon::slot::Slot;
use guesthouse_jail::launch::start_time;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;

type Words = Vec<Vec<u8>>;

/// The RAM's file in shared memory, for a version before the memfds.
const OLD_RAM_FILE: &str = "ram";

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

/// The passt a jailed QEMU of this slot was started with, by the pid and start time the launcher
/// noted, so a pid the system has given to something else since isn't taken for it.
pub fn passt_pid(context: &Context, slot: Option<Slot>) -> Option<u32> {
    let pid_file = context.run_folder(slot).join(crate::vm_args::PASST_PID_FILE);
    let noted = fs::read_to_string(&pid_file).ok()?;
    let (pid, started) = noted.trim().split_once(' ')?;
    let pid: u32 = pid.parse().ok()?;
    let stat = fs::read_to_string(context.processes.join(pid.to_string()).join("stat")).ok()?;
    let same = start_time(&stat).is_some_and(|time| time.to_string() == started);
    same.then_some(pid)
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

/// Whether another VM on this PC has a slot's memory. See [`crate::held_memory`]. A daemon of ours
/// for this slot doesn't count, as it holds the memory until it's told to stop.
pub fn memory_taken(context: &Context, slot: Option<Slot>) -> bool {
    let slot = slot.unwrap_or(context.slot);
    holders(context, slot).iter().any(|(pid, _)| !is_own_daemon(context, &pid.to_string(), slot))
}

/// Identifies what a VM left behind if the guest shut it down, as nothing cleaned up then: the
/// memory our daemon for the slot still holds, all pinned, or a file a version before the memfds
/// left in shared memory. Without QEMU either is stale unless a stop is just getting to it, so the
/// caller waits to see the same one again.
pub fn leftovers(context: &Context, slot: Option<Slot>) -> Option<u64> {
    let slot = slot.unwrap_or(context.slot);
    if context.second_instance()
        || qemu_pid(context, Some(slot)).is_some()
        || memory_taken(context, Some(slot))
    {
        return None;
    }
    let old_file = fs::metadata(context.paths.shared_file(OLD_RAM_FILE, slot)).ok().map(|found| found.ino());
    old_file.or_else(|| holders(context, slot).first().map(|(_, inode)| *inode))
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
    fn passt_is_found_by_its_pid_and_only_if_it_started_when_noted() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = context.run_folder(None);
        fs::create_dir_all(&folder).unwrap();
        assert_eq!(passt_pid(&context, None), None);
        fs::write(folder.join(crate::vm_args::PASST_PID_FILE), "4242 987654\n").unwrap();
        assert_eq!(passt_pid(&context, None), None, "no such process");
        fake_process(&context, 4242, &["/usr/bin/passt", "--quiet"], &[], "");
        let stat = |started: u32| {
            format!("4242 (passt.avx2) S 1 4242 4242 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {started} 1 2")
        };
        let stat_file = context.processes.join("4242/stat");
        fs::write(&stat_file, stat(1_000_000)).unwrap();
        assert_eq!(passt_pid(&context, None), None, "a pid given to something else since");
        fs::write(&stat_file, stat(987_654)).unwrap();
        assert_eq!(passt_pid(&context, None), Some(4242));
        assert_eq!(passt_pid(&context, Some(slots::number(1))), None);
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
    fn a_memfd_our_daemon_still_holds_is_a_leftover_and_anyone_elses_a_taken_slot() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let slot = slots::number(1);
        fake_daemon(&context, 100, 1, "/memfd:gpupv-iomem-1 (deleted)");
        assert!(!memory_taken(&context, Some(slot)));
        assert_eq!(leftovers(&context, Some(slot)), Some(42), "nothing else frees it once QEMU has gone");
        assert_eq!(leftovers(&context, Some(slots::first())), None);
        fake_process(&context, 200, &["/other/qemu"], &[], "/memfd:gpupv-ram-1 (deleted)");
        assert!(memory_taken(&context, Some(slot)), "another install's VM is in the slot");
        assert_eq!(leftovers(&context, Some(slot)), None);
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
