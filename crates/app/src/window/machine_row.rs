//! One machine in the library, with what can be done to it now.

use super::Window;
use crate::application::APP_ID;
use crate::rows::{confirm, on_yes, question};
use adw::prelude::*;
use guesthouse_app_core::machine::{Kind, Machine};
use guesthouse_app_core::setup::checks::driver_warning;
use guesthouse_app_core::start_checks::SECOND_INSTANCE_REFUSAL;
use guesthouse_app_core::tasks::{State, Status, no_slot_text};
use std::cell::RefCell;
use std::rc::Rc;

/// Shown at half the size it's kept at, for screens that scale by two.
const THUMBNAIL_SHOWN: i32 = 96;
const THUMBNAIL_HEIGHT: i32 = 54;
const ICON_SIZE: i32 = 48;

/// One machine's settings, shared by the row and whatever page shows them.
pub type MachineHandle = Rc<RefCell<Machine>>;

struct Parts {
    row: adw::ActionRow,
    icon: gtk::Image,
    picture: gtk::Picture,
    frame: adw::Clamp,
    spinner: gtk::Spinner,
    start: gtk::Button,
    show: gtk::Button,
    stop: gtk::Button,
    force: gtk::Button,
}

#[derive(Clone)]
pub struct MachineRow {
    window: Window,
    machine: MachineHandle,
    status: Rc<RefCell<Status>>,
    parts: Rc<Parts>,
}

fn suffix_button(row: &adw::ActionRow, label: &str, style: Option<&str>) -> gtk::Button {
    let button = crate::rows::row_button(label);
    if let Some(style) = style {
        button.add_css_class(style);
    }
    row.add_suffix(&button);
    button
}

impl MachineRow {
    pub fn new(window: &Window, machine: Machine) -> Self {
        let row = adw::ActionRow::builder().title(&machine.name).use_markup(false).build();
        let icon = gtk::Image::builder()
            .icon_name(APP_ID)
            .pixel_size(ICON_SIZE)
            .width_request(THUMBNAIL_SHOWN)
            .height_request(THUMBNAIL_HEIGHT)
            .build();
        row.add_prefix(&icon);
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .overflow(gtk::Overflow::Hidden)
            .build();
        picture.add_css_class("machine-thumbnail");
        // A picture asks for its image's own size, so this holds it to the size of the icon.
        let frame = adw::Clamp::builder()
            .child(&picture)
            .maximum_size(THUMBNAIL_SHOWN)
            .tightening_threshold(THUMBNAIL_SHOWN)
            .valign(gtk::Align::Center)
            .height_request(THUMBNAIL_HEIGHT)
            .build();
        row.add_prefix(&frame);
        let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
        row.add_suffix(&spinner);
        let start = suffix_button(&row, "Start", Some("suggested-action"));
        let show = suffix_button(&row, "Show Screen", None);
        let stop = suffix_button(&row, "Shut Down", None);
        let force = suffix_button(&row, "Force Off", Some("destructive-action"));
        let settings = crate::rows::icon_button("emblem-system-symbolic", "Settings");
        row.add_suffix(&settings);
        // So that a click picks the machine Ctrl+, is for, as the keyboard does.
        let click = gtk::GestureClick::new();
        let focused = row.clone();
        click.connect_pressed(move |_, _, _, _| {
            focused.grab_focus();
        });
        row.add_controller(click);
        let parts = Parts { row, icon, picture, frame, spinner, start, show, stop, force };
        let this = Self {
            window: window.clone(),
            machine: Rc::new(RefCell::new(machine)),
            status: Rc::default(),
            parts: Rc::new(parts),
        };
        this.show_thumbnail();
        this.connect_buttons(&settings);
        this.set_status(&Status::default(), false);
        this
    }

