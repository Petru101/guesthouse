#!/usr/bin/env python3
"""A guest someone installed by hand: which distribution it is, what its package manager is asked for,
setup-own.sh in a scratch root with fake commands, and gpu-up.sh, kernel-hook.sh and check.sh in the
mode setup-own.sh leaves behind."""
import os
import shutil
import subprocess
import tempfile
import unittest

from test_check import FakeGuest
from test_gpu_up import Guest, write

GUEST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "guest")
V = "615.78.08"
KERNEL = "6.17.4-300.fc45.x86_64"

# What each distribution's /etc/os-release says, cut to the lines that matter.
OS_RELEASES = {
    "fedora": 'NAME="Fedora Linux"\nID=fedora\nPRETTY_NAME="Fedora Linux 45 (Workstation Edition)"\n',
    "rocky": 'ID="rocky"\nID_LIKE="rhel centos fedora"\nPRETTY_NAME="Rocky Linux 10.1"\n',
    "debian": 'PRETTY_NAME="Debian GNU/Linux 13 (trixie)"\nID=debian\n',
    "ubuntu": 'PRETTY_NAME="Ubuntu 26.04 LTS"\nID=ubuntu\nID_LIKE=debian\n',
    "mint": 'NAME="Linux Mint"\nID=linuxmint\nID_LIKE="ubuntu debian"\n',
    "pop": 'NAME="Pop!_OS"\nID=pop\nID_LIKE="ubuntu debian"\n',
    "arch": 'NAME="Arch Linux"\nID=arch\nPRETTY_NAME="Arch Linux"\n',
    "endeavouros": 'NAME="EndeavourOS"\nID="endeavouros"\nID_LIKE="arch"\n',
    "cachyos": 'NAME="CachyOS Linux"\nID=cachyos\nID_LIKE=arch\n',
    "tumbleweed": 'NAME="openSUSE Tumbleweed"\nID="opensuse-tumbleweed"\nID_LIKE="opensuse suse"\n',
    "leap": 'NAME="openSUSE Leap"\nID="opensuse-leap"\nID_LIKE="suse opensuse"\n',
    "alpine": 'NAME="Alpine Linux"\nID=alpine\n',
    "void": 'NAME="Void"\nID="void"\n',
    "gentoo": 'NAME=Gentoo\nID=gentoo\n',
    "nixos": 'NAME=NixOS\nID=nixos\n',
}
FAMILIES = {
    "fedora": "fedora", "rocky": "fedora", "debian": "debian", "ubuntu": "debian", "mint": "debian",
    "pop": "debian", "arch": "arch", "endeavouros": "arch", "cachyos": "arch", "tumbleweed": "suse",
    "leap": "suse", "alpine": None, "void": None, "gentoo": None, "nixos": None,
}
MANAGERS = {"fedora": "dnf", "debian": "apt-get", "arch": "pacman", "suse": "zypper"}


def distro(root, call):
    """Runs one of distro.sh's functions, with NVFWD_ROOT at root."""
    script = f'. "{GUEST}/common.sh"; . "{GUEST}/distro.sh"; {call}'
    result = subprocess.run(["bash", "-c", script], env=dict(os.environ, NVFWD_ROOT=root),
                            capture_output=True, text=True)
    return result.returncode, result.stdout.strip()


