//! The refresh rate, and the viewer's overlay.

use super::Shared;
use crate::rows::choice_row;
use crate::viewer::OVERLAY_KEYS;
use adw::prelude::*;
use guesthouse_app_core::machine::{Kind, MAX_REFRESH_HZ, MIN_REFRESH_HZ, REFRESH_RATES, read_refresh};
use guesthouse_app_core::tasks::Status;
use std::rc::Rc;

/// The guest keeps its own rate until a display driver can be told, so nothing applies this yet.
const RATE_NOTE: &str = "Kept for later. The guest stays at its own rate for now.";

struct Parts {
    shared: Shared,
    group: adw::PreferencesGroup,
    rate: adw::ComboRow,
    custom: adw::EntryRow,
}

#[derive(Clone)]
pub struct Display(Rc<Parts>);

impl Display {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let machine = shared.machine.borrow().clone();
        let group = adw::PreferencesGroup::builder().title("Display").build();
        group.set_visible(machine.kind == Kind::Windows);
        page.add(&group);
        let mut names: Vec<String> = REFRESH_RATES.iter().map(|hz| format!("{hz} Hz")).collect();
        names.push("Custom".to_owned());
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let rate = choice_row("Refresh rate", &names);
        let current = REFRESH_RATES.iter().position(|hz| *hz == machine.refresh_hz);
        rate.set_selected(current.unwrap_or(REFRESH_RATES.len()) as u32);
        group.add(&rate);
        let custom = adw::EntryRow::builder()
            .title("Custom rate in Hz")
            .text(machine.refresh_hz.to_string())
            .show_apply_button(true)
            .input_purpose(gtk::InputPurpose::Digits)
            .build();
        group.add(&custom);
        let this = Self(Rc::new(Parts { shared: shared.clone(), group, rate, custom }));
        this.show_custom_rate();
        this.connect();
        this.add_viewer_group(page);
        this
    }

    fn add_viewer_group(&self, page: &adw::PreferencesPage) {
        let group = adw::PreferencesGroup::builder().title("Viewer").build();
        page.add(&group);
        let shared = &self.0.shared;
        let overlay = adw::SwitchRow::builder()
            .title("Performance overlay")
            .active(shared.machine.borrow().overlay)
            .subtitle(format!(
                "Shows frames a second and the host GPU's load over the screen. {OVERLAY_KEYS} turns it on and off there."
            ))
            .build();
        let saving = shared.clone();
        overlay.connect_active_notify(move |row| {
            saving.machine.borrow_mut().overlay = row.is_active();
            saving.save();
        });
        group.add(&overlay);
    }

    fn connect(&self) {
        let this = self.clone();
        self.0.rate.connect_selected_notify(move |row| {
            this.show_custom_rate();
            if let Some(hz) = REFRESH_RATES.get(row.selected() as usize) {
                this.set_rate(*hz);
            }
        });
        let this = self.clone();
        self.0.custom.connect_changed(move |row| {
            row.remove_css_class("error");
            this.show_custom_rate();
        });
        let this = self.clone();
        self.0.custom.connect_apply(move |row| this.on_custom_rate(row));
    }

    fn show_custom_rate(&self) {
        let custom = self.0.rate.selected() as usize == REFRESH_RATES.len();
        self.0.custom.set_visible(custom);
        let text =
            format!("From {MIN_REFRESH_HZ} to {MAX_REFRESH_HZ} Hz. The guest may not reach a high rate.");
        self.0.group.set_description(custom.then_some(text.as_str()));
    }

    fn on_custom_rate(&self, row: &adw::EntryRow) {
        match read_refresh(&row.text()) {
            Err(failure) => {
                row.add_css_class("error");
                // A toast would cover this row in a narrow window.
                self.0.group.set_description(Some(&failure.0));
            }
            // Selecting it sets the rate.
            Ok(hz) => match REFRESH_RATES.iter().position(|named| *named == hz) {
                Some(index) => self.0.rate.set_selected(index as u32),
                None => self.set_rate(hz),
            },
        }
    }

    fn set_rate(&self, hz: u32) {
        let shared = &self.0.shared;
        if hz == shared.machine.borrow().refresh_hz {
            return;
        }
        shared.machine.borrow_mut().refresh_hz = hz;
        self.0.custom.set_text(&hz.to_string());
        shared.save();
    }

    pub fn set_status(&self, status: &Status) {
        self.0.rate.set_sensitive(!status.busy);
        self.0.custom.set_sensitive(!status.busy);
        let note = if status.busy { status.step.as_str() } else { RATE_NOTE };
        self.0.rate.set_subtitle(note);
    }
}
