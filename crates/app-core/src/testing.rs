//! What the tests share: scratch folders, and contexts that can't reach the real PC.

use crate::context::Context;
use crate::paths::{Firmware, Paths, Variables};
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

/// Paths that all lie in the scratch folder, so that no test can touch a real machine, a real run
/// folder or real shared memory.
pub fn scratch_paths(scratch: &Scratch) -> Paths {
    let folder = |name: &str| scratch.path(name).to_str().unwrap().to_owned();
    let (home, shm, program) = (folder("home"), folder("shm"), folder("program"));
    // As a real program folder does, this means GPU machines are possible.
    fs::create_dir_all(scratch.path("program/guest")).unwrap();
    let runtime = folder("xdg-run");
    let mut paths = Paths::new(
        &Variables::of(&[
            ("HOME", &home),
            ("XDG_RUNTIME_DIR", &runtime),
            ("GUESTHOUSE_SHARED_MEMORY", &shm),
            ("GUESTHOUSE_PROGRAM", &program),
        ]),
        None,
    );
    paths.firmware = Firmware {
        code: scratch.path("firmware/code"),
        vars: scratch.write("firmware/vars", "blank firmware settings"),
        code_secure: scratch.path("firmware/code-secure"),
        vars_secure: scratch.write("firmware/vars-secure", "firmware settings with keys"),
    };
    fs::create_dir_all(&paths.shared_memory).unwrap();
    paths
}

pub fn scratch_context(scratch: &Scratch) -> Context {
    let mut context = Context::new(scratch_paths(scratch), crate::slots::first());
    context.processes = scratch.path("proc");
    fs::create_dir_all(&context.processes).unwrap();
    context
}
