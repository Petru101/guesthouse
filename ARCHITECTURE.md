# Architecture

## The shape of it

```
Windows guest                 QEMU                      guesthouse-daemon
  vendor driver                                         ┌───────────────────────┐
  graphics kernel  ─ VMBus ─▶ forwards every packet ──▶ │ transport  frames     │
                              as a frame on a socket    │ messages   parsing    │
                                                        │ core       the books  │
                                                        │ backend    the vendor │
                                                        └───────────────────────┘
```

The guest thinks it's talking to a Hyper-V host. QEMU moves bytes and knows nothing about graphics. Everything else happens in the daemon: it plays the host's half of the protocol, and hands anything vendor-specific to a backend.

The daemon runs as one process per VM.

## The crates

They're in `crates/`, listed here from the bottom up. Each depends only on crates above it in this list.

| Crate | Its job |
|---|---|
| `transport` | The byte stream from QEMU: frames, packet kinds, guest page ranges. |
| `messages` | What the guest's messages look like, and how to write the answers. No state, no decisions. |
| `backend` | The `Backend` trait: what a GPU vendor must provide. Also the decision on whether to start. |
| `backend-standin` | A pretend GPU, for tests only. |
| `core` | `Session`: the tables for one guest, and the answer to each message. Vendor-neutral. |
| `daemon` | Slots, shared memory, sockets, and the main loop. |
| `replay` | Replays a recording and compares every reply. |
| `frame-socket` | The four messages that bring frames to a viewer, on their own so a producer can use them as they are. |
| `app-core` | The desktop app's work with no UI: machines, tasks, setup, a VM's start, and `guesthouse-task` and `guesthouse-share`. |
| `app` | The desktop app's GTK 4 and libadwaita pages. |

To follow one message, start at `Server::read_channel` in `daemon`, go through `Router::handle_frame`, and land in `Session::dispatch` in `core/src/session/mod.rs`. The handlers are grouped by subject in that folder.

## One thread

Everything happens on one thread. That's why the message path has no locks, and why both channels share a single order of events. Things that have to wait for a fence go in a list, and the main loop calls `Session::poll` while that list has anything in it.

The cost is that a slow backend call holds up both channels. Backend calls on the message path must not block.

The tests hold this path to a standard: `core/tests/no_allocation.rs` fails if a steady flow of submissions and signals touches the allocator, and `cargo bench -p guesthouse-core` gives the time per message. Those numbers cover parsing, bookkeeping and building the reply, and not the socket, QEMU or the guest. `daemon/examples/round_trip.rs` times a round trip through the socket against any running daemon.

## Slots

`GUESTHOUSE_SLOT` names the VM, from 0 to 3, and unset means 0. Everything the daemon touches is named from it in `daemon/src/slot.rs`: the run folder, the shared memory files, the ports. Slot 0 has the plain names, and other slots add their number. Anything else in the variable stops the daemon, because a guess could reach another VM's files.

## The vendor line

A vendor backend implements `Backend` and depends on the `backend` crate. It never sees the core. It gets plain values, and asks for what it needs through `HostServices`. Going the other way, the core treats a vendor's private data as opaque bytes and passes them along unread. The one exception is the small record for surfaces the guest's kernel makes itself, which is our own format, in `messages/src/standard.rs`.

If you find yourself wanting to name a vendor in `core`, something belongs in a backend instead.

AMD and Intel backends should be possible through this same trait. None exist yet, and there are no empty stubs for them. Only NVIDIA is planned so far, as one backend crate.

## Three layers inside a backend

Different cards and drivers differ in different ways, and those ways shouldn't get tangled. A backend is built from these parts:

- **Vendor.** The backend crate itself.
- **Generation.** A small profile of what an architecture changes: class numbers, channel types, shader version rules. Blackwell would come first, and Ada or Ampere should only need another profile.
- **Driver branch.** A second profile, beside generations, of what a driver branch changes in its data layouts: control and allocation structure sizes on the host, and the layout of the guest's record.

