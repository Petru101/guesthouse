//! The repo's own guest folder: everything the app sends is there, and nothing that must not be.

use std::fs;
use std::path::PathBuf;

const SENT_TO_THE_GUEST: [&str; 13] = [
    "SetupComplete.cmd",
    "agent.ps1",
    "autounattend.xml",
    "check.ps1",
    "clipboard-ask.ps1",
    "clipboard-start.ps1",
    "clipboard.ps1",
    "display.ps1",
    "finish.ps1",
    "nv-files.ps1",
    "setmode.ps1",
    "setup.ps1",
    "share-start.ps1",
];

fn guest_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../guest").join(name)
}

#[test]
fn everything_the_app_sends_is_in_the_repo() {
    for name in SENT_TO_THE_GUEST {
        assert!(guest_file(name).is_file(), "{name} is missing");
    }
}

#[test]
fn the_answer_file_holds_no_password() {
    let answers = fs::read_to_string(guest_file("autounattend.xml")).unwrap();
    assert_eq!(answers.matches("<Value>@PASSWORD@</Value>").count(), 2);
    assert_eq!(answers.matches("<PlainText>true</PlainText>").count(), 2);
}

#[test]
fn only_the_host_reaches_the_agent() {
    let setup = fs::read_to_string(guest_file("setup.ps1")).unwrap();
    assert!(setup.contains("-RemoteAddress 10.0.2.2"));
    let agent = fs::read_to_string(guest_file("agent.ps1")).unwrap();
    assert!(agent.contains("$hostAddress = '10.0.2.2'"));
    assert!(agent.contains("RemoteEndPoint"));
    assert!(agent.contains("ReadTimeout"));
}

#[test]
fn no_token_is_kept_in_the_folder() {
    assert!(!guest_file(".token").exists());
    assert!(!guest_file("token.txt").exists());
}
