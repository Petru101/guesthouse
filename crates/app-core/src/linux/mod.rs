//! Ubuntu guests. Their route is its own: Ubuntu's installer runs with no hands, and the host's
//! driver reaches the guest on a read-only image, with the calls forwarded through a QEMU device. If
//! anything about that is in doubt, the GPU stays off and the VM still boots.

pub mod driver_files;
pub mod driver_image;
pub mod driver_versions;
pub mod layout;
pub mod seed;
mod tables;

use crate::context::Context;
use std::path::PathBuf;

const SENSOR_DAEMON: &str = "nvfwd-sensord";

/// The device's helper, which is built with the device and sits beside its QEMU.
pub fn sensor_daemon(context: &Context) -> PathBuf {
    context.paths.qemu.with_file_name(SENSOR_DAEMON)
}

/// The guests' sources, inside the program folder.
pub fn guest_sources(context: &Context) -> PathBuf {
    context.paths.helper(layout::SOURCES_FOLDER)
}
