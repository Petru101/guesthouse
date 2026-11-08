#!/usr/bin/env python3
"""A guest installed by hand, with its own login screen and desktop on the GPU: gpu-up.sh choosing where
the desktop goes, desktop-watch.sh taking it back to the plain screen, setup-own.sh and check.sh."""
import os
import subprocess
import tempfile
import unittest

from test_check import FakeGuest
from test_gpu_up import Guest, write
from test_own_install import OwnGuest

V = "615.78.08"
SETTINGS = "run/environment.d/90-nvfwd.conf"
IGNORED = "run/udev/rules.d/71-nvfwd-plain-card.rules"


class DesktopGuest(Guest):
    """gpu-up.sh's scratch guest after setup-own.sh, with a plain card, ours, and a display manager."""

    def __init__(self, folder, manager="gdm.service"):
        super().__init__(folder)
        write(os.path.join(self.root, "etc/nvfwd/own-install.conf"), "family=fedora\nlibraries=/usr/lib64\n")
        self.serial = os.path.join(self.root, "dev/ttyS0")
        write(self.serial, "")
        self.add_device()
        self.add_layers()
        write(os.path.join(self.root, "sys/class/drm/card0/device/device"), "0x1111\n")
        write(os.path.join(self.root, "sys/class/drm/card1/device/device"), "0x11ea\n")
        self.connector = os.path.join(self.root, "sys/class/drm/card1-Virtual-1")
        write(os.path.join(self.connector, "status"), "connected\n")
        write(os.path.join(self.connector, "enabled"), "disabled\n")
        self.fake("systemctl", "exit 0")
        self.fake("sleep", "exit 0")
        if manager:
            self.set_manager(manager)

    def set_manager(self, name):
        link = os.path.join(self.root, "etc/systemd/system/display-manager.service")
        os.makedirs(os.path.dirname(link), exist_ok=True)
        if os.path.lexists(link):
            os.remove(link)
        os.symlink(f"/usr/lib/systemd/system/{name}", link)

    def said(self):
        with open(self.serial) as f:
            return f.read().splitlines()

    def exists(self, name):
        return os.path.exists(os.path.join(self.root, name))

    def read(self, name):
        with open(os.path.join(self.root, name)) as f:
            return f.read()

    def watch(self, seconds=0):
        environment = dict(os.environ, NVFWD_ROOT=self.root, CALLS=self.calls, NVFWD_LIGHT_SECONDS=str(seconds),
                           PATH=f"{self.bin}:{os.environ['PATH']}")
        return subprocess.run(["bash", os.path.join(self.scripts, "desktop-watch.sh")], env=environment,
                              capture_output=True, text=True)

    def calls_in_order(self):
        with open(self.calls) as f:
            return [line.strip() for line in f]


