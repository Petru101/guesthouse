//! What qemu-img says about a disk.

pub mod backing;

use crate::failure::{Failure, Result};
use serde_json::Value;
use std::ffi::OsStr;
use std::process::Command;

const QEMU_IMG_INFO_SECONDS: &str = "30";

/// What qemu-img says about a disk. `-U` reads it even while the VM has it open.
pub fn disk_info(path: &str) -> Result<Value> {
    info(path.as_ref(), None).map_err(|reason| Failure(format!("Could not read the disk: {reason}")))
}

/// Read as `format`, if one is given, and not as whatever the file looks like. Says why not if it
/// can't be read.
fn info(path: &OsStr, format: Option<&str>) -> std::result::Result<Value, String> {
    let mut command = Command::new("timeout");
    command.args([QEMU_IMG_INFO_SECONDS, "qemu-img", "info", "-U", "--output=json"]);
    if let Some(format) = format {
        command.args(["-f", format]);
    }
    let result = command.arg(path).output().map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
    }
    serde_json::from_slice(&result.stdout).map_err(|error| error.to_string())
}

/// The disk's size as the guest sees it.
pub fn disk_size_gib(path: &str) -> Result<f64> {
    let size = disk_info(path)?["virtual-size"].as_f64().unwrap_or(0.0);
    Ok(size / crate::host::GIB as f64)
}
