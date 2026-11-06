//! What this install can do with the GPU. The app runs without the program folder, and then plain VMs
//! are what it has. A daemon alone says nothing, as every build of this repo makes one, and neither
//! do the guest's files, which the repo has too. So a build of the repo also needs a QEMU with the patch.

use crate::context::Context;
use crate::linux::guest_sources;
use crate::machine::Kind;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// Said wherever a GPU machine would have been offered.
pub const NO_GPU_YET: &str = "GPU machines aren't available yet. The part that talks to the card isn't public, so this build runs plain VMs.";
const UBUNTU_SETUP: &str = "setup/seed";
/// What only the patched QEMU has.
const PATCHED_QEMU_DEVICE: &str = "gpupv-render";

pub fn gpu_ready(context: &Context) -> bool {
    let paths = &context.paths;
    paths.guest_files().is_dir() && (paths.guest_files_in_program() || qemu_is_patched(&paths.qemu))
}

/// QEMU's answer doesn't change while the app runs, and this is asked often.
fn qemu_is_patched(qemu: &Path) -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(|| qemu_lists_our_device(qemu))
}

fn qemu_lists_our_device(qemu: &Path) -> bool {
    let listing = Command::new(qemu).args(["-device", "help"]).output();
    listing.is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(PATCHED_QEMU_DEVICE))
}

fn ubuntu_install_ready(context: &Context) -> bool {
    guest_sources(context).join(UBUNTU_SETUP).is_dir()
}

/// What a disc can become here. Without the files that install it by itself, it's a plain VM that the
/// user installs by hand.
pub fn kind_offered(context: &Context, kind: Kind) -> Kind {
    match kind {
        Kind::Windows if !gpu_ready(context) => Kind::Plain,
        Kind::Linux if !ubuntu_install_ready(context) => Kind::Plain,
        kind => kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_the_patched_qemu_lists_our_device() {
        let scratch = Scratch::new();
        let plain = scratch.write("plain", "#!/bin/sh\necho 'name \"virtio-net-pci\"'\n");
        let patched = scratch.write("patched", "#!/bin/sh\necho 'name \"gpupv-render\"'\n");
        for file in [&plain, &patched] {
            fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(!qemu_lists_our_device(&plain));
        assert!(qemu_lists_our_device(&patched));
        assert!(!qemu_lists_our_device(&scratch.path("no-such-qemu")));
    }

    #[test]
    fn with_no_program_folder_everything_is_a_plain_vm() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fs::remove_dir_all(context.paths.guest_files()).unwrap();
        assert!(!gpu_ready(&context));
        assert_eq!(kind_offered(&context, Kind::Windows), Kind::Plain);
        assert_eq!(kind_offered(&context, Kind::Linux), Kind::Plain);
        assert_eq!(kind_offered(&context, Kind::Plain), Kind::Plain);
    }

    #[test]
    fn the_program_folder_brings_what_it_holds() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert!(gpu_ready(&context));
        assert_eq!(kind_offered(&context, Kind::Windows), Kind::Windows);
        assert_eq!(kind_offered(&context, Kind::Linux), Kind::Plain, "Ubuntu's setup files aren't here");
        fs::create_dir_all(guest_sources(&context).join(UBUNTU_SETUP)).unwrap();
        assert_eq!(kind_offered(&context, Kind::Linux), Kind::Linux);
    }
}
