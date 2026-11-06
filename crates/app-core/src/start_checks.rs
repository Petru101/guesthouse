//! Why a machine can't start on this PC now. The judging is plain data in and words out, so a test
//! can state any host it likes.

use crate::context::Context;
use crate::failure::Result;
use crate::host::{self, GIB, QEMU_FILES, physical_cores, ram_limit_for, shared_memory_used, vcache_cpus};
use crate::machine::Machine;
use crate::network_settings::{network_start_problems, other_machines};
use crate::processes::{memory_taken, qemu_pid, running_slots};
use crate::settings::max_vram_gib;
use crate::slots;
use crate::tasks::no_slot_text;
use std::path::Path;

pub const ALREADY_RUNNING: &str = "This machine is running.";
pub const MEMORY_TAKEN: &str =
    "Another VM on this PC has just taken this slot's shared memory. Start the machine again.";
pub const SECOND_INSTANCE_REFUSAL: &str = "A second instance leaves GPU-PV alone.";

/// What several machines share, read once from the PC.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedFacts {
    /// The most RAM this machine can have on this PC now.
    pub ram_limit: u64,
    /// How much of the other VMs' RAM is already in shared memory, in GiB.
    pub touched_gib: u64,
    pub max_vram_gib: u32,
    pub vcache_cpus: u32,
}

impl SharedFacts {
    pub fn read(context: &Context, machine: &Machine) -> Result<Self> {
        let running = running_slots(context);
        let others = running.keys().filter(|slot| **slot != context.slot.number());
        let touched: u64 =
            others.map(|slot| shared_memory_used(context, slots::number(*slot), &QEMU_FILES[..1])).sum();
        Ok(Self {
            ram_limit: ram_limit_for(context, machine)?,
            touched_gib: touched / GIB,
            max_vram_gib: max_vram_gib(context),
            vcache_cpus: vcache_cpus(&context.cpus).len() as u32,
        })
    }
}

/// What the machines running now leave of the card. One with no cap has all of it.
pub fn video_memory_left(others: &[Machine], max_vram_gib: u32) -> u32 {
    let gpu = others.iter().filter(|other| other.has_gpu());
    let used: u32 = gpu.map(|other| if other.vram_gib > 0 { other.vram_gib } else { max_vram_gib }).sum();
    max_vram_gib.saturating_sub(used)
}

/// What several machines share: the memory that holds them, and the card's. None of it counts if
/// this one runs alone, so a single VM is told what it always was.
pub fn shared_problems(machine: &Machine, others: &[Machine], facts: &SharedFacts) -> Vec<String> {
    if others.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    // RAM they haven't touched yet will still come out of the same tmpfs. Their IO windows count as
    // far as they have gone: reserving 8 GiB each would stop two 16 GiB guests on a 46 GiB tmpfs.
    let reserved: u64 =
        others.iter().filter(|other| other.has_gpu()).map(|other| u64::from(other.ram_gib)).sum();
    let untouched = reserved.saturating_sub(facts.touched_gib);
    if machine.has_gpu() && u64::from(machine.ram_gib) + untouched > facts.ram_limit {
        found.push("This PC's shared memory cannot hold this machine beside the ones running.".to_owned());
    }
    if machine.has_gpu() {
        let left = video_memory_left(others, facts.max_vram_gib);
        if left == 0 {
            found.push(
                "The running machine has all the video memory. Give it a cap in its settings first."
                    .to_owned(),
            );
        } else if machine.vram_gib > left {
            found.push(format!("Only {left} GiB of video memory is left beside the machines running."));
        }
    }
    let pinned: u32 = others.iter().filter(|other| other.vcache_pin).map(|other| other.cores).sum();
    if machine.vcache_pin && pinned + machine.cores > facts.vcache_cpus {
        found.push("The V-Cache cores are taken by the machines running.".to_owned());
    }
    found
}

