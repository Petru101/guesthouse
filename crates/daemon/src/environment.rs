//! Settings that arrive in environment variables, shared with the scripts that start a VM.

const GIB_SUFFIX: char = 'G';
const DEFAULT_GUEST_RAM_GIB: u64 = 16;

/// Reads a size like `16G`. Anything else is `None`.
fn gibibytes(text: &str) -> Option<u64> {
    let digits = text.strip_suffix(GIB_SUFFIX)?;
    let all_digits = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit());
    all_digits.then(|| digits.parse::<u64>().ok()).flatten()?.checked_mul(1 << 30)
}

/// The guest's RAM, from `GUESTHOUSE_RAM`.
pub fn guest_ram(value: Option<&str>) -> u64 {
    value.and_then(gibibytes).unwrap_or(DEFAULT_GUEST_RAM_GIB << 30)
}

/// The most video memory the guest may have, from `GUESTHOUSE_VRAM`. Zero and unset mean no cap.
pub fn video_cap(value: Option<&str>) -> Option<u64> {
    value.and_then(gibibytes).filter(|&bytes| bytes != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_defaults_when_unset_or_unreadable() {
        assert_eq!(guest_ram(None), 16 << 30);
        assert_eq!(guest_ram(Some("lots")), 16 << 30);
        assert_eq!(guest_ram(Some("32G")), 32 << 30);
    }

    #[test]
    fn a_cap_needs_a_size() {
        assert_eq!(video_cap(None), None);
        assert_eq!(video_cap(Some("0G")), None);
        assert_eq!(video_cap(Some("G")), None);
        assert_eq!(video_cap(Some("8G")), Some(8 << 30));
    }
}
