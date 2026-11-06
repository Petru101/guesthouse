//! The guest's agent: PowerShell over a socket, with each machine's own token.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use crate::random::random_hex;
use crate::text::{base64_decode, base64_encode};
use guesthouse_daemon::slot::Slot;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

const CONNECT_SECONDS: u64 = 5;
/// Far more than any script of ours prints.
const MOST_ANSWER: usize = 4 << 20;
const ANSWER_CHUNK: usize = 64 << 10;
const TOKEN_BYTES: usize = 16;
/// The most text the clipboard passes either way.
pub const MOST_CLIPBOARD_TEXT: usize = 1 << 20;
pub const QUICK_SECONDS: u64 = 10;
const CLIPBOARD_START_SECONDS: u64 = 20;

pub trait GuestAgent: Send + Sync {
    /// Runs PowerShell in the guest, and returns what it said. Nothing if the agent isn't up yet.
    fn ask(&self, port: u16, token: &str, script: &str, seconds: u64) -> String;
}

pub struct TcpAgent;

impl GuestAgent for TcpAgent {
    fn ask(&self, port: u16, token: &str, script: &str, seconds: u64) -> String {
        talk(port, token, script, seconds).unwrap_or_default()
    }
}

fn talk(port: u16, token: &str, script: &str, seconds: u64) -> std::io::Result<String> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(CONNECT_SECONDS))?;
    stream.set_write_timeout(Some(Duration::from_secs(seconds.max(1))))?;
    writeln!(stream, "{token} {}", base64_encode(script.as_bytes()))?;
    let answer = read_answer(&mut stream, Duration::from_secs(seconds))?;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// As much of the answer as comes in `patience`, up to the most an answer can be. Each read has what
/// is left of the time, so an answer that keeps trickling ends as one that stops does.
fn read_answer(stream: &mut TcpStream, patience: Duration) -> std::io::Result<Vec<u8>> {
    let end = Instant::now() + patience;
    let mut answer = Vec::new();
    let mut chunk = [0; ANSWER_CHUNK];
    while answer.len() < MOST_ANSWER {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        stream.set_read_timeout(Some(left))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(size) => answer.extend_from_slice(&chunk[..size]),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break,
            Err(error) => return Err(error),
        }
    }
    answer.truncate(MOST_ANSWER);
    Ok(answer)
}

/// The machine's token, made the first time. Only an install calls this: the guest learns the token
/// from the install disc, so a new one for a machine already installed would lock the app out of it.
pub fn make_token(context: &Context, machine: &Machine) -> Result<String> {
    let path = machine.token_file(context);
    if !path.exists() {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        writeln!(file, "{}", random_hex::<TOKEN_BYTES>())?;
    }
    Ok(fs::read_to_string(path)?.trim().to_owned())
}

/// Takes a token from a file the user chose, for a guest installed before each machine had its own.
/// It's done once: a machine that has a token keeps it.
pub fn import_token(context: &Context, machine: &Machine, file: &Path) -> Result<()> {
    let text = fs::read_to_string(file)?;
    let token = text.trim();
    if token.len() != TOKEN_BYTES * 2 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Failure::new("This file does not hold a guest token."));
    }
    let path = machine.token_file(context);
    if path.exists() {
        return Err(Failure::new("This machine already has its token."));
    }
    let mut new = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    writeln!(new, "{token}")?;
    Ok(())
}

fn read_token(context: &Context, machine: &Machine) -> Option<String> {
    let text = fs::read_to_string(machine.token_file(context)).ok()?;
    Some(text.trim().to_owned())
}

/// A script from the program folder.
pub fn guest_script(context: &Context, name: &str) -> Result<String> {
    Ok(fs::read_to_string(context.paths.guest_files().join(name))?)
}

impl Context {
    /// Asks this machine's agent. Nothing comes back if the agent isn't up, or the machine has no
    /// token, which from the app's side is the same thing.
    pub fn ask_guest(&self, machine: &Machine, script: &str, seconds: u64, slot: Option<Slot>) -> String {
        let Some(token) = read_token(self, machine) else { return String::new() };
        self.agent.ask(self.agent_port(slot), &token, script, seconds)
    }
}

/// Says whether the helper is running in the user's session.
pub fn start_guest_clipboard(context: &Context, machine: &Machine, slot: Option<Slot>) -> Result<bool> {
    let helper = base64_encode(&fs::read(context.paths.guest_files().join("clipboard.ps1"))?);
    let script = format!("$helper = '{helper}'\n{}", guest_script(context, "clipboard-start.ps1")?);
    Ok(context.ask_guest(machine, &script, CLIPBOARD_START_SECONDS, slot).contains("started"))
}