class Detection(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.root = folder.name

    def test_each_distribution_gets_its_family_or_none(self):
        for name, text in OS_RELEASES.items():
            write(os.path.join(self.root, name), text)
            code, family = distro(self.root, f'distro_family "{self.root}/{name}"')
            if FAMILIES[name] is None:
                self.assertNotEqual(code, 0, name)
                self.assertEqual(family, "", name)
            else:
                self.assertEqual((code, family), (0, FAMILIES[name]), name)

    def test_each_family_has_its_manager_and_library_folder(self):
        folders = {"fedora": "/usr/lib64", "suse": "/usr/lib64", "arch": "/usr/lib",
                   "debian": "/usr/lib/x86_64-linux-gnu"}
        for family, manager in MANAGERS.items():
            self.assertEqual(distro(self.root, f"package_manager {family}")[1], manager)
            self.assertEqual(distro(self.root, f"libraries_folder {family}")[1], folders[family])

    def test_headers_are_for_the_running_kernel(self):
        self.assertEqual(distro(self.root, f"headers_package fedora {KERNEL}")[1], f"kernel-devel-{KERNEL}")
        self.assertEqual(distro(self.root, "headers_package debian 6.12.43+deb13-amd64")[1],
                         "linux-headers-6.12.43+deb13-amd64")
        self.assertEqual(distro(self.root, "headers_package suse 6.17.1-1-default")[1], "kernel-default-devel")

    def test_arch_headers_follow_the_kernel_package(self):
        self.assertEqual(distro(self.root, "headers_package arch 6.17.4-arch1-1")[1], "linux-headers")
        write(os.path.join(self.root, "usr/lib/modules/6.12.50-1-lts/pkgbase"), "linux-lts\n")
        self.assertEqual(distro(self.root, "headers_package arch 6.12.50-1-lts")[1], "linux-lts-headers")

    def test_debian_and_ubuntu_keep_headers_coming_with_new_kernels(self):
        for kernel, flavour in (("6.12.43+deb13-amd64", "amd64"), ("6.12.43+deb13-cloud-amd64", "cloud-amd64"),
                                ("6.1.0-25-amd64", "amd64"), ("7.0.0-38-generic", "generic")):
            self.assertEqual(distro(self.root, f"headers_follower debian {kernel}")[1],
                             f"linux-headers-{flavour}", kernel)
        for family in ("fedora", "arch", "suse"):
            self.assertEqual(distro(self.root, f"headers_follower {family} {KERNEL}")[1], "")

    def test_each_family_has_a_hook_or_says_it_builds_at_boot(self):
        hooks = {family: distro(self.root, f"hook_kind {family}")[1] for family in MANAGERS}
        self.assertEqual(hooks, {"fedora": "kernel-install", "debian": "kernel-postinst", "arch": "pacman",
                                 "suse": "none"})


class PackageLists(unittest.TestCase):
    def names(self, family):
        with open(os.path.join(GUEST, "files", f"own-packages-{family}.txt")) as f:
            return [line.strip() for line in f if line.strip() and not line.startswith("#")]

    def test_every_family_has_a_list_with_no_repeats(self):
        for family in MANAGERS:
            names = self.names(family)
            self.assertGreater(len(names), 10, family)
            self.assertEqual(len(names), len(set(names)), family)

    def test_each_list_has_the_compiler_the_shader_tool_and_python(self):
        needed = {
            "fedora": ("gcc", "make", "glslang", "vulkan-loader-devel", "pipewire-devel", "python3"),
            "debian": ("build-essential", "glslang-tools", "libvulkan-dev", "libpipewire-0.3-dev", "python3"),
            "arch": ("base-devel", "glslang", "vulkan-headers", "libpipewire", "python"),
            "suse": ("gcc", "make", "glslang-devel", "vulkan-devel", "pipewire-devel", "python3"),
        }
        for family, names in needed.items():
            for name in names:
                self.assertIn(name, self.names(family), family)

    def test_no_list_names_a_kernel(self):
        for family in MANAGERS:
            for name in self.names(family):
                self.assertFalse(name.startswith(("kernel", "linux-headers", "linux-image")), name)


class OwnGuest:
    """A scratch root with a driver image, and a fake for every command setup-own.sh asks."""

    def __init__(self, folder, release="fedora", kernel=KERNEL):
        self.root = os.path.join(folder, "root")
        self.bin = os.path.join(folder, "bin")
        self.calls = os.path.join(folder, "calls")
        self.image = os.path.join(folder, "image")
        shutil.copytree(GUEST, os.path.join(self.image, "setup"))
        write(os.path.join(self.image, "manifest"), f"version={V}\nid={V}-abc\ntested=yes\n")
        write(os.path.join(self.image, "sources.tar"), "")
        write(os.path.join(self.image, "setup/build-parts.sh"),
              '#!/bin/bash\necho "build-parts $*" >> "$CALLS"\nmkdir -p "$NVFWD_ROOT/lib/modules/$3/updates/nvfwd"\n',
              True)
        write(os.path.join(self.root, "etc/os-release"), OS_RELEASES[release])
        os.makedirs(os.path.join(self.root, "run/systemd/system"))
        open(self.calls, "w").close()
        self.fake("id", "echo 0")
        self.kernel = kernel
        self.fake("uname", f'[ "$1" = -m ] && echo x86_64 || echo {kernel}')
        for name in ("systemctl", "semanage", "restorecon"):
            self.fake(name, "exit 0")
        # Each package manager brings the running kernel's headers, as a real one would.
        for name in MANAGERS.values():
            self.fake(name, self.bring_headers())

    def bring_headers(self):
        folder = f"$NVFWD_ROOT/lib/modules/{self.kernel}/build"
        return f'mkdir -p "{folder}"; touch "{folder}/Makefile"'

    def fake(self, name, body):
        write(os.path.join(self.bin, name), f'#!/bin/bash\necho "{name} $*" >> "$CALLS"\n{body}\n', True)

    def run(self):
        environment = dict(os.environ, NVFWD_ROOT=self.root, CALLS=self.calls,
                           PATH=f"{self.bin}:/usr/bin:/bin")
        return subprocess.run(["bash", os.path.join(self.image, "setup/setup-own.sh")], env=environment,
                              capture_output=True, text=True)

    def lines(self, start):
        with open(self.calls) as f:
            return [line.strip() for line in f if line.startswith(start)]

    def exists(self, name):
        return os.path.lexists(os.path.join(self.root, name))

    def read(self, name):
        with open(os.path.join(self.root, name)) as f:
            return f.read()


class SetUp(unittest.TestCase):
    def guest(self, release="fedora", kernel=KERNEL):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        return OwnGuest(folder.name, release, kernel)

    def assert_refused(self, guest, words):
        result = guest.run()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("Not set up: ", result.stderr)
        self.assertIn(words, result.stderr)
        self.assertFalse(guest.exists("var/lib/nvfwd/own-setup-done"))
        return result

    def test_an_unknown_distribution_is_refused_by_name(self):
        guest = self.guest("alpine")
        result = self.assert_refused(guest, "Alpine Linux isn't one this knows")
        self.assertIn("Fedora, Debian, Ubuntu, Arch or openSUSE", result.stderr)
        self.assertEqual(guest.lines("apk"), [])

    def test_it_needs_root(self):
        guest = self.guest()
        guest.fake("id", "echo 1000")
        self.assert_refused(guest, "with sudo")

    def test_it_needs_systemd(self):
        guest = self.guest()
        os.rmdir(os.path.join(guest.root, "run/systemd/system"))
        self.assert_refused(guest, "systemd")

    def test_an_image_based_system_is_refused(self):
        guest = self.guest()
        write(os.path.join(guest.root, "run/ostree-booted"), "")
        self.assert_refused(guest, "ostree")

    def test_secure_boot_is_refused_and_off_is_not(self):
        guest = self.guest()
        variable = os.path.join(guest.root,
                                "sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c")
        os.makedirs(os.path.dirname(variable))
        with open(variable, "wb") as f:
            f.write(b"\x06\x00\x00\x00\x01")
        self.assert_refused(guest, "Secure Boot")
        with open(variable, "wb") as f:
            f.write(b"\x06\x00\x00\x00\x00")
        self.assertEqual(guest.run().returncode, 0)

    def test_it_runs_only_from_the_image(self):
        guest = self.guest()
        os.remove(os.path.join(guest.image, "manifest"))
        self.assert_refused(guest, "LABEL=NVFWD_DRIVER")

    def test_a_manager_that_is_missing_is_named(self):
        guest = self.guest("debian")
        os.remove(os.path.join(guest.bin, "apt-get"))
        self.assert_refused(guest, "has no apt-get")

    def test_no_headers_for_the_running_kernel_says_to_reboot(self):
        guest = self.guest("arch")
        guest.fake("pacman", "exit 0")
        self.assert_refused(guest, "reboot and run this again")

    def test_a_failed_install_stops_it(self):
        guest = self.guest()
        guest.fake("dnf", "exit 1")
        self.assert_refused(guest, "dnf could not install")

    def test_a_failed_build_shows_its_log(self):
        guest = self.guest()
        write(os.path.join(guest.image, "setup/build-parts.sh"), "#!/bin/bash\necho 'nvidia.c: error'; exit 2\n", True)
        result = self.assert_refused(guest, "the build failed")
        self.assertIn("nvidia.c: error", result.stderr)

    def test_fedora(self):
        guest = self.guest("fedora")
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        [install] = guest.lines("dnf install -y")
        self.assertIn(f"kernel-devel-{KERNEL}", install)
        self.assertIn("glslang", install)
        self.assertNotIn("policycoreutils", install, "no SELinux in this root")
        self.assertIn("kernel-install", result.stdout)
        self.assertTrue(os.access(os.path.join(guest.root, "etc/kernel/install.d/90-nvfwd.install"), os.X_OK))
        self.assertEqual(guest.read("etc/nvfwd/own-install.conf"), "family=fedora\nlibraries=/usr/lib64\n")
        self.assertEqual(guest.lines("build-parts"), [f"build-parts {guest.image}/sources.tar {V} {KERNEL}"])
        self.assertEqual(guest.read(f"lib/modules/{KERNEL}/updates/nvfwd/built-for").strip(), f"{V}-abc {KERNEL}")
        self.assertIn("systemctl enable nvfwd-gpu.service nvfwd-desktop-watch.service", guest.lines("systemctl"))
        self.assertIn("ExecStart=/usr/lib/nvfwd/gpu-up.sh", guest.read("etc/systemd/system/nvfwd-gpu.service"))
        self.assertIn("blacklist nvidia", guest.read("etc/modprobe.d/nvfwd.conf"))
        self.assertIn("PowerKeyIgnoreInhibited=yes", guest.read("etc/systemd/logind.conf.d/90-nvfwd.conf"))
        self.assertNotIn("drm", guest.read("etc/udev/rules.d/70-nvfwd.rules").split("\n", 2)[-1])
        self.assertEqual(os.readlink(os.path.join(guest.root, "usr/local/sbin/nvfwd-check")), "/usr/lib/nvfwd/check.sh")
        self.assertTrue(os.access(os.path.join(guest.root, "usr/lib/nvfwd/gpu-up.sh"), os.X_OK))
        self.assertTrue(guest.exists("var/lib/nvfwd/own-setup-done"))
        self.assertIn("Reboot", result.stdout)
        for left_alone in ("etc/default/grub.d", "usr/bin/Xwayland.real", "etc/systemd/user",
                           "etc/systemd/system/nvfwd-desktop.service"):
            self.assertFalse(guest.exists(left_alone), left_alone)

    def test_fedora_with_selinux_labels_the_scripts_and_never_turns_it_off(self):
        guest = self.guest("fedora")
        write(os.path.join(guest.root, "sys/fs/selinux/enforce"), "1\n")
        guest.fake("setenforce", "exit 0")
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("policycoreutils-python-utils", guest.lines("dnf install -y")[0])
        self.assertEqual(guest.lines("semanage"), ["semanage fcontext -a -t bin_t /usr/lib/nvfwd(/.*)?"])
        self.assertIn(os.path.join(guest.root, "usr/lib/nvfwd"), guest.lines("restorecon")[0])
        self.assertEqual(guest.lines("setenforce"), [])
        self.assertIn("SELinux is on", result.stdout)

    def test_debian_refreshes_and_keeps_headers_coming(self):
        guest = self.guest("debian", "6.12.43+deb13-amd64")
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        installs = guest.lines("apt-get")
        self.assertEqual(installs[0], "apt-get update")
        self.assertIn("linux-headers-6.12.43+deb13-amd64", installs[1])
        self.assertIn("--no-install-recommends", installs[1])
        self.assertTrue(installs[2].endswith(" linux-headers-amd64"), installs[2])
        for hook in ("etc/kernel/postinst.d/nvfwd", "etc/kernel/header_postinst.d/nvfwd"):
            self.assertTrue(os.access(os.path.join(guest.root, hook), os.X_OK), hook)
        self.assertIn("/usr/lib/x86_64-linux-gnu", guest.read("etc/nvfwd/own-install.conf"))

    def test_a_missing_follower_is_said_and_not_fatal(self):
        guest = self.guest("ubuntu", "7.0.0-38-generic")
        guest.fake("apt-get", f'[ "$4" = linux-headers-generic ] && exit 100; {guest.bring_headers()}')
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("install the headers by hand", result.stdout)

    def test_arch_gets_a_pacman_hook_and_no_sync(self):
        guest = self.guest("endeavouros", "6.17.4-arch1-1")
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        [install] = guest.lines("pacman")
        self.assertTrue(install.startswith("pacman -S --needed --noconfirm "), install)
        self.assertIn("linux-headers", install)
        self.assertIn("Target = usr/lib/modules/*/build/Makefile", guest.read("etc/pacman.d/hooks/90-nvfwd.hook"))
        self.assertIn("libraries=/usr/lib\n", guest.read("etc/nvfwd/own-install.conf"))

    def test_opensuse_builds_at_boot_and_says_so(self):
        guest = self.guest("tumbleweed", "6.17.1-1-default")
        result = guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(guest.lines("zypper")[0], "zypper --non-interactive refresh")
        self.assertTrue(guest.lines("zypper")[1].endswith(" kernel-default-devel"), guest.lines("zypper")[1])
        self.assertIn("at its first boot", result.stdout)
        self.assertFalse(guest.exists("etc/kernel"))

    def test_a_second_run_takes_the_new_scripts(self):
        guest = self.guest()
        self.assertEqual(guest.run().returncode, 0)
        write(os.path.join(guest.root, "usr/lib/nvfwd/stale.sh"), "")
        self.assertEqual(guest.run().returncode, 0)
        self.assertFalse(guest.exists("usr/lib/nvfwd/stale.sh"))
        self.assertEqual(len(guest.lines("build-parts")), 2)


class OwnGpuUp(unittest.TestCase):
    """gpu-up.sh after setup-own.sh: the same checks, then the GPU is for programs."""

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.guest = Guest(folder.name)
        write(os.path.join(self.guest.root, "etc/nvfwd/own-install.conf"), "family=fedora\nlibraries=/usr/lib64\n")
        self.serial = os.path.join(self.guest.root, "dev/ttyS0")
        write(self.serial, "")
        self.guest.add_device()
        self.guest.add_layers()

    def said(self):
        with open(self.serial) as f:
            return f.read()

    def test_with_no_display_manager_the_gpu_is_for_programs(self):
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.guest.state()["gpu"], "on")
        self.assertEqual(self.guest.state()["desktop"], "plain")
        self.assertEqual(self.guest.called("modprobe nvidia-drm"), 0)
        self.assertEqual(self.guest.called("udevadm trigger"), 0)
        self.assertEqual(self.said(), f"nvfwd-gpu on {V}\nnvfwd-desktop plain no display manager is enabled\n")

    def test_libraries_go_in_the_familys_folder(self):
        self.guest.run()
        self.assertTrue(os.path.islink(os.path.join(self.guest.root, "usr/lib64/gbm/nvidia-drm_gbm.so")))
        self.assertTrue(os.path.islink(os.path.join(self.guest.root, "usr/lib64/libGLX_dri3.so.0")))
        self.assertFalse(os.path.exists(os.path.join(self.guest.root, "usr/lib/x86_64-linux-gnu")))

    def test_the_host_hears_why_the_gpu_is_off(self):
        self.guest.script("driver-version.py", "echo 620.10")
        self.guest.run()
        self.assertTrue(self.said().startswith("nvfwd-gpu off the image holds driver"), self.said())

    def test_selinux_mounts_the_image_as_libraries(self):
        write(os.path.join(self.guest.root, "sys/fs/selinux/enforce"), "1\n")
        self.guest.run()
        self.assertEqual(self.guest.called("context=system_u:object_r:lib_t:s0"), 1)


