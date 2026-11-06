//! Turning a URL into a path in the shared folder, and refusing any that leads out of it. Links count:
//! a path is judged by where it really goes, even if what it points at doesn't exist yet. The file
//! system is only ever touched through `anchored`, so this is a filter and not what keeps a path inside.

use super::anchored::Root;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};

/// Past this many links in one path, it's a loop.
const MOST_LINKS: u32 = 40;

pub struct Folder {
    root: PathBuf,
    pub files: Root,
}

impl Folder {
    pub fn new(root: &Path) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        Ok(Self { files: Root::new(&root)?, root })
    }

    pub fn is_root(&self, path: &Path) -> bool {
        path.as_os_str().is_empty()
    }

    fn holds(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }

    /// The path of a URL from the folder's root, or `None` if it leads out of the folder.
    pub fn path_of(&self, target: &str) -> Option<PathBuf> {
        let url = url_path(target);
        let parts: Vec<&[u8]> =
            url.split(|byte| *byte == b'/').filter(|part| !matches!(*part, b"" | b".")).collect();
        let bad = |part: &&[u8]| *part == b".." || part.contains(&0) || part.contains(&b'\\');
        if parts.iter().any(bad) {
            return None;
        }
        let path = parts.iter().fold(self.root.clone(), |path, part| path.join(OsStr::from_bytes(part)));
        let parent_inside =
            parts.is_empty() || real_path(path.parent()?).is_some_and(|real| self.holds(&real));
        let inside = parent_inside && real_path(&path).is_some_and(|real| self.holds(&real));
        inside.then(|| path.strip_prefix(&self.root).unwrap_or(&path).to_owned())
    }

    /// Where a URL really leads, for what follows links. An absolute link means nothing to `anchored`,
    /// which treats the folder as the root of everything, so it gets the path with its links undone.
    pub fn followed_path_of(&self, target: &str) -> Option<PathBuf> {
        let path = self.path_of(target)?;
        let real = real_path(&self.root.join(path))?;
        Some(real.strip_prefix(&self.root).ok()?.to_owned())
    }
}

/// The part of a URL that names a path, as it was sent.
pub fn raw_url_path(target: &str) -> Vec<u8> {
    let after_host =
        target.split_once("://").map_or(target, |(_, rest)| rest.find('/').map_or("", |at| &rest[at..]));
    after_host.split(['?', '#']).next().unwrap_or("").as_bytes().to_vec()
}

/// The part of a URL that names a path, decoded.
pub fn url_path(target: &str) -> Vec<u8> {
    unquote(&raw_url_path(target))
}

fn unquote(text: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(text.len());
    let hex = |at: usize| Some(char::from(*text.get(at)?).to_digit(16)? as u8);
    let mut at = 0;
    while at < text.len() {
        match (text[at], hex(at + 1), hex(at + 2)) {
            (b'%', Some(high), Some(low)) => {
                decoded.push(high << 4 | low);
                at += 3;
            }
            (byte, ..) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    decoded
}

/// A path as a URL holds it.
pub fn quote(text: &[u8]) -> String {
    let safe = |byte: u8| byte.is_ascii_alphanumeric() || b"/_.-~".contains(&byte);
    text.iter()
        .map(|&byte| if safe(byte) { char::from(byte).to_string() } else { format!("%{byte:02X}") })
        .collect()
}

/// Where a path really leads, following links through parts that exist and keeping the rest as it
/// is. `None` for a loop of links. `fs::canonicalize` won't do, as it refuses a path that doesn't
/// exist, and a link to nowhere is what a guest would write through.
fn real_path(path: &Path) -> Option<PathBuf> {
    let mut pending: VecDeque<OsString> = names(path).into();
    let mut done = PathBuf::from("/");
    let mut links = 0;
    while let Some(name) = pending.pop_front() {
        if name == ".." {
            done.pop();
            continue;
        }
        let next = done.join(&name);
        match fs::read_link(&next) {
            Ok(target) => {
                links += 1;
                if links > MOST_LINKS {
                    return None;
                }
                if target.is_absolute() {
                    done = PathBuf::from("/");
                }
                for part in names(&target).into_iter().rev() {
                    pending.push_front(part);
                }
            }
            Err(_) => done = next,
        }
    }
    Some(done)
}

fn names(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_owned()),
            Component::ParentDir => Some(OsString::from_vec(b"..".to_vec())),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => None,
        })
        .collect()
}
