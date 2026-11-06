//! Installing and starting an Ubuntu guest. If the host can't honestly offer its driver, the guest
//! boots without the GPU and is told why.

use super::{KEY_POLL, Task, plan_for};
use crate::disk_image::disk_size_gib;
use crate::failure::{Failure, Result};
use crate::iso::kernel_package;
use crate::library::make_disk;
use crate::linux::driver_image::{Outcome, prepare};
use crate::linux::driver_versions::RpmHost;
use crate::linux::forwarding::forwarding_problem;
use crate::linux::guest_sources;
use crate::linux::seed::make_seed;
use crate::network::{Protocol, port_taken};
use crate::start_checks::start_problems;
use crate::vm_args::{Boot, INSTALL_LOG, UbuntuBoot, UbuntuInstall, ssh_port};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Only a finished install writes this line. The VM powers off either way.
const INSTALL_FINISHED: &[u8] = b"nvfwd-install-finished";
const INSTALL_HOURS: u64 = 2;
const REASON_FILE: &str = "gpu-off-reason";
const SENSOR_DAEMON: &str = "nvfwd-sensord";
const ALLOW_LIST: &str = "allow.txt";
const TURNED_OFF: &str = "The GPU is turned off in this machine's settings.";

fn isoinfo_files(iso: &str) -> Result<String> {
    let listing = Command::new("isoinfo").args(["-R", "-f", "-i", iso]).output()?;
    Ok(String::from_utf8_lossy(&listing.stdout).into_owned())
}

fn extract(iso: &str, name: &str, to: &Path) -> Result<()> {
    let data = Command::new("isoinfo").args(["-R", "-i", iso, "-x", name]).output()?.stdout;
    if data.is_empty() {
        return Err(Failure(format!("{iso} has no {name}, so it is not an Ubuntu server ISO.")));
    }
    fs::write(to, data)?;
    Ok(())
}

fn installed(serial: &[u8]) -> bool {
    serial.split(|byte| *byte == b'\n').any(|line| line.starts_with(INSTALL_FINISHED))
}

/// Takes a new machine from its ISO to its first boot. Each run starts again from nothing.
pub fn install(task: &mut Task) -> Result<()> {
    let context = task.context;
    if !task.machine.owns_files(context) {
        return Err(Failure::new(
            "This machine's disk is not in its folder, so nothing will install over it.",
        ));
    }
    let mut found = start_problems(context, &task.machine)?;
    found.retain(|problem| !problem.contains("is missing"));
    if !Path::new(&task.machine.iso).exists() {
        found.push(format!("{} is missing.", task.machine.iso));
    }
    if let Some(problem) = found.into_iter().next() {
        return Err(Failure(problem));
    }
    task.step("Preparing the install");
    let iso = task.machine.iso.clone();
    let listing = isoinfo_files(&iso)?;
    let package = listing.lines().find_map(kernel_package).map(str::to_owned);
    let package = package.ok_or_else(|| {
        Failure(format!("{iso} holds no generic kernel in its pool, so it is not an Ubuntu server ISO."))
    })?;
    let seed = make_seed(context, &task.machine, &package)?;
    let boot = task.machine.folder(context).join("boot");
    fs::create_dir_all(&boot)?;
    let (kernel, initrd) = (boot.join("vmlinuz"), boot.join("initrd"));
    extract(&iso, "/casper/vmlinuz", &kernel)?;
    extract(&iso, "/casper/initrd", &initrd)?;
    let size = disk_size_gib(&task.machine.disk)? as u32;
    fs::remove_file(&task.machine.disk)?;
    make_disk(context, &task.machine, size)?;
    task.step("Installing Ubuntu");
    let install = UbuntuInstall { iso, seed, kernel, initrd };
    let plan = plan_for(context, &task.machine, Boot::UbuntuInstall(install));
    task.host.start(&plan, &task.machine)?;
    wait_for_poweroff(task)?;
    let serial = fs::read(task.run_file(INSTALL_LOG)).unwrap_or_default();
    if !installed(&serial) {
        return Err(Failure(format!(
            "The install failed. See the end of {}",
            task.run_file(INSTALL_LOG).display()
        )));
    }
    let _ = fs::remove_dir_all(boot);
    task.machine.installing = false;
    task.machine.iso.clear();
    task.machine.save(context)?;
    start(task)
}

fn wait_for_poweroff(task: &Task) -> Result<()> {
    let end = task.now() + Duration::from_secs(INSTALL_HOURS * 3600);
    while task.pid().is_some() {
        if task.now() >= end {
            return Err(Failure(format!(
                "The install did not finish in {INSTALL_HOURS} hours. Show Screen says how far it got."
            )));
        }
        task.sleep(KEY_POLL);
    }
    Ok(())
}

fn gpu_boot(task: &mut Task) -> Result<UbuntuBoot> {
    let context = task.context;
    let reason_file = task.run_file(REASON_FILE);
    let refused = |task: &mut Task, reason: String| -> Result<UbuntuBoot> {
        fs::create_dir_all(task.context.run_folder(None))?;
        fs::write(&reason_file, format!("{reason}\n"))?;
        Ok(UbuntuBoot::NoGpu { reason_file: reason_file.clone() })
    };
    if !task.machine.gpu_preview {
        return refused(task, TURNED_OFF.to_owned());
    }
    let sources = guest_sources(context);
    let allow_list = sources.join(ALLOW_LIST);
    if let Some(reason) = forwarding_problem(&context.paths.qemu, &allow_list) {
        return refused(task, reason);
    }
    // It's fine if this isn't there: the device goes on without sensors, and logs why.
    let sensor_daemon = context.paths.qemu.with_file_name(SENSOR_DAEMON);
    task.step("Getting the driver image ready");
    let store = &context.paths.store;
    let tree = env_folder("NVIDIA_SOURCE").unwrap_or_else(|| store.join("src/open-gpu-kernel-modules"));
    let host = RpmHost { version_file: context.driver_version_file.clone() };
    match or_refused(prepare(&host, &sources, &tree, &store.join("linux-driver"))) {
        Outcome::Refused(reason) => {
            task.error = format!("Starting without the GPU: {reason}");
            refused(task, reason)
        }
        Outcome::Ready { image, warning } => {
            task.error = warning.unwrap_or_default();
            Ok(UbuntuBoot::WithGpu { image, allow_list, sensor_daemon })
        }
    }
}

