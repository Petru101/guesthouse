//! Landlock's three system calls, which libc names but doesn't wrap.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::ptr;

const CREATE_RULESET_VERSION: libc::c_uint = 1 << 0;
const RULE_PATH_BENEATH: libc::c_int = 1;
const RULE_NET_PORT: libc::c_int = 2;

/// The first three fields of the kernel's struct. An older kernel takes a longer struct as long as
/// what it doesn't know is zero, and a newer one takes this shorter one.
#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[repr(C)]
struct NetPortAttr {
    allowed_access: u64,
    port: u64,
}

fn checked(result: libc::c_long) -> io::Result<libc::c_long> {
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

/// The kernel's Landlock ABI, or 0 if it has none or it's turned off.
pub fn abi() -> u32 {
    // SAFETY: with no attributes and this flag, the call only reads nothing and returns a number.
    let version = unsafe {
        libc::syscall(libc::SYS_landlock_create_ruleset, ptr::null::<u8>(), 0, CREATE_RULESET_VERSION)
    };
    u32::try_from(version).unwrap_or(0)
}

pub fn create_ruleset(fs: u64, net: u64, scoped: u64) -> io::Result<OwnedFd> {
    let attr = RulesetAttr { handled_access_fs: fs, handled_access_net: net, scoped };
    // SAFETY: the pointer and size are this struct's, which lives across the call.
    let fd = checked(unsafe {
        libc::syscall(libc::SYS_landlock_create_ruleset, &raw const attr, size_of::<RulesetAttr>(), 0)
    })?;
    // SAFETY: the kernel has just made this descriptor for us, and nothing else holds it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

pub fn add_path_rule(ruleset: &OwnedFd, path: BorrowedFd, access: u64) -> io::Result<()> {
    let attr = PathBeneathAttr { allowed_access: access, parent_fd: path.as_raw_fd() };
    // SAFETY: both descriptors are open for the call, and the struct lives across it.
    checked(unsafe {
        libc::syscall(libc::SYS_landlock_add_rule, ruleset.as_raw_fd(), RULE_PATH_BENEATH, &raw const attr, 0)
    })?;
    Ok(())
}

pub fn add_port_rule(ruleset: &OwnedFd, port: u16, access: u64) -> io::Result<()> {
    let attr = NetPortAttr { allowed_access: access, port: u64::from(port) };
    // SAFETY: the descriptor is open for the call, and the struct lives across it.
    checked(unsafe {
        libc::syscall(libc::SYS_landlock_add_rule, ruleset.as_raw_fd(), RULE_NET_PORT, &raw const attr, 0)
    })?;
    Ok(())
}

pub fn restrict_self(ruleset: &OwnedFd) -> io::Result<()> {
    // SAFETY: the descriptor is open for the call.
    checked(unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset.as_raw_fd(), 0) })?;
    Ok(())
}
