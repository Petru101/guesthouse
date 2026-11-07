//! Whether this PC's screen can show HDR. A task has no display, so the app finds out and leaves the
//! answer in its settings. GDK can't say whether a monitor is in HDR, so this asks Mutter.

mod mutter;

use super::Window;
use crate::viewer::takes_hdr_frames;
use adw::prelude::*;
use guesthouse_app_core::display::{SCREEN_HDR_PREF, ScreenHdr};
use serde_json::json;

/// Says yes or no for a compositor that can't, such as a headless one.
const OVERRIDE_VARIABLE: &str = "GUESTHOUSE_SCREEN_HDR";

impl Window {
    fn remember_screen_hdr(&self, answer: ScreenHdr) {
        self.remember(json!({SCREEN_HDR_PREF: answer.word()}));
    }

    /// Finds out, and leaves the answer for the next task.
    pub fn note_screen_hdr(&self) {
        if let Ok(said) = std::env::var(OVERRIDE_VARIABLE) {
            self.remember_screen_hdr(if said == "1" { ScreenHdr::Shows } else { ScreenHdr::Cannot });
            return;
        }
        if !takes_hdr_frames(&WidgetExt::display(self.widget())) {
            self.remember_screen_hdr(ScreenHdr::Cannot);
            return;
        }
        mutter::ask(self);
    }
}
