//! Reading the guest driver's INF file, which setup unpacked from the user's own installer.

use std::collections::HashMap;

/// The file's name in the driver's folder.
pub const INF_NAME: &str = "nv_dispi.inf";
const SERVICE_BINARY: &str = "ServiceBinary";
const KERNEL_DRIVER_ENDING: &str = ".sys";

/// An INF's sections in file order. Names are lower case, as INF names ignore case.
#[derive(Debug, Default)]
pub struct Inf {
    sections: Vec<(String, Vec<String>)>,
    by_name: HashMap<String, usize>,
}

/// A value for the adapter's registry key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfValue {
    Number(u32),
    Text(String),
    TextList(Vec<String>),
}

fn section_header(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let (name, after) = rest.split_once(']')?;
    let after = after.trim_start();
    (!name.is_empty() && (after.is_empty() || after.starts_with(';'))).then_some(name)
}

/// The text of a quoted string list.
fn quoted(text: &str) -> Vec<String> {
    text.split('"').skip(1).step_by(2).map(str::to_owned).collect()
}

fn parse_number(text: &str) -> Option<u32> {
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

impl Inf {
    /// Reads an INF's bytes. These files are Latin-1.
    pub fn parse(bytes: &[u8]) -> Self {
        let text: String = bytes.iter().map(|&byte| char::from(byte)).collect();
        let mut inf = Self::default();
        let mut current = None;
        for line in text.lines().map(str::trim) {
            if let Some(name) = section_header(line) {
                current = Some(inf.section_index(name.to_lowercase()));
            } else if let Some(index) = current.filter(|_| !line.is_empty() && !line.starts_with(';')) {
                inf.sections[index].1.push(line.to_owned());
            }
        }
        inf
    }

    /// The file name of the kernel driver, from the service the INF installs for it.
    pub fn kernel_driver(&self) -> Option<&str> {
        let lines = self.sections.iter().flat_map(|(_, lines)| lines);
        let binaries = lines
            .filter_map(|line| line.split_once('='))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case(SERVICE_BINARY));
        binaries
            .filter_map(|(_, path)| path.trim().rsplit('\\').next())
            .find(|file| file.to_lowercase().ends_with(KERNEL_DRIVER_ENDING))
    }

    fn section_index(&mut self, name: String) -> usize {
        if let Some(&index) = self.by_name.get(&name) {
            return index;
        }
        self.sections.push((name.clone(), Vec::new()));
        self.by_name.insert(name, self.sections.len() - 1);
        self.sections.len() - 1
    }

    fn section(&self, name: &str) -> &[String] {
        self.by_name.get(name).map_or(&[], |&index| &self.sections[index].1)
    }

    /// The card's install section, from the device lists. The last list is for the newest Windows.
    pub fn card_section(&self, vendor: u16, device: u16) -> Option<String> {
        let wanted = format!("pci\\ven_{vendor:04x}&dev_{device:04x}");
        let mut found = None;
        for (name, lines) in &self.sections {
            if !name.starts_with("nvidia_devices.") {
                continue;
            }
            for line in lines {
                let value = line.split_once('=').map_or("", |(_, value)| value);
                let mut parts = value.split(',').map(str::trim);
                let (Some(section), Some(id)) = (parts.next(), parts.next()) else { continue };
                if id.to_lowercase().starts_with(&wanted) {
                    found = Some(section.to_lowercase());
                }
            }
        }
        found
    }

    /// The values under the adapter's key that a card's section adds, by name. `%13%` stands for
    /// the driver's folder.
    pub fn adapter_values(&self, section: &str, driver_folder: &str) -> HashMap<String, InfValue> {
        let mut found = HashMap::new();
        for line in self.section(section) {
            let Some((key, lists)) = line.split_once('=') else { continue };
            if !key.trim().eq_ignore_ascii_case("addreg") {
                continue;
            }
            for name in lists.split(',') {
                for entry in self.section(&name.trim().to_lowercase()) {
                    if let Some((name, value)) = parse_value(entry, driver_folder) {
                        found.insert(name, value);
                    }
                }
            }
        }
        found
    }
}

fn parse_value(entry: &str, driver_folder: &str) -> Option<(String, InfValue)> {
    let rest = entry.strip_prefix("HKR,,")?;
    let (name, rest) = rest.split_once(',')?;
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let expand = |text: String| text.replace("%13%", driver_folder);
    if let Some(number) = rest.strip_prefix("%REG_DWORD%,") {
        Some((name.to_owned(), InfValue::Number(parse_number(number)?)))
    } else if let Some(text) = rest.strip_prefix("%REG_SZ%,") {
        let first = quoted(text).into_iter().next()?;
        Some((name.to_owned(), InfValue::Text(expand(first))))
    } else if let Some(text) = rest.strip_prefix("%REG_MULTI_SZ%,") {
        let list = quoted(text).into_iter().map(expand).collect();
        Some((name.to_owned(), InfValue::TextList(list)))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VENDOR: u16 = 0x10DE;
    const TEXT: &str = "[Version]\n\
        DriverVer = 01/02/2026,1.2.3.4\n\
        ; a comment\n\
        [Nvidia_Devices.NTamd64.10.0]\n\
        %A% = Section_A, PCI\\VEN_10DE&DEV_1234&SUBSYS_0\n\
        [NVIDIA_Devices.NTamd64.10.0...1]\n\
        %A% = Section_B, PCI\\VEN_10DE&DEV_1234\n\
        [section_b]\n\
        AddReg = list_one, List_Two\n\
        [list_one]\n\
        HKR,,Number,%REG_DWORD%,0x10\n\
        HKR,,Plain,%REG_DWORD%,10\n\
        HKR,,Text,%REG_SZ%,\"%13%\\file.dll\"\n\
        HKR,,Other,%REG_NONE%,\"x\"\n\
        [list_two]\n\
        HKR,,Many,%REG_MULTI_SZ%,\"a\",\"%13%\\b\"\n\
        [services]\n\
        ServiceBinary = %13%\\Helper\\helper.exe\n\
        ServiceBinary = %13%\\kernel.sys\n";

    fn inf() -> Inf {
        Inf::parse(TEXT.as_bytes())
    }

    #[test]
    fn the_last_device_list_wins() {
        assert_eq!(inf().card_section(VENDOR, 0x1234), Some("section_b".into()));
    }

    #[test]
    fn a_card_not_listed_has_no_section() {
        assert_eq!(inf().card_section(VENDOR, 0x9999), None);
    }

    #[test]
    fn values_come_from_every_list_of_the_section() {
        let values = inf().adapter_values("section_b", "C:\\Folder");
        assert_eq!(values["Number"], InfValue::Number(16));
        assert_eq!(values["Plain"], InfValue::Number(10));
        assert_eq!(values["Text"], InfValue::Text("C:\\Folder\\file.dll".into()));
        assert_eq!(values["Many"], InfValue::TextList(vec!["a".into(), "C:\\Folder\\b".into()]));
        assert!(!values.contains_key("Other"));
    }

    #[test]
    fn the_kernel_driver_is_the_service_that_is_a_driver() {
        assert_eq!(inf().kernel_driver(), Some("kernel.sys"));
        assert_eq!(Inf::parse(b"[services]\nServiceBinary = a.exe\n").kernel_driver(), None);
    }

    #[test]
    fn a_header_may_have_a_comment_after_it() {
        assert_eq!(section_header("[name] ; why"), Some("name"));
        assert_eq!(section_header("[name] other"), None);
    }
}
