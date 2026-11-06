//! Says what an ISO is by looking inside it. Windows ISOs are UDF, which isoinfo can't read, so this
//! asks 7z, the one tool that reads both.

use crate::failure::{Failure, Result};
use crate::machine::Kind;
use std::collections::HashMap;
use std::io::{ErrorKind, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// What the answer file and its generic key install.
pub const WINDOWS_EDITION: &str = "Windows 11 Pro";
const FIRST_WINDOWS_11_BUILD: u32 = 22000;
const WIM_AMD64: &str = "9";
const WIM_MAGIC: &[u8; 8] = b"MSWIM\0\0\0";
const WIM_HEADER_SIZE: usize = 208;
/// Where the header says the XML block is: its size, then its offset.
const WIM_XML_ENTRY: usize = 0x48;
/// The top byte of the size holds flags.
const WIM_SIZE_MASK: u64 = (1 << 56) - 1;
/// The XML block is a few hundred KiB. The size comes from the file, so it's held to something sane.
const MOST_WIM_XML: u64 = 64 << 20;
const READ_SECONDS: u64 = 60;
const SKIP_CHUNK: usize = 1 << 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iso {
    /// Windows, or plain for anything that doesn't install by itself.
    pub kind: Kind,
    /// What it is, in a few words.
    pub title: String,
    pub edition: String,
}

impl Iso {
    fn plain(title: &str) -> Self {
        Self { kind: Kind::Plain, title: title.to_owned(), edition: String::new() }
    }
}

/// The text between `<tag>` and `</tag>`.
fn field<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let start = text.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = text[start..].find(&format!("</{tag}>"))?;
    Some(&text[start..start + end])
}

/// Each image's name, build and architecture, from a WIM's XML block.
fn images_in(text: &str) -> Vec<(String, u32, String)> {
    let mut images = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<IMAGE ") {
        let Some(end) = rest[start..].find("</IMAGE>") else { break };
        let image = &rest[start..start + end];
        let build = field(image, "BUILD").and_then(|text| text.parse().ok());
        if let (Some(name), Some(build), Some(arch)) = (field(image, "NAME"), build, field(image, "ARCH")) {
            images.push((name.to_owned(), build, arch.to_owned()));
        }
        rest = &rest[start + end..];
    }
    images
}

fn utf16_text(data: &[u8]) -> String {
    let units: Vec<u16> = data.as_chunks().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    String::from_utf16_lossy(&units)
}

/// The XML block of a WIM or ESD, which is near its end. 7z can only stream the file, which takes
/// seconds. `None` if this isn't a WIM.
fn wim_xml(stream: &mut impl Read) -> Option<String> {
    let mut header = [0; WIM_HEADER_SIZE];
    stream.read_exact(&mut header).ok()?;
    if &header[..WIM_MAGIC.len()] != WIM_MAGIC {
        return None;
    }
    let number = |at: usize| Some(u64::from_le_bytes(header[at..at + 8].try_into().ok()?));
    let size = (number(WIM_XML_ENTRY)? & WIM_SIZE_MASK).min(MOST_WIM_XML);
    let mut left = number(WIM_XML_ENTRY + 8)?.checked_sub(WIM_HEADER_SIZE as u64)?;
    let mut chunk = vec![0; SKIP_CHUNK];
    while left > 0 {
        let wanted = left.min(SKIP_CHUNK as u64) as usize;
        match stream.read(&mut chunk[..wanted]) {
            Ok(0) | Err(_) => return None,
            Ok(read) => left -= read as u64,
        }
    }
    let mut data = Vec::new();
    stream.take(size).read_to_end(&mut data).ok()?;
    Some(utf16_text(&data))
}

fn wim_images(path: &str, entry: &str) -> Vec<(String, u32, String)> {
    let child = Command::new("7z")
        .args(["x", "-so", path, entry])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return Vec::new() };
    let found = child.stdout.take().and_then(|mut stream| wim_xml(&mut stream));
    let _ = child.kill();
    let _ = child.wait();
    found.as_deref().map(images_in).unwrap_or_default()
}

/// 7z's listing of a disc, held to a time limit as a damaged file can take for ever.
fn list_disc(path: &str) -> Result<String> {
    let child = Command::new("7z")
        .args(["l", "-slt", path])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = child.map_err(|error| match error.kind() {
        ErrorKind::NotFound => Failure::new("Reading an ISO needs 7z, which is not installed."),
        _ => error.into(),
    })?;
    let mut stdout = child.stdout.take().ok_or_else(|| Failure::new("7z gave no output."))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stdout.read_to_end(&mut text);
        let _ = sender.send(text);
    });
    let Ok(text) = receiver.recv_timeout(Duration::from_secs(READ_SECONDS)) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failure::new("The ISO took too long to read."));
    };
    let status = child.wait()?;
    let text = String::from_utf8_lossy(&text).into_owned();
    let is_disc = text.lines().any(|line| line == "Type = Iso" || line == "Type = Udf");
    if !status.success() || !is_disc {
        return Err(Failure::new("This file is not a disc image."));
    }
    Ok(text)
}

