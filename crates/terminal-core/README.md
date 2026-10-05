# Terminal core

The native session layer uses `alacritty_terminal` 0.26 for VT emulation and
`portable-pty` 0.9 for Unix PTYs and Windows ConPTY. It runs real child processes.

`TerminalSession::spawn(SessionOptions, Repaint)` creates a session. Its public
API provides byte input, bracketed paste, resize, bounded scrollback, metadata,
revision counters, bounded clipboard/notification events, selection, and budgeted regex search.
`viewport()` returns project-owned immutable cells, colors, cursor, modes and
selection coordinates; its revision is captured under the grid mutex. No backend
re-export or mutable lock is public. The adapter copies damaged rows and shares
unchanged rows with `Arc`, while the renderer owns shaping and painting caches.
Scroll/reflow/geometry/color changes fully invalidate the required rows. Cache
rows until `revision()` changes; avoid capturing complete history on frames.
Call `acknowledge_repaint()` once at the beginning of each UI frame for visible
sessions, **before** reading their metadata, revisions, or grids. Output wakeups
coalesce until that acknowledgement; every parsed batch still advances its
revision. The first change after acknowledgement immediately requests a frame.
Lifecycle, errors, title, working-directory and bell notifications bypass grid
coalescing. A newly nonempty event queue also schedules a frame, including for
hidden sessions. Acknowledge hidden
sessions when they become visible. Acknowledging after a snapshot can lose a
change that arrived between that snapshot and acknowledgement.

`SessionMetadata::reported_cwd` retains the last valid OSC 7 or OSC 9;9
directory separately from `cwd`, which can also come from local process polling
(Linux only). Remote clients use the explicit report because their local process
directory does not identify the remote shell's directory. Each fresh session
starts with no report. On Windows, a local `cmd` or PowerShell started without
explicit arguments reports OSC 9;9 from each prompt: cmd through a prefixed
`PROMPT`, PowerShell through `-NoExit -Command` wrapping the profile's prompt.
PowerShell's location is not its process directory, so polling cannot replace it.
Because any program's output can claim a directory, Windows UNC and device paths
are refused so a report cannot make later splits or restores reach a network share.

