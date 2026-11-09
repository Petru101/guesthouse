#!/usr/bin/env python3
"""check.sh against a scratch root and fake commands: a guest where all works, and ways it does not.
What matters here is the verdict and its exit code, as a person or a script acts on those."""
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

GUEST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "guest")
V = "615.78.08"
PID = "4242"


class FakeGuest:
    def __init__(self, folder):
        self.root = os.path.join(folder, "root")
        self.bin = os.path.join(folder, "bin")
        os.makedirs(self.bin)
        self.scripts = os.path.join(folder, "scripts")
        shutil.copytree(GUEST, self.scripts)
        self.file("var/lib/nvfwd/first-boot-done", "")
        self.file("etc/modprobe.d/nvfwd.conf", "blacklist nvidia\noptions nvidia-drm modeset=1 fbdev=0\n")
        self.file("etc/systemd/logind.conf.d/90-nvfwd.conf", "[Login]\nPowerKeyIgnoreInhibited=yes\n")
        self.file("usr/bin/Xwayland.real", "", True)
        self.file("lib/modules/7.0/build/Makefile", "")
        self.file("opt/ff/firefox/firefox", "", True)
        for mask in ("pipewire.socket", "pipewire.service", "wireplumber.service"):
            os.makedirs(os.path.join(self.root, "etc/systemd/user"), exist_ok=True)
            os.symlink("/dev/null", os.path.join(self.root, "etc/systemd/user", mask))
        self.file(f"proc/{PID}/status", "Name: gnome-shell\nCapAmb:\t0000000000000000\n")
        self.file(f"proc/{PID}/maps", "libgallium libGLX_nvidia.so libVkLayer_zink_on_nvidia /dev/nvidia0\n")
        self.file("sys/class/drm/card1/device/device", "0x11ea\n")
        self.file("sys/class/drm/card1-HDMI-A-1/status", "connected\n")
        self.file("run/user/1000/nvfwd-session.env", "WAYLAND_DISPLAY=wayland-0\n")
        self.file("run/user/1000/.mutter-Xwaylandauth.abc", "")
        for module in ("nvidia", "nvidia_modeset", "nvidia_drm"):
            os.makedirs(os.path.join(self.root, "sys/module", module), exist_ok=True)
        self.file("sys/module/nvidia/version", V + "\n")
        self.file(f"opt/nv/driver/lib/libcuda.so.{V}", "")
        self.file("opt/nv/built/vkcompute", "#!/bin/sh\necho 'checked 16 values, 0 wrong'\n", True)
        self.file("opt/nv/built/cudavec", "#!/bin/sh\necho 'seed 1: checked 4 values, 0 wrong'\n", True)
        self.state(f"gpu=on\nversion={V}\ntested=yes\n")
        self.fake("dpkg-query", "echo 'install ok installed'")
        self.fake("systemctl", '[ "$1" = get-default ] && echo graphical.target; exit 0')
        self.fake("id", '[ "$1" = -u ] && echo 1000; exit 0')
        self.fake("uname", "echo 7.0")
        self.fake("ldconfig", "echo libcuda.so.1")
        self.fake("ldd", "echo fine")
        self.fake("nvidia-smi", "echo 'GPU 0: NVIDIA GeForce RTX 5090 (UUID: GPU-1)'")
        self.fake("vulkaninfo", "echo 'driverName = NVIDIA'")
        self.fake("pgrep", f"echo {PID}")
        self.fake("runuser", 'shift 3; exec "$@"')
        self.fake("gdbus", "echo \"(true, '1')\"")
        self.fake("glxinfo", "echo 'OpenGL renderer string: zink Vulkan 1.4'")
        self.script("driver-version.py", f"#!/bin/sh\necho {V}\n")

    def file(self, name, text, executable=False):
        path = os.path.join(self.root, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)
        if executable:
            os.chmod(path, os.stat(path).st_mode | stat.S_IEXEC)

    def state(self, text):
        self.file("run/nvfwd/state", text)

    def fake(self, name, body):
        path = os.path.join(self.bin, name)
        with open(path, "w") as f:
            f.write(f"#!/bin/bash\n{body}\n")
        os.chmod(path, 0o755)

    def script(self, name, text):
        path = os.path.join(self.scripts, name)
        with open(path, "w") as f:
            f.write(text)
        os.chmod(path, 0o755)

    def run(self, *arguments):
        environment = dict(os.environ, NVFWD_ROOT=self.root, PATH=f"{self.bin}:{os.environ['PATH']}")
        return subprocess.run(["bash", os.path.join(self.scripts, "check.sh"), *arguments], env=environment,
                              capture_output=True, text=True)


