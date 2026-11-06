//! The vendor's management library, through its documented C API.
//!
//! It is loaded when the daemon starts and never linked, so a machine without it only goes without
//! readings. Each getter is looked up alone, so an older library loses that reading and no more.

use super::sampler::{FastReadings, SensorSource, SlowReadings};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::mem;
use std::ptr;

pub(super) const LIBRARY_NAME: &CStr = c"libnvidia-ml.so.1";

const SUCCESS: c_int = 0;
/// From the API's enumerations.
const SENSOR_GPU: c_int = 0;
const CLOCK_GRAPHICS: c_int = 0;
const CLOCK_MEMORY: c_int = 2;

/// The library's name for a card.
type Device = *mut c_void;

/// The API's memory structure.
#[repr(C)]
#[derive(Default)]
struct MemoryInfo {
    total: u64,
    free: u64,
    used: u64,
}

type Init = unsafe extern "C" fn() -> c_int;
type HandleByUuid = unsafe extern "C" fn(*const c_char, *mut Device) -> c_int;
type ReadWord = unsafe extern "C" fn(Device, *mut c_uint) -> c_int;
type ReadWordOf = unsafe extern "C" fn(Device, c_int, *mut c_uint) -> c_int;
type ReadMemory = unsafe extern "C" fn(Device, *mut MemoryInfo) -> c_int;

/// The text form of a card's identity, as the library takes it.
pub(super) fn uuid_text(bytes: &[u8; 16]) -> String {
    let hex = |range: std::ops::Range<usize>| -> String {
        bytes[range].iter().map(|byte| format!("{byte:02x}")).collect()
    };
    format!("GPU-{}-{}-{}-{}-{}", hex(0..4), hex(4..6), hex(6..8), hex(8..10), hex(10..16))
}

/// The library, set up for one card.
pub(super) struct Library {
    device: Device,
    temperature: Option<ReadWordOf>,
    clock: Option<ReadWordOf>,
    most_clock: Option<ReadWordOf>,
    power: Option<ReadWord>,
    power_limit: Option<ReadWord>,
    fan: Option<ReadWord>,
    memory: Option<ReadMemory>,
}

/// Looks a function up by name.
///
/// # Safety
///
/// `F` must be a function pointer type matching the API's signature for `name`.
unsafe fn symbol<F: Copy>(library: *mut c_void, name: &CStr) -> Option<F> {
    // SAFETY: the library handle is open, and the name ends in a zero.
    let address = unsafe { libc::dlsym(library, name.as_ptr()) };
    // SAFETY: a function pointer and a data pointer are the same size here, and the caller vouches
    // for the signature.
    (!address.is_null()).then(|| unsafe { mem::transmute_copy(&address) })
}

impl Library {
    /// Loads the library and finds this card. `None` if any of that can't be done.
    pub(super) fn open(name: &CStr, card_uuid: &str) -> Option<Self> {
        // SAFETY: loading a library by name. Nothing else in this process depends on it.
        let library = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if library.is_null() {
            return None;
        }
        // SAFETY: each type below is that function's documented signature.
        unsafe {
            let init: Init = symbol(library, c"nvmlInit_v2")?;
            let by_uuid: HandleByUuid = symbol(library, c"nvmlDeviceGetHandleByUUID")?;
            if init() != SUCCESS {
                return None;
            }
            let uuid = CString::new(card_uuid).ok()?;
            let mut device = ptr::null_mut();
            if by_uuid(uuid.as_ptr(), &mut device) != SUCCESS {
                return None;
            }
            Some(Self {
                device,
                temperature: symbol(library, c"nvmlDeviceGetTemperature"),
                clock: symbol(library, c"nvmlDeviceGetClockInfo"),
                most_clock: symbol(library, c"nvmlDeviceGetMaxClockInfo"),
                power: symbol(library, c"nvmlDeviceGetPowerUsage"),
                power_limit: symbol(library, c"nvmlDeviceGetEnforcedPowerLimit"),
                fan: symbol(library, c"nvmlDeviceGetFanSpeed"),
                memory: symbol(library, c"nvmlDeviceGetMemoryInfo"),
            })
        }
    }

    fn word(&self, getter: Option<ReadWord>) -> Option<u32> {
        let mut value = 0;
        // SAFETY: the signature was vouched for at lookup, and `value` outlives the call.
        (unsafe { getter?(self.device, &mut value) } == SUCCESS).then_some(value)
    }

    fn word_of(&self, getter: Option<ReadWordOf>, which: c_int) -> Option<u32> {
        let mut value = 0;
        // SAFETY: as above.
        (unsafe { getter?(self.device, which, &mut value) } == SUCCESS).then_some(value)
    }

    fn memory(&self) -> Option<MemoryInfo> {
        let mut info = MemoryInfo::default();
        // SAFETY: as above.
        (unsafe { self.memory?(self.device, &mut info) } == SUCCESS).then_some(info)
    }
}

impl SensorSource for Library {
    fn read_fast(&mut self) -> FastReadings {
        FastReadings {
            temperature_celsius: self.word_of(self.temperature, SENSOR_GPU),
            power_milliwatts: self.word(self.power),
            graphics_megahertz: self.word_of(self.clock, CLOCK_GRAPHICS),
            memory_megahertz: self.word_of(self.clock, CLOCK_MEMORY),
        }
    }

    fn read_slow(&mut self) -> SlowReadings {
        let memory = self.memory();
        SlowReadings {
            fan_percent: self.word(self.fan),
            memory_used_bytes: memory.as_ref().map(|memory| memory.used),
            memory_total_bytes: memory.as_ref().map(|memory| memory.total),
            power_limit_milliwatts: self.word(self.power_limit),
            graphics_most_megahertz: self.word_of(self.most_clock, CLOCK_GRAPHICS),
            memory_most_megahertz: self.word_of(self.most_clock, CLOCK_MEMORY),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_is_simply_no_library() {
        assert!(Library::open(c"/nonexistent/no-such-library.so", "GPU-0").is_none());
    }

    #[test]
    fn an_identity_reads_in_the_usual_groups() {
        let bytes: [u8; 16] = std::array::from_fn(|index| index as u8);
        assert_eq!(uuid_text(&bytes), "GPU-00010203-0405-0607-0809-0a0b0c0d0e0f");
    }
}
