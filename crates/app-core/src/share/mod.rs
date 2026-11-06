//! A small WebDAV server for the folder a machine shares with its guest.
//!
//! QEMU starts this for each connection the guest makes to the share's address, with the connection
//! on stdin and stdout, so no port on the host is open to anything else. Windows has a WebDAV client
//! built in, which works with no account and no change to the guest's security settings.
//!
//! The guest isn't trusted: nothing here reaches outside the folder, links included.

mod http;
mod inside;
mod listing;

use http::{Request, reply};
use inside::{Folder, url_path};
use listing::{entry, http_date, lock_body, proppatch_body, xml_response};
use std::fs::{self, File};
use std::io::{self, BufRead, ErrorKind, Read, Write, copy};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::Path;

/// Windows sends a few hundred bytes. This is the only body held whole.
const MOST_PROPPATCH: u64 = 256 << 10;
const ALLOWED_METHODS: &str = "OPTIONS,GET,HEAD,PUT,DELETE,MKCOL,MOVE,COPY,PROPFIND,PROPPATCH,LOCK,UNLOCK";
const XML: &str = "application/xml; charset=\"utf-8\"";
/// There's no way to tell what kind a guest's file is, and a WebDAV client doesn't ask.
const ANY_FILE: &str = "application/octet-stream";
/// Locks are granted and not kept: one guest, one user.
const LOCK_TOKEN: &str = "opaquelocktoken:guesthouse";
const CLOSE: [(&str, &str); 1] = [("Connection", "close")];

pub struct Share {
    folder: Folder,
}

impl Share {
    pub fn new(root: &Path) -> io::Result<Self> {
        Ok(Self { folder: Folder::new(root)? })
    }

    /// Answers requests until the connection ends.
    pub fn serve(&mut self, mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
        loop {
            let Some(mut request) = Request::read(&mut input, &mut output)? else { return Ok(()) };
            let keep_going = self.answer(&mut request, &mut input, &mut output)?;
            output.flush()?;
            if !keep_going || request.close {
                return Ok(());
            }
        }
    }

