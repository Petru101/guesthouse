#!/usr/bin/env python3
"""Prints the version of the kernel driver that really answers on the host.

The question goes through the guest's module and the nvfwd device to the host's own driver, which
answers from what is loaded there. So this checks the host's claim in the image, instead of
trusting it. NVIDIA's user-mode driver asks the same question when it starts.
"""
import fcntl
import struct
import sys

CONTROL_NODE = "/dev/nvidiactl"
# From NVIDIA's nv-ioctl.h and nv-ioctl-numbers.h.
IOCTL_MAGIC = ord("F")
CHECK_VERSION_STR = 200 + 10
QUERY = ord("2")
RECOGNIZED = 1
VERSION_LENGTH = 64
REQUEST_FORMAT = f"II{VERSION_LENGTH}s"
READ_WRITE = 3


def request_number():
    size = struct.calcsize(REQUEST_FORMAT)
    return READ_WRITE << 30 | size << 16 | IOCTL_MAGIC << 8 | CHECK_VERSION_STR


def parse_reply(reply):
    _, answer, version = struct.unpack(REQUEST_FORMAT, bytes(reply))
    if answer != RECOGNIZED:
        return None
    return version.split(b"\0")[0].decode("ascii", "replace")


def main():
    buffer = bytearray(struct.pack(REQUEST_FORMAT, QUERY, 0, b""))
    try:
        with open(CONTROL_NODE, "rb", buffering=0) as node:
            fcntl.ioctl(node, request_number(), buffer)
    except OSError as error:
        sys.exit(f"{CONTROL_NODE}: {error}")
    version = parse_reply(buffer)
    if not version:
        sys.exit("the driver did not recognise the question")
    print(version)


if __name__ == "__main__":
    main()
