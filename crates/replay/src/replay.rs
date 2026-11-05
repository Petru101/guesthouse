//! Feeding a recording through the router.

use crate::answers::{Provenance, RecordedAnswers};
use crate::explain::{explain, first_difference};
use crate::recording::{BadRecording, Record, RecordedFrame, Recording};
use crate::report::{Difference, Report};
use guesthouse_backend::{AdapterFacts, GuestMemory, PciCard};
use guesthouse_backend_standin::SparseMemory;
use guesthouse_core::{Config, MemoryLimits, Session};
use guesthouse_daemon::guest_memory::WINDOW_BASE;
use guesthouse_daemon::router::{Channel, FrameSink, Router};
use guesthouse_messages::{Command, MessageHeader, strip_extended_header};
use guesthouse_transport::{Frame, FrameFlags, FrameHeader, FrameKind, split_ranges};
use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

const CHANNEL_GLOBAL: u8 = 0;
const CHANNEL_VGPU: u8 = 1;

/// Functions in the reference daemon that took GPU addresses for reasons the core has too. Takes by
/// any other caller are a vendor's channels or copies, which the replay repeats by hand.
const CORE_ADDRESS_CALLERS: [&str; 4] =
    ["fence_block", "reply_map_gpu_va", "reply_reserve_gpu_va", "surface_address"];

/// Bytes of the clock answer that hold the host's CPU clock, which can never repeat.
const CLOCK_ANSWER_CPU_CLOCK: std::ops::Range<usize> = 20..28;

type ReplaySession = Session<RecordedAnswers, SparseMemory>;

fn channel_of(number: u8) -> Result<Channel, BadRecording> {
    match number {
        CHANNEL_GLOBAL => Ok(Channel::Global),
        CHANNEL_VGPU => Ok(Channel::Vgpu),
        other => Err(BadRecording(format!("unknown channel {other}"))),
    }
}

/// Keeps what the router sends.
#[derive(Default)]
struct Sink {
    reply: Option<Vec<u8>>,
    notices: Vec<Vec<u8>>,
}

impl FrameSink for Sink {
    fn send(&mut self, _channel: Channel, header: FrameHeader, payload: &[u8]) -> io::Result<()> {
        if header.kind == FrameKind::Completion {
            self.reply = Some(payload.to_vec());
        } else {
            self.notices.push(payload.to_vec());
        }
        Ok(())
    }
}

fn build_session(recording: &Recording<'_>) -> Result<ReplaySession, BadRecording> {
    if recording.number("arena")? != 0 {
        return Err(BadRecording("this session used the arena, which is not ported yet".into()));
    }
    let facts = AdapterFacts {
        card: PciCard {
            vendor: recording.number("vendor")? as u16,
            device: recording.number("device")? as u16,
            revision: recording.number("revision")? as u8,
        },
        name: recording.setting("adapter_name")?.to_owned(),
        video_memory: recording.number("card_dedicated")?,
        node_count: recording.number("node_count")? as u32,
        guest_driver_version: 0,
    };
    let cap = recording.number("video_cap")?;
    let limits = MemoryLimits {
        video_cap: (cap != 0).then_some(cap),
        video_budget: recording.number("video_budget")?,
        system_budget: recording.number("guest_ram")?,
    };
    let config = Config {
        limits,
        async_messages: recording.number("async_messages")? != 0,
        // A replay has no GPU to wait for, and its fences move only when the recording says.
        frame_wait: Duration::ZERO,
    };
    let memory = SparseMemory::new(WINDOW_BASE + recording.number("window_size")?);
    Ok(Session::new(RecordedAnswers::new(facts), memory, config))
}

/// The message inside a frame, as the router will see it.
fn message_of<'a>(frame: &RecordedFrame<'a>) -> Option<&'a [u8]> {
    let message = if FrameFlags(frame.flags).has_ranges() {
        split_ranges(frame.payload).ok()?.1
    } else {
        frame.payload
    };
    Some(strip_extended_header(message))
}

/// Finds the recorded reply to each message, by channel and transaction.
fn expected_replies<'a>(recording: &Recording<'a>) -> HashMap<usize, &'a [u8]> {
    let mut waiting: HashMap<(u8, u64), usize> = HashMap::new();
    let mut replies = HashMap::new();
    for (index, record) in recording.records.iter().enumerate() {
        match record {
            Record::Incoming(frame) if FrameFlags(frame.flags).wants_completion() => {
                waiting.insert((frame.channel, frame.transaction), index);
            }
            Record::Outgoing(frame) if FrameKind::from_wire(frame.kind) == FrameKind::Completion => {
                if let Some(incoming) = waiting.remove(&(frame.channel, frame.transaction)) {
                    replies.insert(incoming, frame.payload);
                }
            }
            _ => {}
        }
    }
    replies
}

/// Counts what the host sent by itself in the recording.
fn recorded_notices<'a>(recording: &Recording<'a>) -> HashMap<&'a [u8], usize> {
    let mut notices = HashMap::new();
    for record in &recording.records {
        if let Record::Outgoing(frame) = record
            && FrameKind::from_wire(frame.kind) == FrameKind::Inband
        {
            *notices.entry(frame.payload).or_insert(0) += 1;
        }
    }
    notices
}

