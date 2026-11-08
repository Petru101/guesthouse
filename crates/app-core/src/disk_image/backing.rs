//! The files a disk is built on. A qcow2 overlay, made by hand or brought in from elsewhere, reads
//! what it hasn't changed from its backing file, and that one may have its own. The app's own
//! snapshots live inside the qcow2, so the disks it makes have none.
//!
//! A jailed QEMU only reaches the files it's given, so these have to be found before it starts. Each
//! one is read in the format the one above it names, as QEMU does: a guest can write anything to a
//! raw disk, and a guessed format would let it name a file of the user's as a backing file.

use super::info;
use crate::failure::{Failure, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

/// The format QEMU is told the machine's disk is in.
const DISK_FORMAT: &str = "qcow2";
/// A raw file is all data, so a chain ends there.
const RAW: &str = "raw";
/// Far more than any real chain. Past this, it's more likely a loop than a disk.
const MOST_BACKING_FILES: usize = 16;
const FIX_FORMAT: &str = "Name it with qemu-img rebase -u -F FORMAT -b BACKING DISK.";

fn refusal(file: &Path, why: &str) -> Failure {
    Failure(format!("The VM did not start, as its disk is built on {}, which {why}.", file.display()))
}

/// Says why QEMU couldn't open this backing file, if it couldn't.
fn check(file: &Path) -> Result<()> {
    let metadata = file.metadata().map_err(|_| refusal(file, "is missing"))?;
    if !metadata.is_file() {
        return Err(refusal(file, "isn't a plain file"));
    }
    File::open(file).map_err(|error| refusal(file, &format!("can't be read: {error}")))?;
    Ok(())
}

/// A name QEMU reads as something other than a file: `json:{...}`, or a protocol's URL.
fn names_no_file(name: &str) -> bool {
    name.starts_with("json:") || name.contains("://")
}

/// The files the machine's disk is built on, nearest first, and none for a disk that stands alone.
/// Refuses a chain QEMU couldn't open, so a start says which file it is.
pub fn backing_files(disk: &Path) -> Result<Vec<PathBuf>> {
    let mut chain: Vec<PathBuf> = Vec::new();
    let (mut image, mut format) = (disk.to_owned(), DISK_FORMAT.to_owned());
    loop {
        let found = match info(image.as_os_str(), Some(&format)) {
            Ok(found) => found,
            // A disk that's missing or can't be read is QEMU's to say, as it always has been.
            Err(_) if chain.is_empty() => return Ok(chain),
            Err(reason) => return Err(refusal(&image, &format!("can't be read: {reason}"))),
        };
        // qemu-img gives the name as QEMU resolves it, against the folder of the file that names it.
        let Some(name) = found["full-backing-filename"].as_str() else { return Ok(chain) };
        if names_no_file(name) {
            return Err(Failure(format!(
                "The VM did not start, as its disk is built on {name}, which isn't a file."
            )));
        }
        let file = std::path::absolute(name)?;
        let Some(named) = found["backing-filename-format"].as_str() else {
            return Err(refusal(&file, &format!("has no format named. {FIX_FORMAT}")));
        };
        check(&file)?;
        if file == disk || chain.contains(&file) {
            return Err(refusal(&file, "is built on itself"));
        }
        if chain.len() == MOST_BACKING_FILES {
            return Err(refusal(&file, &format!("is more than {MOST_BACKING_FILES} files deep")));
        }
        chain.push(file.clone());
        if named == RAW {
            return Ok(chain);
        }
        (image, format) = (file, named.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::process::Command;

    fn qemu_img(words: &[&str]) {
        let done = Command::new("qemu-img").arg(words[0]).arg("-q").args(&words[1..]).status().unwrap();
        assert!(done.success(), "qemu-img {words:?}");
    }

    /// A qcow2 at `name` in the scratch folder, on `backing` as qemu-img would be told it.
    fn overlay(scratch: &Scratch, name: &str, backing: Option<(&str, &str)>) -> PathBuf {
        let path = scratch.path(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let path_text = path.to_str().unwrap();
        match backing {
            Some((file, format)) => qemu_img(&["create", "-f", "qcow2", "-b", file, "-F", format, path_text]),
            None => qemu_img(&["create", "-f", "qcow2", path_text, "1G"]),
        }
        path
    }

    #[test]
    fn a_disk_on_its_own_has_none() {
        let scratch = Scratch::new();
        let disk = overlay(&scratch, "disk.qcow2", None);
        assert_eq!(backing_files(&disk).unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_disk_qemu_cant_open_is_left_for_it_to_say() {
        let scratch = Scratch::new();
        let not_a_disk = scratch.write("disk.qcow2", "a disk");
        for disk in [Path::new("/nowhere/disk.qcow2"), &not_a_disk] {
            assert_eq!(backing_files(disk).unwrap(), Vec::<PathBuf>::new());
        }
    }

    #[test]
    fn relative_names_are_read_from_the_folder_of_the_file_that_names_them() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "bases/base.qcow2", None);
        let middle = overlay(&scratch, "bases/middle.qcow2", Some(("base.qcow2", "qcow2")));
        let disk = overlay(&scratch, "machine/disk.qcow2", Some(("../bases/middle.qcow2", "qcow2")));
        let chain = backing_files(&disk).unwrap();
        let resolved: Vec<PathBuf> = chain.iter().map(|file| file.canonicalize().unwrap()).collect();
        assert_eq!(resolved, [middle.canonicalize().unwrap(), base.canonicalize().unwrap()]);
        assert!(chain.iter().all(|file| file.is_absolute()), "{chain:?}");
    }

    #[test]
    fn an_absolute_name_is_taken_as_it_is() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "elsewhere/base.qcow2", None);
        let disk = overlay(&scratch, "machine/disk.qcow2", Some((base.to_str().unwrap(), "qcow2")));
        assert_eq!(backing_files(&disk).unwrap(), [base]);
    }

    #[test]
    fn a_raw_base_ends_the_chain() {
        let scratch = Scratch::new();
        let base = scratch.write("base.img", vec![0; 4096]);
        let disk = overlay(&scratch, "disk.qcow2", Some(("base.img", "raw")));
        assert_eq!(backing_files(&disk).unwrap(), [base]);
    }

    #[test]
    fn a_missing_file_anywhere_in_the_chain_is_named() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "base.qcow2", None);
        overlay(&scratch, "middle.qcow2", Some(("base.qcow2", "qcow2")));
        let disk = overlay(&scratch, "disk.qcow2", Some(("middle.qcow2", "qcow2")));
        std::fs::remove_file(&base).unwrap();
        let refused = backing_files(&disk).unwrap_err();
        assert_eq!(
            refused.0,
            format!("The VM did not start, as its disk is built on {}, which is missing.", base.display())
        );
    }

    #[test]
    fn a_folder_is_not_a_backing_file() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "base.qcow2", None);
        let disk = overlay(&scratch, "disk.qcow2", Some(("base.qcow2", "qcow2")));
        std::fs::remove_file(&base).unwrap();
        std::fs::create_dir(&base).unwrap();
        assert!(backing_files(&disk).unwrap_err().0.ends_with("isn't a plain file."));
    }

    #[test]
    fn a_loop_is_refused() {
        let scratch = Scratch::new();
        overlay(&scratch, "first.qcow2", None);
        let second = overlay(&scratch, "second.qcow2", Some(("first.qcow2", "qcow2")));
        let first = scratch.path("first.qcow2");
        // Unsafe, so qemu-img only writes the name and doesn't open the loop it makes.
        qemu_img(&[
            "rebase",
            "-u",
            "-f",
            "qcow2",
            "-b",
            "second.qcow2",
            "-F",
            "qcow2",
            first.to_str().unwrap(),
        ]);
        assert!(backing_files(&second).unwrap_err().0.ends_with("is built on itself."));
    }
}
