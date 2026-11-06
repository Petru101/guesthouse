//! Whether a newer release is out, as the install's own script says. A build of the repo has no such
//! script, so it never asks. It says nothing if it can't tell: no network is no reason to bother anyone.

use crate::context::Context;
use std::process::{Command, Stdio};

const INSTALL_SCRIPT: &str = "install.sh";
const CHECK_OPTION: &str = "--check-update";
/// What the script exits with when there's something newer.
const UPDATE_AVAILABLE: i32 = 10;
const UPDATE_WORD: &str = "update-available";
/// The script gives up on the network well before this.
const MOST_SECONDS: &str = "60";

/// The newest version, from a line like `update-available 1.0 1.1`.
fn newer_version(said: &str) -> Option<String> {
    let line = said.lines().find(|line| line.starts_with(UPDATE_WORD))?;
    let [_, _, newest] = line.split_whitespace().collect::<Vec<_>>()[..] else { return None };
    Some(newest.to_owned())
}

/// The version to update to, if there's a newer one. This can take a while, so it's for a thread.
pub fn newer_release(context: &Context) -> Option<String> {
    let script = context.paths.helper(INSTALL_SCRIPT);
    if !script.is_file() {
        return None;
    }
    let result = Command::new("timeout")
        .args([MOST_SECONDS, "sh"])
        .arg(script)
        .arg(CHECK_OPTION)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    (result.status.code() == Some(UPDATE_AVAILABLE))
        .then(|| newer_version(&String::from_utf8_lossy(&result.stdout)))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn the_newest_version_is_the_lines_last_word() {
        assert_eq!(newer_version("update-available 1.0 1.1\n"), Some("1.1".to_owned()));
        assert_eq!(newer_version("up-to-date 1.1\n"), None);
        assert_eq!(newer_version("update-available 1.0\n"), None);
    }

    fn with_script(scratch: &Scratch, text: &str) -> Option<String> {
        scratch.write("program/install.sh", text);
        newer_release(&scratch_context(scratch))
    }

    #[test]
    fn a_newer_release_is_said() {
        let scratch = Scratch::new();
        let found = with_script(&scratch, "echo update-available 1.0 1.2\nexit 10\n");
        assert_eq!(found, Some("1.2".to_owned()));
    }

    #[test]
    fn nothing_is_said_when_up_to_date_or_when_it_cant_tell() {
        let scratch = Scratch::new();
        assert_eq!(with_script(&scratch, "echo up-to-date 1.0\nexit 0\n"), None);
        assert_eq!(with_script(&scratch, "echo unknown 1.0\nexit 1\n"), None);
    }

    #[test]
    fn a_build_with_no_script_never_asks() {
        let scratch = Scratch::new();
        assert_eq!(newer_release(&scratch_context(&scratch)), None);
    }
}
