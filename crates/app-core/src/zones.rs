//! The host's time zone, in the words Windows Setup wants.
//!
//! The table from IANA zones to Windows' names isn't in this repo. It ships with the guest's other
//! files, as lines of `IANA-name<TAB>Windows-name`. Without it a guest stays on UTC, which is also
//! what the answer file's template says.

use crate::context::Context;
use chrono::{Datelike, Local};
use std::fs;
use std::path::Path;
use std::process::Command;

pub const HOST_ZONE_LINK: &str = "/etc/localtime";
const ZONE_TABLE: &str = "windows-zones.txt";
const ZONE_DATABASE: &str = "/usr/share/zoneinfo";
const FALLBACK: &str = "UTC";

/// The host's IANA zone, from where the link points. Empty if that says nothing.
pub fn host_zone(link: impl AsRef<Path>) -> String {
    let target = fs::read_link(link).unwrap_or_default();
    let text = target.to_string_lossy();
    text.split_once("zoneinfo/").map_or_else(String::new, |(_, zone)| zone.to_owned())
}

fn read_table(context: &Context) -> Vec<(String, String)> {
    let text = fs::read_to_string(context.paths.guest_files().join(ZONE_TABLE)).unwrap_or_default();
    let split = |line: &str| line.split_once('\t').map(|(zone, name)| (zone.to_owned(), name.to_owned()));
    text.lines().filter_map(split).filter(|(_, name)| plain_name(name)).collect()
}

/// A bad value must never reach Windows Setup.
fn plain_name(name: &str) -> bool {
    name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b" .+()-".contains(&byte))
}

fn is_zone(zone: &str) -> bool {
    let safe = !zone.is_empty() && !zone.split('/').any(|part| part == ".." || part.is_empty());
    safe && Path::new(ZONE_DATABASE).join(zone).is_file()
}

/// A zone's UTC offsets on 1 January and 1 July of a year, or `None` if there's no such zone.
fn offsets(zone: &str, year: i32) -> Option<(String, String)> {
    if !is_zone(zone) {
        return None;
    }
    let offset = |month: u32| {
        let moment = format!("{year}-{month:02}-01 12:00 UTC");
        let result = Command::new("date").env("TZ", zone).args(["-d", &moment, "+%z"]).output().ok()?;
        Some(String::from_utf8_lossy(&result.stdout).trim().to_owned())
    };
    Some((offset(1)?, offset(7)?))
}

/// Windows' name for an IANA zone. One not in the table gets the first zone with the same offsets,
/// which shows the right local time but can differ in when daylight saving changes. That can leave
/// the guest an hour out for some weeks of the year. The offsets are next year's, as a zone that has
/// just changed its rules still has this year's old ones. With no match it stays UTC.
pub fn windows_zone(context: &Context, zone: &str) -> String {
    let table = read_table(context);
    if let Some((_, name)) = table.iter().find(|(known, _)| known == zone) {
        return name.clone();
    }
    let year = Local::now().year() + 1;
    let Some(wanted) = offsets(zone, year) else { return FALLBACK.to_owned() };
    let same = table.iter().find(|(known, _)| offsets(known, year).as_ref() == Some(&wanted));
    same.map_or_else(|| FALLBACK.to_owned(), |(_, name)| name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};
    use std::os::unix::fs::symlink;

    /// Made-up Windows names, so that no real table is needed here.
    fn with_table(scratch: &Scratch) -> Context {
        scratch.write(
            "program/guest/windows-zones.txt",
            "Etc/UTC\tUTC\nAmerica/Los_Angeles\tCoast Zone\nAsia/Tokyo\tIsland Zone\nEurope/Berlin\tMiddle Zone\nbad/one\t<x>\n",
        );
        scratch_context(scratch)
    }

    #[test]
    fn a_known_zone_comes_from_the_table() {
        let scratch = Scratch::new();
        let context = with_table(&scratch);
        assert_eq!(windows_zone(&context, "America/Los_Angeles"), "Coast Zone");
        assert_eq!(windows_zone(&context, "Etc/UTC"), "UTC");
    }

    #[test]
    fn an_unknown_zone_stays_utc() {
        let scratch = Scratch::new();
        let context = with_table(&scratch);
        for zone in ["Mars/Olympus_Mons", "", "<TimeZone>", "America", "../../etc/passwd", "bad/one"] {
            assert_eq!(windows_zone(&context, zone), "UTC", "{zone}");
        }
    }

    #[test]
    fn a_zone_not_in_the_table_takes_one_with_the_same_offsets() {
        let scratch = Scratch::new();
        let context = with_table(&scratch);
        if !is_zone("Europe/Paris") {
            return;
        }
        assert_eq!(windows_zone(&context, "Europe/Paris"), "Middle Zone");
        assert_eq!(windows_zone(&context, "Asia/Seoul"), "Island Zone");
    }

    #[test]
    fn with_no_table_a_guest_stays_on_utc() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(windows_zone(&context, "Europe/Paris"), "UTC");
    }

    #[test]
    fn the_hosts_zone_is_read_from_the_link() {
        let scratch = Scratch::new();
        let link = scratch.path("localtime");
        assert_eq!(host_zone(&link), "", "no link says nothing");
        symlink("../usr/share/zoneinfo/America/Los_Angeles", &link).unwrap();
        assert_eq!(host_zone(&link), "America/Los_Angeles");
        fs::remove_file(&link).unwrap();
        symlink("/some/file", &link).unwrap();
        assert_eq!(host_zone(&link), "", "not a zone");
    }
}
