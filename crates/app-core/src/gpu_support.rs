//! What this install can do with the GPU. The app runs without the program folder, and then plain VMs
//! are what it has. A daemon alone says nothing, as every build of this repo makes one.

use crate::context::Context;
use crate::linux::layout::SEED_FOLDER;
use crate::linux::{guest_sources, sensor_daemon};
use crate::machine::Kind;

/// Said wherever a GPU machine would have been offered.
pub const NO_GPU_YET: &str = "GPU machines aren't available yet. The part that talks to the card isn't public, so this build runs plain VMs.";

/// Said when an Ubuntu disc can't install by itself here.
pub const NO_UBUNTU_INSTALL_YET: &str =
    "The files that install Ubuntu by itself aren't public yet, so this build treats it like any other disc.";
const UBUNTU_WITH_GPU: &str =
    "Installs by itself. Its GPU is a preview, and if the GPU can't start it boots with a plain screen.";
const UBUNTU_WITHOUT_GPU: &str =
    "Installs by itself, and boots with a plain screen. This QEMU has no GPU device for it.";

pub fn gpu_ready(context: &Context) -> bool {
    context.paths.guest_files().is_dir()
}

fn ubuntu_install_ready(context: &Context) -> bool {
    guest_sources(context).join(SEED_FOLDER).is_dir()
}

/// What an Ubuntu machine gets here, which depends on the QEMU.
pub fn ubuntu_note(context: &Context) -> &'static str {
    if sensor_daemon(context).exists() { UBUNTU_WITH_GPU } else { UBUNTU_WITHOUT_GPU }
}

/// Why a disc of this kind is only a plain VM here, if it is.
pub fn plain_reason(context: &Context, disc: Kind) -> &'static str {
    match disc {
        Kind::Windows if !gpu_ready(context) => NO_GPU_YET,
        Kind::Linux if !ubuntu_install_ready(context) => NO_UBUNTU_INSTALL_YET,
        _ => "",
    }
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
    fn each_kind_of_disc_says_what_it_lacks() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(plain_reason(&context, Kind::Windows), "");
        assert_eq!(plain_reason(&context, Kind::Linux), NO_UBUNTU_INSTALL_YET);
        assert_eq!(plain_reason(&context, Kind::Plain), "");
        fs::remove_dir_all(context.paths.guest_files()).unwrap();
        assert_eq!(plain_reason(&context, Kind::Windows), NO_GPU_YET);
    }

    #[test]
    fn ubuntu_says_whether_this_qemu_has_its_gpu() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.paths.qemu = scratch.path("bin/qemu");
        assert_eq!(ubuntu_note(&context), UBUNTU_WITHOUT_GPU);
        scratch.write("bin/nvfwd-sensord", "");
        assert_eq!(ubuntu_note(&context), UBUNTU_WITH_GPU);
    }

    #[test]
    fn the_program_folder_brings_what_it_holds() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert!(gpu_ready(&context));
        assert_eq!(kind_offered(&context, Kind::Windows), Kind::Windows);
        assert_eq!(kind_offered(&context, Kind::Linux), Kind::Plain, "Ubuntu's setup files aren't here");
        fs::create_dir_all(guest_sources(&context).join(SEED_FOLDER)).unwrap();
        assert_eq!(kind_offered(&context, Kind::Linux), Kind::Linux);
    }
}
