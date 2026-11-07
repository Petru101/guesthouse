//! The jail around a GPU machine's QEMU: what `guesthouse-jail` lets it reach, and bubblewrap's
//! namespaces around that where the PC allows them.
//!
//! The plan names this VM's files one by one, so the machine's folder, with its token and settings,
//! stays out, and so does every other VM. What the network may do follows the machine's mode: NAT
//! is the internet for the guest, so only binds are held to the forwarded ports.

use super::arguments::{MONITOR_SOCKETS, words};
use super::{QemuJail, VmPlan};
use crate::network::{Mode, Protocol, bridge_helper};
use guesthouse_jail::launch::{Launch, Tap, UnixSocket};
use guesthouse_jail::{Grant, Network, Plan, Ports};
use std::path::{Path, PathBuf};

/// Where the launcher leaves QEMU's unix sockets, when it opens them: monitors first, then clients.
const FIRST_UNIX_FD: i32 = 20;
const CLIENT_SOCKETS: [(&str, &str); 3] = [("gpg", "global.sock"), ("gpv", "vgpu.sock"), ("tpm", "tpm.sock")];
const RECONNECT_MS: u32 = 500;
/// QEMU's pid file, which names the run folder in its words when no monitor socket does.
pub const PID_FILE: &str = "qemu.pid";

/// Where a bridged network's device is open for QEMU. Out of the way of what QEMU opens itself.
pub const TAP_FD: i32 = 10;
const DNS_PORT: u16 = 53;
/// Any free port, which sending a datagram binds first.
const EPHEMERAL_PORT: u16 = 0;
/// QEMU's own filter, on top. Starting programs stays allowed, for passt and the shared folder's.
/// Its privileges group stays out: it ends QEMU's user network's child at `setsid`, before the
/// shared folder's program runs, and no new privileges, which the launcher sets, already covers it.
const QEMU_SANDBOX: &str = "on,obsolete=deny,spawn=allow,resourcecontrol=deny";

/// What any QEMU reads. `/proc` and `/sys` give nothing away, as Landlock doesn't let a jailed
/// process look into one outside its jail.
const SYSTEM: [(Grant, &str); 11] = [
    (Grant::Run, "/usr"),
    (Grant::Read, "/etc/ld.so.cache"),
    (Grant::Read, "/etc/nsswitch.conf"),
    (Grant::Read, "/etc/passwd"),
    (Grant::Read, "/etc/group"),
    (Grant::Read, "/etc/hosts"),
    (Grant::Read, "/etc/resolv.conf"),
    (Grant::Read, "/etc/qemu"),
    (Grant::Read, "/run/systemd/resolve"),
    (Grant::Read, "/sys"),
    (Grant::Read, "/proc"),
];
const DEVICES: [&str; 5] = ["/dev/kvm", "/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"];
/// What bubblewrap's namespaces start from, before this VM's own files go in. The network stays, for
/// QEMU's user network. `--new-session` is left out so the app can still stop QEMU with its process
/// group, and the filter refuses typing into a terminal instead. The pid namespace is left out too:
/// in one, the viewer's pid reads as 0, and QEMU's D-Bus display then refuses to let it in. Landlock
/// still keeps QEMU from tracing what's outside its jail, and from signalling it where it has scopes.
const NAMESPACES: &str = "bwrap --unshare-user --unshare-ipc --unshare-uts --unshare-cgroup-try \
    --tmpfs /tmp --ro-bind /usr /usr --symlink usr/lib /lib --symlink usr/lib64 /lib64 --symlink usr/bin /bin \
    --symlink usr/sbin /sbin --proc /proc --dev /dev --ro-bind /sys /sys";
/// Already in the namespaces' base.
const IN_THE_BASE: [&str; 3] = ["/usr", "/proc", "/sys"];
const PATH: &str = "/usr/bin:/usr/sbin";

/// The folder above the one QEMU is in: a QEMU in its build folder finds its firmware and libraries
/// through links to there, and for the system's it's `/usr`.
fn qemu_tree(qemu: &Path) -> PathBuf {
    qemu.parent().and_then(Path::parent).unwrap_or(qemu).to_owned()
}

impl VmPlan {
    fn forwarded_ports(&self, protocol: Protocol) -> Vec<u16> {
        let forwards = self.network.forwards.iter().filter(|forward| forward.protocol == protocol);
        forwards.filter_map(|forward| u16::try_from(forward.host).ok()).collect()
    }

