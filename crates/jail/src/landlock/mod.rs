//! Landlock: which paths a process may reach and how, and which ports it may use.
//!
//! A ruleset handles every right the kernel knows, so whatever a plan doesn't grant is refused. The
//! only exception is the network: a kind of use the plan leaves as [`Ports::Any`] isn't handled at
//! all. A kernel with an older ABI handles fewer rights, and those it doesn't know stay open.

mod rights;
mod sys;

pub use rights::NEWEST_KNOWN_ABI;
pub(crate) use rights::missing_at;

use crate::{Applied, Grant, Plan, Ports, no_new_privileges};
use std::fs::OpenOptions;
use std::io::{self, ErrorKind};
use std::os::fd::AsFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The kernel's Landlock ABI, or 0 if it has none.
pub fn abi() -> u32 {
    sys::abi()
}

/// The rights for one path, cut down to what the kernel knows and to what a file can take.
fn rights_for(grant: Grant, is_folder: bool, handled: u64) -> u64 {
    let rights = rights::of_grant(grant) & handled;
    if is_folder { rights } else { rights & rights::FILE_RIGHTS }
}

/// A path that isn't there is left out: nothing can be opened there anyway. One that can't be opened
/// for some other reason stops the jail, as the plan would be wrong.
fn add_path(ruleset: &std::os::fd::OwnedFd, grant: Grant, path: &Path, handled: u64) -> io::Result<()> {
    let opened = OpenOptions::new().read(true).custom_flags(libc::O_PATH | libc::O_CLOEXEC).open(path);
    let file = match opened {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        opened => {
            opened.map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))?
        }
    };
    let rights = rights_for(grant, file.metadata()?.is_dir(), handled);
    if rights == 0 {
        return Ok(());
    }
    sys::add_path_rule(ruleset, file.as_fd(), rights)
        .map_err(|error| io::Error::new(error.kind(), format!("a rule for {}: {error}", path.display())))
}

/// The network rights to handle, and the port rules that let some of them through.
fn network(plan: &Plan, abi: u32) -> (u64, Vec<(u16, u64)>) {
    let network = &plan.network;
    let kinds = [
        (&network.tcp_bind, rights::BIND_TCP),
        (&network.tcp_connect, rights::CONNECT_TCP),
        (&network.udp_bind, rights::BIND_UDP),
        (&network.udp_send, rights::SEND_UDP),
    ];
    let known = rights::net_rights(abi);
    let mut handled = 0;
    let mut ports = Vec::new();
    for (wanted, right) in kinds {
        let Ports::Only(allowed) = wanted else { continue };
        if known & right == 0 {
            continue;
        }
        handled |= right;
        ports.extend(allowed.iter().map(|port| (*port, right)));
    }
    (handled, ports)
}

/// Puts this thread, and what it starts from now on, under the plan. Threads that already exist
/// aren't covered, so call this before there are any: see [`crate::only_thread`].
pub fn apply(plan: &Plan) -> io::Result<Applied> {
    let abi = abi();
    if abi == 0 {
        return Err(io::Error::new(ErrorKind::Unsupported, "this kernel has no Landlock"));
    }
    no_new_privileges()?;
    let handled_fs = rights::fs_rights(abi);
    let (handled_net, ports) = network(plan, abi);
    let ruleset = sys::create_ruleset(handled_fs, handled_net, rights::scopes(abi))?;
    for (grant, path) in &plan.paths {
        add_path(&ruleset, *grant, path, handled_fs)?;
    }
    for (port, right) in ports {
        sys::add_port_rule(&ruleset, port, right)?;
    }
    sys::restrict_self(&ruleset)?;
    Ok(Applied { landlock_abi: abi })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Network;

    #[test]
    fn a_file_gets_only_what_a_file_can_take() {
        let all = rights::fs_rights(NEWEST_KNOWN_ABI);
        assert_eq!(
            rights_for(Grant::Own, false, all),
            rights::READ_FILE | rights::WRITE_FILE | rights::TRUNCATE | rights::RESOLVE_UNIX
        );
        assert_eq!(
            rights_for(Grant::Device, false, rights::fs_rights(4)),
            rights::READ_FILE | rights::WRITE_FILE
        );
        assert_eq!(rights_for(Grant::Socket, false, rights::fs_rights(8)), 0);
    }

    #[test]
    fn open_kinds_of_network_are_not_handled() {
        let plan = Plan {
            paths: Vec::new(),
            network: Network {
                tcp_connect: Ports::Any,
                udp_bind: Ports::Any,
                udp_send: Ports::Any,
                tcp_bind: Ports::Only(vec![80]),
            },
        };
        assert_eq!(network(&plan, NEWEST_KNOWN_ABI), (rights::BIND_TCP, vec![(80, rights::BIND_TCP)]));
        let closed = Plan::default();
        assert_eq!(network(&closed, 4).0, rights::BIND_TCP | rights::CONNECT_TCP);
        assert_eq!(network(&closed, 3).0, 0);
    }
}
