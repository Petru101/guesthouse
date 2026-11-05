//! `round_trip SOCKET`: times submissions through a running daemon, playing QEMU on its vgpu socket.
//!
//! The time includes the socket and both processes waking up, so it can compare two daemons on the
//! same machine, whatever language they are in. It does not include QEMU, VMBus or the guest.

use guesthouse_messages::wire::{set_u32, set_u64, u32_at};
use guesthouse_messages::{Command, HEADER_SIZE};
use guesthouse_transport::{FrameFlags, FrameHeader, FrameKind, HEADER_SIZE as FRAME_HEADER_SIZE};
use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Instant;

const WARM_UP: usize = 2_000;
const ROUNDS: usize = 50_000;
const IO_SPACE: u64 = 1 << 40;
const QUEUE_PRIVATE_SIZE: usize = 0x80;
const PROCESS: u32 = 0x50;
const WANTS_COMPLETION: FrameFlags = FrameFlags(1);

fn call(stream: &mut UnixStream, message: &[u8], transaction: u64, reply: &mut Vec<u8>) {
    let header = FrameHeader {
        payload_len: message.len() as u32,
        kind: FrameKind::Inband,
        flags: WANTS_COMPLETION,
        transaction,
    };
    let frame = [&header.encode()[..], message].concat();
    stream.write_all(&frame).unwrap();
    let mut head = [0; FRAME_HEADER_SIZE];
    stream.read_exact(&mut head).unwrap();
    reply.resize(FrameHeader::parse(&head).payload_len as usize, 0);
    stream.read_exact(reply).unwrap();
}

fn message(command: Command, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    set_u32(&mut bytes, 8, PROCESS);
    set_u32(&mut bytes, 16, command.to_wire());
    bytes
}

fn main() {
    let path = env::args().nth(1).expect("usage: round_trip SOCKET");
    let mut stream = UnixStream::connect(path).unwrap();
    let mut reply = Vec::new();

    let mut io_space = message(Command::SetIoSpace, HEADER_SIZE + 16);
    set_u64(&mut io_space, HEADER_SIZE, IO_SPACE);
    call(&mut stream, &io_space, 1, &mut reply);
    call(&mut stream, &message(Command::CreateContext, 0x38), 2, &mut reply);
    let context = u32_at(&reply, 0x18).unwrap();
    let mut queue = message(Command::CreateHardwareQueue, 0x44 + QUEUE_PRIVATE_SIZE);
    set_u32(&mut queue, 0x38, context);
    set_u32(&mut queue, 0x40, QUEUE_PRIVATE_SIZE as u32);
    call(&mut stream, &queue, 3, &mut reply);
    let queue = u32_at(&reply, 0x1C).unwrap();

    let mut submission = message(Command::SubmitToHardwareQueue, 0x50 + 0x40);
    set_u32(&mut submission, 0x18, queue);
    let mut times = Vec::with_capacity(ROUNDS);
    for round in 0..WARM_UP + ROUNDS {
        set_u64(&mut submission, 0x20, round as u64 + 1);
        let started = Instant::now();
        call(&mut stream, &submission, round as u64 + 4, &mut reply);
        if round >= WARM_UP {
            times.push(started.elapsed());
        }
    }
    times.sort();
    let percentile = |share: f64| times[((ROUNDS - 1) as f64 * share) as usize].as_secs_f64() * 1e6;
    println!("median {:.1} us, 99th percentile {:.1} us", percentile(0.5), percentile(0.99));
}
