//! `guesthouse`: the desktop app.

use std::env;

const GTK_DEBUG_VARIABLE: &str = "GDK_DEBUG";
/// GTK hides Wayland's colour management from itself without this, and HDR needs it. It's a debug
/// flag, so a new GTK could change it.
const COLOUR_MANAGEMENT: &str = "color-mgmt";
/// The first GTK that has that flag. An older one prints a warning at anyone who starts the app.
const FIRST_GTK_WITH_THE_FLAG: (u32, u32, u32) = (4, 16, 6);

/// What `GDK_DEBUG` should be for this GTK. None leaves it as it is.
fn debug_flags(flags: Option<String>, gtk: (u32, u32, u32)) -> Option<String> {
    (gtk >= FIRST_GTK_WITH_THE_FLAG).then(|| with_colour_management(flags))
}

fn with_colour_management(flags: Option<String>) -> String {
    match flags {
        Some(flags) if flags.split([',', ':']).any(|flag| flag == COLOUR_MANAGEMENT) => flags,
        Some(flags) if !flags.is_empty() => format!("{flags},{COLOUR_MANAGEMENT}"),
        _ => COLOUR_MANAGEMENT.to_owned(),
    }
}

fn main() -> gtk::glib::ExitCode {
    let gtk = (gtk::major_version(), gtk::minor_version(), gtk::micro_version());
    if let Some(flags) = debug_flags(env::var(GTK_DEBUG_VARIABLE).ok(), gtk) {
        // SAFETY: nothing else is running yet, and GTK reads this when it starts, which is after this.
        unsafe { env::set_var(GTK_DEBUG_VARIABLE, flags) };
    }
    guesthouse_app::application::run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_own_flags_stay() {
        assert_eq!(with_colour_management(None), COLOUR_MANAGEMENT);
        assert_eq!(with_colour_management(Some("gl-debug".into())), "gl-debug,color-mgmt");
        assert_eq!(with_colour_management(Some("color-mgmt,gl-debug".into())), "color-mgmt,gl-debug");
    }

    #[test]
    fn a_gtk_without_the_flag_is_not_given_it() {
        assert_eq!(debug_flags(None, (4, 14, 5)), None);
        assert_eq!(debug_flags(Some("gl-debug".into()), (4, 16, 5)), None);
    }

    #[test]
    fn a_gtk_with_the_flag_is_given_it() {
        assert_eq!(debug_flags(None, (4, 16, 6)).as_deref(), Some(COLOUR_MANAGEMENT));
        assert_eq!(debug_flags(None, (4, 24, 0)).as_deref(), Some(COLOUR_MANAGEMENT));
    }
}
