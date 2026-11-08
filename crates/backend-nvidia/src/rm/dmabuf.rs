//! The requests that turn video memory into a dmabuf another process can read where it lies.
//!
//! It takes three steps. The memory goes into a file the RM knows, that file goes into a GEM object
//! on the card's render node, and the GEM object gives the dmabuf. This is how NVIDIA's own GBM makes
//! them. The layouts are in public headers: the RM's Unix control header, the modeset interface,
//! the DRM driver's ioctl header, and the kernel's `drm.h`.

use crate::profile::HostBranch;
use crate::rm::Handle;
use guesthouse_backend::PciAddress;
use std::fs;
use std::path::{Path, PathBuf};

const DRM_CLASS_FOLDER: &str = "/sys/class/drm";
const DRM_DEVICE_FOLDER: &str = "/dev/dri";
const RENDER_NODE_PREFIX: &str = "renderD";
const DRIVER_LINK: &str = "device/driver";
const DEVICE_LINK: &str = "device";
const DRIVER_NAME: &str = "nvidia";

const IOCTL_WRITE: u64 = 1 << 30;
const IOCTL_READ_WRITE: u64 = 3 << 30;
const IOCTL_SIZE_SHIFT: u32 = 16;
const DRM_IOCTL_MAGIC: u64 = b'd' as u64;
const DRM_GEM_CLOSE: u64 = 0x09;
const DRM_PRIME_HANDLE_TO_FD: u64 = 0x2D;
/// Where a DRM driver's own ioctls begin.
const DRM_COMMAND_BASE: u64 = 0x40;
const DRM_NVIDIA_GEM_IMPORT_MEMORY: u64 = DRM_COMMAND_BASE + 1;
const GEM_CLOSE_SIZE: usize = 8;
const PRIME_SIZE: usize = 12;

/// Says the object being exported is one of the RM's own.
const EXPORT_TYPE_RM_OBJECT: u32 = 1;
const EXPORT_PARAM_TYPE: usize = 0;
const EXPORT_PARAM_DEVICE: usize = 4;
const EXPORT_PARAM_PARENT: usize = 8;
const EXPORT_PARAM_OBJECT: usize = 12;
const EXPORT_PARAM_FD: usize = 16;

/// Rows in order, which is the only layout this backend's ring uses.
const LAYOUT_PITCH: u32 = 1;
const IMPORT_PARAM_FD: usize = 0;
const IMPORT_PARAM_LAYOUT: usize = 4;

const IMPORT_REQUEST_SIZE: usize = 0;
const IMPORT_REQUEST_PARAMS: usize = 8;
const IMPORT_REQUEST_PARAMS_SIZE: usize = 16;

const PRIME_HANDLE: usize = 0;
const PRIME_FLAGS: usize = 4;
const PRIME_FD: usize = 8;
const NO_DESCRIPTOR: i32 = -1;

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn get32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn drm_number(direction: u64, number: u64, size: usize) -> u64 {
    direction | (size as u64) << IOCTL_SIZE_SHIFT | DRM_IOCTL_MAGIC << 8 | number
}

pub fn import_number(sizes: &HostBranch) -> u64 {
    drm_number(IOCTL_READ_WRITE, DRM_NVIDIA_GEM_IMPORT_MEMORY, sizes.gem_import_request)
}

pub fn prime_number() -> u64 {
    drm_number(IOCTL_READ_WRITE, DRM_PRIME_HANDLE_TO_FD, PRIME_SIZE)
}

pub fn gem_close_number() -> u64 {
    drm_number(IOCTL_WRITE, DRM_GEM_CLOSE, GEM_CLOSE_SIZE)
}

