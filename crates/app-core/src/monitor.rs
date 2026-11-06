//! QEMU's monitor, as a question and an answer.

use crate::context::Context;
use guesthouse_daemon::slot::Slot;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

pub const MAIN_SOCKET: &str = "qmp.sock";
/// A second monitor, for watching an install while the first is left free.
pub const WATCH_SOCKET: &str = "watch.sock";
const MONITOR_SECONDS: u64 = 2;

/// A QMP command and its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub name: String,
    pub arguments: Value,
}

impl Command {
    pub fn new(name: &str, arguments: Value) -> Self {
        Self { name: name.to_owned(), arguments }
    }

    pub fn bare(name: &str) -> Self {
        Self::new(name, json!({}))
    }
}

pub trait Monitor: Send + Sync {
    /// Sends one command and returns QEMU's answer. `None` if the monitor is busy or there is no VM.
    fn ask(&self, socket: &Path, command: &Command) -> Option<Value>;
}

pub struct QmpMonitor;

impl Monitor for QmpMonitor {
    fn ask(&self, socket: &Path, command: &Command) -> Option<Value> {
        let stream = UnixStream::connect(socket).ok()?;
        let time = Some(Duration::from_secs(MONITOR_SECONDS));
        stream.set_read_timeout(time).ok()?;
        stream.set_write_timeout(time).ok()?;
        let mut writer = stream.try_clone().ok()?;
        let mut reader = BufReader::new(stream);
        read_line(&mut reader)?;
        converse(&mut reader, &mut writer, &Command::bare("qmp_capabilities"))?;
        converse(&mut reader, &mut writer, command)
    }
}

fn read_line(reader: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    (reader.read_line(&mut line).ok()? > 0).then_some(line)
}

/// Events can come before the answer, so anything else is passed over.
fn converse(reader: &mut impl BufRead, writer: &mut impl Write, command: &Command) -> Option<Value> {
    let request = json!({"execute": command.name, "arguments": command.arguments});
    writeln!(writer, "{request}").ok()?;
    loop {
        let answer: Value = serde_json::from_str(&read_line(reader)?).ok()?;
        if answer.get("return").is_some() || answer.get("error").is_some() {
            return Some(answer);
        }
    }
}

impl Context {
    pub fn ask_monitor(&self, slot: Option<Slot>, command: &Command) -> Option<Value> {
        self.monitor.ask(&self.run_folder(slot).join(MAIN_SOCKET), command)
    }

    pub fn ask_watch_monitor(&self, command: &Command) -> Option<Value> {
        self.monitor.ask(&self.run_folder(None).join(WATCH_SOCKET), command)
    }

    /// QEMU's own word for whether the guest runs, or `None` if the monitor is busy.
    pub fn run_state(&self, slot: Option<Slot>) -> Option<String> {
        let answer = self.ask_monitor(slot, &Command::bare("query-status"))?;
        Some(answer["return"]["status"].as_str()?.to_owned())
    }
}

/// Whether this answer is a refusal, or no answer at all.
pub fn failed(answer: &Option<Value>) -> bool {
    answer.as_ref().is_none_or(|found| found.get("error").is_some())
}

#[cfg(test)]
pub mod stand_in {
    //! A monitor that answers from a list and remembers what it was asked.

    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct RecordingMonitor {
        pub commands: Mutex<Vec<Command>>,
        pub answer: Mutex<Option<Value>>,
    }

    impl RecordingMonitor {
        pub fn answering(answer: Option<Value>) -> Self {
            Self { commands: Mutex::default(), answer: Mutex::new(answer) }
        }

        pub fn set_answer(&self, answer: Option<Value>) {
            *self.answer.lock().unwrap() = answer;
        }

        pub fn asked(&self) -> Vec<Command> {
            self.commands.lock().unwrap().clone()
        }
    }

    impl Monitor for RecordingMonitor {
        fn ask(&self, _socket: &Path, command: &Command) -> Option<Value> {
            self.commands.lock().unwrap().push(command.clone());
            self.answer.lock().unwrap().clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::os::unix::net::UnixListener;
    use std::thread;

    #[test]
    fn events_before_the_answer_are_passed_over() {
        let scratch = Scratch::new();
        let path = scratch.path("qmp.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
            stream.write_all(b"{\"QMP\": {}}\n").unwrap();
            let _ = lines.next();
            stream.write_all(b"{\"return\": {}}\n").unwrap();
            let request: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            stream.write_all(b"{\"event\": \"RESET\"}\n{\"return\": {\"status\": \"running\"}}\n").unwrap();
            request
        });
        let answer = QmpMonitor.ask(&path, &Command::bare("query-status")).unwrap();
        assert_eq!(answer["return"]["status"], "running");
        assert_eq!(server.join().unwrap()["execute"], "query-status");
    }

    #[test]
    fn no_vm_is_no_answer() {
        let scratch = Scratch::new();
        assert_eq!(QmpMonitor.ask(&scratch.path("gone.sock"), &Command::bare("query-status")), None);
    }

    #[test]
    fn a_refusal_counts_as_failed() {
        assert!(failed(&None));
        assert!(failed(&Some(json!({"error": {"desc": "locked"}}))));
        assert!(!failed(&Some(json!({"return": {}}))));
    }
}
