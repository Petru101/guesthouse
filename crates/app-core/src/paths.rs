//! Where everything lives. Nothing here is relative to a project folder, so an install can go anywhere.
//!
//! The data store is the only large thing, and `~/.config/guesthouse/store` can move it to another
//! disk. A VM's sockets go in the runtime folder, the log in the state folder, and QEMU and the
//! guest's files in the program folder.

use guesthouse_daemon::slot::Slot;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const STORE_VARIABLE: &str = "GUESTHOUSE_STORE";
const MACHINES_VARIABLE: &str = "GUESTHOUSE_MACHINES";
const PROGRAM_VARIABLE: &str = "GUESTHOUSE_PROGRAM";
const SHARED_MEMORY_VARIABLE: &str = "GUESTHOUSE_SHARED_MEMORY";
/// Keeps a second copy of the app clear of the first, whose VM it must not disturb.
const RUN_VARIABLE: &str = "RUN_DIR";
const QEMU_VARIABLE: &str = "QEMU";
const AGENT_PORT_VARIABLE: &str = "AGENT_PORT";

const APP_FOLDER: &str = "guesthouse";
const STORE_POINTER: &str = "store";
const INSTALLED_PROGRAM: &str = "/usr/libexec/guesthouse";
const USER_PROGRAM: &str = ".local/lib/guesthouse/current";
/// What the program folder holds besides the binaries, which tells it from any other folder.
const PROGRAM_MARK: &str = "guest";
const SHARED_MEMORY: &str = "/dev/shm";
const QEMU_PROGRAM: &str = "qemu/build/qemu-system-x86_64";
const SYSTEM_QEMU: &str = "qemu-system-x86_64";

/// The environment, as a value, so that a test can say what it holds.
#[derive(Debug, Clone, Default)]
pub struct Variables(HashMap<String, String>);

impl Variables {
    pub fn from_process() -> Self {
        Self(env::vars().collect())
    }

    pub fn of(pairs: &[(&str, &str)]) -> Self {
        Self(pairs.iter().map(|(name, value)| ((*name).to_owned(), (*value).to_owned())).collect())
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str).filter(|value| !value.is_empty())
    }

    fn folder(&self, name: &str) -> Option<PathBuf> {
        self.get(name).map(PathBuf::from)
    }

    /// `$NAME`, or `~/fallback` as the XDG rules say when it is unset.
    fn xdg(&self, name: &str, fallback: &str) -> PathBuf {
        self.folder(name).unwrap_or_else(|| self.home().join(fallback))
    }

    fn home(&self) -> PathBuf {
        self.folder("HOME").unwrap_or_default()
    }
}

/// Every folder and program the app uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The data store: machines, setup's files and the driver.
    pub store: PathBuf,
    pub machines: PathBuf,
    pub config: PathBuf,
    /// Where the log goes.
    pub state: PathBuf,
    /// The folder that holds each slot's run folder, and what the daemon is given as its base.
    pub runtime: PathBuf,
    /// The program folder: QEMU and the guest's files. It isn't in this repo, and the app runs plain
    /// VMs without it.
    pub program: PathBuf,
    /// Where this app's own programs are, which is beside the running one if it can say.
    pub binaries: PathBuf,
    pub shared_memory: PathBuf,
    pub qemu: PathBuf,
    /// A run folder given by hand, which marks this as a second instance. See [`Paths::second_instance`].
    pub own_run: Option<PathBuf>,
    pub own_agent_port: Option<u16>,
    pub firmware: Firmware,
}

/// The firmware files, which come from the distribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    pub code: PathBuf,
    /// Goes with `vars`.
    pub vars: PathBuf,
    /// The 4 MiB firmware, the one with TPM drivers.
    pub code_secure: PathBuf,
    /// With Microsoft's keys, so Secure Boot is on. It goes with `code_secure`.
    pub vars_secure: PathBuf,
}

impl Default for Firmware {
    fn default() -> Self {
        const FOLDER: &str = "/usr/share/edk2/ovmf";
        let file = |name: &str| Path::new(FOLDER).join(name);
        Self {
            code: file("OVMF_CODE.secboot.fd"),
            vars: file("OVMF_VARS.fd"),
            code_secure: file("OVMF_CODE_4M.secboot.qcow2"),
            vars_secure: file("OVMF_VARS_4M.secboot.qcow2"),
        }
    }
}

impl Paths {
    pub fn from_process() -> Self {
        let helpers = env::current_exe().ok().and_then(|path| Some(path.parent()?.to_owned()));
        Self::new(&Variables::from_process(), helpers.as_deref())
    }

