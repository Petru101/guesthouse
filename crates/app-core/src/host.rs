//! What this PC can give a machine: its cores, its memory and its disk.

use crate::context::Context;
use crate::failure::Result;
use crate::guest_memory::NO_ROOM_WORDS;
use crate::machine::{Machine, sorted_names};
use crate::slots;
use guesthouse_daemon::slot::Slot;
use rustix::fs::statvfs;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub const GIB: u64 = 1 << 30;
/// What a GPU guest's IO space is counted at, on the same tmpfs as RAM and in the PC's RAM. It fills
/// only as far as the guest's locked allocations go, which on Windows desktops was 130 to 270 MiB.
/// Games can lock more, and a full tmpfs stops the VM, so this keeps several times that.
pub const IO_SPACE_GIB: u64 = 2;
/// Running out of tmpfs kills QEMU with SIGBUS, so stop short of it.
pub const TMPFS_SPARE_GIB: u64 = 1;
/// The host keeps a share of its RAM for itself, never less than the least and never more than the
/// most. A guest's RAM is claimed whole and the GPU pins what it uses, so swap can't make room later.
const HOST_SHARE_DIVISOR: u64 = 4;
/// A desktop with a browser and Steam open, and the viewer and the daemon beside them.
pub const HOST_KEEPS_LEAST_GIB: u64 = 6;
/// What the host always kept, which on a large PC is plenty.
pub const HOST_KEEPS_MOST_GIB: u64 = 16;
/// What has to stay free when a GPU machine starts, beside its RAM and IO space, so the host isn't left
/// with nothing the moment the guest's RAM is claimed.
pub const FREE_AT_START_GIB: u64 = 2;
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

    fn tmpfs_binds(&self) -> bool {
        self.tmpfs_room < self.ram_room()
    }

    /// Where the RAM goes, as a sentence or two.
    fn reasons(&self) -> String {
        let mut text = format!(
            "This PC has {} GiB of RAM and keeps {} GiB for itself",
            self.total / GIB,
            self.keeps.div_ceil(GIB)
        );
        if self.others >= GIB / 2 {
            text += &format!(", and the VMs running hold {} GiB", self.others.div_ceil(GIB));
        }
        text += ".";
        if self.tmpfs_binds() {
            text += &format!(
                " Guest memory lives in /dev/shm, which holds {} GiB and keeps {} GiB of that for the GPU's IO space and a margin.",
                self.tmpfs_size / GIB,
                IO_SPACE_GIB + TMPFS_SPARE_GIB
            );
        }
        text
    }

    /// What a RAM setting says under it, for a machine that needs at least `least` GiB.
    pub fn explanation(&self, least: u64) -> String {
        let limit = self.limit_gib();
        if limit >= least {
            return format!("Up to {limit} GiB. {}", self.reasons());
        }
        let way_out = if self.tmpfs_binds() {
            let taken = (self.tmpfs_size - self.tmpfs_room).div_ceil(GIB);
            format!("Make it larger: sudo mount -o remount,size={}G /dev/shm", least + taken)
        } else if self.others >= GIB / 2 {
            "Stop another VM to make room.".to_owned()
        } else {
            "A GPU machine needs a PC with more RAM.".to_owned()
        };
        format!(
            "A GPU machine needs at least {least} GiB, and this PC can give one {limit} GiB. {} {way_out}",
            self.reasons()
        )
    }
}

/// How a RAM setting is offered. A machine that couldn't start isn't offered at all: the setting
/// stays at the least and says why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RamField {
    pub lower: u64,
    pub upper: u64,
    pub value: u64,
    /// Whether a machine of the least size fits at all.
    pub fits: bool,
    pub subtitle: String,
}

impl RamField {
    /// All in GiB: the limit, the least the machine needs, and what it would like.
    pub fn new(limit: u64, least: u64, wanted: u64, explanation: String) -> Self {
        let fits = limit >= least;
        let upper = if fits { limit } else { least };
        Self { lower: least, upper, value: wanted.clamp(least, upper), fits, subtitle: explanation }
    }

    /// The setting for a GPU machine in `slot`.
    pub fn for_gpu(context: &Context, slot: Slot, least: u64, wanted: u64) -> Self {
        let offer = match context.ram_limit {
            Some(limit) => Ok((limit, format!("Up to {limit} GiB."))),
            None => ram_offer(context, slot).map(|offer| (offer.limit_gib(), offer.explanation(least))),
        };
        let (limit, explanation) =
            offer.unwrap_or_else(|error| (0, format!("This PC's memory can't be read: {error}")));
        Self::new(limit, least, wanted, explanation)
    }

