//! `guesthouse-jail PLAN [--passt PROGRAM FD PID_FILE [--passt-word WORD]...]
//! [--tap HELPER BRIDGE FD] -- PROGRAM ARGUMENTS`: puts itself in the jail the plan describes, with
//! QEMU's filter, and becomes the program. The jail carries across the exec, so the program can't
//! leave it.
//!
//! `--passt` starts passt first, outside the jail, as it builds a sandbox of its own that can't be
//! made inside one, and leaves the other end of its connection open at `FD`. Its pid and start time
//! go in `PID_FILE`. `--tap` gets a bridged
//! network's device from QEMU's setuid helper first, as a helper can't gain its privileges once the
//! jail is on. The device is left open at `FD` for the program.
//!
//! With `--open-only` instead of a plan, it opens those and runs the program as it is. That's the
//! step before bubblewrap, whose namespaces a setuid helper can't work in, and which then runs this
//! launcher again with the plan.

mod handover;
mod passt;
mod tap;

pub use passt::{start as start_passt, start_time};
pub use tap::open as open_tap;

use crate::seccomp::{self, Threads};
use crate::{Plan, filters, landlock};
use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

const END_OF_PLAN: &str = "--";
const TAP: &str = "--tap";
const PASST: &str = "--passt";
const PASST_WORD: &str = "--passt-word";
const OPEN_ONLY: &str = "--open-only";
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

/// passt, which the launcher starts with these words and connects to the program at `fd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passt {
    pub program: PathBuf,
    pub fd: i32,
    pub pid_file: PathBuf,
    pub words: Vec<String>,
}

/// Everything the launcher was told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The jail, or `None` to open what's asked for and run the program as it is.
    pub plan: Option<Plan>,
    pub passt: Option<Passt>,
    pub tap: Option<Tap>,
    pub unix_sockets: Vec<UnixSocket>,
    /// Refuse the program any unix socket it would make, which leaves it the ones opened for it.
    pub no_new_unix_sockets: bool,
    pub program: Vec<OsString>,
}

impl Launch {
    /// The words that tell the launcher this, up to the program's, which are the caller's to add.
    pub fn words(&self) -> Vec<String> {
        let mut words = match &self.plan {
            Some(plan) => plan.words(),
            None => vec![OPEN_ONLY.to_owned()],
        };
        if let Some(passt) = &self.passt {
            words.extend([
                PASST.to_owned(),
                passt.program.display().to_string(),
                passt.fd.to_string(),
                passt.pid_file.display().to_string(),
            ]);
            for word in &passt.words {
                words.extend([PASST_WORD.to_owned(), word.clone()]);
            }
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
            plan: Some(Plan::default()),
            passt: None,
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
                OPEN_ONLY => launch.plan = None,
                PASST => {
                    let (program, fd, pid_file) = (next(PASST)?, next(PASST)?, next(PASST)?);
                    let fd =
                        fd.parse().map_err(|_| format!("{PASST} needs a descriptor number, not {fd:?}"))?;
                    let pid_file = pid_file.into();
                    launch.passt = Some(Passt { program: program.into(), fd, pid_file, words: Vec::new() });
                }
                PASST_WORD => {
                    let word = next(PASST_WORD)?;
                    let passt =
                        launch.passt.as_mut().ok_or_else(|| format!("{PASST_WORD} comes after {PASST}"))?;
                    passt.words.push(word);
                }
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
                    let plan = launch.plan.as_mut().ok_or_else(|| format!("{OPEN_ONLY} takes no {name}"))?;
                    if !plan.take(&name, &value)? {
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

    /// Jails this process and becomes the program. It only comes back if something failed. passt goes
    /// first, so nothing else opened for the program reaches it.
    pub fn run(self) -> io::Error {
        if let Some(passt) = &self.passt
            && let Err(error) = passt::start(passt)
        {
            return error;
        }
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
        if let Some(plan) = &self.plan
            && let Err(error) = self.jail(plan)
        {
            return error;
        }
        let (program, arguments) = self.program.split_first().expect("parse makes sure there's a program");
        Command::new(program).args(arguments).exec()
    }

    fn jail(&self, plan: &Plan) -> io::Result<()> {
        let applied = landlock::apply(plan)?;
        let filter = filters::qemu(self.no_new_unix_sockets);
        seccomp::install(&filter, Threads::This)?;
        let missing = applied.missing();
        let leaves_out = if missing.is_empty() {
            String::new()
        } else {
            format!(", which leaves out {}", missing.join(", "))
        };
        let sockets = if self.no_new_unix_sockets { ", and no unix socket of its own" } else { "" };
        eprintln!("jailed, with Landlock {}{leaves_out}{sockets}", applied.landlock_abi);
        Ok(())
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
            plan: Some(plan),
            passt: Some(Passt {
                program: "/usr/bin/passt".into(),
                fd: 11,
                pid_file: "/r/passt.pid".into(),
                words: ["--quiet", "--mtu", "1500"].map(str::to_owned).to_vec(),
            }),
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
    fn open_only_has_no_plan_and_takes_none() {
        let wanted = Launch { plan: None, unix_sockets: Vec::new(), ..launch() };
        let mut words: Vec<OsString> = wanted.words().into_iter().map(OsString::from).collect();
        assert_eq!(words[0], OPEN_ONLY);
        words.extend(wanted.program.iter().cloned());
        assert_eq!(Launch::parse(words).unwrap(), wanted);
        let parse = |words: &[&str]| Launch::parse(words.iter().map(OsString::from));
        assert!(parse(&["--open-only", "--read", "/usr", "--", "bwrap"]).unwrap_err().contains("--read"));
        assert!(parse(&["--passt-word", "--quiet", "--", "qemu"]).unwrap_err().contains("--passt"));
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
