//! A jail a process puts itself in, with nothing but what an unprivileged user has.
//!
//! Three parts, all of which carry across `execve` and can't be undone:
//!
//! - [`landlock`]: which files and folders it may open, and how, and which ports it may use.
//! - [`seccomp`]: which system calls it may make.
//! - no new privileges, which seccomp needs anyway, so a setuid program gains nothing.
//!
//! The daemon applies these to itself after its setup. QEMU gets them from `guesthouse-jail`, which
//! applies them and then runs it. What each of them may do is in [`filters`] and in the caller's
//! [`Plan`]. A kernel that has only some of Landlock's rights gets the ones it has, and [`Applied`]
//! says which.

pub mod filters;
pub mod landlock;
mod plan;
pub mod seccomp;
mod support;

pub use plan::{Grant, Network, Plan, Ports};
pub use support::{Support, only_thread};

use std::io;

/// What a jail really got, for the log and for the machine's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    pub landlock_abi: u32,
}

impl Applied {
    /// The parts a kernel below the newest known ABI leaves out, in words.
    pub fn missing(&self) -> Vec<&'static str> {
        landlock::missing_at(self.landlock_abi)
    }
}

/// Stops this process and everything it starts from gaining privileges, through setuid programs or
/// file capabilities. seccomp needs it before an unprivileged process may install a filter.
pub fn no_new_privileges() -> io::Result<()> {
    // SAFETY: this sets one flag the kernel keeps for this thread, and takes no pointers.
    let done = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if done == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}
