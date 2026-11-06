//! The network's mode, MAC address and bridge. Port forwards have their own group.

use super::forwards::Forwards;
use super::{Shared, is_off};
use crate::common::in_background;
use crate::rows::{choice_row, row_button};
use adw::prelude::*;
use guesthouse_app_core::bridge;
use guesthouse_app_core::library::unused_mac;
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::network::{self, DEFAULT_MAC, InUse, Mode};
use guesthouse_app_core::network_settings::{network_in_use, set_bridge, set_mac, set_network};
use guesthouse_app_core::tasks::{State, Status};
use guesthouse_app_core::vm_args::passt_installed;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Parts {
    shared: Shared,
    group: adw::PreferencesGroup,
    mode: adw::ComboRow,
    passt: adw::SwitchRow,
    bridge: adw::EntryRow,
    bridge_state: adw::ActionRow,
    bridge_help: gtk::Button,
    mac: adw::EntryRow,
    forwards: Forwards,
    in_use: Cell<Option<InUse>>,
    mac_problem: RefCell<Option<String>>,
    off: Cell<bool>,
}

#[derive(Clone)]
pub struct Network(Rc<Parts>);

impl Network {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let machine = shared.machine.borrow().clone();
        // Ubuntu's network is fixed, with its SSH port forwarded.
        let shown = machine.kind != Kind::Linux;
        let group = adw::PreferencesGroup::builder().title("Network").visible(shown).build();
        page.add(&group);
        let names: Vec<&str> = Mode::ALL.iter().map(|mode| mode.name()).collect();
        let mode = choice_row("Mode", &names);
        mode.set_selected(
            Mode::ALL.iter().position(|found| *found == machine.network_mode).unwrap_or(0) as u32
        );
        mode.set_subtitle_lines(0);
        group.add(&mode);
        let passt = adw::SwitchRow::builder().title("NAT through passt").active(machine.passt).build();
        group.add(&passt);
        let bridge =
            adw::EntryRow::builder().title("Bridge").text(&machine.bridge).show_apply_button(true).build();
        group.add(&bridge);
        let bridge_state = adw::ActionRow::builder().title("Bridge status").subtitle_lines(0).build();
        let bridge_help = row_button("Show What to Do");
        bridge_state.add_suffix(&bridge_help);
        group.add(&bridge_state);
        let text = if machine.mac.is_empty() { DEFAULT_MAC } else { &machine.mac };
        let mac = adw::EntryRow::builder().title("MAC address").text(text).show_apply_button(true).build();
        let new_mac = row_button("New");
        new_mac.set_tooltip_text(Some("Make a New Address"));
        mac.add_suffix(&new_mac);
        group.add(&mac);
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            group,
            mode,
            passt,
            bridge,
            bridge_state,
            bridge_help,
            mac,
            forwards: Forwards::new(shared, page),
            in_use: Cell::new(None),
            mac_problem: RefCell::default(),
            off: Cell::new(true),
        }));
        this.connect(&new_mac);
        this.show();
        this
    }

    fn connect(&self, new_mac: &gtk::Button) {
        let parts = &self.0;
        let this = self.clone();
        parts.mode.connect_selected_notify(move |row| this.on_mode(Mode::ALL[row.selected() as usize]));
        let this = self.clone();
        parts.passt.connect_active_notify(move |row| {
            this.0.shared.machine.borrow_mut().passt = row.is_active();
            this.0.shared.save();
            this.show();
        });
        let this = self.clone();
        parts.bridge.connect_apply(move |row| {
            let shared = &this.0.shared;
            shared.change(|machine| set_bridge(&shared.window.context(), machine, &row.text()));
            this.show();
        });
        let this = self.clone();
        parts.mac.connect_apply(move |row| this.on_mac(row));
        let this = self.clone();
        parts.mac.connect_changed(move |row| {
            row.remove_css_class("error");
            *this.0.mac_problem.borrow_mut() = None;
            this.show();
        });
        let this = self.clone();
        new_mac.connect_clicked(move |_| {
            let mac = &this.0.mac;
            mac.set_text(&unused_mac(&this.0.shared.window.context()));
            this.on_mac(mac);
        });
        let this = self.clone();
        parts.bridge_help.connect_clicked(move |_| this.on_bridge_help());
    }

    fn on_mode(&self, mode: Mode) {
        let shared = &self.0.shared;
        shared.change(|machine| set_network(&shared.window.context(), machine, mode));
        let mac = shared.machine.borrow().mac.clone();
        self.0.mac.set_text(if mac.is_empty() { DEFAULT_MAC } else { &mac });
        self.show();
    }

    fn on_mac(&self, row: &adw::EntryRow) {
        let shared = &self.0.shared;
        let refused = set_mac(&shared.window.context(), &mut shared.machine.borrow_mut(), &row.text());
        match refused {
            Err(failure) => {
                row.add_css_class("error");
                // A toast would cover this row in a narrow window.
                *self.0.mac_problem.borrow_mut() = Some(failure.0);
                self.show();
            }
            Ok(()) => {
                let mac = shared.machine.borrow().mac.clone();
                row.set_text(&mac);
                shared.window.refresh();
            }
        }
    }

    fn on_bridge_help(&self) {
        let shared = &self.0.shared;
        let context = shared.window.context();
        let host = &context.bridge;
        let name = shared.machine.borrow().bridge.clone();
        let interface = host.default_interface();
        let wireless = interface.as_deref().is_some_and(|found| host.is_wireless(found));
        let body = bridge::advice(&name, interface.as_deref(), wireless, host.filtered());
        let exists = host.exists(&name);
        let ready_to_run = exists || (interface.is_some() && !wireless);
        let commands = ready_to_run.then(|| host.commands(&name, interface.as_deref(), exists));
        show_root_step(
            shared,
            &format!("{body} This page sees it as soon as it is done."),
            commands.as_deref(),
        );
    }

    fn show(&self) {
        let parts = &self.0;
        let machine = parts.shared.machine.borrow().clone();
        let context = parts.shared.window.context();
        let off = parts.off.get();
        let installed = passt_installed();
        let bridged = machine.network_mode == Mode::Bridged;
        parts.mode.set_subtitle(&network::describe(machine.network_mode, machine.passt, installed));
        let notes = if off {
            parts.mac_problem.borrow().clone().unwrap_or_default()
        } else {
            let running = network::in_use_text(parts.in_use.get());
            format!("The VM must be off to change these. {running}").trim().to_owned()
        };
        parts.group.set_description(Some(&notes).filter(|text| !text.is_empty()).map(String::as_str));
        parts.mode.set_sensitive(off);
        parts.bridge.set_sensitive(off);
        parts.mac.set_sensitive(off);
        parts.passt.set_visible(machine.network_mode == Mode::Nat);
        parts.passt.set_sensitive(off && installed);
        parts.passt.set_subtitle(if installed { "Needs no root." } else { "passt is not installed." });
        parts.bridge.set_visible(bridged);
        parts.bridge_state.set_visible(bridged);
        if bridged {
            let found = context.bridge.problems(&machine.bridge);
            let ready = format!("{} is ready.", machine.bridge);
            let text = if found.is_empty() { ready } else { found.join(" ") };
            parts.bridge_state.set_subtitle(&gtk::glib::markup_escape_text(&text));
            parts.bridge_help.set_visible(!found.is_empty());
        }
        parts.forwards.set_off(off);
    }

    fn read_in_use(&self) {
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().clone();
        let reading = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || Ok(network_in_use(&reading, &machine)),
            move |result| match result {
                Ok(found) => {
                    this.0.in_use.set(found);
                    this.show();
                }
                Err(text) => this.0.shared.window.problem(&text),
            },
        );
    }

    pub fn set_status(&self, status: &Status) {
        self.0.off.set(is_off(status));
        if status.state == State::Running && self.0.in_use.get().is_none() {
            self.read_in_use();
        } else if status.state != State::Running {
            self.0.in_use.set(None);
        }
        self.show();
    }
}

/// The one step only the user can do, with commands that can be copied.
pub fn show_root_step(shared: &Shared, body: &str, commands: Option<&str>) {
    let dialog = adw::AlertDialog::new(Some("One Step Needs Root"), Some(body));
    if let Some(commands) = commands {
        let no_hyphens = gtk::pango::AttrList::new();
        no_hyphens.insert(gtk::pango::AttrInt::new_insert_hyphens(false));
        let command = gtk::Label::builder()
            .label(commands)
            .selectable(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::Char)
            .xalign(0.0)
            .attributes(&no_hyphens)
            .build();
        command.add_css_class("monospace");
        dialog.set_extra_child(Some(&command));
    }
    dialog.add_response("close", "Close");
    dialog.present(Some(&shared.dialog));
}
