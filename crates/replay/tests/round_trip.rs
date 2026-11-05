//! A recording made from the core's own answers must replay clean, and a change must be caught.

use guesthouse_backend::Backend;
use guesthouse_backend_standin::{SparseMemory, StandIn};
use guesthouse_core::{Config, MemoryLimits, Session};
use guesthouse_messages::wire::{set_u32, set_u64};
use guesthouse_messages::{Command, HEADER_SIZE};
use guesthouse_replay::recording::{RecordedFrame, Recording, RecordingWriter};
use guesthouse_replay::replay::replay;

const WANTS_COMPLETION: u16 = 1;
const INBAND: u16 = 6;
const COMPLETION: u16 = 0xB;
const VGPU: u8 = 1;
const IO_SPACE: u64 = 1 << 40;
const WINDOW_SIZE: u64 = 1 << 30;

fn settings() -> Vec<(&'static str, String)> {
    let facts = StandIn::new().facts().clone();
    vec![
        ("arena", "0".into()),
        ("vendor", facts.card.vendor.to_string()),
        ("device", facts.card.device.to_string()),
        ("revision", facts.card.revision.to_string()),
        ("adapter_name", facts.name),
        ("card_dedicated", facts.video_memory.to_string()),
        ("node_count", facts.node_count.to_string()),
        ("video_cap", "0".into()),
        ("video_budget", (6u64 << 30).to_string()),
        ("guest_ram", (16u64 << 30).to_string()),
        ("async_messages", "0".into()),
        ("window_size", WINDOW_SIZE.to_string()),
    ]
}

fn message(command: Command, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    set_u32(&mut bytes, 8, 0x4000_2000);
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

fn messages() -> Vec<Vec<u8>> {
    let mut io_space = message(Command::SetIoSpace, HEADER_SIZE + 16);
    set_u64(&mut io_space, HEADER_SIZE, IO_SPACE);
    let mut fence = message(Command::CreateSyncObject, 0x30);
    set_u32(&mut fence, 0x20, 5);
    let mut reserve = message(Command::ReserveGpuAddresses, 0x40);
    set_u64(&mut reserve, 0x38, 0x1000);
    vec![
        io_space,
        message(Command::OpenAdapter, HEADER_SIZE + 8),
        message(Command::CreateDevice, HEADER_SIZE),
        fence,
        reserve,
        message(Command::GetAdapterIdentity, HEADER_SIZE),
    ]
}

/// Runs the messages through a real session and writes down what it said.
fn record() -> Vec<u8> {
    let backend = StandIn::new();
    let limits = MemoryLimits::new(backend.facts(), None, 16 << 30);
    let window_end = guesthouse_daemon::guest_memory::WINDOW_BASE + WINDOW_SIZE;
    let mut session = Session::new(backend, SparseMemory::new(window_end), Config::new(limits));
    let mut writer = RecordingWriter::new(&settings());
    let mut out = Vec::new();
    for (index, message) in messages().iter().enumerate() {
        let transaction = index as u64 + 1;
        let frame = |kind, flags, payload| RecordedFrame { channel: VGPU, flags, kind, transaction, payload };
        writer.incoming(frame(INBAND, WANTS_COMPLETION, message));
        session.handle_message(message, &mut out);
        writer.outgoing(frame(COMPLETION, 0, &out));
    }
    writer.finish()
}

#[test]
fn a_session_replays_clean_against_itself() {
    let bytes = record();
    let report = replay(&Recording::parse(&bytes).unwrap()).unwrap();
    assert!(report.passed(), "{report}");
    assert_eq!(report.messages, messages().len());
    assert_eq!(report.worked_identical, messages().len());
}

#[test]
fn a_changed_byte_is_found_and_explained() {
    let mut bytes = record();
    let needle = 0x4000_1080u32.to_le_bytes();
    let at = bytes.windows(4).rposition(|window| window == needle).unwrap();
    bytes[at] = 0xC0;
    let report = replay(&Recording::parse(&bytes).unwrap()).unwrap();
    assert!(!report.passed());
    let first = report.first_difference.as_ref().unwrap();
    assert_eq!(first.command, "CreateSyncObject");
    assert!(first.reason.contains("handle"), "{}", first.reason);
}

#[test]
fn a_session_with_the_arena_is_refused_plainly() {
    let mut settings = settings();
    settings[0].1 = "1".into();
    let bytes = RecordingWriter::new(&settings).finish();
    let error = replay(&Recording::parse(&bytes).unwrap()).unwrap_err();
    assert!(error.0.contains("arena"));
}
