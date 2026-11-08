//! What the forwarded GPU's QEMU may reach beyond what any jailed QEMU does. The device opens the card
//! itself, so it gets the card's nodes, which a Windows guest's QEMU never does, and it reads the
//! driver image and the allow list and runs the sensor daemon. It looks through `/sys` for the GPU's
//! render node, which any QEMU may read. The device's own checks are still the wall between a guest
//! and the host's driver: this only limits what a QEMU gone wrong can open.
//!
//! Of the card nodes, it gets the driver's two shared ones and the chosen card's own, so on a PC with
//! two cards the other card stays out of its reach. The RM won't make a device for a GPU whose file
//! the process hasn't open, so that holds for the RM's calls too.

use super::{Boot, UbuntuBoot, VmPlan};
use crate::linux::card::ForwardedCard;
use guesthouse_jail::{Grant, Plan};
use std::path::{Path, PathBuf};

/// Where the card's nodes are.
pub(super) const DEVICES: &str = "/dev";
/// The driver's nodes that aren't any one card's: its control file and UVM's.
const SHARED_NODES: [&str; 2] = ["nvidiactl", "nvidia-uvm"];
const CARD_NODE_PREFIX: &str = "nvidia";
const RENDER_NODES: &str = "dri";

/// What the device is started with.
pub(super) struct ForwardedGpu<'a> {
    pub image: &'a Path,
    pub allow_list: &'a Path,
    pub sensor_daemon: &'a Path,
    pub card: &'a ForwardedCard,
}

/// The nodes QEMU may open for this card, under `devices`, of those there are.
pub(super) fn gpu_nodes(devices: &Path, card: &ForwardedCard) -> Vec<PathBuf> {
    let mut nodes: Vec<PathBuf> = SHARED_NODES.iter().map(|name| devices.join(name)).collect();
    nodes.push(devices.join(format!("{CARD_NODE_PREFIX}{}", card.minor)));
    nodes.extend(card.render_node.iter().map(|name| devices.join(RENDER_NODES).join(name)));
    nodes.retain(|node| node.exists());
    nodes
}

impl VmPlan {
    /// The forwarded GPU, if this boot has it.
    pub(super) fn forwarded_gpu(&self) -> Option<ForwardedGpu<'_>> {
        let (Boot::Ubuntu(UbuntuBoot::WithGpu { image, allow_list, sensor_daemon, card })
        | Boot::OwnLinux { gpu: UbuntuBoot::WithGpu { image, allow_list, sensor_daemon, card }, .. }) =
            &self.boot
        else {
            return None;
        };
        Some(ForwardedGpu { image, allow_list, sensor_daemon, card })
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
        for node in gpu_nodes(devices, gpu.card) {
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
        on_card(0, "renderD129")
    }

    fn on_card(minor: u32, render_node: &str) -> Boot {
        Boot::Ubuntu(UbuntuBoot::WithGpu {
            image: "/images/driver.iso".into(),
            allow_list: "/p/linux/allow.txt".into(),
            sensor_daemon: "/q/nvfwd-sensord".into(),
            card: ForwardedCard { pci: "0000:02:00.0".into(), minor, render_node: Some(render_node.into()) },
        })
    }

    fn devices(scratch: &Scratch) -> PathBuf {
        for name in [
            "dev/nvidia0",
            "dev/nvidia1",
            "dev/nvidiactl",
            "dev/nvidia-uvm",
            "dev/nvidia-uvm-tools",
            "dev/nvidia-modeset",
            "dev/kvm",
            "dev/sda",
            "dev/dri/card0",
            "dev/dri/renderD128",
            "dev/dri/renderD129",
            "dev/dri/renderD130",
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
        let wanted: Vec<PathBuf> = ["nvidiactl", "nvidia-uvm", "nvidia0", "dri/renderD129"]
            .iter()
            .map(|node| folder.join(node))
            .collect();
        assert_eq!(nodes, wanted);
    }

    #[test]
    fn on_a_pc_with_two_cards_it_sees_only_the_chosen_one() {
        let scratch = Scratch::new();
        let folder = devices(&scratch);
        let mut rules = Plan::default();
        plan_of(on_card(1, "renderD130"), &scratch).grant_forwarding(&mut rules, &folder);
        let nodes = granted(&rules, Grant::Device);
        for node in ["nvidia1", "dri/renderD130", "nvidiactl", "nvidia-uvm"] {
            assert!(nodes.contains(&folder.join(node)), "{node}");
        }
        for node in ["nvidia0", "dri/renderD128", "dri/renderD129", "nvidia-uvm-tools", "nvidia-modeset"] {
            assert!(!nodes.contains(&folder.join(node)), "{node}");
        }
    }

    #[test]
    fn a_machine_of_your_own_gets_the_same_nodes_as_ubuntu() {
        let scratch = Scratch::new();
        let folder = devices(&scratch);
        let Boot::Ubuntu(gpu) = on_card(1, "renderD130") else { unreachable!() };
        let mut rules = Plan::default();
        plan_of(Boot::OwnLinux { iso: String::new(), gpu }, &scratch).grant_forwarding(&mut rules, &folder);
        let nodes = granted(&rules, Grant::Device);
        let wanted: Vec<PathBuf> = ["nvidiactl", "nvidia-uvm", "nvidia1", "dri/renderD130"]
            .iter()
            .map(|node| folder.join(node))
            .collect();
        assert_eq!(nodes, wanted);
    }

    #[test]
    fn a_node_that_is_not_there_is_not_granted() {
        let scratch = Scratch::new();
        let folder = devices(&scratch);
        let mut rules = Plan::default();
        plan_of(on_card(7, "renderD200"), &scratch).grant_forwarding(&mut rules, &folder);
        let nodes = granted(&rules, Grant::Device);
        assert_eq!(nodes, [folder.join("nvidiactl"), folder.join("nvidia-uvm")]);
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
