//! Bytes too big for a packet, handed over as a sealed file in memory. A viewer maps it, and neither
//! side can change it under the other.

use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::OwnedFd;

/// Makes the file. `name` only shows up in `/proc`.
pub fn sealed_bytes(name: &str, bytes: &[u8]) -> io::Result<OwnedFd> {
    let fd = memfd_create(name, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)?;
    let mut file = File::from(fd);
    file.write_all(bytes)?;
    let seals = SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL;
    fcntl_add_seals(&file, seals)?;
    Ok(file.into())
}

/// A file in memory of `size` bytes that its maker writes and a viewer reads, sealed so that its
/// length never changes.
pub fn sized_buffer(name: &str, size: u64) -> io::Result<OwnedFd> {
    let fd = memfd_create(name, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)?;
    let file = File::from(fd);
    file.set_len(size)?;
    fcntl_add_seals(&file, SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL)?;
    Ok(file.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek};

    #[test]
    fn a_sized_buffer_keeps_its_length() {
        let file = File::from(sized_buffer("test", 8192).unwrap());
        assert_eq!(file.metadata().unwrap().len(), 8192);
        assert!(file.set_len(4096).is_err());
        assert!(file.set_len(16384).is_err());
    }

    #[test]
    fn what_goes_in_can_be_read_and_not_changed() {
        let fd = sealed_bytes("test", b"picture").unwrap();
        let mut file = File::from(fd);
        file.rewind().unwrap();
        let mut read = Vec::new();
        file.read_to_end(&mut read).unwrap();
        assert_eq!(read, b"picture");
        assert!(file.write_all(b"x").is_err());
    }
}
