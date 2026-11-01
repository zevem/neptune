# Terminal core ownership

The public seam is `lib.rs`, `view.rs`, `input.rs`, `search.rs`, and `error.rs`.
Desktop code uses owned snapshots and semantic session operations. Backend types
remain private to `session`, `engine`, `runtime`, `transport` and
`shell_integration`.

- `session.rs`: lifecycle/API orchestration; `runtime/state.rs`: synchronization,
  backpressure and events; `runtime/mod.rs`: reader/writer/parser workers.
- `transport.rs`: what feeds a session's engine (source, sink, process) and the
  PTY/ConPTY transport. Engine replies go to `Sink::reply`, separate from input.
- `engine.rs`: backend mapping and damage-aware snapshot extraction.
- `platform/mod.rs`: interruptible Unix I/O, Linux cwd polling, ConPTY teardown.
- `shell_integration/mod.rs`: portable OSC 7/9;9 and OSC 133 tracking, Windows
  shell prompt reporting.
- `tests.rs`: headless parser/API contracts; `tests/session.rs` and
  `tests/windows_conpty.rs`: actual transport/process contracts.

Grid mutations commit revisions while holding the terminal mutex. Snapshot
extraction locks terminal before viewport cache; resize locks config then terminal
before notifying the PTY and updating prompt/size state. Repaint callbacks must
only schedule work, never synchronously reenter a session. The renderer
acknowledges repaint before taking its snapshot. Synchronized output must still
flush its last update on the parser deadline. Worker counts are reserved before
thread startup and remain counted through teardown; UI shutdown never joins.

Read the relevant section of [README.md](README.md) when changing transport
budgets, repaint/resize ordering, OSC prompt handling or search. Keep bounded
search cancellable and revision-aware; wide/wrapped coordinates and explicit
limit errors are part of the public contract.

Choose the narrowest meaningful test target/filter; these are alternatives:

```sh
cargo test -p terminal-core --lib --locked
cargo test -p terminal-core --test session TEST_NAME --locked
cargo test -p terminal-core --test windows_conpty TEST_NAME --locked
cargo clippy -p terminal-core --lib --locked -- -D warnings
```

Replace `TEST_NAME` with the affected test; omit the filter only when the whole
target's contract changes. Dependency/public-seam changes can use
`python3 scripts/check-architecture.py`. Check examples only if they changed.

Unix PTY tests require a Unix host and `/bin/sh`; the zsh regression requires zsh.
ConPTY fixtures require Windows and do not execute on Linux/macOS. Headless contracts
and input replay tests run without shells, fonts, GPU or native windows.
