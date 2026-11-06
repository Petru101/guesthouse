use super::*;
use crate::testing::Scratch;
use std::fs;
use std::io::Cursor;
use std::os::unix::fs::symlink;

type Fields<'a> = &'a [(&'a str, &'a str)];

/// One request, as the guest would send it.
fn request(method: &str, path: &str, fields: Fields, body: &[u8]) -> Vec<u8> {
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: x\r\n");
    if !fields.iter().any(|(name, _)| *name == "Content-Length") {
        text += &format!("Content-Length: {}\r\n", body.len());
    }
    for (name, value) in fields {
        text += &format!("{name}: {value}\r\n");
    }
    [text.as_bytes(), b"\r\n", body].concat()
}

/// Sends requests to the share, and returns each reply's status and body.
fn dav(root: &Path, requests: &[Vec<u8>]) -> Vec<(u16, Vec<u8>)> {
    let mut output = Vec::new();
    Share::new(root).unwrap().serve(Cursor::new(requests.concat()), &mut output).unwrap();
    let mut replies = Vec::new();
    let mut data = &output[..];
    while !data.is_empty() {
        let end = data.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&data[..end]).into_owned();
        data = &data[end + 4..];
        let code = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        let size: usize =
            head.lines().find_map(|line| line.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
        replies.push((code, data[..size].to_vec()));
        data = &data[size..];
    }
    replies
}

fn codes(root: &Path, requests: &[Vec<u8>]) -> Vec<u16> {
    dav(root, requests).into_iter().map(|reply| reply.0).collect()
}

fn get(path: &str) -> Vec<u8> {
    request("GET", path, &[], b"")
}

/// A shared folder, with links out of it and one inside it.
fn shared(scratch: &Scratch) -> std::path::PathBuf {
    let root = scratch.path("shared");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("hello.txt"), "hi").unwrap();
    let secret = scratch.write("secret.txt", "secret");
    symlink(&secret, root.join("leak.txt")).unwrap();
    symlink(scratch.root(), root.join("leak")).unwrap();
    symlink(root.join("hello.txt"), root.join("inside.txt")).unwrap();
    root
}

#[test]
fn files_and_listings_are_served() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    assert_eq!(dav(&root, &[get("/hello.txt")]), [(200, b"hi".to_vec())]);
    let replies = dav(&root, &[request("PROPFIND", "/", &[("Depth", "1")], b"")]);
    let listing = String::from_utf8_lossy(&replies[0].1);
    assert_eq!(replies[0].0, 207);
    assert!(listing.contains("hello.txt") && listing.contains("sub/"));
    assert!(!listing.contains("leak"), "a link out of the folder must not even show");
}

#[test]
fn nothing_outside_the_folder_can_be_read() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    for path in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/sub/../../secret.txt",
        "/leak.txt",
        "/leak/secret.txt",
        "/..%5csecret.txt",
    ] {
        assert_eq!(dav(&root, &[get(path)]), [(404, Vec::new())], "{path} leads out of the folder");
    }
}

#[test]
fn nothing_outside_the_folder_can_be_changed() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    let requests = [
        request("PUT", "/../out.txt", &[], b"x"),
        request("PUT", "/leak.txt", &[], b"x"),
        request("DELETE", "/leak.txt", &[], b""),
        request("DELETE", "/", &[], b""),
        request("MKCOL", "/../made", &[], b""),
        request("MOVE", "/hello.txt", &[("Destination", "http://x/../moved")], b""),
        request("MOVE", "/leak.txt", &[("Destination", "http://x/kept.txt")], b""),
        request("COPY", "/hello.txt", &[("Destination", "http://x/leak/copied")], b""),
    ];
    assert_eq!(codes(&root, &requests), [403; 8]);
    let names = crate::machine::sorted_names(&root);
    assert_eq!(names, ["hello.txt", "inside.txt", "leak", "leak.txt", "sub"]);
    assert_eq!(fs::read_to_string(scratch.path("secret.txt")).unwrap(), "secret");
}

#[test]
fn a_link_to_nowhere_outside_cannot_be_written_through() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    symlink(scratch.path("not-yet"), root.join("dangling")).unwrap();
    assert_eq!(codes(&root, &[request("PUT", "/dangling", &[], b"x")]), [403]);
    assert!(!scratch.path("not-yet").exists());
}

#[test]
fn a_loop_of_links_leads_nowhere() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    symlink(root.join("b"), root.join("a")).unwrap();
    symlink(root.join("a"), root.join("b")).unwrap();
    assert_eq!(codes(&root, &[get("/a")]), [404]);
}

#[test]
fn a_link_inside_the_folder_is_fine() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    assert_eq!(dav(&root, &[get("/inside.txt")]), [(200, b"hi".to_vec())]);
}

