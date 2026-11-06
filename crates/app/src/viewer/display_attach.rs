//! Joining QEMU's D-Bus display, which has no bus: QEMU is given one socket to take calls on, and a
//! second to make its own. It stands still from the answer to `RegisterListener` until the second one
//! is up, so nothing may come between them.

use gtk::gio::prelude::*;
use gtk::{gio, glib};
use rustix::net::{
    self, AddressFamily, SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketFlags, SocketType,
};
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, ErrorKind, IoSlice};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

const ANSWER_TIME: Duration = Duration::from_secs(5);
const ANSWER_MS: i32 = 5000;
const DISPLAY_INTERFACE: &str = "org.qemu.Display1";
const CONSOLE: &str = "/org/qemu/Display1/Console_0";

fn pair() -> io::Result<(OwnedFd, OwnedFd)> {
    Ok(net::socketpair(AddressFamily::UNIX, SocketType::STREAM, SocketFlags::CLOEXEC, None)?)
}

fn to_io(error: glib::Error) -> io::Error {
    io::Error::other(error.to_string())
}

fn write_with_fd(stream: &UnixStream, command: &Value, fd: Option<&OwnedFd>) -> io::Result<()> {
    let text = format!("{command}\n");
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    let rights = fd.map(|fd| [fd.as_fd()]);
    if let Some(rights) = &rights {
        control.push(SendAncillaryMessage::ScmRights(rights));
    }
    net::sendmsg(stream, &[IoSlice::new(text.as_bytes())], &mut control, SendFlags::NOSIGNAL)?;
    Ok(())
}

fn ask(
    stream: &UnixStream,
    reader: &mut impl BufRead,
    command: Value,
    fd: Option<&OwnedFd>,
) -> io::Result<()> {
    write_with_fd(stream, &command, fd)?;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        let answer: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
        if answer.get("return").is_some() {
            return Ok(());
        }
        if let Some(error) = answer.get("error") {
            return Err(io::Error::other(error["desc"].to_string()));
        }
    }
}

/// `gio` owns the file from here on.
fn connect_peer(file: OwnedFd, flags: gio::DBusConnectionFlags) -> io::Result<gio::DBusConnection> {
    let socket = gio::Socket::from_fd(file).map_err(to_io)?;
    let stream = socket.connection_factory_create_connection();
    let flags = gio::DBusConnectionFlags::AUTHENTICATION_CLIENT | flags;
    gio::DBusConnection::new_sync(&stream, None, flags, None, gio::Cancellable::NONE).map_err(to_io)
}

/// Returns the connection QEMU will call the listener on, with messages held back until
/// `start_message_processing`, so the caller can register its objects first.
pub fn attach(monitor: &Path) -> io::Result<gio::DBusConnection> {
    let stream = UnixStream::connect(monitor)?;
    stream.set_read_timeout(Some(ANSWER_TIME))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut greeting = String::new();
    reader.read_line(&mut greeting)?;
    let (ours, theirs) = pair()?;
    let (listener, listener_theirs) = pair()?;
    ask(&stream, &mut reader, json!({"execute": "qmp_capabilities"}), None)?;
    let name = json!({"fdname": "display"});
    ask(&stream, &mut reader, json!({"execute": "getfd", "arguments": name}), Some(&theirs))?;
    let client = json!({"protocol": "@dbus-display", "fdname": "display"});
    ask(&stream, &mut reader, json!({"execute": "add_client", "arguments": client}), None)?;
    let control = connect_peer(ours, gio::DBusConnectionFlags::NONE)?;
    let fds = gio::UnixFDList::from_array([listener_theirs]);
    let handle = glib::variant::Handle(0);
    control
        .call_with_unix_fd_list_sync(
            None,
            CONSOLE,
            &format!("{DISPLAY_INTERFACE}.Console"),
            "RegisterListener",
            Some(&(handle,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            ANSWER_MS,
            Some(&fds),
            gio::Cancellable::NONE,
        )
        .map_err(to_io)?;
    let connection = connect_peer(listener, gio::DBusConnectionFlags::DELAY_MESSAGE_PROCESSING)?;
    let _ = control.close_sync(gio::Cancellable::NONE);
    Ok(connection)
}
