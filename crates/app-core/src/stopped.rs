//! What to say when a VM stops and nobody here asked it to. QEMU can go in many ways and the app isn't
//! its parent, so all it sees is a VM that's gone. A guest that shut itself down looks the same.

use crate::context::Context;
use crate::log::write_log;
use crate::machine::Machine;
use crate::slots;
use crate::tasks::read_task;
use crate::vm_args::OUTPUT_FILE;
use std::fs;

/// What this app does to stop a VM on purpose.
const STOPPING_ACTIONS: [&str; 3] = ["stop", "force", "install"];
const LAST_WORDS_LINES: usize = 3;

fn last_words(context: &Context, slot: u8) -> String {
    let file = context.run_folder(Some(slots::number(slot))).join(OUTPUT_FILE);
    let text = fs::read(file).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().rev().take(LAST_WORDS_LINES).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join(" ")
}

/// What to tell the user about a VM that was running in `slot` and has gone, if it's worth telling.
pub fn stopped_by_itself(context: &Context, machine: &Machine, slot: u8) -> Option<String> {
    let task = read_task(context, Some(slots::number(slot)));
    if task.machine == machine.id && STOPPING_ACTIONS.contains(&task.action.as_str()) {
        return None;
    }
    let words = last_words(context, slot);
    let _ = write_log(context, &format!("{} stopped by itself. QEMU's last words: {words}", machine.id));
    Some(format!(
        "{} stopped, and this app didn't stop it. Unless the guest shut itself down, the log has more.",
        machine.name
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::task_path;
    use crate::testing::{Scratch, scratch_context};
    use serde_json::json;

    fn after_action(context: &Context, action: &str) {
        let file = task_path(context, Some(slots::number(1)));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let record = json!({"action": action, "machine": "vm", "pid": 0, "step": "", "error": ""});
        fs::write(file, record.to_string()).unwrap();
    }

    fn machine() -> Machine {
        Machine::new("vm", "My VM", "/d", "/v")
    }

    #[test]
    fn a_vm_that_went_with_no_stop_says_so() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        after_action(&context, "start");
        let folder = context.run_folder(Some(slots::number(1)));
        fs::write(folder.join(OUTPUT_FILE), "first\nsecond\nthird\nfourth\n").unwrap();
        let said = stopped_by_itself(&context, &machine(), 1).unwrap();
        assert!(said.contains("My VM stopped"), "{said}");
        assert!(crate::log::read_log(&context).contains("second third fourth"));
    }

    #[test]
    fn a_vm_the_app_stopped_says_nothing() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        for action in ["stop", "force"] {
            after_action(&context, action);
            assert_eq!(stopped_by_itself(&context, &machine(), 1), None);
        }
    }
}