    /// `running_from` is the folder of the running program, which an unpacked release runs from.
    pub fn new(variables: &Variables, running_from: Option<&Path>) -> Self {
        let config = variables.xdg("XDG_CONFIG_HOME", ".config").join(APP_FOLDER);
        let data = variables.xdg("XDG_DATA_HOME", ".local/share").join(APP_FOLDER);
        let state = variables.xdg("XDG_STATE_HOME", ".local/state").join(APP_FOLDER);
        let store = variables
            .folder(STORE_VARIABLE)
            .or_else(|| store_pointer(&config.join(STORE_POINTER), &variables.home()))
            .unwrap_or(data);
        let machines = variables.folder(MACHINES_VARIABLE).unwrap_or_else(|| store.join("machines"));
        // With no runtime folder the state folder is the next private place. Sockets need a short path.
        let runtime = variables
            .folder("XDG_RUNTIME_DIR")
            .map_or_else(|| state.join("runtime"), |folder| folder.join(APP_FOLDER));
        let program = program_folder(variables, running_from);
        Self {
            qemu: variables.folder(QEMU_VARIABLE).unwrap_or_else(|| qemu_program(variables, &program)),
            binaries: running_from.map_or_else(|| program.clone(), Path::to_owned),
            store,
            machines,
            config,
            state,
            runtime,
            program,
            shared_memory: variables
                .folder(SHARED_MEMORY_VARIABLE)
                .unwrap_or_else(|| PathBuf::from(SHARED_MEMORY)),
            own_run: variables.folder(RUN_VARIABLE),
            own_agent_port: variables.get(AGENT_PORT_VARIABLE).and_then(|text| text.parse().ok()),
            firmware: Firmware::default(),
        }
    }

    /// A second instance runs beside a VM that is not its own, with its own run folder. It runs
    /// machines with no GPU and Windows installs up to their last shutdown. The GPU's steps find VMs
    /// by program, not by run folder, so it never takes them: they would stop the other VM.
    pub fn second_instance(&self) -> bool {
        self.own_run.is_some()
    }

    pub fn run_folder(&self, slot: Slot) -> PathBuf {
        self.own_run.clone().unwrap_or_else(|| slot.run_folder(&self.runtime))
    }

    pub fn agent_port(&self, slot: Slot) -> u16 {
        self.own_agent_port.unwrap_or_else(|| slot.agent_port())
    }

    /// A file in shared memory, like `ram`, `iomem` or `display`.
    pub fn shared_file(&self, name: &str, slot: Slot) -> PathBuf {
        let path = slot.shared_file(name);
        self.shared_memory.join(path.file_name().unwrap_or_default())
    }

    pub fn log_file(&self) -> PathBuf {
        self.state.join("guesthouse.log")
    }

    pub fn setup_folder(&self) -> PathBuf {
        self.store.join("setup")
    }

    pub fn setup_file(&self) -> PathBuf {
        self.setup_folder().join("setup.json")
    }

    pub fn prefs_file(&self) -> PathBuf {
        self.config.join("app.json")
    }

    /// The files that go into a Windows guest, from the program folder.
    pub fn guest_files(&self) -> PathBuf {
        self.program.join("guest")
    }

    /// A file in the program folder.
    pub fn helper(&self, name: &str) -> PathBuf {
        self.program.join(name)
    }

    /// One of this app's own programs.
    pub fn binary(&self, name: &str) -> PathBuf {
        self.binaries.join(name)
    }
}

/// The first line of the pointer file, which can start with `~/`.
fn store_pointer(pointer: &Path, home: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(pointer).ok()?;
    let line = text.lines().next()?.trim();
    match line {
        "" => None,
        _ => Some(line.strip_prefix("~/").map_or_else(|| line.into(), |rest| home.join(rest))),
    }
}

/// The QEMU that comes with the program folder, or else the system's, which runs plain VMs.
fn qemu_program(variables: &Variables, program: &Path) -> PathBuf {
    let own = program.join(QEMU_PROGRAM);
    if own.exists() {
        return own;
    }
    let path = variables.get("PATH").unwrap_or_default();
    let system = env::split_paths(&path).map(|folder| folder.join(SYSTEM_QEMU)).find(|file| file.exists());
    system.unwrap_or(own)
}

