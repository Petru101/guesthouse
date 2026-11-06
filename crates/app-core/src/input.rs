//! QEMU's input, through a monitor socket of its own.
//!
//! QEMU serves one client on this socket and leaves the next waiting, so a second viewer has no input
//! until the first closes. Nothing is sent until QEMU answers, or a viewer that waited would end up
//! replaying everything that was typed at it. Nothing here ever waits, as a hung QEMU must not freeze
//! the window: the window holds the way out.

use rustix::net::{self, AddressFamily, SendFlags, SocketAddrUnix, SocketFlags, SocketType};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, ErrorKind};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

pub const INPUT_SOCKET: &str = "input.sock";
/// QEMU's names for the devices.
pub const TABLET: &str = "QEMU HID Tablet";
pub const MOUSE: &str = "QEMU HID Mouse";
/// The range of the tablet's axes.
pub const TABLET_RANGE: i64 = 32767;
const MOST_UNSENT: usize = 1 << 20;
const GREETING: &[u8] = b"{\"execute\":\"qmp_capabilities\"}\n{\"execute\":\"query-mice\",\"id\":\"mice\"}\n";
const MICE_REPLY: &str = "mice";
const RESET_EVENT: &str = "RESET";
/// Linux key codes are the PC scan code for the main block. These are the ones that differ.
const EXTENDED_KEYS: [(u32, u32); 18] = [
    (96, 0x9C),
    (97, 0x9D),
    (98, 0xB5),
    (99, 0xB7),
    (100, 0xB8),
    (102, 0xC7),
    (103, 0xC8),
    (104, 0xC9),
    (105, 0xCB),
    (106, 0xCD),
    (107, 0xCF),
    (108, 0xD0),
    (109, 0xD1),
    (110, 0xD2),
    (111, 0xD3),
    (125, 0xDB),
    (126, 0xDC),
    (127, 0xDD),
];

#[derive(Default)]
struct Shared {
    answered: AtomicBool,
    gone: AtomicBool,
    resets: AtomicU32,
    mice: Mutex<Option<HashMap<String, u64>>>,
}

pub struct Guest {
    stream: UnixStream,
    unsent: Vec<u8>,
    shared: Arc<Shared>,
}

impl Guest {
    pub fn connect(path: &Path) -> io::Result<Self> {
        // A blocking connect would wait for a place in QEMU's queue.
        let socket = net::socket_with(AddressFamily::UNIX, SocketType::STREAM, SocketFlags::NONBLOCK, None)?;
        net::connect(&socket, &SocketAddrUnix::new(path)?)?;
        let stream = UnixStream::from(socket);
        stream.set_nonblocking(false)?;
        let shared = Arc::new(Shared::default());
        let reader = stream.try_clone()?;
        let on_thread = shared.clone();
        thread::spawn(move || drain(reader, &on_thread));
        let mut guest = Self { stream, unsent: GREETING.to_vec(), shared };
        guest.flush();
        Ok(guest)
    }

    pub fn alive(&self) -> bool {
        !self.shared.gone.load(Ordering::Relaxed)
    }

    /// How many times the guest has reset since this was last asked.
    pub fn take_resets(&self) -> u32 {
        self.shared.resets.swap(0, Ordering::Relaxed)
    }

    /// Whether QEMU has said which mice it has.
    pub fn knows_mice(&self) -> bool {
        self.shared.mice.lock().is_ok_and(|mice| mice.is_some())
    }

    /// Whether the VM has the relative mouse, which capturing needs.
    pub fn has_mouse(&self) -> bool {
        self.mouse_index(MOUSE).is_some()
    }

    fn mouse_index(&self, name: &str) -> Option<u64> {
        let mice = self.shared.mice.lock().ok()?;
        let mice = mice.as_ref()?;
        mice.contains_key(MOUSE).then(|| mice.get(name).copied())?
    }

    fn send(&mut self, events: Value) {
        self.execute("input-send-event", json!({"events": events}));
    }

    fn execute(&mut self, command: &str, arguments: Value) {
        if self.shared.answered.load(Ordering::Relaxed) && self.unsent.len() < MOST_UNSENT {
            let message = json!({"execute": command, "arguments": arguments});
            self.unsent.extend(message.to_string().bytes());
            self.unsent.push(b'\n');
        }
        self.flush();
    }