pub fn stop_guest_clipboard(context: &Context, machine: &Machine, slot: Option<Slot>) {
    context.ask_guest(machine, "Stop-ScheduledTask -TaskName gpupv-clipboard", QUICK_SECONDS, slot);
}

/// What the helper answered, or `None` if it didn't. The agent can wrap long lines.
fn ask_guest_clipboard(
    context: &Context,
    machine: &Machine,
    request: &str,
    slot: Option<Slot>,
) -> Result<Option<String>> {
    let script = format!("$request = '{request}'\n{}", guest_script(context, "clipboard-ask.ps1")?);
    let answer = context.ask_guest(machine, &script, QUICK_SECONDS, slot);
    if !answer.starts_with("answer") {
        return Ok(None);
    }
    Ok(Some(answer.split_whitespace().skip(1).collect()))
}

/// The guest's text with Linux line ends, or `None` if the helper didn't answer.
pub fn read_guest_clipboard(
    context: &Context,
    machine: &Machine,
    slot: Option<Slot>,
) -> Result<Option<String>> {
    let Some(answer) = ask_guest_clipboard(context, machine, "get", slot)? else { return Ok(None) };
    let data = base64_decode(&answer).unwrap_or_default();
    Ok(Some(String::from_utf8_lossy(&data).replace("\r\n", "\n")))
}

/// Says whether it reached the guest. Windows expects its own line ends.
pub fn write_guest_clipboard(
    context: &Context,
    machine: &Machine,
    text: &str,
    slot: Option<Slot>,
) -> Result<bool> {
    let data = base64_encode(text.replace('\n', "\r\n").as_bytes());
    Ok(ask_guest_clipboard(context, machine, &data, slot)?.as_deref() == Some("set"))
}

#[cfg(test)]
pub mod stand_in {
    //! An agent that answers from a list and remembers what it was asked.

    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct ScriptedAgent {
        pub answers: Mutex<VecDeque<String>>,
        pub requests: Mutex<Vec<(u16, String, String)>>,
    }

    impl ScriptedAgent {
        pub fn answering(answers: &[&str]) -> Self {
            let answers = answers.iter().map(|text| (*text).to_owned()).collect();
            Self { answers: Mutex::new(answers), requests: Mutex::default() }
        }
    }

    impl GuestAgent for ScriptedAgent {
        fn ask(&self, port: u16, token: &str, script: &str, _seconds: u64) -> String {
            self.requests.lock().unwrap().push((port, token.to_owned(), script.to_owned()));
            self.answers.lock().unwrap().pop_front().unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::ScriptedAgent;
    use super::*;
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::os::unix::fs::MetadataExt;
    use std::sync::Arc;
    use std::thread;

    fn machine(context: &Context, id: &str) -> Machine {
        let machine = Machine::new(id, id, "/d", "/v");
        machine.save(context).unwrap();
        machine
    }

    fn with_guest_files(scratch: &Scratch) {
        scratch.write("program/guest/clipboard.ps1", "the helper");
        scratch.write("program/guest/clipboard-start.ps1", "start it");
        scratch.write("program/guest/clipboard-ask.ps1", "ask it");
    }

    fn answering_with(reply: impl FnOnce(TcpStream) + Send + 'static) -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let stream = listener.accept().unwrap().0;
            // Read first: closing on a request nobody read could lose the answer.
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            reply(stream);
        });
        port
    }

    #[test]
    fn an_answer_that_keeps_trickling_ends_on_time() {
        let port = answering_with(|mut stream| {
            while stream.write_all(b"x").is_ok() {
                thread::sleep(Duration::from_millis(100));
            }
        });
        let start = Instant::now();
        let answer = TcpAgent.ask(port, "token", "up", 1);
        assert!(start.elapsed() < Duration::from_secs(3), "it took {:?}", start.elapsed());
        assert!(!answer.is_empty());
    }

    #[test]
    fn an_answer_has_a_size_limit() {
        let port = answering_with(|mut stream| {
            let block = vec![b'x'; 1 << 20];
            while stream.write_all(&block).is_ok() {}
        });
        assert_eq!(TcpAgent.ask(port, "token", "up", 20).len(), MOST_ANSWER);
    }

    #[test]
    fn a_whole_answer_comes_back() {
        let port = answering_with(|mut stream| stream.write_all(b"finished\r\n").unwrap());
        assert_eq!(TcpAgent.ask(port, "token", "up", 5), "finished\r\n");
    }

