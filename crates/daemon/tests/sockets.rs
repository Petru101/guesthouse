//! The daemon end to end over real sockets, with this test playing QEMU.

use guesthouse_backend::Backend;
use guesthouse_backend_standin::{SparseMemory, StandIn};
use guesthouse_core::{Config, MemoryLimits, Session};
use guesthouse_daemon::router::{Channel, Router};
use guesthouse_daemon::server::Server;
use guesthouse_messages::wire::{set_u32, set_u64, u32_at, u64_at};
use guesthouse_messages::{Command, HEADER_SIZE};
use guesthouse_transport::{FrameFlags, FrameHeader, FrameKind, HEADER_SIZE as FRAME_HEADER_SIZE};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

const STEP: Option<Duration> = Some(Duration::from_millis(200));
const WANTS_COMPLETION: FrameFlags = FrameFlags(1);

type TestServer = Server<StandIn, SparseMemory>;

fn run_folder(name: &str) -> PathBuf {
    env::temp_dir().join(format!("guesthouse-sockets-{}-{name}", process::id()))
}

fn server(folder: &Path) -> TestServer {
    let backend = StandIn::new();
    let limits = MemoryLimits::new(backend.facts(), None, 1 << 30);
    let session = Session::new(backend, SparseMemory::new(1 << 41), Config::new(limits));
    Server::bind(folder, Router::new(session)).unwrap()
}

fn connect(server: &mut TestServer, folder: &Path, channel: Channel) -> UnixStream {
    let stream = UnixStream::connect(folder.join(channel.socket_name())).unwrap();
    server.step(STEP).unwrap();
    stream
}

fn send(stream: &mut UnixStream, kind: FrameKind, flags: FrameFlags, transaction: u64, payload: &[u8]) {
    let header = FrameHeader { payload_len: payload.len() as u32, kind, flags, transaction };
    stream.write_all(&header.encode()).unwrap();
    stream.write_all(payload).unwrap();
}

fn receive(stream: &mut UnixStream) -> (FrameHeader, Vec<u8>) {
    let mut header = [0; FRAME_HEADER_SIZE];
    stream.read_exact(&mut header).unwrap();
    let header = FrameHeader::parse(&header);
    let mut payload = vec![0; header.payload_len as usize];
    stream.read_exact(&mut payload).unwrap();
    (header, payload)
}

