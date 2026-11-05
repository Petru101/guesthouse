# The QEMU patch

Guesthouse needs a few small changes to QEMU. They're in `qemu-gpupv.patch`, on top of QEMU 11.1.3 (commit `005c61c0ef6f`).

QEMU itself knows nothing about any GPU. It offers the guest the devices and VMBus channels it expects on Hyper-V, and passes every packet to a separate daemon over a socket.

```sh
git clone --depth 1 --branch v11.1.3 https://gitlab.com/qemu-project/qemu.git
cd qemu
git am /path/to/qemu-gpupv.patch
mkdir build && cd build
../configure --target-list=x86_64-softmmu --enable-kvm --enable-slirp --enable-vnc \
  --enable-pipewire --audio-drv-list=pipewire --enable-libusb \
  --disable-docs --disable-guest-agent --disable-tools --disable-werror --disable-gtk \
  --disable-sdl --disable-opengl --disable-virglrenderer --disable-spice --disable-curses \
  --disable-modules
ninja qemu-system-x86_64
```

You'll need QEMU's usual build dependencies. The installer will do all this for you once there is one.

The patch is GPL-2.0-or-later.
