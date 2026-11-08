#!/usr/bin/env python3
"""The small pieces of the guest side: the version question, the units, the package lists."""
import configparser
import importlib.util
import os
import struct
import subprocess
import unittest

SETUP = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GUEST = os.path.join(SETUP, "guest")
spec = importlib.util.spec_from_file_location("driver_version", os.path.join(GUEST, "driver-version.py"))
driver_version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver_version)


def unit(name):
    parser = configparser.RawConfigParser(strict=False)
    parser.optionxform = str
    parser.read(os.path.join(GUEST, "units", name))
    return parser


class VersionQuestion(unittest.TestCase):
    def test_request_is_nvidias_own_number(self):
        # _IOWR('F', 210, nv_ioctl_rm_api_version_t), a struct of 72 bytes.
        self.assertEqual(driver_version.request_number(), 0xC04846D2)

    def test_reply_is_read(self):
        reply = struct.pack(driver_version.REQUEST_FORMAT, driver_version.QUERY, 1, b"615.78.08")
        self.assertEqual(driver_version.parse_reply(reply), "615.78.08")

    def test_an_unrecognised_reply_is_no_version(self):
        reply = struct.pack(driver_version.REQUEST_FORMAT, driver_version.QUERY, 0, b"")
        self.assertIsNone(driver_version.parse_reply(reply))


class AllowList(unittest.TestCase):
    def entries(self):
        with open(os.path.join(os.path.dirname(SETUP), "allow.txt")) as f:
            return {tuple(line.split()[:2]) for line in f if line.strip()}

    def test_nvenc_has_what_it_needs(self):
        for entry in (("ctrl", "0x800201"), ("ctrl", "0x801109"), ("ctrl", "0x801f02"), ("ctrl", "0x2080016c"),
                      ("ctrl", "0x20808163"), ("class", "0x2")):
            self.assertIn(entry, self.entries())

    def test_its_two_neighbours_stay_refused(self):
        for control in ("0x20808164", "0x20808165"):
            self.assertFalse([entry for entry in self.entries() if entry[1] == control])


class Units(unittest.TestCase):
    def test_a_dead_gpu_step_cannot_stop_the_desktop(self):
        desktop = unit("nvfwd-desktop.service")
        self.assertIn("nvfwd-gpu.service", desktop["Unit"]["Wants"])
        self.assertNotIn("Requires", desktop["Unit"])
        self.assertNotIn("BindsTo", desktop["Unit"])

    def test_the_session_drops_the_capability_bwrap_refuses(self):
        self.assertIn("--ambient-caps=-wake_alarm", unit("nvfwd-desktop.service")["Service"]["ExecStart"])

    def test_first_boot_makes_no_ordering_cycle_with_cloud_final(self):
        first_boot = unit("nvfwd-first-boot.service")
        self.assertIn("cloud-final.service", first_boot["Unit"]["After"])
        self.assertEqual(first_boot["Unit"]["DefaultDependencies"], "no")

    def test_every_file_a_unit_runs_is_ours_and_executable(self):
        for name, key in (("nvfwd-first-boot.service", "ExecStart"), ("nvfwd-gpu.service", "ExecStart")):
            path = unit(name)["Service"][key].replace("/usr/lib/nvfwd", GUEST)
            self.assertTrue(os.access(path, os.X_OK), path)


class SessionSettings(unittest.TestCase):
    def test_they_are_lines_a_shell_and_environment_d_both_read(self):
        with open(os.path.join(GUEST, "files/gpu-session.env")) as f:
            lines = [line.strip() for line in f if line.strip() and not line.startswith("#")]
        names = [line.split("=", 1)[0] for line in lines]
        for line in lines:
            self.assertRegex(line, r"^[A-Z_]+=[^ '\"$]+$")
        self.assertIn("GALLIUM_DRIVER", names)
        output = subprocess.run(["bash", "-c", f'set -a; . {GUEST}/files/gpu-session.env; echo "$ZINK_DEBUG"'],
                                capture_output=True, text=True, check=True).stdout
        self.assertEqual(output.strip(), "noshobj")


class PowerButton(unittest.TestCase):
    def test_it_powers_off_whatever_a_desktop_holds(self):
        parser = configparser.RawConfigParser()
        parser.optionxform = str
        parser.read(os.path.join(GUEST, "files/logind-nvfwd.conf"))
        self.assertEqual(parser["Login"]["HandlePowerKey"], "poweroff")
        self.assertEqual(parser["Login"]["PowerKeyIgnoreInhibited"], "yes")

    def test_both_routes_install_it(self):
        for script in ("first-boot.sh", "setup-own.sh"):
            with open(os.path.join(GUEST, script)) as f:
                self.assertIn("logind.conf.d/90-nvfwd.conf", f.read(), script)


class FirstBoot(unittest.TestCase):
    def test_it_never_unloads_itself_while_it_runs(self):
        with open(os.path.join(GUEST, "first-boot.sh")) as f:
            text = f.read()
        self.assertIn("systemctl disable --no-reload nvfwd-first-boot.service", text)
        self.assertLess(text.index('touch "$done_marker"'), text.index("systemctl disable --no-reload"))


class Scripts(unittest.TestCase):
    def shell_scripts(self):
        for folder in (GUEST,):
            for name in sorted(os.listdir(folder)):
                if name.endswith(".sh"):
                    yield os.path.join(folder, name)

    def test_all_parse(self):
        for path in self.shell_scripts():
            subprocess.run(["bash", "-n", path], check=True)

    def test_run_directly_ones_are_executable(self):
        for path in self.shell_scripts():
            if os.path.basename(path) not in ("common.sh", "distro.sh", "own-desktop.sh"):
                self.assertTrue(os.access(path, os.X_OK), path)


class Packages(unittest.TestCase):
    def names(self, file):
        with open(os.path.join(GUEST, "files", file)) as f:
            return [line.strip() for line in f if line.strip() and not line.startswith("#")]

    def test_no_test_only_tools_or_debug_symbols(self):
        everything = self.names("packages.txt") + self.names("packages-no-recommends.txt")
        for name in everything:
            self.assertFalse(name.endswith("-dbgsym"), name)
        for name in ("gdb", "strace", "ltrace"):
            self.assertNotIn(name, everything)

    def test_what_the_build_needs_is_listed(self):
        everything = self.names("packages.txt") + self.names("packages-no-recommends.txt")
        for name in ("build-essential", "libvulkan-dev", "libwayland-dev",
                     "libpipewire-0.3-dev", "libglib2.0-dev", "glslang-tools", "pkg-config"):
            self.assertIn(name, everything)


if __name__ == "__main__":
    unittest.main()
