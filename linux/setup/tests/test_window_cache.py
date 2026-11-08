#!/usr/bin/env python3
"""window-cache.sh on sample MTRRs, and gpu-up.sh giving it the device's memory before the module loads."""
import os
import subprocess
import tempfile
import unittest

from test_gpu_up import Guest, write

GUEST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "guest")
OVMF = ("reg00: base=0x080000000 ( 2048MB), size= 2048MB, count=1: uncachable\n"
        "reg01: base=0x4000000000 (262144MB), size=262144MB, count=1: uncachable\n")
RESOURCES = ("0x0000000080889000 0x0000000080889fff 0x0000000000040200\n"
             "0x0000000000000000 0x0000000000000000 0x0000000000000000\n"
             "0x0000005000000000 0x0000005001ffffff 0x000000000014220c\n"
             "0x0000000000000000 0x0000000000000000 0x0000000000000000\n"
             "0x0000004000000000 0x0000004fffffffff 0x000000000014220c\n")


def run(root, *arguments):
    return subprocess.run(["bash", os.path.join(GUEST, "window-cache.sh"), *arguments],
                          env=dict(os.environ, NVFWD_ROOT=root), capture_output=True, text=True)


class WindowCache(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.root = folder.name
        self.mtrr = os.path.join(self.root, "proc/mtrr")

    def written(self):
        with open(self.mtrr) as f:
            return f.read().splitlines()[2:]

    def test_the_window_comes_out_of_ovmfs_range_and_the_rest_stays(self):
        write(self.mtrr, OVMF)
        result = run(self.root, str(0x4000000000), str(0x1000000000))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.written(), ["disable=1",
                                          "base=0x6000000000 size=0x2000000000 type=uncachable",
                                          "base=0x5000000000 size=0x1000000000 type=uncachable"],
                         "the largest first")

    def test_every_piece_is_a_power_of_two_on_its_own_size(self):
        write(self.mtrr, OVMF)
        run(self.root, str(0x4000000000), str(0x1002000000))
        for line in self.written()[1:]:
            base, size = (int(word.split("=")[1], 16) for word in line.split()[:2])
            self.assertEqual(size & (size - 1), 0, line)
            self.assertEqual(base % size, 0, line)
        self.assertIn("base=0x5002000000 size=0x2000000 type=uncachable", self.written())
        self.assertEqual(self.written()[1], "base=0x6000000000 size=0x2000000000 type=uncachable")

    def test_a_cacheable_window_is_left_alone(self):
        write(self.mtrr, "reg00: base=0x0c0000000 ( 3072MB), size= 1024MB, count=1: uncachable\n")
        run(self.root, str(0x4000000000), str(0x1000000000))
        with open(self.mtrr) as f:
            self.assertEqual(len(f.read().splitlines()), 1)

    def test_with_no_mtrrs_it_does_nothing(self):
        self.assertEqual(run(self.root, "0", "4096").returncode, 0)


class GpuUpCachesTheDeviceFirst(unittest.TestCase):
    def test_the_slots_and_window_go_cacheable_before_the_module_loads(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        guest = Guest(folder.name)
        guest.add_device()
        guest.add_layers()
        write(os.path.join(guest.root, "sys/bus/pci/devices/0000:00:02.0/resource"), RESOURCES)
        write(os.path.join(guest.root, "proc/mtrr"), OVMF)
        guest.script("window-cache.sh", 'echo "window-cache $*" >> "$CALLS"')
        guest.run()
        with open(guest.calls) as f:
            calls = [line.strip() for line in f]
        self.assertIn(f"window-cache {0x4000000000} {0x1002000000}", calls)
        self.assertLess(calls.index(f"window-cache {0x4000000000} {0x1002000000}"), calls.index("modprobe nvidia"))


if __name__ == "__main__":
    unittest.main()
