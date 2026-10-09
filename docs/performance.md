# Performance measurements

Neptune uses a native Rust PTY and terminal parser with a GPU-rendered interface.
The parser, PTY transport, shaping, GPU work, and input latency need separate
measurements. A fast parser alone does not establish terminal rendering speed.

The [Linux prerelease freeze investigation](linux-freeze-investigation.md)
records a GPU acquisition stall after idle, the tested presentation change,
and the distinction between XWayland evidence and the unresolved native
Wayland report.

## Recorded parser baseline

On 2026-09-30, the release parser benchmark recorded:

| Workload | Throughput | Retained history |
| --- | ---: | ---: |
| Plain log | 144.3 MiB/s | 10,000 rows |
| ANSI + Unicode | 153.5 MiB/s | 10,000 rows |

The process's peak resident memory after both cases was **42,796 KiB**, about
41.8 MiB. This is the benchmark process's lifetime high-water mark, including
the generated payload, parser, terminal grid, scrollback, and allocator state.
It is not the native application's memory usage or an isolated grid allocation.

The development host was an Intel Core i9-13900HX with 32 logical CPUs, Ubuntu
26.04.1 LTS, Linux 7.0.0-34-generic, and Rust 1.97.1 on x86_64. The release
profile uses thin LTO and one codegen unit. These are local baseline results,
not a comparison with Ghostty, cmux, or any other terminal. Record repeated
measurements on each target platform before publishing comparative claims.

### Workload and reproduction

Run from the repository root:

```sh
cargo run --release --locked -p terminal-core --example parser_bench
```

The implementation is in
[`parser_bench.rs`](../crates/terminal-core/examples/parser_bench.rs). Each case
uses a fresh 160-column × 40-row terminal and the engine's 10,000-row default
scrollback. One fixture line primes the parser before timing begins. The timed
loop feeds approximately 64 MiB through `Processor::advance` in 64 KiB batches.

Both fixture lines are 96 UTF-8 bytes. The program repeats a line 10,922 times
to make a 1,048,512-byte payload, then parses that payload 64 times: exactly
67,104,768 bytes, or 63.996 MiB per case. The displayed size rounds to 64.00 MiB.

- The plain-log fixture contains an ISO timestamp, log level, worker/task
  fields, a duration, and CRLF. Its line length stays below the terminal width.
- The ANSI + Unicode fixture includes green foreground, bold/reset sequences,
  RGB foreground, a checkmark, two wide CJK characters, an accented Latin
  character, a combining accent, and CRLF.

Payload creation is outside the timed region. The benchmark verifies that
history finishes at exactly 10,000 rows. Its memory number comes from Linux
`VmHWM`; other platforms print throughput and history without this memory line.
The two fixtures exercise different parser and cell-update paths, so their
relative throughput is not a general ranking of ANSI versus plain output.

This measures parser and scrollback work only. It excludes shell startup,
operating-system PTY transfer, UI scheduling, font shaping, GPU rendering,
compositor presentation, and keyboard-to-screen latency. It also lacks a
statistical distribution: the table records one local run. For regression
analysis, repeat the benchmark and retain raw output together with build,
hardware, power mode, and competing workload information.

## Real PTY pipeline baseline

The release [`pty_bench`](../crates/terminal-core/examples/pty_bench.rs) measured
**53.0 MiB/s** on the same Linux host: exactly 64 MiB received and parsed in
1.208 seconds. The original `pty-bench.log` was local-only and is not available
in the [tracked evidence archive](artifact-archive.md).
Peak process RSS was 42,180 KiB. This is another engine-only process, rather
than the desktop application's resident memory.

```sh
cargo run --release --locked -p terminal-core --example pty_bench
```

The real child disables echo and output newline translation with `stty`, then
POSIX `awk` produces 524,288 lines of 128 bytes including CRLF. A ready handshake
separates setup from the measured interval. Timing includes input dispatch,
producer startup and output generation, native PTY transfer, bounded buffering,
VT parsing, 10,000 retained history rows, child exit, and final output drainage.
The example verifies both byte counters and the final visible marker. The
5.105 ms spawn-to-ready interval and 93.293 ms remaining worker cleanup are
reported separately and excluded from throughput.

A simulated 120 Hz consumer acknowledges pending redraw notifications before
reading state. The run produced 450,497 terminal revisions and **142 repaint
requests**. The callback only increments a counter; requests are not rendered
frames. Output notifications coalesce until the next acknowledgement, while
metadata, error, and final-exit notifications still wake consumers. Real PTY
tests cover an immediate first wakeup, a later change after acknowledgement,
burst coalescing, and delivery of the final exited frame. This avoids invoking
the GUI callback once for every small read during sustained output.

Producer speed, PTY scheduling, and host load affect the pipeline rate. This is
one local baseline, without a distribution. It excludes GUI scheduling, font
shaping, GPU submission/presentation, and keyboard-to-screen latency. The Unix
fixture requires `/bin/sh`, `stty`, and `awk`; it is not a ConPTY benchmark.

## Native application CPU and memory

