//! How a VM starts and stops. Everything that knows how the VM and the daemon are brought up is behind
//! [`VmHost`], as the daemon's start-up is changing. Machines with and without a GPU both go through it.

mod local;
mod watched;

pub use local::LocalHost;
pub use watched::{last_words, run_watched};

use crate::context::Context;
use crate::failure::Result;
use crate::machine::Machine;
use crate::monitor::{Command as Qmp, WATCH_SOCKET};
use crate::vm_args::VmPlan;

/// How a VM stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// The guest shut itself down.
    Cleanly,
    /// The guest took too long, so QEMU was told to end. For the guest that's a power cut.
    TurnedOff,
}

/// The small programs that reach into a running Windows guest. They ship with the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Helper {
    DesktopSize,
    SetDesktopMode,
    SetRefresh(u32),
    /// A debug stub stops the guest when something connects, and only a detach starts it again.
    DetachDebugger,
}

impl Helper {
    fn command(self) -> (&'static str, Vec<String>) {
        match self {
            Self::DesktopSize => ("scripts/gdisplay.sh", Vec::new()),
            Self::SetDesktopMode => ("scripts/gsetmode.sh", Vec::new()),
            Self::SetRefresh(hz) => ("scripts/sethz.sh", vec![hz.to_string()]),
            Self::DetachDebugger => ("scripts/rsp.py", vec!["detach".to_owned()]),
        }
    }
}

/// What a helper said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelperOutput {
    pub code: i32,
    pub text: String,
}

pub trait VmHost: Send + Sync {
    /// Starts the VM, with the daemon first if the plan has a GPU, and returns once it answers.
    fn start(&self, plan: &VmPlan, machine: &Machine) -> Result<()>;
    /// Asks the guest to shut down, and waits this long before turning it off.
    fn stop(&self, machine: &Machine, seconds: u64) -> Result<Stopped>;
    /// Ends the daemon and gives back the shared memory, once QEMU has gone.
    fn release(&self) -> Result<()>;
    fn helper(&self, helper: Helper) -> Result<HelperOutput>;
}

/// Presses a key on the watch monitor, which leaves the main one free.
pub fn send_key(context: &Context, key: &str) {
    let command = Qmp::new("send-key", serde_json::json!({"keys": [{"type": "qcode", "data": key}]}));
    context.monitor.ask(&context.run_folder(None).join(WATCH_SOCKET), &command);
}

#[cfg(test)]
pub mod stand_in {
    use super::*;
    use crate::failure::Failure;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct RecordingHost {
        pub calls: Mutex<Vec<String>>,
        pub plans: Mutex<Vec<VmPlan>>,
        pub helper_answers: Mutex<VecDeque<HelperOutput>>,
        pub stop_answer: Mutex<Option<Result<Stopped>>>,
        pub start_failure: Mutex<Option<Failure>>,
    }

    impl RecordingHost {
        pub fn answering(answers: &[(i32, &str)]) -> Self {
            let list =
                answers.iter().map(|(code, text)| HelperOutput { code: *code, text: (*text).to_owned() });
            Self { helper_answers: Mutex::new(list.collect()), ..Self::default() }
        }

        fn note(&self, text: String) {
            self.calls.lock().unwrap().push(text);
        }

        pub fn said(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl VmHost for RecordingHost {
        fn start(&self, plan: &VmPlan, _machine: &Machine) -> Result<()> {
            self.note(format!("start {}", if plan.is_gpu() { "gpu" } else { "plain" }));
            self.plans.lock().unwrap().push(plan.clone());
            self.start_failure.lock().unwrap().clone().map_or(Ok(()), Err)
        }

        fn stop(&self, _machine: &Machine, seconds: u64) -> Result<Stopped> {
            self.note(format!("stop {seconds}"));
            self.stop_answer.lock().unwrap().clone().unwrap_or(Ok(Stopped::Cleanly))
        }

        fn release(&self) -> Result<()> {
            self.note("release".to_owned());
            Ok(())
        }

        fn helper(&self, helper: Helper) -> Result<HelperOutput> {
            self.note(format!("{helper:?}"));
            Ok(self.helper_answers.lock().unwrap().pop_front().unwrap_or_default())
        }
    }
}

#[cfg(test)]
mod tests;
