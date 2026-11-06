//! The dialog that makes a machine. Not here yet.

use crate::window::Window;

#[derive(Clone)]
pub struct NewMachineDialog;

impl NewMachineDialog {
    pub fn new(_window: &Window) -> Self {
        Self
    }

    pub fn present(&self) {}

    pub fn set_vm_running(&self, _running: bool) {}
}