/// The cap a machine starts with: its own, or what the others leave. Alone it needs none.
pub fn video_cap(machine: &Machine, others: &[Machine], max_vram_gib: u32) -> u32 {
    match (machine.vram_gib, others.is_empty()) {
        (0, false) => video_memory_left(others, max_vram_gib),
        (cap, _) => cap,
    }
}

/// Says why this machine can't start on this host now.
pub fn start_problems(context: &Context, machine: &Machine) -> Result<Vec<String>> {
    let mut found = machine.problems(max_vram_gib(context));
    for path in [&machine.disk, &machine.vars] {
        if !path.is_empty() && !Path::new(path).exists() {
            found.push(format!("{path} is missing."));
        }
    }
    if !machine.iso.is_empty() && !Path::new(&machine.iso).exists() {
        found.push(format!("{} is missing. Eject it in the settings to boot from the disk.", machine.iso));
    }
    if !machine.share.is_empty() && !Path::new(&machine.share).is_dir() {
        found.push(format!("{} is missing. Choose another shared folder in the settings.", machine.share));
    }
    if machine.has_gpu() && context.second_instance() {
        found.push(SECOND_INSTANCE_REFUSAL.to_owned());
    }
    let cores = physical_cores(&context.cpus);
    if machine.cores > cores {
        found.push(format!("This PC has {cores} cores, so a machine can have that many at most."));
    }
    let limit = ram_limit_for(context, machine)?;
    if u64::from(machine.ram_gib) > limit {
        found.push(format!("This PC can give a machine {limit} GiB at most right now."));
    }
    if machine.vcache_pin {
        let cpus = host::vcache_cpus(&context.cpus);
        if cpus.is_empty() {
            found.push("This CPU has no V-Cache cores to pin to.".to_owned());
        } else if machine.cores as usize > cpus.len() {
            found.push(format!(
                "The V-Cache cores run {} threads, too few for {} cores.",
                cpus.len(),
                machine.cores
            ));
        }
    }
    let others = other_machines(context);
    if others.iter().any(|other| other.disk == machine.disk) {
        found.push(ALREADY_RUNNING.to_owned());
    }
    if qemu_pid(context, None).is_some() {
        found.push(no_slot_text());
    } else if machine.has_gpu() && memory_taken(context, None) {
        found.push(MEMORY_TAKEN.to_owned());
    }
    found.extend(shared_problems(machine, &others, &SharedFacts::read(context, machine)?));
    found.extend(network_start_problems(context, machine, &others));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::stand_in::fake_cpus;
    use crate::machine::Kind;
    use crate::processes::stand_in::fake_process;
    use crate::testing::{Scratch, scratch_context};
    use std::fs;

    fn machine(id: &str) -> Machine {
        Machine::new(id, id, &format!("/{id}"), "/v")
    }

    fn facts() -> SharedFacts {
        SharedFacts { ram_limit: 36, touched_gib: 0, max_vram_gib: 28, vcache_cpus: 0 }
    }

    fn says(found: &[String], text: &str) -> bool {
        found.iter().any(|problem| problem.contains(text))
    }

    #[test]
    fn one_with_no_cap_has_all_the_card() {
        let (first, second) = (machine("first"), machine("second"));
        let others = [first.clone()];
        assert_eq!(video_memory_left(&others, 28), 0);
        assert!(says(&shared_problems(&second, &others, &facts()), "all the video memory"));
    }

    #[test]
    fn a_cap_leaves_the_rest_for_the_next_machine() {
        let others = [Machine { vram_gib: 10, ..machine("first") }];
        let mut second = machine("second");
        assert_eq!(video_memory_left(&others, 28), 18);
        assert!(shared_problems(&second, &others, &facts()).is_empty());
        assert_eq!(video_cap(&second, &others, 28), 18, "what the first leaves");
        second.vram_gib = 20;
        assert!(says(&shared_problems(&second, &others, &facts()), "18 GiB"));
    }

    #[test]
    fn each_machine_counts_in_full_against_shared_memory() {
        let others = [Machine { vram_gib: 10, ..machine("first") }];
        let small = SharedFacts { ram_limit: 30, ..facts() };
        assert!(says(&shared_problems(&machine("second"), &others, &small), "shared memory"));
        assert!(shared_problems(&machine("second"), &others, &facts()).is_empty());
        let touched = SharedFacts { touched_gib: 16, ..small };
        assert!(
            shared_problems(&machine("second"), &others, &touched).is_empty(),
            "what is touched is in the limit"
        );
    }

    #[test]
    fn the_vcache_cores_count_together() {
        let others = [Machine { vcache_pin: true, vram_gib: 10, ..machine("first") }];
        let second = Machine { vcache_pin: true, cores: 9, ..machine("second") };
        let with_cores = SharedFacts { vcache_cpus: 16, ..facts() };
        assert!(says(&shared_problems(&second, &others, &with_cores), "V-Cache"));
    }

    #[test]
    fn alone_nothing_is_shared() {
        assert!(shared_problems(&machine("second"), &[], &facts()).is_empty());
        assert_eq!(video_cap(&machine("second"), &[], 28), 0);
    }

    fn startable(scratch: &Scratch) -> (Context, Machine) {
        let mut context = scratch_context(scratch);
        context.cpus = fake_cpus(scratch, "cpus", &[32768; 16]);
        context.meminfo = scratch.write("meminfo", "MemAvailable:   100000000 kB\n");
        let disk = scratch.write("disk", "").display().to_string();
        let vars = scratch.write("vars", "").display().to_string();
        let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", &disk, &vars) };
        (context, machine)
    }

    #[test]
    fn a_ready_plain_machine_has_no_problems() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        assert_eq!(start_problems(&context, &machine).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn missing_things_are_named() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let gone = scratch.path("gone").display().to_string();
        let problems = |machine: &Machine| start_problems(&context, machine).unwrap();
        assert!(says(&problems(&Machine { disk: gone.clone(), ..machine.clone() }), "is missing"));
        assert!(says(&problems(&Machine { iso: gone.clone(), ..machine.clone() }), "Eject it"));
        assert!(says(&problems(&Machine { share: gone, ..machine.clone() }), "shared folder"));
    }

    #[test]
    fn more_than_the_pc_has_is_refused() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let found = start_problems(&context, &Machine { cores: 9, ram_gib: 1000, ..machine }).unwrap();
        assert!(says(&found, "8 cores") && says(&found, "GiB at most"));
    }

    #[test]
    fn pinning_needs_vcache_cores() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let found = start_problems(&context, &Machine { vcache_pin: true, ..machine }).unwrap();
        assert!(says(&found, "no V-Cache"));
    }

    #[test]
    fn a_second_instance_leaves_the_gpu_alone() {
        let scratch = Scratch::new();
        let (mut context, machine) = startable(&scratch);
        context.paths.own_run = Some(scratch.path("second"));
        let gpu = Machine { kind: Kind::Windows, ..machine };
        assert!(says(&start_problems(&context, &gpu).unwrap(), "leaves GPU-PV alone"));
    }

    #[test]
    fn memory_another_vm_took_says_so_only_for_a_gpu_machine() {
        let scratch = Scratch::new();
        let (context, machine) = startable(&scratch);
        let memory = context.paths.shared_file("ram", slots::first());
        fs::write(&memory, "").unwrap();
        fake_process(
            &context,
            200,
            &["/other/guesthouse-daemon", "/other"],
            &[],
            &memory.display().to_string(),
        );
        let gpu = Machine { kind: Kind::Windows, ..machine.clone() };
        // The fake meminfo says nothing about the fake shared memory's real size, so only the words count.
        assert!(says(&start_problems(&context, &gpu).unwrap(), MEMORY_TAKEN));
        assert!(
            !says(&start_problems(&context, &machine).unwrap(), MEMORY_TAKEN),
            "a plain VM does not use it"
        );
    }
}
