//! The clipboard and the shared folder.

use super::Shared;
use crate::common::choose_folder;
use crate::rows::row_button;
use adw::prelude::*;
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::start_checks::share_problem;
use guesthouse_app_core::tasks::{State, Status};
use std::cell::Cell;
use std::rc::Rc;

struct Parts {
    shared: Shared,
    folder: adw::ActionRow,
    clear: gtk::Button,
    off: Cell<bool>,
}

#[derive(Clone)]
pub struct Sharing(Rc<Parts>);

impl Sharing {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let machine = shared.machine.borrow().clone();
        // The shared folder and the clipboard both reach a guest through what Ubuntu doesn't have yet.
        let group =
            adw::PreferencesGroup::builder().title("Sharing").visible(machine.kind != Kind::Linux).build();
        page.add(&group);
        let clipboard = adw::SwitchRow::builder()
            .title("Clipboard")
            .active(machine.clipboard)
            .visible(machine.kind == Kind::Windows)
            .subtitle(
                "Shares copied text with the guest, both ways, while its screen is open and its window has \
                 focus. Takes effect when the screen opens.",
            )
            .build();
        let saving = shared.clone();
        clipboard.connect_active_notify(move |row| {
            saving.machine.borrow_mut().clipboard = row.is_active();
            saving.save();
        });
        group.add(&clipboard);
        let folder = adw::ActionRow::builder().title("Shared folder").subtitle_lines(2).build();
        let choose = row_button("Choose");
        let clear = row_button("Clear");
        folder.add_suffix(&choose);
        folder.add_suffix(&clear);
        group.add(&folder);
        let this = Self(Rc::new(Parts { shared: shared.clone(), folder, clear, off: Cell::new(true) }));
        let handle = this.clone();
        this.0.clear.connect_clicked(move |_| handle.set_folder(String::new()));
        let handle = this.clone();
        choose.connect_clicked(move |_| handle.on_choose());
        this.show_folder();
        this
    }

    fn show_folder(&self) {
        let machine = self.0.shared.machine.borrow();
        let place = if machine.kind == Kind::Windows {
            "as a drive, from its next start, for files up to 4 GiB"
        } else {
            "at http://10.0.2.4/, through a WebDAV client"
        };
        let note = if self.0.off.get() { "" } else { " The VM must be off to change this." };
        let share = &machine.share;
        let text = if share.is_empty() {
            format!("None. The guest would see it {place}.{note}")
        } else {
            format!("{share}. The guest sees it {place}.{note}")
        };
        self.0.folder.set_subtitle(&gtk::glib::markup_escape_text(&text));
        self.0.clear.set_visible(!share.is_empty());
    }

    fn on_choose(&self) {
        let window = self.0.shared.window.clone();
        let failing = window.clone();
        let this = self.clone();
        choose_folder(
            window.widget(),
            "Choose a Folder to Share",
            move |path| this.set_folder(path),
            move |text| failing.problem(&text),
        );
    }

    fn set_folder(&self, path: String) {
        if let Some(problem) = share_problem(&self.0.shared.window.context(), &path) {
            self.0.shared.window.problem(&problem);
            return;
        }
        self.0.shared.machine.borrow_mut().share = path;
        self.0.shared.save();
        self.show_folder();
    }

    pub fn set_status(&self, status: &Status) {
        self.0.off.set(status.state == State::Off);
        self.0.folder.set_sensitive(super::is_off(status));
        self.show_folder();
    }
}
