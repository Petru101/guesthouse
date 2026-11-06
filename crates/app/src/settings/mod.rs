//! A machine's settings. Not here yet.

use crate::window::{MachineHandle, Window};
use guesthouse_app_core::tasks::Status;

#[derive(Clone)]
pub struct SettingsDialog(MachineHandle);

impl SettingsDialog {
    pub fn new(_window: &Window, machine: MachineHandle) -> Self {
        Self(machine)
    }

    pub fn machine_id(&self) -> String {
        self.0.borrow().id.clone()
    }

    pub fn present(&self) {}

    pub fn set_status(&self, _status: &Status) {}
}
