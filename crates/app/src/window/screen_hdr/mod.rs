//! Whether this PC's screen can show HDR. A task has no display, so the app finds out and leaves the
//! answer in its settings. GDK can't say whether a monitor is in HDR, so this asks the compositor
//! through the colour management protocol, for the monitors the window is on, and keeps the answer up
//! to date as the window moves or HDR is turned on or off. A compositor without the protocol may still
//! be a GNOME that Mutter's D-Bus can answer for.

mod mutter;
mod output_colours;

use super::Window;
use crate::viewer::takes_hdr_frames;
use adw::prelude::*;
use gtk::{gdk, glib};
use guesthouse_app_core::display::{SCREEN_HDR_PREF, ScreenHdr};
use output_colours::OutputColours;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Says yes or no for a compositor that can't, such as a headless one.
const OVERRIDE_VARIABLE: &str = "GUESTHOUSE_SCREEN_HDR";
/// Nothing says when the compositor has told this queue something, as GTK does the reading, so what it
/// said is taken in this often. A look costs nothing when it said nothing.
const LOOK_SECONDS: u32 = 1;

/// Follows the compositor's answer while the window is open.
pub struct ScreenWatch {
    colours: OutputColours,
    /// The monitors the window is on, as the compositor has told GDK.
    on: RefCell<Vec<gdk::Monitor>>,
    said: Cell<Option<ScreenHdr>>,
}

fn all_monitors(display: &gdk::Display) -> Vec<gdk::Monitor> {
    display.monitors().iter::<gdk::Monitor>().flatten().collect()
}

impl ScreenWatch {
    fn answer(&self) -> ScreenHdr {
        self.colours.answer(&self.on.borrow())
    }
}

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
        match self.screen_watch() {
            Some(watch) => {
                watch.colours.catch_up();
                let answer = watch.answer();
                watch.said.set(Some(answer));
                self.remember_screen_hdr(answer);
            }
            None => mutter::ask(self),
        }
    }

    /// The watch, started the first time. `None` if the compositor doesn't offer the protocol.
    fn screen_watch(&self) -> Option<Rc<ScreenWatch>> {
        self.0.screen_watch.get_or_init(|| self.start_screen_watch()).clone()
    }

    fn start_screen_watch(&self) -> Option<Rc<ScreenWatch>> {
        let display = WidgetExt::display(self.widget());
        let colours = OutputColours::new(&display)?;
        colours.follow(&all_monitors(&display));
        colours.settle();
        let watch = Rc::new(ScreenWatch { colours, on: RefCell::default(), said: Cell::new(None) });

        let (this, watching) = (self.clone(), watch.clone());
        display.monitors().connect_items_changed(move |_, _, _, _| {
            let monitors = all_monitors(&WidgetExt::display(this.widget()));
            // A monitor that's gone may not say the window left it.
            watching.on.borrow_mut().retain(|on| monitors.contains(on));
            watching.colours.follow(&monitors);
            watching.colours.settle();
            this.look_at_screen(&watching);
        });
        if let Some(surface) = self.widget().surface() {
            let (this, watching) = (self.clone(), watch.clone());
            surface.connect_enter_monitor(move |_, monitor| {
                watching.on.borrow_mut().push(monitor.clone());
                this.look_at_screen(&watching);
            });
            let (this, watching) = (self.clone(), watch.clone());
            surface.connect_leave_monitor(move |_, monitor| {
                watching.on.borrow_mut().retain(|on| on != monitor);
                this.look_at_screen(&watching);
            });
        }
        let (this, watching) = (self.clone(), watch.clone());
        glib::timeout_add_seconds_local(LOOK_SECONDS, move || {
            watching.colours.catch_up();
            this.look_at_screen(&watching);
            glib::ControlFlow::Continue
        });
        Some(watch)
    }

    /// Leaves the answer again if it has changed.
    fn look_at_screen(&self, watch: &ScreenWatch) {
        let answer = watch.answer();
        if watch.said.replace(Some(answer)) != Some(answer) {
            self.remember_screen_hdr(answer);
        }
    }
}
