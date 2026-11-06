//! The host's time zone, in the words Windows Setup wants.
//!
//! The table comes from Unicode's public CLDR data, under the Unicode License v3: `data/windows-zones.txt`
//! says where. Anything it can't place stays on UTC, which is also what the answer file's template says.

use chrono::{Datelike, Local};
use std::fs;
use std::path::Path;
use std::process::Command;

pub const HOST_ZONE_LINK: &str = "/etc/localtime";
const ZONE_TABLE: &str = include_str!("../data/windows-zones.txt");
const ZONE_DATABASE: &str = "/usr/share/zoneinfo";
const FALLBACK: &str = "UTC";

/// The host's IANA zone, from where the link points. Empty if that says nothing.
pub fn host_zone(link: impl AsRef<Path>) -> String {
    let target = fs::read_link(link).unwrap_or_default();
    let text = target.to_string_lossy();
    text.split_once("zoneinfo/").map_or_else(String::new, |(_, zone)| zone.to_owned())
}

fn table() -> Vec<(&'static str, &'static str)> {
    let lines = ZONE_TABLE.lines().filter(|line| !line.starts_with('#'));
    lines.filter_map(|line| line.split_once('\t')).filter(|(_, name)| plain_name(name)).collect()
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
pub fn windows_zone(zone: &str) -> String {
    let table = table();
    if let Some((_, name)) = table.iter().find(|(known, _)| *known == zone) {
        return (*name).to_owned();
    }
    let year = Local::now().year() + 1;
    let Some(wanted) = offsets(zone, year) else { return FALLBACK.to_owned() };
    let same = table.iter().find(|(known, _)| offsets(known, year).as_ref() == Some(&wanted));
    same.map_or_else(|| FALLBACK.to_owned(), |(_, name)| (*name).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_known_zone_comes_from_the_table() {
        assert_eq!(windows_zone("America/Los_Angeles"), "Pacific Standard Time");
        assert_eq!(windows_zone("Europe/Berlin"), "W. Europe Standard Time");
        assert_eq!(windows_zone("Asia/Tokyo"), "Tokyo Standard Time");
        assert_eq!(windows_zone("Etc/UTC"), "UTC");
    }

    #[test]
    fn the_table_is_whole_and_holds_plain_names() {
        assert_eq!(table().len(), 139);
        assert_eq!(table()[0], ("Etc/UTC", "UTC"));
    }

    #[test]
    fn an_unknown_zone_stays_utc() {
        for zone in ["Mars/Olympus_Mons", "", "<TimeZone>", "America", "../../etc/passwd", "Antarctica/Troll"]
        {
            assert_eq!(windows_zone(zone), "UTC", "{zone}");
        }
    }

    #[test]
    fn a_zone_not_in_the_table_takes_one_with_the_same_offsets() {
        if !is_zone("Europe/Amsterdam") {
            return;
        }
        for (zone, name) in [
            ("Asia/Kolkata", "India Standard Time"),
            ("Europe/Kyiv", "GTB Standard Time"),
            ("Europe/Amsterdam", "W. Europe Standard Time"),
            ("Australia/Sydney", "AUS Eastern Standard Time"),
            ("America/Phoenix", "US Mountain Standard Time"),
        ] {
            assert_eq!(windows_zone(zone), name, "{zone}");
        }
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