    /// Sends what is waiting, as far as it can go without waiting.
    pub fn flush(&mut self) {
        match send_without_waiting(&self.stream, &self.unsent) {
            Ok(sent) => {
                self.unsent.drain(..sent);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(_) => self.unsent.clear(),
        }
    }

    pub fn move_to(&mut self, x: i64, y: i64) {
        self.send(json!([
            {"type": "abs", "data": {"axis": "x", "value": x}},
            {"type": "abs", "data": {"axis": "y", "value": y}},
        ]));
    }

    pub fn nudge(&mut self, x: i64, y: i64) {
        self.send(json!([
            {"type": "rel", "data": {"axis": "x", "value": x}},
            {"type": "rel", "data": {"axis": "y", "value": y}},
        ]));
    }

    /// Picks the device that gets buttons and the wheel: the first in QEMU's list that takes them.
    /// Motion goes by kind, absolute to the tablet and relative to the mouse. A button on the tablet
    /// would also put the guest's cursor back where the tablet last left it. A VM with no mouse is
    /// left as it is.
    pub fn point_with(&mut self, name: &str) {
        if let Some(index) = self.mouse_index(name) {
            self.execute("human-monitor-command", json!({"command-line": format!("mouse_set {index}")}));
        }
    }

    pub fn button(&mut self, name: &str, down: bool) {
        self.send(json!([{"type": "btn", "data": {"button": name, "down": down}}]));
    }

    pub fn key(&mut self, code: u32, down: bool) {
        let number = EXTENDED_KEYS.iter().find(|(key, _)| *key == code).map_or(code, |found| found.1);
        self.send(
            json!([{"type": "key", "data": {"key": {"type": "number", "data": number}, "down": down}}]),
        );
    }
}

impl Drop for Guest {
    /// A thread is reading, so closing alone would leave the connection open and QEMU would never
    /// serve the next viewer.
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// One non-blocking send, as a flag on the call and not the socket, which the reader shares.
fn send_without_waiting(stream: &UnixStream, data: &[u8]) -> io::Result<usize> {
    if data.is_empty() {
        return Ok(0);
    }
    net::send(stream, data, SendFlags::DONTWAIT | SendFlags::NOSIGNAL).map_err(io::Error::from)
}

/// Takes in QEMU's replies. Only the list of mice matters, and it comes first, and the guest's resets.
fn drain(stream: UnixStream, shared: &Shared) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        if !matches!(reader.read_line(&mut line), Ok(size) if size > 0) {
            break;
        }
        shared.answered.store(true, Ordering::Relaxed);
        let Ok(reply) = serde_json::from_str::<Value>(&line) else { continue };
        if reply["event"] == RESET_EVENT {
            shared.resets.fetch_add(1, Ordering::Relaxed);
        }
        if reply["id"] == MICE_REPLY {
            let list = reply["return"].as_array().cloned().unwrap_or_default();
            let named = list
                .iter()
                .filter_map(|mouse| Some((mouse["name"].as_str()?.to_owned(), mouse["index"].as_u64()?)));
            if let Ok(mut mice) = shared.mice.lock() {
                *mice = Some(named.collect());
            }
        }
    }
    shared.gone.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Answers as QEMU's input socket does, and passes on what it's sent.
    fn fake_qemu(path: &Path, mice: Value) -> mpsc::Receiver<Value> {
        let listener = UnixListener::bind(path).unwrap();
        let (sender, commands) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"{\"QMP\": {}}\n").unwrap();
            for line in BufReader::new(stream.try_clone().unwrap()).lines() {
                let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
                let reply = if request["id"] == "mice" {
                    json!({"return": mice, "id": "mice"})
                } else {
                    json!({"return": {}})
                };
                writeln!(stream, "{reply}").unwrap();
                if sender.send(request).is_err() {
                    return;
                }
            }
        });
        commands
    }

    fn wait_for(condition: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < end, "it never happened");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn next(commands: &mpsc::Receiver<Value>) -> Value {
        commands.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn capture_moves_the_buttons_to_the_mouse_and_back() {
        let scratch = Scratch::new();
        let path = scratch.path("both.sock");
        let both = json!([{"name": "QEMU PS/2 Mouse", "index": 2}, {"name": TABLET, "index": 3}, {"name": MOUSE, "index": 4}]);
        let commands = fake_qemu(&path, both);
        let mut guest = Guest::connect(&path).unwrap();
        wait_for(|| guest.has_mouse());
        guest.point_with(MOUSE);
        guest.nudge(3, -2);
        guest.point_with(TABLET);
        assert_eq!(next(&commands)["execute"], "qmp_capabilities");
        assert_eq!(next(&commands)["execute"], "query-mice");
        assert_eq!(next(&commands)["arguments"]["command-line"], "mouse_set 4");
        let events = next(&commands);
        assert_eq!(events["arguments"]["events"][0]["data"], json!({"axis": "x", "value": 3}));
        assert_eq!(events["arguments"]["events"][1]["data"], json!({"axis": "y", "value": -2}));
        assert_eq!(next(&commands)["arguments"]["command-line"], "mouse_set 3");
    }

    #[test]
    fn a_vm_with_no_mouse_is_left_as_it_is() {
        let scratch = Scratch::new();
        let path = scratch.path("tablet.sock");
        let commands =
            fake_qemu(&path, json!([{"name": "QEMU PS/2 Mouse", "index": 2}, {"name": TABLET, "index": 3}]));
        let mut guest = Guest::connect(&path).unwrap();
        wait_for(|| guest.knows_mice());
        assert!(!guest.has_mouse());
        guest.point_with(TABLET);
        guest.move_to(1, 2);
        next(&commands);
        next(&commands);
        assert_eq!(next(&commands)["execute"], "input-send-event");
    }

    #[test]
    fn extended_keys_get_their_scan_codes() {
        let scratch = Scratch::new();
        let path = scratch.path("keys.sock");
        let commands = fake_qemu(&path, json!([]));
        let mut guest = Guest::connect(&path).unwrap();
        wait_for(|| guest.knows_mice());
        guest.key(103, true);
        guest.key(30, false);
        next(&commands);
        next(&commands);
        assert_eq!(next(&commands)["arguments"]["events"][0]["data"]["key"]["data"], 0xC8);
        assert_eq!(next(&commands)["arguments"]["events"][0]["data"]["key"]["data"], 30);
    }

    #[test]
    fn nothing_is_sent_until_qemu_answers() {
        let scratch = Scratch::new();
        let path = scratch.path("waiting.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let mut guest = Guest::connect(&path).unwrap();
        guest.key(30, true);
        assert!(guest.unsent.is_empty(), "the greeting went, and a key has nowhere to go yet");
    }
}
