//! The account the install made in the guest, and where its password is kept. The name was the user's
//! to choose before the install, and the password file is theirs to read.

use super::Shared;
use adw::prelude::*;
use guesthouse_app_core::machine::Kind;

const WINDOWS_NOTE: &str =
    "Windows signs in by itself. The password is for permission prompts and Remote Desktop.";
const LINUX_NOTE: &str = "For a console login if the desktop won't start. The app gets in with its own key.";

pub struct Account;

impl Account {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let machine = shared.machine.borrow().clone();
        let note = match machine.kind {
            Kind::Windows => WINDOWS_NOTE,
            Kind::Linux => LINUX_NOTE,
            Kind::Plain => return Self,
        };
        let group = adw::PreferencesGroup::builder().title("Account").build();
        page.add(&group);
        let file = machine.password_file(&shared.window.context());
        let row = adw::ActionRow::builder()
            .title(machine.account())
            .subtitle(format!("{note} The password is in {}.", file.display()))
            .use_markup(false)
            .subtitle_selectable(true)
            .build();
        group.add(&row);
        Self
    }
}
