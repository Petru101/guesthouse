//! The tables a guest's modules are built with. They have to match the driver the host is running, so
//! they are worked out from that driver's public source: the source is read for names, and the C
//! compiler measures what those names mean.

mod calls;
mod classes;
mod controls;

use crate::failure::{Failure, Result};
use crate::machine::sorted_names;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const TABLE_FILES: [&str; 3] = ["nv_tables.h", "uvm_tables.h", "class_tables.h"];
const SDK: &str = "src/common/sdk/nvidia/inc";
const COMMON_INCLUDES: &str = "#include <stddef.h>\n#include <stdio.h>\n#include \"nvtypes.h\"\n";
const PROGRAM_START: &str = "int main(void){\n";
const PROGRAM_END: &str = "return 0;}\n";
const MOST_ERROR_TEXT: usize = 3000;

/// Writes the three tables into `folder`. `tree` is the driver's source.
pub fn write_tables(tree: &Path, folder: &Path) -> Result<()> {
    let work = folder.join("measuring");
    fs::create_dir_all(&work)?;
    fs::write(folder.join(TABLE_FILES[0]), controls::table(tree, &work)?)?;
    fs::write(folder.join(TABLE_FILES[1]), calls::table(tree, &work)?)?;
    fs::write(folder.join(TABLE_FILES[2]), classes::table(tree, &work)?)?;
    fs::remove_dir_all(&work)?;
    Ok(())
}

fn sdk(tree: &Path) -> PathBuf {
    tree.join(SDK)
}

/// Headers aren't always valid UTF-8, and only ASCII names are looked for.
fn read_text(path: &Path) -> Result<String> {
    let data = fs::read(path).map_err(|error| Failure(format!("{}: {error}", path.display())))?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}

/// Every file under `folder` of `root`, as a path from `root`.
fn files_under(root: &Path, folder: &str, found: &mut Vec<String>) {
    for name in sorted_names(&root.join(folder)) {
        let path = format!("{folder}/{name}");
        if root.join(&path).is_dir() {
            files_under(root, &path, found);
        } else {
            found.push(path);
        }
    }
}

fn includes(headers: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    headers.into_iter().map(|name| format!("#include \"{}\"\n", name.as_ref())).collect()
}

/// Builds and runs a program of `printf`s, and returns what it printed.
fn measure(work: &Path, source: &str, flags: &[String]) -> Result<String> {
    let (code, program) = (work.join("measure.c"), work.join("measure"));
    fs::write(&code, source)?;
    let built = Command::new("gcc").arg("-w").args(flags).arg("-o").arg(&program).arg(&code).output()?;
    if !built.status.success() {
        let said = String::from_utf8_lossy(&built.stderr);
        return Err(Failure(said.chars().take(MOST_ERROR_TEXT).collect()));
    }
    let ran = Command::new(&program).output()?;
    if !ran.status.success() {
        return Err(Failure::new("A program measuring the driver's types failed."));
    }
    Ok(String::from_utf8_lossy(&ran.stdout).into_owned())
}

fn include_flag(folder: &Path) -> String {
    format!("-I{}", folder.display())
}
