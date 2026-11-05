# Guesthouse

GPU virtual machines on Linux, with the host keeping its card.

The idea: a Windows guest runs its GPU vendor's normal driver, and shares the card the Linux desktop is already using. Nothing is passed through and the host never gives the card up. Hyper-V can do this, and calls it GPU-PV. KVM can't, and this project is about teaching it.

## Status

This is early, and it doesn't run a guest yet.

A prototype of the idea works in a private lab repo, as a Python daemon. This repo is the start of its rewrite in Rust. So far it has the parts that won't change soon:

- reading and answering the guest's messages
- keeping track of processes, allocations, fences and GPU addresses
- the line a GPU vendor's backend plugs in at
- a tool that replays a recorded session and compares every reply with the prototype's

There's no GPU backend at all yet, so a normal build starts up, says so, and stops. Nothing here has met a real guest.

## Finding your way around

[ARCHITECTURE.md](ARCHITECTURE.md) explains how the pieces fit. [CONTRIBUTING.md](CONTRIBUTING.md) explains how to work on them, and the legal rules that matter here.

```sh
cargo test                       # everything
cargo bench -p guesthouse-core   # the message path, in nanoseconds
```

## Licence

There's no licence yet. That's still to be decided.
