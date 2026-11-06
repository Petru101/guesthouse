//! What this PC can give a machine: its cores, its memory and its disk.

use crate::context::Context;
use crate::failure::Result;
use crate::machine::{Machine, sorted_names};
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
/// The GPU pins guest pages, so swap can't make room for the host.
pub const HOST_KEEPS_GIB: u64 = 16;
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

/// The most RAM a machine can have, in GiB. All in bytes; `ours` is what the VM has on tmpfs now.
///
/// Guest RAM and the IO space share the tmpfs, and past its limit QEMU dies. The IO space can reach
/// 8 GiB, though the rest of the 64 GiB window file is never touched.
pub fn ram_limit(tmpfs_size: u64, tmpfs_used: u64, ours: u64, available: u64) -> u64 {
    let room = (tmpfs_size + ours) as i128 - tmpfs_used as i128;
    let tmpfs_room = room - ((IO_SPACE_GIB + TMPFS_SPARE_GIB) * GIB) as i128;
    let host_room = (available + ours) as i128 - ((IO_SPACE_GIB + HOST_KEEPS_GIB) * GIB) as i128;
    (tmpfs_room.min(host_room).div_euclid(GIB.into())).max(0) as u64
}

pub fn shared_memory_used(context: &Context, slot: Slot, names: &[&str]) -> u64 {
    let used = |name: &&str| fs::metadata(context.paths.shared_file(name, slot));
    names.iter().filter_map(|name| used(name).ok()).map(|found| found.blocks() * FILE_BLOCK).sum()
}

pub fn available_ram(context: &Context) -> u64 {
    let text = fs::read_to_string(&context.meminfo).unwrap_or_default();
    let line = text.lines().find_map(|line| line.strip_prefix("MemAvailable:"));
    let kib = line.and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok());
    kib.unwrap_or(0) * 1024
}

/// The most RAM a GPU machine can have now.
pub fn host_ram_limit(context: &Context) -> Result<u64> {
    if let Some(limit) = context.ram_limit {
        return Ok(limit);
    }
    let disk = statvfs(&context.paths.shared_memory).map_err(std::io::Error::from)?;
    let ours = shared_memory_used(context, context.slot, &QEMU_FILES);
    let (size, free) = (disk.f_blocks * disk.f_frsize, disk.f_bfree * disk.f_frsize);
    Ok(ram_limit(size, size - free, ours, available_ram(context)))
}

/// The most RAM a machine with no GPU can have: ordinary memory, which swap can back.
pub fn plain_ram_limit(context: &Context) -> u64 {
    (available_ram(context).saturating_sub(HOST_KEEPS_GIB * GIB) / GIB).max(1)
}

pub fn ram_limit_for(context: &Context, machine: &Machine) -> Result<u64> {
    if machine.has_gpu() { host_ram_limit(context) } else { Ok(plain_ram_limit(context)) }
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

    #[test]
    fn the_ram_limit_counts_what_shares_the_tmpfs() {
        assert_eq!(ram_limit(46 * GIB, 0, 0, 90 * GIB), 37, "the tmpfs limits it, less the IO space");
        assert_eq!(ram_limit(46 * GIB, 17 * GIB, 17 * GIB, 70 * GIB), 37, "the VM's own files do not count");
        assert_eq!(ram_limit(46 * GIB, 10 * GIB, 0, 90 * GIB), 27, "someone else's tmpfs files do");
        assert_eq!(ram_limit(46 * GIB, 0, 0, 40 * GIB), 16, "the host keeps its share");
        assert_eq!(ram_limit(46 * GIB, 0, 0, 10 * GIB), 0);
    }
}
