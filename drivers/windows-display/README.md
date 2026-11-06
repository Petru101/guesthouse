# Display driver

This is the indirect display driver for Windows guests. It gives Windows a real monitor at the host's refresh rate, with HDR, and sends each frame to the host without a CPU copy.

It's a fork of [zako-vdd](https://github.com/qiin2333/zako-vdd), by qiin2333 and its other authors, at v0.18.2, commit abadb3a. Nearly all of it is their work and it stays under their MIT licence, in [LICENSE](LICENSE). Their README is [README-zako.md](README-zako.md). I left their folders and names alone so that their fixes stay easy to take.

Nothing here has run in a guest yet. It builds with the EWDK, and that's as far as it's got.

## What I added

Everything of mine is in `ZakoVDD/Guesthouse/`, plus a few lines in zako's swap chain loop and callbacks.

| File | Job |
|---|---|
| `Common/Include/guesthouse_display_protocol.h` | The Escape messages, shared with the daemon |
| `HostLink` | The ring conversation: REGISTER, FRAME, and which slot to use next |
| `KernelEscapeChannel` | Sends those through `D3DKMTEscape` |
| `FramePass`, `Shaders/` | One compute pass from Windows' surface to a linear slot |
| `ColourTransform`, `MonitorColour` | Windows' 3x4 transform, composed into one matrix |
| `CursorImage` | The hardware cursor for the pass, if it's turned on |
| `HostExporter` | The steps of each frame, in order |

Zako's frame channel for Sunshine is still here. It only starts copying frames once something asks about it.

## Building

It needs the WDK, so it builds on Windows. `build-vm/start.sh` boots a plain VM with the EWDK ISO as a CD and no GPU, and `build-vm/build.sh` sends the source over and builds it. By hand, mount the ISO, run `LaunchBuildEnv.cmd`, then:

    msbuild ZakoVDD.sln /p:Configuration=Release /p:Platform=x64 /p:SignMode=Off

I turned warnings-as-errors off, because the EWDK's newer analysis flags zako's code in places I haven't touched. My files build with no warnings.

## What's been checked

On Linux, `tests/run.sh` runs the ring logic against a fake daemon, and the shader's colour maths from the shader's own file. `tests/compile-mingw.sh` compiles the D3D11 and Win32 parts against mingw's headers.

The whole driver and its shader compile and link with the EWDK. None of it has been loaded or run.
