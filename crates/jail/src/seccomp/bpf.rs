//! A filter as the kernel's classic BPF. Each rule is a short block that jumps to the next block when
//! its call doesn't match, so no jump is ever longer than one block.

use super::{Action, Condition, Filter, Rule};
use libc::sock_filter;
use std::io;

const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
/// Calls with this bit set are x32's, which have numbers of their own.
const X32_CALL_BIT: u32 = 0x4000_0000;
const NUMBER_AT: u32 = 0;
const ARCH_AT: u32 = 4;
const ARGUMENTS_AT: u32 = 16;
const ARGUMENT_SIZE: u32 = 8;
const MOST_INSTRUCTIONS: usize = 4096;

const LOAD_WORD: u16 = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
const JUMP_EQUAL: u16 = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
const JUMP_ANY_BIT: u16 = (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16;
const AND: u16 = (libc::BPF_ALU | libc::BPF_AND | libc::BPF_K) as u16;
const RETURN: u16 = (libc::BPF_RET | libc::BPF_K) as u16;

const RETURN_KILL_PROCESS: u32 = 0x8000_0000;
const RETURN_TRAP: u32 = 0x0003_0000;
const RETURN_ERRNO: u32 = 0x0005_0000;
const RETURN_ALLOW: u32 = 0x7fff_0000;
const ERRNO_MASK: u32 = 0xffff;

fn statement(code: u16, k: u32) -> sock_filter {
    sock_filter { code, jt: 0, jf: 0, k }
}

fn jump(code: u16, k: u32, if_true: u8, if_false: u8) -> sock_filter {
    sock_filter { code, jt: if_true, jf: if_false, k }
}

fn verdict(action: Action) -> sock_filter {
    let k = match action {
        Action::Allow => RETURN_ALLOW,
        Action::Fail(errno) => RETURN_ERRNO | (errno as u32 & ERRNO_MASK),
        Action::Trap => RETURN_TRAP,
        Action::Kill => RETURN_KILL_PROCESS,
    };
    statement(RETURN, k)
}

/// The low word of an argument, which comes first on a little-endian machine.
fn load_argument(index: u8) -> sock_filter {
    statement(LOAD_WORD, ARGUMENTS_AT + u32::from(index) * ARGUMENT_SIZE)
}

/// The test, ending in a jump to the verdict right after it when it passes, or one past it when not.
fn test(condition: Condition) -> Vec<sock_filter> {
    let (to_verdict, past_it) = (0, 1);
    match condition {
        Condition::Equals(index, value) => {
            vec![load_argument(index), jump(JUMP_EQUAL, value, to_verdict, past_it)]
        }
        Condition::HasAll(index, bits) => {
            vec![load_argument(index), statement(AND, bits), jump(JUMP_EQUAL, bits, to_verdict, past_it)]
        }
        Condition::HasAny(index, bits) => {
            vec![load_argument(index), jump(JUMP_ANY_BIT, bits, to_verdict, past_it)]
        }
        Condition::HasNone(index, bits) => {
            vec![load_argument(index), jump(JUMP_ANY_BIT, bits, past_it, to_verdict)]
        }
    }
}

/// One rule's block. It starts and ends with the call's number in the accumulator.
fn block(rule: &Rule) -> Vec<sock_filter> {
    let Some(condition) = rule.when else {
        return vec![jump(JUMP_EQUAL, rule.call as u32, 0, 1), verdict(rule.action)];
    };
    let mut body = test(condition);
    body.push(verdict(rule.action));
    body.push(statement(LOAD_WORD, NUMBER_AT));
    let mut block = vec![jump(JUMP_EQUAL, rule.call as u32, 0, body.len() as u8)];
    block.extend(body);
    block
}

pub fn compile(filter: &Filter) -> io::Result<Vec<sock_filter>> {
    let mut program = vec![
        statement(LOAD_WORD, ARCH_AT),
        jump(JUMP_EQUAL, AUDIT_ARCH_X86_64, 1, 0),
        verdict(Action::Kill),
        statement(LOAD_WORD, NUMBER_AT),
        jump(JUMP_ANY_BIT, X32_CALL_BIT, 0, 1),
        verdict(Action::Kill),
    ];
    for rule in &filter.rules {
        program.extend(block(rule));
    }
    program.push(verdict(filter.otherwise));
    if program.len() > MOST_INSTRUCTIONS {
        return Err(io::Error::other("the filter is longer than the kernel takes"));
    }
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the program as the kernel would, on one call, and gives the verdict's value.
    fn run(program: &[sock_filter], number: u32, arguments: [u64; 6]) -> u32 {
        let mut data = [0u32; 16];
        data[0] = number;
        data[1] = AUDIT_ARCH_X86_64;
        for (index, argument) in arguments.iter().enumerate() {
            data[4 + 2 * index] = *argument as u32;
            data[5 + 2 * index] = (*argument >> 32) as u32;
        }
        let (mut at, mut accumulator) = (0, 0u32);
        loop {
            let instruction = program[at];
            at += 1;
            match instruction.code {
                LOAD_WORD => accumulator = data[instruction.k as usize / 4],
                AND => accumulator &= instruction.k,
                RETURN => return instruction.k,
                JUMP_EQUAL | JUMP_ANY_BIT => {
                    let taken = if instruction.code == JUMP_EQUAL {
                        accumulator == instruction.k
                    } else {
                        accumulator & instruction.k != 0
                    };
                    at += usize::from(if taken { instruction.jt } else { instruction.jf });
                }
                other => panic!("no such instruction {other:#x}"),
            }
        }
    }

    fn filter() -> Filter {
        Filter {
            rules: vec![
                Rule::always(libc::SYS_read as _, Action::Allow),
                Rule::when(
                    libc::SYS_clone as _,
                    Condition::HasAll(0, libc::CLONE_THREAD as u32),
                    Action::Allow,
                ),
                Rule::when(
                    libc::SYS_ioctl as _,
                    Condition::Equals(1, libc::TIOCSTI as u32),
                    Action::Fail(libc::EPERM),
                ),
                Rule::when(
                    libc::SYS_unshare as _,
                    Condition::HasNone(0, libc::CLONE_NEWUSER as u32),
                    Action::Allow,
                ),
                Rule::when(libc::SYS_socket as _, Condition::HasAny(0, 0), Action::Allow),
                Rule::always(libc::SYS_ioctl as _, Action::Allow),
            ],
            otherwise: Action::Trap,
        }
    }

    #[test]
    fn each_rule_decides_its_own_call() {
        let program = compile(&filter()).unwrap();
        let call = |number: libc::c_long, first: u64, second: u64| {
            run(&program, number as u32, [first, second, 0, 0, 0, 0])
        };
        assert_eq!(call(libc::SYS_read, 0, 0), RETURN_ALLOW);
        assert_eq!(call(libc::SYS_write, 0, 0), RETURN_TRAP);
        assert_eq!(call(libc::SYS_clone, libc::CLONE_THREAD as u64 | libc::CLONE_VM as u64, 0), RETURN_ALLOW);
        assert_eq!(call(libc::SYS_clone, libc::SIGCHLD as u64, 0), RETURN_TRAP, "a fork is no thread");
        assert_eq!(call(libc::SYS_ioctl, 3, libc::TIOCSTI), RETURN_ERRNO | libc::EPERM as u32);
        assert_eq!(call(libc::SYS_ioctl, 3, 0x5401), RETURN_ALLOW, "the next rule for the call still counts");
        assert_eq!(call(libc::SYS_unshare, libc::CLONE_NEWNS as u64, 0), RETURN_ALLOW);
        assert_eq!(call(libc::SYS_unshare, (libc::CLONE_NEWUSER | libc::CLONE_NEWNS) as u64, 0), RETURN_TRAP);
        assert_eq!(call(libc::SYS_socket, 2, 0), RETURN_TRAP, "no bit in an empty mask");
    }

    #[test]
    fn other_architectures_and_x32_are_killed() {
        let program = compile(&filter()).unwrap();
        assert_eq!(run(&program, libc::SYS_read as u32 | X32_CALL_BIT, [0; 6]), RETURN_KILL_PROCESS);
        let mut foreign = program.clone();
        foreign[1].k = 0;
        assert_eq!(run(&foreign, libc::SYS_read as u32, [0; 6]), RETURN_KILL_PROCESS);
    }
}