Generations and branches stay separate, so they never multiply into a matrix of every generation with every branch. `Backend::open` is where a backend picks one of each. Nothing in the trait is per-generation or per-branch, so all of this stays inside the backend.

Cards are never listed in code as behaviour. Name, video memory, engines and copy engine details are read from the live card, and end up in `AdapterFacts`. The only per-card list a backend may keep is a label, `CardLabel::Tested` or `Experimental`, and that label changes nothing the daemon does.

## Starting up

`daemon/src/startup.rs` runs three steps:

1. `Backend::survey` looks at what's installed and reports the driver versions and branches, and what's been tested. It must not send any request to the host's driver. Reading a version from sysfs is fine, and opening the device is not.
2. `decide_start` in `backend/src/driver.rs` judges the report, the same way for every vendor:
   - a known branch at a tested version runs
   - a known branch at another point release runs, with a warning
   - an unknown branch is refused
   - host and guest on different branches are refused, until that pair has been tested
3. Only then does `Backend::open` touch the card.

A refusal is a plain sentence, and it comes before any request reaches the host's driver, since we'd otherwise be handing a driver structures of a size it may not expect.

A real check has to compare what it found against the branch profiles. Asking the driver for its version and repeating that back to it passes on every version, so it proves nothing.

The driver version the guest is told comes from setup, which reads it from the driver's INF (`driver_version_from_inf`). It reaches the core through `AdapterFacts::guest_driver_version`, and it's never a constant.

## The record-and-compare test

The Python prototype is the reference for behaviour, and the replay tool is how we check we match it.

A recording holds what the prototype received and sent in a real session, along with what the replay can't work out alone: when fences moved, when the prototype took GPU addresses for its own channels, and whether each submission reached the GPU. The format is described in `replay/src/recording.rs`.

`guesthouse-replay FILE` feeds the received frames through the real `Router`, compares every reply byte for byte, and names the first difference with a likely cause.

There's a catch to keep in mind. With no vendor backend, `RecordedAnswers` plays that part by reading vendor data out of the recording, and bytes taken from the recording can't fail to match. So the report keeps three counts apart:

- replies the core worked out alone, which are real evidence
- replies with vendor fields taken from the recording, where the rest of the reply is real evidence
- replies supplied whole by the recording, which are no evidence at all

The host's own clock in the clock answer is masked, since it can never repeat. Messages the host sends by itself are compared as a set and not in order, because in a real session their order depends on the GPU's timing.

## The app

`app-core` needs no display, so nearly all of the app can be tested alone. Each machine is one JSON file. Anything slow runs as a task: `guesthouse-task` is a process of its own that outlives the app and leaves its steps in a small file, so the app can close at any moment and read where things are when it opens again.

`Context` holds what the code needs from the PC, and tests swap its parts for stand-ins. A VM's QEMU arguments are built as data in `vm_args`. How a VM and the daemon come up is behind `VmHost`, and everything that knows the daemon's start-up is in `daemon.rs`, so a change there stays there.

Nothing is relative to a project folder. Machines are in the data store, which `~/.config/guesthouse/store` can move, sockets are in the XDG runtime folder, the log is in the state folder, and what ships with the app is in a program folder found from where it runs. Each machine has its own token for the guest's agent. Files for the guest aren't in this repo: they ship with the app.

The viewer only knows `FrameSource`, which has two kinds. `SocketFrames` takes a guest's frames from the frame socket, with a new texture for each frame, and gives a slot back when GTK lets go of its texture. `DbusFrames` listens to QEMU's D-Bus display, which is all that firmware, an install and a plain VM have, and a GPU guest has it too until its first frame and again after a reset.

## Not here yet

Left out for now, because they're still changing in the prototype:

- the NVIDIA backend, and with it real submissions
- the daemon's end of the frame socket

Also not ported: the arena (video memory the guest's CPU can reach), the experiment switches in the prototype's run folder, and copies to the screen running off the message thread.
