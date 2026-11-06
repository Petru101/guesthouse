//! The seed disc an unattended install reads: the answers, and the scripts for first boot. It also
//! makes root's key, and a password for logging in at a console if the desktop won't start. Both stay
//! as they are if they're already there.

use super::guest_sources;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::iso::iso_maker;
use crate::machine::Machine;
use crate::snapshots::copy_tree;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A random password's size, before base64.
const PASSWORD_BYTES: &str = "12";
const PRIVATE: u32 = 0o600;
/// The folder's mode becomes that of `/usr/lib/nvfwd`, which the desktop's user must enter.
const OPEN_FOLDER: u32 = 0o755;
const CAST_FILES: [&str; 2] = ["cast/Xwayland", "cast/firefox.desktop"];

pub fn seed_folder(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join("seed")
}

pub fn seed_disc(context: &Context, machine: &Machine) -> PathBuf {
    seed_folder(context, machine).join("cidata.iso")
}

pub fn password_file(context: &Context, machine: &Machine) -> PathBuf {
    seed_folder(context, machine).join("desk-password")
}

fn output(command: &mut Command) -> Result<String> {
    let result = command.output()?;
    if !result.status.success() {
        let said = String::from_utf8_lossy(&result.stderr);
        return Err(Failure(format!("{:?} failed: {}", command.get_program(), said.trim())));
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

fn ssh_key(folder: &Path) -> Result<String> {
    fs::create_dir_all(folder)?;
    let key = folder.join("id");
    if !key.is_file() {
        output(
            Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-C", "nvfwd", "-f"]).arg(&key),
        )?;
    }
    Ok(fs::read_to_string(folder.join("id.pub"))?.trim().to_owned())
}

fn password_hash(file: &Path) -> Result<String> {
    if !file.is_file() {
        let password = output(Command::new("openssl").args(["rand", "-base64", PASSWORD_BYTES]))?;
        let mut new = fs::OpenOptions::new().write(true).create_new(true).mode(PRIVATE).open(file)?;
        writeln!(new, "{password}")?;
    }
    let mut child = Command::new("openssl")
        .args(["passwd", "-6", "-stdin"])
        .stdin(Stdio::from(fs::File::open(file)?))
        .stdout(Stdio::piped())
        .spawn()?;
    let mut hash = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        std::io::Read::read_to_string(&mut stdout, &mut hash)?;
    }
    child.wait()?;
    Ok(hash.trim().to_owned())
}

/// The answers, with this machine's secrets and the kernel on its ISO.
pub fn fill_template(template: &str, hash: &str, key: &str, kernel_package: &str) -> String {
    let filled = template.replace("@PASSWORD_HASH@", hash).replace("@SSH_KEY@", key);
    filled.replace("@KERNEL_PACKAGE@", kernel_package)
}

fn make_payload(sources: &Path, folder: &Path) -> Result<PathBuf> {
    let payload = folder.join("payload");
    let _ = fs::remove_dir_all(&payload);
    copy_tree(&sources.join("setup/guest"), &payload)?;
    fs::set_permissions(&payload, fs::Permissions::from_mode(OPEN_FOLDER))?;
    // These two are needed before any driver image has been seen.
    for name in CAST_FILES {
        let file = Path::new(name).file_name().unwrap_or_default();
        fs::copy(sources.join(name), payload.join(file))?;
    }
    let archive = folder.join("nvfwd-guest.tar");
    output(
        Command::new("tar")
            .args(["--owner=root", "--group=root", "-cf"])
            .arg(&archive)
            .arg("-C")
            .arg(&payload)
            .arg("."),
    )?;
    let _ = fs::remove_dir_all(&payload);
    Ok(archive)
}

/// Writes the seed disc, and returns where it is.
pub fn make_seed(context: &Context, machine: &Machine, kernel_package: &str) -> Result<PathBuf> {
    let sources = guest_sources(context);
    let folder = seed_folder(context, machine);
    fs::create_dir_all(&folder)?;
    let key = ssh_key(&machine.folder(context).join("ssh"))?;
    let hash = password_hash(&password_file(context, machine))?;
    let template = fs::read_to_string(sources.join("setup/seed/user-data.template"))?;
    let user_data = folder.join("user-data");
    fs::write(&user_data, fill_template(&template, &hash, &key, kernel_package) + "\n")?;
    let meta_data = folder.join("meta-data");
    fs::copy(sources.join("setup/seed/meta-data"), &meta_data)?;
    let archive = make_payload(&sources, &folder)?;
    let disc = seed_disc(context, machine);
    output(
        Command::new(iso_maker())
            .args(["-quiet", "-r", "-V", "cidata", "-o"])
            .arg(&disc)
            .arg("-graft-points")
            .arg(format!("user-data={}", user_data.display()))
            .arg(format!("meta-data={}", meta_data.display()))
            .arg(format!("nvfwd-guest.tar={}", archive.display())),
    )?;
    let _ = fs::remove_file(archive);
    Ok(disc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Kind;
    use crate::testing::{Scratch, scratch_context};
    use std::os::unix::fs::MetadataExt;

    fn with_sources(scratch: &Scratch) {
        let template = "password: \"@PASSWORD_HASH@\"\nkey: \"@SSH_KEY@\"\nkernel: \"@KERNEL_PACKAGE@\"\n";
        scratch.write("program/linux/setup/seed/user-data.template", template);
        scratch.write("program/linux/setup/seed/meta-data", "instance-id: nvfwd-1\n");
        scratch.write("program/linux/setup/guest/first-boot.sh", "#!/bin/sh\n");
        scratch.write("program/linux/cast/Xwayland", "#!/bin/sh\n");
        scratch.write("program/linux/cast/firefox.desktop", "[Desktop Entry]\n");
    }

    fn machine(context: &Context) -> Machine {
        let machine = Machine { kind: Kind::Linux, ..Machine::new("ubuntu", "Ubuntu", "/d", "") };
        fs::create_dir_all(machine.folder(context)).unwrap();
        machine
    }

    fn on_disc(disc: &Path, name: &str) -> Vec<u8> {
        Command::new("isoinfo").args(["-R", "-x", name, "-i"]).arg(disc).output().unwrap().stdout
    }

    #[test]
    fn every_mark_in_the_template_is_filled() {
        let filled = fill_template("@PASSWORD_HASH@ @SSH_KEY@ @KERNEL_PACKAGE@ @SSH_KEY@", "h", "k", "p");
        assert_eq!(filled, "h k p k");
    }

    #[test]
    fn the_disc_holds_the_answers_and_the_first_boot_scripts() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        with_sources(&scratch);
        let vm = machine(&context);
        let disc = make_seed(&context, &vm, "linux-image-7.0.0-38-generic").unwrap();
        let answers = String::from_utf8(on_disc(&disc, "/user-data")).unwrap();
        assert!(answers.contains("linux-image-7.0.0-38-generic") && answers.contains("ssh-ed25519"));
        assert!(answers.contains("password: \"$6$"), "only the hash goes in");
        let tar = scratch.path("guest.tar");
        fs::write(&tar, on_disc(&disc, "/nvfwd-guest.tar")).unwrap();
        let listing = Command::new("tar").args(["-tf"]).arg(&tar).output().unwrap().stdout;
        let listing = String::from_utf8_lossy(&listing);
        assert!(listing.contains("./first-boot.sh") && listing.contains("./Xwayland"));
    }

    #[test]
    fn secrets_are_private_and_stay_as_they_were() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        with_sources(&scratch);
        let vm = machine(&context);
        make_seed(&context, &vm, "linux-image-7.0.0-38-generic").unwrap();
        let file = password_file(&context, &vm);
        let password = fs::read_to_string(&file).unwrap();
        let key = fs::read_to_string(vm.folder(&context).join("ssh/id.pub")).unwrap();
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, PRIVATE);
        make_seed(&context, &vm, "linux-image-7.0.0-38-generic").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), password);
        assert_eq!(fs::read_to_string(vm.folder(&context).join("ssh/id.pub")).unwrap(), key);
    }
}
