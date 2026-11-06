//! The disk's size, and what is in the drive.

use super::{Shared, is_off};
use crate::common::in_background;
use crate::rows::{boxed_rows, number_row, on_yes, question, row_button};
use adw::prelude::*;
use guesthouse_app_core::disk_image::disk_size_gib;
use guesthouse_app_core::library::{MAX_DISK_GIB, eject};
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::tasks::Status;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Parts {
    shared: Shared,
    disk: adw::ActionRow,
    grow: gtk::Button,
    drive: adw::ActionRow,
    eject: gtk::Button,
    size: Cell<Option<f64>>,
    busy: Cell<bool>,
    status: RefCell<Status>,
}

#[derive(Clone)]
pub struct Disk(Rc<Parts>);

impl Disk {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let group = adw::PreferencesGroup::builder().title("Disk").build();
        page.add(&group);
        let disk = adw::ActionRow::builder().title("Disk size").subtitle("Reading the disk").build();
        let grow = row_button("Grow");
        grow.set_sensitive(false);
        disk.add_suffix(&grow);
        group.add(&disk);
        let plain = shared.machine.borrow().kind == Kind::Plain;
        let drive = adw::ActionRow::builder().title("ISO").subtitle_lines(1).visible(plain).build();
        let eject = row_button("Eject");
        drive.add_suffix(&eject);
        group.add(&drive);
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            disk,
            grow,
            drive,
            eject,
            size: Cell::new(None),
            busy: Cell::new(false),
            status: RefCell::default(),
        }));
        let handle = this.clone();
        this.0.grow.connect_clicked(move |_| handle.on_grow());
        let handle = this.clone();
        this.0.eject.connect_clicked(move |_| handle.on_eject());
        this.show_drive();
        this.read_disk();
        this
    }

    fn show_drive(&self) {
        let iso = self.0.shared.machine.borrow().iso.clone();
        let text = if iso.is_empty() { "Empty. The machine boots from its disk." } else { iso.as_str() };
        self.0.drive.set_subtitle(&gtk::glib::markup_escape_text(text));
        self.0.eject.set_sensitive(!iso.is_empty());
    }

    fn on_eject(&self) {
        let shared = &self.0.shared;
        shared.change(|machine| eject(&shared.window.context(), machine));
        self.show_drive();
    }

    fn read_disk(&self) {
        let context = self.0.shared.window.context();
        let disk = self.0.shared.machine.borrow().disk.clone();
        let this = self.clone();
        in_background(
            &context,
            move || disk_size_gib(&disk),
            move |result| match result {
                Ok(size) => {
                    this.0.size.set(Some(size));
                    this.show_size();
                }
                Err(text) => this.0.disk.set_subtitle(&gtk::glib::markup_escape_text(&text)),
            },
        );
    }

    fn show_size(&self) {
        let off = is_off(&self.0.status.borrow());
        self.0.grow.set_sensitive(off && self.0.size.get().is_some());
        if let Some(size) = self.0.size.get() {
            let note = if off { "" } else { " It can only grow while the VM is off." };
            self.0.disk.set_subtitle(&format!("{size} GiB.{note}"));
        }
    }

    fn on_grow(&self) {
        let Some(current) = self.0.size.get() else { return };
        let smallest = current.trunc() + 1.0;
        let size = number_row("New size in GiB", smallest, smallest, MAX_DISK_GIB.into());
        let dialog = question(
            "Grow the Disk?",
            "The guest will not use the new space until you extend its volume, which on Windows is in Disk \
             Management. The disk cannot shrink again.",
            "Grow",
            adw::ResponseAppearance::Destructive,
        );
        dialog.set_extra_child(Some(&boxed_rows(&[size.upcast_ref()])));
        let shared = self.0.shared.clone();
        on_yes(&dialog, move || {
            let gib = (size.value() as u32).to_string();
            shared.window.launch("grow", &shared.machine.borrow(), &[gib], None);
        });
        dialog.present(Some(&self.0.shared.dialog));
    }

    pub fn set_status(&self, status: &Status) {
        *self.0.status.borrow_mut() = status.clone();
        self.show_drive();
        self.show_size();
        if self.0.busy.replace(status.busy) != status.busy {
            self.read_disk();
        }
    }
}
