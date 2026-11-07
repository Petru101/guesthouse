//! Where a mouse capture is, between the key, the compositor and the guest. Nothing here touches a
//! window, so the key's logic can be tested without one.

/// How far a capture has got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Capture {
    #[default]
    Off,
    /// The compositor has been asked, and locks the pointer once it's over the window.
    Asked,
    /// The pointer is locked and its motion goes to the guest.
    Held,
}

/// What the screen has to do after a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Ask the compositor for the lock.
    Ask,
    /// The lock is held: hand the guest the mouse.
    Enter,
    /// Give the lock up, and the mouse back to the tablet if the guest had it.
    Leave {
        had_mouse: bool,
    },
}

impl Capture {
    pub fn asked(self) -> bool {
        self != Self::Off
    }

    pub fn held(self) -> bool {
        self == Self::Held
    }

    /// The key or the button: one press starts a capture, and the next ends it, whether or not the
    /// compositor has granted it yet.
    pub fn toggled(self) -> (Self, Step) {
        match self {
            Self::Off => (Self::Asked, Step::Ask),
            Self::Asked | Self::Held => (Self::Off, self.leaving()),
        }
    }

    /// Focus went, the guest went, or the compositor has ended the lock.
    pub fn dropped(self) -> (Self, Step) {
        match self {
            Self::Off => (Self::Off, Step::Nothing),
            Self::Asked | Self::Held => (Self::Off, self.leaving()),
        }
    }

    /// What the compositor has said since it was asked. A lock granted after the capture ended is
    /// nothing to act on: the screen has already let it go.
    pub fn followed(self, held: bool, ended: bool, guest_alive: bool) -> (Self, Step) {
        match self {
            Self::Off => (Self::Off, Step::Nothing),
            _ if ended || !guest_alive => self.dropped(),
            Self::Asked if held => (Self::Held, Step::Enter),
            _ => (self, Step::Nothing),
        }
    }

    fn leaving(self) -> Step {
        Step::Leave { had_mouse: self.held() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_asks_and_then_lets_go() {
        let (asked, step) = Capture::Off.toggled();
        assert_eq!((asked, step), (Capture::Asked, Step::Ask));
        let (held, step) = asked.followed(true, false, true);
        assert_eq!((held, step), (Capture::Held, Step::Enter));
        assert_eq!(held.toggled(), (Capture::Off, Step::Leave { had_mouse: true }));
    }

    #[test]
    fn a_second_press_before_the_lock_is_granted_takes_the_ask_back() {
        let (asked, _) = Capture::Off.toggled();
        assert_eq!(asked.toggled(), (Capture::Off, Step::Leave { had_mouse: false }));
    }

    #[test]
    fn a_lock_granted_after_the_key_let_go_is_ignored() {
        let (off, _) = Capture::Off.toggled().0.toggled();
        assert_eq!(off.followed(true, false, true), (Capture::Off, Step::Nothing));
        assert_eq!(off.dropped(), (Capture::Off, Step::Nothing));
    }

    #[test]
    fn the_compositor_ending_the_lock_gives_the_mouse_back() {
        let held = Capture::Held;
        assert_eq!(held.followed(false, true, true), (Capture::Off, Step::Leave { had_mouse: true }));
        assert_eq!(
            Capture::Asked.followed(false, true, true),
            (Capture::Off, Step::Leave { had_mouse: false })
        );
    }

    #[test]
    fn a_guest_that_went_ends_the_capture() {
        assert_eq!(
            Capture::Held.followed(true, false, false),
            (Capture::Off, Step::Leave { had_mouse: true })
        );
    }

    #[test]
    fn nothing_said_yet_changes_nothing() {
        assert_eq!(Capture::Asked.followed(false, false, true), (Capture::Asked, Step::Nothing));
        assert_eq!(Capture::Held.followed(true, false, true), (Capture::Held, Step::Nothing));
    }

    #[test]
    fn losing_focus_drops_a_capture_at_any_stage() {
        assert_eq!(Capture::Asked.dropped(), (Capture::Off, Step::Leave { had_mouse: false }));
        assert_eq!(Capture::Held.dropped(), (Capture::Off, Step::Leave { had_mouse: true }));
    }
}
