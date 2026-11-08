//! What has to be true of this PC for QEMU to start with the forwarded GPU. Each of these would stop
//! QEMU dead, and that would cost the VM, so they're asked first and cost only the GPU. Its jail is
//! asked about with the rest of the start, as for Windows: see `jailing`.

use super::gpu_off::{GUEST_NO_ALLOW_LIST, GUEST_NO_DEVICE, GpuOff};
use std::path::Path;
use std::process::{Command, Stdio};

const CHECK_SECONDS: &str = "10";
/// QEMU exits with 0 either way. It only lists options on stdout for a device it knows.
const DEVICE_QUESTION: &str = "nvfwd,help";
const DEVICE_ANSWER: &str = "nvfwd options:";
/// The namespaces the jail uses, which a PC can refuse, with nothing else in the way.
const SANDBOX_QUESTION: [&str; 11] = [
    "--unshare-user",
    "--unshare-ipc",
    "--unshare-uts",
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
    let mut question = Command::new("timeout");
    question.arg(CHECK_SECONDS).arg(qemu).args(["-device", DEVICE_QUESTION]).stdin(Stdio::null());
    let said = question.output().map(|answer| String::from_utf8_lossy(&answer.stdout).into_owned());
    said.is_ok_and(|text| text.contains(DEVICE_ANSWER))
}

/// Whether bubblewrap can make namespaces here. Ubuntu 24.04 can refuse it, through AppArmor.
pub fn sandbox_runs() -> bool {
    answers_yes(Command::new("timeout").arg(CHECK_SECONDS).arg("bwrap").args(SANDBOX_QUESTION))
}

/// Says why QEMU can't start with the forwarded GPU, or `None` if it can.
pub fn forwarding_problem(qemu: &Path, allow_list: &Path) -> Option<GpuOff> {
    if !has_device(qemu) {
        let detail =
            format!("{} has no nvfwd device. It needs a build with the device added.", qemu.display());
        return Some(GpuOff::new(GUEST_NO_DEVICE, detail));
    }
    (!allow_list.exists()).then(|| {
        let detail = format!("{} is missing, so no call could reach the host.", allow_list.display());
        GpuOff::new(GUEST_NO_ALLOW_LIST, detail)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    /// As the real one does, it exits with 0 whether or not it knows the device.
    fn fake_qemu(scratch: &Scratch, answer: &str) -> std::path::PathBuf {
        scratch.program("qemu", &format!("#!/bin/sh\necho '{answer}'\nexit 0\n"))
    }

    #[test]
    fn a_qemu_with_no_device_is_named() {
        let scratch = Scratch::new();
        let said = forwarding_problem(&fake_qemu(&scratch, ""), Path::new("/nothing")).unwrap();
        assert_eq!(said.for_guest, GUEST_NO_DEVICE);
    }

    #[test]
    fn a_qemu_that_is_not_there_counts_as_having_no_device() {
        let said = forwarding_problem(Path::new("/nothing/qemu"), Path::new("/nothing")).unwrap();
        assert_eq!(said.for_guest, GUEST_NO_DEVICE);
    }

    #[test]
    fn a_missing_allow_list_is_named() {
        let scratch = Scratch::new();
        let said =
            forwarding_problem(&fake_qemu(&scratch, DEVICE_ANSWER), &scratch.path("allow.txt")).unwrap();
        assert!(said.detail.contains("allow.txt"));
        assert!(!said.guest_text().contains('/'));
    }
}
