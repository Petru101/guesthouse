//! What the performance overlay says. Every line is a real reading or says it has none. Frame figures
//! come from frames counted, so the frame time is the average over the last second: a single frame's
//! time would need work on each frame.

use crate::gpu_sample::Sample;
use std::time::Duration;

const UNKNOWN: &str = "not available";
/// The sampler reads each second, so a reading this old means it has stopped.
pub const STALE_SECONDS: u64 = 3;
const MIB_PER_GIB: f64 = 1024.0;

/// Frames shown over the last seconds. A still screen sends none, and has no frame time to give.
pub fn frame_lines(frames: u32, seconds: f64) -> Vec<String> {
    if seconds <= 0.0 {
        return vec![format!("Frames a second: {UNKNOWN}"), format!("Frame time: {UNKNOWN}")];
    }
    if frames == 0 {
        return vec!["Frames a second: 0".to_owned(), "Frame time: no new frames".to_owned()];
    }
    let rate = f64::from(frames) / seconds;
    vec![
        format!("Frames a second: {rate:.0}"),
        format!("Frame time: {:.1} ms", seconds / f64::from(frames) * 1000.0),
    ]
}

fn reading<T>(value: Option<T>, text: impl Fn(T) -> String) -> String {
    value.map_or_else(|| UNKNOWN.to_owned(), text)
}

fn memory_line(used: Option<u64>, total: Option<u64>) -> String {
    match (used, total) {
        (Some(used), Some(total)) => {
            let gib = |mib: u64| mib as f64 / MIB_PER_GIB;
            format!("Video memory: {:.1} of {:.1} GiB", gib(used), gib(total))
        }
        _ => format!("Video memory: {UNKNOWN}"),
    }
}

/// What the host's card says. Old or missing readings aren't shown as current.
pub fn gpu_lines(sample: Option<&Sample>, now: Duration) -> Vec<String> {
    let current =
        sample.filter(|sample| now.saturating_sub(sample.taken).as_secs_f64() <= STALE_SECONDS as f64);
    let Some(sample) = current else { return vec![format!("Host GPU: {UNKNOWN}")] };
    vec![
        format!("Host GPU load: {}", reading(sample.load_percent, |value| format!("{value}%"))),
        format!("Host GPU power: {}", reading(sample.power_watts, |value| format!("{value:.0} W"))),
        format!("Host GPU clock: {}", reading(sample.clock_mhz, |value| format!("{value} MHz"))),
        memory_line(sample.memory_used_mib, sample.memory_total_mib),
    ]
}

/// What the picture on screen is. An HDR picture comes with how bright the guest's desktop wants its
/// white, and an SDR one says so, so a missing line can't be read as either.
pub fn picture_line(sdr_white_nits: Option<u32>) -> String {
    match sdr_white_nits {
        Some(nits) => format!("HDR on, desktop white {nits} nits"),
        None => "SDR picture".to_owned(),
    }
}

pub fn text(
    frames: u32,
    seconds: f64,
    sample: Option<&Sample>,
    now: Duration,
    sdr_white_nits: Option<u32>,
) -> String {
    let mut lines = frame_lines(frames, seconds);
    lines.extend(gpu_lines(sample, now));
    lines.push(picture_line(sdr_white_nits));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    fn whole() -> Sample {
        Sample {
            taken: at(99),
            load_percent: Some(42),
            power_watts: Some(129.6),
            clock_mhz: Some(2400),
            memory_used_mib: Some(5120),
            memory_total_mib: Some(32768),
        }
    }

    #[test]
    fn frames_are_counted_not_guessed() {
        assert_eq!(frame_lines(144, 1.0), ["Frames a second: 144", "Frame time: 6.9 ms"]);
        assert_eq!(frame_lines(0, 1.0), ["Frames a second: 0", "Frame time: no new frames"]);
        assert_eq!(frame_lines(0, 0.0), ["Frames a second: not available", "Frame time: not available"]);
    }

    #[test]
    fn no_reading_says_so_and_guesses_nothing() {
        assert_eq!(gpu_lines(None, at(100)), ["Host GPU: not available"]);
    }

    #[test]
    fn a_whole_reading_is_shown_in_full() {
        assert_eq!(
            gpu_lines(Some(&whole()), at(100)),
            [
                "Host GPU load: 42%",
                "Host GPU power: 130 W",
                "Host GPU clock: 2400 MHz",
                "Video memory: 5.0 of 32.0 GiB"
            ]
        );
    }

    #[test]
    fn a_real_zero_is_not_a_missing_reading() {
        let sample = Sample { taken: at(99), load_percent: Some(0), ..Sample::default() };
        assert_eq!(
            gpu_lines(Some(&sample), at(100)),
            [
                "Host GPU load: 0%",
                "Host GPU power: not available",
                "Host GPU clock: not available",
                "Video memory: not available"
            ]
        );
    }

    #[test]
    fn a_reading_that_stopped_coming_does_not_pass_for_now() {
        assert_eq!(gpu_lines(Some(&whole()), at(99 + STALE_SECONDS + 1)), ["Host GPU: not available"]);
    }

    #[test]
    fn the_picture_is_named_either_way() {
        assert_eq!(picture_line(Some(240)), "HDR on, desktop white 240 nits");
        assert_eq!(picture_line(None), "SDR picture");
    }

    #[test]
    fn the_text_is_the_lines_together() {
        assert_eq!(text(60, 1.0, None, at(0), None).matches('\n').count(), 3);
    }
}