class UbuntuStaysAsItWas(unittest.TestCase):
    def test_no_own_install_means_the_desktop_steps_and_no_selinux_label(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        guest = Guest(folder.name)
        guest.add_device()
        guest.add_layers()
        guest.run()
        self.assertEqual(guest.called("modprobe nvidia-drm"), 1)
        self.assertEqual(guest.called("mount -t iso9660 -o ro,nodev,nosuid /dev/vdb"), 1)
        self.assertTrue(os.path.islink(os.path.join(guest.root, "usr/lib/x86_64-linux-gnu/libGLX_dri3.so.0")))


class KernelHook(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.guest = Guest(folder.name)
        write(os.path.join(self.guest.root, "etc/nvfwd/own-install.conf"), "family=arch\nlibraries=/usr/lib\n")
        self.guest.script("build-parts.sh",
                          'echo "built $3" >> "$CALLS"; mkdir -p "$NVFWD_ROOT/lib/modules/$3/updates/nvfwd"')

    def run_hook(self, *arguments, given=""):
        environment = dict(os.environ, NVFWD_ROOT=self.guest.root, CALLS=self.guest.calls,
                           PATH=f"{self.guest.bin}:{os.environ['PATH']}")
        return subprocess.run(["bash", os.path.join(self.guest.scripts, "kernel-hook.sh"), *arguments],
                              env=environment, input=given, capture_output=True, text=True)

    def mount_image(self):
        write(os.path.join(self.guest.root, "opt/nv/driver/manifest"), f"version={V}\nid={V}-abc\n")

    def headers(self, kernel):
        write(os.path.join(self.guest.root, f"lib/modules/{kernel}/build/Makefile"), "")

    def test_it_builds_for_the_new_kernel_once(self):
        self.mount_image()
        self.headers("6.18.1-arch1-1")
        for _ in range(2):
            result = self.run_hook("6.18.1-arch1-1")
            self.assertEqual(result.returncode, 0)
        self.assertEqual(self.guest.called("built 6.18.1-arch1-1"), 1)

    def test_with_no_image_or_no_headers_it_leaves_it_to_the_boot(self):
        result = self.run_hook("6.18.1-arch1-1")
        self.assertEqual(result.returncode, 0)
        self.assertIn("next boot", result.stdout)
        self.mount_image()
        result = self.run_hook("6.18.1-arch1-1")
        self.assertIn("no headers", result.stdout)
        self.assertEqual(self.guest.called("built"), 0)

    def test_a_failed_build_never_fails_the_package_manager(self):
        self.mount_image()
        self.headers("6.18.1-arch1-1")
        self.guest.script("build-parts.sh", "echo broken; exit 2")
        result = self.run_hook("6.18.1-arch1-1")
        self.assertEqual(result.returncode, 0)
        self.assertIn("failed", result.stdout)

    def test_pacman_paths_name_the_kernels(self):
        self.mount_image()
        for kernel in ("6.18.1-arch1-1", "6.12.60-1-lts"):
            self.headers(kernel)
        given = "usr/lib/modules/6.18.1-arch1-1/build/Makefile\nusr/lib/modules/6.12.60-1-lts/build/Makefile\n"
        self.run_hook("--paths", given=given)
        self.assertEqual(self.guest.called("built 6.18.1-arch1-1"), 1)
        self.assertEqual(self.guest.called("built 6.12.60-1-lts"), 1)

    def test_the_hooks_call_it_as_each_manager_does(self):
        for name in ("90-nvfwd.install", "kernel-postinst"):
            with open(os.path.join(GUEST, "files/hooks", name)) as f:
                self.assertIn("exec /usr/lib/nvfwd/kernel-hook.sh", f.read())
            self.assertTrue(os.access(os.path.join(GUEST, "files/hooks", name), os.X_OK), name)
        with open(os.path.join(GUEST, "files/hooks/90-nvfwd.hook")) as f:
            text = f.read()
        self.assertIn("kernel-hook.sh --paths", text)
        self.assertIn("NeedsTargets", text)


class OwnCheck(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.guest = FakeGuest(folder.name)
        self.guest.file("etc/nvfwd/own-install.conf", "family=debian\nlibraries=/usr/lib/x86_64-linux-gnu\n")
        self.guest.file("var/lib/nvfwd/own-setup-done", "")
        os.remove(os.path.join(self.guest.root, "var/lib/nvfwd/first-boot-done"))
        for module in ("nvidia_modeset", "nvidia_drm"):
            os.rmdir(os.path.join(self.guest.root, "sys/module", module))
        self.guest.fake("pgrep", "exit 1")

    def test_a_working_gpu_passes_with_no_desktop_of_ours(self):
        result = self.guest.run()
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("PASS everything", result.stdout)
        self.assertNotIn("gnome-shell", result.stdout)
        self.assertNotIn("first boot", result.stdout)

    def test_gpu_off_is_its_own_verdict(self):
        self.guest.state("gpu=off\nreason=no nvfwd device\n")
        result = self.guest.run()
        self.assertEqual(result.returncode, 2, result.stdout)
        self.assertIn("GPU is off: no nvfwd device", result.stdout)

    def test_selinux_wants_the_scripts_labelled_as_programs(self):
        self.guest.file("sys/fs/selinux/enforce", "1\n")
        self.guest.fake("stat", "echo system_u:object_r:lib_t:s0")
        result = self.guest.run()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("lib_t, not a program", result.stdout)


if __name__ == "__main__":
    unittest.main()
