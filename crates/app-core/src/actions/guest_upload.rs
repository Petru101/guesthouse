//! Copies a big file into a running guest through its agent. The agent takes each request as one
//! line of text, so a file of a hundred megabytes goes a piece at a time, and under a name of its own
//! until the last piece is in, so a copy cut short is never taken for the file.

use super::Task;
use crate::failure::Result;
use crate::text::base64_encode;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// How much of the file one request carries.
const PIECE_BYTES: usize = 2 << 20;
const PIECE_SECONDS: u64 = 30;
const FINISH_SECONDS: u64 = 30;
const PIECE_MARK: &str = "piece written";
const SIZE_MARK: &str = "size ";
const PART: &str = ".part";

/// A path or name as PowerShell reads it between single quotes.
pub(super) fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Writes `data` at `offset` in `part`. Anything after it goes, so a piece sent again ends the file
/// in the same place. The mark comes only after the write, and the catch keeps a failed call from
/// letting the script go on to it.
fn piece_script(part: &str, offset: u64, data: &[u8]) -> String {
    format!(
        "try {{ $file = [IO.File]::Open({}, 'OpenOrCreate', 'Write'); \
         try {{ $file.SetLength({offset}); $file.Position = {offset}; \
         $data = [Convert]::FromBase64String('{}'); $file.Write($data, 0, $data.Length) }} \
         finally {{ $file.Close() }}; '{PIECE_MARK}' }} catch {{ \"FAILED: $_\" }}",
        quoted(part),
        base64_encode(data)
    )
}

fn finish_script(part: &str, guest: &str) -> String {
    format!(
        "Move-Item -LiteralPath {} -Destination {} -Force; '{SIZE_MARK}' + (Get-Item -LiteralPath {}).Length",
        quoted(part),
        quoted(guest),
        quoted(guest)
    )
}

/// Copies `local` to the path `guest` in the guest, and says whether all of it got there.
pub(super) fn put_big_file(task: &Task, local: &Path, guest: &str) -> Result<bool> {
    let part = format!("{guest}{PART}");
    let mut file = File::open(local)?;
    let size = file.metadata()?.len();
    let mut piece = vec![0; PIECE_BYTES];
    let mut offset = 0;
    // At least once, so an empty file is made too.
    loop {
        let read = file.read(&mut piece)?;
        if read == 0 && offset > 0 {
            break;
        }
        let said = task.ask_guest(&piece_script(&part, offset, &piece[..read]), PIECE_SECONDS);
        if !said.contains(PIECE_MARK) {
            return Ok(false);
        }
        offset += read as u64;
        if read == 0 {
            break;
        }
    }
    let said = task.ask_guest(&finish_script(&part, guest), FINISH_SECONDS);
    let guest_size = said.split(SIZE_MARK).nth(1).and_then(|rest| rest.trim().parse::<u64>().ok());
    Ok(offset == size && guest_size == Some(size))
}

#[cfg(test)]
pub mod stand_in {
    //! A guest that runs the scripts above on files it keeps in memory, and answers a list request
    //! with what it has.

    use super::*;
    use crate::agent::GuestAgent;
    use crate::text::base64_decode;
    use regex::Regex;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FilesGuest {
        pub files: Mutex<BTreeMap<String, Vec<u8>>>,
        /// What a request that is neither a piece nor a finish gets.
        pub other_answer: Mutex<String>,
        /// Fails each piece after this many.
        pub pieces_allowed: Mutex<Option<usize>>,
        pub requests: Mutex<Vec<String>>,
    }