    fn connect_buttons(&self, settings: &gtk::Button) {
        let this = self.clone();
        self.parts.start.connect_clicked(move |_| this.on_start());
        let this = self.clone();
        self.parts.show.connect_clicked(move |_| this.window.show_screen(this.machine.clone()));
        let this = self.clone();
        self.parts
            .stop
            .connect_clicked(move |_| this.window.launch("stop", &this.machine.borrow(), &[], None));
        let this = self.clone();
        self.parts.force.connect_clicked(move |_| this.on_force());
        let this = self.clone();
        settings.connect_clicked(move |_| this.window.show_settings(this.machine.clone()));
    }

    pub fn widget(&self) -> &adw::ActionRow {
        &self.parts.row
    }

    pub fn machine(&self) -> MachineHandle {
        self.machine.clone()
    }

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// No picture, or one that can't be read, leaves the icon.
    pub fn show_thumbnail(&self) {
        let path = self.machine.borrow().thumbnail(&self.window.context());
        let texture = gtk::gdk::Texture::from_filename(path).ok();
        self.parts.picture.set_paintable(texture.as_ref());
        self.parts.frame.set_visible(texture.is_some());
        self.parts.icon.set_visible(texture.is_none());
    }

    /// The words under the machine's name.
    fn subtitle(&self, status: &Status, blocked: bool, second: bool) -> String {
        let machine = self.machine.borrow();
        let state = status.state.name();
        let mut lines = vec![if !status.step.is_empty() {
            format!("{state}: {}", status.step)
        } else if status.screen.is_empty() {
            format!("{state}. {}", machine.summary())
        } else {
            format!("{state}. {}", machine.summary_showing(&status.screen))
        }];
        if machine.installing && status.state == State::Off {
            lines[0] = "The install did not finish. Start Over runs it again from the beginning.".to_owned();
        }
        if !status.error.is_empty() {
            lines.push(status.error.clone());
        }
        if !status.display_problem.is_empty() {
            lines.push(status.display_problem.clone());
        }
        if blocked {
            lines.push(no_slot_text(&self.window.context()));
        }
        if second {
            lines.push(SECOND_INSTANCE_REFUSAL.to_owned());
        }
        lines.join("\n")
    }

    pub fn set_status(&self, status: &Status, vm_running: bool) {
        *self.status.borrow_mut() = status.clone();
        let (installing, has_gpu) = {
            let machine = self.machine.borrow();
            (machine.installing, machine.has_gpu())
        };
        let state = status.state;
        let off = state == State::Off;
        let blocked = off && vm_running;
        let second = off && self.window.context().second_instance() && has_gpu;
        let parts = &self.parts;
        parts.row.set_subtitle(&self.subtitle(status, blocked, second));
        parts.spinner.set_visible(status.busy);
        parts.start.set_label(if installing { "Start Over" } else { "Start" });
        parts.start.set_visible(off);
        parts.start.set_sensitive(!status.busy && !blocked && !second);
        parts.show.set_visible(matches!(state, State::Starting | State::Installing | State::Running));
        parts.stop.set_visible(state == State::Running);
        parts.stop.set_sensitive(!status.busy);
        parts.force.set_visible(!off);
    }

    fn start(&self) {
        let machine = self.machine.borrow();
        let action = if machine.installing { "install" } else { "start" };
        self.window.launch(action, &machine, &[], None);
    }

    fn on_start(&self) {
        let warning = if self.machine.borrow().kind == Kind::Windows {
            driver_warning(&self.window.context())
        } else {
            None
        };
        let Some(warning) = warning else {
            self.start();
            return;
        };
        let dialog =
            question("Different NVIDIA Driver", &warning, "Start Anyway", adw::ResponseAppearance::Default);
        let this = self.clone();
        on_yes(&dialog, move || this.start());
        dialog.present(Some(self.window.widget()));
    }

    fn on_force(&self) {
        let this = self.clone();
        confirm(
            self.window.widget(),
            "Force Off?",
            "This cuts the power. The guest gets no chance to save anything.",
            "Force Off",
            move || this.window.launch("force", &this.machine.borrow(), &[], None),
        );
    }
}
