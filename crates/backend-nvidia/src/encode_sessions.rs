//! Encoding sessions: the host's driver decides how many a card may have.
//!
//! NVIDIA limits how many sessions a GeForce card runs at once, and this project must not lift that.
//! So this keeps no count. When the guest's runtime asks to open a session, the daemon takes a slot
//! from the host's driver with the same control a native session takes one with, and tells the guest
//! what the driver said. That's one count for the whole card, shared with the host's own sessions and
//! every other daemon, with no number of ours to go stale. The driver gives a daemon's slots back
//! when its file closes, so a daemon that dies leaves nothing behind.
//!
//! The guest's runtime then makes one context on each encoder, so a process's sessions are as many
//! as it has contexts on its busiest encoder, plus any it was told yes for and hasn't started.

use crate::rm::classes::{CONTROL_ENCODE_SLOT_GIVE_BACK, CONTROL_ENCODE_SLOT_TAKE};
use crate::rm::device::Device;
use crate::rm::{Rm, STATUS_OK};
use std::collections::HashMap;

const SLOT_PARAMS: usize = 4;

#[derive(Debug, Default)]
struct ProcessSessions {
    slots: u32,
    /// Told yes, and no contexts for it yet.
    starting: u32,
    contexts_on_node: HashMap<u32, u32>,
}

impl ProcessSessions {
    fn running(&self) -> u32 {
        self.contexts_on_node.values().copied().max().unwrap_or(0)
    }
}

/// See the module documentation.
#[derive(Debug, Default)]
pub struct EncodeSessions {
    processes: HashMap<u32, ProcessSessions>,
}

fn give_back<R: Rm>(device: &mut Device<R>) {
    let subdevice = device.subdevice;
    match device.rm.control(subdevice, CONTROL_ENCODE_SLOT_GIVE_BACK, &mut [0; SLOT_PARAMS]) {
        Ok(STATUS_OK) => {}
        Ok(status) => log::warn!("giving back an encoding slot: status {status:#x}"),
        Err(error) => log::warn!("giving back an encoding slot: {error}"),
    }
}

impl EncodeSessions {
    /// Whether a process may open one more session, which is the driver's answer. If so it holds a slot.
    pub fn ask<R: Rm>(&mut self, device: &mut Device<R>, process: u32) -> bool {
        let subdevice = device.subdevice;
        match device.rm.control(subdevice, CONTROL_ENCODE_SLOT_TAKE, &mut [0; SLOT_PARAMS]) {
            Ok(STATUS_OK) => {
                let sessions = self.processes.entry(process).or_default();
                sessions.slots += 1;
                sessions.starting += 1;
                true
            }
            Ok(status) => {
                log::debug!("the driver has no encoding slot: status {status:#x}");
                false
            }
            Err(error) => {
                log::warn!("asking for an encoding slot: {error}");
                false
            }
        }
    }

    /// Notes a context on an encoder. A new highest count is a session under way.
    pub fn context_made(&mut self, process: u32, node: u32) {
        let sessions = self.processes.entry(process).or_default();
        let before = sessions.running();
        *sessions.contexts_on_node.entry(node).or_default() += 1;
        if sessions.running() > before {
            sessions.starting = sessions.starting.saturating_sub(1);
        }
    }

    /// Notes that a context on an encoder has gone. A lower highest count is a session over.
    pub fn context_gone<R: Rm>(&mut self, device: &mut Device<R>, process: u32, node: u32) {
        let Some(sessions) = self.processes.get_mut(&process) else { return };
        let before = sessions.running();
        if let Some(count) = sessions.contexts_on_node.get_mut(&node) {
            *count = count.saturating_sub(1);
        }
        if sessions.running() < before && sessions.slots > 0 {
            sessions.slots -= 1;
            give_back(device);
        }
    }

    /// A process has ended, so all it held goes back, however it ended.
    pub fn process_gone<R: Rm>(&mut self, device: &mut Device<R>, process: u32) {
        let Some(sessions) = self.processes.remove(&process) else { return };
        (0..sessions.slots).for_each(|_| give_back(device));
    }