fn program_folder(variables: &Variables, running_from: Option<&Path>) -> PathBuf {
    if let Some(folder) = variables.folder(PROGRAM_VARIABLE) {
        return folder;
    }
    let here = running_from.filter(|folder| folder.join(PROGRAM_MARK).is_dir());
    let user = variables.home().join(USER_PROGRAM);
    here.map(Path::to_owned)
        .or_else(|| user.join(PROGRAM_MARK).is_dir().then_some(user))
        .unwrap_or_else(|| PathBuf::from(INSTALLED_PROGRAM))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    fn slot(number: &str) -> Slot {
        Slot::parse(Some(number)).unwrap()
    }

    #[test]
    fn with_no_qemu_of_its_own_the_systems_is_used() {
        let folder = env::temp_dir().join(format!("gh-paths-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("qemu-system-x86_64"), "").unwrap();
        let path = folder.to_string_lossy().into_owned();
        let paths = Paths::new(&Variables::of(&[("HOME", "/h"), ("PATH", &path)]), None);
        assert_eq!(paths.qemu, folder.join("qemu-system-x86_64"));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn everything_follows_the_xdg_folders() {
        let variables = Variables::of(&[
            ("HOME", "/h"),
            ("XDG_DATA_HOME", "/data"),
            ("XDG_STATE_HOME", "/state"),
            ("XDG_CONFIG_HOME", "/config"),
            ("XDG_RUNTIME_DIR", "/run/user/1"),
        ]);
        let paths = Paths::new(&variables, None);
        assert_eq!(paths.store, Path::new("/data/guesthouse"));
        assert_eq!(paths.machines, Path::new("/data/guesthouse/machines"));
        assert_eq!(paths.log_file(), Path::new("/state/guesthouse/guesthouse.log"));
        assert_eq!(paths.prefs_file(), Path::new("/config/guesthouse/app.json"));
        assert_eq!(paths.run_folder(slot("0")), Path::new("/run/user/1/guesthouse/run"));
        assert_eq!(paths.run_folder(slot("2")), Path::new("/run/user/1/guesthouse/run-2"));
    }

    #[test]
    fn unset_folders_fall_back_under_home() {
        let paths = Paths::new(&Variables::of(&[("HOME", "/h")]), None);
        assert_eq!(paths.store, Path::new("/h/.local/share/guesthouse"));
        assert_eq!(paths.state, Path::new("/h/.local/state/guesthouse"));
        assert_eq!(paths.config, Path::new("/h/.config/guesthouse"));
        assert_eq!(paths.runtime, Path::new("/h/.local/state/guesthouse/runtime"));
    }

    #[test]
    fn the_pointer_moves_the_store() {
        let scratch = Scratch::new();
        let config = scratch.path("config");
        fs::create_dir_all(config.join(APP_FOLDER)).unwrap();
        fs::write(config.join("guesthouse/store"), "~/big/disk\nanything else\n").unwrap();
        let variables = Variables::of(&[("HOME", "/h"), ("XDG_CONFIG_HOME", config.to_str().unwrap())]);
        assert_eq!(Paths::new(&variables, None).store, Path::new("/h/big/disk"));
        fs::write(config.join("guesthouse/store"), "/mnt/disk\n").unwrap();
        assert_eq!(Paths::new(&variables, None).store, Path::new("/mnt/disk"));
        fs::write(config.join("guesthouse/store"), "\n").unwrap();
        assert_eq!(Paths::new(&variables, None).store, Path::new("/h/.local/share/guesthouse"));
    }

    #[test]
    fn the_variable_beats_the_pointer() {
        let variables = Variables::of(&[("HOME", "/h"), ("GUESTHOUSE_STORE", "/s")]);
        assert_eq!(Paths::new(&variables, None).store, Path::new("/s"));
    }

    #[test]
    fn the_program_folder_is_found_from_the_running_program() {
        let scratch = Scratch::new();
        let release = scratch.path("release");
        fs::create_dir_all(release.join("guest")).unwrap();
        let variables = Variables::of(&[("HOME", "/h")]);
        let paths = Paths::new(&variables, Some(&release));
        assert_eq!(paths.program, release);
        assert_eq!(paths.qemu, release.join("qemu/build/qemu-system-x86_64"));
        let elsewhere = Paths::new(&variables, Some(&scratch.path("elsewhere")));
        assert_eq!(elsewhere.program, Path::new("/usr/libexec/guesthouse"));
    }

    #[test]
    fn a_run_folder_given_by_hand_makes_a_second_instance() {
        let plain = Paths::new(&Variables::of(&[("HOME", "/h")]), None);
        assert!(!plain.second_instance());
        let second =
            Paths::new(&Variables::of(&[("HOME", "/h"), ("RUN_DIR", "/r"), ("AGENT_PORT", "9")]), None);
        assert!(second.second_instance());
        assert_eq!(second.run_folder(slot("3")), Path::new("/r"));
        assert_eq!(second.agent_port(slot("3")), 9);
    }

    #[test]
    fn shared_files_keep_the_daemons_names() {
        let paths = Paths::new(&Variables::of(&[("HOME", "/h"), ("GUESTHOUSE_SHARED_MEMORY", "/shm")]), None);
        assert_eq!(paths.shared_file("ram", slot("0")), Path::new("/shm/gpupv-ram"));
        assert_eq!(paths.shared_file("iomem", slot("2")), Path::new("/shm/gpupv-iomem-2"));
    }
}
