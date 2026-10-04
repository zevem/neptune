# Architecture and renderer decision

Neptune is a Rust desktop terminal: native windows, operating-system PTYs, a terminal state engine, and GPU rendering. The application uses `eframe`/`egui` with `wgpu`; no browser or webview hosts its interface. This is a cross-platform implementation, but release readiness and platform support must be established by the verification below rather than inferred from its dependencies.

## Why the first version does not embed Ghostty's renderer

Ghostty was researched on 2026-09-30 at upstream commit `76895d97b74ff6b24c2b1543bcd69ccc18048a4d`. There are two different APIs with different purposes:

| API | What it provides | Suitability for Neptune |
| --- | --- | --- |
| `libghostty-internal`, `include/ghostty.h` | Ghostty application surfaces, including renderer and platform integration | The header identifies the macOS app as its only consumer, discourages external embedding, and exposes macOS/iOS platform surfaces. It is not a portable Linux/Windows GPU surface interface. |
| `libghostty-vt`, `include/ghostty/vt.h` | Terminal parsing, state, scrollback, input encoding, and render-state extraction | A viable future terminal engine for Linux, Windows, and macOS. It does not draw glyphs or create windows, and its C API is explicitly unstable. |

These distinctions are explicit in [the internal embedding header](https://github.com/ghostty-org/ghostty/blob/76895d97b74ff6b24c2b1543bcd69ccc18048a4d/include/ghostty.h), [the VT header](https://github.com/ghostty-org/ghostty/blob/76895d97b74ff6b24c2b1543bcd69ccc18048a4d/include/ghostty/vt.h), and [the upstream README](https://github.com/ghostty-org/ghostty/blob/76895d97b74ff6b24c2b1543bcd69ccc18048a4d/README.md).

The official [Ghostling example](https://github.com/ghostty-org/ghostling/blob/main/README.md) demonstrates using the VT library with a separately implemented renderer and window layer. It requires Zig in addition to the host build tools. Upstream's [CMake integration](https://github.com/ghostty-org/ghostty/blob/76895d97b74ff6b24c2b1543bcd69ccc18048a4d/CMakeLists.txt) delegates to Zig and has additional static-link considerations for SIMD dependencies on Windows.

The initial engine is `alacritty_terminal`, with PTYs provided by `portable-pty`. These are Rust dependencies with published releases. This choice avoids committing the first release to an unversioned C ABI or to platform-specific Ghostty surfaces. It does **not** mean that Neptune inherits Alacritty's renderer or its benchmark results. [Alacritty's upstream documentation](https://github.com/alacritty/alacritty/blob/master/README.md) lists Linux, macOS, and Windows support. [WezTerm's PTY implementation](https://github.com/wezterm/wezterm/blob/main/pty/src/lib.rs) selects Unix PTYs or Windows ConPTY.

## Component boundaries

`neptune-model` owns ordered workspaces and folder groups, group and pane membership, stable group/pane/workspace/split identities, validated layouts, lifecycle, targeted commands and the optional SSH destination of a remote workspace. Groups are one level deep, retain empty folders and collapsed state, and share the workspace count limit. Group commands persist organization without restarting sessions; removing a group retains its workspaces. Workspace state version 8 stores folders, mixed sidebar order, tab layouts, optional per-pane coding-agent resume references with the pull requests an agent linked to its pane, and optional group default directories, and reads versions 1–7. A group default sets the first directory of new local workspaces; tabs and splits continue to inherit the source pane’s current directory. Group directory edits and native folder selection share one pending operation. Filesystem validation and waiting for the parented native picker happen off the UI thread; picking edits only the draft until Save. Existing sessions and remote directory semantics are unchanged. It depends only on `serde`. Model fields are private, and the pure controller returns effects; it opens no windows, shells or files.

`terminal-core` owns PTY sessions and terminal state. Each pane is an independent session. Its public contract contains project-owned input modes, coordinates, colors, selection, events, owned viewport snapshots and budgeted search. Alacritty types and mutable grid locks remain internal.

A remote workspace is still made of ordinary PTY sessions: the desktop starts the system OpenSSH client as each pane's process, with `-t`, the validated destination after `--`, and a quoted remote bootstrap command. Neptune implements no SSH protocol, stores no credentials and adds no dependency for it. The bootstrap runs on a Unix host after authentication, opens its login shell, and adds temporary OSC 7 directory hooks to Zsh without editing user configuration. A remote pane keeps its local SSH-client directory and its last reported remote directory separately in the model. `terminal-core` exposes the last OSC 7 directory independently of local process polling. Generation-tagged controller commands adopt those reports, including for hidden panes and immediately before shutdown; workspace state saves the optional `remote_cwd` of each pane, introduced in version 3, without local filesystem validation. Splits inherit the stable source pane's remote directory, and reconnect and restoration launch fresh login shells there through quoted startup arguments. Connecting to another host or disconnecting clears remote directories. Versions 1–3 remain readable; older builds protect version 4 from replacement.

The desktop library connects the controller to `runtime/sessions.rs`, the background persistence writer, platform services and `ui/` widgets. The session manager bounds startup concurrency and retains resource reservations for starting/closing sessions. `terminal_view/` prepares/caches/paints snapshots and returns interactions; painting receives no live session. `input.rs` normalizes egui events and delegates protocol encoding to terminal-core. `platform/` owns clipboard, fonts and window operations. The file explorer keeps its tree, search results and preview in `app/explorer.rs`: one worker lists folders, applies create/rename/delete and re-reads the folders in view on a two-second timer (reporting only changes), another reads previews, and each search walks on its own cancellable thread. `ui/explorer.rs` draws what they reported and emits events; `platform/files.rs` hands a file to the native file manager or its default application. Nothing of it is saved, and paths and file contents stay out of diagnostics. The explorer is one tab of the trailing panel; `ui/panel.rs` owns the panel's toggle, slide, width and tabs, and `app/panel.rs` its events. The other tab lists running CLI agents. What an agent is doing is desktop state, never the model's: hooks injected by `runtime/agents.rs` reduce each event to a closed signal (no prompt, command or result leaves the hook process), the bridge keeps the latest state per pane generation and open invocation, and `agent_activity.rs` reconciles it each frame with the terminal's title and reply keys, waking only for the next deadline. `ui/agents.rs` draws the rows; none of it is saved or sent to diagnostics.

`persistence/workspace_state.rs` owns independent versioned DTOs and the legacy migration fixture. Invalid entries produce contextual diagnostics. Recovery copies protect damaged/lossy state before replacement; unreadable and unsupported schemas disable workspace saves. `persistence/window_state.rs` stores the normal window dimensions in logical points and its maximized state independently of workspace restoration and settings; invalid, unreadable or unsupported window files remain read-only. One background writer accepts immutable workspace, config and window snapshots, coalesces each destination, debounces changes for 75 ms, uses atomic replacement and acknowledges saved generations. Intentional shutdown flushes the latest accepted snapshots with a bounded wait.

The display path is:

```text
shell/TUI ⇄ native PTY ⇄ terminal-core ⇄ owned viewport ⇄ terminal_view ⇄ egui/wgpu ⇄ native window
```

PTY startup, reading, VT parsing, configuration loading and persistence run off the UI thread. Rendering reads a visible snapshot; it never waits for a shell command to finish. Search compilation happens when the query changes, and scans use explicit row/time budgets with revision/query/session identity. This separation makes a future `libghostty-vt` adapter practical while preserving the workspace UI and PTY lifecycle. Such an adapter is planned, not implemented.

See [the development contract](../AGENTS.md), [desktop ownership](../src/AGENTS.md), [model ownership](../crates/neptune-model/AGENTS.md) and [core threading contracts](../crates/terminal-core/AGENTS.md) for change paths and focused checks. `scripts/check-architecture.py` enforces model dependencies, desktop backend isolation and snapshot-only rendering; Rust privacy enforces validated model mutation and sealed terminal state.

`eframe` supports native Linux, macOS, and Windows applications and a `wgpu` renderer; see [the framework documentation](https://github.com/emilk/egui/blob/master/crates/eframe/README.md). `wgpu` selects a backend available on the host, normally Vulkan on Linux, Direct3D 12 on Windows, or Metal on macOS; see [its supported-platform matrix](https://github.com/gfx-rs/wgpu/blob/trunk/README.md#supported-platforms). A GPU-backed framework alone does not guarantee low latency or high throughput: the terminal paint path must also be measured.

## Performance strategy

- Keep terminal I/O independent of rendering. Parse buffered PTY reads and coalesce repaint notifications rather than posting an event for every byte.
- Bound scrollback and output queues. Render only visible rows. Hidden sessions continue processing output without submitting their glyphs to the GPU.
- Keep terminal locks short. Copy or extract the visible state while locked, then shape text and paint after releasing the lock.
- Cache glyphs and row layout. Avoid rebuilding text layout or allocating a widget per cell on every frame. Invalidate caches on terminal changes, font changes, zoom, and DPI changes.
- Schedule frames from input, output, resize, and cursor deadlines. Idle terminals should sleep rather than continuously repaint.
- Use optimized release builds for performance measurements. Developer builds enable extra checks and are not comparable to shipped terminals.

Some of these are release requirements rather than guarantees of the initial implementation. Profiling must determine whether egui's text path is sufficient. If it dominates frame time, preserve egui for chrome and replace terminal painting with a dedicated `wgpu` instanced glyph renderer sharing the font atlas.

Ghostty's [render-state API](https://github.com/ghostty-org/ghostty/blob/76895d97b74ff6b24c2b1543bcd69ccc18048a4d/include/ghostty/vt/render.h) is a useful future integration point: it supports dirty rows, row identity, and a two-phase extraction flow that can release terminal access before deferred work completes. A future adapter must honor synchronized output holds and render-state lifetimes; polling the raw grid indiscriminately would lose those benefits.

## Verification and release gates

Record the host OS, GPU/backend, display scale, window size, font size, build profile, and dependency lockfile for every result. Measure both the parser and the complete PTY-to-screen path. Throughput numbers alone do not establish interaction latency.

| Area | Evidence required before a production release |
| --- | --- |
| Linux, macOS, Windows | Native release builds and interactive PTY smoke tests on each OS; ConPTY verification on Windows; Wayland and X11 verification on Linux |
| Terminal correctness | Real shells and TUIs, alternate screen, resize/reflow, Unicode widths, combining marks, true color, bracketed paste, mouse reporting, selection, search, and synchronized output |
| Lifecycle | Closing panes and the application releases PTYs and child processes; exited shells are represented accurately; failed spawn/resize/write operations are visible |
| Performance | Parser throughput, sustained output throughput, input-to-display latency, frame-time percentiles, idle CPU, memory at maximum scrollback, and behavior with many panes |
| Desktop quality | DPI changes, IME, clipboard, keyboard layouts, screen-reader navigation, window controls, packaging, signing, and update strategy |
| Visual quality | Screenshots of the running native application at several sizes and states, followed by design critique and corrections |

Kitty keyboard mode negotiation and press/release/repeat encoding are implemented and covered by protocol tests. Native keypad identity, lock-state modifiers, and some platform-specific key combinations still need dedicated verification. Kitty graphics, full text shaping/font fallback, and platform integrations should only be advertised when implemented and verified. Sessions restore workspace organization and start fresh processes. Recognized local coding agents can reopen provider-owned conversations by session ID through the [agent integration](agent-sessions.md); this does not preserve live processes or unfinished tool execution. Restoring a remote workspace opens new SSH connections; it does not resume the previous remote shells.

## Reconsidering libghostty-vt

Build a small proof of concept pinned to a specific Ghostty commit, with generated bindings hidden behind safe Rust ownership types. Validate Linux, macOS, and Windows build/link behavior, callback lifetimes, input encoding, resize, and incremental rendering. Compare it against the current engine using identical workloads before changing the default. Keep the Zig toolchain and vendored sources reproducible, and treat upstream API updates as explicit migrations.
