//! QEMU's monitor, as a question and an answer.

use crate::context::Context;
use guesthouse_daemon::slot::Slot;
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, IoSlice, Write};
use std::mem::MaybeUninit;
use std::os::fd::BorrowedFd;
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
    /// Hands QEMU an open file with the first command, and sends the rest on the same connection, as
    /// QEMU only keeps a file it was handed while that connection lasts or something has taken it.
    /// The answers, up to the first refusal or the first that didn't come.
    fn ask_handing(&self, socket: &Path, file: BorrowedFd<'_>, commands: &[Command]) -> Vec<Value>;
}

pub struct QmpMonitor;

/// A connection that has said hello, ready for commands.
fn connect(socket: &Path) -> Option<(BufReader<UnixStream>, UnixStream)> {
    let stream = UnixStream::connect(socket).ok()?;
    let time = Some(Duration::from_secs(MONITOR_SECONDS));
    stream.set_read_timeout(time).ok()?;
    stream.set_write_timeout(time).ok()?;
    let mut writer = stream.try_clone().ok()?;
    let mut reader = BufReader::new(stream);
    read_line(&mut reader)?;
    converse(&mut reader, &mut writer, &Command::bare("qmp_capabilities"))?;
    Some((reader, writer))
}

impl Monitor for QmpMonitor {
    fn ask(&self, socket: &Path, command: &Command) -> Option<Value> {
        let (mut reader, mut writer) = connect(socket)?;
        converse(&mut reader, &mut writer, command)
    }

    fn ask_handing(&self, socket: &Path, file: BorrowedFd<'_>, commands: &[Command]) -> Vec<Value> {
        let mut answers = Vec::new();
        let Some((mut reader, mut writer)) = connect(socket) else { return answers };
        for (at, command) in commands.iter().enumerate() {
            let sent =
                if at == 0 { send_with_file(&writer, command, file) } else { send(&mut writer, command) };
            let Some(answer) = sent.and_then(|()| read_answer(&mut reader)) else { break };
            let refused = answer.get("error").is_some();
            answers.push(answer);
            if refused {
                break;
            }
        }
        answers
    }
}

fn read_line(reader: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    (reader.read_line(&mut line).ok()? > 0).then_some(line)
}

fn request_line(command: &Command) -> String {
    format!("{}\n", json!({"execute": command.name, "arguments": command.arguments}))
}

fn send(writer: &mut impl Write, command: &Command) -> Option<()> {
    writer.write_all(request_line(command).as_bytes()).ok()
}

/// The file goes with the first bytes of the line, which is how QEMU takes it.
fn send_with_file(writer: &UnixStream, command: &Command, file: BorrowedFd<'_>) -> Option<()> {
    let line = request_line(command);
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = SendAncillaryBuffer::new(&mut space);
    let files = [file];
    ancillary.push(SendAncillaryMessage::ScmRights(&files)).then_some(())?;
    let sent = sendmsg(writer, &[IoSlice::new(line.as_bytes())], &mut ancillary, SendFlags::empty()).ok()?;
    let mut rest = writer;
    rest.write_all(&line.as_bytes()[sent..]).ok()
}

/// Events can come before the answer, so anything else is passed over.
fn converse(reader: &mut impl BufRead, writer: &mut impl Write, command: &Command) -> Option<Value> {
    send(writer, command)?;
    read_answer(reader)
}

fn read_answer(reader: &mut impl BufRead) -> Option<Value> {
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

    pub fn ask_monitor_handing(
        &self,
        slot: Option<Slot>,
        file: BorrowedFd<'_>,
        commands: &[Command],
    ) -> Vec<Value> {
        self.monitor.ask_handing(&self.run_folder(slot).join(MAIN_SOCKET), file, commands)
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

        fn ask_handing(&self, socket: &Path, _file: BorrowedFd<'_>, commands: &[Command]) -> Vec<Value> {
            let mut answers = Vec::new();
            for command in commands {
                let Some(answer) = self.ask(socket, command) else { break };
                let refused = answer.get("error").is_some();
                answers.push(answer);
                if refused {
                    break;
                }
            }
            answers
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

    /// Takes one message and any files that came with it, as QEMU's monitor does.
    fn receive(stream: &UnixStream) -> (String, Vec<std::path::PathBuf>) {
        use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
        use std::os::fd::AsRawFd;
        let mut bytes = [0u8; 4096];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut received = RecvAncillaryBuffer::new(&mut space);
        let parts = &mut [std::io::IoSliceMut::new(&mut bytes)];
        let got = recvmsg(stream, parts, &mut received, RecvFlags::empty()).unwrap();
        let mut files = Vec::new();
        for message in received.drain() {
            if let RecvAncillaryMessage::ScmRights(handed) = message {
                let named = |file: std::os::fd::OwnedFd| {
                    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap()
                };
                files.extend(handed.map(named));
            }
        }
        (String::from_utf8_lossy(&bytes[..got.bytes]).into_owned(), files)
    }

    #[test]
    fn a_file_goes_with_the_first_command_and_the_rest_follow_on_the_same_connection() {
        use std::os::fd::AsFd;
        let scratch = Scratch::new();
        let path = scratch.path("qmp.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let handed = scratch.write("handed", "the file QEMU gets");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"{\"QMP\": {}}\n").unwrap();
            let mut said = Vec::new();
            for _ in 0..4 {
                said.push(receive(&stream));
                stream.write_all(b"{\"return\": {}}\n").unwrap();
            }
            said
        });
        let file = std::fs::File::open(&handed).unwrap();
        let commands = [Command::bare("add-fd"), Command::bare("device_add"), Command::bare("remove-fd")];
        assert_eq!(QmpMonitor.ask_handing(&path, file.as_fd(), &commands).len(), 3);
        let said = server.join().unwrap();
        let names: Vec<&str> = ["qmp_capabilities", "add-fd", "device_add", "remove-fd"].to_vec();
        for ((text, _), name) in said.iter().zip(&names) {
            assert!(text.contains(name), "{text}");
        }
        let files: Vec<usize> = said.iter().map(|(_, files)| files.len()).collect();
        assert_eq!(files, [0, 1, 0, 0], "the file came with add-fd alone");
        assert_eq!(said[1].1, [handed]);
    }

    #[test]
    fn handing_stops_at_a_refusal() {
        let monitor = stand_in::RecordingMonitor::answering(Some(json!({"error": {"desc": "no"}})));
        let file = std::fs::File::open("/dev/null").unwrap();
        let commands = [Command::bare("add-fd"), Command::bare("device_add")];
        let answers = monitor.ask_handing(Path::new("/nowhere"), std::os::fd::AsFd::as_fd(&file), &commands);
        assert_eq!((answers.len(), monitor.asked().len()), (1, 1));
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
