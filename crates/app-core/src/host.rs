//! What this PC can give a machine: its cores, its memory and its disk.

use crate::context::Context;
use crate::failure::Result;
use crate::machine::{Machine, sorted_names};
use crate::slots;
use guesthouse_daemon::slot::Slot;
use rustix::fs::statvfs;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub const GIB: u64 = 1 << 30;
/// The part of the shared window the guest can fill, on the same tmpfs as RAM.
pub const IO_SPACE_GIB: u64 = 8;
/// Running out of tmpfs kills QEMU with SIGBUS, so stop short of it.
pub const TMPFS_SPARE_GIB: u64 = 1;
/// The host keeps a share of its RAM for itself, never less than the least and never more than the
/// most. A guest's RAM is claimed whole and the GPU pins what it uses, so swap can't make room later.
const HOST_SHARE_DIVISOR: u64 = 4;
/// A desktop with a browser and Steam open, and the viewer and the daemon beside them.
pub const HOST_KEEPS_LEAST_GIB: u64 = 6;
/// What the host always kept, which on a large PC is plenty.
pub const HOST_KEEPS_MOST_GIB: u64 = 16;
const KIB: u64 = 1024;
/// The files QEMU keeps in shared memory.
pub const QEMU_FILES: [&str; 2] = ["ram", "iomem"];
const FILE_BLOCK: u64 = 512;

/// The numbers of `cpuN` folders.
fn cpu_numbers(root: &Path) -> Vec<u32> {
    sorted_names(root).iter().filter_map(|name| name.strip_prefix("cpu")?.parse().ok()).collect()
}

/// The CPUs with the larger L3, or nothing if they all have the same.
pub fn vcache_cpus(root: &Path) -> Vec<u32> {
    let mut sizes = BTreeMap::new();
    for cpu in cpu_numbers(root) {
        let text = fs::read_to_string(root.join(format!("cpu{cpu}/cache/index3/size"))).unwrap_or_default();
        if let Ok(size) = text.trim().trim_end_matches('K').parse::<u64>() {
            sizes.insert(cpu, size);
        }
    }
    let largest = sizes.values().copied().max();
    if sizes.values().collect::<BTreeSet<_>>().len() < 2 {
        return Vec::new();
    }
    sizes.iter().filter(|(_, size)| Some(**size) == largest).map(|(cpu, _)| *cpu).collect()
}

/// Writes CPUs the way taskset takes them: 0-7,16-23.
pub fn cpu_list(cpus: &[u32]) -> String {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &cpu in cpus {
        match runs.last_mut() {
            Some(run) if run.1 + 1 == cpu => run.1 = cpu,
            _ => runs.push((cpu, cpu)),
        }
    }
    let text = |(first, last): &(u32, u32)| {
        if first == last { first.to_string() } else { format!("{first}-{last}") }
    };
    runs.iter().map(text).collect::<Vec<_>>().join(",")
}

/// The most cores a machine can have: past this, vCPUs and the host would fight for them.
pub fn physical_cores(root: &Path) -> u32 {
    let mut cores = BTreeSet::new();
    for cpu in cpu_numbers(root) {
        let folder = root.join(format!("cpu{cpu}/topology"));
        let read = |name: &str| fs::read_to_string(folder.join(name)).ok();
        if let (Some(core), Some(package)) = (read("core_id"), read("physical_package_id")) {
            cores.insert((package, core));
        }
    }
    cores.len() as u32
}

/// What the host keeps of `total` bytes of RAM.
pub fn host_keeps(total: u64) -> u64 {
    (total / HOST_SHARE_DIVISOR).clamp(HOST_KEEPS_LEAST_GIB * GIB, HOST_KEEPS_MOST_GIB * GIB)
}

/// What a PC offers a GPU machine, and why, all in bytes. It comes from the PC's RAM, the tmpfs and
/// what the other VMs hold, which only change when a VM starts or stops, so it stays the same while
/// programs come and go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RamOffer {
    pub total: u64,
    pub keeps: u64,
    /// What the VMs in the other slots hold.
    pub others: u64,
    /// What the tmpfs leaves for guest RAM, which past its limit kills QEMU.
    pub tmpfs_room: u64,
    pub tmpfs_size: u64,
}

