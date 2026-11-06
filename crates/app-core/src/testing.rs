//! What the tests share: scratch folders.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A folder that goes when the test ends.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new() -> Self {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let folder = std::env::temp_dir().join(format!("guesthouse-test-{}-{number}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        Self(folder)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    pub fn root(&self) -> &Path {
        &self.0
    }

    /// Writes a file, making its folder.
    pub fn write(&self, name: &str, data: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, data).unwrap();
        path
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
