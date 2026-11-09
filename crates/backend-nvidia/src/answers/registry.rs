//! The adapter's registry values, which apps look up to find their drivers.

use crate::inf::{Inf, InfValue};
use guesthouse_backend::guest_text::{utf16_list, utf16_zero};
use guesthouse_backend::{PciCard, Refusal, RegistryName, RegistryValue};
use guesthouse_messages::registry::{TYPE_NUMBER, TYPE_STRING, TYPE_STRING_LIST};
use std::collections::HashMap;

/// Where the guest finds the driver, which it translates to the host driver store. The files must
/// be there, and the guest setup puts them there.
pub const DRIVER_STORE: &str = "C:\\Windows\\System32\\DriverStore\\FileRepository\\nv_dispi.inf_amd64_gpupv";

/// What apps look up. DXCore reads the first to learn that an adapter does graphics, and the
/// Vulkan, OpenGL and OpenCL loaders find their drivers by the others.
const VALUES: [&str; 13] = [
    "DXCoreAttributes",
    "VulkanDriverName",
    "VulkanDriverNameWoW",
    "VulkanImplicitLayers",
    "VulkanImplicitLayersWow",
    "OpenGLDriverName",
    "OpenGLDriverNameWoW",
    "OpenGLVersion",
    "OpenGLVersionWoW",
    "OpenGLFlags",
    "OpenGLFlagsWoW",
    "OpenCLDriverName",
    "OpenCLDriverNameWow",
];

fn registry_form(value: InfValue) -> (u32, Vec<u8>) {
    match value {
        InfValue::Number(number) => (TYPE_NUMBER, number.to_le_bytes().to_vec()),
        InfValue::Text(text) => (TYPE_STRING, utf16_zero(&text)),
        InfValue::TextList(list) => (TYPE_STRING_LIST, utf16_list(&list)),
    }
}

/// The values, by lower-case name, as registry names ignore case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterRegistry {
    values: HashMap<String, (u32, Vec<u8>)>,
    kernel_driver: String,
}

impl AdapterRegistry {
    /// Reads the values the INF adds for this card. All of them are needed, so a missing one refuses.
    pub fn from_inf(inf: &Inf, card: &PciCard) -> Result<Self, Refusal> {
        let section = inf.card_section(card.vendor, card.device).ok_or_else(|| {
            Refusal(format!("The driver's INF does not list this card ({:04x}).", card.device))
        })?;
        let mut found = inf.adapter_values(&section, DRIVER_STORE);
        let mut values = HashMap::new();
        for name in VALUES {
            let value = found
                .remove(name)
                .ok_or_else(|| Refusal(format!("The driver's INF does not give {name} for this card.")))?;
            values.insert(name.to_lowercase(), registry_form(value));
        }
        let no_driver = || Refusal("The driver's INF names no kernel driver.".to_owned());
        let kernel_driver = inf.kernel_driver().ok_or_else(no_driver)?.to_owned();
        Ok(Self { values, kernel_driver })
    }

    pub fn lookup(&self, name: &RegistryName) -> Option<RegistryValue> {
        match name {
            RegistryName::DriverStorePath => {
                Some(RegistryValue { value_type: None, bytes: utf16_zero(DRIVER_STORE) })
            }
            RegistryName::DriverImagePath => {
                let path = format!("{DRIVER_STORE}\\{}", self.kernel_driver);
                Some(RegistryValue { value_type: None, bytes: utf16_zero(&path) })
            }
            RegistryName::Value(name) => self.values.get(name).map(|(value_type, bytes)| RegistryValue {
                value_type: Some(*value_type),
                bytes: bytes.clone(),
            }),
        }
    }

    /// The raw bytes of a value.
    pub fn bytes(&self, name: &str) -> Option<&[u8]> {
        self.values.get(name).map(|(_, bytes)| bytes.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: PciCard = PciCard { vendor: 0x10DE, device: 0x1234, revision: 1 };

    fn inf_text() -> String {
        let mut text = String::from(
            "[NVIDIA_Devices.NTamd64.10.0]\n%A% = Section, PCI\\VEN_10DE&DEV_1234\n[section]\nAddReg = values\n[values]\n",
        );
        for name in VALUES {
            let line = match name {
                "VulkanImplicitLayers" => format!("HKR,,{name},%REG_MULTI_SZ%,\"%13%\\a\",\"b\"\n"),
                _ if name.starts_with("OpenGLV") || name.starts_with("OpenGLF") => {
                    format!("HKR,,{name},%REG_DWORD%,0x2\n")
                }
                _ => format!("HKR,,{name},%REG_SZ%,\"%13%\\file\"\n"),
            };
            text.push_str(&line);
        }
        text.push_str("[services]\nServiceBinary = %13%\\kernel.sys\n");
        text
    }

    fn registry() -> AdapterRegistry {
        AdapterRegistry::from_inf(&Inf::parse(inf_text().as_bytes()), &CARD).unwrap()
    }

    #[test]
    fn a_text_value_ends_in_a_zero_and_names_the_guests_folder() {
        let value = registry().lookup(&RegistryName::Value("vulkandrivername".into())).unwrap();
        assert_eq!(value.value_type, Some(TYPE_STRING));
        assert_eq!(value.bytes, utf16_zero(&format!("{DRIVER_STORE}\\file")));
    }

    #[test]
    fn a_list_ends_in_two_zeros() {
        let value = registry().lookup(&RegistryName::Value("vulkanimplicitlayers".into())).unwrap();
        assert_eq!(value.value_type, Some(TYPE_STRING_LIST));
        assert_eq!(value.bytes[value.bytes.len() - 4..], [0; 4]);
    }

    #[test]
    fn a_missing_value_refuses_and_says_which() {
        let text = inf_text().replace("OpenCLDriverNameWow", "Other");
        let error = AdapterRegistry::from_inf(&Inf::parse(text.as_bytes()), &CARD).unwrap_err();
        assert!(error.0.contains("OpenCLDriverNameWow"));
    }

    #[test]
    fn a_card_not_in_the_inf_refuses() {
        let card = PciCard { device: 0x9999, ..CARD };
        assert!(AdapterRegistry::from_inf(&Inf::parse(inf_text().as_bytes()), &card).is_err());
    }

    #[test]
    fn the_paths_have_no_type_to_match() {
        let value = registry().lookup(&RegistryName::DriverImagePath).unwrap();
        assert_eq!(value.value_type, None);
        assert_eq!(value.bytes, utf16_zero(&format!("{DRIVER_STORE}\\kernel.sys")));
    }
}