impl RamOffer {
    /// From the tmpfs's size and use, what this slot's VM has on it now, the PC's RAM, and what
    /// the other VMs hold.
    pub fn new(tmpfs_size: u64, tmpfs_used: u64, ours: u64, total: u64, others: u64) -> Self {
        let held_by_others = tmpfs_used.saturating_sub(ours);
        let tmpfs_room = tmpfs_size.saturating_sub(held_by_others + (IO_SPACE_GIB + TMPFS_SPARE_GIB) * GIB);
        Self { total, keeps: host_keeps(total), others, tmpfs_room, tmpfs_size }
    }

    fn ram_room(&self) -> u64 {
        self.total.saturating_sub(self.keeps + IO_SPACE_GIB * GIB + self.others)
    }

    /// The most RAM a GPU machine can have, in GiB.
    pub fn limit_gib(&self) -> u64 {
        self.ram_room().min(self.tmpfs_room) / GIB
    }
}

pub fn shared_memory_used(context: &Context, slot: Slot, names: &[&str]) -> u64 {
    let used = |name: &&str| fs::metadata(context.paths.shared_file(name, slot));
    names.iter().filter_map(|name| used(name).ok()).map(|found| found.blocks() * FILE_BLOCK).sum()
}

fn meminfo_bytes(context: &Context, field: &str) -> u64 {
    let text = fs::read_to_string(&context.meminfo).unwrap_or_default();
    let line = text.lines().find_map(|line| line.strip_prefix(field)?.strip_prefix(':'));
    let kib = line.and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok());
    kib.unwrap_or(0) * KIB
}

pub fn available_ram(context: &Context) -> u64 {
    meminfo_bytes(context, "MemAvailable")
}

pub fn total_ram(context: &Context) -> u64 {
    meminfo_bytes(context, "MemTotal")
}

/// What this PC offers a GPU machine in `slot`. Its own memory, if it's running, counts as free.
pub fn ram_offer(context: &Context, slot: Slot) -> Result<RamOffer> {
    let disk = statvfs(&context.paths.shared_memory).map_err(std::io::Error::from)?;
    let ours = shared_memory_used(context, slot, &QEMU_FILES);
    let others = slots::all()
        .filter(|other| *other != slot)
        .map(|other| shared_memory_used(context, other, &QEMU_FILES))
        .sum();
    let (size, free) = (disk.f_blocks * disk.f_frsize, disk.f_bfree * disk.f_frsize);
    Ok(RamOffer::new(size, size - free, ours, total_ram(context), others))
}

/// The most RAM a GPU machine can have.
pub fn host_ram_limit(context: &Context) -> Result<u64> {
    match context.ram_limit {
        Some(limit) => Ok(limit),
        None => Ok(ram_offer(context, context.slot)?.limit_gib()),
    }
}

/// The most RAM a machine with no GPU can have: ordinary memory, which swap can back.
pub fn plain_ram_limit(context: &Context) -> u64 {
    let total = total_ram(context);
    (total.saturating_sub(host_keeps(total)) / GIB).max(1)
}

/// The most RAM this machine can have. A GPU machine that's running counts its own as free.
pub fn ram_limit_for(context: &Context, machine: &Machine) -> Result<u64> {
    if !machine.has_gpu() {
        return Ok(plain_ram_limit(context));
    }
    if let Some(limit) = context.ram_limit {
        return Ok(limit);
    }
    let slot = crate::tasks::slot_of(context, machine).unwrap_or(context.slot);
    Ok(ram_offer(context, slot)?.limit_gib())
}

pub fn free_gib(path: &Path) -> Result<f64> {
    let disk = statvfs(path).map_err(std::io::Error::from)?;
    Ok((disk.f_bavail * disk.f_frsize) as f64 / GIB as f64)
}

#[cfg(test)]
pub mod stand_in {
    use crate::testing::Scratch;
    use std::path::PathBuf;

