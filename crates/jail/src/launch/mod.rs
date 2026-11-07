//! `guesthouse-jail PLAN [--own-sandboxes] [--tap HELPER BRIDGE FD] -- PROGRAM ARGUMENTS`: puts itself
//! in the jail the plan describes, with QEMU's filter, and becomes the program. The jail carries
//! across the exec, so the program can't leave it.
//!
//! `--own-sandboxes` is for a program that starts one that makes namespaces of its own, as passt does.
//! `--tap` gets a bridged network's device from QEMU's setuid helper first, as a helper can't gain
//! its privileges once the jail is on. The device is left open at `FD` for the program.

mod handover;
mod tap;

pub use tap::open as open_tap;

use crate::seccomp::{self, Threads};
use crate::{Plan, filters, landlock};
use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

const END_OF_PLAN: &str = "--";
const OWN_SANDBOXES: &str = "--own-sandboxes";
const TAP: &str = "--tap";
const UNIX_LISTEN: &str = "--unix-listen";
const UNIX_CONNECT: &str = "--unix-connect";
const NO_NEW_UNIX_SOCKETS: &str = "--no-new-unix-sockets";

/// A unix socket the launcher opens for the program, so the program needs to make none itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnixSocket {
    pub path: PathBuf,
    pub fd: i32,
    /// Listening on the path, rather than connected to it.
    pub listen: bool,
}

/// What a bridged network's device comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tap {
    pub helper: PathBuf,
    pub bridge: String,
    pub fd: i32,
}

/// Everything the launcher was told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub plan: Plan,
    pub own_sandboxes: bool,
    pub tap: Option<Tap>,
    pub unix_sockets: Vec<UnixSocket>,
    /// Refuse the program any unix socket it would make, which leaves it the ones opened for it.
    pub no_new_unix_sockets: bool,
    pub program: Vec<OsString>,
}

impl Launch {
    /// The words that tell the launcher this, up to the program's, which are the caller's to add.
    pub fn words(&self) -> Vec<String> {
        let mut words = self.plan.words();
        if self.own_sandboxes {
            words.push(OWN_SANDBOXES.to_owned());
        }
        if let Some(tap) = &self.tap {
            words.extend([
                TAP.to_owned(),
                tap.helper.display().to_string(),
                tap.bridge.clone(),
                tap.fd.to_string(),
            ]);
        }
        for socket in &self.unix_sockets {
            let kind = if socket.listen { UNIX_LISTEN } else { UNIX_CONNECT };
            words.extend([kind.to_owned(), socket.path.display().to_string(), socket.fd.to_string()]);
        }
        if self.no_new_unix_sockets {
            words.push(NO_NEW_UNIX_SOCKETS.to_owned());
        }
        words.push(END_OF_PLAN.to_owned());
        words
    }

    pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut launch = Self {
            plan: Plan::default(),
            own_sandboxes: false,
            tap: None,
            unix_sockets: Vec::new(),
            no_new_unix_sockets: false,
            program: Vec::new(),
        };
        let mut words = arguments.into_iter();
        let mut next = |after: &str| {
            let word = words.next().ok_or_else(|| format!("{after} needs a value"))?;
            word.into_string().map_err(|word| format!("{after} can't take {word:?}"))
        };
        loop {
            let name = next("the plan")?;
            match name.as_str() {
                END_OF_PLAN => break,
                OWN_SANDBOXES => launch.own_sandboxes = true,
                NO_NEW_UNIX_SOCKETS => launch.no_new_unix_sockets = true,
                UNIX_LISTEN | UNIX_CONNECT => {
                    let (path, fd) = (next(&name)?, next(&name)?);
                    let fd =
                        fd.parse().map_err(|_| format!("{name} needs a descriptor number, not {fd:?}"))?;
                    launch.unix_sockets.push(UnixSocket {
                        path: path.into(),
                        fd,
                        listen: name == UNIX_LISTEN,
                    });
                }
                TAP => {
                    let (helper, bridge, fd) = (next(TAP)?, next(TAP)?, next(TAP)?);
                    let fd =
                        fd.parse().map_err(|_| format!("{TAP} needs a descriptor number, not {fd:?}"))?;
                    launch.tap = Some(Tap { helper: helper.into(), bridge, fd });
                }
                _ => {
                    let value = next(&name)?;
                    if !launch.plan.take(&name, &value)? {
                        return Err(format!("{name} isn't something the jail knows"));
                    }
                }
            }
        }
        launch.program = words.collect();
        if launch.program.is_empty() {
            return Err("there's no program to run".to_owned());
        }
        Ok(launch)
    }

    /// Jails this process and becomes the program. It only comes back if something failed.
    pub fn run(self) -> io::Error {
        if let Some(tap) = &self.tap
            && let Err(error) = tap::open(tap)
        {
            return error;
        }
        for socket in &self.unix_sockets {
            if let Err(error) = handover::open_unix(socket) {
                return error;
            }
        }
        let applied = match landlock::apply(&self.plan) {
            Ok(applied) => applied,
            Err(error) => return error,
        };
        let filter = filters::qemu(self.own_sandboxes, self.no_new_unix_sockets);
        if let Err(error) = seccomp::install(&filter, Threads::This) {
            return error;
        }
        let missing = applied.missing();
        let leaves_out = if missing.is_empty() {
            String::new()
        } else {
            format!(", which leaves out {}", missing.join(", "))
        };
        eprintln!("jailed, with Landlock {}{leaves_out}", applied.landlock_abi);
        let (program, arguments) = self.program.split_first().expect("parse makes sure there's a program");
        Command::new(program).args(arguments).exec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Grant, Ports};

    fn launch() -> Launch {
        let mut plan = Plan::default();
        plan.grant(Grant::Run, "/usr").grant(Grant::Write, "/m/a/disk.qcow2");
        plan.network.tcp_bind = Ports::Only(vec![18768]);
        Launch {
            plan,
            own_sandboxes: true,
            tap: Some(Tap { helper: "/usr/libexec/qemu-bridge-helper".into(), bridge: "br0".into(), fd: 3 }),
            unix_sockets: vec![UnixSocket { path: "/r/qmp.sock".into(), fd: 20, listen: true }],
            no_new_unix_sockets: true,
            program: ["/usr/bin/qemu-system-x86_64", "-name", "gpupv", "--"].map(OsString::from).to_vec(),
        }
    }

    #[test]
    fn the_words_say_it_all_and_the_program_keeps_its_own() {
        let wanted = launch();
        let mut words: Vec<OsString> = wanted.words().into_iter().map(OsString::from).collect();
        words.extend(wanted.program.iter().cloned());
        let read = Launch::parse(words).unwrap();
        assert_eq!(read, wanted);
    }

    #[test]
    fn what_it_does_not_know_stops_it() {
        let parse = |words: &[&str]| Launch::parse(words.iter().map(OsString::from));
        assert!(parse(&["--everything", "/", "--", "qemu"]).unwrap_err().contains("--everything"));
        assert!(parse(&["--read", "/usr", "--"]).unwrap_err().contains("no program"));
        assert!(parse(&["--read"]).is_err());
        assert!(parse(&["--tap", "/helper", "br0", "three", "--", "qemu"]).is_err());
    }
}