fn is_handled(frame: &RecordedFrame<'_>) -> bool {
    let kind = FrameKind::from_wire(frame.kind);
    !kind.is_lifecycle_event() && kind != FrameKind::Completion
}

/// Takes what can never match out of a pair of replies.
fn mask_volatile(command: Command, reply: &mut [u8]) {
    if command == Command::QueryClockCalibration && reply.len() >= CLOCK_ANSWER_CPU_CLOCK.end {
        reply[CLOCK_ANSWER_CPU_CLOCK].fill(0);
    }
}

/// Everything between one message and the next that shapes the replay of the first.
struct Around {
    fences_in_handler: Vec<(u64, u64)>,
    fences_after: Vec<(u64, u64)>,
    work: Vec<bool>,
    vendor_addresses: Vec<(u64, u32)>,
}

fn around(records: &[Record<'_>]) -> Around {
    let mut found = Around {
        fences_in_handler: Vec::new(),
        fences_after: Vec::new(),
        work: Vec::new(),
        vendor_addresses: Vec::new(),
    };
    for record in records {
        match *record {
            Record::Incoming(_) => break,
            Record::Fence { address, value, in_handler: true } => {
                found.fences_in_handler.push((address, value));
            }
            Record::Fence { address, value, in_handler: false } => found.fences_after.push((address, value)),
            Record::Work { on_gpu, .. } => found.work.push(on_gpu),
            Record::GpuAddresses { size, align, caller, .. } if !CORE_ADDRESS_CALLERS.contains(&caller) => {
                found.vendor_addresses.push((size, align));
            }
            _ => {}
        }
    }
    found
}

/// Replays a whole recording. Differences go in the report and do not stop it, as a later count
/// says how far one mistake spread.
pub fn replay(recording: &Recording<'_>) -> Result<Report, BadRecording> {
    let mut router = Router::new(build_session(recording)?);
    let replies = expected_replies(recording);
    let mut unmatched = recorded_notices(recording);
    let mut report = Report::default();
    let mut sink = Sink::default();
    // A fixed time: deadlines would otherwise depend on how fast the replay runs.
    let now = Instant::now();

    for (index, record) in recording.records.iter().enumerate() {
        let Record::Incoming(frame) = record else { continue };
        if !is_handled(frame) {
            continue;
        }
        let shaped_by = around(&recording.records[index + 1..]);
        for &(address, value) in &shaped_by.fences_in_handler {
            router.session_mut().memory_mut().write_u64(address, value);
        }
        let message = message_of(frame).ok_or_else(|| BadRecording("a frame cannot be read".into()))?;
        let expected = replies.get(&index).copied();
        let work = shaped_by.work.iter().copied();
        router.session_mut().backend_mut().expect(message, expected.unwrap_or(&[]), work);

        let header = FrameHeader {
            payload_len: frame.payload.len() as u32,
            kind: FrameKind::from_wire(frame.kind),
            flags: FrameFlags(frame.flags),
            transaction: frame.transaction,
        };
        let channel = channel_of(frame.channel)?;
        let sent = Frame { header, payload: frame.payload };
        router
            .handle_frame(channel, sent, &mut sink)
            .map_err(|error| BadRecording(format!("message {}: {error}", report.messages)))?;
        for (size, align) in shaped_by.vendor_addresses {
            let taken = router.session_mut().take_gpu_addresses(size, u64::from(align));
            taken.map_err(|error| BadRecording(error.to_string()))?;
        }
        for (address, value) in shaped_by.fences_after {
            router.session_mut().memory_mut().write_u64(address, value);
        }
        router.poll(now, &mut sink).map_err(|error| BadRecording(error.to_string()))?;

        let provenance = router.session().backend().provenance();
        compare(message, frame, expected, sink.reply.take(), provenance, &mut report);
        report.messages += 1;
        for notice in sink.notices.drain(..) {
            match unmatched.get_mut(notice.as_slice()) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    report.notices_matched += 1;
                }
                _ => report.notices_extra += 1,
            }
        }
    }
    report.notices_missing = unmatched.values().sum();
    Ok(report)
}

fn compare(
    message: &[u8],
    frame: &RecordedFrame<'_>,
    expected: Option<&[u8]>,
    actual: Option<Vec<u8>>,
    provenance: Provenance,
    report: &mut Report,
) {
    let header = MessageHeader::parse(message);
    let command = header.map_or(Command::Unknown(0), |header| header.command);
    *report.by_command.entry(format!("{command:?}")).or_insert(0) += 1;
    let (expected, mut actual) = match (expected, actual) {
        (Some(expected), Some(actual)) => (expected.to_vec(), actual),
        (None, None) => return,
        (None, Some(_)) => {
            report.no_recorded_reply += 1;
            return;
        }
        (Some(expected), None) => (expected.to_vec(), Vec::new()),
    };
    let mut expected = expected;
    mask_volatile(command, &mut expected);
    mask_volatile(command, &mut actual);
    let Some(offset) = first_difference(&expected, &actual) else {
        report.count(provenance);
        return;
    };
    report.different += 1;
    if report.first_difference.is_none() {
        report.first_difference = Some(Difference {
            message_index: report.messages,
            command: format!("{command:?}"),
            transaction: frame.transaction,
            offset,
            reason: explain(command, offset, &expected, &actual),
            expected_bytes: expected,
            actual_bytes: actual,
        });
    }
}
