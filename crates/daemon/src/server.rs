//! The sockets QEMU connects to, and the loop that serves them.
//!
//! One thread does everything. That is what keeps the message path free of locks, and it gives the
//! session one order for the messages of both channels.

use crate::router::{Channel, FrameSink, Router};
use guesthouse_backend::{Backend, GuestMemory};
use guesthouse_transport::{FrameHeader, FrameStream};
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::ptr;
use std::time::{Duration, Instant};

/// How often to look at fences while something waits on them. The GPU gives no signal we can sleep on.
const FENCE_POLL: Duration = Duration::from_micros(100);
const NO_DESCRIPTOR: i32 = -1;

struct Connection {
    stream: UnixStream,
    frames: FrameStream,
}

struct Port {
    channel: Channel,
    listener: UnixListener,
    connection: Option<Connection>,
}

/// Writes through whatever connections are up. Without one the frame has nowhere to go, as when QEMU
/// has left, and is dropped.
struct Connections<'a> {
    ports: &'a mut [Port],
    scratch: &'a mut Vec<u8>,
}

impl FrameSink for Connections<'_> {
    fn send(&mut self, channel: Channel, header: FrameHeader, payload: &[u8]) -> io::Result<()> {
        let port =
            self.ports.iter_mut().find(|port| port.channel == channel).expect("every channel has a port");
        let Some(connection) = &mut port.connection else { return Ok(()) };
        // One write, so a frame is one system call and the two parts cannot be separated.
        self.scratch.clear();
        self.scratch.extend_from_slice(&header.encode());
        self.scratch.extend_from_slice(payload);
        connection.stream.write_all(self.scratch)
    }
}

/// One VM's sockets and session.
pub struct Server<B, M> {
    router: Router<B, M>,
    ports: Vec<Port>,
    waiting: bool,
    scratch: Vec<u8>,
}

impl<B: Backend, M: GuestMemory> Server<B, M> {
    /// Listens in `run_folder`, replacing sockets a previous daemon left behind.
    pub fn bind(run_folder: &Path, router: Router<B, M>) -> io::Result<Self> {
        fs::create_dir_all(run_folder)?;
        let mut ports = Vec::new();
        for channel in Channel::ALL {
            let path = run_folder.join(channel.socket_name());
            match fs::remove_file(&path) {
                Err(error) if error.kind() != ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            ports.push(Port { channel, listener: UnixListener::bind(path)?, connection: None });
        }
        Ok(Self { router, ports, waiting: false, scratch: Vec::new() })
    }

    pub fn router(&self) -> &Router<B, M> {
        &self.router
    }

    /// Serves until an error nothing can recover from.
    pub fn run(&mut self) -> io::Result<()> {
        loop {
            let timeout = self.waiting.then_some(FENCE_POLL);
            self.step(timeout)?;
        }
    }

    /// Waits for something to happen, for at most `timeout` if given, and deals with it.
    pub fn step(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        // Each port has two places, its listener and its connection. Poll skips a negative descriptor.
        let mut descriptors = [readable(NO_DESCRIPTOR); 2 * Channel::ALL.len()];
        for (port, pair) in self.ports.iter().zip(descriptors.as_chunks_mut::<2>().0) {
            pair[0].fd = port.listener.as_raw_fd();
            pair[1].fd = port.connection.as_ref().map_or(NO_DESCRIPTOR, |open| open.stream.as_raw_fd());
        }
        wait(&mut descriptors, timeout)?;
        for (index, pair) in descriptors.as_chunks::<2>().0.iter().enumerate() {
            if pair[1].revents != 0 {
                self.read_channel(index);
            }
            if pair[0].revents != 0 {
                let (stream, _) = self.ports[index].listener.accept()?;
                log::info!("QEMU connected to {:?}", self.ports[index].channel);
                self.ports[index].connection = Some(Connection { stream, frames: FrameStream::new() });
            }
        }
        let mut sink = Connections { ports: &mut self.ports, scratch: &mut self.scratch };
        self.waiting = self.router.poll(Instant::now(), &mut sink)?;
        Ok(())
    }

    /// Reads and handles what a channel has. A channel that fails closes, and QEMU can connect again.
    fn read_channel(&mut self, index: usize) {
        let channel = self.ports[index].channel;
        let Some(connection) = &mut self.ports[index].connection else { return };
        let mut open = match connection.frames.fill_from(&mut connection.stream) {
            Ok(0) => false,
            Ok(_) => true,
            Err(error) => {
                log::warn!("{channel:?}: {error}");
                false
            }
        };
        // Taken out for the loop, as replies need the ports while a frame borrows from this.
        let mut frames = std::mem::take(&mut connection.frames);
        while open && let Some(frame) = frames.next_frame() {
            let mut sink = Connections { ports: &mut self.ports, scratch: &mut self.scratch };
            if let Err(error) = self.router.handle_frame(channel, frame, &mut sink) {
                log::warn!("{channel:?}: {error}");
                open = false;
            }
        }
        match &mut self.ports[index].connection {
            Some(connection) if open => connection.frames = frames,
            connection => {
                log::info!("{channel:?}: connection closed");
                *connection = None;
            }
        }
    }
}

fn readable(fd: i32) -> libc::pollfd {
    libc::pollfd { fd, events: libc::POLLIN, revents: 0 }
}

fn wait(descriptors: &mut [libc::pollfd], timeout: Option<Duration>) -> io::Result<()> {
    let time = timeout.map(|timeout| libc::timespec {
        tv_sec: timeout.as_secs() as libc::time_t,
        tv_nsec: libc::c_long::from(timeout.subsec_nanos() as i32),
    });
    let time = time.as_ref().map_or(ptr::null(), ptr::from_ref);
    // SAFETY: both pointers are valid for the call, and the length is the slice's own.
    let count = unsafe {
        libc::ppoll(descriptors.as_mut_ptr(), descriptors.len() as libc::nfds_t, time, ptr::null())
    };
    if count < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}