#[test]
fn a_copy_as_windows_makes_it_goes_through() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    let requests = [
        request("MKCOL", "/new%20folder", &[], b""),
        request("PUT", "/new%20folder/f%C3%BC.txt", &[], b"data"),
        request("PROPPATCH", "/new%20folder/f%C3%BC.txt", &[], b"<Win32LastModifiedTime/>"),
        request("LOCK", "/new%20folder/f%C3%BC.txt", &[], b""),
        request("UNLOCK", "/new%20folder/f%C3%BC.txt", &[], b""),
        request("MOVE", "/new%20folder/f%C3%BC.txt", &[("Destination", "http://x/new%20folder/b.txt")], b""),
        request("COPY", "/new%20folder/b.txt", &[("Destination", "http://x/c.txt"), ("Overwrite", "F")], b""),
        request("COPY", "/hello.txt", &[("Destination", "http://x/c.txt"), ("Overwrite", "F")], b""),
        request("DELETE", "/new%20folder", &[], b""),
    ];
    assert_eq!(codes(&root, &requests), [201, 201, 207, 200, 204, 201, 201, 412, 204]);
    assert_eq!(fs::read_to_string(root.join("c.txt")).unwrap(), "data");
    assert!(!root.join("new folder").exists());
}

#[test]
fn the_simple_answers_are_right() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    assert_eq!(codes(&root, &[request("OPTIONS", "/", &[], b""), get("/gone")]), [200, 404]);
}

#[test]
fn what_follows_a_refusal_is_never_read() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    let after = request("MKCOL", "/after", &[], b"");
    let too_long = ((256u64 << 10) + 1).to_string();
    let far_too_long = (1u64 << 62).to_string();
    for length in [too_long.as_str(), &far_too_long] {
        let first = request("PROPPATCH", "/hello.txt", &[("Content-Length", length)], b"");
        assert_eq!(codes(&root, &[first, after.clone()]), [413]);
    }
    let huge = "9".repeat(5000);
    for length in ["many", "-1", "0x10", "1_0", "", huge.as_str()] {
        for method in ["PUT", "PROPPATCH", "MKCOL"] {
            let first = request(method, "/bad.txt", &[("Content-Length", length)], b"");
            assert_eq!(codes(&root, &[first, after.clone()]), [400], "{length}");
        }
    }
    let body = vec![b'x'; 256 << 10];
    assert_eq!(
        codes(&root, &[request("PROPPATCH", "/hello.txt", &[], &body)]),
        [207],
        "the limit itself is fine"
    );
    assert!(!root.join("after").exists() && !root.join("bad.txt").exists());
}

#[test]
fn a_refused_put_does_not_leave_its_body_to_be_read_as_a_request() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    let body = request("MKCOL", "/smuggled", &[], b"");
    let requests = [request("PUT", "/sub", &[], &body), get("/hello.txt")];
    assert_eq!(codes(&root, &requests), [403, 200]);
    assert!(!root.join("smuggled").exists());
}

#[test]
fn a_folder_cannot_be_copied_into_itself() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    assert_eq!(codes(&root, &[request("COPY", "/", &[("Destination", "http://x/sub/all")], b"")]), [403]);
    assert!(!root.join("sub/all").exists());
}

#[test]
fn a_copied_link_stays_a_link() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    let copy = request("COPY", "/sub", &[("Destination", "http://x/other")], b"");
    fs::write(root.join("sub/file"), "x").unwrap();
    symlink("../hello.txt", root.join("sub/link")).unwrap();
    assert_eq!(codes(&root, &[copy]), [201]);
    assert!(root.join("other/link").is_symlink());
}

#[test]
fn a_client_that_asks_is_told_to_go_on() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    let mut output = Vec::new();
    let put = request("PUT", "/new.txt", &[("Expect", "100-continue")], b"data");
    Share::new(&root).unwrap().serve(Cursor::new(put), &mut output).unwrap();
    assert!(output.starts_with(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201"));
}

#[test]
fn a_request_with_too_many_headers_is_dropped() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    let mut request = b"GET /a.txt HTTP/1.1\r\n".to_vec();
    for number in 0..100_000 {
        request.extend(format!("X-Field-{number}: value\r\n").as_bytes());
    }
    request.extend(b"\r\n");
    assert!(dav(&root, &[request]).is_empty());
}

#[test]
fn a_request_with_too_many_header_bytes_is_dropped() {
    let scratch = Scratch::new();
    let root = shared(&scratch);
    let mut request = b"GET /a.txt HTTP/1.1\r\n".to_vec();
    for _ in 0..40 {
        request.extend(format!("X-Field: {}\r\n", "a".repeat(10_000)).as_bytes());
    }
    request.extend(b"\r\n");
    assert!(dav(&root, &[request]).is_empty());
}

#[test]
fn a_path_checked_before_a_link_moved_in_stays_inside() {
    let scratch = Scratch::new();
    let root = scratch.path("outer/shared");
    fs::create_dir_all(root.join("x")).unwrap();
    fs::create_dir_all(root.join("d")).unwrap();
    symlink("..", root.join("d/l")).unwrap();
    let folder = inside::Folder::new(&root).unwrap();
    let checked = folder.path_of("/x/f").unwrap();
    fs::remove_dir(root.join("x")).unwrap();
    fs::rename(root.join("d/l"), root.join("x")).unwrap();
    folder.files.create(&checked).unwrap();
    assert!(!scratch.path("outer/f").exists(), "nothing may appear outside the folder");
    assert!(root.join("f").exists(), "the link's way up stops at the folder");
}
