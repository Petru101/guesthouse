# Windows on an AMD card

This is experimental. It works on the one card I have, and nothing else has tried it yet.

## What's run

The integrated Radeon in a Ryzen 9 9950X3D: RDNA2, 1002:13C0, on Fedora 45's amdgpu, with AMD's Windows driver 32.0.21045.20006 in the guest.

| | |
|---|---|
| Desktop | works, and the guest sees AMD's vendor ID |
| Direct3D 11 | works, the test triangle matches WARP pixel for pixel |
| Direct3D 12 | works |
| Vulkan in a window | works, 0 wrong pixels, about 63 fps in my present test |
| OpenGL in a window | works, 4.6 compatibility and 3.3 core, 0 wrong pixels, about 35 fps |
| Hardware video decode | not yet, it falls back to the CPU |

OpenGL is slower than Vulkan for now because each frame waits on its copy to the screen.

## Setting it up

1. Download AMD's Windows driver installer for your card from AMD's site. AMD ships each release in packages for different generations, so get the one that lists your card.
2. In the app, make a new Windows machine and pick the AMD card. When there's no AMD driver yet, the dialog asks for the installer and unpacks it for you.
3. Install as usual. Windows comes up with AMD's driver.

A machine made for AMD stays on AMD. It can move to another AMD card while it's off, but an NVIDIA card needs a new machine, and the other way round.

## Other AMD cards

I'd expect other RDNA2 cards, like the RX 6000 series, to be closest to working, as they use the same driver release. A discrete card keeps most things in its own video memory where an integrated one uses system memory, and that's where I'd expect the first problems.

RDNA3 cards need AMD's newer driver package, which the backend knows about but hasn't run against a real guest. RDNA4 changes the memory layouts the screen copy reads, so OpenGL windows won't show there until that's done. Older cards like Polaris aren't handled.

If you have an AMD card and want to help, [#1](https://github.com/xnixy/guesthouse/issues/1) is the place.

## Linux guests on AMD

That's a different route, through virtio-gpu's native context, and it's in [`linux/`](../linux/README.md#an-amd-card).
