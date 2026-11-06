//! Tasks as the app sees them: the small file each one writes, what is going on with a machine, and
//! starting one. Actions themselves are in [`crate::actions`].

use crate::context::{Context, SLOT_VARIABLE};
use crate::daemon::detached;
use crate::failure::{Failure, Result};
use crate::log::open_for_appending;
use crate::machine::Machine;
use crate::processes::{leftovers, memory_taken, running_slots, uses_disk};
use crate::settings::write_json;
use crate::slots;
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
/// How a machine is told it would be one too many.
pub fn no_slot_text() -> String {
    format!("{SLOTS} VMs are running, which is as many as Guesthouse runs at once.")
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
}

fn task_path(context: &Context, slot: Option<Slot>) -> PathBuf {
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
    slots::all().find(|slot| {
        !running.contains_key(&slot.number())
            && !task_alive(context, &read_task(context, Some(*slot)))
            && !memory_taken(context, Some(*slot))
            && leftovers(context, Some(*slot)).is_none()
    })
}

/// Looks at what is really running, so a VM started some other way counts like any other.
pub fn status(context: &Context, machine: &Machine) -> Status {
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
        let died = if task.pid != 0 { "The task stopped before it finished. The log has more." } else { "" };
        let error = if task.error.is_empty() { died.to_owned() } else { task.error.clone() };
        return Status { state, error, slot, ..Status::default() };
    }
    let state = match task.action.as_str() {
        "start" => State::Starting,
        "install" => State::Installing,
        "stop" | "force" | "clean_up" => State::Stopping,
        _ => state,
    };
    Status { state, step: task.step.clone(), error: task.error.clone(), busy: true, slot: Some(number) }
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
        .ok_or_else(|| Failure(no_slot_text()))?;
    let task = read_task(context, Some(slot));
    let mut arguments = arguments.to_vec();
    if task_alive(context, &task) {
        if action != "force" {
            return Err(Failure::new(STILL_BUSY));
        }
        arguments = vec![task.pid.to_string()];
    }
    fs::create_dir_all(context.run_folder(Some(slot)))?;
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
