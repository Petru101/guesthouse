//! A pretend QEMU display: the monitor that hands over a socket, and the D-Bus calls QEMU makes on it.

use gtk::gio::prelude::*;
use gtk::{gio, glib};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};
use std::io::{IoSliceMut, Write};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;

const CONSOLE_XML: &str = r#"<node><interface name="org.qemu.Display1.Console">
  <method name="RegisterListener"><arg type="h" direction="in"/></method>
</interface></node>"#;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 200;
const STRIDE: u32 = WIDTH * 4;
const PIXMAN_X8R8G8B8: u32 = 0x2002_0888;
const PACE: Duration = Duration::from_millis(100);
const CALL_MS: i32 = 2000;
const LISTENER_PATH: &str = "/org/qemu/Display1/Listener";
const LISTENER: &str = "org.qemu.Display1.Listener";

/// What the monitor said, and any descriptor with it.
fn read_message(stream: &UnixStream) -> Option<(String, Option<OwnedFd>)> {
    let mut bytes = [0; 512];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let mut buffers = [IoSliceMut::new(&mut bytes)];
    let received = rustix::net::recvmsg(stream, &mut buffers, &mut control, RecvFlags::empty()).ok()?;
    let mut fd = None;
    for item in control.drain() {
        if let RecvAncillaryMessage::ScmRights(mut rights) = item {
            fd = rights.next();
        }
    }
    (received.bytes > 0).then(|| (String::from_utf8_lossy(&bytes[..received.bytes]).into_owned(), fd))
}

fn server_connection(file: OwnedFd, flags: gio::DBusConnectionFlags) -> gio::DBusConnection {
    let stream = gio::Socket::from_fd(file).unwrap().connection_factory_create_connection();
    let flags = gio::DBusConnectionFlags::AUTHENTICATION_SERVER
        | gio::DBusConnectionFlags::AUTHENTICATION_ALLOW_ANONYMOUS
        | flags;
    let guid = gio::functions::dbus_generate_guid();
    gio::DBusConnection::new_sync(&stream, Some(guid.as_str()), flags, None, gio::Cancellable::NONE).unwrap()
}

fn register_console(connection: &gio::DBusConnection, listener: Sender<OwnedFd>) {
    let info = gio::DBusNodeInfo::for_xml(CONSOLE_XML).unwrap();
    let interface = info.lookup_interface("org.qemu.Display1.Console").unwrap();
    connection
        .register_object("/org/qemu/Display1/Console_0", &interface)
        .method_call(move |_, _, _, _, _, _, invocation| {
            let fds = invocation.message().unix_fd_list().unwrap();
            listener.send(fds.get(0).unwrap()).unwrap();
            invocation.return_value(None);
        })
        .build()
        .unwrap();
}

fn call(connection: &gio::DBusConnection, method: &str, arguments: &glib::Variant) -> bool {
    let answer = connection.call_sync(
        None,
        LISTENER_PATH,
        LISTENER,
        method,
        Some(arguments),
        None,
        gio::DBusCallFlags::NONE,
        CALL_MS,
        gio::Cancellable::NONE,
    );
    answer.is_ok()
}

/// A whole frame, and then a square going down it in rectangles, as VGA would send them.
fn draw(connection: &gio::DBusConnection) {
    let whole = vec![0x40u8; (STRIDE * HEIGHT) as usize];
    call(connection, "Scanout", &(WIDTH, HEIGHT, STRIDE, PIXMAN_X8R8G8B8, whole).to_variant());
    for step in 0.. {
        let (side, y) = (40i32, step * 4 % (HEIGHT as i32 - 40));
        let square = vec![0xC0u8; (side * side * 4) as usize];
        let update = (20i32, y, side, side, (side * 4) as u32, PIXMAN_X8R8G8B8, square).to_variant();
        if !call(connection, "Update", &update) {
            return;
        }
        thread::sleep(PACE);
    }
}

fn serve_client(mut stream: UnixStream) {
    stream.write_all(b"{\"QMP\": {}}\n").unwrap();
    let mut file = None;
    let (found, listener) = channel();
    let context = glib::MainContext::new();
    loop {
        let (text, fd) = read_message(&stream).unwrap();
        file = fd.or(file);
        stream.write_all(b"{\"return\": {}}\n").unwrap();
        if text.contains("add_client") {
            break;
        }
    }
    context
        .with_thread_default(|| {
            let control =
                server_connection(file.unwrap(), gio::DBusConnectionFlags::DELAY_MESSAGE_PROCESSING);
            register_console(&control, found);
            control.start_message_processing();
            let fd = loop {
                context.iteration(true);
                if let Ok(fd) = listener.try_recv() {
                    break fd;
                }
            };
            // The answer has to go out before the client can come to the other socket.
            while context.iteration(false) {}
            draw(&server_connection(fd, gio::DBusConnectionFlags::NONE));
        })
        .unwrap();
}

/// Serves one viewer from a thread, on this monitor socket.
pub fn serve(monitor: &Path) {
    let _ = std::fs::remove_file(monitor);
    let listener = UnixListener::bind(monitor).unwrap();
    thread::spawn(move || serve_client(listener.accept().unwrap().0));
}
