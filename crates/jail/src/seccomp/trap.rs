//! What a refused call with [`super::Action::Trap`] turns into: one line on standard error the first
//! time each call is refused, and `ENOSYS` for the caller, which most code takes as "not here".
//!
//! A filter that's missing a call then costs a feature and leaves the reason in the log, where a
//! killed process would have left nothing.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

/// Where the call's number is in the kernel's `siginfo_t` for `SIGSYS`, after the three ints, the
/// padding and the address of the call.
const CALL_NUMBER_AT: usize = 24;
const NUMBERS_NOTED: usize = 1024;
const BITS: usize = u64::BITS as usize;
const MOST_DIGITS: usize = 20;
const STANDARD_ERROR: i32 = 2;
const SAID: &[u8] = b"the jail refused system call ";

static NOTED: [AtomicU64; NUMBERS_NOTED / BITS] = [const { AtomicU64::new(0) }; NUMBERS_NOTED / BITS];

/// Whether this is the first time this call is refused. Calls past the table are always said.
fn first_time(number: usize) -> bool {
    let Some(word) = NOTED.get(number / BITS) else { return true };
    let bit = 1 << (number % BITS);
    word.fetch_or(bit, Ordering::Relaxed) & bit == 0
}

/// The line, made with nothing that allocates or locks, as this runs in a signal handler.
fn line_for(number: usize, line: &mut [u8; SAID.len() + MOST_DIGITS + 1]) -> usize {
    line[..SAID.len()].copy_from_slice(SAID);
    let mut digits = [0u8; MOST_DIGITS];
    let mut count = 0;
    let mut left = number;
    loop {
        digits[count] = b'0' + (left % 10) as u8;
        count += 1;
        left /= 10;
        if left == 0 {
            break;
        }
    }
    for (place, digit) in digits[..count].iter().rev().enumerate() {
        line[SAID.len() + place] = *digit;
    }
    line[SAID.len() + count] = b'\n';
    SAID.len() + count + 1
}

extern "C" fn on_refused(_signal: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    // SAFETY: the kernel hands a SIGSYS handler a full siginfo and the thread's saved registers. The
    // call's return value is what's in rax when the handler returns.
    let number = unsafe {
        let number = info.cast::<u8>().add(CALL_NUMBER_AT).cast::<i32>().read();
        let registers = &mut (*context.cast::<libc::ucontext_t>()).uc_mcontext.gregs;
        registers[libc::REG_RAX as usize] = -i64::from(libc::ENOSYS);
        number as usize
    };
    if first_time(number) {
        let mut line = [0; SAID.len() + MOST_DIGITS + 1];
        let length = line_for(number, &mut line);
        // SAFETY: the buffer is on this stack and the length is inside it.
        unsafe { libc::write(STANDARD_ERROR, line.as_ptr().cast(), length) };
    }
}

/// Turns refused calls into a line and `ENOSYS`, for the whole process. It goes in before the
/// filter, so the first refusal already finds it.
pub fn report_refusals() -> io::Result<()> {
    // SAFETY: a zeroed sigaction is a valid one with no flags, and every field set below is the
    // kernel's to read.
    let done = unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_refused as *const () as usize;
        action.sa_flags = libc::SA_SIGINFO;
        libc::sigemptyset(&raw mut action.sa_mask);
        libc::sigaction(libc::SIGSYS, &raw const action, std::ptr::null_mut())
    };
    if done == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_call_is_said_once() {
        assert!(first_time(1000));
        assert!(!first_time(1000));
        assert!(first_time(NUMBERS_NOTED + 5));
        assert!(first_time(NUMBERS_NOTED + 5), "past the table, every time");
    }

    #[test]
    fn the_line_names_the_call() {
        let mut line = [0; SAID.len() + MOST_DIGITS + 1];
        let length = line_for(101, &mut line);
        assert_eq!(&line[..length], b"the jail refused system call 101\n");
        let length = line_for(0, &mut line);
        assert_eq!(&line[..length], b"the jail refused system call 0\n");
    }
}
