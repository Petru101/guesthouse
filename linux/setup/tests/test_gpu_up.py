#!/usr/bin/env python3
"""gpu-up.sh and link-driver.sh in a scratch root, with every system command a fake. The rule under
test: whatever goes wrong, the script exits 0, the state says why, and no driver link is left."""
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

GUEST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "guest")
V = "615.78.08"
KERNEL = "7.0.0-38-generic"


def write(path, text, executable=False):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    if executable:
        os.chmod(path, os.stat(path).st_mode | stat.S_IEXEC)


class Guest:
    def __init__(self, folder):
        self.root = os.path.join(folder, "root")
        self.bin = os.path.join(folder, "bin")
        self.scripts = os.path.join(folder, "scripts")
        self.calls = os.path.join(folder, "calls")
        shutil.copytree(GUEST, self.scripts)
        os.makedirs(self.root)
        open(self.calls, "w").close()
        # Fake commands note their arguments. mount fills the driver folder as a real mount would.
        for name in ("modprobe", "ldconfig", "udevadm", "umount"):
            self.fake(name, "exit 0")
        self.fake("uname", f"echo {KERNEL}")
        self.fake("mountpoint", 'test -e "$NVFWD_ROOT/opt/nv/driver/manifest"')
        self.fake("blkid", "echo /dev/vdb")
        self.set_image(V)
        self.script("build-parts.sh", 'echo built >> "$CALLS"; mkdir -p "$NVFWD_ROOT/lib/modules/' + KERNEL + '/updates/nvfwd"')
        self.script("driver-version.py", f"echo {V}")
        for program in ("cudavec", "vkcompute"):
            self.program(program, "echo checked 16 values, 0 wrong")

    def fake(self, name, body):
        write(os.path.join(self.bin, name), f'#!/bin/bash\necho "{name} $*" >> "$CALLS"\n{body}\n', True)

    def script(self, name, body):
        write(os.path.join(self.scripts, name), f"#!/bin/bash\n{body}\n", True)

    def program(self, name, body):
        write(os.path.join(self.root, "opt/nv/built", name), f"#!/bin/bash\n{body}\n", True)

    def set_image(self, version, image_id=None):
        image_id = image_id or f"{version}-abc"
        self.fake("mount", f"""d=$NVFWD_ROOT/opt/nv/driver; mkdir -p $d/lib $d/bin $d/json/egl_external_platform.d
printf 'version={version}\\nid={image_id}\\ntested=yes\\n' > $d/manifest; touch $d/sources.tar
touch $d/json/egl_external_platform.d/10_nvidia_wayland.json""")

    def add_device(self, device="0x11ea"):
        folder = os.path.join(self.root, "sys/bus/pci/devices/0000:00:02.0")
        write(os.path.join(folder, "vendor"), "0x1234\n")
        write(os.path.join(folder, "device"), device + "\n")

    def add_layers(self):
        write(os.path.join(self.root, "opt/nv/layer/VkLayer_zink_on_nvidia.json"), "{}")

    def run(self):
        environment = dict(os.environ, NVFWD_ROOT=self.root, CALLS=self.calls, PATH=f"{self.bin}:{os.environ['PATH']}")
        return subprocess.run(["bash", os.path.join(self.scripts, "gpu-up.sh")], env=environment,
                              capture_output=True, text=True)

    def state(self):
        with open(os.path.join(self.root, "run/nvfwd/state")) as f:
            return dict(line.strip().split("=", 1) for line in f)

    def called(self, text):
        with open(self.calls) as f:
            return sum(text in line for line in f)

    def links(self):
        found = []
        for folder, _, files in os.walk(os.path.join(self.root, "usr")):
            found += [name for name in files if os.path.islink(os.path.join(folder, name))]
        return found