    /// The setting for a machine with no GPU.
    pub fn for_plain(context: &Context, least: u64, wanted: u64) -> Self {
        let limit = plain_ram_limit(context);
        Self::new(limit, least, wanted, format!("Up to {limit} GiB."))
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

/// Whether `ram_bytes` of guest RAM fits in what's really free now. The page cache and the programs
/// open move that from one moment to the next, so it's only asked at a start. `None` is yes, and
/// otherwise it says what's short and what to do.
pub fn fits_now(context: &Context, ram_bytes: u64) -> Option<String> {
    if context.ram_limit.is_some_and(|limit| ram_bytes <= limit * GIB) {
        return None;
    }
    let available = available_ram(context);
    let needed = ram_bytes + (IO_SPACE_GIB + FREE_AT_START_GIB) * GIB;
    (needed > available).then(|| short_now(ram_bytes, available))
}

fn short_now(ram_bytes: u64, available: u64) -> String {
    format!(
        "This machine needs {} GiB of RAM, and {} {NO_ROOM_WORDS} right now, with {} GiB of that kept for the \
         GPU and the PC. Close some programs, like a browser or a game, or stop another VM, and start it \
         again. Or give it less RAM.",
        ram_bytes.div_ceil(GIB),
        available / GIB,
        IO_SPACE_GIB + FREE_AT_START_GIB,
    )
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
        assert_eq!(usual(sold_with(16)).limit_gib(), 4, "a 16 GB PC, held to its 7.8 GiB tmpfs");
        assert_eq!(RamOffer::new(16 * GIB, 65 << 20, 0, 30 * GIB, 0).limit_gib(), 12, "issue 9's PC");
        assert_eq!(usual(sold_with(32)).limit_gib(), 12);
        assert_eq!(
            RamOffer::new(46 * GIB, 0, 0, 91 * GIB, 0).limit_gib(),
            43,
            "the tmpfs, less the IO space"
        );
        assert_eq!(RamOffer::new(46 * GIB, 0, 0, 40 * GIB, 0).limit_gib(), 28, "the host keeps its share");
        assert_eq!(usual(sold_with(8)).limit_gib(), 0);
    }

    #[test]
    fn a_low_offer_says_why_and_what_to_do() {
        let small = RamOffer::new(32 * GIB, 0, 0, sold_with(8), 0).explanation(4);
        assert!(
            small.starts_with("A GPU machine needs at least 4 GiB, and this PC can give one 0 GiB."),
            "{small}"
        );
        assert!(small.contains("This PC has 7 GiB of RAM and keeps 6 GiB for itself."), "{small}");
        assert!(small.ends_with("needs a PC with more RAM."), "{small}");
        let crowded = RamOffer::new(46 * GIB, 0, 0, 16 * GIB, 6 * GIB).explanation(4);
        assert!(
            crowded.contains("the VMs running hold 6 GiB")
                && crowded.ends_with("Stop another VM to make room.")
        );
        let tmpfs = RamOffer::new(5 * GIB, 0, 0, 32 * GIB, 0).explanation(4);
        assert!(
            tmpfs.contains("/dev/shm, which holds 5 GiB") && tmpfs.ends_with("remount,size=7G /dev/shm"),
            "{tmpfs}"
        );
        let fine = RamOffer::new(16 * GIB, 65 << 20, 0, 30 * GIB, 0).explanation(4);
        assert!(fine.starts_with("Up to 12 GiB. This PC has 30 GiB of RAM and keeps 8 GiB"), "{fine}");
    }

    #[test]
    fn a_setting_never_offers_a_machine_that_cant_start() {
        let short = RamField::new(2, 4, 16, "why".into());
        assert_eq!((short.lower, short.upper, short.value, short.fits), (4, 4, 4, false));
        let roomy = RamField::new(21, 4, 16, "fine".into());
        assert_eq!((roomy.lower, roomy.upper, roomy.value, roomy.fits), (4, 21, 16, true));
        assert_eq!(RamField::new(8, 4, 16, String::new()).value, 8, "what it would like, as far as it goes");
    }

    #[test]
    fn a_gpu_setting_stays_the_same_however_little_is_free_this_second() {
        let scratch = Scratch::new();
        let mut context = crate::testing::scratch_context(&scratch);
        context.ram_limit = None;
        context.meminfo = scratch.write("meminfo", "MemTotal: 8126464 kB\nMemAvailable: 6000000 kB\n");
        let field = RamField::for_gpu(&context, crate::slots::first(), 4, 16);
        assert!(!field.fits && field.subtitle.starts_with("A GPU machine needs at least 4 GiB"), "{field:?}");
        context.meminfo = scratch.write("meminfo", "MemTotal: 31457280 kB\nMemAvailable: 1000000 kB\n");
        let field = RamField::for_gpu(&context, crate::slots::first(), 4, 16);
        assert!(field.fits, "{field:?}");
        assert_eq!(field.lower, 4);
    }

    #[test]
    fn a_start_is_checked_against_what_is_free_then() {
        let scratch = Scratch::new();
        let mut context = crate::testing::scratch_context(&scratch);
        context.ram_limit = None;
        context.meminfo = scratch.write("meminfo", "MemTotal: 31457280 kB\nMemAvailable: 26214400 kB\n");
        assert_eq!(fits_now(&context, 12 * GIB), None);
        assert_eq!(fits_now(&context, 21 * GIB), None, "25 GiB free holds 21 with 4 to spare");
        let short = fits_now(&context, 22 * GIB).unwrap();
        assert!(
            short.starts_with("This machine needs 22 GiB of RAM, and 25 GiB is free right now"),
            "{short}"
        );
        assert!(short.contains("Close some programs"), "{short}");
        assert!(crate::guest_memory::is_no_room(&crate::Failure(short)));
    }

    #[test]
    fn the_other_vms_count_and_this_slots_own_does_not() {
        let alone = RamOffer::new(46 * GIB, 17 * GIB, 17 * GIB, 91 * GIB, 0);
        assert_eq!(alone.limit_gib(), 43, "the VM's own files do not count");
        let beside = RamOffer::new(46 * GIB, 17 * GIB, 0, 91 * GIB, 17 * GIB);
        assert_eq!(beside.limit_gib(), 26, "another VM's do, on the tmpfs and in RAM");
        assert_eq!(RamOffer::new(46 * GIB, 0, 0, 40 * GIB, 20 * GIB).limit_gib(), 8);
    }
}
