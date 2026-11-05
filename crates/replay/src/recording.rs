//! The recording file format.
//!
//! A text head, then binary records:
//!
//! ```text
//! guesthouse-recording 1
//! key=value
//! ...
//! (an empty line)
//! ```
//!
//! Each record is a 28-byte little-endian head and then `length` payload bytes:
//!
//! | Field | Size | Incoming and outgoing | Fence | GPU addresses | Work |
//! |-------|------|-----------------------|-------|---------------|------|
//! | kind | 1 | 1, 2 | 3 | 4 | 5 |
//! | channel | 1 | 0 global, 1 vgpu | | | |
//! | flags | 2 | frame flags | 1 if read while handling | | 1 if the GPU took it |
//! | number | 4 | frame kind | | alignment | |
//! | a | 8 | transaction | fence address | address | queue |
//! | b | 8 | | value | size | fence value |
//! | length | 4 | payload bytes | 0 | caller's name | 0 |
//!
//! Recordings hold guest data and vendor bytes. They never go in git.

use std::collections::HashMap;
use std::fmt;

/// The first line of every recording.
pub const MAGIC_LINE: &str = "guesthouse-recording 1";

const RECORD_HEAD_SIZE: usize = 28;

const KIND_INCOMING: u8 = 1;
const KIND_OUTGOING: u8 = 2;
const KIND_FENCE: u8 = 3;
const KIND_GPU_ADDRESSES: u8 = 4;
const KIND_WORK: u8 = 5;

/// A recording that cannot be read, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadRecording(pub String);

impl fmt::Display for BadRecording {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BadRecording {}

/// A frame as it crossed the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedFrame<'a> {
    pub channel: u8,
    pub flags: u16,
    pub kind: u16,
    pub transaction: u64,
    pub payload: &'a [u8],
}

/// One entry of a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Record<'a> {
    Incoming(RecordedFrame<'a>),
    Outgoing(RecordedFrame<'a>),
    /// A fence changed. `in_handler` says the message thread read it, so that a decision in the
    /// current message may rest on it.
    Fence {
        address: u64,
        value: u64,
        in_handler: bool,
    },
    /// The reference daemon took GPU addresses, and `caller` is the function that did.
    GpuAddresses {
        address: u64,
        size: u64,
        align: u32,
        caller: &'a str,
    },
    /// What became of a submission.
    Work {
        queue: u64,
        value: u64,
        on_gpu: bool,
    },
}

/// A parsed recording, borrowing the file's bytes.
#[derive(Debug)]
pub struct Recording<'a> {
    pub settings: HashMap<&'a str, &'a str>,
    pub records: Vec<Record<'a>>,
}

impl<'a> Recording<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, BadRecording> {
        let (settings, mut at) = parse_head(bytes)?;
        let mut records = Vec::new();
        while at < bytes.len() {
            let (record, next) = parse_record(bytes, at)?;
            records.push(record);
            at = next;
        }
        Ok(Self { settings, records })
    }

    /// A setting that must be there.
    pub fn setting(&self, name: &str) -> Result<&'a str, BadRecording> {
        let missing = || BadRecording(format!("the recording does not say {name}"));
        self.settings.get(name).copied().ok_or_else(missing)
    }

    pub fn number(&self, name: &str) -> Result<u64, BadRecording> {
        let text = self.setting(name)?;
        text.parse().map_err(|_| BadRecording(format!("{name} is not a number: {text}")))
    }
}

fn parse_head(bytes: &[u8]) -> Result<(HashMap<&str, &str>, usize), BadRecording> {
    let bad = |text: &str| BadRecording(text.to_owned());
    let mut settings = HashMap::new();
    let mut at = 0;
    let mut first = true;
    loop {
        let rest = bytes.get(at..).ok_or_else(|| bad("the head never ends"))?;
        let end = rest.iter().position(|&byte| byte == b'\n').ok_or_else(|| bad("the head never ends"))?;
        let line = std::str::from_utf8(&rest[..end]).map_err(|_| bad("the head is not text"))?;
        at += end + 1;
        if first {
            if line != MAGIC_LINE {
                return Err(bad("this is not a recording this version can read"));
            }
            first = false;
        } else if line.is_empty() {
            return Ok((settings, at));
        } else {
            let (key, value) = line.split_once('=').ok_or_else(|| bad("a head line has no ="))?;
            settings.insert(key, value);
        }
    }
}

