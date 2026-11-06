//! Going through the slots. What each one owns is named in the daemon's own `slot` module.

use guesthouse_daemon::slot::{SLOTS, Slot};

pub fn first() -> Slot {
    number(0)
}

pub fn all() -> impl Iterator<Item = Slot> {
    (0..SLOTS).map(number)
}

/// Panics on a number no slot has, which only a mistake in this crate could pass.
pub fn number(number: u8) -> Slot {
    Slot::parse(Some(&number.to_string())).expect("a slot number below SLOTS")
}