fn message(command: Command, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

#[test]
fn only_their_owner_can_reach_the_sockets() {
    let folder = run_folder("modes");
    let server = server(&folder);
    for name in [Channel::Global.socket_name(), Channel::Vgpu.socket_name()] {
        let mode = fs::metadata(folder.join(name)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{name}");
    }
    drop(server);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_request_gets_a_completion_with_its_transaction() {
    let folder = run_folder("completion");
    let mut server = server(&folder);
    let mut vgpu = connect(&mut server, &folder, Channel::Vgpu);
    send(
        &mut vgpu,
        FrameKind::Inband,
        WANTS_COMPLETION,
        0x55,
        &message(Command::OpenAdapter, HEADER_SIZE + 8),
    );
    server.step(STEP).unwrap();
    let (header, payload) = receive(&mut vgpu);
    assert_eq!(header.kind, FrameKind::Completion);
    assert_eq!(header.transaction, 0x55);
    assert_eq!(u32_at(&payload, 8), Ok(40));
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn nothing_answers_a_request_that_wants_no_completion() {
    let folder = run_folder("silent");
    let mut server = server(&folder);
    let mut vgpu = connect(&mut server, &folder, Channel::Vgpu);
    send(&mut vgpu, FrameKind::Inband, FrameFlags(0), 1, &message(Command::CreateDevice, HEADER_SIZE));
    send(&mut vgpu, FrameKind::Inband, WANTS_COMPLETION, 2, &message(Command::CreateDevice, HEADER_SIZE));
    server.step(STEP).unwrap();
    let (header, payload) = receive(&mut vgpu);
    assert_eq!(header.transaction, 2);
    assert_eq!(u32_at(&payload, 0), Ok(0x4000_1080), "the first message still made its handle");
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn ranges_and_the_extended_header_are_stripped() {
    let folder = run_folder("ranges");
    let mut server = server(&folder);
    let mut vgpu = connect(&mut server, &folder, Channel::Vgpu);
    let mut payload = 1u32.to_le_bytes().to_vec();
    payload.extend_from_slice(&[0; 16]);
    payload.extend_from_slice(&16u32.to_le_bytes());
    payload.extend_from_slice(&[0; 12]);
    payload.extend_from_slice(&message(Command::CreateDevice, HEADER_SIZE));
    send(&mut vgpu, FrameKind::GpaDirect, FrameFlags(3), 7, &payload);
    server.step(STEP).unwrap();
    assert_eq!(receive(&mut vgpu).1.len(), 4);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn both_channels_share_one_session() {
    let folder = run_folder("shared");
    let mut server = server(&folder);
    let mut global = connect(&mut server, &folder, Channel::Global);
    let mut vgpu = connect(&mut server, &folder, Channel::Vgpu);
    send(&mut global, FrameKind::Inband, WANTS_COMPLETION, 1, &message(Command::CreateDevice, HEADER_SIZE));
    server.step(STEP).unwrap();
    send(&mut vgpu, FrameKind::Inband, WANTS_COMPLETION, 1, &message(Command::CreateDevice, HEADER_SIZE));
    server.step(STEP).unwrap();
    let first = u32_at(&receive(&mut global).1, 0).unwrap();
    let second = u32_at(&receive(&mut vgpu).1, 0).unwrap();
    assert_eq!(second, first + 0x40);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_notice_goes_out_on_the_global_channel_before_the_reply() {
    let folder = run_folder("notice");
    let mut server = server(&folder);
    let mut global = connect(&mut server, &folder, Channel::Global);
    let mut vgpu = connect(&mut server, &folder, Channel::Vgpu);
    let mut wait = message(Command::WaitOnCpu, 0x38);
    set_u64(&mut wait, 0x28, 0xE);
    send(&mut vgpu, FrameKind::Inband, WANTS_COMPLETION, 1, &wait);
    server.step(STEP).unwrap();
    let (header, payload) = receive(&mut global);
    assert_eq!((header.kind, header.transaction), (FrameKind::Inband, 0));
    assert_eq!(u64_at(&payload, 24), Ok(0xE));
    assert_eq!(receive(&mut vgpu).0.kind, FrameKind::Completion);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn qemu_can_come_back_and_the_session_stays() {
    let folder = run_folder("again");
    let mut server = server(&folder);
    let mut vgpu = connect(&mut server, &folder, Channel::Vgpu);
    send(&mut vgpu, FrameKind::Inband, WANTS_COMPLETION, 1, &message(Command::CreateDevice, HEADER_SIZE));
    server.step(STEP).unwrap();
    let first = u32_at(&receive(&mut vgpu).1, 0).unwrap();
    drop(vgpu);
    server.step(STEP).unwrap();
    let mut vgpu = connect(&mut server, &folder, Channel::Vgpu);
    send(&mut vgpu, FrameKind::Inband, WANTS_COMPLETION, 2, &message(Command::CreateDevice, HEADER_SIZE));
    server.step(STEP).unwrap();
    assert_eq!(u32_at(&receive(&mut vgpu).1, 0), Ok(first + 0x40));
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_frame_with_a_bad_range_list_closes_the_channel_and_nothing_else() {
    let folder = run_folder("bad");
    let mut server = server(&folder);
    let mut vgpu = connect(&mut server, &folder, Channel::Vgpu);
    send(&mut vgpu, FrameKind::GpaDirect, FrameFlags(3), 1, &[0xFF; 4]);
    server.step(STEP).unwrap();
    let mut byte = [0];
    assert_eq!(vgpu.read(&mut byte).unwrap(), 0);
    let mut vgpu = connect(&mut server, &folder, Channel::Vgpu);
    send(&mut vgpu, FrameKind::Inband, WANTS_COMPLETION, 2, &message(Command::CreateDevice, HEADER_SIZE));
    server.step(STEP).unwrap();
    assert_eq!(receive(&mut vgpu).0.transaction, 2);
    fs::remove_dir_all(folder).unwrap();
}
