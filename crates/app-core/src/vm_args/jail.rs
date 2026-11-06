//! The first step of a jail for a forwarded GPU: QEMU runs in a bubblewrap sandbox, with no root. It
//! sees the system's programs and libraries read-only, the GPU's device nodes, KVM, and this VM's own
//! files. The rest of the user's home and of `/dev` aren't there. It keeps the network, for `-nic user`.
//! This limits what a QEMU gone wrong can open. It does nothing about what the host's driver does with
//! a call, and the device's own checks are still the wall for that.

use super::{Boot, UbuntuBoot, VmPlan};
use crate::machine::sorted_names;
use std::path::Path;

/// Namespaces of its own, a fresh session, and only the environment it's given. It doesn't die with
/// its parent: a VM outlives the app that started it. The device looks through `/sys` for the GPU's
/// render node, the sensor daemon reads the driver's version there, and `-nic user` reads
/// `/etc/resolv.conf`, which is often a link into `/run`.
const SANDBOX: &str = "bwrap --unshare-user --unshare-pid --unshare-ipc --unshare-uts --unshare-cgroup-try \
    --new-session --clearenv --setenv PATH /usr/bin --tmpfs /tmp --ro-bind /usr /usr --ro-bind-try /etc/ld.so.cache /etc/ld.so.cache \
    --ro-bind-try /etc/nsswitch.conf /etc/nsswitch.conf --ro-bind-try /etc/hosts /etc/hosts \
    --ro-bind-try /etc/resolv.conf /etc/resolv.conf \
    --symlink usr/lib /lib --symlink usr/lib64 /lib64 --symlink usr/bin /bin --proc /proc --dev /dev \
    --ro-bind /sys/class/drm /sys/class/drm --ro-bind /sys/devices /sys/devices \
    --ro-bind /sys/module/nvidia/version /sys/module/nvidia/version \
    --ro-bind-try /run/systemd/resolve /run/systemd/resolve";
const GPU_NODE_PREFIXES: [&str; 3] = ["nvidia", "kvm", "dri/renderD"];
/// Nodes a guest's calls have no use for.
const LEFT_OUT: [&str; 3] = ["nvidia-modeset", "nvidia-caps", "nvidia-nvswitchctl"];

/// The device nodes QEMU may open, under `devices`.
fn gpu_nodes(devices: &Path) -> Vec<String> {
    let mut names = sorted_names(devices);
    names.extend(sorted_names(&devices.join("dri")).iter().map(|name| format!("dri/{name}")));
    names.retain(|name| {
        GPU_NODE_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) && !LEFT_OUT.contains(&name.as_str())
    });
    names.iter().map(|name| devices.join(name).display().to_string()).collect()
}

fn bind(kind: &str, path: &str) -> [String; 3] {
    [kind.to_owned(), path.to_owned(), path.to_owned()]
}

fn folder_of(path: &Path) -> String {
    path.parent().unwrap_or(path).display().to_string()
}

impl VmPlan {
    /// What goes in front of QEMU. Only a VM with the forwarded GPU has one.
    pub(super) fn jail(&self, devices: &Path) -> Vec<String> {
        let Boot::Ubuntu(UbuntuBoot::WithGpu { image, allow_list, sensor_daemon }) = &self.boot else {
            return Vec::new();
        };
        let mut command: Vec<String> = SANDBOX.split_whitespace().map(str::to_owned).collect();
        // A QEMU in its build folder finds its firmware through links to the folder above.
        let qemu_tree = folder_of(Path::new(&folder_of(&self.qemu)));
        for path in [qemu_tree, folder_of(sensor_daemon)] {
            command.extend(bind("--ro-bind", &path));
        }
        for file in [image, allow_list] {
            command.extend(bind("--ro-bind", &file.display().to_string()));
        }
        if let Some(folder) = &self.qemu_files {
            command.extend(bind("--ro-bind", &folder.display().to_string()));
        }
        // The disk and not its folder, which holds the machine's settings and keys.
        command.extend(bind("--bind", &self.disk));
        command.extend(bind("--bind", &self.run_folder.display().to_string()));
        for node in gpu_nodes(devices) {
            command.extend(bind("--dev-bind", &node));
        }
        command.push("--".to_owned());
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{Kind, Machine};
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};

    fn jail_of(boot: Boot, devices: &Path, scratch: &Scratch) -> Vec<String> {
        let context = scratch_context(scratch);
        let machine =
            Machine { kind: Kind::Linux, ..Machine::new("ubuntu", "Ubuntu", "/m/ubuntu/disk.qcow2", "") };
        VmPlan::new(&context, &machine, boot, slots::first(), 0).jail(devices)
    }

    fn with_gpu() -> Boot {
        Boot::Ubuntu(UbuntuBoot::WithGpu {
            image: "/images/driver.iso".into(),
            allow_list: "/p/linux/allow.txt".into(),
            sensor_daemon: "/q/nvfwd-sensord".into(),
        })
    }

    fn devices(scratch: &Scratch) -> std::path::PathBuf {
        for name in [
            "dev/nvidia0",
            "dev/nvidiactl",
            "dev/nvidia-uvm",
            "dev/nvidia-modeset",
            "dev/kvm",
            "dev/sda",
            "dev/dri/card0",
            "dev/dri/renderD128",
        ] {
            scratch.write(name, "");
        }
        scratch.path("dev")
    }

    fn says(command: &[String], kind: &str, path: &str) -> bool {
        command.windows(3).any(|three| three[0] == kind && three[1] == path && three[2] == path)
    }

    #[test]
    fn it_sees_the_gpu_and_kvm_and_no_other_device() {
        let scratch = Scratch::new();
        let folder = devices(&scratch);
        let command = jail_of(with_gpu(), &folder, &scratch);
        for node in ["nvidia0", "nvidiactl", "nvidia-uvm", "kvm", "dri/renderD128"] {
            assert!(says(&command, "--dev-bind", &folder.join(node).display().to_string()), "{node}");
        }
        for node in ["nvidia-modeset", "sda", "dri/card0"] {
            assert!(!says(&command, "--dev-bind", &folder.join(node).display().to_string()), "{node}");
        }
    }

    #[test]
    fn only_its_own_folders_can_be_written() {
        let scratch = Scratch::new();
        let command = jail_of(with_gpu(), &devices(&scratch), &scratch);
        let written: Vec<&String> =
            command.windows(2).filter(|two| two[0] == "--bind").map(|two| &two[1]).collect();
        assert_eq!(written.len(), 2);
        assert_eq!(written[0], "/m/ubuntu/disk.qcow2");
        assert!(!written.iter().any(|path| *path == "/m/ubuntu"), "the machine's folder stays out");
        assert!(says(&command, "--ro-bind", "/p/linux/allow.txt"));
        assert!(says(&command, "--ro-bind", "/images/driver.iso"));
        assert!(!says(&command, "--ro-bind", "/etc"), "only the files QEMU reads from /etc");
        assert_eq!(command.last().map(String::as_str), Some("--"));
    }

    #[test]
    fn it_is_for_the_forwarded_gpu_alone() {
        let scratch = Scratch::new();
        let no_gpu = Boot::Ubuntu(UbuntuBoot::NoGpu { reason_file: "/r/reason".into() });
        assert!(jail_of(no_gpu, &devices(&scratch), &scratch).is_empty());
    }
}
