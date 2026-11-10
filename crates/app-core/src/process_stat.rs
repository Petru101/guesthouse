//! Whether a process is still going, from `/proc/PID/stat`. A pid alone can't say: the system gives
//! it to something else once the process has gone, so a process is known by its start time too.

use crate::context::Context;
use guesthouse_jail::launch::start_time;
use std::fs;

/// How `stat` says a process has ended: a zombie, and one on its way out.
const ENDED_STATES: [char; 2] = ['Z', 'X'];

pub fn read(context: &Context, pid: u32) -> Option<String> {
    fs::read_to_string(context.processes.join(pid.to_string()).join("stat")).ok()
}

/// Whether a process has gone, or is on its way out, by its `stat`, if there was one to read.
pub fn has_ended(stat: Option<&str>) -> bool {
    // The state comes after the program's name, which can hold anything, brackets too.
    let state =
        stat.and_then(|stat| stat.rsplit_once(')')).and_then(|(_, rest)| rest.trim_start().chars().next());
    state.is_none_or(|state| ENDED_STATES.contains(&state))
}

/// A process as it was when it was seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Seen {
    started: u64,
    pub pid: u32,
}

impl Seen {
    /// The process with this pid, if it's going.
    pub fn now(context: &Context, pid: u32) -> Option<Self> {
        let stat = read(context, pid)?;
        if has_ended(Some(&stat)) {
            return None;
        }
        Some(Self { started: start_time(&stat)?, pid })
    }

    /// Whether this same process is still going.
    pub fn is_alive(&self, context: &Context) -> bool {
        Self::now(context, self.pid) == Some(*self)
    }
}

#[cfg(test)]
pub mod stand_in {
    use crate::context::Context;
    use std::fs;

    /// What `/proc/PID/stat` says of a process in this state that started at this tick.
    pub fn write_stat(context: &Context, pid: u32, state: char, started: u64) {
        let folder = context.processes.join(pid.to_string());
        fs::create_dir_all(&folder).unwrap();
        let stat =
            format!("{pid} (a (b)) {state} 1 {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {started} 1 2");
        fs::write(folder.join("stat"), stat).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::write_stat;
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn a_process_is_alive_until_it_ends_or_its_pid_goes_to_another() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(Seen::now(&context, 60), None);
        write_stat(&context, 60, 'S', 1000);
        let seen = Seen::now(&context, 60).unwrap();
        assert!(seen.is_alive(&context));
        write_stat(&context, 60, 'Z', 1000);
        assert!(!seen.is_alive(&context), "a zombie has ended");
        write_stat(&context, 60, 'R', 2000);
        assert!(!seen.is_alive(&context), "a later process with the same pid is another");
        fs::remove_dir_all(context.processes.join("60")).unwrap();
        assert!(!seen.is_alive(&context));
    }

    #[test]
    fn the_older_of_two_processes_comes_first() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        write_stat(&context, 900, 'S', 10);
        write_stat(&context, 100, 'S', 20);
        assert!(Seen::now(&context, 900) < Seen::now(&context, 100));
    }
}