class Verdict(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.guest = FakeGuest(folder.name)

    def plain(self, reason="the host offered no driver image"):
        self.guest.state(f"gpu=off\nreason={reason}\n")
        self.guest.file(f"proc/{PID}/maps", "libgallium\n")

    def test_all_good(self):
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("PASS everything", result.stdout)
        self.assertNotIn("FAIL", result.stdout)

    def test_the_user_is_the_one_the_install_named(self):
        self.guest.file("etc/nvfwd/account.conf", "user=ana\n")
        self.guest.fake("id", 'echo "$@" >> "$NVFWD_ROOT/id-calls"; [ "$1" = -u ] && echo 1000; exit 0')
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("PASS user ana exists", result.stdout)
        with open(os.path.join(self.guest.root, "id-calls")) as f:
            self.assertIn("-u ana", f.read())

    def test_a_guest_from_before_the_setting_keeps_its_user(self):
        self.assertIn("PASS user desk exists", self.guest.run().stdout)

    def test_gpu_off_is_named_and_not_called_a_pass(self):
        self.plain()
        result = self.guest.run()
        self.assertEqual(result.returncode, 2, result.stdout)
        self.assertIn("the host offered no driver image", result.stdout)

    def test_gpu_off_when_a_test_expects_it(self):
        self.plain()
        self.assertEqual(self.guest.run("--gpu-off-expected").returncode, 0)

    def test_plain_desktop_on_nvidia_fails(self):
        self.guest.state("gpu=off\nreason=x\n")
        result = self.guest.run("--gpu-off-expected")
        self.assertEqual(result.returncode, 1)
        self.assertIn("FAIL plain desktop stays off", result.stdout)

    def test_a_wrong_value_from_the_gpu_fails(self):
        self.guest.file("opt/nv/built/cudavec", "#!/bin/sh\necho 'checked 4 values, 3 wrong'\n", True)
        result = self.guest.run()
        self.assertEqual(result.returncode, 1)
        self.assertIn("FAIL CUDA kernel", result.stdout)

    def test_the_host_answering_another_version_fails(self):
        self.guest.script("driver-version.py", "#!/bin/sh\necho 620.10\n")
        self.assertIn("FAIL the host's driver says", self.guest.run().stdout)

    def test_a_session_with_the_capability_left_fails(self):
        self.guest.file(f"proc/{PID}/status", "CapAmb:\t0000000800000000\n")
        self.assertIn("FAIL session holds no ambient", self.guest.run().stdout)

    def test_shell_on_software_fails_with_the_gpu_on(self):
        self.guest.file(f"proc/{PID}/maps", "libgallium\n")
        self.assertIn("FAIL gnome-shell has libGLX_nvidia", self.guest.run().stdout)

    def test_no_shell_at_all(self):
        self.guest.fake("pgrep", "exit 1")
        result = self.guest.run()
        self.assertEqual(result.returncode, 1)
        self.assertIn("FAIL gnome-shell running", result.stdout)

    def test_an_app_that_opens_no_window(self):
        self.guest.fake("gdbus", "echo \"(true, '0')\"")
        script = os.path.join(self.guest.scripts, "check.sh")
        with open(script) as f:
            text = f.read().replace("app_wait_seconds=30", "app_wait_seconds=1")
        with open(script, "w") as f:
            f.write(text)
        self.assertIn("FAIL firefox opens no window", self.guest.run().stdout)

    def test_no_sensors_is_a_warning_and_not_a_failure(self):
        self.guest.fake("nvidia-smi", """case "$*" in *temperature*) echo '[N/A]' ;; *) echo 'GPU 0: RTX' ;; esac""")
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("WARN no sensor readings", result.stdout)

    def test_sensors_that_read_say_nothing(self):
        self.guest.fake("nvidia-smi", """case "$*" in *temperature*) echo 42 ;; *) echo 'GPU 0: RTX' ;; esac""")
        self.assertNotIn("no sensor readings", self.guest.run().stdout)

    def test_an_untested_driver_is_warned_about(self):
        self.guest.state(f"gpu=on\nversion={V}\ntested=no\n")
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("WARN", result.stdout)


if __name__ == "__main__":
    unittest.main()
