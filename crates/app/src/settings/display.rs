//! The screen: its size, rate and HDR, the driver behind them, and the viewer's overlay. A change
//! reaches a running machine at once if it's on the display driver, and otherwise the next start.

use super::Shared;
use crate::rows::{choice_row, row_button};
use crate::viewer::OVERLAY_KEYS;
use adw::prelude::*;
use guesthouse_app_core::display::{
    DisplayState, Driver, DriverPackage, forget_note, read_state, screen_shows_hdr,
};
use guesthouse_app_core::machine::{
    Kind, MAX_REFRESH_HZ, MIN_REFRESH_HZ, REFRESH_RATES, SCREENS, read_refresh,
};
use guesthouse_app_core::slots;
use guesthouse_app_core::tasks::{State, Status};
use std::cell::RefCell;
use std::rc::Rc;

const RATE_NOTE: &str = "Set each time the machine starts.";
const BASIC_NOTE: &str =
    "Windows is on its basic display, which has one rate. This is for the display driver.";
const HDR_NOTE: &str = "Windows goes to HDR when the machine starts, or at once if it's running.";
const NO_HDR_NOTE: &str = "This screen can't show HDR, so Windows stays in SDR.";
const DRIVER_ROW: &str = "Display driver";
const DRIVER_IN: &str = "In Windows. It was signed on this machine with a certificate made for it, which Windows \
                         trusts as a root. Its private key was deleted before the install.";
const DRIVER_NEXT: &str = "Goes into Windows by itself at the next start. It's signed there with a certificate \
                           made for this machine, which Windows then trusts as a root. Its private key is deleted \
                           before the install.";
const DRIVER_NONE: &str = "This build has no display driver, so Windows stays on its basic display.";

struct Parts {
    shared: Shared,
    group: adw::PreferencesGroup,
    screen: adw::ComboRow,
    rate: adw::ComboRow,
    custom: adw::EntryRow,
    hdr: adw::SwitchRow,
    driver: adw::ActionRow,
    install: gtk::Button,
    status: RefCell<Status>,
}

#[derive(Clone)]
pub struct Display(Rc<Parts>);

impl Display {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let machine = shared.machine.borrow().clone();
        let group = adw::PreferencesGroup::builder().title("Display").build();
        group.set_visible(machine.kind == Kind::Windows);
        page.add(&group);
        let screen = choice_row("Screen size", &SCREENS);
        screen.set_selected(SCREENS.iter().position(|size| *size == machine.screen).unwrap_or(0) as u32);
        group.add(&screen);
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
        let hdr = adw::SwitchRow::builder().title("HDR").active(machine.hdr).build();
        group.add(&hdr);
        let driver = adw::ActionRow::builder().title(DRIVER_ROW).build();
        let install = row_button("Install Driver");
        driver.add_suffix(&install);
        group.add(&driver);
        let status = RefCell::default();
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            group,
            screen,
            rate,
            custom,
            hdr,
            driver,
            install,
            status,
        }));
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
        self.0.screen.connect_selected_notify(move |row| {
            let wanted = SCREENS[row.selected() as usize];
            if wanted != this.0.shared.machine.borrow().screen {
                this.0.shared.machine.borrow_mut().screen = wanted.to_owned();
                this.changed();
            }
        });
        let this = self.clone();
        self.0.hdr.connect_active_notify(move |row| {
            this.0.shared.machine.borrow_mut().hdr = row.is_active();
            this.changed();
        });
        let this = self.clone();
        self.0.install.connect_clicked(move |_| this.launch("install_display"));
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
        self.changed();
    }

    fn launch(&self, action: &str) {
        let shared = &self.0.shared;
        let slot = self.0.status.borrow().slot.map(slots::number);
        shared.window.launch(action, &shared.machine.borrow(), &[], slot);
    }

    fn state(&self) -> DisplayState {
        let shared = &self.0.shared;
        read_state(&shared.window.context(), &shared.machine.borrow())
    }

    fn on_driver_now(&self) -> bool {
        self.state().running.is_some_and(|screen| screen.on_driver)
    }

    /// Saves a change, and takes it to Windows if Windows can take it now.
    fn changed(&self) {
        self.0.shared.save();
        let shared = &self.0.shared;
        if let Err(failure) = forget_note(&shared.window.context(), &shared.machine.borrow()) {
            shared.window.problem(&failure.0);
        }
        let status = self.0.status.borrow().clone();
        if status.state == State::Running && !status.busy && self.on_driver_now() {
            self.launch("apply_display");
        }
    }

    fn driver_note(&self, state: &DisplayState, has_package: bool) -> String {
        match state.driver {
            Driver::Installed => DRIVER_IN.to_owned(),
            Driver::Failed => state.reason.clone(),
            Driver::Absent if has_package => DRIVER_NEXT.to_owned(),
            Driver::Absent => DRIVER_NONE.to_owned(),
        }
    }

    pub fn set_status(&self, status: &Status) {
        *self.0.status.borrow_mut() = status.clone();
        let parts = &self.0;
        let state = self.state();
        let context = parts.shared.window.context();
        let running = status.state == State::Running;
        let on_driver = state.running.is_some_and(|screen| screen.on_driver);
        let can_change = !status.busy && (!running || on_driver);
        parts.screen.set_sensitive(can_change);
        parts.rate.set_sensitive(can_change);
        parts.custom.set_sensitive(can_change);
        parts.hdr.set_sensitive(can_change);
        let note = if status.busy {
            status.step.clone()
        } else if !status.screen.is_empty() {
            format!("Now {}.", status.screen)
        } else if running {
            BASIC_NOTE.to_owned()
        } else {
            RATE_NOTE.to_owned()
        };
        parts.rate.set_subtitle(&note);
        parts.hdr.set_subtitle(if screen_shows_hdr(&context) { HDR_NOTE } else { NO_HDR_NOTE });
        let package = DriverPackage::find(&context);
        parts.driver.set_subtitle(&self.driver_note(&state, package.is_some()));
        let missing = state.driver != Driver::Installed && package.is_some();
        parts.install.set_visible(missing && running && !status.busy);
    }
}
