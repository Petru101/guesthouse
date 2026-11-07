//! The two filters: an allow-list for the daemon, and a deny-list for QEMU. Why they differ is in
//! each module.

mod daemon;
mod qemu;

pub use daemon::daemon;
pub use qemu::qemu;

use crate::seccomp::{Action, Rule};

fn all(calls: &[libc::c_long], action: Action) -> impl Iterator<Item = Rule> + '_ {
    calls.iter().map(move |call| Rule::always(*call, action))
}
