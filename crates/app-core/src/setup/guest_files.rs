//! What a normal install of NVIDIA's driver puts in Windows' own folders, read from its INF. A guest
//! has only the driver's folder, so a program that looks for one of these by name doesn't find it.
//! This holds file names only, and they come from the user's own driver.

use super::driver::read_infs;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::settings::{driver_folder, read_setup};
use crate::usb::sysfs_text;
use regex::Regex;
use std::path::Path;

/// Destinations as an INF writes them: 11 is System32, and 10 is Windows.
const WINDOWS_FOLDERS: [(&str, &str); 2] = [("11", "System32"), ("10,syswow64", "SysWOW64")];
const SECTION_HEADER: &str = r"^\[(.+?)\]\s*(;.*)?$";
const DEVICE_LIST_PREFIX: &str = "nvidia_devices.";
const DEFAULT_DESTINATION: &str = "defaultdestdir";
const SUBSYSTEM_PREFIX: &str = "&subsys_";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestFile {
    pub folder: String,
    pub source: String,
    pub name: String,
}

/// The sections in file order, as the last list of devices is the one for the newest Windows. Names
/// are lower case, as an INF's are in any case.
struct Sections(Vec<(String, Vec<String>)>);

impl Sections {
    fn read(text: &str) -> Self {
        let header = Regex::new(SECTION_HEADER).unwrap();
        let mut found: Vec<(String, Vec<String>)> = Vec::new();
        let mut current = None;
        for line in text.lines().map(str::trim) {
            if let Some(parts) = header.captures(line) {
                let name = parts[1].to_lowercase();
                let known = found.iter().position(|section| section.0 == name);
                current = Some(known.unwrap_or_else(|| {
                    found.push((name, Vec::new()));
                    found.len() - 1
                }));
            } else if !line.is_empty()
                && !line.starts_with(';')
                && let Some(index) = current
            {
                found[index].1.push(line.to_owned());
            }
        }
        Self(found)
    }

    fn lines(&self, name: &str) -> &[String] {
        self.0.iter().find(|section| section.0 == name).map_or(&[], |section| &section.1)
    }

    /// The card's install section, and how well its line fits: 2 for the card's own subsystem, 1 for
    /// a line with none, and 0 for another laptop's. Of lines that fit as well, the last wins.
    fn card_section(&self, device: u32, subsystem: u32) -> Option<(u8, String)> {
        let wanted = format!("pci\\ven_10de&dev_{device:04x}");
        let lists = self.0.iter().filter(|section| section.0.starts_with(DEVICE_LIST_PREFIX));
        let lines = lists.flat_map(|section| &section.1);
        let sections = lines.filter_map(|line| {
            let (_, value) = line.split_once('=')?;
            let mut parts = value.split(',').map(str::trim);
            let section = parts.next()?;
            let id = parts.next()?.to_lowercase();
            let rest = id.strip_prefix(&wanted)?;
            let fit = match rest.strip_prefix(SUBSYSTEM_PREFIX) {
                None if rest.is_empty() || rest.starts_with('&') => 1,
                None => return None,
                Some(given) => {
                    let given = given.get(..8).and_then(|digits| u32::from_str_radix(digits, 16).ok());
                    if given == Some(subsystem) { 2 } else { 0 }
                }
            };
            Some((fit, section.to_lowercase()))
        });
        sections.fold(None, |best: Option<(u8, String)>, found| match best {
            Some(known) if known.0 > found.0 => Some(known),
            _ => Some(found),
        })
    }

    /// Where a copy section goes. Sections not named go to the default.
    fn destination(&self, name: &str) -> Option<String> {
        let of = |wanted: &str| {
            self.lines("destinationdirs").iter().find_map(|line| {
                let (key, place) = line.split_once('=').unwrap_or((line, ""));
                let place = place.split(';').next().unwrap_or("");
                let place: String = place.split_whitespace().collect();
                (key.trim().to_lowercase() == wanted).then(|| place.to_lowercase())
            })
        };
        of(name).or_else(|| of(DEFAULT_DESTINATION))
    }