    /// CPUs with these L3 sizes, half of them the SMT siblings of the rest.
    pub fn fake_cpus(scratch: &Scratch, name: &str, sizes: &[u32]) -> PathBuf {
        for (cpu, size) in sizes.iter().enumerate() {
            scratch.write(&format!("{name}/cpu{cpu}/cache/index3/size"), format!("{size}K\n"));
            scratch.write(
                &format!("{name}/cpu{cpu}/topology/core_id"),
                format!("{}\n", cpu % (sizes.len() / 2)),
            );
            scratch.write(&format!("{name}/cpu{cpu}/topology/physical_package_id"), "0\n");
        }
        std::fs::create_dir_all(scratch.path(name).join("cpufreq")).unwrap();
        scratch.path(name)
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::fake_cpus;
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn the_larger_cache_is_found_as_on_a_two_die_cpu() {
        let scratch = Scratch::new();
        let sizes: Vec<u32> = [[98304; 8], [32768; 8], [98304; 8], [32768; 8]].concat();
        let root = fake_cpus(&scratch, "cpu", &sizes);
        let expected: Vec<u32> = (0..8).chain(16..24).collect();
        assert_eq!(vcache_cpus(&root), expected);
        assert_eq!(cpu_list(&vcache_cpus(&root)), "0-7,16-23");
        assert_eq!(physical_cores(&root), 16);
    }

    #[test]
    fn with_one_cache_size_there_is_nothing_to_pin_to() {
        let scratch = Scratch::new();
        assert!(vcache_cpus(&fake_cpus(&scratch, "flat", &[32768; 4])).is_empty());
        assert_eq!((cpu_list(&[3]), cpu_list(&[0, 1, 3])), ("3".to_owned(), "0-1,3".to_owned()));
    }

    /// What `MemTotal` says on a PC with this much RAM in it, less what the firmware and the
    /// kernel keep.
    const fn sold_with(gib: u64) -> u64 {
        gib * GIB * 97 / 100
    }

    /// A PC as Linux sets it up: `/dev/shm` holds half its RAM, and nothing else is on it.
    fn usual(total: u64) -> RamOffer {
        RamOffer::new(total / 2, 0, 0, total, 0)
    }

    #[test]
    fn the_host_keeps_a_quarter_but_never_less_than_a_desktop_needs_or_more_than_it_did() {
        assert_eq!(host_keeps(sold_with(16)), HOST_KEEPS_LEAST_GIB * GIB);
        assert_eq!(host_keeps(32 * GIB), 8 * GIB);
        assert_eq!(host_keeps(91 * GIB), HOST_KEEPS_MOST_GIB * GIB);
    }

    #[test]
    fn what_a_pc_offers_follows_its_ram_and_its_tmpfs() {
        assert_eq!(
            usual(sold_with(16)).limit_gib(),
            0,
            "a 16 GB PC: its 7.8 GiB tmpfs is short of the IO space"
        );
        assert_eq!(RamOffer::new(16 * GIB, 65 << 20, 0, 30 * GIB, 0).limit_gib(), 6, "issue 9's PC");
        assert_eq!(usual(sold_with(32)).limit_gib(), 6);
        assert_eq!(
            RamOffer::new(46 * GIB, 0, 0, 91 * GIB, 0).limit_gib(),
            37,
            "the tmpfs, less the IO space"
        );
        assert_eq!(RamOffer::new(46 * GIB, 0, 0, 40 * GIB, 0).limit_gib(), 22, "the host keeps its share");
        assert_eq!(usual(sold_with(8)).limit_gib(), 0);
    }

    #[test]
    fn the_other_vms_count_and_this_slots_own_does_not() {
        let alone = RamOffer::new(46 * GIB, 17 * GIB, 17 * GIB, 91 * GIB, 0);
        assert_eq!(alone.limit_gib(), 37, "the VM's own files do not count");
        let beside = RamOffer::new(46 * GIB, 17 * GIB, 0, 91 * GIB, 17 * GIB);
        assert_eq!(beside.limit_gib(), 20, "another VM's do, on the tmpfs and in RAM");
        assert_eq!(RamOffer::new(46 * GIB, 0, 0, 40 * GIB, 20 * GIB).limit_gib(), 2);
    }
}
