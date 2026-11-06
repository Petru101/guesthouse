//! Tasks as the app sees them: the small file each one writes, what is going on with a machine, and
//! starting one. Actions themselves are in [`crate::actions`].

use crate::context::{Context, SLOT_VARIABLE};
use crate::daemon::detached;
use crate::display::{read_state, ring_in, screen_text};
use crate::failure::{Failure, Result};
use crate::log::open_for_appending;
use crate::machine::Machine;
use crate::processes::{leftovers, memory_taken, running_slots, uses_disk};
use crate::settings::write_json;
use crate::slots;
use guesthouse_daemon::gpu_note::read_note;
use guesthouse_daemon::slot::{SLOTS, Slot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

pub const TASK_PROGRAM: &str = "guesthouse-task";
const TASK_FILE: &str = "guesthouse-task.json";
const STILL_BUSY: &str = "Something else is still in progress.";
const QEMU_PAUSED_WORDS: [&str; 2] = ["paused", "debug"];
/// How `/proc/PID/stat` says a process has ended: a zombie, and one on its way out.
const ENDED_STATES: [char; 2] = ['Z', 'X'];
const TASK_DIED: &str = "The task stopped before it finished. The log has more.";
/// How a machine is told it would be one too many.
/// Why there's no slot. Slots and their shared memory are the whole PC's, so another user's or another
/// install's VM really does take one, and it's only fair to say whose they are.
pub fn no_slot_text(context: &Context) -> String {
    let ours = running_slots(context).len();
    match (SLOTS as usize).saturating_sub(ours) {
        0 => format!("{SLOTS} VMs are running, which is as many as Guesthouse runs at once."),
        others => {
            format!("All {SLOTS} slots are taken, {others} of them by another user or install on this PC.")
        }
    }
}

/// Whether QEMU's word for the guest means it was left stopped.
pub fn is_paused(state: &str) -> bool {
    QEMU_PAUSED_WORDS.contains(&state)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub machine: String,
    /// Zero once it has finished, so a pid left here means it was killed and couldn't say so.
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub step: String,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Off,
    Starting,
    Installing,
    Running,
    Stopping,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Starting => "Starting",
            Self::Installing => "Installing",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// What is happening now.
    pub step: String,
    /// What went wrong last.
    pub error: String,
    /// A task for this machine is running.
    pub busy: bool,
    /// Which VM it is, if it's running or has a task.
    pub slot: Option<u8>,
    /// What's really on the guest's screen, in words, while it's up.
    pub screen: String,
    /// Why the screen isn't what was asked for.
    pub display_problem: String,
}

pub fn task_path(context: &Context, slot: Option<Slot>) -> PathBuf {
    context.run_folder(slot).join(TASK_FILE)
}

pub fn read_task(context: &Context, slot: Option<Slot>) -> TaskRecord {
    let text = fs::read_to_string(task_path(context, slot)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn write_task(context: &Context, slot: Option<Slot>, record: &TaskRecord) -> Result<()> {
    write_json(&task_path(context, slot), record)
}

pub fn task_alive(context: &Context, record: &TaskRecord) -> bool {
    let file = context.processes.join(record.pid.to_string()).join("cmdline");
    let words = fs::read(file).unwrap_or_default();
    words.windows(TASK_PROGRAM.len()).any(|part| part == TASK_PROGRAM.as_bytes())
}

/// Whether a task's process has really gone. `task_alive` can't say: `spawn` returns while the kernel
/// is still loading the task, and until that's done its name isn't there to find.
fn task_ended(context: &Context, record: &TaskRecord) -> bool {
    let file = context.processes.join(record.pid.to_string()).join("stat");
    let stat = fs::read_to_string(file).unwrap_or_default();
    // The state comes after the program's name, which can hold anything, brackets too.
    let state = stat.rsplit_once(')').and_then(|(_, rest)| rest.trim_start().chars().next());
    state.is_none_or(|state| ENDED_STATES.contains(&state))
}

/// The slot this machine is running in or has a task in.
pub fn slot_of(context: &Context, machine: &Machine) -> Option<Slot> {
    let running = running_slots(context);
    let in_use = running.iter().find(|(_, pid)| uses_disk(context, **pid, machine));
    in_use.map(|(number, _)| slots::number(*number)).or_else(|| {
        slots::all().find(|slot| {
            let task = read_task(context, Some(*slot));
            task.machine == machine.id && task_alive(context, &task)
        })
    })
}

/// The lowest slot with no VM, no task, nothing left behind and no other VM on its memory.
pub fn free_slot(context: &Context) -> Option<Slot> {
    let running = running_slots(context);
    let is_free = |slot: &Slot| {
        !running.contains_key(&slot.number())
            && !task_alive(context, &read_task(context, Some(*slot)))
            && !memory_taken(context, Some(*slot))
            && leftovers(context, Some(*slot)).is_none()
    };
    // A window started with a slot named keeps to it, so it can stay clear of the others.
    if context.slot.number() != slots::first().number() {
        return Some(context.slot).filter(is_free);
    }
    slots::all().find(is_free)
}

/// What is going on with a machine, and if its GPU has stopped, that comes first.
pub fn status(context: &Context, machine: &Machine) -> Status {
    let mut found = what_is_running(context, machine);
    let folder = found.slot.map(|number| context.run_folder(Some(slots::number(number))));
    if found.state == State::Running
        && let Some(note) = folder.as_ref().and_then(|folder| read_note(folder))
    {
        found.error = note;
    }
    if machine.has_gpu() {
        let shown = read_state(context, machine);
        found.display_problem = shown.problem().to_owned();
        if let Some(folder) = folder.filter(|_| found.state == State::Running) {
            found.screen = screen_text(&shown, ring_in(&folder));
        }
    }
    found
}

/// Whether the task in `read` ended without finishing. A task's last act is to clear its pid, so this
/// looks again once it has ended: after that it can't write anything more.
fn task_died(context: &Context, number: u8, read: &TaskRecord) -> bool {
    read.pid != 0
        && task_ended(context, read)
        && read_task(context, Some(slots::number(number))).pid == read.pid
}

/// Looks at what is really running, so a VM started some other way counts like any other.
fn what_is_running(context: &Context, machine: &Machine) -> Status {
    let running = running_slots(context);
    let slot = running.iter().find(|(_, pid)| uses_disk(context, **pid, machine)).map(|(number, _)| *number);
    let state = if slot.is_some() { State::Running } else { State::Off };
    let tasks: BTreeMap<u8, TaskRecord> =
        slots::all().map(|number| (number.number(), read_task(context, Some(number)))).collect();
    let mine: Vec<u8> =
        tasks.iter().filter(|(_, task)| task.machine == machine.id).map(|(number, _)| *number).collect();
    let Some(first) = mine.first() else { return Status { state, slot, ..Status::default() } };
    // A finished task's words stay in its slot, and the slot it's running in counts first.
    let number = slot.filter(|running| mine.contains(running)).unwrap_or(*first);
    let task = &tasks[&number];
    if !task_alive(context, task) {
        let died = if task_died(context, number, task) { TASK_DIED } else { "" };
        let error = if task.error.is_empty() { died.to_owned() } else { task.error.clone() };
        return Status { state, error, slot, ..Status::default() };
    }
    let state = match task.action.as_str() {
        "start" => State::Starting,
        "install" => State::Installing,
        "stop" | "force" | "clean_up" => State::Stopping,
        _ => state,
    };
    let (step, error) = (task.step.clone(), task.error.clone());
    Status { state, step, error, busy: true, slot: Some(number), ..Status::default() }
}

/// Starts a task that will outlive the app, and returns at once. It runs in the machine's slot, or
/// the one named, or a free one. Everything it does follows from that.
pub fn launch(
    context: &Context,
    action: &str,
    machine: &Machine,
    arguments: &[String],
    slot: Option<Slot>,
) -> Result<Child> {
    let slot = slot
        .or_else(|| slot_of(context, machine))
        .or_else(|| if context.second_instance() { Some(context.slot) } else { free_slot(context) })
        .ok_or_else(|| Failure(no_slot_text(context)))?;
    let task = read_task(context, Some(slot));
    let mut arguments = arguments.to_vec();
    if task_alive(context, &task) {
        if action != "force" {
            return Err(Failure::new(STILL_BUSY));
        }
        arguments = vec![task.pid.to_string()];
    }
    context.make_run_folder(Some(slot))?;
    let log = open_for_appending(context)?;
    let process = detached(
        Command::new(context.paths.binary(TASK_PROGRAM)).arg(action).arg(&machine.id).args(arguments),
    )
    .env(SLOT_VARIABLE, slot.number().to_string())
    .stdin(Stdio::null())
    .stdout(log.try_clone()?)
    .stderr(log)
    .spawn()?;
    let record = TaskRecord {
        action: action.to_owned(),
        machine: machine.id.clone(),
        pid: process.id(),
        step: "Getting ready".to_owned(),
        error: String::new(),
    };
    write_task(context, Some(slot), &record)?;
    Ok(process)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processes::stand_in::fake_process;
    use crate::testing::{Scratch, scratch_context};
    use guesthouse_daemon::gpu_note::write_note;

    fn machine() -> Machine {
        Machine::new("sample", "Sample", "/d", "/v")
    }

    fn record(machine: &str, pid: u32, error: &str) -> TaskRecord {
        TaskRecord {
            action: "start".into(),
            machine: machine.into(),
            pid,
            step: "x".into(),
            error: error.into(),
        }
    }

    fn running_vm(context: &Context, vm: &Machine) {
        let qemu = context.paths.qemu.display().to_string();
        let monitor = format!("unix:{}/qmp.sock,server=on", context.run_folder(None).display());
        let disk = format!("file={},format=qcow2", vm.disk);
        fake_process(context, 40, &[&qemu, "-qmp", &monitor, &disk], &[], "");
    }

    #[test]
    fn a_vm_whose_gpu_has_stopped_says_so_on_its_row() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        context.make_run_folder(None).unwrap();
        let vm = machine();
        running_vm(&context, &vm);
        assert_eq!(status(&context, &vm).error, "");
        write_note(&context.run_folder(None), true).unwrap();
        let found = status(&context, &vm);
        assert_eq!(found.state, State::Running);
        assert!(found.error.contains("GPU has stopped"), "{found:?}");
    }

    #[test]
    fn a_note_left_by_a_vm_that_has_gone_is_not_shown() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        context.make_run_folder(None).unwrap();
        write_note(&context.run_folder(None), true).unwrap();
        assert_eq!(status(&context, &machine()).error, "");
    }

    #[test]
    fn a_window_with_a_slot_named_keeps_to_it() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        assert_eq!(free_slot(&context).map(Slot::number), Some(0));
        context.slot = slots::number(3);
        assert_eq!(free_slot(&context).map(Slot::number), Some(3));
    }

    #[test]
    fn a_finished_task_leaves_its_error_for_its_own_machine() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(status(&context, &machine()).error, "");
        write_task(&context, None, &record("sample", 0, "It broke.")).unwrap();
        let found = status(&context, &machine());
        assert!(found.error == "It broke." && !found.busy);
        let other = Machine { id: "other".into(), ..machine() };
        assert_eq!(status(&context, &other).error, "", "an error belongs to its own machine");
    }

    #[test]
    fn only_a_task_process_counts_as_a_task() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fake_process(&context, 77, &["/bin/something"], &[], "");
        write_task(&context, None, &record("sample", 77, "")).unwrap();
        assert!(!status(&context, &machine()).busy);
    }

    #[test]
    fn a_task_that_dies_without_a_word_does_not_look_finished() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        write_task(&context, None, &record("sample", (1 << 22) + 1, "")).unwrap();
        let found = status(&context, &machine());
        assert!(!found.busy && found.error.contains("before it finished"));
        write_task(&context, None, &record("sample", 0, "")).unwrap();
        assert_eq!(status(&context, &machine()).error, "", "one that finished said all it had to");
    }

    #[test]
    fn a_task_still_being_loaded_has_not_died() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        write_task(&context, None, &record("sample", 79, "")).unwrap();
        for words in [&["/usr/bin/guesthouse"][..], &[""][..]] {
            fake_process(&context, 79, words, &[], "");
            fs::write(context.processes.join("79/stat"), "79 (a) b) R 1 79").unwrap();
            assert_eq!(status(&context, &machine()).error, "", "{words:?}");
        }
    }

    #[test]
    fn a_task_killed_and_not_yet_collected_has_died() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        write_task(&context, None, &record("sample", 79, "")).unwrap();
        fake_process(&context, 79, &[""], &[], "");
        fs::write(context.processes.join("79/stat"), "79 (guesthouse-task) Z 1 79").unwrap();
        assert_eq!(status(&context, &machine()).error, TASK_DIED);
    }

    #[test]
    fn a_task_that_finishes_just_after_its_record_is_read_has_not_died() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let read_while_running = record("sample", (1 << 22) + 1, "");
        write_task(&context, None, &record("sample", 0, "")).unwrap();
        assert!(!task_died(&context, 0, &read_while_running));
    }

    #[test]
    fn a_live_task_says_what_it_is_doing() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fake_process(
            &context,
            78,
            &["/usr/libexec/guesthouse/guesthouse-task", "install", "sample"],
            &[],
            "",
        );
        let install = TaskRecord { action: "install".into(), ..record("sample", 78, "") };
        write_task(&context, Some(slots::number(1)), &install).unwrap();
        let found = status(&context, &machine());
        assert_eq!((found.state, found.busy, found.slot), (State::Installing, true, Some(1)));
        assert_eq!(found.step, "x");
        assert_eq!(slot_of(&context, &machine()), Some(slots::number(1)));
    }

    #[test]
    fn a_running_machine_is_found_in_its_slot() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let qemu = context.paths.qemu.display().to_string();
        let socket =
            format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slots::number(1)).display());
        fake_process(&context, 90, &[&qemu, "-qmp", &socket, "-drive", "file=/d,format=qcow2"], &[], "");
        assert_eq!(slot_of(&context, &machine()), Some(slots::number(1)));
        let found = status(&context, &machine());
        assert_eq!((found.state, found.slot), (State::Running, Some(1)));
        assert_eq!(free_slot(&context), Some(slots::first()), "the lowest one that is free");
    }

    #[test]
    fn a_slot_held_by_another_vm_is_not_free() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(free_slot(&context), Some(slots::first()));
        let memory = context.paths.shared_file("ram", slots::first());
        fs::write(&memory, "").unwrap();
        fake_process(
            &context,
            201,
            &["/other/guesthouse-daemon", "/other"],
            &[],
            &memory.display().to_string(),
        );
        assert_eq!(free_slot(&context), Some(slots::number(1)));
    }

    #[test]
    fn a_busy_slot_refuses_a_second_task() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fake_process(&context, 79, &["guesthouse-task", "start", "sample"], &[], "");
        write_task(&context, None, &record("sample", 79, "")).unwrap();
        let error = launch(&context, "start", &machine(), &[], Some(slots::first())).unwrap_err();
        assert_eq!(error.0, STILL_BUSY);
    }
}
