//! What has to be true of this PC for QEMU to start with the forwarded GPU. Each of these would stop
//! QEMU or its sandbox dead, and that would cost the VM, so they're asked first and cost only the GPU.

use std::path::Path;
use std::process::{Command, Stdio};

const CHECK_SECONDS: &str = "10";
/// QEMU exits with an error when it doesn't know a device, and lists its options when it does.
const DEVICE_QUESTION: &str = "nvfwd,help";
/// The parts of the jail that a PC can refuse, with nothing else in the way.
const SANDBOX_QUESTION: [&str; 10] = [
    "--unshare-user",
    "--unshare-pid",
    "--ro-bind",
    "/usr",
    "/usr",
    "--symlink",
    "usr/lib64",
    "/lib64",
    "--",
    "/usr/bin/true",
];

fn answers_yes(command: &mut Command) -> bool {
    let quiet = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    quiet.status().is_ok_and(|status| status.success())
}

fn has_device(qemu: &Path) -> bool {
    answers_yes(Command::new("timeout").arg(CHECK_SECONDS).arg(qemu).args(["-device", DEVICE_QUESTION]))
}

fn sandbox_runs() -> bool {
    answers_yes(Command::new("timeout").arg(CHECK_SECONDS).arg("bwrap").args(SANDBOX_QUESTION))
}

/// Says why QEMU can't start with the forwarded GPU, or `None` if it can.
pub fn forwarding_problem(qemu: &Path, allow_list: &Path) -> Option<String> {
    if !has_device(qemu) {
        return Some("This QEMU has no nvfwd device. It needs a build with the device added.".to_owned());
    }
    if !allow_list.exists() {
        return Some(format!("{} is missing, so no call could reach the host.", allow_list.display()));
    }
    (!sandbox_runs()).then(|| {
        "bubblewrap won't run here, and QEMU doesn't get the GPU without its sandbox. Install bubblewrap, \
         and check that this PC allows user namespaces."
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn fake_qemu(scratch: &Scratch, exit_code: u8) -> std::path::PathBuf {
        let qemu = scratch.write("qemu", format!("#!/bin/sh\nexit {exit_code}\n"));
        fs::set_permissions(&qemu, fs::Permissions::from_mode(0o755)).unwrap();
        qemu
    }

    #[test]
    fn a_qemu_with_no_device_is_named() {
        let scratch = Scratch::new();
        let said = forwarding_problem(&fake_qemu(&scratch, 1), Path::new("/nothing")).unwrap();
        assert!(said.contains("no nvfwd device"));
    }

    #[test]
    fn a_qemu_that_is_not_there_counts_as_having_no_device() {
        let said = forwarding_problem(Path::new("/nothing/qemu"), Path::new("/nothing")).unwrap();
        assert!(said.contains("no nvfwd device"));
    }

    #[test]
    fn a_missing_allow_list_is_named() {
        let scratch = Scratch::new();
        let said = forwarding_problem(&fake_qemu(&scratch, 0), &scratch.path("allow.txt")).unwrap();
        assert!(said.contains("allow.txt"));
    }
}