    /// The guest has restarted, so nothing it held is in use.
    pub fn all_gone<R: Rm>(&mut self, device: &mut Device<R>) {
        let slots: u32 = self.processes.drain().map(|(_, sessions)| sessions.slots).sum();
        (0..slots).for_each(|_| give_back(device));
    }

    /// How many slots a process holds, for tests.
    pub fn held_by(&self, process: u32) -> u32 {
        self.processes.get(&process).map_or(0, |sessions| sessions.slots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::HOST_615;
    use crate::rm::fake::{Call, FakeRm};

    const PROCESS: u32 = 7;
    const NODES: [u32; 3] = [6, 7, 8];
    /// What the host's driver says with no slot left, as measured.
    const NO_SLOT: u32 = 0x69;

    fn device(answer: u32) -> Device<FakeRm> {
        let mut rm = FakeRm::new();
        rm.answer(CONTROL_ENCODE_SLOT_TAKE, move |_| answer);
        rm.answer(CONTROL_ENCODE_SLOT_GIVE_BACK, |_| 0);
        Device::open(rm, &HOST_615).unwrap()
    }

    fn given_back(device: &Device<FakeRm>) -> usize {
        let calls = device.rm.calls.iter();
        calls
            .filter(|call| matches!(call, Call::Control { command: CONTROL_ENCODE_SLOT_GIVE_BACK, .. }))
            .count()
    }

    #[test]
    fn the_answer_is_the_drivers() {
        assert!(EncodeSessions::default().ask(&mut device(0), PROCESS));
        assert!(!EncodeSessions::default().ask(&mut device(NO_SLOT), PROCESS));
    }

    #[test]
    fn a_refusal_holds_nothing() {
        let mut sessions = EncodeSessions::default();
        let mut device = device(NO_SLOT);
        sessions.ask(&mut device, PROCESS);
        assert_eq!(sessions.held_by(PROCESS), 0);
        sessions.process_gone(&mut device, PROCESS);
        assert_eq!(given_back(&device), 0);
    }

    fn start(sessions: &mut EncodeSessions, device: &mut Device<FakeRm>) {
        assert!(sessions.ask(device, PROCESS));
        NODES.iter().for_each(|&node| sessions.context_made(PROCESS, node));
    }

    #[test]
    fn a_session_ending_gives_one_slot_back() {
        let mut device = device(0);
        let mut sessions = EncodeSessions::default();
        start(&mut sessions, &mut device);
        start(&mut sessions, &mut device);
        NODES.iter().for_each(|&node| sessions.context_gone(&mut device, PROCESS, node));
        assert_eq!(sessions.held_by(PROCESS), 1);
        assert_eq!(given_back(&device), 1);
    }

    #[test]
    fn a_process_ending_gives_back_all_it_held() {
        let mut device = device(0);
        let mut sessions = EncodeSessions::default();
        start(&mut sessions, &mut device);
        assert!(sessions.ask(&mut device, PROCESS), "told yes and not started");
        sessions.process_gone(&mut device, PROCESS);
        assert_eq!(given_back(&device), 2);
    }

    #[test]
    fn a_guest_restarting_gives_back_what_every_process_held() {
        let mut device = device(0);
        let mut sessions = EncodeSessions::default();
        start(&mut sessions, &mut device);
        assert!(sessions.ask(&mut device, PROCESS + 1));
        sessions.all_gone(&mut device);
        assert_eq!(given_back(&device), 2);
        assert_eq!(sessions.held_by(PROCESS), 0);
    }

    #[test]
    fn a_slot_told_yes_survives_another_session_ending() {
        let mut device = device(0);
        let mut sessions = EncodeSessions::default();
        start(&mut sessions, &mut device);
        assert!(sessions.ask(&mut device, PROCESS));
        NODES.iter().for_each(|&node| sessions.context_gone(&mut device, PROCESS, node));
        assert_eq!(sessions.held_by(PROCESS), 1);
    }

    #[test]
    fn a_process_that_holds_nothing_gives_nothing_back() {
        let mut device = device(0);
        let mut sessions = EncodeSessions::default();
        sessions.context_gone(&mut device, PROCESS, 6);
        sessions.process_gone(&mut device, PROCESS);
        assert_eq!(given_back(&device), 0);
    }
}