class GpuUpChoosesTheScreen(unittest.TestCase):
    def guest(self, manager="gdm.service"):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        return DesktopGuest(folder.name, manager)

    def assert_plain(self, guest, words):
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        state = guest.state()
        self.assertEqual(state["gpu"], "on", "the GPU is still there for programs")
        self.assertEqual(state["desktop"], "plain")
        self.assertIn(words, state["desktop_reason"])
        self.assertFalse(guest.exists(SETTINGS))
        self.assertTrue(guest.said()[-1].startswith("nvfwd-desktop plain "), guest.said())
        return state

    def test_gdm_goes_on_the_gpu(self):
        guest = self.guest()
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(guest.state()["desktop"], "gpu")
        self.assertEqual(guest.called("modprobe nvidia-drm"), 1)
        settings = guest.read(SETTINGS)
        for line in ("GALLIUM_DRIVER=zink", "MESA_LOADER_DRIVER_OVERRIDE=zink", "__GLX_VENDOR_LIBRARY_NAME=dri3"):
            self.assertIn(line + "\n", settings)
        vendors = settings.split("__EGL_VENDOR_LIBRARY_FILENAMES=")[1].split("\n")[0].split(":")
        self.assertEqual(vendors[0], "/opt/nv/built/EGL_nvfwd_xwayland.json", "before Mesa's")
        self.assertTrue(vendors[1].endswith("50_mesa.json"))
        rules = guest.read(IGNORED)
        self.assertIn('KERNEL=="card0"', rules)
        self.assertNotIn("card1", rules)
        self.assertEqual(guest.said(), [f"nvfwd-gpu on {V}", "nvfwd-desktop gpu"])
        self.assertIn(f"built_for={V}-abc", guest.read("run/nvfwd/state"))

    def test_debians_gdm_counts_too(self):
        guest = self.guest("gdm3.service")
        guest.run()
        self.assertEqual(guest.state()["desktop"], "gpu")

    def test_another_display_manager_keeps_the_plain_screen(self):
        guest = self.guest("sddm.service")
        self.assert_plain(guest, "the display manager is sddm, and only GDM")
        self.assertEqual(guest.called("modprobe nvidia-drm"), 0)
        self.assertFalse(guest.exists(IGNORED))

    def test_someone_can_keep_it_plain(self):
        guest = self.guest()
        write(os.path.join(guest.root, "etc/nvfwd/own-install.conf"),
              "family=fedora\nlibraries=/usr/lib64\ndesktop=plain\n")
        self.assert_plain(guest, "desktop=plain in /etc/nvfwd/own-install.conf")

    def test_a_failure_is_remembered_for_its_driver_and_kernel_only(self):
        guest = self.guest()
        write(os.path.join(guest.root, "var/lib/nvfwd/desktop-failed"), f"{V}-abc 7.0.0-38-generic\n")
        self.assert_plain(guest, "Delete /var/lib/nvfwd/desktop-failed to try again")
        guest.set_image(V, f"{V}-def")
        guest.run()
        self.assertEqual(guest.state()["desktop"], "gpu")

    def test_nvidia_drm_failing_costs_only_the_desktop(self):
        guest = self.guest()
        guest.fake("modprobe", '[ "$1" != nvidia-drm ]')
        self.assert_plain(guest, "nvidia-drm would not load")

    def test_gdms_own_rules_are_left_out_before_the_module_loads(self):
        guest = self.guest()
        write(os.path.join(guest.root, "usr/lib/udev/rules.d/61-gdm.rules"), "RUN+=\"disable wayland\"\n")
        guest.run()
        self.assertEqual(guest.read("run/udev/rules.d/61-gdm.rules"), "")
        calls = guest.calls_in_order()
        self.assertLess(calls.index("udevadm control --reload"), calls.index("modprobe nvidia"))

    def test_an_admins_own_gdm_rules_win(self):
        guest = self.guest()
        write(os.path.join(guest.root, "usr/lib/udev/rules.d/61-gdm.rules"), "x\n")
        write(os.path.join(guest.root, "etc/udev/rules.d/61-gdm.rules"), "mine\n")
        guest.run()
        self.assertFalse(guest.exists("run/udev/rules.d/61-gdm.rules"))

    def test_gpu_off_takes_gdms_rules_back(self):
        guest = self.guest()
        write(os.path.join(guest.root, "usr/lib/udev/rules.d/61-gdm.rules"), "x\n")
        guest.script("driver-version.py", "echo 620.10")
        guest.run()
        self.assertEqual(guest.state()["gpu"], "off")
        self.assertFalse(guest.exists("run/udev/rules.d/61-gdm.rules"))
        self.assertFalse(guest.exists(SETTINGS))