/// Says what an ISO is.
pub fn read_iso(path: &str) -> Result<Iso> {
    let listing = list_disc(path)?;
    let names: HashMap<String, &str> = listing
        .lines()
        .filter_map(|line| line.strip_prefix("Path = "))
        .map(|name| (name.to_lowercase(), name))
        .collect();
    let entry = names.get("sources/install.wim").or_else(|| names.get("sources/install.esd"));
    let Some(entry) = entry else { return Ok(Iso::plain("Not Windows")) };
    for (name, build, architecture) in wim_images(path, entry) {
        if name == WINDOWS_EDITION && build >= FIRST_WINDOWS_11_BUILD && architecture == WIM_AMD64 {
            return Ok(Iso { kind: Kind::Windows, title: format!("{name}, build {build}"), edition: name });
        }
    }
    Ok(Iso::plain("Windows, but not 64-bit Windows 11 with the Pro edition"))
}

#[cfg(test)]
pub mod stand_in {
    use super::*;
    use crate::testing::Scratch;
    use std::fs;

    /// Enough of a WIM for `read_iso`: its header, then the XML block where the header says.
    pub fn wim(names: &[&str], build: u32, architecture: u32) -> Vec<u8> {
        const XML_AT: usize = 4096;
        let images: String = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                format!(
                    "<IMAGE INDEX=\"{index}\"><NAME>{name}</NAME><WINDOWS><ARCH>{architecture}</ARCH>\
                     <VERSION><BUILD>{build}</BUILD></VERSION></WINDOWS></IMAGE>"
                )
            })
            .collect();
        let xml: Vec<u8> = format!("<WIM>{images}</WIM>").encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut file = vec![0; XML_AT];
        file[..8].copy_from_slice(WIM_MAGIC);
        file[WIM_XML_ENTRY..WIM_XML_ENTRY + 8].copy_from_slice(&(xml.len() as u64 | 1 << 56).to_le_bytes());
        file[WIM_XML_ENTRY + 8..WIM_XML_ENTRY + 16].copy_from_slice(&(XML_AT as u64).to_le_bytes());
        file.extend(xml);
        file
    }

    pub fn make_iso(scratch: &Scratch, name: &str, files: &[(&str, &[u8])]) -> String {
        for (path, data) in files {
            scratch.write(&format!("{name}.tree/{path}"), data);
        }
        let iso = scratch.path(&format!("{name}.iso"));
        let tree = scratch.path(&format!("{name}.tree"));
        let done = Command::new("genisoimage")
            .args(["-quiet", "-J", "-r", "-udf", "-o"])
            .arg(&iso)
            .arg(&tree)
            .status();
        assert!(done.unwrap().success());
        fs::remove_dir_all(tree).unwrap();
        iso.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::{make_iso, wim};
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn a_windows_11_pro_disc_is_known_from_inside() {
        let scratch = Scratch::new();
        let data = wim(&["Windows 11 Home", "Windows 11 Pro"], 26300, 9);
        let found = read_iso(&make_iso(&scratch, "plain-name", &[("sources/install.wim", &data)])).unwrap();
        assert_eq!((found.kind, found.edition.as_str()), (Kind::Windows, "Windows 11 Pro"));
        assert!(found.title.contains("26300"));
        let data = wim(&["Windows 11 Pro"], 26300, 9);
        let esd = make_iso(&scratch, "esd", &[("sources/install.esd", &data)]);
        assert_eq!(read_iso(&esd).unwrap().kind, Kind::Windows, "an ESD counts too");
    }

    #[test]
    fn anything_else_is_a_plain_vm_whatever_its_name_says() {
        let scratch = Scratch::new();
        let enterprise = wim(&["Windows 11 Enterprise Evaluation"], 26300, 9);
        let ten = wim(&["Windows 11 Pro"], 19045, 9);
        let arm = wim(&["Windows 11 Pro"], 26300, 12);
        let cases: [(&str, &str, &[u8]); 5] = [
            ("windows_11_pro_x64", "boot/grub.cfg", b"linux"),
            ("no-pro", "sources/install.wim", &enterprise),
            ("ten", "sources/install.wim", &ten),
            ("arm", "sources/install.wim", &arm),
            ("broken", "sources/install.wim", b"not a wim"),
        ];
        for (name, path, data) in cases {
            assert_eq!(
                read_iso(&make_iso(&scratch, name, &[(path, data)])).unwrap().kind,
                Kind::Plain,
                "{name}"
            );
        }
    }

    #[test]
    fn a_file_that_is_no_disc_is_refused() {
        let scratch = Scratch::new();
        let path = scratch.write("text.iso", "this is not a disc").display().to_string();
        assert!(read_iso(&path).unwrap_err().0.contains("not a disc image"));
    }

    #[test]
    fn a_build_that_is_no_number_is_passed_over() {
        let xml = "<IMAGE INDEX=\"1\"><NAME>x</NAME><BUILD>many</BUILD><ARCH>9</ARCH></IMAGE>";
        assert!(images_in(xml).is_empty());
    }
}