    impl GuestAgent for FilesGuest {
        fn ask(&self, _port: u16, _token: &str, script: &str, _seconds: u64) -> String {
            self.requests.lock().unwrap().push(script.to_owned());
            let piece =
                Regex::new(r"Open\('([^']+)'.*SetLength\((\d+)\).*FromBase64String\('([^']*)'\)").unwrap();
            let finish = Regex::new(r"Move-Item -LiteralPath '([^']+)' -Destination '([^']+)'").unwrap();
            let mut files = self.files.lock().unwrap();
            if let Some(found) = piece.captures(script) {
                let mut allowed = self.pieces_allowed.lock().unwrap();
                if let Some(left) = allowed.as_mut() {
                    if *left == 0 {
                        return "ERROR: the disk is full".into();
                    }
                    *left -= 1;
                }
                let file = files.entry(found[1].to_owned()).or_default();
                file.truncate(found[2].parse().unwrap());
                file.extend(base64_decode(&found[3]).unwrap());
                return format!("{PIECE_MARK}\r\n");
            }
            if let Some(found) = finish.captures(script) {
                let data = files.remove(&found[1]).unwrap_or_default();
                let size = data.len();
                files.insert(found[2].to_owned(), data);
                return format!("{SIZE_MARK}{size}\r\n");
            }
            self.other_answer.lock().unwrap().clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::FilesGuest;
    use super::*;
    use crate::actions::tests::{machine, plain_task_context};
    use crate::testing::Scratch;
    use crate::vm_host::stand_in::RecordingHost;
    use std::sync::Arc;

    const GUEST: &str = "C:\\store\\big.dll";

    fn with_guest(scratch: &Scratch) -> (crate::context::Context, Arc<FilesGuest>) {
        let mut context = plain_task_context(scratch);
        let guest = Arc::new(FilesGuest::default());
        context.agent = guest.clone();
        (context, guest)
    }

    #[test]
    fn a_file_of_several_pieces_arrives_whole_and_under_its_own_name() {
        let scratch = Scratch::new();
        let (context, guest) = with_guest(&scratch);
        let data: Vec<u8> = (0..PIECE_BYTES * 2 + 1000).map(|index| (index * 7 % 251) as u8).collect();
        let local = scratch.write("big.dll", &data);
        let host = RecordingHost::default();
        let vm = machine(&context);
        crate::agent::make_token(&context, &vm).unwrap();
        let task = Task::new(&context, &host, "start", vm);
        assert!(put_big_file(&task, &local, GUEST).unwrap());
        let files = guest.files.lock().unwrap();
        assert_eq!(files.keys().collect::<Vec<_>>(), [GUEST]);
        assert!(files[GUEST] == data, "the same bytes");
        assert_eq!(guest.requests.lock().unwrap().len(), 4, "three pieces and the finish");
    }

    #[test]
    fn an_empty_file_is_made_too() {
        let scratch = Scratch::new();
        let (context, guest) = with_guest(&scratch);
        let local = scratch.write("empty.json", "");
        let host = RecordingHost::default();
        let vm = machine(&context);
        crate::agent::make_token(&context, &vm).unwrap();
        assert!(put_big_file(&Task::new(&context, &host, "start", vm), &local, GUEST).unwrap());
        assert_eq!(guest.files.lock().unwrap()[GUEST], Vec::<u8>::new());
    }

    #[test]
    fn a_piece_that_fails_leaves_only_the_part_file() {
        let scratch = Scratch::new();
        let (context, guest) = with_guest(&scratch);
        *guest.pieces_allowed.lock().unwrap() = Some(1);
        let local = scratch.write("big.dll", vec![1u8; PIECE_BYTES + 1]);
        let host = RecordingHost::default();
        let vm = machine(&context);
        crate::agent::make_token(&context, &vm).unwrap();
        assert!(!put_big_file(&Task::new(&context, &host, "start", vm), &local, GUEST).unwrap());
        let files = guest.files.lock().unwrap();
        assert_eq!(files.keys().collect::<Vec<_>>(), [&format!("{GUEST}{PART}")]);
    }

    #[test]
    fn a_quote_in_a_name_is_doubled() {
        assert_eq!(quoted("C:\\it's"), "'C:\\it''s'");
    }
}
