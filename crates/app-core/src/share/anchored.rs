//! Everything the share does to the file system, from a handle on its root. Each path is walked by the
//! kernel inside that root, so what a path means can't change between a check and the use of it.

use rustix::fs::{
    AtFlags, CWD, Dir, FileType, Mode, OFlags, ResolveFlags, fstat, mkdirat, openat, openat2, readlinkat,
    renameat, statat, symlinkat, unlinkat,
};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, ErrorKind, copy};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const IN_ROOT: ResolveFlags = ResolveFlags::IN_ROOT.union(ResolveFlags::NO_MAGICLINKS);
const NEW_FILE: Mode = Mode::from_raw_mode(0o666);
const NEW_FOLDER: Mode = Mode::from_raw_mode(0o777);
const THIS_FOLDER: &str = ".";

/// What a listing or a reply needs to know about an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub is_folder: bool,
    pub is_file: bool,
    pub size: u64,
    pub modified: i64,
    pub changed: i64,
}

pub struct Root {
    handle: OwnedFd,
}

/// `path` with an empty one as this folder, which is how the root is written.
fn named(path: &Path) -> &Path {
    if path.as_os_str().is_empty() { Path::new(THIS_FOLDER) } else { path }
}

fn is_dots(name: &OsStr) -> bool {
    matches!(name.as_bytes(), b"." | b"..")
}

fn names_in(folder: impl AsFd) -> io::Result<Vec<(OsString, FileType)>> {
    let mut found = Vec::new();
    for entry in Dir::read_from(folder)? {
        let entry = entry?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes()).to_owned();
        if !is_dots(&name) {
            found.push((name, entry.file_type()));
        }
    }
    Ok(found)
}

fn folder_at(parent: impl AsFd, name: &OsStr) -> io::Result<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    Ok(openat(parent, name, flags, Mode::empty())?)
}

/// A link inside is only a name here, so it goes and what it points at stays.
fn remove_at(parent: &OwnedFd, name: &OsStr) -> io::Result<()> {
    let found = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(found.st_mode) != FileType::Directory {
        return Ok(unlinkat(parent, name, AtFlags::empty())?);
    }
    let folder = folder_at(parent, name)?;
    for (child, _) in names_in(&folder)? {
        remove_at(&folder, &child)?;
    }
    Ok(unlinkat(parent, name, AtFlags::REMOVEDIR)?)
}

/// Keeps links as links: following one could lead out of the share.
fn copy_at(from: &OwnedFd, name: &OsStr, to: &OwnedFd, new_name: &OsStr) -> io::Result<()> {
    let found = statat(from, name, AtFlags::SYMLINK_NOFOLLOW)?;
    match FileType::from_raw_mode(found.st_mode) {
        FileType::Symlink => {
            let target = readlinkat(from, name, Vec::new())?;
            let _ = unlinkat(to, new_name, AtFlags::empty());
            Ok(symlinkat(target, to, new_name)?)
        }
        FileType::Directory => {
            if let Err(error) = mkdirat(to, new_name, NEW_FOLDER)
                && io::Error::from(error).kind() != ErrorKind::AlreadyExists
            {
                return Err(error.into());
            }
            let (source, target) = (folder_at(from, name)?, folder_at(to, new_name)?);
            for (child, _) in names_in(&source)? {
                copy_at(&source, &child, &target, &child)?;
            }
            Ok(())
        }
        _ => {
            let read = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let write = OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let mut source = File::from(openat(from, name, read, Mode::empty())?);
            let mut target = File::from(openat(to, new_name, write, NEW_FILE)?);
            copy(&mut source, &mut target).map(|_| ())
        }
    }
}

impl Root {
    pub fn new(folder: &Path) -> io::Result<Self> {
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
        Ok(Self { handle: openat(CWD, folder, flags, Mode::empty())? })
    }

    fn open(&self, path: &Path, flags: OFlags, mode: Mode) -> io::Result<OwnedFd> {
        Ok(openat2(&self.handle, named(path), flags | OFlags::CLOEXEC, mode, IN_ROOT)?)
    }

    /// The folder an entry is in, and its name there. The root has neither.
    fn parent_of<'a>(&self, path: &'a Path) -> io::Result<(OwnedFd, &'a OsStr)> {
        let name = path.file_name().ok_or_else(|| io::Error::from(ErrorKind::InvalidInput))?;
        let parent = path.parent().unwrap_or(Path::new(""));
        Ok((self.open(parent, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty())?, name))
    }

    /// What's at this path, following links. `None` if there's nothing.
    pub fn info(&self, path: &Path) -> Option<Info> {
        let found = fstat(self.open(path, OFlags::PATH, Mode::empty()).ok()?).ok()?;
        let kind = FileType::from_raw_mode(found.st_mode);
        Some(Info {
            is_folder: kind == FileType::Directory,
            is_file: kind == FileType::RegularFile,
            size: found.st_size as u64,
            modified: found.st_mtime,
            changed: found.st_ctime,
        })
    }

    pub fn read(&self, path: &Path) -> io::Result<File> {
        Ok(File::from(self.open(path, OFlags::RDONLY, Mode::empty())?))
    }

    pub fn create(&self, path: &Path) -> io::Result<File> {
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC;
        Ok(File::from(self.open(path, flags, NEW_FILE)?))
    }

    /// Makes an empty file, and leaves one that's there as it is.
    pub fn touch(&self, path: &Path) -> io::Result<()> {
        self.open(path, OFlags::WRONLY | OFlags::CREATE | OFlags::APPEND, NEW_FILE).map(|_| ())
    }

    pub fn names(&self, path: &Path) -> io::Result<Vec<OsString>> {
        let folder = self.open(path, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty())?;
        Ok(names_in(&folder)?.into_iter().map(|(name, _)| name).collect())
    }

    pub fn make_folder(&self, path: &Path) -> io::Result<()> {
        let (parent, name) = self.parent_of(path)?;
        Ok(mkdirat(&parent, name, NEW_FOLDER)?)
    }

    pub fn remove(&self, path: &Path) -> io::Result<()> {
        let (parent, name) = self.parent_of(path)?;
        remove_at(&parent, name)
    }

    pub fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let ((source, name), (target, new_name)) = (self.parent_of(from)?, self.parent_of(to)?);
        Ok(renameat(&source, name, &target, new_name)?)
    }

    pub fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        let ((source, name), (target, new_name)) = (self.parent_of(from)?, self.parent_of(to)?);
        copy_at(&source, name, &target, new_name)
    }
}
