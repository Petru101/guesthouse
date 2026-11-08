//! Whether a GPU machine, Windows or Linux, may start with no jail, on a PC that can't give it one.
//! Off unless the user turns it on, and the machine's row says so whenever it runs that way.

use super::Shared;
use adw::prelude::*;
use guesthouse_app_core::machine::Kind;

const NOTE: &str = "Only on a PC whose kernel has no Landlock, where QEMU and the daemon then run as you with \
                    nothing around them. A Linux guest gets its GPU that way, and without it starts with none. A \
                    guest that broke out would have your account. The machine's row says so while it runs.";

pub struct Jail;

impl Jail {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let machine = shared.machine.borrow().clone();
        if !machine.has_gpu() && machine.kind != Kind::Linux {
            return Self;
        }
        let group = adw::PreferencesGroup::builder().title("Jail").build();
        page.add(&group);
        let row = adw::SwitchRow::builder()
            .title("Start without a jail")
            .subtitle(NOTE)
            .active(machine.start_without_jail)
            .build();
        let saving = shared.clone();
        row.connect_active_notify(move |row| {
            saving.machine.borrow_mut().start_without_jail = row.is_active();
            saving.save();
        });
        group.add(&row);
        Self
    }
}
