//! Deleting a machine, or forgetting one whose disk lives elsewhere.

use super::{Shared, is_off};
use crate::rows::{confirm, row_button};
use adw::prelude::*;
use guesthouse_app_core::tasks::Status;
use std::path::Path;

pub struct Remove {
    row: adw::ActionRow,
    button: gtk::Button,
}

impl Remove {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let group = adw::PreferencesGroup::builder().title("Remove").build();
        page.add(&group);
        let machine = shared.machine.borrow().clone();
        let owned = machine.owns_files(&shared.window.context());
        let folder = Path::new(&machine.disk).parent().unwrap_or(Path::new("/")).display().to_string();
        let row = adw::ActionRow::builder()
            .title(if owned { "Delete Machine" } else { "Remove From Library" })
            .subtitle(if owned {
                "Deletes its disk too.".to_owned()
            } else {
                format!("Its disk stays in {folder}.")
            })
            .use_markup(false)
            .build();
        let button = row_button(if owned { "Delete" } else { "Remove" });
        button.add_css_class("destructive-action");
        let asking = shared.clone();
        button.connect_clicked(move |_| ask(&asking, owned));
        row.add_suffix(&button);
        group.add(&row);
        Self { row, button }
    }

    pub fn set_status(&self, status: &Status) {
        let off = is_off(status);
        self.button.set_sensitive(off);
        self.row.set_sensitive(off);
        self.button.set_tooltip_text((!off).then_some("Shut the VM down first."));
    }
}

fn ask(shared: &Shared, owned: bool) {
    let machine = shared.machine.borrow().clone();
    let context = shared.window.context();
    let (heading, body, label) = if owned {
        (
            format!("Delete {}?", machine.name),
            format!(
                "This deletes its disk and everything on it, in {}. It cannot be undone.",
                machine.folder(&context).display()
            ),
            "Delete",
        )
    } else {
        (
            format!("Remove {} From the Library?", machine.name),
            format!("Only the entry goes. The disk stays at {} and is not touched.", machine.disk),
            "Remove",
        )
    };
    let shared = shared.clone();
    let parent = shared.dialog.clone();
    confirm(&parent, &heading, &body, label, move || {
        shared.window.remove(shared.machine.borrow().clone());
        shared.dialog.close();
    });
}
