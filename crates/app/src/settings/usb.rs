//! USB devices, which can go to a running VM.

use super::Shared;
use crate::common::in_background;
use crate::rows::{confirm, icon_button, row_button};
use crate::settings::network::show_root_step;
use adw::prelude::*;
use guesthouse_app_core::tasks::{State, Status};
use guesthouse_app_core::usb::{UsbDevice, attach_usb, attached_usb, detach_usb, usb_devices};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

struct Parts {
    shared: Shared,
    group: adw::PreferencesGroup,
    rows: RefCell<Vec<adw::ActionRow>>,
    running: Cell<bool>,
    busy: Cell<bool>,
}

#[derive(Clone)]
pub struct Usb(Rc<Parts>);

impl Usb {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let group = adw::PreferencesGroup::builder().title("USB Devices").build();
        page.add(&group);
        let refresh = icon_button("view-refresh-symbolic", "Look Again");
        group.set_header_suffix(Some(&refresh));
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            group,
            rows: RefCell::default(),
            running: Cell::new(false),
            busy: Cell::new(false),
        }));
        let handle = this.clone();
        refresh.connect_clicked(move |_| handle.read());
        this.read();
        this
    }

    fn read(&self) {
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().clone();
        let running = self.0.running.get();
        let reading = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || {
                let attached = if running { attached_usb(&reading, &machine) } else { BTreeSet::new() };
                Ok((usb_devices(&reading), attached))
            },
            move |result| match result {
                Ok((devices, attached)) => this.show(&devices, &attached),
                Err(text) => this.0.shared.window.problem(&text),
            },
        );
    }

    fn row_for(&self, device: &UsbDevice, attached: &BTreeSet<String>) -> adw::ActionRow {
        let mut notes = vec![format!("{}:{}", device.vendor, device.product)];
        if !device.roles.is_empty() {
            notes.push(format!("This PC is using it: {}", device.roles.join(", ")));
        }
        let row = adw::ActionRow::builder()
            .title(&device.name)
            .use_markup(false)
            .subtitle(notes.join(". "))
            .build();
        let inside = attached.contains(&device.qemu_id());
        let button = row_button(if inside { "Detach" } else { "Attach" });
        let this = self.clone();
        let chosen = device.clone();
        button.connect_clicked(move |_| if inside { this.detach(&chosen) } else { this.on_attach(&chosen) });
        row.add_suffix(&button);
        row
    }

    fn show(&self, devices: &[UsbDevice], attached: &BTreeSet<String>) {
        let parts = &self.0;
        for row in parts.rows.borrow_mut().drain(..) {
            parts.group.remove(&row);
        }
        let mut rows: Vec<adw::ActionRow> =
            devices.iter().map(|device| self.row_for(device, attached)).collect();
        for row in &rows {
            row.set_sensitive(parts.running.get());
        }
        if rows.is_empty() {
            rows.push(adw::ActionRow::builder().title("Nothing is plugged in").build());
        }
        for row in &rows {
            parts.group.add(row);
        }
        *parts.rows.borrow_mut() = rows;
    }

    fn on_attach(&self, device: &UsbDevice) {
        let shared = &self.0.shared;
        if !device.can_open(&shared.window.context()) {
            let body = format!(
                "This user cannot open {}. Run this once in a terminal, then attach it again. It lets whoever \
                 is logged in on this PC use this kind of device.",
                device.name
            );
            show_root_step(shared, &body, Some(&device.access_command()));
        } else if device.roles.is_empty() {
            self.attach(device);
        } else {
            let body = format!(
                "This PC is using it: {}. It stops working here until it is detached or the VM stops. If it \
                 is a disk, unmount it first.",
                device.roles.join(", ")
            );
            let this = self.clone();
            let chosen = device.clone();
            confirm(&shared.dialog, &format!("Attach {}?", device.name), &body, "Attach", move || {
                this.attach(&chosen);
            });
        }
    }

    fn attach(&self, device: &UsbDevice) {
        self.change(device.clone(), attach_usb);
    }

    fn detach(&self, device: &UsbDevice) {
        self.change(device.clone(), detach_usb);
    }

    fn change(
        &self,
        device: UsbDevice,
        action: fn(
            &guesthouse_app_core::context::Context,
            &guesthouse_app_core::machine::Machine,
            &UsbDevice,
        ) -> guesthouse_app_core::Result<()>,
    ) {
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().clone();
        let working = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || action(&working, &machine, &device),
            move |result| {
                if let Err(text) = result {
                    this.0.shared.window.problem(&text);
                }
                this.read();
            },
        );
    }

    pub fn set_status(&self, status: &Status) {
        let parts = &self.0;
        let running = status.state == State::Running;
        parts.group.set_description(Some(if running {
            "Attached devices leave this PC until they are detached or the VM stops."
        } else {
            "The VM must be running to attach one."
        }));
        let changed = parts.busy.replace(status.busy) != status.busy;
        if changed || parts.running.replace(running) != running {
            self.read();
        }
    }
}
