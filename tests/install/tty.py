#!/usr/bin/python3 -I
"""Runs a command with a real controlling terminal, and types the first argument into it.

With TTY_ANSWER_ON set, each answer is typed only when the command shows that text, one answer a time.
Otherwise all of them go in at the start, and anything that reads the terminal first can take them, as
apt-get does in an installer's root step.

A command that says nothing for TTY_QUIET_SECONDS, 300 by default, is waiting for an answer it wasn't
given, so it's stopped and this fails, rather than waiting for ever."""
import os
import pty
import select
import signal
import sys

QUIET_SECONDS = int(os.environ.get("TTY_QUIET_SECONDS", "300"))
ANSWER_ON = os.environ.get("TTY_ANSWER_ON", "").encode()

answers = sys.argv[1].encode().decode("unicode_escape").encode()
pid, terminal = pty.fork()
if pid == 0:
    os.execvp(sys.argv[2], sys.argv[2:])
if ANSWER_ON:
    waiting = answers.splitlines(keepends=True)
    seen = b""
else:
    os.write(terminal, answers)
while True:
    if not select.select([terminal], [], [], QUIET_SECONDS)[0]:
        sys.stdout.buffer.write(b"\ntty.py: nothing said for %d seconds, so it's waiting for an answer it wasn't given\n" % QUIET_SECONDS)
        sys.stdout.buffer.flush()
        os.killpg(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
        sys.exit(1)
    try:
        said = os.read(terminal, 65536)
    except OSError:
        break
    if not said:
        break
    sys.stdout.buffer.write(said.replace(b"\r\n", b"\n"))
    sys.stdout.buffer.flush()
    if ANSWER_ON:
        seen += said
        while ANSWER_ON in seen:
            seen = seen[seen.index(ANSWER_ON) + len(ANSWER_ON):]
            if waiting:
                os.write(terminal, waiting.pop(0))
        seen = seen[-len(ANSWER_ON):]
_, status = os.waitpid(pid, 0)
sys.exit(os.waitstatus_to_exitcode(status))
