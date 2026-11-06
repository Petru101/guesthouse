//! The keyboard and pointer of an Ubuntu guest on the GPU. Its desktop runs on the forwarded card, where
//! QEMU's own input has no way in, so these go down a serial port to the guest's session, a line each.
//! Nothing here ever waits, for the same reason as QEMU's input.

use std::io::{self, ErrorKind};
use std::os::unix::net::UnixStream;
use std::path::Path;

/// Linux's numbers for the mouse buttons, as the guest's session wants them.
const BUTTON_LEFT: u32 = 272;
const BUTTON_RIGHT: u32 = 273;
const BUTTON_MIDDLE: u32 = 274;

pub struct ForwardedInput {
    stream: UnixStream,
    alive: bool,
}

impl ForwardedInput {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        Ok(Self { stream, alive: true })
    }

    pub fn alive(&self) -> bool {
        self.alive
    }

    /// A line that can't go now is dropped: the next motion says the same thing, and a late key press
    /// is worse than a lost one.
    fn send(&mut self, line: &str) {
        let text = format!("{line}\n");
        let flags = rustix::net::SendFlags::DONTWAIT | rustix::net::SendFlags::NOSIGNAL;
        match rustix::net::send(&self.stream, text.as_bytes(), flags).map_err(io::Error::from) {
            Err(error) if error.kind() != ErrorKind::WouldBlock => self.alive = false,
            _ => {}
        }
    }

    /// `x` and `y` are in the picture's own pixels.
    pub fn move_to(&mut self, x: f64, y: f64) {
        self.send(&format!("m {x:.1} {y:.1}"));
    }

    pub fn button(&mut self, name: &str, down: bool) {
        let code = match name {
            "right" => BUTTON_RIGHT,
            "middle" => BUTTON_MIDDLE,
            _ => BUTTON_LEFT,
        };
        self.send(&format!("b {code} {}", u8::from(down)));
    }

    pub fn key(&mut self, code: u32, down: bool) {
        self.send(&format!("k {code} {}", u8::from(down)));
    }

    pub fn scroll(&mut self, steps: i32) {
        self.send(&format!("a 0 {steps}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::io::Read;
    use std::net::Shutdown;
    use std::os::unix::net::UnixListener;

    #[test]
    fn each_event_is_a_line_the_session_reads() {
        let scratch = Scratch::new();
        let path = scratch.path("input.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let mut input = ForwardedInput::connect(&path).unwrap();
        let (mut session, _) = listener.accept().unwrap();
        input.move_to(12.5, 7.0);
        input.button("left", true);
        input.button("right", false);
        input.key(30, true);
        input.scroll(-1);
        drop(input);
        let mut text = String::new();
        session.read_to_string(&mut text).unwrap();
        assert_eq!(text, "m 12.5 7.0\nb 272 1\nb 273 0\nk 30 1\na 0 -1\n");
    }

    #[test]
    fn a_session_that_has_gone_is_noticed() {
        let scratch = Scratch::new();
        let path = scratch.path("input.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let mut input = ForwardedInput::connect(&path).unwrap();
        // Not a drop: a child another test is forking can hold a copy of the socket for a moment.
        listener.accept().unwrap().0.shutdown(Shutdown::Both).unwrap();
        input.key(1, true);
        input.key(1, false);
        assert!(!input.alive());
    }
}
