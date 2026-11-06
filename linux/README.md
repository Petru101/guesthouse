# Linux guests

A Linux guest doesn't use the Windows route. It runs NVIDIA's own Linux user-mode driver, the same files as the host, and a QEMU device forwards its kernel calls to the host's `/dev/nvidia*`. So far that's Ubuntu 26.04.

The app does the host's side of this: the install, the driver image, the QEMU arguments and the viewer. This folder is what goes into a guest, and the device itself is in `qemu/qemu-gpupv.patch`.

## How a call gets through

- **`guest/`** is a kernel module named `nvidia`, with real `/dev/nvidiactl`, `/dev/nvidia0` and `/dev/nvidia-uvm` nodes. It copies a call and everything the call points to into a slot, and says where the pointers and files are.
- **The device, `nvfwd`,** takes the call on the vCPU that made it. It copies the call out of the slot once and works only on that copy. It checks the call against `allow.txt`, finds the pointer and file fields for itself, fills in host values, clears any the guest left alone, and makes the ioctl.
- **A mapping** goes over part of a 64 GiB window in guest physical space. KVM's notifier does the rest, so no memory slot changes per mapping. UVM has to keep the guest's own address, so a UVM mapping gets a memory slot of its own.
- **`kapi/`** stands in for `nvidia-modeset`, under NVIDIA's own `nvidia-drm`. There's one virtual monitor, and a flip goes to the viewer. No display class reaches the host.
- **`layer/`** has two Vulkan layers. `wsi_*.c` answers surface calls, as NVIDIA's own WSI wants its X driver: a Vulkan app on Wayland turns it on with `ENABLE_NVFWD_WSI=1`. `zink_on_nvidia.c` corrects a few things for a compositor on Zink.
- **`cast/`** carries the viewer's keys and pointer into the session, and has a GLX vendor and the Xwayland wrapper for X11 clients.
- **`setup/`** is the guest's side of the install: the seed, first boot, and `gpu-up.sh`, which brings the GPU up on each boot or turns it off and says why.

The card's sensor readings go through `nvfwd-sensord`, a separate program. It keeps the driver's sensor page and copies the parts a guest may see into a page of its own, so `nvidia-smi` shows real readings and QEMU holds no host client.

## Trying it

You need a Fedora host with RPM Fusion's NVIDIA packages, an NVIDIA card with the open kernel modules loaded, and the driver version in `TESTED_VERSIONS` in `crates/app-core/src/linux/driver_versions.rs`. The app reads the driver's files out of those packages with `rpm`, so another distribution won't work yet. You also need `bwrap`, `genisoimage`, `qemu-img` and `swtpm`. It has only been tried on an AMD CPU.

1. Build QEMU as `qemu/README.md` says, and copy `hw/misc/nvfwd/nvfwd-sensord` next to `qemu-system-x86_64`.
2. `cargo build --release`, and `cargo build --release -p guesthouse-app` for the viewer.
3. Get NVIDIA's source at the loaded driver's version, into `src/open-gpu-kernel-modules` in the data store, which is `~/.local/share/guesthouse`, or say where it is with `NVIDIA_SOURCE`. The guest's modules build against its headers, which aren't in this repository.
4. Set `GUESTHOUSE_PROGRAM` to this repository and `QEMU` to your build, and make an Ubuntu machine from Ubuntu's server ISO.

The install takes about ten minutes with no hands, and the first boot builds everything. If anything about the driver is in doubt the GPU stays off and the VM still boots, and the guest says why. As root in the guest, `nvfwd-check` says which parts work.

## How far to trust a guest

A Linux guest gets the GPU the way a program of yours does. It can do what your user can do to the card, it sees whole-card sensor and memory readings, and it can slow or crash your GPU session, which takes your desktop with it. It only gets calls on the list, it can only name its own GPU clients, and QEMU runs in a sandbox with no root. As far as I could test, it can't reach QEMU's memory or anyone else's GPU work. There's no full jail yet, so a bug in NVIDIA's driver or in this device is still a bug in your user account. Treat a guest as being as trusted as you, and don't run one you don't trust.

On a driver point release the sensor daemon doesn't know, CUDA and Vulkan may fail outright instead of just losing sensors. I haven't tried one.

## Limits

Each is a setting on the device, so `-device nvfwd,...,max-vram=12G`. A value of 0 for the rate turns it off. A machine's video memory cap in the app becomes `max-vram`.

| Setting | Default | Past it |
|---|---|---|
| `max-call-rate` | 20,000 calls a second | the call waits on its own vCPU |
| `max-call-burst` | 200,000 calls | the same |
| `max-vram` | 8 GiB | out of memory, as on a full card |
| `max-sysmem` | 8 GiB | out of memory |
| `max-objects` | 65,536 | out of resources |
| `max-maps` | 7,168 | mmap fails with ENOMEM |
| `max-uvm` | 32 GiB | mmap fails with ENOMEM |

The defaults come from what a session really does. GNOME with Files and Terminal peaked at 2,902 forwarded calls a second, 920 objects and 241 mappings, and was never made to wait. A desktop with more on it will hold more, and I haven't measured one.

Vulkan needs more than a 2 GiB `max-vram`: at that value pipelines fail to create. Only memory the guest asks for is counted. Page tables and channels use video memory too, and those are held down only by the object limit. Nothing limits GPU time.

## The jail

QEMU runs in a bubblewrap sandbox when a guest has the forwarded GPU: `crates/app-core/src/vm_args/jail.rs`. It sees the system's programs read-only, the GPU's device nodes, KVM, the VM's disk folder and sockets, and it needs no root. The device won't run as root at all.

This is the first step and no more. QEMU still opens `/dev/nvidia*` itself, with no syscall filter. The rest of the plan is a small proxy that owns those files and passes them to QEMU over a socket, then seccomp around QEMU. That proxy has to answer live, as a guest opens a new file for each mapping.

## Tests

`python3 -m unittest discover -s linux/setup/tests -p 'test_*.py'` needs Python and bash.

`make -C linux/tests/device QEMU=/path/to/patched/qemu BUILD=/somewhere` runs the device's checks with no GPU and no VM, under ASan and UBSan. It needs `libasan`, `libubsan` and glib's headers.

`tests/` has small programs for CUDA, Vulkan and OpenGL. The CUDA and Vulkan ones count wrong values, and `glbench` writes an image to compare with a run on the host.
