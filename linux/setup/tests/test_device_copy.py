"""The guest's modules and the device each have a copy of what they share. They have to stay the same."""
import os
import re
import unittest

LINUX = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PATCH = os.path.join(os.path.dirname(LINUX), "qemu", "qemu-gpupv.patch")


def file_in_patch(name):
    with open(PATCH) as f:
        text = f.read()
    start = text.index(f"+++ b/hw/misc/nvfwd/{name}\n")
    body = text[text.index("@@", start):]
    lines = []
    for line in body.split("\n")[1:]:
        if not line.startswith("+"):
            break
        lines.append(line[1:])
    return "\n".join(lines) + "\n"


class Copies(unittest.TestCase):
    def assert_same(self, name, compare=lambda text: text):
        with open(os.path.join(LINUX, "guest", name)) as f:
            self.assertEqual(compare(f.read()), compare(file_in_patch(name)))

    def test_wire_format(self):
        self.assert_same("nvfwd_proto.h", lambda text: re.sub(r"^/\* Wire format.*\n", "", text, flags=re.M))

    def test_small_tables(self):
        self.assert_same("fd_controls.h")
        self.assert_same("nv_tables_extra.h")


if __name__ == "__main__":
    unittest.main()
