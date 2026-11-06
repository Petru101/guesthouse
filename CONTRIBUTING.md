# Contributing

## Getting going

You need a recent stable Rust, and nothing else. No GPU and no VM are needed for any of this:

```sh
cargo test
cargo clippy --workspace --all-targets --all-features
cargo fmt --check
cargo bench -p guesthouse-core
```

To try the sockets by hand, `cargo run -p guesthouse-daemon --features stand-in` answers messages with a pretend GPU behind them. It's for poking at, and it won't run a guest.

## Legal rules

These matter more than anything else here, because text inside code is something `.gitignore` can't catch.

- Notes, findings and research stay out of this repo, and so does anything quoted from them.
- Nothing copied from a GPU vendor's or Microsoft's files: no data, tables or text.
- A protocol field the code needs goes in as a named constant or type, described in plain words, saying what it does.
- Microsoft's public Linux driver for this protocol is GPL-2. You may read it to understand the protocol. Don't copy its code, comments or structure text, and name things in your own words.
- The QEMU patch in `qemu/` keeps QEMU's licence, GPL-2.0-or-later.
- No secrets, and no paths from your own machine.
- Everything here is GPL-2.0-or-later, and that includes what you add.

## Recordings

Recordings hold guest data. They never go in git. Keep them in `/captures/`, which is ignored.

## Code

- Small modules with one job. A crate's top comment should say what that job is.
- Name your numbers. A bare offset or size should be a constant, unless the test or the line beside it makes it obvious.
- Comments say why. If a comment says what the next line does, delete it.
- The message path shouldn't allocate or lock. Measure first, though: `core/tests/no_allocation.rs` and the benchmark will tell you, and a clever change needs a number behind it.
- Dependencies are few, and each one has a comment saying why it's there.
- `unsafe` lives in `daemon` where the operating system needs it, with a `SAFETY` comment on each block.

## Keeping the layers apart

`core` must never know about a vendor, and a backend must never reach into `core`. If a backend needs something, add a method to `HostServices`, and if the core needs a vendor's answer, add one to `Backend`. See [ARCHITECTURE.md](ARCHITECTURE.md).

## Behaviour

The prototype is the reference, bugs included, until the replay tool has proved the port. If you find a bug in its behaviour, keep the behaviour and write it down. Fixing it comes later, as a change of its own, once a recording can show what the fix changes.

## Tests

Each handler has tests in `core/src/session/tests.rs`, which send whole messages through a `Session` with the stand-in backend. Name a test after what must be true, like `fences_only_go_up`.

`core/tests/handler_fuzz.rs` sends random messages and fails on any panic. `FUZZ_ROUNDS=20000000 FUZZ_SEED=7 cargo test --release -p guesthouse-core --test handler_fuzz` runs one for longer.

## Commits

Small steps, each of which builds and passes its tests. Say what changed and why, in a line a stranger can follow.
