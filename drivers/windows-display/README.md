# Windows display driver

This is the indirect display driver for Windows guests. It gives Windows a real monitor at the host's refresh rate, with HDR, and sends each frame to the host with no CPU copy.

It's a fork of [zako-vdd](https://github.com/qiin2333/zako-vdd), by qiin2333 and its other authors, at v0.18.2, commit abadb3a. Nearly all of it is their work, and the original copyright is in [LICENSE](LICENSE). Their README is [README-upstream.md](README-upstream.md). I gave the driver its own name so that nobody mistakes this for their driver or blames them for mine, and left their folder layout alone so that their fixes stay easy to take.

This folder is under the MIT licence, in [LICENSE](LICENSE), and it stays that way, unlike the rest of this repo. Building it uses Microsoft's kit, and I don't want a question over those terms and the GPL. The driver started from Microsoft's [indirect display sample](https://github.com/microsoft/Windows-driver-samples/tree/main/video/IndirectDisplay), so what's left of that stays under the Microsoft Public License, in [LICENSE-MS-PL](LICENSE-MS-PL).

To build and install it, see [BUILDING.md](BUILDING.md). Only install it in a Guesthouse guest: it's made to be the only display, and on a real PC it would take over from your real monitors.

## What I added

Everything of mine is in `GuesthouseDisplay/Guesthouse/`, plus a few lines in the swap chain loop and callbacks.

| File | Job |
|---|---|
| `Common/Include/guesthouse_display_protocol.h` | The messages between this driver and the daemon |
| `HostLink` | Asks for a slot, says when a frame is in it |
| `KernelEscapeChannel` | Sends those through `D3DKMTEscape` |
| `FramePass`, `Shaders/` | One compute pass from Windows' surface to a linear slot |
| `ColourTransform`, `MonitorColour` | Windows' 3x4 colour transform, as one matrix |
| `CursorLink`, `CursorImage` | The hardware cursor, for a viewer to draw |
| `HostExporter` | The steps of each frame, in order |

The driver makes its monitor when it starts. It keeps it until the guest stops, and ignores `DESTROYMONITOR`, as Sunshine sends that whenever it exits. `setup/take-over-display.ps1` makes that the only display at logon, as only a user's session can do that. Windows then remembers it.

The hardware cursor is on in `vdd_settings.xml`. It goes to the viewer by the frame socket, and with no viewer asking for it the frame pass draws it into the frames.

The frame channel for Sunshine is still here. It only starts copying frames once something asks about it. Its contract with Sunshine keeps zako-vdd's names, in `Common/Include/vdd_control_ioctl.h` and in the shared object names, because Sunshine looks for those. Nobody using this sees them.

## What's been tested

On my PC, with an RTX 5090 and Windows 11 in a VM, in one or two runs of each:

- Known pictures read back on the host from a dmabuf, in SDR and HDR.
- Frames at 1920x1080 at 60 and 3840x2160 at 60, 120 and 240 Hz, SDR and HDR, with none lost over 20 seconds. That was a simple moving picture, into a test client and not onto a monitor.
- Mode changes between 800x600 and 4K, and SDR to HDR and back, with a viewer connected.
- A restart and cold boots with nothing done by hand after the install.
- Installing and loading with Secure Boot on.

`tests/run.sh` runs what needs no Windows on Linux: the ring's rules against a fake daemon, and the shader's colour maths from the shader's own file. `tests/compile-mingw.sh` compiles the D3D11 parts against mingw's headers. `tests/bars/d12hdr.c` is the test app that draws the pictures. `tests/bars/dupcheck.c` checks every pixel of what Desktop Duplication hands an app against those pictures.