/// The control's parameters: this memory goes into the file `fd`.
pub fn export_params(sizes: &HostBranch, device: Handle, memory: Handle, fd: i32) -> Vec<u8> {
    let mut params = vec![0; sizes.export_object_params];
    put32(&mut params, EXPORT_PARAM_TYPE, EXPORT_TYPE_RM_OBJECT);
    put32(&mut params, EXPORT_PARAM_DEVICE, device);
    put32(&mut params, EXPORT_PARAM_PARENT, device);
    put32(&mut params, EXPORT_PARAM_OBJECT, memory);
    put32(&mut params, EXPORT_PARAM_FD, fd as u32);
    params
}

/// What the import reads through a pointer: the file from [`export_params`], and the layout.
pub fn import_params(sizes: &HostBranch, fd: i32) -> Vec<u8> {
    let mut params = vec![0; sizes.gem_import_params];
    put32(&mut params, IMPORT_PARAM_FD, fd as u32);
    put32(&mut params, IMPORT_PARAM_LAYOUT, LAYOUT_PITCH);
    params
}

pub fn import_request(sizes: &HostBranch, size: u64, params: &[u8]) -> Vec<u8> {
    let mut request = vec![0; sizes.gem_import_request];
    put64(&mut request, IMPORT_REQUEST_SIZE, size);
    put64(&mut request, IMPORT_REQUEST_PARAMS, params.as_ptr() as u64);
    put64(&mut request, IMPORT_REQUEST_PARAMS_SIZE, params.len() as u64);
    request
}

pub fn imported_handle(sizes: &HostBranch, request: &[u8]) -> u32 {
    get32(request, sizes.gem_import_request_handle)
}

pub fn prime_request(handle: u32) -> [u8; PRIME_SIZE] {
    let mut request = [0; PRIME_SIZE];
    put32(&mut request, PRIME_HANDLE, handle);
    put32(&mut request, PRIME_FLAGS, (libc::O_CLOEXEC | libc::O_RDWR) as u32);
    put32(&mut request, PRIME_FD, NO_DESCRIPTOR as u32);
    request
}

pub fn prime_descriptor(request: &[u8]) -> i32 {
    get32(request, PRIME_FD) as i32
}

pub fn gem_close_request(handle: u32) -> [u8; GEM_CLOSE_SIZE] {
    let mut request = [0; GEM_CLOSE_SIZE];
    put32(&mut request, 0, handle);
    request
}

/// The render node of the card at this address. It's found by its device, as the node numbers change
/// with what else is in the machine. With no address it's the first card this driver owns, which is
/// only sure to be the right one on a PC with one NVIDIA card.
pub fn render_node(address: Option<&PciAddress>) -> Option<PathBuf> {
    render_node_in(Path::new(DRM_CLASS_FOLDER), address)
}