[`measure-idle.py`](../scripts/measure-idle.py) observes an existing Linux
process. It sends no input or signals and does not start or stop applications.
The default is a five-second warmup followed by a five-second sample, taking
RSS snapshots every 250 ms:

```sh
python3 scripts/measure-idle.py <neptune-pid> \
  --label 'release; one quiet shell; focused; cursor blink enabled' \
  --output artifacts/idle-release.json
```

Use the process running `target/release/neptune`. Let startup, shell output, and
font loading finish. Keep the window visible and settled, leave the same cursor
and focus settings throughout the sample, and stop UI automation. A build that
is receiving clicks, input, output, or resizes is an activity observation,
not an idle measurement. The script records `idle_verified: false` because it
cannot independently establish those conditions; the operator must report them.

CPU is the delta of process-wide `utime + stime` from `/proc/<tgid>/stat`,
converted using `SC_CLK_TCK` and divided by the measured monotonic interval.
This includes all application threads, rather than just the main thread.
It excludes shell/child processes and GPU execution. A reported 100% means
one fully occupied CPU core; multiple busy threads can exceed 100%. The Linux
[process accounting implementation](https://github.com/torvalds/linux/blob/master/fs/proc/array.c)
uses thread-group CPU accounting for this process record.

RSS is sampled from `/proc/<pid>/status`. The JSON preserves start/end,
sampled minimum/maximum/mean, and the process's lifetime high-water mark.
These are resident-memory observations, not proportional/private memory or
GPU memory; RSS can include shared mappings. The
[Linux proc documentation](https://docs.kernel.org/filesystems/proc.html)
describes these memory fields. The script checks process start time to reject
PID reuse and fails if the application exits during measurement.

### Recorded release idle observation

On 2026-09-30, the normal release executable was observed with one quiet zsh
session, a visible and focused window, and cursor blinking disabled. The window
was 1180×760 logical points at 2× scale under XWayland; rendering used Vulkan on
an NVIDIA GeForce RTX 4060 Laptop GPU. Startup and a real Neovim smoke test had
finished. No input, terminal output, resize, or UI automation occurred during
the five-second warmup or twenty-second sample.

| Measurement | Observed value |
| --- | ---: |
| Process CPU, as a fraction of one core | 0.5% |
| Process CPU time over 20.000572 seconds | 0.10 seconds |
| RSS throughout 81 samples | 230,228 KiB (224.8 MiB) |
| Application threads | 14 |

The raw record is [idle-release-final.json](https://github.com/zevem/neptune/blob/994604ee9d27c907d32630287c7aaba43eb7302e/artifacts/idle-release-final.json).
Its `idle_verified: false` field remains unchanged: the script cannot infer
activity conditions, which are recorded above by the operator. This single
local observation includes application/driver mappings in RSS and excludes child
process CPU and GPU execution. It does not establish an idle distribution or
other platforms' behavior. The older five-second development artifact is not
the accepted release observation.

### Native Wayland idle after the interface rebuild

The observation above was taken under XWayland. On native Wayland (GNOME,
1.5× scale, same GPU) the same quiet window did not sleep: the input method
answered every cursor-area update with another empty pre-edit event, the
toolkit repaints for any event and then updates the cursor area again, and the
window repainted at the display rate. `--diagnostics` frame counters showed
about 144 frames per second for a build of the commit before the interface
rebuild, and 2 per second on X11 (the diagnostics timer). The desktop now drops
pre-edit updates that repeat an already empty composition before the toolkit
sees them (`input::drop_redundant_preedits`); changes to or from an active
composition are kept. With that filter the debug build showed 2 frames per
second on native Wayland as well.

On 2026-09-30 the rebuilt normal release executable was then observed on native
Wayland with one quiet zsh session, a visible window, cursor blinking disabled
and no input, output, resize or automation during a five-second warmup and
twenty-second sample. Window focus was not verified.

| Measurement | Observed value |
| --- | ---: |
| Process CPU, as a fraction of one core | 0.7% |
| Process CPU time over 20.00056 seconds | 0.14 seconds |
| RSS at end of sample | 219,304 KiB (214.2 MiB) |
| Application threads | 18 |

The raw record is
[idle-release-wayland.json](https://github.com/zevem/neptune/blob/994604ee9d27c907d32630287c7aaba43eb7302e/artifacts/ui-rebuild/idle-release-wayland.json).
It is one local observation with the same scope limits as above. An input
method that is actively composing, other compositors and other input-method
frameworks were not observed.

For very low CPU usage, use a longer `--duration`, since five-second measurements
are limited by the operating system's CPU tick resolution. Observe both
cursor-blink settings and both focused/unfocused states before establishing a
broader idle baseline.

## Architecture refactor resource scenarios

[`measure-scale.py`](../scripts/measure-scale.py) launches an isolated Linux/X11
inspection release for each 1/8/32/64-pane scenario. Workspaces contain at most
eight visible panes; additional workspaces retain live hidden sessions. It
exercises quiet shells, one visible output producer, all visible panes producing
output, one producer hidden by zoom/workspace selection, and six split/close
cycles. At 64 panes the lifecycle case closes and fully releases a slot before
splitting. The one-pane hidden case is explicitly inapplicable.

```sh
cargo build --release --features inspection --bins --locked
python3 scripts/measure-scale.py --counts 1 8 32 64 --seconds 3 \
  --output artifacts/scale
```

Each producer must create its startup marker and deliver at least 10,000 parsed
bytes to its intended pane before sampling, and remain active during the sample.
Producers write approximately
1.8 KiB every 15 ms and have a finite lifetime. Every case verifies full session
teardown and reaps only its owned process. Output, state, endpoint, screenshot
and diagnostic logs belong to that case. The report records compiler, platform,
features and binary/lockfile hashes.

CPU/RSS use a separate quiet interval after command probes finish. CPU includes
Neptune's threads, excluding shell/producer processes and GPU execution. Frame
percentiles describe application UI CPU work in a rolling sample of up to 2,048
frames across the run, including setup and probes; they exclude GPU presentation.
The five latency samples measure inspection text/key roundtrip to shell-created
file readiness. They include protocol/readiness polling and **are not native
keyboard-to-display latency**. Small-sample p95/p99 use nearest rank and retain
the slowest observation; five samples cannot establish a stable tail distribution.
Three-second CPU observations have coarse OS tick resolution. These scenarios
validate bounded behavior and provide local observations, rather than a
comparative performance claim.

The [accepted Linux summary](https://github.com/zevem/neptune/blob/994604ee9d27c907d32630287c7aaba43eb7302e/artifacts/architecture-scale-accepted/accepted-summary.json)
records 19 passing applicable cases and the one-pane hidden-output exception.
Each accepted output case has parsed-byte activity evidence through sampling;
one 64-pane several-output case was repeated with a longer bounded producer
lifetime after the first producers stopped before sampling. All cases confirmed
complete teardown. The raw records retain p50/p95/p99, byte/lock/cache counters,
CPU/RSS and metadata. Earlier incomplete or unverified observations are not
accepted results.

The refactored real-PTY pipeline also verified exactly **67,108,864 bytes**
received and parsed, the final marker, 10,000 retained rows and worker cleanup.
Its [raw verification output](https://github.com/zevem/neptune/blob/994604ee9d27c907d32630287c7aaba43eb7302e/artifacts/architecture-accepted/20261001T012656Z-6ff804df/pty-bench.log)
records 2.021 seconds, 31.7 MiB/s and 88.166 ms cleanup. This run overlapped native
verification on the host, so it is transport correctness evidence and is not an
uncontended throughput baseline.

## Retained-memory growth probe

[`measure-memory.py`](../scripts/measure-memory.py) exercises the real Linux/X11
desktop in fresh storage. Each case uses one `/bin/sh` session, 10,000 history
rows, a 1180×760 logical-point window and cursor blinking disabled. Build only
the two native inspection targets:

```sh
cargo build -p neptune-terminal --release --features inspection --locked --bin neptune --bin neptune-inspect
python3 scripts/measure-memory.py --repeats 3 --output artifacts/memory
```

The terminal case parses six batches of 30,000 synthetic lines, verifying a
shell-created completion marker and parsed-byte counters before each sample.
The explorer case creates 20 folders with 10,000 empty files each, opens and
collapses every folder, samples after each four folders, closes the panel and
verifies reopening. It captures the actual native Files screen, including a
640×400 window. Fixtures and captures contain only synthetic content. Runs retain
failures and verify complete session teardown before marking a case passed.
Use `--scenarios explorer-close` to measure hiding the panel with a 10,000-item
folder still expanded, separately from repeated folder browsing.

Each sample reports the median of five Linux `/proc` readings after a settling
interval: RSS, proportional set size (PSS), private clean plus dirty memory and
the process's lifetime peak RSS. These are process observations; they exclude
shell/child memory and GPU memory, and allocator retention can keep RSS high
after objects are released. A steady plateau after filling history is distinct
from growth proportional to the number of folders visited. Collapsed explorer
branches release their listings, hidden trees release their rendered rows, and
late listing replies cannot restore either cache.

For comparisons, retain the original release executables and use `--app` and
`--client` to select them. Reuse the synthetic directory printed under the first
run's output with `--fixture PATH` so both versions see identical paths and
files. Compare repeated cases with the same settings, display, fixture, script
and settling interval. Stop task-owned compilation and other probes while
sampling, and record competing work on the host.
Binary, script and lockfile hashes, compiler, platform and sample records are
saved in `report.json`; raw diagnostics exclude terminal contents. The probe's
accounting adapter has a focused check:

```sh
python3 -m unittest discover -s scripts -p test_measure_memory.py
```

## Remaining measurements

PTY-to-screen throughput, native input-to-screen latency, GPU presentation-time
distribution and GPU memory remain unmeasured. The Linux scenario probe observes
startup, scaling, application frame CPU, process CPU/RSS and inspection-command
latency separately. Repeat controlled observations on real Linux, macOS and
Windows hardware before claiming release-level parity.
