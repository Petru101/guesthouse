//! What qemu-img says about a disk.

use crate::failure::{Failure, Result};
use serde_json::Value;
use std::process::Command;

const QEMU_IMG_INFO_SECONDS: &str = "30";

/// What qemu-img says about a disk. `-U` reads it even while the VM has it open.
pub fn disk_info(path: &str) -> Result<Value> {
    let result = Command::new("timeout")
        .args([QEMU_IMG_INFO_SECONDS, "qemu-img", "info", "-U", "--output=json", path])
        .output()?;
    if !result.status.success() {
        let said = String::from_utf8_lossy(&result.stderr);
        return Err(Failure(format!("Could not read the disk: {}", said.trim())));
    }
    serde_json::from_slice(&result.stdout)
        .map_err(|error| Failure(format!("Could not read the disk: {error}")))
}

/// The disk's size as the guest sees it.
pub fn disk_size_gib(path: &str) -> Result<f64> {
    let size = disk_info(path)?["virtual-size"].as_f64().unwrap_or(0.0);
    Ok(size / crate::host::GIB as f64)
}