    /// Says whether the connection can go on.
    fn answer(
        &self,
        request: &mut Request,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> io::Result<bool> {
        if !request.length_is_valid() {
            reply(output, 400, b"", &CLOSE)?;
            return Ok(false);
        }
        let method = request.method.clone();
        match method.as_str() {
            "OPTIONS" => self.options(request, input, output)?,
            "PROPFIND" => self.propfind(request, input, output)?,
            "PROPPATCH" => return self.proppatch(request, input, output),
            "HEAD" => self.get(request, output, false)?,
            "GET" => self.get(request, output, true)?,
            "PUT" => self.put(request, input, output)?,
            "DELETE" => self.delete(request, input, output)?,
            "MKCOL" => self.make_folder(request, input, output)?,
            "MOVE" => self.move_entry(request, input, output)?,
            "COPY" => self.copy_entry(request, input, output)?,
            "LOCK" => self.lock(request, input, output)?,
            "UNLOCK" => {
                request.discard_body(input)?;
                reply(output, 204, b"", &[])?;
            }
            _ => {
                reply(output, 501, b"", &CLOSE)?;
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn options(
        &self,
        request: &Request,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> io::Result<()> {
        request.discard_body(input)?;
        reply(output, 200, b"", &[("DAV", "1,2"), ("MS-Author-Via", "DAV"), ("Allow", ALLOWED_METHODS)])
    }

    fn propfind(
        &self,
        request: &Request,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> io::Result<()> {
        request.discard_body(input)?;
        let Some(path) = self.folder.path_of(&request.target).filter(|path| path.exists()) else {
            return reply(output, 404, b"", &[]);
        };
        let href = url_path(&request.target);
        let mut entries = vec![entry(&path, &href)?];
        if path.is_dir() && request.header("Depth") != Some("0") {
            let parent = href.strip_suffix(b"/").unwrap_or(&href);
            let mut names: Vec<_> = fs::read_dir(&path)?.flatten().map(|found| found.file_name()).collect();
            names.sort();
            for name in names {
                let child = path.join(&name);
                let child_href = [parent, b"/", name.as_bytes()].concat();
                let target = inside::quote(&child_href);
                if self.folder.path_of(&target).is_some() && child.exists() {
                    entries.push(entry(&child, &child_href)?);
                }
            }
        }
        reply(output, 207, xml_response(&entries.concat()).as_bytes(), &[("Content-Type", XML)])
    }

    /// Windows sets file times this way, and a refusal fails the whole copy.
    fn proppatch(
        &self,
        request: &Request,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> io::Result<bool> {
        let length = request.length();
        if length > MOST_PROPPATCH {
            reply(output, 413, b"", &CLOSE)?;
            return Ok(false);
        }
        let mut body = Vec::new();
        input.by_ref().take(length).read_to_end(&mut body)?;
        let text = String::from_utf8_lossy(&body);
        let href = String::from_utf8_lossy(&inside::raw_url_path(&request.target)).into_owned();
        reply(output, 207, proppatch_body(&href, &text).as_bytes(), &[("Content-Type", XML)])?;
        Ok(true)
    }

    fn get(&self, request: &Request, output: &mut impl Write, send: bool) -> io::Result<()> {
        let Some(path) = self.folder.path_of(&request.target).filter(|path| path.is_file()) else {
            return reply(output, 404, b"", &[]);
        };
        let found = fs::metadata(&path)?;
        let (size, modified) = (found.len().to_string(), http_date(found.mtime()));
        let headers = [("Content-Type", ANY_FILE), ("Content-Length", &size), ("Last-Modified", &modified)];
        http::headers(output, 200, &headers)?;
        if send {
            copy(&mut File::open(path)?, output)?;
        }
        Ok(())
    }

    fn put(&self, request: &Request, input: &mut impl BufRead, output: &mut impl Write) -> io::Result<()> {
        let path = self.folder.path_of(&request.target).filter(|path| !path.is_dir());
        let Some(path) = path else {
            request.discard_body(input)?;
            return reply(output, 403, b"", &[]);
        };
        let new = !path.exists();
        match File::create(&path) {
            Ok(mut file) => {
                copy(&mut input.by_ref().take(request.length()), &mut file)?;
                reply(output, if new { 201 } else { 204 }, b"", &[])
            }
            Err(error) => {
                request.discard_body(input)?;
                reply_error(output, &error)
            }
        }
    }

    fn delete(&self, request: &Request, input: &mut impl BufRead, output: &mut impl Write) -> io::Result<()> {
        request.discard_body(input)?;
        let Some(path) = self.folder.path_of(&request.target).filter(|path| !self.folder.is_root(path))
        else {
            return reply(output, 403, b"", &[]);
        };
        let done = if path.is_dir() && !path.is_symlink() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        answered(output, done, 204)
    }

    fn make_folder(
        &self,
        request: &Request,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> io::Result<()> {
        request.discard_body(input)?;
        let Some(path) = self.folder.path_of(&request.target) else { return reply(output, 403, b"", &[]) };
        answered(output, fs::create_dir(path), 201)
    }

    fn destination(&self, request: &Request) -> Option<std::path::PathBuf> {
        let target = request.header("Destination").filter(|text| !text.is_empty()).unwrap_or(&request.target);
        self.folder.path_of(target).filter(|path| !self.folder.is_root(path))
    }

    fn move_entry(
        &self,
        request: &Request,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> io::Result<()> {
        request.discard_body(input)?;
        let path = self.folder.path_of(&request.target).filter(|path| !self.folder.is_root(path));
        let (Some(path), Some(target)) = (path, self.destination(request)) else {
            return reply(output, 403, b"", &[]);
        };
        let replaced = target.exists();
        if replaced && request.header("Overwrite") == Some("F") {
            return reply(output, 412, b"", &[]);
        }
        answered(output, fs::rename(path, target), if replaced { 204 } else { 201 })
    }

    fn copy_entry(
        &self,
        request: &Request,
        input: &mut impl BufRead,
        output: &mut impl Write,
    ) -> io::Result<()> {
        request.discard_body(input)?;
        let (Some(path), Some(target)) = (self.folder.path_of(&request.target), self.destination(request))
        else {
            return reply(output, 403, b"", &[]);
        };
        // A folder copied into itself would go on until the disk was full.
        if target.starts_with(&path) && path.is_dir() {
            return reply(output, 403, b"", &[]);
        }
        let replaced = target.exists();
        if replaced && request.header("Overwrite") == Some("F") {
            return reply(output, 412, b"", &[]);
        }
        answered(output, copy_tree(&path, &target), if replaced { 204 } else { 201 })
    }

    fn lock(&self, request: &Request, input: &mut impl BufRead, output: &mut impl Write) -> io::Result<()> {
        request.discard_body(input)?;
        let Some(path) = self.folder.path_of(&request.target) else { return reply(output, 403, b"", &[]) };
        let created = !path.exists();
        if created && let Err(error) = File::options().append(true).create(true).open(&path) {
            return reply_error(output, &error);
        }
        let token = format!("<{LOCK_TOKEN}>");
        let headers = [("Lock-Token", token.as_str()), ("Content-Type", XML)];
        reply(output, if created { 201 } else { 200 }, lock_body(LOCK_TOKEN).as_bytes(), &headers)
    }
}

/// Copies a file or folder, keeping links as links: following one could lead out of the share.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let found = fs::symlink_metadata(from)?;
    if found.is_symlink() {
        let _ = fs::remove_file(to);
        symlink(fs::read_link(from)?, to)
    } else if found.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// What went wrong on the file system, in HTTP.
fn reply_error(output: &mut impl Write, error: &io::Error) -> io::Result<()> {
    let code = match error.kind() {
        ErrorKind::NotFound => 404,
        ErrorKind::PermissionDenied => 403,
        _ => 409,
    };
    reply(output, code, b"", &[])
}

fn answered(output: &mut impl Write, done: io::Result<()>, code: u16) -> io::Result<()> {
    match done {
        Ok(()) => reply(output, code, b"", &[]),
        Err(error) => reply_error(output, &error),
    }
}

#[cfg(test)]
mod tests;
