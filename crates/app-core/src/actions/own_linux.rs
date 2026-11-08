//! Starting a plain machine that holds a Linux the user installed, with the forwarded GPU turned on in
//! its settings. The host's side is the Ubuntu route's: the same checks, image and jail, and anything
//! in doubt only costs the GPU. Inside, setup-own.sh from the image does the rest.

use super::ubuntu::{gpu_boot, jailed};
use super::{Task, plan_for};
use crate::failure::{Failure, Result};
use crate::start_checks::start_problems;
use crate::vm_args::Boot;

pub fn start(task: &mut Task) -> Result<()> {
    if let Some(problem) = start_problems(task.context, &task.machine)?.into_iter().next() {
        return Err(Failure(problem));
    }
    let gpu = gpu_boot(task)?;
    let boot = Boot::OwnLinux { iso: task.machine.iso.clone(), gpu };
    let plan = jailed(task, plan_for(task.context, &task.machine, boot))?;
    task.step("Starting the VM");
    task.host.start(&plan, &task.machine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{Kind, Machine};
    use crate::network::Mode;
    use crate::testing::{Scratch, scratch_context};
    use crate::vm_args::UbuntuBoot;
    use crate::vm_host::stand_in::RecordingHost;
    use std::fs;

    fn fedora(context: &crate::context::Context) -> Machine {
        let folder = context.paths.machines.join("fedora");
        fs::create_dir_all(&folder).unwrap();
        let (disk, vars) = (folder.join("disk.qcow2"), folder.join("vars.fd"));
        fs::write(&disk, "").unwrap();
        fs::write(&vars, "").unwrap();
        let machine = Machine {
            kind: Kind::Plain,
            linux_gpu: true,
            cores: 2,
            ram_gib: 2,
            ..Machine::new("fedora", "Fedora", &disk.to_string_lossy(), &vars.to_string_lossy())
        };
        machine.save(context).unwrap();
        machine
    }

    fn roomy(scratch: &Scratch) -> crate::context::Context {
        let mut context = scratch_context(scratch);
        context.cpus = crate::host::stand_in::fake_cpus(scratch, "cpus", &[32768; 8]);
        context.meminfo =
            scratch.write("meminfo", "MemTotal:       100000000 kB\nMemAvailable:   100000000 kB\n");
        context
    }

    #[test]
    fn it_starts_its_own_line_and_a_qemu_with_no_device_only_costs_the_gpu() {
        let scratch = Scratch::new();
        let context = roomy(&scratch);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", fedora(&context));
        start(&mut task).unwrap();
        let plans = host.plans.lock().unwrap();
        let Boot::OwnLinux { gpu: UbuntuBoot::NoGpu { reason_file }, .. } = &plans[0].boot else {
            panic!("it should have started with no GPU: {:?}", plans[0].boot)
        };
        assert!(fs::read_to_string(reason_file).unwrap().contains("no nvfwd device"));
        assert!(task.error.starts_with("Starting without the GPU: "), "{}", task.error);
    }

    fn a_kernel_with_landlock(_: &crate::vm_args::VmPlan) -> crate::jailing::Offer {
        crate::jailing::Offer {
            support: guesthouse_jail::Support { landlock_abi: 10, seccomp: true },
            ..crate::jailing::Offer::nothing()
        }
    }

    #[test]
    fn passt_a_bridge_and_a_shared_folder_keep_the_gpu_in_its_jail() {
        let scratch = Scratch::new();
        let mut context = roomy(&scratch);
        context.jail_offer = a_kernel_with_landlock;
        for change in [
            (|m: &mut Machine| m.network_mode = Mode::Bridged) as fn(&mut Machine),
            |m| m.passt = true,
            |m| m.share = "/srv/shared".into(),
        ] {
            let mut machine = fedora(&context);
            change(&mut machine);
            let host = RecordingHost::default();
            let mut task = Task::new(&context, &host, "start", machine);
            let gpu = UbuntuBoot::WithGpu {
                image: "/store/driver.iso".into(),
                allow_list: "/p/allow.txt".into(),
                sensor_daemon: "/p/nvfwd-sensord".into(),
                card: crate::linux::card::ForwardedCard {
                    pci: "0000:02:00.0".into(),
                    minor: 0,
                    render_node: None,
                },
            };
            let boot = Boot::OwnLinux { iso: String::new(), gpu };
            let wanted = plan_for(&context, &task.machine, boot);
            let plan = jailed(&mut task, wanted).unwrap();
            assert!(plan.has_forwarded_gpu() && plan.jail.is_some(), "{:?}", task.machine);
            assert_eq!(task.error, "");
        }
    }
}
