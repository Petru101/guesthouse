//! The account an unattended install makes in the guest. The user may name it and give it a password
//! before the install; otherwise it gets the name the guest scripts have always used, and a random
//! password that stays in the machine's folder.

use crate::machine::Kind;

/// The Windows account, which signs in by itself.
pub const WINDOWS_USER: &str = "gpupv";
/// Ubuntu's desktop user.
pub const LINUX_USER: &str = "desk";
/// Both installers take the password as one line, and Windows stops at 127 characters.
const MOST_PASSWORD_CHARS: usize = 127;
const MOST_WINDOWS_NAME_CHARS: usize = 20;
const MOST_LINUX_NAME_CHARS: usize = 32;
/// Names Windows has, or makes during setup, so an account of that name can't be made.
const WINDOWS_RESERVED: [&str; 8] = [
    "administrator",
    "guest",
    "defaultaccount",
    "defaultuser0",
    "defaultuser1",
    "wdagutilityaccount",
    "system",
    "krbtgt",
];
/// Ubuntu's own users, which the installer refuses to make again.
const LINUX_RESERVED: [&str; 32] = [
    "root",
    "daemon",
    "bin",
    "sys",
    "sync",
    "games",
    "man",
    "lp",
    "mail",
    "news",
    "uucp",
    "proxy",
    "www-data",
    "backup",
    "list",
    "irc",
    "nobody",
    "_apt",
    "sshd",
    "messagebus",
    "polkitd",
    "gdm",
    "gnome-initial-setup",
    "tss",
    "uuidd",
    "tcpdump",
    "landscape",
    "fwupd-refresh",
    "pollinate",
    "lxd",
    "dhcpcd",
    "syslog",
];

/// The name the guest's account gets when the user chose none.
pub fn default_user(kind: Kind) -> &'static str {
    match kind {
        Kind::Linux => LINUX_USER,
        Kind::Windows | Kind::Plain => WINDOWS_USER,
    }
}

fn windows_name_problem(name: &str) -> Option<String> {
    let allowed = |letter: char| letter.is_ascii_alphanumeric() || matches!(letter, '-' | '_' | '.' | ' ');
    if name.chars().count() > MOST_WINDOWS_NAME_CHARS {
        return Some(format!("A Windows user name has at most {MOST_WINDOWS_NAME_CHARS} characters."));
    }
    if !name.chars().all(allowed) || name != name.trim() {
        return Some(
            "A Windows user name can hold letters, digits, dots, dashes, underscores and spaces between words."
                .to_owned(),
        );
    }
    if name.chars().all(|letter| letter == '.') {
        return Some("A Windows user name needs a letter or a digit.".to_owned());
    }
    if WINDOWS_RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
        return Some(format!("Windows has an account called {name} already. Choose another name."));
    }
    None
}

fn linux_name_problem(name: &str) -> Option<String> {
    let mut letters = name.chars();
    let first_fits = letters.next().is_some_and(|first| first.is_ascii_lowercase() || first == '_');
    let rest_fit = letters
        .all(|letter| letter.is_ascii_lowercase() || letter.is_ascii_digit() || matches!(letter, '-' | '_'));
    if name.chars().count() > MOST_LINUX_NAME_CHARS {
        return Some(format!("A Linux user name has at most {MOST_LINUX_NAME_CHARS} characters."));
    }
    if !first_fits || !rest_fit {
        return Some(
            "A Linux user name starts with a small letter or an underscore, and holds small letters, digits, dashes and underscores."
                .to_owned(),
        );
    }
    if LINUX_RESERVED.contains(&name) || name.starts_with("systemd-") {
        return Some(format!("Ubuntu has a user called {name} already. Choose another name."));
    }
    None
}

/// Why this name won't do for the account, if it won't. Empty means the default, which is fine.
pub fn name_problem(kind: Kind, name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    match kind {
        Kind::Windows => windows_name_problem(name),
        Kind::Linux => linux_name_problem(name),
        Kind::Plain => None,
    }
}

/// Why this password won't do, if it won't. Empty means a random one, which is fine.
pub fn password_problem(password: &str) -> Option<String> {
    if password.chars().count() > MOST_PASSWORD_CHARS {
        return Some(format!("The password has at most {MOST_PASSWORD_CHARS} characters."));
    }
    if password.chars().any(char::is_control) {
        return Some("The password can't hold line breaks or tabs.".to_owned());
    }
    None
}

/// The text as it goes inside an XML element or attribute.
pub fn xml_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for letter in text.chars() {
        match letter {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_names_the_scripts_have_always_used() {
        assert_eq!(default_user(Kind::Windows), "gpupv");
        assert_eq!(default_user(Kind::Linux), "desk");
        assert_eq!(name_problem(Kind::Windows, ""), None);
        assert_eq!(name_problem(Kind::Linux, ""), None);
    }

    #[test]
    fn a_windows_name_is_what_windows_takes() {
        for name in ["Ana", "ana.b", "Ana B", "user-1", "a_b", "x".repeat(20).as_str()] {
            assert_eq!(name_problem(Kind::Windows, name), None, "{name}");
        }
        for (name, said) in [
            ("x".repeat(21).as_str(), "at most 20"),
            ("ana/b", "can hold"),
            ("ana@b", "can hold"),
            (" ana", "can hold"),
            ("ana'b", "can hold"),
            ("...", "needs a letter"),
            ("Administrator", "already"),
            ("defaultuser0", "already"),
        ] {
            assert!(
                name_problem(Kind::Windows, name).is_some_and(|problem| problem.contains(said)),
                "{name}"
            );
        }
    }

    #[test]
    fn a_linux_name_is_what_the_installer_takes() {
        for name in ["ana", "_ana", "ana-b_2", "x".repeat(32).as_str()] {
            assert_eq!(name_problem(Kind::Linux, name), None, "{name}");
        }
        for (name, said) in [
            ("x".repeat(33).as_str(), "at most 32"),
            ("Ana", "small letter"),
            ("1ana", "small letter"),
            ("ana b", "small letter"),
            ("root", "already"),
            ("systemd-network", "already"),
        ] {
            assert!(name_problem(Kind::Linux, name).is_some_and(|problem| problem.contains(said)), "{name}");
        }
    }

    #[test]
    fn a_plain_machine_has_no_account_to_name() {
        assert_eq!(name_problem(Kind::Plain, "anything goes/"), None);
    }

    #[test]
    fn a_password_is_one_line_windows_can_take() {
        assert_eq!(password_problem(""), None);
        assert_eq!(password_problem("p4ss <&> \"quoted\" 'and' ünïcode"), None);
        assert!(password_problem(&"x".repeat(128)).is_some());
        assert!(password_problem("two\nlines").is_some());
        assert!(password_problem("a\tb").is_some());
    }

    #[test]
    fn xml_text_escapes_what_xml_reads() {
        assert_eq!(xml_text("a<b>&\"c'"), "a&lt;b&gt;&amp;&quot;c&apos;");
        assert_eq!(xml_text("plain"), "plain");
    }
}
