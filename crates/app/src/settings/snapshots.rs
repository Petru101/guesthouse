//! Snapshots: taking one, going back, and deleting.

use super::{Shared, is_off};
use crate::common::in_background;
use crate::rows::{boxed_rows, confirm, icon_button, on_yes, question, row_button};
use adw::prelude::*;
use chrono::{Local, TimeZone};
use guesthouse_app_core::snapshots::{MAX_SNAPSHOT_NAME, Snapshot, list_snapshots};
use guesthouse_app_core::tasks::Status;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const NOT_INSTALLED: &str = "Windows has not finished installing.";
const SHUT_DOWN_FIRST: &str = "Shut the VM down first.";

fn taken_text(snapshot: &Snapshot) -> String {
    let time = Local.timestamp_opt(snapshot.taken, 0).single();
    time.map_or_else(String::new, |time| time.format("%-d %B %Y, %H:%M").to_string())
}

struct Parts {
    shared: Shared,
    group: adw::PreferencesGroup,
    take: gtk::Button,
    rows: RefCell<Vec<adw::ActionRow>>,
    can_change: Cell<bool>,
    busy: Cell<bool>,
}

#[derive(Clone)]
pub struct Snapshots(Rc<Parts>);

impl Snapshots {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let group = adw::PreferencesGroup::builder()
            .title("Snapshots")
            .description("They live inside the disk file, which grows as the guest changes what they hold.")
            .build();
        page.add(&group);
        let take = row_button("Take Snapshot");
        take.set_sensitive(false);
        group.set_header_suffix(Some(&take));
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            group,
            take,
            rows: RefCell::default(),
            can_change: Cell::new(false),
            busy: Cell::new(false),
        }));
        let handle = this.clone();
        this.0.take.connect_clicked(move |_| handle.on_take());
        this.read();
        this
    }

    fn read(&self) {
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().clone();
        let reading = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || Ok(list_snapshots(&reading, &machine)),
            move |result| match result {
                Ok(found) => this.show(&found),
                Err(text) => this.0.shared.window.problem(&text),
            },
        );
    }

    fn row_for(&self, snapshot: &Snapshot) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&snapshot.name)
            .use_markup(false)
            .subtitle(taken_text(snapshot))
            .build();
        let back = row_button("Go Back");
        let this = self.clone();
        let chosen = snapshot.clone();
        back.connect_clicked(move |_| this.on_go_back(&chosen));
        row.add_suffix(&back);
        let delete = icon_button("user-trash-symbolic", "Delete Snapshot");
        let this = self.clone();
        let chosen = snapshot.clone();
        delete.connect_clicked(move |_| this.on_delete(&chosen));
        row.add_suffix(&delete);
        row
    }

    fn show(&self, snapshots: &[Snapshot]) {
        let parts = &self.0;
        for row in parts.rows.borrow_mut().drain(..) {
            parts.group.remove(&row);
        }
        let mut rows: Vec<adw::ActionRow> = snapshots.iter().map(|snapshot| self.row_for(snapshot)).collect();
        if rows.is_empty() {
            rows.push(adw::ActionRow::builder().title("None yet").build());
        }
        for row in &rows {
            row.set_sensitive(parts.can_change.get());
            parts.group.add(row);
        }
        *parts.rows.borrow_mut() = rows;
    }

    fn on_take(&self) {
        let name = adw::EntryRow::builder().title("Name").max_length(MAX_SNAPSHOT_NAME as i32).build();
        let dialog = question(
            "Take a Snapshot",
            "This keeps the disk and the firmware settings as they are now.",
            "Take Snapshot",
            adw::ResponseAppearance::Suggested,
        );
        dialog.set_extra_child(Some(&boxed_rows(&[name.upcast_ref()])));
        dialog.set_response_enabled("yes", false);
        let enabling = dialog.clone();
        name.connect_changed(move |row| enabling.set_response_enabled("yes", !row.text().trim().is_empty()));
        let shared = self.0.shared.clone();
        on_yes(&dialog, move || {
            shared.window.launch("take_snapshot", &shared.machine.borrow(), &[name.text().to_string()], None);
        });
        dialog.present(Some(&self.0.shared.dialog));
    }

    fn on_go_back(&self, snapshot: &Snapshot) {
        let shared = self.0.shared.clone();
        let tag = snapshot.tag.clone();
        let body = format!(
            "Everything done on {} since {} is lost: files, programs and changes to the guest. If the disk \
             grew since then, it shrinks back. Other snapshots stay. This cannot be undone.",
            shared.machine.borrow().name,
            taken_text(snapshot)
        );
        let parent = shared.dialog.clone();
        confirm(&parent, &format!("Go Back to {}?", snapshot.name), &body, "Go Back", move || {
            shared.window.launch("go_back", &shared.machine.borrow(), std::slice::from_ref(&tag), None);
        });
    }

    fn on_delete(&self, snapshot: &Snapshot) {
        let shared = self.0.shared.clone();
        let tag = snapshot.tag.clone();
        let body = "You can no longer go back to it. The machine stays as it is now.";
        let parent = shared.dialog.clone();
        confirm(&parent, &format!("Delete {}?", snapshot.name), body, "Delete", move || {
            shared.window.launch(
                "delete_snapshot",
                &shared.machine.borrow(),
                std::slice::from_ref(&tag),
                None,
            );
        });
    }

    pub fn set_status(&self, status: &Status) {
        let parts = &self.0;
        let installing = parts.shared.machine.borrow().installing;
        let can_change = is_off(status) && !installing;
        parts.can_change.set(can_change);
        parts.take.set_sensitive(can_change);
        let why = if installing { NOT_INSTALLED } else { SHUT_DOWN_FIRST };
        parts.take.set_tooltip_text((!can_change).then_some(why));
        for row in parts.rows.borrow().iter() {
            row.set_sensitive(can_change);
        }
        if parts.busy.replace(status.busy) != status.busy {
            self.read();
        }
    }
}
