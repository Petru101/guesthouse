//! What the code needs from the PC, in one value that a test can fill with stand-ins.

use crate::agent::{GuestAgent, TcpAgent};
use crate::bridge::BridgeHost;
use crate::clock::{Clock, RealClock};
use crate::monitor::{Monitor, QmpMonitor};
use crate::paths::Paths;
use guesthouse_daemon::slot::Slot;
use std::path::PathBuf;
use std::sync::Arc;

/// The variable that names the slot of a task.
pub const SLOT_VARIABLE: &str = guesthouse_daemon::slot::VARIABLE;

#[derive(Clone)]
pub struct Context {
    pub paths: Paths,
    /// This process's slot. A task has its VM's. The window has the first, and names others.
    pub slot: Slot,
    pub processes: PathBuf,
    pub cpus: PathBuf,
    pub usb_devices: PathBuf,
    pub usb_nodes: PathBuf,
    pub pci_devices: PathBuf,
    pub pci_names: PathBuf,
    pub kvm: PathBuf,
    pub driver_version_file: PathBuf,
    pub meminfo: PathBuf,
    /// Stands in for what this PC can give a GPU machine, which depends on everything else running.
    pub ram_limit: Option<u64>,
    pub bridge: BridgeHost,
    pub monitor: Arc<dyn Monitor>,
    pub agent: Arc<dyn GuestAgent>,
    pub clock: Arc<dyn Clock>,
    /// Replaces the room an install needs, for a test on a small disk.
    pub install_room: Option<u64>,
}

impl Context {
    pub fn new(paths: Paths, slot: Slot) -> Self {
        Self {
            paths,
            slot,
            processes: "/proc".into(),
            cpus: "/sys/devices/system/cpu".into(),
            usb_devices: "/sys/bus/usb/devices".into(),
            usb_nodes: "/dev/bus/usb".into(),
            pci_devices: "/sys/bus/pci/devices".into(),
            pci_names: "/usr/share/hwdata/pci.ids".into(),
            kvm: "/dev/kvm".into(),
            driver_version_file: "/sys/module/nvidia/version".into(),
            meminfo: "/proc/meminfo".into(),
            ram_limit: None,
            bridge: BridgeHost::default(),
            monitor: Arc::new(QmpMonitor),
            agent: Arc::new(TcpAgent),
            clock: Arc::new(RealClock::default()),
            install_room: None,
        }
    }

    /// For this process. A slot that can't be read stops it, as a guess could reach another VM's files.
    pub fn from_process() -> crate::Result<Self> {
        let slot = Slot::from_environment().map_err(|error| crate::Failure::new(error.to_string()))?;
        Ok(Self::new(Paths::from_process(), slot))
    }

    pub fn second_instance(&self) -> bool {
        self.paths.second_instance()
    }

    pub fn run_folder(&self, slot: Option<Slot>) -> PathBuf {
        self.paths.run_folder(slot.unwrap_or(self.slot))
    }

    pub fn agent_port(&self, slot: Option<Slot>) -> u16 {
        self.paths.agent_port(slot.unwrap_or(self.slot))
    }

    pub fn install_room_gib(&self) -> u64 {
        self.install_room.unwrap_or(crate::library::INSTALL_ROOM_GIB)
    }
}