fn render_node_in(class_folder: &Path, address: Option<&PciAddress>) -> Option<PathBuf> {
    let mut names: Vec<_> = fs::read_dir(class_folder)
        .ok()?
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with(RENDER_NODE_PREFIX))
        .collect();
    names.sort();
    let driven_here = |name: &&std::ffi::OsString| {
        let driver = fs::canonicalize(class_folder.join(name).join(DRIVER_LINK));
        driver.is_ok_and(|path| path.file_name().is_some_and(|driver| driver == DRIVER_NAME))
    };
    // The node's device link ends in the PCI address of the card it belongs to.
    let at_address = |name: &&std::ffi::OsString| {
        let Some(wanted) = address else { return true };
        let device = fs::canonicalize(class_folder.join(name).join(DEVICE_LINK));
        let found = device.ok().and_then(|path| PciAddress::parse(&path.file_name()?.to_string_lossy()));
        found.as_ref() == Some(wanted)
    };
    names.iter().filter(at_address).find(driven_here).map(|name| Path::new(DRM_DEVICE_FOLDER).join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::HOST_615;
    use std::os::unix::fs::symlink;
    use std::{env, process};

    #[test]
    fn request_numbers_follow_the_kernels_encoding() {
        assert_eq!(import_number(&HOST_615), 0xC020_6441);
        assert_eq!(prime_number(), 0xC00C_642D);
        assert_eq!(gem_close_number(), 0x4008_6409);
    }

    #[test]
    fn the_export_names_the_memory_under_its_device() {
        let params = export_params(&HOST_615, 0xC000_0001, 0xC000_1234, 9);
        assert_eq!(params.len(), HOST_615.export_object_params);
        assert_eq!(get32(&params, 0), 1);
        assert_eq!(get32(&params, 4), 0xC000_0001);
        assert_eq!(get32(&params, 8), 0xC000_0001);
        assert_eq!(get32(&params, 12), 0xC000_1234);
        assert_eq!(get32(&params, 16), 9);
        assert_eq!(get32(&params, 20), 0, "no flags");
    }

    #[test]
    fn the_import_points_at_its_parameters_and_wants_rows_in_order() {
        let params = import_params(&HOST_615, 7);
        assert_eq!(params.len(), HOST_615.gem_import_params);
        assert_eq!((get32(&params, 0), get32(&params, 4)), (7, LAYOUT_PITCH));
        let mut request = import_request(&HOST_615, 0x20000, &params);
        assert_eq!(request.len(), HOST_615.gem_import_request);
        assert_eq!(request[..8], 0x20000u64.to_le_bytes());
        assert_eq!(request[8..16], (params.as_ptr() as u64).to_le_bytes());
        assert_eq!(request[16..24], (params.len() as u64).to_le_bytes());
        request[24] = 5;
        assert_eq!(imported_handle(&HOST_615, &request), 5);
    }

    #[test]
    fn the_dmabuf_is_asked_for_read_write_and_not_inherited() {
        let request = prime_request(3);
        assert_eq!(get32(&request, 0), 3);
        assert_eq!(get32(&request, 4) as i32, libc::O_CLOEXEC | libc::O_RDWR);
        assert_eq!(prime_descriptor(&request), NO_DESCRIPTOR);
    }

    /// A DRM class folder whose nodes link to PCI devices named by address, as sysfs does.
    fn class_folder(name: &str, nodes: &[(&str, &str, &str)]) -> PathBuf {
        let class = env::temp_dir().join(format!("guesthouse-drm-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&class);
        for (node, address, driver) in nodes {
            let device = class.join("devices").join(address);
            fs::create_dir_all(&device).unwrap();
            let target = class.join("drivers").join(driver);
            fs::create_dir_all(&target).unwrap();
            let _ = symlink(&target, device.join("driver"));
            fs::create_dir_all(class.join(node)).unwrap();
            symlink(&device, class.join(node).join("device")).unwrap();
        }
        class
    }

    #[test]
    fn the_render_node_is_the_one_this_driver_owns() {
        let nodes = [("renderD128", "0000:7c:00.0", "amdgpu"), ("renderD129", "0000:02:00.0", "nvidia")];
        let class = class_folder("one", &[nodes[0], nodes[1], ("card0", "0000:02:00.0", "nvidia")]);
        assert_eq!(render_node_in(&class, None), Some(PathBuf::from("/dev/dri/renderD129")));
        let address = PciAddress::parse("0000:02:00.0").unwrap();
        assert_eq!(render_node_in(&class, Some(&address)), Some(PathBuf::from("/dev/dri/renderD129")));
        fs::remove_dir_all(class).unwrap();
    }

    #[test]
    fn of_two_cards_the_render_node_is_the_one_at_the_address() {
        let class = class_folder(
            "two",
            &[("renderD128", "0000:01:00.0", "nvidia"), ("renderD129", "0000:02:00.0", "nvidia")],
        );
        let second = PciAddress::parse("0000:02:00.0").unwrap();
        assert_eq!(render_node_in(&class, Some(&second)), Some(PathBuf::from("/dev/dri/renderD129")));
        let gone = PciAddress::parse("0000:03:00.0").unwrap();
        assert_eq!(render_node_in(&class, Some(&gone)), None);
        fs::remove_dir_all(class).unwrap();
    }
}
