//! What the daemon may call once it's serving. Everything it needs is open by then, so this is the
//! short list of what the message loop, the sensor thread and the host's management library use.
//! Anything else is refused with a line in the log, so a call missing here shows up there.

use super::all;
use crate::seccomp::{Action, Condition, Filter, Rule};
use libc::*;

/// Most often first: the loop polls, reads and writes, and the backend's work is ioctls.
const HOT: [c_long; 10] = [
    SYS_ppoll,
    SYS_read,
    SYS_write,
    SYS_recvfrom,
    SYS_sendto,
    SYS_recvmsg,
    SYS_sendmsg,
    SYS_ioctl,
    SYS_futex,
    SYS_sched_yield,
];

const MEMORY: [c_long; 10] = [
    SYS_mmap,
    SYS_munmap,
    SYS_mprotect,
    SYS_mremap,
    SYS_madvise,
    SYS_msync,
    SYS_brk,
    SYS_mlock,
    SYS_munlock,
    SYS_membarrier,
];

/// Opening is Landlock's to decide, so these are safe to have.
const FILES: [c_long; 41] = [
    SYS_openat,
    SYS_open,
    SYS_close,
    SYS_close_range,
    SYS_readv,
    SYS_writev,
    SYS_pread64,
    SYS_pwrite64,
    SYS_lseek,
    SYS_fstat,
    SYS_newfstatat,
    SYS_stat,
    SYS_lstat,
    SYS_statx,
    SYS_statfs,
    SYS_fstatfs,
    SYS_getdents64,
    SYS_readlink,
    SYS_readlinkat,
    SYS_access,
    SYS_faccessat,
    SYS_faccessat2,
    SYS_fcntl,
    SYS_flock,
    SYS_ftruncate,
    SYS_fallocate,
    SYS_fsync,
    SYS_fdatasync,
    SYS_unlink,
    SYS_unlinkat,
    SYS_rename,
    SYS_renameat,
    SYS_renameat2,
    SYS_mkdir,
    SYS_mkdirat,
    SYS_dup,
    SYS_dup2,
    SYS_dup3,
    SYS_pipe2,
    SYS_getcwd,
    SYS_memfd_create,
];

/// On sockets it already has: the viewer's and QEMU's.
const SOCKETS: [c_long; 10] = [
    SYS_accept,
    SYS_accept4,
    SYS_shutdown,
    SYS_getsockopt,
    SYS_setsockopt,
    SYS_getsockname,
    SYS_getpeername,
    SYS_socketpair,
    SYS_recvmmsg,
    SYS_sendmmsg,
];

const WAITING: [c_long; 18] = [
    SYS_poll,
    SYS_select,
    SYS_pselect6,
    SYS_epoll_pwait2,
    SYS_epoll_create1,
    SYS_epoll_ctl,
    SYS_epoll_wait,
    SYS_epoll_pwait,
    SYS_eventfd2,
    SYS_timerfd_create,
    SYS_timerfd_settime,
    SYS_timerfd_gettime,
    SYS_nanosleep,
    SYS_clock_nanosleep,
    SYS_clock_gettime,
    SYS_clock_getres,
    SYS_gettimeofday,
    SYS_restart_syscall,
];

const PROCESS: [c_long; 31] = [
    SYS_exit,
    SYS_exit_group,
    SYS_rt_sigaction,
    SYS_rt_sigprocmask,
    SYS_rt_sigreturn,
    SYS_sigaltstack,
    SYS_getpid,
    SYS_gettid,
    SYS_getppid,
    SYS_getuid,
    SYS_geteuid,
    SYS_getgid,
    SYS_getegid,
    SYS_getresuid,
    SYS_getresgid,
    SYS_getgroups,
    SYS_set_robust_list,
    SYS_get_robust_list,
    SYS_set_tid_address,
    SYS_rseq,
    SYS_getrandom,
    SYS_sched_getaffinity,
    SYS_sched_getparam,
    SYS_sched_getscheduler,
    SYS_getpriority,
    SYS_uname,
    SYS_sysinfo,
    SYS_getrusage,
    SYS_times,
    SYS_getrlimit,
    SYS_get_mempolicy,
];

/// It reaches nothing new: what it serves on was made before the filter.
const NETWORK: [c_long; 4] = [SYS_socket, SYS_connect, SYS_bind, SYS_listen];

/// Naming threads, the timer slack the loop sets, and reading its own settings.
const PRCTL_OPTIONS: [c_int; 6] =
    [PR_SET_NAME, PR_GET_NAME, PR_SET_TIMERSLACK, PR_GET_TIMERSLACK, PR_SET_VMA, PR_GET_DUMPABLE];

/// The filter for a daemon whose process id is `own_pid`, which is the one process it may signal.
pub fn daemon(own_pid: i32) -> Filter {
    let mut rules: Vec<Rule> = all(&HOT, Action::Allow).collect();
    for calls in [&MEMORY[..], &FILES, &SOCKETS, &WAITING, &PROCESS] {
        rules.extend(all(calls, Action::Allow));
    }
    // A thread, and never a new process. clone3 keeps its flags in memory, where a filter can't look,
    // so it says it isn't there and glibc goes back to clone. A fork is refused plainly and not
    // trapped, as glibc blocks signals around it, and a blocked SIGSYS ends the process.
    rules.push(Rule::when(SYS_clone, Condition::HasAll(0, CLONE_THREAD as u32), Action::Allow));
    rules.push(Rule::always(SYS_clone, Action::Fail(EPERM)));
    rules.push(Rule::always(SYS_clone3, Action::Fail(ENOSYS)));
    rules.push(Rule::when(SYS_tgkill, Condition::Equals(0, own_pid as u32), Action::Allow));
    rules.push(Rule::when(SYS_prlimit64, Condition::Equals(0, 0), Action::Allow));
    rules.extend(
        PRCTL_OPTIONS
            .iter()
            .map(|option| Rule::when(SYS_prctl, Condition::Equals(0, *option as u32), Action::Allow)),
    );
    rules.extend(all(&NETWORK, Action::Fail(EACCES)));
    Filter { rules, otherwise: Action::Trap }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_call_is_listed_twice() {
        let filter = daemon(1);
        let mut unconditional: Vec<c_long> =
            filter.rules.iter().filter(|rule| rule.when.is_none()).map(|rule| rule.call).collect();
        let count = unconditional.len();
        unconditional.sort_unstable();
        unconditional.dedup();
        assert_eq!(unconditional.len(), count);
    }

    #[test]
    fn what_reaches_out_is_not_on_the_list() {
        let filter = daemon(1);
        for call in [
            SYS_execve,
            SYS_execveat,
            SYS_fork,
            SYS_vfork,
            SYS_ptrace,
            SYS_process_vm_readv,
            SYS_kill,
            SYS_unshare,
            SYS_setns,
            SYS_mount,
            SYS_bpf,
            SYS_io_uring_setup,
        ] {
            assert!(
                filter.rules.iter().all(|rule| rule.call != call || rule.action != Action::Allow),
                "{call}"
            );
        }
    }
}