    #[test]
    fn each_machine_has_its_own_private_token() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let (first, second) = (machine(&context, "first"), machine(&context, "second"));
        let token = make_token(&context, &first).unwrap();
        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert_eq!(make_token(&context, &first).unwrap(), token, "made once");
        assert_ne!(make_token(&context, &second).unwrap(), token);
        assert_eq!(fs::metadata(first.token_file(&context)).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn an_old_token_is_taken_once_from_the_file_the_user_chose() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let old = machine(&context, "old");
        let junk = scratch.write("junk", "not a token");
        assert!(import_token(&context, &old, &junk).unwrap_err().0.contains("does not hold"));
        assert!(!old.token_file(&context).exists());
        let file = scratch.write("token", format!("{}\n", "ab".repeat(TOKEN_BYTES)));
        import_token(&context, &old, &file).unwrap();
        assert_eq!(fs::read_to_string(old.token_file(&context)).unwrap().trim(), "ab".repeat(TOKEN_BYTES));
        assert_eq!(fs::metadata(old.token_file(&context)).unwrap().mode() & 0o777, 0o600);
        assert!(import_token(&context, &old, &file).unwrap_err().0.contains("already has"));
    }

    #[test]
    fn a_machine_with_no_token_gets_no_new_one_by_being_asked() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let agent = Arc::new(ScriptedAgent::answering(&["up"]));
        context.agent = agent.clone();
        let old = machine(&context, "old");
        assert_eq!(context.ask_guest(&old, "x", 1, None), "");
        assert!(agent.requests.lock().unwrap().is_empty());
        assert!(!old.token_file(&context).exists());
    }

    #[test]
    fn the_right_token_goes_to_the_right_slots_port() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let agent = Arc::new(ScriptedAgent::answering(&["up"]));
        context.agent = agent.clone();
        let vm = machine(&context, "vm");
        let token = make_token(&context, &vm).unwrap();
        assert_eq!(context.ask_guest(&vm, "x", 1, Some(slots::number(2))), "up");
        let requests = agent.requests.lock().unwrap();
        assert_eq!(requests[0], (18767, token, "x".to_owned()));
    }

    #[test]
    fn the_request_is_one_line_with_the_script_in_base64() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            stream.write_all(b"answered").unwrap();
            line
        });
        assert_eq!(TcpAgent.ask(port, "token", "Get-Date", 5), "answered");
        let line = server.join().unwrap();
        let (token, script) = line.trim().split_once(' ').unwrap();
        assert_eq!((token, base64_decode(script).unwrap()), ("token", b"Get-Date".to_vec()));
    }

    #[test]
    fn no_listener_is_an_empty_answer() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(TcpAgent.ask(port, "token", "up", 1), "");
    }

    #[test]
    fn the_clipboard_goes_through_the_helper() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        with_guest_files(&scratch);
        let text = "ünï\nline 'two'";
        let encoded = base64_encode(text.replace('\n', "\r\n").as_bytes());
        let wrapped = format!("answer {}\r\n{}\r\n", &encoded[..10], &encoded[10..]);
        let agent = Arc::new(ScriptedAgent::answering(&[
            &wrapped,
            "answer \r\n",
            "ERROR: refused\r\n",
            "",
            "answer set\r\n",
            "answer \r\n",
            "started\r\n",
            "nobody is logged in\r\n",
        ]));
        context.agent = agent.clone();
        let vm = machine(&context, "vm");
        make_token(&context, &vm).unwrap();
        let read = || read_guest_clipboard(&context, &vm, None).unwrap();
        assert_eq!(read().as_deref(), Some(text), "long lines come back wrapped, and Windows line ends go");
        assert_eq!(read().as_deref(), Some(""), "an empty clipboard is not the helper missing");
        assert_eq!(read(), None);
        assert_eq!(read(), None);
        assert!(write_guest_clipboard(&context, &vm, text, None).unwrap());
        assert!(!write_guest_clipboard(&context, &vm, text, None).unwrap());
        assert!(start_guest_clipboard(&context, &vm, None).unwrap());
        assert!(!start_guest_clipboard(&context, &vm, None).unwrap());
        let requests = agent.requests.lock().unwrap();
        assert!(requests[0].2.starts_with("$request = 'get'\n"));
        assert!(requests[4].2.contains(&format!("$request = '{encoded}'\n")));
        let helper = requests[6].2.strip_prefix("$helper = '").unwrap().split('\'').next().unwrap();
        assert_eq!(base64_decode(helper).unwrap(), b"the helper");
    }
}
