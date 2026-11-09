//! Newer releases, as the install's own script finds them, and updating to one from the app. A build of
//! the repo has no such script, so it never asks and can't update itself.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::log::write_log;
use std::process::{Command, Stdio};

const INSTALL_SCRIPT: &str = "install.sh";
const CHECK_OPTION: &str = "--check-update";
const UPDATE_OPTION: &str = "--update";
/// Tells the script the app is running it, and which process the app is.
const FROM_APP_OPTION: &str = "--from-app=";
/// What the script exits with when there's something newer.
const UPDATE_AVAILABLE: i32 = 10;
const UPDATE_WORD: &str = "update-available";
const UP_TO_DATE_WORD: &str = "up-to-date";
/// The script gives up on the network well before this.
const MOST_SECONDS: &str = "60";
/// Where the script says what went wrong: `install.sh: ` and the reason.
const SCRIPT_PREFIX: &str = "install.sh: ";
const NO_SCRIPT: &str =
    "This copy of Guesthouse wasn't installed with install.sh, so it can't update itself.";

/// The version this app is.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Newer(String),
    Latest,
    /// No network, or the script couldn't say. It isn't worth bothering anyone about by itself.
    CantTell,
    /// A build of the repo, which updates through git.
    NotInstalled,
}

/// What the script said, from a line like `update-available 1.0 1.1` or `up-to-date 1.1`.
fn read_answer(said: &str, exit: Option<i32>) -> Found {
    let words = |word: &str| -> Option<Vec<&str>> {
        Some(said.lines().find(|line| line.starts_with(word))?.split_whitespace().collect())
    };
    match exit {
        Some(UPDATE_AVAILABLE) => match words(UPDATE_WORD).as_deref() {
            Some([_, _, newest]) => Found::Newer((*newest).to_owned()),
            _ => Found::CantTell,
        },
        Some(0) if words(UP_TO_DATE_WORD).is_some() => Found::Latest,
        _ => Found::CantTell,
    }
}

/// Whether there's a newer release. This can take a while, so it's for a thread.
pub fn check(context: &Context) -> Found {
    let script = context.paths.helper(INSTALL_SCRIPT);
    if !script.is_file() {
        return Found::NotInstalled;
    }
    let Ok(result) = Command::new("timeout")
        .args([MOST_SECONDS, "sh"])
        .arg(script)
        .arg(CHECK_OPTION)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Found::CantTell;
    };
    read_answer(&String::from_utf8_lossy(&result.stdout), result.status.code())
}

/// The script's own reason for stopping, or its last words if it gave none.
fn reason(said: &str) -> String {
    let last = said.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
    let reason = said.lines().rev().find_map(|line| line.strip_prefix(SCRIPT_PREFIX)).unwrap_or(last);
    format!("The update stopped: {reason}")
}

/// Brings the install up to the latest release, while the app (process `app`) runs. Its VMs must be
/// off. The app has asked already, and if root is needed, pkexec asks for the password in a window.
/// Everything the script said goes in the log. This takes minutes, so it's for a thread.
pub fn update(context: &Context, app: u32) -> Result<()> {
    let script = context.paths.helper(INSTALL_SCRIPT);
    if !script.is_file() {
        return Err(Failure::new(NO_SCRIPT));
    }
    let result = Command::new("sh")
        .arg(script)
        .arg(UPDATE_OPTION)
        .arg(format!("{FROM_APP_OPTION}{app}"))
        .stdin(Stdio::null())
        .output()?;
    let said =
        format!("{}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    let _ = write_log(context, &format!("app: the update said:\n{said}"));
    if result.status.success() { Ok(()) } else { Err(Failure::new(reason(&said))) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn the_newest_version_is_the_lines_last_word() {
        assert_eq!(read_answer("update-available 1.0 1.1\n", Some(10)), Found::Newer("1.1".to_owned()));
        assert_eq!(read_answer("update-available 1.0\n", Some(10)), Found::CantTell);
        assert_eq!(read_answer("up-to-date 1.1\n", Some(0)), Found::Latest);
        assert_eq!(read_answer("unknown 1.1\n", Some(1)), Found::CantTell);
        assert_eq!(read_answer("", None), Found::CantTell);
    }

    fn with_script(scratch: &Scratch, text: &str) -> Found {
        scratch.write("program/install.sh", text);
        check(&scratch_context(scratch))
    }

    #[test]
    fn a_newer_release_is_said() {
        let scratch = Scratch::new();
        let found = with_script(&scratch, "echo update-available 1.0 1.2\nexit 10\n");
        assert_eq!(found, Found::Newer("1.2".to_owned()));
    }

    #[test]
    fn being_up_to_date_is_told_from_not_knowing() {
        let scratch = Scratch::new();
        assert_eq!(with_script(&scratch, "echo up-to-date 1.0\nexit 0\n"), Found::Latest);
        assert_eq!(with_script(&scratch, "echo unknown 1.0\nexit 1\n"), Found::CantTell);
    }

    #[test]
    fn a_build_with_no_script_never_asks() {
        let scratch = Scratch::new();
        assert_eq!(check(&scratch_context(&scratch)), Found::NotInstalled);
        assert_eq!(update(&scratch_context(&scratch), 1), Err(Failure::new(NO_SCRIPT)));
    }

    #[test]
    fn the_update_says_which_process_the_app_is() {
        let scratch = Scratch::new();
        scratch.write("program/install.sh", "[ \"$1 $2\" = '--update --from-app=42' ] || exit 3\n");
        assert_eq!(update(&scratch_context(&scratch), 42), Ok(()));
    }

    #[test]
    fn a_stopped_update_gives_the_scripts_reason() {
        let scratch = Scratch::new();
        scratch.write(
            "program/install.sh",
            "echo Checking this PC.\necho 'install.sh: Guesthouse is running, so nothing changes yet.' >&2\n\
             echo '123 qemu-system-x86_64' >&2\nexit 1\n",
        );
        let failure = update(&scratch_context(&scratch), 1).unwrap_err();
        assert_eq!(failure.0, "The update stopped: Guesthouse is running, so nothing changes yet.");
    }
}
