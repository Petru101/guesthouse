# Display driver

This is the indirect display driver for Windows guests. It gives Windows a real monitor at the host's refresh rate, with HDR, and sends each frame to the host without a CPU copy.

It's a fork of [zako-vdd](https://github.com/qiin2333/zako-vdd), by qiin2333 and its other authors, at v0.18.2, commit abadb3a. Nearly all of it is their work and it stays under their MIT licence, in [LICENSE](LICENSE). Their README is [README-zako.md](README-zako.md). I left their folders and names alone so that their fixes stay easy to take.

Nothing here has run in a guest yet, and it hasn't been built with the WDK either. See "What's been checked".

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

It needs the WDK, so it builds on Windows. Mount the EWDK ISO, run `LaunchBuildEnv.cmd`, then:

    msbuild ZakoVDD.sln /p:Configuration=Release /p:Platform=x64

## What's been checked

On Linux, `tests/run.sh` runs the ring logic against a fake daemon, and the shader's colour maths from the shader's own file. `tests/compile-mingw.sh` compiles the D3D11 and Win32 parts against mingw's headers.

Not checked at all: anything that includes `iddcx.h`, and `FramePass.hlsl` itself, which no compiler has seen.