fn u64_of(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

fn u32_of(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn parse_record(bytes: &[u8], at: usize) -> Result<(Record<'_>, usize), BadRecording> {
    let cut_short = || BadRecording(format!("the record at byte {at} is cut short"));
    let head = bytes.get(at..at + RECORD_HEAD_SIZE).ok_or_else(cut_short)?;
    let [kind, channel, flags_low, flags_high, ..] = *head else { return Err(cut_short()) };
    let flags = u16::from_le_bytes([flags_low, flags_high]);
    let number = u32_of(head, 4);
    let (a, b) = (u64_of(head, 8), u64_of(head, 16));
    let length = u32_of(head, 24) as usize;
    let start = at + RECORD_HEAD_SIZE;
    let payload = bytes.get(start..start + length).ok_or_else(cut_short)?;
    let frame = RecordedFrame { channel, flags, kind: number as u16, transaction: a, payload };
    let record = match kind {
        KIND_INCOMING => Record::Incoming(frame),
        KIND_OUTGOING => Record::Outgoing(frame),
        KIND_FENCE => Record::Fence { address: a, value: b, in_handler: flags != 0 },
        KIND_GPU_ADDRESSES => {
            let caller = std::str::from_utf8(payload).map_err(|_| cut_short())?;
            Record::GpuAddresses { address: a, size: b, align: number, caller }
        }
        KIND_WORK => Record::Work { queue: a, value: b, on_gpu: flags != 0 },
        other => return Err(BadRecording(format!("unknown record kind {other} at byte {at}"))),
    };
    Ok((record, start + length))
}

/// Writes recordings, for tests. The reference daemon's recorder writes the same format.
#[derive(Debug, Default)]
pub struct RecordingWriter {
    bytes: Vec<u8>,
}

impl RecordingWriter {
    pub fn new(settings: &[(&str, String)]) -> Self {
        let mut bytes = format!("{MAGIC_LINE}\n").into_bytes();
        for (key, value) in settings {
            bytes.extend_from_slice(format!("{key}={value}\n").as_bytes());
        }
        bytes.push(b'\n');
        Self { bytes }
    }

    fn head(&mut self, kind: u8, channel: u8, flags: u16, number: u32) {
        self.bytes.extend_from_slice(&[kind, channel]);
        self.bytes.extend_from_slice(&flags.to_le_bytes());
        self.bytes.extend_from_slice(&number.to_le_bytes());
    }

    fn tail(&mut self, a: u64, b: u64, payload: &[u8]) {
        self.bytes.extend_from_slice(&a.to_le_bytes());
        self.bytes.extend_from_slice(&b.to_le_bytes());
        self.bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        self.bytes.extend_from_slice(payload);
    }

    fn frame(&mut self, kind: u8, frame: RecordedFrame<'_>) {
        self.head(kind, frame.channel, frame.flags, u32::from(frame.kind));
        self.tail(frame.transaction, 0, frame.payload);
    }

    pub fn incoming(&mut self, frame: RecordedFrame<'_>) {
        self.frame(KIND_INCOMING, frame);
    }

    pub fn outgoing(&mut self, frame: RecordedFrame<'_>) {
        self.frame(KIND_OUTGOING, frame);
    }

    pub fn fence(&mut self, address: u64, value: u64, in_handler: bool) {
        self.head(KIND_FENCE, 0, u16::from(in_handler), 0);
        self.tail(address, value, &[]);
    }

    pub fn gpu_addresses(&mut self, address: u64, size: u64, align: u32, caller: &str) {
        self.head(KIND_GPU_ADDRESSES, 0, 0, align);
        self.tail(address, size, caller.as_bytes());
    }

    pub fn work(&mut self, queue: u64, value: u64, on_gpu: bool) {
        self.head(KIND_WORK, 0, u16::from(on_gpu), 0);
        self.tail(queue, value, &[]);
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(payload: &[u8]) -> RecordedFrame<'_> {
        RecordedFrame { channel: 1, flags: 1, kind: 6, transaction: 9, payload }
    }

    #[test]
    fn every_record_kind_survives_a_round_trip() {
        let mut writer = RecordingWriter::new(&[("guest_ram", "1024".to_owned())]);
        writer.incoming(frame(b"in"));
        writer.outgoing(frame(b"out"));
        writer.fence(0x1000, 5, true);
        writer.gpu_addresses(0x2000, 0x10, 64, "caller");
        writer.work(0x40, 3, true);
        let bytes = writer.finish();
        let recording = Recording::parse(&bytes).unwrap();
        assert_eq!(recording.number("guest_ram"), Ok(1024));
        assert_eq!(
            recording.records,
            [
                Record::Incoming(frame(b"in")),
                Record::Outgoing(frame(b"out")),
                Record::Fence { address: 0x1000, value: 5, in_handler: true },
                Record::GpuAddresses { address: 0x2000, size: 0x10, align: 64, caller: "caller" },
                Record::Work { queue: 0x40, value: 3, on_gpu: true },
            ]
        );
    }

    #[test]
    fn other_files_are_refused() {
        assert!(Recording::parse(b"something else\n\n").is_err());
        assert!(Recording::parse(b"no newline").is_err());
    }

    #[test]
    fn a_cut_record_is_refused() {
        let mut writer = RecordingWriter::new(&[]);
        writer.incoming(frame(b"payload"));
        let bytes = writer.finish();
        assert!(Recording::parse(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn a_missing_setting_says_which() {
        let bytes = RecordingWriter::new(&[]).finish();
        let recording = Recording::parse(&bytes).unwrap();
        assert!(recording.number("guest_ram").unwrap_err().0.contains("guest_ram"));
    }
}