class GpuUp(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.guest = Guest(folder.name)

    def assert_off(self, words):
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        state = self.guest.state()
        self.assertEqual(state["gpu"], "off")
        self.assertIn(words, state["reason"])
        self.assertEqual(self.guest.links(), [])
        return state

    def test_no_device(self):
        self.assert_off("no nvfwd device")
        self.assertEqual(self.guest.called("modprobe nvidia"), 0)

    def test_the_hosts_own_reason_reaches_the_guest(self):
        write(os.path.join(self.guest.root, "sys/firmware/qemu_fw_cfg/by_name/opt/nvfwd/gpu-off-reason/raw"),
              "REFUSED: branch 620 is unknown.\nThe VM will start without a GPU.\n")
        self.assert_off("branch 620 is unknown")

    def test_someone_elses_pci_device_is_not_ours(self):
        self.guest.add_device("0x1111")
        self.assert_off("no nvfwd device")

    def test_no_image(self):
        self.guest.add_device()
        self.guest.fake("blkid", "exit 2")
        self.assert_off("no driver image")

    def test_image_with_no_manifest(self):
        self.guest.add_device()
        self.guest.fake("mount", 'mkdir -p "$NVFWD_ROOT/opt/nv/driver"; : > "$NVFWD_ROOT/opt/nv/driver/manifest"')
        self.assert_off("no usable manifest")

    def test_build_failure(self):
        self.guest.add_device()
        self.guest.script("build-parts.sh", "echo no headers; exit 1")
        self.assert_off("build for driver")
        with open(os.path.join(self.guest.root, "var/log/nvfwd/build.log")) as f:
            self.assertIn("no headers", f.read())
        self.assertEqual(self.guest.called("modprobe nvidia"), 0)

    def test_module_will_not_load(self):
        self.guest.add_device()
        self.guest.fake("modprobe", "exit 1")
        self.assert_off("would not load")

    def test_a_cuda_that_fails_turns_the_gpu_off_and_says_how(self):
        self.guest.add_device()
        self.guest.program("cudavec", "echo 'cuInit(0) = 100'; exit 1")
        state = self.assert_off("CUDA failed its first check")
        self.assertIn("cuInit(0) = 100", state["reason"])
        self.assertIn("calls.log", state["reason"])
        self.assertEqual(self.guest.called("modprobe nvidia-drm"), 0)

    def test_wrong_values_count_as_a_failure(self):
        self.guest.add_device()
        self.guest.program("vkcompute", "echo checked 16 values, 3 wrong")
        self.assert_off("Vulkan failed its first check")
        self.assertEqual(self.guest.called("modprobe nvidia-drm"), 0)

    def test_a_check_that_hangs_turns_the_gpu_off(self):
        self.guest.add_device()
        self.guest.fake("timeout", "exit 124")
        self.assert_off("first check")

    def test_host_runs_another_driver_than_the_image_holds(self):
        self.guest.add_device()
        self.guest.script("driver-version.py", "echo 620.10")
        state = self.assert_off("the host has 620.10 loaded")
        self.assertIn(V, state["reason"])
        self.assertEqual(self.guest.called("modprobe nvidia-drm"), 0)

    def test_host_gives_no_answer(self):
        self.guest.add_device()
        self.guest.script("driver-version.py", "echo nope >&2; exit 1")
        self.assert_off("did not answer: nope")

    def test_nvidia_drm_will_not_load(self):
        self.guest.add_device()
        self.guest.fake("modprobe", '[ "$1" != nvidia-drm ]')
        self.assert_off("nvidia-drm would not load")

    def test_all_good(self):
        self.guest.add_device()
        self.guest.add_layers()
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.guest.state()["gpu"], "on")
        self.assertEqual(self.guest.state()["version"], V)
        links = self.guest.links()
        for name in ("nvidia-smi", "nvidia-drm_gbm.so", "10_nvidia.json", "nvidia_icd.x86_64.json",
                     "10_nvidia_wayland.json", "VkLayer_zink_on_nvidia.json", "libGLX_dri3.so.0"):
            self.assertIn(name, links)
        with open(os.path.join(self.guest.root, "etc/ld.so.conf.d/00-nvfwd.conf")) as f:
            self.assertIn("/opt/nv/driver/lib", f.read())

    def test_rebuilds_only_when_something_changed(self):
        self.guest.add_device()
        self.guest.add_layers()
        self.guest.run()
        self.guest.run()
        self.assertEqual(self.guest.called("built"), 1)
        self.guest.set_image("620.10")
        self.guest.script("driver-version.py", "echo 620.10")
        self.guest.run()
        self.assertEqual(self.guest.called("built"), 2)
        self.assertEqual(self.guest.state()["version"], "620.10")

    def test_a_new_kernel_rebuilds(self):
        self.guest.add_device()
        self.guest.add_layers()
        self.guest.run()
        self.guest.fake("uname", "echo 7.0.0-40-generic")
        self.guest.script("build-parts.sh", 'echo built >> "$CALLS"; mkdir -p "$NVFWD_ROOT/lib/modules/7.0.0-40-generic/updates/nvfwd"')
        self.guest.run()
        self.assertEqual(self.guest.called("built"), 2)

    def test_new_sources_rebuild_the_same_driver(self):
        self.guest.add_device()
        self.guest.add_layers()
        self.guest.run()
        self.guest.set_image(V, f"{V}-def")
        self.guest.run()
        self.assertEqual(self.guest.called("built"), 2)

    def test_a_boot_without_the_gpu_clears_the_last_one(self):
        self.guest.add_device()
        self.guest.add_layers()
        self.guest.run()
        self.assertNotEqual(self.guest.links(), [])
        shutil.rmtree(os.path.join(self.guest.root, "sys"))
        self.assert_off("no nvfwd device")
        self.assertFalse(os.path.exists(os.path.join(self.guest.root, "etc/ld.so.conf.d/00-nvfwd.conf")))

    def test_the_plain_card_is_hidden_and_ours_is_not(self):
        self.guest.add_device()
        self.guest.add_layers()
        write(os.path.join(self.guest.root, "sys/class/drm/card0/device/device"), "0x1111\n")
        write(os.path.join(self.guest.root, "sys/class/drm/card1/device/device"), "0x11ea\n")
        self.guest.run()
        with open(os.path.join(self.guest.root, "run/udev/rules.d/71-nvfwd-ignored-cards.rules")) as f:
            rules = f.read()
        self.assertIn('KERNEL=="card0"', rules)
        self.assertNotIn("card1", rules)

    def test_connectors_are_probed(self):
        self.guest.add_device()
        self.guest.add_layers()
        status = os.path.join(self.guest.root, "sys/class/drm/card1-HDMI-A-1/status")
        write(status, "")
        self.guest.run()
        with open(status) as f:
            self.assertEqual(f.read().strip(), "detect")


if __name__ == "__main__":
    unittest.main()
