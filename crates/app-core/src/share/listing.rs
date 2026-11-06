//! The XML a WebDAV client expects.

use crate::share::inside::quote;
use chrono::DateTime;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// File times Windows sets, and is told it has, which it needs to finish a copy.
const WINDOWS_TIMES: [&str; 4] =
    ["Win32CreationTime", "Win32LastAccessTime", "Win32LastModifiedTime", "Win32FileAttributes"];
const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>";

pub fn http_date(seconds: i64) -> String {
    let time = DateTime::from_timestamp(seconds, 0).unwrap_or_default();
    time.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn entry(path: &Path, href: &[u8]) -> io::Result<String> {
    let info = fs::metadata(path)?;
    let folder = info.is_dir();
    let kind = if folder { "<D:collection/>" } else { "" };
    let size = if folder {
        String::new()
    } else {
        format!("<D:getcontentlength>{}</D:getcontentlength>", info.len())
    };
    let slash = if folder && !href.ends_with(b"/") { "/" } else { "" };
    Ok(format!(
        "<D:response><D:href>{}{slash}</D:href><D:propstat><D:prop>\
         <D:resourcetype>{kind}</D:resourcetype>{size}\
         <D:getlastmodified>{}</D:getlastmodified>\
         <D:creationdate>{}</D:creationdate>\
         </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        escape(&quote(href)),
        http_date(info.mtime()),
        http_date(info.ctime()),
    ))
}

pub fn xml_response(entries: &str) -> String {
    format!("{XML_HEADER}<D:multistatus xmlns:D=\"DAV:\">{entries}</D:multistatus>")
}

pub fn proppatch_body(href: &str, request: &str) -> String {
    let named = WINDOWS_TIMES.iter().filter(|name| request.contains(**name));
    let names: String = named.map(|name| format!("<{name} xmlns=\"urn:schemas-microsoft-com:\"/>")).collect();
    xml_response(&format!(
        "<D:response><D:href>{}</D:href><D:propstat><D:prop>{names}</D:prop>\
         <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        escape(href)
    ))
}

pub fn lock_body(token: &str) -> String {
    format!(
        "{XML_HEADER}<D:prop xmlns:D=\"DAV:\"><D:lockdiscovery><D:activelock>\
         <D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope><D:depth>0</D:depth>\
         <D:timeout>Second-3600</D:timeout><D:locktoken><D:href>{token}</D:href></D:locktoken>\
         </D:activelock></D:lockdiscovery></D:prop>"
    )
}
