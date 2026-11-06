//! Locking the pointer to the window, for games. Only a compositor can do this, so it's behind a small
//! interface, and a desktop that can't has none.

use gtk::gdk;

pub trait PointerLock {
    /// Asks for the lock on a surface, and says if it could. It starts when the pointer is on it.
    fn lock(&mut self, surface: &gdk::Surface) -> bool;
    fn unlock(&mut self);
    /// Whole units moved since the last call, without the compositor's acceleration.
    fn take_motion(&mut self) -> (i64, i64);
    /// The compositor has granted the lock.
    fn held(&self) -> bool;
    /// The compositor has ended it, as when the window loses focus.
    fn ended(&self) -> bool;
}

/// The display's lock. `None` if this desktop can't do it.
pub fn pointer_lock(_display: &gdk::Display) -> Option<Box<dyn PointerLock>> {
    None
}
