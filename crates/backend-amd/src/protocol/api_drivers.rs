//! Where the guest's Vulkan and OpenGL loaders find AMD's drivers.
//!
//! Neither loader reads the guest's own registry for a paravirtual adapter. Vulkan's asks the
//! adapter for registry values by name, and OpenGL's asks with adapter query 2. The values are the
//! ones AMD's INF adds for the adapter, pointing into the guest's copy of the driver store, where the
//! Direct3D drivers are.

use super::guest_driver::DRIVER_STORE;
use guesthouse_backend::guest_text::{utf16_list, utf16_zero};
use guesthouse_backend::{RegistryValue, VendorQuery};
use guesthouse_messages::registry::{TYPE_NUMBER, TYPE_STRING, TYPE_STRING_LIST};

/// The adapter query OpenGL's loader asks for its driver with.
pub const QUERY_OPENGL: u32 = 2;
/// The answer's path field: MAX_PATH UTF-16 units. The version and flags follow it.
const OPENGL_NAME_BYTES: usize = 520;
const OPENGL_DRIVER: &str = "atig6pxx.dll";
const OPENGL_DRIVER_32_BIT: &str = "atiglpxx.dll";
/// AMD's INF sets no OpenGLVersion or OpenGLFlags. These are what NVIDIA's INF sets, which Windows'
/// OpenGL loader takes for an installable driver of the current kind.
const OPENGL_VERSION: u32 = 0x1000;
const OPENGL_FLAGS: u32 = 3;

/// A value as the INF writes it, with file names in the driver store.
enum Form {
    File(&'static str),
    Files(&'static [&'static str]),
    Number(u32),
}

/// The values, by the names the INF gives them. Registry names ignore case.
const VALUES: [(&str, Form); 9] = [
    ("VulkanDriverName", Form::File("amd-vulkan64.json")),
    ("VulkanDriverNameWow", Form::File("amd-vulkan32.json")),
    ("VulkanImplicitLayers", Form::File("amd-vulkan64.json")),
    ("VulkanImplicitLayersWow", Form::File("amd-vulkan32.json")),
    ("OpenGLDriverName", Form::Files(&[OPENGL_DRIVER])),
    ("OpenGLDriverNameWoW", Form::Files(&[OPENGL_DRIVER_32_BIT])),
    // The OpenGL driver above is a loader, and loads the driver itself from these.
    ("OpenGLVendorName", Form::Files(&["atio6axx.dll", "atio6axx.dll"])),
    ("OpenGLVendorNameWoW", Form::Files(&["atioglxx.dll", "atioglxx.dll"])),
    ("OpenGLDynamicPX", Form::Number(1)),
];

fn in_store(file: &str) -> String {
    format!("{DRIVER_STORE}\\{file}")
}

/// The value named `name`, if it is one of these.
pub fn registry_value(name: &str) -> Option<RegistryValue> {
    let (_, form) = VALUES.iter().find(|(known, _)| known.eq_ignore_ascii_case(name))?;
    let (value_type, bytes) = match form {
        Form::File(file) => (TYPE_STRING, utf16_zero(&in_store(file))),
        Form::Files(files) => {
            let paths: Vec<String> = files.iter().map(|file| in_store(file)).collect();
            (TYPE_STRING_LIST, utf16_list(&paths))
        }
        Form::Number(number) => (TYPE_NUMBER, number.to_le_bytes().to_vec()),
    };
    Some(RegistryValue { value_type: Some(value_type), bytes })
}

/// Answers OpenGL's query: the driver's path, then its version and flags.
pub fn write_opengl_info(query: &VendorQuery<'_>, out: &mut Vec<u8>) {
    let driver = if query.process_is_32_bit { OPENGL_DRIVER_32_BIT } else { OPENGL_DRIVER };
    let start = out.len();
    let mut path = utf16_zero(&in_store(driver));
    path.truncate(OPENGL_NAME_BYTES);
    out.extend_from_slice(&path);
    out.resize(start + OPENGL_NAME_BYTES, 0);
    out.extend_from_slice(&OPENGL_VERSION.to_le_bytes());
    out.extend_from_slice(&OPENGL_FLAGS.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: &[u8]) -> String {
        let units = bytes.as_chunks::<2>().0.iter().map(|unit| u16::from_le_bytes(*unit));
        String::from_utf16_lossy(&units.take_while(|&unit| unit != 0).collect::<Vec<u16>>())
    }

    #[test]
    fn vulkan_s_driver_is_found_by_any_case_of_its_name() {
        let value = registry_value("vulkandrivername").unwrap();
        assert_eq!(value.value_type, Some(TYPE_STRING));
        assert_eq!(text(&value.bytes), in_store("amd-vulkan64.json"));
    }

    #[test]
    fn opengl_s_vendor_driver_is_a_list_of_two() {
        let value = registry_value("OpenGLVendorName").unwrap();
        assert_eq!(value.value_type, Some(TYPE_STRING_LIST));
        assert_eq!(value.bytes, utf16_list(&[in_store("atio6axx.dll"), in_store("atio6axx.dll")]));
    }

    #[test]
    fn a_name_the_inf_does_not_give_has_no_value() {
        assert!(registry_value("OpenCLDriverName").is_none());
    }

    fn opengl_answer(process_is_32_bit: bool) -> Vec<u8> {
        let query = VendorQuery { kind: QUERY_OPENGL, size: 528, data: &[], process_is_32_bit };
        let mut out = Vec::new();
        write_opengl_info(&query, &mut out);
        out
    }

    #[test]
    fn opengl_s_answer_is_the_path_then_the_version_and_flags() {
        let out = opengl_answer(false);
        assert_eq!(out.len(), OPENGL_NAME_BYTES + 8);
        assert_eq!(text(&out), in_store(OPENGL_DRIVER));
        assert_eq!(out[OPENGL_NAME_BYTES..], [0x00, 0x10, 0, 0, 3, 0, 0, 0]);
    }

    #[test]
    fn a_32_bit_process_gets_the_32_bit_opengl_driver() {
        assert_eq!(text(&opengl_answer(true)), in_store(OPENGL_DRIVER_32_BIT));
    }
}
