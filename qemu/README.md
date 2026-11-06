# The QEMU patch

Guesthouse needs some changes to QEMU. They're in `qemu-gpupv.patch`, on top of QEMU 11.1.3 (commit `005c61c0ef6f`).

For a Windows guest, QEMU knows nothing about any GPU. It offers the guest the devices and VMBus channels it expects on Hyper-V, and passes every packet to a separate daemon over a socket.

For a Linux guest the patch adds a device, `nvfwd`, in `hw/misc/nvfwd`. It forwards the calls of NVIDIA's Linux driver to the host's. `linux/README.md` says how that works.

```sh
git clone --depth 1 --branch v11.1.3 https://gitlab.com/qemu-project/qemu.git
cd qemu
git am /path/to/qemu-gpupv.patch
mkdir build && cd build
../configure --target-list=x86_64-softmmu --enable-kvm --enable-slirp --enable-dbus-display \
  --enable-pipewire --audio-drv-list=pipewire --enable-libusb \
  --disable-docs --disable-guest-agent --disable-tools --disable-werror --disable-gtk \
  --disable-sdl --disable-opengl --disable-virglrenderer --disable-spice --disable-curses \
  --disable-modules
ninja qemu-system-x86_64 hw/misc/nvfwd/nvfwd-sensord
cp hw/misc/nvfwd/nvfwd-sensord .
```

You'll need QEMU's usual build dependencies. A release comes with QEMU built, so this is only for building it yourself.

The patch is GPL-2.0-or-later, apart from the tables in `hw/misc/nvfwd`, which come from NVIDIA's open modules and keep NVIDIA's MIT notice.