    fn jail_network(&self) -> Network {
        if self.network.mode == Mode::Nat && self.network.passt {
            // passt does the NAT, and it's QEMU's child, so it has this jail too.
            return Network {
                tcp_bind: Ports::Any,
                tcp_connect: Ports::Any,
                udp_bind: Ports::Any,
                udp_send: Ports::Any,
            };
        }
        let mut tcp_bind = vec![self.agent_port];
        tcp_bind.extend(self.forwarded_ports(Protocol::Tcp));
        // The user network reaches the shared folder's program through a pair of loopback TCP
        // sockets it makes for each connection, on a port it can't know beforehand.
        if self.share.is_some() {
            tcp_bind.push(EPHEMERAL_PORT);
        }
        let tcp_bind = Ports::Only(tcp_bind);
        if self.network.mode == Mode::Nat {
            return Network { tcp_bind, tcp_connect: Ports::Any, udp_bind: Ports::Any, udp_send: Ports::Any };
        }
        // A restricted user network only answers forwards, and asks the host's resolver for names.
        let mut udp_bind = vec![EPHEMERAL_PORT];
        udp_bind.extend(self.forwarded_ports(Protocol::Udp));
        let tcp_connect = if self.share.is_some() { Ports::Any } else { Ports::none() };
        Network {
            tcp_bind,
            tcp_connect,
            udp_bind: Ports::Only(udp_bind),
            udp_send: Ports::Only(vec![DNS_PORT]),
        }
    }

    fn firmware_code(&self) -> &Path {
        if self.tpm.is_some() { &self.firmware.code_secure } else { &self.firmware.code }
    }

    /// Every path QEMU may reach, and its network.
    pub fn qemu_jail_plan(&self, jail: &QemuJail) -> Plan {
        let mut plan = Plan { network: self.jail_network(), ..Plan::default() };
        for (grant, path) in SYSTEM {
            plan.grant(grant, path);
        }
        plan.grant(Grant::Run, qemu_tree(&self.qemu));
        if let Some(folder) = &self.qemu_files {
            plan.grant(Grant::Read, folder);
        }
        plan.grant(Grant::Read, self.firmware_code());
        if let Some(disc) = &self.tools_disc {
            plan.grant(Grant::Read, disc);
        }
        for device in DEVICES {
            plan.grant(Grant::Device, device);
        }
        for file in [Path::new(&self.disk), Path::new(&self.vars), &self.ram_file, &self.io_file] {
            plan.grant(Grant::Write, file);
        }
        if let Some(folder) = &self.share {
            plan.grant(Grant::Run, &self.share_program).grant(Grant::Own, folder);
        }
        if let Some(socket) = &jail.sound_socket {
            plan.grant(Grant::Socket, socket);
        }
        plan.grant(Grant::Own, &self.run_folder);
        plan
    }

    fn tap(&self) -> Option<Tap> {
        (self.network.mode == Mode::Bridged).then(|| Tap {
            helper: bridge_helper().into(),
            bridge: self.network.bridge.clone(),
            fd: TAP_FD,
        })
    }

    /// Whether this QEMU runs in bubblewrap's namespaces as well. A bridged network's helper has to
    /// gain its privileges, and inside bubblewrap it can't.
    pub fn in_namespaces(&self, jail: &QemuJail) -> bool {
        jail.namespaces && self.tap().is_none()
    }

    /// bubblewrap's words, with each path of the plan bound at its own place, so the plan means the
    /// same inside.
    fn namespace_words(plan: &Plan, launcher: &Path) -> Vec<String> {
        let mut words: Vec<String> = NAMESPACES.split_whitespace().map(str::to_owned).collect();
        let bind = |kind: &str, path: &Path| {
            [kind.to_owned(), path.display().to_string(), path.display().to_string()]
        };
        words.extend(bind("--ro-bind", launcher));
        for (grant, path) in &plan.paths {
            if IN_THE_BASE.iter().any(|base| path.starts_with(base)) {
                continue;
            }
            let kind = match grant {
                Grant::Read | Grant::Run => "--ro-bind-try",
                Grant::Write | Grant::Own | Grant::Socket => "--bind-try",
                Grant::Device => "--dev-bind-try",
            };
            words.extend(bind(kind, path));
        }
        words.push("--".to_owned());
        words
    }