    /// The sections an install section's CopyFiles lines name.
    fn copy_sections(&self, section: &str) -> Vec<String> {
        let lines = self.lines(section).iter().filter_map(|line| {
            let (key, names) = line.split_once('=')?;
            (key.trim().to_lowercase() == "copyfiles").then_some(names)
        });
        let names = lines.flat_map(|names| names.split(','));
        names.filter(|name| !name.trim().starts_with('@')).map(|name| name.trim().to_lowercase()).collect()
    }
}

fn windows_folder(destination: Option<String>) -> Option<&'static str> {
    let destination = destination?;
    WINDOWS_FOLDERS.iter().find(|(place, _)| *place == destination).map(|(_, folder)| *folder)
}

/// An INF line is the name to give a file, then the source's name if that differs.
fn guest_file(folder: &str, line: &str) -> GuestFile {
    let line = line.split(';').next().unwrap_or("");
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let source = fields.get(1).filter(|name| !name.is_empty()).unwrap_or(&fields[0]);
    GuestFile { folder: folder.to_owned(), source: (*source).to_owned(), name: fields[0].to_owned() }
}

/// What the card's section copies to System32 and SysWOW64, from the INF that lists the card best.
/// Desktop cards are in the first, `nv_dispi.inf`, and a laptop's card only in its maker's, by the
/// subsystem the laptop gives it. The first INF wins a tie. `None` if no INF lists the card.
pub fn guest_files(texts: &[String], device: u32, subsystem: u32) -> Option<Vec<GuestFile>> {
    let mut best: Option<(u8, Sections, String)> = None;
    for text in texts {
        let sections = Sections::read(text);
        if let Some((fit, section)) = sections.card_section(device, subsystem)
            && best.as_ref().is_none_or(|known| fit > known.0)
        {
            best = Some((fit, sections, section));
        }
    }
    let (_, sections, section) = best?;
    let mut files = Vec::new();
    for name in sections.copy_sections(&section) {
        // Anything else is the driver's own folder, or a place with no use in a guest.
        if let Some(folder) = windows_folder(sections.destination(&name)) {
            files.extend(sections.lines(&name).iter().map(|line| guest_file(folder, line)));
        }
    }
    Some(files)
}

/// A card's subsystem as an INF writes it, the subsystem's device in the high half. Zero if sysfs
/// doesn't say.
fn card_subsystem(device_folder: &Path) -> u32 {
    let read = |field: &str| {
        let text = sysfs_text(&device_folder.join(field));
        u32::from_str_radix(text.trim_start_matches("0x"), 16).ok()
    };
    match (read("subsystem_vendor"), read("subsystem_device")) {
        (Some(vendor), Some(device)) => device << 16 | vendor,
        _ => 0,
    }
}

/// The list the guest's script reads: folder, source and name, split by tabs.
pub fn file_list(files: &[GuestFile]) -> String {
    files.iter().map(|file| format!("{}\t{}\t{}\n", file.folder, file.source, file.name)).collect()
}

/// The list for this PC's driver and card.
pub fn guest_file_list(context: &Context) -> Result<String> {
    let setup = read_setup(context);
    let device = setup.get("card").and_then(|card| card.get("device")).and_then(|device| device.as_u64());
    let device = device.ok_or_else(|| Failure::new("Setup doesn't know the card yet."))? as u32;
    let pci = setup.get("card").and_then(|card| card.get("pci")).and_then(|pci| pci.as_str());
    let subsystem = pci.map_or(0, |pci| card_subsystem(&context.pci_devices.join(pci)));
    let files = guest_files(&read_infs(&driver_folder(context))?, device, subsystem);
    let files =
        files.ok_or_else(|| Failure(format!("The driver's INFs don't list this card ({device:04x}).")))?;
    Ok(file_list(&files))
}