/// Anything that goes wrong making the image is a reason to lose the GPU, and never the VM.
fn or_refused(prepared: Result<Outcome>) -> Outcome {
    prepared.unwrap_or_else(|failure| {
        Outcome::Refused(format!("The driver image could not be made: {}", failure.0))
    })
}

fn env_folder(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// Boots it, with the GPU if that can be offered. The guest does the rest on its own.
pub fn start(task: &mut Task) -> Result<()> {
    if let Some(problem) = start_problems(task.context, &task.machine)?.into_iter().next() {
        return Err(Failure(problem));
    }
    // QEMU's own message wouldn't say what to do about it.
    let port = ssh_port(task.context.slot);
    if port_taken(Protocol::Tcp, port) {
        return Err(Failure(format!(
            "Port {port} on this PC is in use by another program, and Ubuntu's SSH needs it. Set SSH_PORT to another one."
        )));
    }
    let boot = gpu_boot(task)?;
    task.step("Starting the VM");
    let plan = plan_for(task.context, &task.machine, Boot::Ubuntu(boot));
    task.host.start(&plan, &task.machine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{Kind, Machine};
    use crate::testing::{Scratch, scratch_context};
    use crate::vm_host::stand_in::RecordingHost;
    use std::os::unix::fs::PermissionsExt;

    fn ubuntu(context: &crate::context::Context) -> Machine {
        let folder = context.paths.machines.join("ubuntu");
        fs::create_dir_all(&folder).unwrap();
        let disk = folder.join("disk.qcow2");
        fs::write(&disk, "").unwrap();
        let machine = Machine {
            kind: Kind::Linux,
            cores: 2,
            ram_gib: 2,
            ..Machine::new("ubuntu", "Ubuntu", &disk.to_string_lossy(), "")
        };
        machine.save(context).unwrap();
        machine
    }

    fn off_reason(task: &mut Task) -> String {
        let boot = gpu_boot(task).unwrap();
        let UbuntuBoot::NoGpu { reason_file } = boot else { panic!("it should have no GPU") };
        fs::read_to_string(reason_file).unwrap()
    }

    #[test]
    fn only_a_line_of_its_own_counts_as_finished() {
        assert!(installed(b"booting\nnvfwd-install-finished\npowering off\n"));
        assert!(!installed(b"echo nvfwd-install-finished > /dev/ttyS0\n"));
        assert!(!installed(b""));
    }

    #[test]
    fn a_machine_with_its_gpu_turned_off_tells_the_guest_why() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let vm = Machine { gpu_preview: false, ..ubuntu(&context) };
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", vm);
        assert_eq!(off_reason(&mut task).trim(), TURNED_OFF);
    }

    #[test]
    fn a_qemu_with_no_device_boots_with_no_gpu() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", ubuntu(&context));
        assert!(off_reason(&mut task).contains("no nvfwd device"));
    }

    /// A folder where QEMU has the device and bwrap works, whatever this PC has.
    fn qemu_with_the_device(scratch: &Scratch, context: &mut crate::context::Context) {
        context.paths.qemu = scratch.path("qemu/qemu-system-x86_64");
        scratch.write("qemu/qemu-system-x86_64", "#!/bin/sh\nexit 0\n");
        fs::set_permissions(&context.paths.qemu, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_missing_sensor_daemon_is_not_what_turns_the_gpu_off() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        qemu_with_the_device(&scratch, &mut context);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", ubuntu(&context));
        assert!(!off_reason(&mut task).contains("no nvfwd device"));
    }

    #[test]
    fn a_failure_making_the_image_only_turns_the_gpu_off() {
        let said = or_refused(Err(Failure::new("gcc found no such header.")));
        assert_eq!(
            said,
            Outcome::Refused("The driver image could not be made: gcc found no such header.".to_owned())
        );
    }

    #[test]
    fn a_port_in_use_is_named_before_qemu_can_fail_on_it() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.cpus = crate::host::stand_in::fake_cpus(&scratch, "cpus", &[32768; 8]);
        context.meminfo = scratch.write("meminfo", "MemAvailable:   100000000 kB\n");
        let listener = std::net::TcpListener::bind(("127.0.0.1", ssh_port(context.slot)));
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", ubuntu(&context));
        let found = start(&mut task);
        // Whoever holds the port, this test or another program, the words are the same.
        assert!(listener.is_ok() || found.is_err());
        assert!(found.unwrap_err().0.contains("Set SSH_PORT"));
    }

    #[test]
    fn an_iso_with_no_kernel_says_it_is_not_the_server_iso() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        context.cpus = crate::host::stand_in::fake_cpus(&scratch, "cpus", &[32768; 8]);
        context.meminfo = scratch.write("meminfo", "MemAvailable:   100000000 kB\n");
        let iso = crate::iso::stand_in::make_iso(&scratch, "other", &[("boot/grub.cfg", b"linux")]);
        let vm = Machine { installing: true, iso, ..ubuntu(&context) };
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "install", vm);
        assert!(install(&mut task).unwrap_err().0.contains("not an Ubuntu server ISO"));
    }
}