    /// What goes in front of QEMU for a jailed GPU machine, up to the program's own words.
    pub(super) fn jail_words(&self) -> Vec<String> {
        let Some(jail) = &self.jail else { return Vec::new() };
        let plan = self.qemu_jail_plan(jail);
        let mut words =
            if self.in_namespaces(jail) { Self::namespace_words(&plan, &jail.launcher) } else { Vec::new() };
        let launch = Launch {
            plan,
            own_sandboxes: self.network.passt,
            tap: self.tap(),
            unix_sockets: self.unix_sockets(),
            no_new_unix_sockets: self.passes_unix_sockets(),
            program: Vec::new(),
        };
        words.push(jail.launcher.display().to_string());
        words.extend(launch.words());
        words
    }

    /// The launcher opens every unix socket QEMU needs, and QEMU may make none, so a QEMU that's been
    /// taken over can't reach the session bus or any other socket of the desktop's, whatever the
    /// kernel's Landlock and with or without namespaces. Only with no sound, as PipeWire's client
    /// makes its own socket.
    pub(super) fn passes_unix_sockets(&self) -> bool {
        self.jail.as_ref().is_some() && self.sound.is_none()
    }

    fn client_names(&self) -> impl Iterator<Item = (usize, &'static str, &'static str)> + '_ {
        let tpm = self.tpm.is_some();
        CLIENT_SOCKETS
            .into_iter()
            .enumerate()
            .filter(move |(_, (id, _))| *id != "tpm" || tpm)
            .map(|(at, (id, file))| (at, id, file))
    }

    fn unix_sockets(&self) -> Vec<UnixSocket> {
        if !self.passes_unix_sockets() {
            return Vec::new();
        }
        let monitors = MONITOR_SOCKETS.iter().enumerate().map(|(at, name)| UnixSocket {
            path: self.run_folder.join(name),
            fd: FIRST_UNIX_FD + at as i32,
            listen: true,
        });
        let mut sockets: Vec<UnixSocket> = monitors.collect();
        let first_client = FIRST_UNIX_FD + MONITOR_SOCKETS.len() as i32;
        sockets.extend(self.client_names().map(|(at, _, file)| UnixSocket {
            path: self.run_folder.join(file),
            fd: first_client + at as i32,
            listen: false,
        }));
        sockets
    }

    /// The monitors on the descriptors the launcher opened, and a pid file that names the run folder.
    pub(super) fn monitors_on_descriptors(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        for at in 0..MONITOR_SOCKETS.len() {
            let fd = FIRST_UNIX_FD + at as i32;
            arguments.extend(words(&[
                "-chardev",
                &format!("socket,id=monitor{at},fd={fd},server=on,wait=off"),
                "-mon",
                &format!("chardev=monitor{at},mode=control"),
            ]));
        }
        arguments.extend(words(&["-pidfile", &self.run_folder.join(PID_FILE).display().to_string()]));
        arguments
    }

    /// A socket QEMU connects to: on the launcher's descriptor, or by its path.
    pub(super) fn client_socket(&self, id: &str, file: &str) -> String {
        let first_client = FIRST_UNIX_FD + MONITOR_SOCKETS.len() as i32;
        let on_fd = self.client_names().find(|(_, name, _)| *name == id);
        match on_fd {
            Some((at, _, _)) if self.passes_unix_sockets() => {
                format!("socket,id={id},fd={}", first_client + at as i32)
            }
            _ if id == "tpm" => format!("socket,id={id},path={}", self.run_folder.join(file).display()),
            _ => format!(
                "socket,id={id},path={},reconnect-ms={RECONNECT_MS}",
                self.run_folder.join(file).display()
            ),
        }
    }

    /// QEMU's own filter, if this QEMU has one. passt's sandbox mounts, which the filter refuses.
    pub(super) fn qemu_sandbox_arguments(&self) -> Vec<String> {
        match &self.jail {
            Some(jail) if jail.qemu_sandbox && !self.network.passt => {
                vec!["-sandbox".to_owned(), QEMU_SANDBOX.to_owned()]
            }
            _ => Vec::new(),
        }
    }

    /// A jailed QEMU starts with only the environment it needs, as the user's may hold secrets. Its
    /// temporary files, like the pid file passt is started with, go in its run folder, as the PC's
    /// `/tmp` is out of its reach.
    pub(super) fn jailed_environment(&self, jail: &QemuJail) -> Vec<(String, String)> {
        let mut environment = vec![
            ("PATH".to_owned(), PATH.to_owned()),
            ("TMPDIR".to_owned(), self.run_folder.display().to_string()),
        ];
        if let Some(folder) = jail.sound_socket.as_deref().and_then(Path::parent) {
            environment.push(("XDG_RUNTIME_DIR".to_owned(), folder.display().to_string()));
        }
        environment
    }
}

#[cfg(test)]
mod tests;
