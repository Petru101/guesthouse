#!/usr/bin/python3 -I
"""Runs a command with a real controlling terminal, and types the first argument into it."""
import os
import pty
import sys

answers = sys.argv[1].encode().decode("unicode_escape").encode()
pid, terminal = pty.fork()
if pid == 0:
    os.execvp(sys.argv[2], sys.argv[2:])
os.write(terminal, answers)
while True:
    try:
        said = os.read(terminal, 65536)
    except OSError:
        break
    if not said:
        break
    sys.stdout.buffer.write(said.replace(b"\r\n", b"\n"))
    sys.stdout.buffer.flush()
_, status = os.waitpid(pid, 0)
sys.exit(os.waitstatus_to_exitcode(status))