class WatchKeepsADesktop(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.guest = DesktopGuest(folder.name)
        self.guest.run()

    def test_a_lit_screen_is_left_alone(self):
        write(os.path.join(self.guest.connector, "enabled"), "enabled\n")
        result = self.guest.watch(seconds=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.guest.state()["desktop"], "gpu")
        self.assertEqual(self.guest.called("systemctl"), 0)

    def test_a_screen_that_never_lights_boots_again_on_the_plain_screen(self):
        result = self.guest.watch(seconds=0)
        self.assertEqual(result.returncode, 0, result.stderr)
        state = self.guest.state()
        self.assertEqual(state["desktop"], "plain")
        self.assertIn("didn't come up on the GPU", state["desktop_reason"])
        self.assertEqual(self.guest.read("var/lib/nvfwd/desktop-failed").strip(), f"{V}-abc 7.0.0-38-generic")
        self.assertEqual(self.guest.calls_in_order()[-1], "systemctl reboot")
        self.assertTrue(self.guest.said()[-1].startswith("nvfwd-desktop plain the login screen didn't"))
        self.guest.run()
        self.assertEqual(self.guest.state()["desktop"], "plain", "the next boot starts there")

    def test_a_plain_boot_has_nothing_to_watch(self):
        self.guest.set_manager("sddm.service")
        self.guest.run()
        self.guest.watch(seconds=0)
        self.assertEqual(self.guest.called("systemctl"), 0)
        self.assertFalse(self.guest.exists("var/lib/nvfwd/desktop-failed"))


class SetUpForTheDesktop(unittest.TestCase):
    def guest(self, manager):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        guest = OwnGuest(folder.name)
        if manager:
            link = os.path.join(guest.root, "etc/systemd/system/display-manager.service")
            os.makedirs(os.path.dirname(link), exist_ok=True)
            os.symlink(f"/usr/lib/systemd/system/{manager}", link)
        return guest

    def test_gdm_waits_for_the_gpu_step(self):
        guest = self.guest("gdm.service")
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        drop_in = guest.read("etc/systemd/system/gdm.service.d/nvfwd.conf")
        self.assertIn("After=nvfwd-gpu.service", drop_in)
        self.assertNotIn("Requires=", drop_in)
        self.assertIn("ExecStart=/usr/lib/nvfwd/desktop-watch.sh",
                      guest.read("etc/systemd/system/nvfwd-desktop-watch.service"))
        self.assertIn("GDM and your desktop go on the GPU", result.stdout)

    def test_another_display_manager_is_left_as_it_is(self):
        guest = self.guest("sddm.service")
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(guest.exists("etc/systemd/system/sddm.service.d"))
        self.assertIn("Your display manager is sddm", result.stdout)

    def test_a_choice_of_the_plain_screen_survives_a_new_run(self):
        guest = self.guest("gdm.service")
        guest.run()
        with open(os.path.join(guest.root, "etc/nvfwd/own-install.conf"), "a") as f:
            f.write("desktop=plain\n")
        guest.run()
        self.assertIn("desktop=plain\n", guest.read("etc/nvfwd/own-install.conf"))


class CheckSaysWhereTheDesktopIs(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.guest = FakeGuest(folder.name)
        self.guest.file("etc/nvfwd/own-install.conf", "family=fedora\nlibraries=/usr/lib64\n")
        self.guest.file("var/lib/nvfwd/own-setup-done", "")

    def test_a_desktop_on_the_gpu_is_checked(self):
        self.guest.state(f"gpu=on\nversion={V}\ntested=yes\ndesktop=gpu\n")
        self.guest.file(SETTINGS, "GALLIUM_DRIVER=zink\n")
        self.guest.file("sys/class/drm/card1-HDMI-A-1/enabled", "enabled\n")
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stdout)
        for line in ("PASS the GPU screen is lit", "PASS gnome-shell runs on the GPU", "PASS module nvidia_drm"):
            self.assertIn(line, result.stdout)

    def test_a_dark_gpu_screen_fails(self):
        self.guest.state(f"gpu=on\nversion={V}\ntested=yes\ndesktop=gpu\n")
        self.guest.file(SETTINGS, "GALLIUM_DRIVER=zink\n")
        result = self.guest.run()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("FAIL the GPU screen is lit", result.stdout)

    def test_a_plain_desktop_says_why_and_passes(self):
        for module in ("nvidia_modeset", "nvidia_drm"):
            os.rmdir(os.path.join(self.guest.root, "sys/module", module))
        self.guest.state(f"gpu=on\nversion={V}\ntested=yes\ndesktop=plain\ndesktop_reason=the display manager is sddm\n")
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("desktop on the plain screen: the display manager is sddm", result.stdout)


if __name__ == "__main__":
    unittest.main()