Each session has a reader, writer, and parser/process worker. The reader uses
pooled 64 KiB buffers and an eight-message output channel. Parsing releases the
grid mutex between 16 KiB slices. Input accepts at most one MiB per call and two
MiB pending across at most 64 messages. Clipboard delivery holds at most 64
notifications and one MiB of payload, with a 64 KiB per-event maximum. Overflow
drops oldest notifications, oversized events are discarded, and metrics count
both. BEL uses a generic attention notification; OSC 9/99/777 process notifications
and lines an integration addresses to the application (`OSC 7717;neptune;…`,
at most 512 printable ASCII bytes, passed on unread) share this queue, and their observer bounds
sequences, text and incomplete chunks independently. See [notification protocol
scope](../../docs/notifications.md#protocol-scope-and-limits). Clipboard reads remain disabled; the application runtime consumes stores. Slow child processes apply PTY
backpressure instead of blocking the UI or accumulating unlimited output.
Synchronized application frames suppress redundant redraws and flush on the
parser's 150 ms deadline even if no additional output arrives.

Resize acquires the parser grid lock before notifying the PTY, then updates both
the grid and size-query metadata under that lock. A child responding immediately
to SIGWINCH therefore cannot redraw into stale grid geometry. If the input queue
rejects the resize, both dimensions remain unchanged.

OSC 133 semantic prompt markers enable prompt redraw handling on resize. The
engine records the exact grid row at `A`/initial `P`, follows that row through
scrolling by its allocation identity, and clears the live redrawable prompt
before reflow. It retains the shell's vertical cursor distance while the shell
redraws its own prompt and input buffer after SIGWINCH. Completed command output
and history keep their normal reflow; `C`/`D`/terminal reset end the active prompt.
Alternate screens and `redraw=0` are preserved. Tracking uses no sentinel cell
attributes and searches rows only when resizing. Unmarked shells keep ordinary
VT reflow; the engine cannot identify their live prompt safely. Markers inside a
pending synchronized frame are conservatively ignored until a subsequent mark
can identify an actual grid position.

The default history is 10,000 lines. Dimension/history changes must fit the
16,000,000 logical cell budget, including both visible grids. This is a logical
allocation guard, not a strict resident-memory cap: cell extras, parser buffers,
and allocator capacity are additional memory.

`SearchQuery::compile` runs once per query; `begin_search` and `search_step`
scan with row/time budgets. Each row is copied under its own short grid lock;
regex matching happens after releasing it. Requests are cancellable and reject
stale revisions. Unicode/wide-character coordinates and wrapped logical lines
are preserved in both directions. Logical lines above one MiB report
`LimitExceeded` explicitly instead of reporting no match.

`SessionError::kind()` distinguishes backpressure, closed sessions, invalid
geometry/options, spawn failures and invalid search queries while preserving
error details. Cumulative diagnostics record startup/cleanup time, queue
saturation, dropped events, snapshot/lock time and extracted rows/cells without
recording terminal contents. `TRANSPORT_BUFFER_BUDGET` reserves known transport
buffers (including VTE's two MiB synchronized-update allocation) separately from
the logical grid budget; allocator overhead and extras remain additional.

Shutdown requests return without joining workers. A cloneable
`shutdown_completion()` handle reports completion after all workers stop, even
when the session itself is released. Worker reservations exist before threads
start, so a rapid close cannot mistake unstarted workers for complete cleanup. Unix I/O uses nonblocking
descriptors and interruptible polling. Unix shutdown releases the output channel
and PTY master handles before reaping the child, so BSD terminal output draining
cannot wait on readers that have already stopped. Windows starts a short-lived close worker
after the child exits because `ClosePseudoConsole` can wait for final output to
drain. The reader and parser remain active until that output reaches EOF. Child
processes are reaped by their owning worker.
While output is flowing, child liveness is polled at most once per 100 ms rather
than on every PTY read; EOF still triggers an immediate exit check.

## Verification

```sh
cargo test -p terminal-core
cargo clippy -p terminal-core --all-targets -- -D warnings
cargo run --release -p terminal-core --example parser_bench
cargo run --release -p terminal-core --example pty_bench
cargo run -p terminal-core --example resize_probe
```

Unix integration tests exercise actual child input, kernel resize, terminal
replies, bracketed paste, synchronized-output timeout, and shutdown. A real zsh
regression (requires `zsh`, installed in Linux CI and shipped on macOS) checks a
multiline prompt with saturated 10,000-row history, wrapped input, and width and
height changes. The manual resize probe uses the host's configured zsh in `/tmp`
and reports geometry and prompt assertions without logging its terminal text.
Windows
integration tests spawn their own native executable inside ConPTY and check raw
console input, native window dimensions, sanitized paste bytes, final output
after natural exit, and closure during a large pending write. The ignored fixture
is invoked by the five active Windows tests and is not run directly by Cargo.

The parser benchmark processes 64 MiB per workload with 10,000 history rows. Its
timings include VT parsing and history management, and exclude PTY transport,
text shaping, GPU work, startup time, and input latency.

The Unix PTY pipeline benchmark requires `/bin/sh`, `stty`, and POSIX `awk` and
adds no Cargo dependencies. A ready handshake first confirms that the real child
has disabled echo and output newline translation. It then emits exactly 64 MiB
as 524,288 CRLF lines of 128 bytes into a 160 × 40 terminal with 10,000 history
rows. Timing starts before releasing the child producer, includes input dispatch,
awk launch and output production, native PTY transport, buffering, VT parsing,
and final output drainage, and ends after child exit and EOF parsing. Setup and
remaining worker teardown are reported separately and excluded from throughput.
The example verifies both byte counters and the final visible output marker.
Its simulated 120 Hz consumer acknowledges pending notifications before reads;
the repaint callback only increments an atomic counter. Reported callbacks are
requests rather than rendered frames. GUI scheduling, shaping, GPU work, and
screen/input latency are outside this benchmark. Non-Unix hosts report that the
fixture is unsupported.


### Close-time process checks

`TerminalSession::check_process_activity` returns a bounded, one-result receiver.
The existing engine worker services one coalesced pending request; it performs
no periodic process scan, reads no command lines/environment, and forces a wake
when the result is ready. A superseded request disconnects. Callers must bound
their wait and treat timeout/disconnection as `Unknown`, never as idle.

Linux and macOS compare the PTY foreground process group with the session
process group, then inspect children and the executable. Linux checks each
thread's `/proc/.../children` with thread/time bounds; macOS uses libproc.
Windows uses a bounded Tool Help process snapshot. A recognized shell with no
children is `Idle`; a different executable (including `exec` replacements),
foreground job or child is `Running`. Unavailable metadata is `Unknown`.
Observations are advisory: jobs can start/exit between inspection and close,
shell builtins have no separate process, and reparented/detached jobs are outside
the child tree. The desktop conservatively treats SSH as active.

Native contracts: `cargo test -p terminal-core --test session process_activity --locked`
on Linux/macOS; `cargo test -p terminal-core --test windows_conpty process_activity --locked`
on Windows. Linux execution alone does not validate macOS or ConPTY behavior.
