//! The page that shows a guest's screen. Not here yet.

use crate::window::{MachineHandle, Window};
use guesthouse_app_core::tasks::Status;

pub const OVERLAY_KEYS: &str = "Ctrl+Alt+P";

#[derive(Clone)]
pub struct ScreenPage(adw::NavigationPage, MachineHandle);

impl ScreenPage {
    pub fn new(_window: &Window, machine: MachineHandle) -> Self {
        Self(adw::NavigationPage::builder().title("Screen").build(), machine)
    }

    pub fn widget(&self) -> &adw::NavigationPage {
        &self.0
    }

    pub fn machine_id(&self) -> String {
        self.1.borrow().id.clone()
    }

    pub fn show_hint(&self, _status: Option<&Status>) {}

    pub fn save_thumbnail_now(&self) {}

    pub fn release(&self) {}

    pub fn toggle_capture(&self) {}

    pub fn toggle_overlay(&self) {}

    pub fn toggle_fullscreen(&self) {}
}
