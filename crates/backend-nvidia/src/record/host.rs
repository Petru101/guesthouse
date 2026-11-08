//! What the record says about the host itself.

use guesthouse_backend::PciAddress;
use std::fs;
use std::process::Command;

const MEMORY_INFO: &str = "/proc/meminfo";
const CPU_INFO: &str = "/proc/cpuinfo";
const NOMINAL_FREQUENCY: &str = "/sys/devices/system/cpu/cpu0/acpi_cppc/nominal_freq";
const SMI: &str = "nvidia-smi";
const HERTZ_PER_MEGAHERTZ: u64 = 1_000_000;

/// See the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFacts {
    pub total_ram_kib: u64,
    pub has_avx: bool,
    /// The CPU's nominal clock in megahertz, where the host says.
    pub cpu_megahertz: u32,
    /// The card's rated graphics and memory clocks in hertz.
    pub rated_clocks: (u64, u64),
}

fn first_number(text: &str) -> Option<u64> {
    text.split_whitespace().nth(1)?.parse().ok()
}

/// `nvidia-smi` takes a PCI address for `--id`. Its own first card stands in when the address isn't
/// known, which is only sure to be the right one on a PC with one card.
fn smi_id(address: Option<&PciAddress>) -> String {
    format!("--id={}", address.map_or_else(|| "0".to_owned(), PciAddress::to_string))
}

fn rated_clocks(address: Option<&PciAddress>) -> Result<(u64, u64), String> {
    let output = Command::new(SMI)
        .args([
            "--query-gpu=clocks.max.graphics,clocks.max.memory",
            "--format=csv,noheader,nounits",
            &smi_id(address),
        ])
        .output()
        .map_err(|error| format!("Reading the card's rated clocks needs {SMI}: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut clocks = text.split(',').map(|clock| clock.trim().parse::<u64>());
    match (clocks.next(), clocks.next()) {
        (Some(Ok(graphics)), Some(Ok(memory))) => {
            Ok((graphics * HERTZ_PER_MEGAHERTZ, memory * HERTZ_PER_MEGAHERTZ))
        }
        _ => Err(format!("{SMI} did not give the card's clocks")),
    }
}

/// Reads the host's facts, with the clocks of the card at this address. This runs the vendor's own
/// command-line tool for the clocks.
pub fn read_host_facts(address: Option<&PciAddress>) -> Result<HostFacts, String> {
    read_host_facts_with_clocks(rated_clocks(address)?)
}

/// Reads what the host says about itself, given the card's clocks.
pub fn read_host_facts_with_clocks(rated_clocks: (u64, u64)) -> Result<HostFacts, String> {
    let memory = fs::read_to_string(MEMORY_INFO).map_err(|error| error.to_string())?;
    let cpu = fs::read_to_string(CPU_INFO).map_err(|error| error.to_string())?;
    let nominal = fs::read_to_string(NOMINAL_FREQUENCY).ok();
    Ok(HostFacts {
        total_ram_kib: first_number(&memory).ok_or("no memory total in the host's memory info")?,
        has_avx: cpu.contains(" avx "),
        cpu_megahertz: nominal.and_then(|text| text.trim().parse().ok()).unwrap_or(0),
        rated_clocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clocks_are_asked_of_the_card_at_the_address() {
        let address = PciAddress::parse("0000:02:00.0").unwrap();
        assert_eq!(smi_id(Some(&address)), "--id=0000:02:00.0");
        assert_eq!(smi_id(None), "--id=0", "as before, when the address isn't known");
    }

    #[test]
    fn the_total_is_the_first_line() {
        assert_eq!(first_number("MemTotal:   1024 kB\nMemFree: 1 kB\n"), Some(1024));
    }
}
