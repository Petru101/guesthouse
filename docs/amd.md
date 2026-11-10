# Windows on an AMD card

This is experimental. It works on the card I have and on a few testers' cards, and there's a lot it hasn't met yet.

## What's run

My card is the integrated Radeon in a Ryzen 9 9950X3D: RDNA2, 1002:13C0, on Fedora 45's amdgpu, with AMD's Windows driver 32.0.21045.20006 in the guest.

| | |
|---|---|
| Desktop | works, on my display driver at the size and refresh you pick, and the guest sees AMD's vendor ID |
| HDR | works, where the screen has it and the machine's card draws the desktop |
| Direct3D 11 | works, the test triangle matches WARP pixel for pixel |
| Direct3D 12 | works |
| Vulkan in a window | works, 0 wrong pixels, 144 fps at 144 Hz in my present test |
| OpenGL in a window | works, 4.6 compatibility and 3.3 core, 0 wrong pixels, about 100 to 110 fps |
| Hardware video decode | not yet, it falls back to the CPU |

Testers have run it on these too, on CachyOS with Mesa 26.2:

| Card | How it went |
|---|---|
| RX 7900 XTX, RDNA3 | installs, display driver at 1920x1080 144 Hz, basic 3D works |
| RX 6600 XT, RDNA2 | works: OpenGL games and Edge on the GPU. One old Direct3D 9 game shows stripes |
| RX 6900 XT, RDNA2 | on 0.3.0 the display driver failed and Windows stayed on its basic display. Not tried on 0.4.0 yet |

## Setting it up

1. Download AMD's Windows driver installer for your card from AMD's site. AMD ships each release in packages for different generations, so get the one that lists your card.
2. In the app, make a new Windows machine and pick the AMD card. When there's no AMD driver yet, the dialog asks for the installer and unpacks it for you.
3. Install as usual. Windows comes up with AMD's driver.

A machine made for AMD stays on AMD. It can move to another AMD card while it's off, but an NVIDIA card needs a new machine, and the other way round.

## Other AMD cards

Other RX 6000 and RX 7000 cards should work like the ones above. RX 5000 cards use the same driver package as the RX 6000s, but nobody has tried one. RDNA4 cards, the RX 9000 series, lay out their memory differently, and the copies to the screen don't handle that yet. Older cards like Polaris aren't handled.

If you have an AMD card and want to help, [#1](https://github.com/xnixy/guesthouse/issues/1) is the place.

## Linux guests on AMD

That's a different route, through virtio-gpu's native context, and it's in [`linux/`](../linux/README.md#an-amd-card).
