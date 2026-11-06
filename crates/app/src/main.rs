//! `guesthouse`: the desktop app.

use std::env;

const GTK_DEBUG_VARIABLE: &str = "GDK_DEBUG";
/// GTK hides Wayland's colour management from itself without this, and HDR needs it. It's a debug
/// flag, so a new GTK could change it.
const COLOUR_MANAGEMENT: &str = "color-mgmt";

fn with_colour_management(flags: Option<String>) -> String {
    match flags {
        Some(flags) if flags.split([',', ':']).any(|flag| flag == COLOUR_MANAGEMENT) => flags,
        Some(flags) if !flags.is_empty() => format!("{flags},{COLOUR_MANAGEMENT}"),
        _ => COLOUR_MANAGEMENT.to_owned(),
    }
}

fn main() -> gtk::glib::ExitCode {
    let flags = with_colour_management(env::var(GTK_DEBUG_VARIABLE).ok());
    // SAFETY: nothing else is running yet, and GTK reads this when it starts, which is after this.
    unsafe { env::set_var(GTK_DEBUG_VARIABLE, flags) };
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
}
