//! When faults stop a guest's GPU work, the app has to be able to hear of it.

use guesthouse_backend::Backend;
use guesthouse_backend_standin::{SparseMemory, StandIn};
use guesthouse_core::{Config, MOST_FAULTS, MemoryLimits, Session};
use guesthouse_daemon::gpu_note::read_note;
use guesthouse_daemon::router::Router;
use guesthouse_daemon::server::Server;
use guesthouse_messages::wire::{set_u32, set_u64, u32_at};
use guesthouse_messages::{Command, HEADER_SIZE};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{env, fs, process};

const WINDOW_END: u64 = (1 << 40) + (1 << 30);
const IO_SPACE: u64 = 1 << 40;
const NO_WAIT: Option<Duration> = Some(Duration::ZERO);
/// Longer than a queue gets.
const LATER: Duration = Duration::from_secs(6);

type TestServer = Server<StandIn, SparseMemory>;

fn message(command: Command, process: u32, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size.max(HEADER_SIZE)];
    set_u32(&mut bytes, 8, process);
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

fn send(server: &mut TestServer, request: &[u8]) -> Vec<u8> {
    let mut reply = Vec::new();
    let _ = server.router_mut().session_mut().handle_message(request, None, &mut reply);
    reply
}

fn server_in(folder: &Path) -> TestServer {
    let mut backend = StandIn::new();
    backend.run_work = true;
    let limits = MemoryLimits::new(backend.facts(), None, 16 << 30);
    let session = Session::new(backend, SparseMemory::new(WINDOW_END), Config::new(limits));
    let mut server = Server::bind(folder, Router::new(session)).unwrap();
    let mut io_space = message(Command::SetIoSpace, 0, HEADER_SIZE + 16);
    set_u64(&mut io_space, HEADER_SIZE, IO_SPACE);
    set_u64(&mut io_space, HEADER_SIZE + 8, 8 << 30);
    send(&mut server, &io_space);
    server
}

/// Gives a new process a queue, with work on the GPU that will never finish.
fn hang_a_process(server: &mut TestServer, process: u32) {
    let context = send(server, &message(Command::CreateContext, process, 0x34));
    let mut queue = message(Command::CreateHardwareQueue, process, 0x48);
    set_u32(&mut queue, 0x38, u32_at(&context, 0x18).unwrap());
    set_u32(&mut queue, 0x40, 4);
    let queue = send(server, &queue);
    let mut work = message(Command::SubmitToHardwareQueue, process, 0x50);
    set_u32(&mut work, 0x18, u32_at(&queue, 0x1C).unwrap());
    set_u64(&mut work, 0x20, 1);
    send(server, &work);
    server.router_mut().session_mut().poll(Instant::now() + LATER);
}

fn scratch_folder(name: &str) -> PathBuf {
    env::temp_dir().join(format!("guesthouse-gpu-note-{name}-{}", process::id()))
}

#[test]
fn the_app_hears_when_faults_stop_all_work_and_when_a_restart_ends_that() {
    let folder = scratch_folder("stopped");
    let mut server = server_in(&folder);
    server.step(NO_WAIT).unwrap();
    assert_eq!(read_note(&folder), None);
    (0..MOST_FAULTS as u32).for_each(|process| hang_a_process(&mut server, process + 1));
    server.step(NO_WAIT).unwrap();
    assert!(read_note(&folder).is_some(), "the note should be there");
    server.router_mut().session_mut().guest_reset();
    server.step(NO_WAIT).unwrap();
    assert_eq!(read_note(&folder), None);
    fs::remove_dir_all(&folder).unwrap();
}

#[test]
fn a_note_left_by_an_earlier_run_goes() {
    let folder = scratch_folder("stale");
    fs::create_dir_all(&folder).unwrap();
    guesthouse_daemon::gpu_note::write_note(&folder, true).unwrap();
    let _server = server_in(&folder);
    assert_eq!(read_note(&folder), None);
    fs::remove_dir_all(&folder).unwrap();
}