#[cfg(test)]
pub mod stand_in {
    /// As much of an INF as the guest's files need.
    pub const FILES_INF: &str = r#"[DestinationDirs]
DefaultDestDir = 11
system_files = 11 ; System32
wow_files = 10, SysWOW64
own_files = 13

[Manufacturer]
%NVIDIA% = NVIDIA_Devices,NTamd64.10.0...17134,NTamd64.10.0...26200

[NVIDIA_Devices.NTamd64.10.0...17134]
%NVIDIA_DEV.2B85% = Section_Old, PCI\VEN_10DE&DEV_2B85
[NVIDIA_Devices.NTamd64.10.0...26200]
%NVIDIA_DEV.2B85% = Section_New, PCI\VEN_10DE&DEV_2B85

[Section_Old]
CopyFiles = old_files
[Section_New]
CopyFiles = system_files, wow_files
CopyFiles = own_files, @single.dll, default_files

[old_files]
old.dll
[system_files]
one.dll
two.dll,two_source.dll ; a comment
[wow_files]
one32.dll
[own_files]
stays.dll
[default_files]
plain.dll
"#;
}

#[cfg(test)]
mod tests {
    use super::stand_in::FILES_INF;
    use super::*;

    fn places(device: u32) -> Vec<(String, String, String)> {
        let files = guest_files(&[FILES_INF.to_owned()], device, 0).unwrap();
        files.into_iter().map(|file| (file.folder, file.source, file.name)).collect()
    }

    fn place(folder: &str, source: &str, name: &str) -> (String, String, String) {
        (folder.to_owned(), source.to_owned(), name.to_owned())
    }

    #[test]
    fn the_newest_windowss_section_decides() {
        assert_eq!(
            places(0x2B85),
            [
                place("System32", "one.dll", "one.dll"),
                place("System32", "two_source.dll", "two.dll"),
                place("SysWOW64", "one32.dll", "one32.dll"),
                place("System32", "plain.dll", "plain.dll"),
            ]
        );
    }

    #[test]
    fn a_card_the_inf_lacks_is_none() {
        assert_eq!(guest_files(&[FILES_INF.to_owned()], 0x2C02, 0), None);
    }

    const LAPTOP_INF: &str = r#"[DestinationDirs]
DefaultDestDir = 11
[NVIDIA_Devices.NTamd64.10.0...26200]
%NVIDIA_DEV.25A2.0156.1558% = Section_A, PCI\VEN_10DE&DEV_25A2&SUBSYS_01561558
%NVIDIA_DEV.25A2.3A58.17AA% = Section_B, PCI\VEN_10DE&DEV_25A2&SUBSYS_3A5817AA
[Section_A]
CopyFiles = a_files
[Section_B]
CopyFiles = b_files
[a_files]
a.dll
[b_files]
b.dll
"#;

    fn names(texts: &[String], device: u32, subsystem: u32) -> Vec<String> {
        guest_files(texts, device, subsystem).unwrap().into_iter().map(|file| file.name).collect()
    }

    #[test]
    fn a_laptops_card_is_found_in_its_makers_inf_by_subsystem() {
        let texts = [FILES_INF.to_owned(), LAPTOP_INF.to_owned()];
        assert_eq!(names(&texts, 0x25A2, 0x0156_1558), ["a.dll"]);
        assert_eq!(names(&texts, 0x25A2, 0x3A58_17AA), ["b.dll"]);
        assert_eq!(names(&texts, 0x25A2, 0x1111_2222), ["b.dll"], "another laptop's line will do");
        assert_eq!(names(&texts, 0x2B85, 0x3A58_17AA).len(), 4, "a desktop card stays in the first");
    }

    #[test]
    fn the_subsystem_is_read_as_an_inf_writes_it() {
        let scratch = crate::testing::Scratch::new();
        let folder = scratch.path("pci/0000:01:00.0");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("subsystem_vendor"), "0x17aa\n").unwrap();
        std::fs::write(folder.join("subsystem_device"), "0x3a58\n").unwrap();
        assert_eq!(card_subsystem(&folder), 0x3A58_17AA);
        assert_eq!(card_subsystem(&scratch.path("pci/none")), 0);
    }

    #[test]
    fn the_list_is_split_by_tabs() {
        let file = GuestFile { folder: "System32".into(), source: "a_src.dll".into(), name: "a.dll".into() };
        assert_eq!(file_list(&[file]), "System32\ta_src.dll\ta.dll\n");
    }
}
