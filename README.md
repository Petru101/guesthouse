# Guesthouse

**Windows and Linux VMs on KVM that use the same GPU my desktop is using, with NVIDIA's real driver inside.**

![Godot's Forward+ renderer on Vulkan inside a Windows VM](docs/images/vulkan-in-windows.jpg)

That's Godot's full Forward+ renderer on Vulkan, in Windows, in a VM on Fedora. Nothing is passed through. The host never gives up the card, so my GNOME desktop keeps running on it the whole time.

> **Status:** you can't install this yet. The version that works is a prototype on my PC, and this repo is its rewrite in Rust, which doesn't run a guest so far. See [where this is at](#where-this-is-at).

## Why I'm building this

If you want a proper GPU in a Windows VM on Linux, you normally get two options. You pass the whole card through with VFIO and the host loses it, or you buy a datacenter card and a vGPU licence.

Hyper-V has a third way, called GPU-PV. The guest runs the vendor's normal driver and shares the host's card. It's the same plumbing WSL uses. KVM doesn't have it, and I wanted it, because my one big GPU is also the one running my desktop.

So this project teaches KVM to speak Hyper-V's GPU protocol. Windows boots, finds what looks like a Hyper-V GPU, and loads NVIDIA's unmodified driver. After that, the commands the driver writes run directly on the card.

## Where this is at

The prototype is a Python daemon and a desktop app. It's still private while I sort out licensing and clean it up.

This repo is where it's all heading. So far it has:

- the QEMU patch, in `qemu/`
- the code that reads and answers the guest's messages
- the tracking of processes, allocations, fences and GPU addresses
- the line a GPU vendor's backend plugs in at, so AMD and Intel can fit in later
- a tool that replays a recorded session and compares every reply with the prototype's

There's no GPU backend here yet, so a build starts, tells you that, and stops. The next big step is moving the NVIDIA part over, and after that the app and an installer.

## What the prototype does

Everything here was measured on one machine: Ryzen 9 9950X3D, RTX 5090, Fedora 45, NVIDIA 615.78.08 on the host and 617.42 in Windows.

### Windows guests

Direct3D 12, Direct3D 11, Vulkan and OpenGL all run on the real card, along with the desktop and normal apps. A fresh install takes about 9 minutes from picking the ISO to the GPU working.

**Is it really the GPU, at full speed?** Yes. This is my own benchmark, run on Linux and then inside the VM:

| Test | Linux host | Windows guest |
|---|---|---|
| Compute | 115–119 TFLOPS | 122.6 TFLOPS |
| Video memory copy | 1,064–1,072 GB/s | 1,036 GB/s |
| One dispatch with a barrier | 2.05 µs | 2.05 µs |
| Empty submission round trip | 21 µs | 240 µs |

Once work reaches the card it runs at full speed. Getting it there is what costs extra.

**How fast is a real program?** Superposition at 1080p Extreme in Direct3D 11 scores **20,912**, averaging 156 fps with a minimum of 98. That's with no viewer open. The lowest 5090 I could find on Unigine's leaderboard is around 31,000, so call it two thirds of what the card can do. Something is still eating the rest, and I don't know what yet.

![Superposition's result in the VM: 20,912 at 1080p Extreme](docs/images/superposition-result.jpg)

![Superposition running in the VM](docs/images/superposition-running.jpg)

*Dumps of the guest's real screen, shrunk for this page. The running shot is from an earlier run at the same settings. The result screen says "GPU: NVIDIA" because the launcher can't read the card's name yet, and its RAM line is wrong: the VM had 16 GB.*

A heavy Godot test scene at 1280x720 does about 690 fps natively. In the VM it's about 196 on Vulkan and 158–186 on Direct3D 12. GPU time per frame is the same as native, about 1.2 ms, so that gap isn't the GPU either.

**Is the picture right?** I don't judge that by eye. These tests compare exact values, and the Vulkan and OpenGL ones check the final screen pixel by pixel:

| Test | Wrong |
|---|---|
| Direct3D 12 frames | 0 of 300 |
| Vulkan through a real swapchain | 0 of 256,000 pixels |
| OpenGL through a real window | 0 of 256,000 pixels |

### Linux guests

Linux takes a different route. The guest runs NVIDIA's real Linux driver and its calls get forwarded to the host's driver. So far that's Ubuntu 26.04.1.

`nvidia-smi`, CUDA and Vulkan work, and speed is about the same as the host: 111–113 TFLOPS on Vulkan in both, and a CUDA launch takes 14.5 µs against 13.9. GNOME runs on it through Zink, with Firefox, Files, Settings and a terminal all working. Godot's Windows build even runs in there through Wine on Wayland.

Delay is where it's behind. A simple app takes about 20 to 30 ms from key press to picture, against about 11.5 on the host, and Godot takes 50 to 100 ms.

## How it works

```
Windows guest                              Linux host
┌──────────────────────────┐               ┌──────────────────────────────┐
│ your app                 │               │ a daemon, playing the part   │
│ NVIDIA's Windows driver  │               │ of Hyper-V                   │
│ dxgkrnl (GPU-PV client)  │ ── VMBus ──▶  │ NVIDIA's Linux driver        │
└──────────────────────────┘    (QEMU)     │ RTX 5090                     │
                                           └──────────────────────────────┘
```

There are three pieces:

- **A QEMU patch**, about 600 lines of code, which gives the guest the devices and the VMBus channels it expects to find on Hyper-V.
- **A daemon**, which answers everything Windows asks, sets up real GPU channels through the Linux driver, and rings their doorbells when the guest has work.
- **A desktop app**, Guesthouse, so that none of this needs a terminal. You pick a Windows ISO, and it installs Windows unattended and brings up the GPU.

Apart from one small workaround, the guest's commands aren't translated. The Windows driver writes them, and the GPU runs them. Finished frames stay in video memory and reach the viewer as dmabufs.

## My favourite bugs

**"The window is occluded."** Every Vulkan present failed with `PRESENT_OCCLUDED` while the window sat there in plain sight. I spent hours in Windows' visibility code. The real cause was my own daemon: Windows was sending it a copy to do, and it was refusing with "not surfaces I know". That line had been in my own log the whole time.

**1,366 allocations.** Vulkan fell over at 4K. The driver wanted a 256 MiB block in 32 MiB pieces, and my daemon kept answering "here's 192 KiB", so as far as I can tell the driver politely asked again, about 1,366 times per block. Answering with what it asked for took one run from 8,129 allocations to 374, and 4K started working.

**The blank Explorer.** WinUI apps opened with empty windows. Their frames were waiting on a CPU event that nobody ever signalled.

**The psychedelic sky.** Geometry perfect, colours from another dimension. A 10-bit image was being copied byte for byte into an 8-bit surface.

![A 10-bit image read as 8-bit](docs/images/bug-10-bit.jpg)

## What doesn't work yet

I'd rather you hear this from me:

- **You can't install it.** See above.
- **It has only ever run on my PC, on an RTX 5090.** A clean clone does set itself up and pass its tests now, but that's still my machine and my card.
- **Drivers are tied together.** Linux and Windows have to come from the same NVIDIA branch, and a new branch means new work.
- **CUDA, NVENC and NVDEC don't work in Windows yet.** `nvidia-smi` can't open the card there either, and Task Manager doesn't show the GPU.
- **No HDR yet.** It's a must for me, and it needs a new display path.
- **The viewer drops frames.** At 144 Hz about 116 frames a second reach it, and about 90 at 4K.
- **One GPU VM at a time** for now.

## What's next

1. Move the NVIDIA backend here, so this repo can boot a guest
2. The app and an installer, so you can try it
3. CUDA and video in Windows
4. Find the missing third of the speed
5. HDR
6. Several GPU VMs at once

## Trying the code

You need a recent stable Rust, and no GPU or VM.

```sh
cargo test                       # everything
cargo bench -p guesthouse-core   # the message path, in nanoseconds
```

[ARCHITECTURE.md](ARCHITECTURE.md) explains how the pieces fit, and [CONTRIBUTING.md](CONTRIBUTING.md) how to work on them.

## Bring your own

This repo has no NVIDIA or Microsoft files in it. You supply your own Windows ISO and NVIDIA's driver. This is a personal project, and it has nothing to do with NVIDIA or Microsoft.

## Licence

GPL-2.0-or-later, the QEMU patch included. See [LICENSE](LICENSE).
