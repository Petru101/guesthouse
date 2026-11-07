//! What this kernel can do for a jail, asked without changing anything.

use crate::landlock;
use std::fs;
use std::io;

const OWN_THREADS: &str = "/proc/self/task";

/// What the running kernel offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Support {
    pub landlock_abi: u32,
    pub seccomp: bool,
}

impl Support {
    pub fn probe() -> Self {
        // SAFETY: this only reads this thread's seccomp mode. A kernel without seccomp says EINVAL.
        let seccomp = unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } >= 0;
        Self { landlock_abi: landlock::abi(), seccomp }
    }

    /// Why a jail can't be had here, if it can't. These are the parts that need no namespaces.
    pub fn refusal(&self) -> Option<&'static str> {
        if self.landlock_abi == 0 {
            return Some(
                "this kernel has no Landlock, or it's turned off. Landlock is in the kernels of Fedora, Ubuntu \
                 24.04 and Debian 13, and lsm= on the kernel's command line can leave it out.",
            );
        }
        (!self.seccomp).then_some("this kernel has no seccomp filters.")
    }
}

/// Landlock covers only the thread that applies it and what that thread starts later. This says
/// whether this process has just the one thread, so a jail can't leave one out.
pub fn only_thread() -> io::Result<bool> {
    Ok(fs::read_dir(OWN_THREADS)?.count() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_landlock_is_a_refusal_and_its_reason_says_so() {
        let support = Support { landlock_abi: 0, seccomp: true };
        assert!(support.refusal().unwrap().contains("Landlock"));
        assert_eq!(Support { landlock_abi: 4, seccomp: true }.refusal(), None);
        assert!(Support { landlock_abi: 4, seccomp: false }.refusal().is_some());
    }
}
