//! Ubuntu guests. Their route is its own: Ubuntu's installer runs with no hands, and the host's
//! driver reaches the guest on a read-only image, with the calls forwarded through a QEMU device. If
//! anything about that is in doubt, the GPU stays off and the VM still boots.

pub mod driver_files;
pub mod driver_host;
pub mod driver_image;
pub mod driver_versions;
pub mod forwarding;
pub mod seed;
mod tables;

use crate::context::Context;
use std::path::PathBuf;

/// What ships with the app for these guests, inside the program folder.
pub fn guest_sources(context: &Context) -> PathBuf {
    context.paths.helper("linux")
}
