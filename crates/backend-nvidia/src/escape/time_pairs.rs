//! Pairs of CPU and GPU time, which a guest's OpenGL driver asks for many times a second.
//!
//! The real control takes the RM's locks, so it queues behind whatever else the card is doing,
//! including the host's desktop. Most calls take 100 µs, and under load some take over 10 ms, all
//! of it on the one thread that serves the guest. The GPU's timer can be read from a mapped page
//! instead, which gives the same numbers with no lock. Only the plain OS clock is answered this way.
//! Anything else goes on to the RM, which knows what to do with it.
//!
//! The layout is the public `NV2080_CTRL_TIMER_GET_GPU_CPU_TIME_CORRELATION_INFO_PARAMS`.

use crate::rm::Rm;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const CONTROL: u32 = 0x2080_0406;

const CLOCK_ID: usize = 0;
const SAMPLE_COUNT: usize = 1;
const SAMPLES: usize = 8;
const SAMPLE_SIZE: usize = 16;
const GPU_TIME_IN_SAMPLE: usize = 8;
/// The size of the list in the structure, and so the most a guest can ask for.
const MOST_SAMPLES: usize = 16;
/// The clock the RM calls OSTIME: microseconds since 1970 on the host.
const CLOCK_OS_TIME: u8 = 1;

fn os_time_micros() -> Option<u64> {
    let since_1970 = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    u64::try_from(since_1970.as_micros()).ok()
}

/// Fills in the pairs the guest asked for. False if the real control has to answer: then nothing
/// that matters has been written, as the RM writes every sample itself.
pub(super) fn answer<R: Rm>(rm: &R, params: &mut [u8]) -> bool {
    let (Some(&clock), Some(&count)) = (params.get(CLOCK_ID), params.get(SAMPLE_COUNT)) else {
        return false;
    };
    let count = usize::from(count);
    if clock != CLOCK_OS_TIME || count == 0 || count > MOST_SAMPLES {
        return false;
    }
    let Some(samples) = params.get_mut(SAMPLES..SAMPLES + count * SAMPLE_SIZE) else { return false };
    for sample in samples.as_chunks_mut::<SAMPLE_SIZE>().0 {
        // CPU first, so the pair's two readings sit as close together as they can.
        let (Some(cpu), Some(gpu)) = (os_time_micros(), rm.read_gpu_timer()) else { return false };
        let (cpu_field, gpu_field) = sample.split_at_mut(GPU_TIME_IN_SAMPLE);
        cpu_field.copy_from_slice(&cpu.to_le_bytes());
        gpu_field.copy_from_slice(&gpu.to_le_bytes());
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rm::fake::FakeRm;

    const PARAMS_SIZE: usize = SAMPLES + MOST_SAMPLES * SAMPLE_SIZE;
    const TIMER: u64 = 0x18E5_0000_0000_0000;

    fn asking(clock: u8, count: u8) -> [u8; PARAMS_SIZE] {
        let mut params = [0; PARAMS_SIZE];
        params[CLOCK_ID] = clock;
        params[SAMPLE_COUNT] = count;
        params
    }

    fn timer() -> FakeRm {
        let mut rm = FakeRm::new();
        rm.gpu_timer = Some(TIMER);
        rm
    }

    fn field(params: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(params[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn each_sample_asked_for_gets_both_times() {
        let mut params = asking(CLOCK_OS_TIME, 5);
        assert!(answer(&timer(), &mut params));
        for sample in 0..5 {
            let at = SAMPLES + sample * SAMPLE_SIZE;
            assert_ne!(field(&params, at), 0);
            assert_eq!(field(&params, at + GPU_TIME_IN_SAMPLE), TIMER);
        }
        assert_eq!(field(&params, SAMPLES + 5 * SAMPLE_SIZE), 0, "no more than it asked for");
    }

    #[test]
    fn the_cpu_time_is_the_hosts_in_microseconds() {
        let mut params = asking(CLOCK_OS_TIME, 1);
        let before = os_time_micros().unwrap();
        assert!(answer(&timer(), &mut params));
        let said = field(&params, SAMPLES);
        assert!(said >= before && said <= os_time_micros().unwrap());
    }

    #[test]
    fn other_clocks_are_left_to_the_rm() {
        for clock in [0, 2, 3, 4, 255] {
            assert!(!answer(&timer(), &mut asking(clock, 1)), "{clock}");
        }
    }

    #[test]
    fn a_count_the_structure_cannot_hold_is_left_to_the_rm() {
        for count in [0, 17, 255] {
            assert!(!answer(&timer(), &mut asking(CLOCK_OS_TIME, count)), "{count}");
        }
        assert!(answer(&timer(), &mut asking(CLOCK_OS_TIME, MOST_SAMPLES as u8)));
    }

    #[test]
    fn no_timer_is_left_to_the_rm() {
        assert!(!answer(&FakeRm::new(), &mut asking(CLOCK_OS_TIME, 1)));
    }

    #[test]
    fn short_parameters_are_left_to_the_rm() {
        assert!(!answer(&timer(), &mut [CLOCK_OS_TIME]));
        assert!(!answer(&timer(), &mut [CLOCK_OS_TIME, 2, 0, 0, 0, 0, 0, 0, 0, 0]));
    }
}
