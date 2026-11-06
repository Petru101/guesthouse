//! A guest's memory, reserved before it starts. Its file in shared memory fills as the guest touches
//! it, so without this a VM can pass every check and die later when other VMs have taken the room. All
//! of it is claimed now instead, which takes about a second for 16 GiB, and a VM either has it or
//! doesn't start.

use crate::failure::{Failure, Result};
use crate::host::{GIB, free_gib};
use rustix::fs::{FallocateFlags, fallocate};
use rustix::io::Errno;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const PRIVATE_FILE: u32 = 0o600;
const NO_ROOM_WORDS: &str = "GiB is free";

/// Whether a failed start was only for want of memory.
pub fn is_no_room(failure: &Failure) -> bool {
    failure.0.contains(NO_ROOM_WORDS)
}

fn no_room(file: &Path, bytes: u64) -> Failure {
    let folder = file.parent().unwrap_or(file);
    let free = free_gib(folder).unwrap_or(0.0);
    Failure(format!(
        "This machine needs {} GiB of {}, and {free:.0} {NO_ROOM_WORDS}. Stop another VM, or give this one less RAM.",
        bytes.div_ceil(GIB),
        folder.display(),
    ))
}

/// Makes the file and claims all of it. A failure leaves no file behind.
pub fn reserve(file: &Path, bytes: u64) -> Result<()> {
    // The slot's old file has just gone, so anything here now isn't ours to touch.
    let opened = OpenOptions::new().write(true).create_new(true).mode(PRIVATE_FILE).open(file)?;
    let claimed = fallocate(&opened, FallocateFlags::empty(), 0, bytes);
    drop(opened);
    claimed.map_err(|error| {
        let failure = match error {
            Errno::NOSPC | Errno::FBIG => no_room(file, bytes),
            other => Failure(format!("The VM's memory could not be reserved: {other}")),
        };
        let _ = fs::remove_file(file);
        failure
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::os::unix::fs::MetadataExt;

    const BLOCK: u64 = 512;

    #[test]
    fn a_file_already_there_is_left_alone() {
        let scratch = Scratch::new();
        let file = scratch.write("ram", "someone else's");
        assert!(reserve(&file, 4096).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "someone else's");
    }

    #[test]
    fn the_whole_file_is_claimed_at_once() {
        let scratch = Scratch::new();
        let file = scratch.path("ram");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        reserve(&file, 1 << 20).unwrap();
        let found = fs::metadata(&file).unwrap();
        assert_eq!(found.len(), 1 << 20);
        assert!(found.blocks() * BLOCK >= 1 << 20, "it must take up its room now, and not later");
    }

    #[test]
    fn what_doesnt_fit_is_refused_and_leaves_nothing() {
        let scratch = Scratch::new();
        let file = scratch.path("ram");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let error = reserve(&file, 1 << 62).unwrap_err();
        assert!(error.0.contains("GiB is free"), "{error}");
        assert!(!file.exists());
    }
}
