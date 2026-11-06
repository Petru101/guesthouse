//! Port forwards from this PC into the guest.

use super::Shared;
use crate::rows::{boxed_rows, choice_row, icon_button, on_yes, question, row_button};
use adw::prelude::*;
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::network::{Forward, Protocol};
use guesthouse_app_core::network_settings::{add_forward, read_forward, remove_forward};
use std::cell::RefCell;
use std::rc::Rc;

const FORWARD_HELP: &str = "A program on this PC connects to the host port, and reaches the guest port.";

struct Parts {
    shared: Shared,
    group: adw::PreferencesGroup,
    add: gtk::Button,
    rows: RefCell<Vec<adw::ActionRow>>,
}

#[derive(Clone)]
pub struct Forwards(Rc<Parts>);

/// What is typed in the Add dialog.
#[derive(Clone)]
struct Typed {
    protocol: adw::ComboRow,
    host: adw::EntryRow,
    guest: adw::EntryRow,
}

impl Typed {
    fn read(&self, shared: &Shared) -> guesthouse_app_core::Result<Forward> {
        let protocol = Protocol::ALL[self.protocol.selected() as usize];
        let context = shared.window.context();
        read_forward(&context, &shared.machine.borrow(), protocol, &self.host.text(), &self.guest.text())
    }

    /// Says what is wrong as it's typed, once there's something to judge.
    fn check(&self, shared: &Shared, dialog: &adw::AlertDialog) {
        let found = self.read(shared);
        let filled = !self.host.text().trim().is_empty() && !self.guest.text().trim().is_empty();
        let body = found.as_ref().err().filter(|_| filled).map_or(FORWARD_HELP, |failure| failure.0.as_str());
        dialog.set_body(body);
        dialog.set_response_enabled("yes", found.is_ok());
    }
}

impl Forwards {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let group = adw::PreferencesGroup::builder()
            .title("Port Forwards")
            .description("They reach the guest from this PC only. Other PCs can use Bridged.")
            .visible(shared.machine.borrow().kind != Kind::Linux)
            .build();
        page.add(&group);
        let add = row_button("Add");
        group.set_header_suffix(Some(&add));
        let this = Self(Rc::new(Parts { shared: shared.clone(), group, add, rows: RefCell::default() }));
        let handle = this.clone();
        this.0.add.connect_clicked(move |_| handle.on_add());
        this.show();
        this
    }

    fn show(&self) {
        let parts = &self.0;
        for row in parts.rows.borrow_mut().drain(..) {
            parts.group.remove(&row);
        }
        let forwards = parts.shared.machine.borrow().forwards.clone();
        for item in forwards {
            let note =
                format!("127.0.0.1:{} on this PC reaches port {} in the guest.", item.host, item.guest);
            let row = adw::ActionRow::builder().title(item.text()).subtitle(note).build();
            let remove = icon_button("user-trash-symbolic", "Remove Forward");
            let this = self.clone();
            remove.connect_clicked(move |_| {
                let shared = &this.0.shared;
                shared.change(|machine| remove_forward(&shared.window.context(), machine, &item));
                this.show();
            });
            row.add_suffix(&remove);
            parts.group.add(&row);
            parts.rows.borrow_mut().push(row);
        }
    }

    fn on_add(&self) {
        let names: Vec<&str> = Protocol::ALL.iter().map(|protocol| protocol.name()).collect();
        let typed = Typed {
            protocol: choice_row("Protocol", &names),
            host: adw::EntryRow::builder()
                .title("Host port")
                .input_purpose(gtk::InputPurpose::Digits)
                .build(),
            guest: adw::EntryRow::builder()
                .title("Guest port")
                .input_purpose(gtk::InputPurpose::Digits)
                .build(),
        };
        let dialog = question("Add a Port Forward", FORWARD_HELP, "Add", adw::ResponseAppearance::Suggested);
        let rows =
            boxed_rows(&[typed.protocol.upcast_ref(), typed.host.upcast_ref(), typed.guest.upcast_ref()]);
        dialog.set_extra_child(Some(&rows));
        dialog.set_response_enabled("yes", false);
        let shared = self.0.shared.clone();
        for row in [&typed.host, &typed.guest] {
            let (shared, typed, dialog) = (shared.clone(), typed.clone(), dialog.clone());
            row.connect_changed(move |_| typed.check(&shared, &dialog));
        }
        let (checking, reading, asking) = (shared.clone(), typed.clone(), dialog.clone());
        typed.protocol.connect_selected_notify(move |_| reading.check(&checking, &asking));
        let this = self.clone();
        on_yes(&dialog, move || {
            let shared = &this.0.shared;
            if let Ok(item) = typed.read(shared) {
                shared.change(|machine| add_forward(&shared.window.context(), machine, item));
                this.show();
            }
        });
        dialog.present(Some(&self.0.shared.dialog));
    }

    pub fn set_off(&self, off: bool) {
        self.0.add.set_sensitive(off);
        for row in self.0.rows.borrow().iter() {
            row.set_sensitive(off);
        }
    }
}
