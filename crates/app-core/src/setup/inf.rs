//! Reading a driver's INF: its version.

use crate::failure::{Failure, Result};

/// INF files are Latin-1.
pub fn latin1(data: &[u8]) -> String {
    data.iter().map(|&byte| char::from(byte)).collect()
}

/// The version in the INF, as 32.0.16.1742.
pub fn inf_driver_version(text: &str) -> Result<String> {
    let found = text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "DriverVer").then_some(value)?.split_once(',').map(|(_, version)| version.trim())
    });
    let version: String =
        found.unwrap_or("").chars().take_while(|letter| letter.is_ascii_digit() || *letter == '.').collect();
    if version.matches('.').count() != 3 {
        return Err(Failure::new("The driver's INF has no version."));
    }
    Ok(version)
}

/// The name NVIDIA sells a driver by, which is the last five digits of its version.
pub fn retail_version(version: &str) -> String {
    let digits: String = version.split('.').skip(2).collect();
    let digits = &digits[digits.len().saturating_sub(5)..];
    let (major, minor) = digits.split_at(digits.len().min(3));
    format!("{major}.{minor}")
}

#[cfg(test)]
pub mod stand_in {
    /// As much of an INF as this reads.
    pub const FAKE_INF: &str = "[Version]\nDriverVer = 01/01/2026, 32.0.16.1742\n";
}

#[cfg(test)]
mod tests {
    use super::stand_in::FAKE_INF;
    use super::*;

    #[test]
    fn versions_are_read_and_named() {
        assert_eq!(retail_version("32.0.16.1742"), "617.42");
        assert_eq!(retail_version("32.0.15.9186"), "591.86");
        assert_eq!(inf_driver_version(FAKE_INF).unwrap(), "32.0.16.1742");
        assert!(inf_driver_version("[Version]\n").unwrap_err().0.contains("no version"));
    }
}
