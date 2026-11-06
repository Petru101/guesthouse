//! The tables a guest's modules are built with. They have to match the driver the host is running, so
//! they are worked out from that driver's public source: the source is read for names, and the C
//! compiler measures what those names mean.

mod calls;
mod classes;
mod controls;

use crate::failure::{Failure, Result};
use crate::machine::sorted_names;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const TABLE_FILES: [&str; 3] = ["nv_tables.h", "uvm_tables.h", "class_tables.h"];
/// What the device in `qemu/` was built with: sha256 of the tables' entries. A guest's tables come from
/// the driver the host has now, but the device's were fixed when QEMU was built. If a driver moves a
/// field the device checks, the two would disagree on what a call holds.
const DEVICE_TABLES: &str = "172f2e3f2db3880316b53bd69822ee6764deadc006357ffc35a13db1ed080c5a";
const ENTRY_START: &str = "{";
const COMMENT_START: &str = "/*";
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

/// The tables' entries with nothing else, so that a comment or a licence makes no difference.
fn entries(tables: &[String]) -> String {
    let lines = tables.iter().flat_map(|table| table.lines());
    let wanted = lines.filter(|line| line.starts_with(ENTRY_START));
    wanted.map(|line| format!("{}\n", line.split(COMMENT_START).next().unwrap_or(line).trim_end())).collect()
}

fn sha256(text: &str) -> Result<String> {
    let mut child = Command::new("sha256sum").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let mut input = child.stdin.take().ok_or_else(|| Failure::new("sha256sum took no input."))?;
    input.write_all(text.as_bytes())?;
    drop(input);
    let said = child.wait_with_output()?;
    Ok(String::from_utf8_lossy(&said.stdout).split_whitespace().next().unwrap_or_default().to_owned())
}

/// Whether the tables in `folder` say the same as the ones the device was built with.
pub fn match_the_device(folder: &Path) -> Result<bool> {
    let read = |name: &&str| read_text(&folder.join(name));
    let tables = TABLE_FILES.iter().map(read).collect::<Result<Vec<String>>>()?;
    Ok(sha256(&entries(&tables))? == DEVICE_TABLES)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    const PATCH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../qemu/qemu-gpupv.patch");

    /// One file's lines in the patch, which is the only place the device's tables are kept here.
    fn table_in_patch(patch: &str, name: &str) -> String {
        let start = patch.find(&format!("+++ b/hw/misc/nvfwd/{name}\n")).expect("the table is in the patch");
        let lines = patch[start..].lines().skip(2);
        let added = lines.take_while(|line| line.starts_with('+'));
        added.map(|line| format!("{}\n", &line[1..])).collect()
    }

    fn device_tables() -> Vec<String> {
        let patch = fs::read_to_string(PATCH).unwrap();
        TABLE_FILES.iter().map(|name| table_in_patch(&patch, name)).collect()
    }

    #[test]
    fn the_digest_is_of_the_tables_in_the_device() {
        assert_eq!(sha256(&entries(&device_tables())).unwrap(), DEVICE_TABLES);
    }

    #[test]
    fn a_comment_changes_nothing_and_a_size_does() {
        let scratch = Scratch::new();
        let mut tables = device_tables();
        for (name, table) in TABLE_FILES.iter().zip(&tables) {
            scratch.write(name, format!("/* another header */\n{table}"));
        }
        assert!(match_the_device(&scratch.path("")).unwrap());
        tables[2] = tables[2].replacen("{ 0x0, 4 }", "{ 0x0, 8 }", 1);
        scratch.write(TABLE_FILES[2], &tables[2]);
        assert!(!match_the_device(&scratch.path("")).unwrap());
    }
}
