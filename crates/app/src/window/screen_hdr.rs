//! Whether this PC's screen can show HDR. A task has no display, so the app finds out and leaves the
//! answer in its settings. GDK can't say whether a monitor is in HDR, so this asks Mutter, and on a
//! desktop with no Mutter what the display takes has to do.

use super::Window;
use crate::viewer::takes_hdr_frames;
use adw::prelude::*;
use gtk::{gio, glib};
use guesthouse_app_core::display::SCREEN_HDR_PREF;
use serde_json::json;

const BUS_NAME: &str = "org.gnome.Mutter.DisplayConfig";
const OBJECT_PATH: &str = "/org/gnome/Mutter/DisplayConfig";
const CALL_MILLISECONDS: i32 = 2000;
const COLOUR_MODE: &str = "color-mode";
const COLOUR_MODE_BT2100: u32 = 1;
/// Says yes or no for a compositor that can't, such as a headless one.
const OVERRIDE_VARIABLE: &str = "GUESTHOUSE_SCREEN_HDR";
/// Where the monitors are in Mutter's reply, and where each one's properties are.
const MONITORS_AT: usize = 1;
const PROPERTIES_AT: usize = 2;

/// Reads `GetCurrentState`'s reply: `(serial, monitors, logical monitors, properties)`.
fn a_monitor_is_in_hdr(reply: &glib::Variant) -> bool {
    let monitors = reply.child_value(MONITORS_AT);
    (0..monitors.n_children()).any(|index| {
        let properties = monitors.child_value(index).child_value(PROPERTIES_AT);
        let mode = glib::VariantDict::new(Some(&properties)).lookup::<u32>(COLOUR_MODE);
        mode.ok().flatten() == Some(COLOUR_MODE_BT2100)
    })
}

impl Window {
    fn remember_screen_hdr(&self, shows: bool) {
        self.remember(json!({SCREEN_HDR_PREF: shows}));
    }

    /// Finds out, and leaves the answer for the next task.
    pub fn note_screen_hdr(&self) {
        if let Ok(said) = std::env::var(OVERRIDE_VARIABLE) {
            self.remember_screen_hdr(said == "1");
            return;
        }
        let takes_frames = takes_hdr_frames(&WidgetExt::display(self.widget()));
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE);
        let (true, Ok(bus)) = (takes_frames, bus) else {
            self.remember_screen_hdr(false);
            return;
        };
        let this = self.clone();
        bus.call(
            Some(BUS_NAME),
            OBJECT_PATH,
            BUS_NAME,
            "GetCurrentState",
            None,
            None,
            gio::DBusCallFlags::NONE,
            CALL_MILLISECONDS,
            gio::Cancellable::NONE,
            move |reply| this.remember_screen_hdr(reply.map_or(true, |reply| a_monitor_is_in_hdr(&reply))),
        );
    }
}
