//! Random bytes for MAC addresses, tokens and snapshot tags.

use std::fs::File;
use std::io::Read;

const RANDOM_DEVICE: &str = "/dev/urandom";

/// Panics if the kernel's random device can't be read, which only a broken system does.
pub fn random_bytes<const SIZE: usize>() -> [u8; SIZE] {
    let mut bytes = [0; SIZE];
    File::open(RANDOM_DEVICE).and_then(|mut device| device.read_exact(&mut bytes)).expect("no random device");
    bytes
}

pub fn random_hex<const SIZE: usize>() -> String {
    random_bytes::<SIZE>().iter().map(|byte| format!("{byte:02x}")).collect()
}
