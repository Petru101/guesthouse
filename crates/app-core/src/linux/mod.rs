//! Ubuntu guests. Their route is its own: Ubuntu's installer runs with no hands, and the host's
//! driver reaches the guest on a read-only image, with the calls forwarded through a QEMU device. If
//! anything about that is in doubt, the GPU stays off and the VM still boots.

pub mod driver_files;
pub mod driver_image;
pub mod driver_versions;
