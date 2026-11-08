//! What the forwarded GPU's QEMU may reach beyond what any jailed QEMU does. The device opens the card
//! itself, so it gets the card's nodes, which a Windows guest's QEMU never does, and it reads the
//! driver image and the allow list and runs the sensor daemon. It looks through `/sys` for the GPU's
//! render node, which any QEMU may read. The device's own checks are still the wall between a guest
//! and the host's driver: this only limits what a QEMU gone wrong can open.

use super::{Boot, UbuntuBoot, VmPlan};
use crate::machine::sorted_names;
use guesthouse_jail::{Grant, Plan};
use std::path::{Path, PathBuf};

/// Where the card's nodes are.
pub(super) const DEVICES: &str = "/dev";
const GPU_NODE_PREFIXES: [&str; 2] = ["nvidia", "dri/renderD"];
/// Nodes a guest's calls have no use for.
const LEFT_OUT: [&str; 3] = ["nvidia-modeset", "nvidia-caps", "nvidia-nvswitchctl"];

/// What the device is started with.
pub(super) struct ForwardedGpu<'a> {
    pub image: &'a Path,
    pub allow_list: &'a Path,
    pub sensor_daemon: &'a Path,
}

/// The card's nodes QEMU may open, under `devices`.
pub(super) fn gpu_nodes(devices: &Path) -> Vec<PathBuf> {
    let mut names = sorted_names(devices);
    names.extend(sorted_names(&devices.join("dri")).iter().map(|name| format!("dri/{name}")));
    names.retain(|name| {
        GPU_NODE_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) && !LEFT_OUT.contains(&name.as_str())
    });
    names.iter().map(|name| devices.join(name)).collect()
}

impl VmPlan {
    /// The forwarded GPU, if this boot has it.
    pub(super) fn forwarded_gpu(&self) -> Option<ForwardedGpu<'_>> {
        let Boot::Ubuntu(UbuntuBoot::WithGpu { image, allow_list, sensor_daemon }) = &self.boot else {
            return None;
        };
        Some(ForwardedGpu { image, allow_list, sensor_daemon })
    }

    /// Whether this start wants a jailed QEMU for the forwarded GPU.
    pub fn has_forwarded_gpu(&self) -> bool {
        self.forwarded_gpu().is_some()
    }

    /// The forwarded GPU's own files and the card's nodes found under `devices`.
    pub(super) fn grant_forwarding(&self, plan: &mut Plan, devices: &Path) {
        let Some(gpu) = self.forwarded_gpu() else { return };
        plan.grant(Grant::Read, gpu.image).grant(Grant::Read, gpu.allow_list);
        plan.grant(Grant::Run, gpu.sensor_daemon);
        for node in gpu_nodes(devices) {
            plan.grant(Grant::Device, node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{Kind, Machine};
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};

    fn plan_of(boot: Boot, scratch: &Scratch) -> VmPlan {
        let context = scratch_context(scratch);
        let machine =
            Machine { kind: Kind::Linux, ..Machine::new("ubuntu", "Ubuntu", "/m/ubuntu/disk.qcow2", "") };
        VmPlan::new(&context, &machine, boot, slots::first(), 0)
    }

    fn with_gpu() -> Boot {
        Boot::Ubuntu(UbuntuBoot::WithGpu {
            image: "/images/driver.iso".into(),
            allow_list: "/p/linux/allow.txt".into(),
            sensor_daemon: "/q/nvfwd-sensord".into(),
        })
    }

    fn devices(scratch: &Scratch) -> PathBuf {
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

    fn granted(plan: &Plan, grant: Grant) -> Vec<PathBuf> {
        plan.paths.iter().filter(|(given, _)| *given == grant).map(|(_, path)| path.clone()).collect()
    }

    #[test]
    fn it_sees_the_gpu_and_no_other_device_of_the_card() {
        let scratch = Scratch::new();
        let folder = devices(&scratch);
        let mut rules = Plan::default();
        plan_of(with_gpu(), &scratch).grant_forwarding(&mut rules, &folder);
        let nodes = granted(&rules, Grant::Device);
        for node in ["nvidia0", "nvidiactl", "nvidia-uvm", "dri/renderD128"] {
            assert!(nodes.contains(&folder.join(node)), "{node}");
        }
        for node in ["nvidia-modeset", "sda", "dri/card0", "kvm"] {
            assert!(!nodes.contains(&folder.join(node)), "{node}");
        }
    }

    #[test]
    fn it_reads_the_image_and_the_list_and_runs_the_sensor_daemon() {
        let scratch = Scratch::new();
        let mut rules = Plan::default();
        plan_of(with_gpu(), &scratch).grant_forwarding(&mut rules, &devices(&scratch));
        let read = granted(&rules, Grant::Read);
        assert!(read.contains(&"/images/driver.iso".into()) && read.contains(&"/p/linux/allow.txt".into()));
        assert_eq!(granted(&rules, Grant::Run), [PathBuf::from("/q/nvfwd-sensord")]);
        assert!(granted(&rules, Grant::Write).is_empty() && granted(&rules, Grant::Own).is_empty());
    }

    #[test]
    fn it_is_for_the_forwarded_gpu_alone() {
        let scratch = Scratch::new();
        let no_gpu = Boot::Ubuntu(UbuntuBoot::NoGpu { reason_file: "/r/reason".into() });
        let plan = plan_of(no_gpu, &scratch);
        assert!(!plan.has_forwarded_gpu());
        let mut rules = Plan::default();
        plan.grant_forwarding(&mut rules, &devices(&scratch));
        assert!(rules.paths.is_empty());
    }
}
