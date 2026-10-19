# Development guide

Neptune uses `egui`/`eframe` with `wgpu`, `alacritty_terminal` for terminal state, and `portable-pty` for Unix PTYs and Windows ConPTY. Its terminal and chrome remain native; optional [browser previews](browser.md) run in an isolated CEF helper. This repository is an initial implementation: Linux is the local verification platform; macOS and Windows need native runtime verification before a production release.

[Project overview](../README.md) · [Build and run](installation.md#build-and-run)

Commands in this guide run from the repository root.

- [Development builds](#development-builds)
- [Verification](#verification)
- [Native inspection](#native-inspection)
- [Performance and Ghostty](#performance-and-ghostty)
- [Continuous integration](#continuous-integration)
- [Verification artifacts](#verification-artifacts)
- [Release artifacts](#release-artifacts)
- [Website](#website)

## Development builds

The repository defaults to two Cargo build jobs per invocation so agents working
in parallel leave capacity for the desktop. To cover existing worktrees before
they receive the repository configuration, set the same limit in your user Cargo
configuration (`~/.cargo/config.toml`, or `$CARGO_HOME/config.toml` when set),
keeping existing configuration entries:

```toml
[build]
jobs = 2
```

## Verification

See [the verification record and release checklist](verification.md) for test scope, native screenshots, and platform checks still required for distribution. Local development uses the narrowest meaningful affected crate/target tests; CI owns workspace-wide formatting, lint, tests and builds. Run repo-wide checks locally only when explicitly requested. The owning [agent guides](../AGENTS.md) provide scoped commands.

Use the [isolated native harness](#native-inspection) below for interactive review. For targeted framebuffer capture and Linux/X11 OS-input smoke testing, follow [the native verification procedure](verification.md#native-application-verification), including a fresh data root and the exact owned window/inspection endpoint.

## Native inspection

Developer inspection is opt-in through the `inspection` feature. `neptune-inspect` exposes the native accessibility tree and supports `screenshot`, `key`, `text`, `click`, `context` (secondary click), `double-click`, `drag`, `press`, `release`, and `resize` commands for repeatable native visual review. The normal release binary has no inspection listener. Independent launches must follow [the run-isolation rules](../scripts/AGENTS.md).

Inspection protocol v1 does not carry a viewport ID. With multiple native windows,
requests and replies can come from different viewport passes, so the tree and
screenshot do not reliably describe the same window. Review detached windows
with native OS input and screenshots targeted at the exact task-owned window ID;
on Linux/X11, pass that ID to the [native verification tools](verification.md#native-application-verification).

The native integration exposes a window handle only for its original viewport.
File and folder pickers requested by additional windows therefore open without
a native parent. The supplemental Linux file-drag listener also belongs to the
original native surface. X11 child windows use toolkit file-drop events; native
Wayland child-window file drops require additional toolkit support and are
currently unavailable. Workspace tear-off keeps the pressed native surface with
the moving workspace so its compositor move can use the original pointer press.

On native Wayland, `platform/wayland_seat.rs` holds one data device on the
root window's connection for both file drops and workspace drags. Compositors
give drag-and-drop only to a client's newest data device, and the toolkit's
clipboard creates one for every window, so the seat renews its device after
each new native window. A workspace held out of a sidebar is carried as an
`xdg_toplevel_drag_v1` transfer that moves its new window under the pointer;
`app/windows/carry.rs` docks and tears it off as the compositor reports the
pointer over Neptune's windows. This needs each child window's toplevel and
surface, which the [patched eframe](../vendor/eframe/NEPTUNE-PATCH.md) exposes
through `eframe::viewport_window`. Wayland windows cannot be hidden, so when the
root window's own workspaces close, it shows another window's workspaces
instead and that window's native window closes. `native-workspace-windows.py`
covers X11 only; Wayland tear-off and rejoin were checked by hand with real
input on GNOME (Mutter 50).

On macOS, `platform/workspace_drag.rs` queries the current Quartz pointer and
primary button only during a workspace gesture. The existing drag repaint
deadline drives those queries, so no event tap or background observer runs while
idle. Pointer and window positions use the same top-left desktop screen points;
Retina density is kept out of that conversion, while app zoom still applies.
The pressed native surface stays with the workspace as it docks and tears off
again. An emptied root adopts a surviving window's workspaces and geometry, so
there is no spare hidden root window. The focused macOS regression filter is:

```sh
cargo test -p neptune-terminal --lib --features inspection --locked -- a_macos_
```

Those tests cover the desktop coordinator and read-only Quartz sampling, not
real pointer delivery or visual acceptance. Native dragging, stacking and the
Dock/window menu still require running-app review on macOS.

Workspace tear-off has a focused Linux/X11 workflow using actual pointer and
keyboard input, a fresh data root, and native windows matched to the exact owned
process PID. It measures the bootstrap window's display scale before converting
logical UI points to native X11 pixels:

```sh
python3 scripts/native-workspace-windows.py --output artifacts/native-workspace-windows
```

Run it sequentially with other desktop input tests. It checks held drags from
the original and a child window, unchanged live shell identity, closing the
original native window while a sibling stays usable, and moving a workspace
back into an existing window. Inspect its native captures, including the row
menu and destination submenu, detach/rejoin/quit palette commands, narrow
detached window and close confirmation. Inspection is used only before the first
tear-off; later input and screenshots target exact X11 window IDs. History
markers in the captures need visual review. The harness focuses its owned native
window and calibrates actual XTest pointer motion before pressing. Startup motion
retries are bounded and recorded; clicks require a validated row position. For a
second monitor at the same scale, pass a
physical X11 desktop point and monitor rectangle chosen from that host's layout:

```sh
python3 scripts/native-workspace-windows.py --drag-to-native-point X,Y --drag-to-native-region LEFT,TOP,RIGHT,BOTTOM
```

That optional first tear-off requests motion to the supplied point and verifies
that the same held native window's center enters the supplied monitor rectangle.
It records actual window geometry and pointer samples, captures the held window,
then moves back and waits for its center to leave that rectangle. Pointer queries
can lag during the window manager's grab, so they serve as diagnostics rather
than exact position assertions. Return motion uses the adapter's last requested
point rather than the possibly stale query. The harness does not discover
monitors or verify mixed-DPI scale transitions. This workflow does not establish
Wayland, macOS or Windows correctness.

Native regression runs own their process, temporary data directory, and inspection
endpoint; they never use your saved workspaces:

```sh
cargo build -p neptune-terminal --features inspection --locked --bin neptune --bin neptune-inspect
python3 scripts/native-harness.py --restore --output artifacts/native-review
```

Use `--diagnostics` for JSON counters with pane/session generations, resource
reservations, snapshot work, and frame p50/p95/p99. Terminal contents are excluded.
`python3 scripts/measure-scale.py --help` describes reproducible many-pane runs.

## Performance and Ghostty

PTY reading and terminal parsing happen outside the UI thread. History is bounded and the renderer consumes the visible viewport. Performance work focuses on lock duration, repaint coalescing, cached text layout, idle CPU, and sustained output. Benchmarks must use release builds. No claim of being faster than Ghostty or Alacritty is made without reproducible end-to-end measurements. See [measured performance and reproducible probes](performance.md).

Ghostty's public `libghostty-vt` is a possible future engine, but it supplies terminal state and render-state extraction rather than GPU drawing. Its API is still unstable. The full Ghostty surface API is internal and tailored to its macOS application, so it is not the current cross-platform renderer. See [the architecture decision and release gates](architecture.md) for primary sources, the migration plan, and the verification matrix.

## Continuous integration

GitHub Actions runs CI on pull requests and every push to `main`, including after a PR merges. PR runs check out GitHub's test merge commit, combining the PR with its base branch when the event triggers; pushes to `main` test the resulting commit. Change classification compares PR merge commits with their first parent and pushes with the previous `main` tip, covering the entire pushed range for merge, squash, and rebase merges. After change classification, fast formatting/architecture validation, the native platform matrix, and latest-stable compatibility run in parallel; documentation/evidence-only changes skip compilation within successful jobs. Default and inspection configurations are tested on Linux, macOS, and Windows. Linux also runs Clippy and isolated X11 inspection/restoration using the same build, while a separate job checks all targets/features with latest stable Rust. CI uses an unoptimized profile without debug symbols and caches dependencies even after failed runs; performance measurements and releases use optimized builds.

CI uses read-only repository permissions and a timeout for every job. New commits cancel older CI runs for the same PR; each push to `main` runs independently. The final `Check` status succeeds only when every CI job succeeds, rejecting failed, cancelled, or skipped jobs. The `main` ruleset requires this single `Check` status from GitHub Actions.

The active `main` ruleset requires only a pull request and passing CI. It does not require approvals, resolved review threads, a merge queue, or the PR branch to be up to date before merging. CI runs again on the resulting `main` commit after the merge.

## Verification artifacts

`artifacts/` is ignored local output for screenshots, reports, logs and isolated
test data. Verification scripts create their output directories as needed; the
folder does not need to exist in a fresh checkout. Keep generated files out of
source control.

Historical verification links point to the [evidence archive](artifact-archive.md)
at a fixed Git commit. New PR evidence uses the
[GitHub attachment workflow](agents/issue-tracker.md#pr-attachments). CI uploads
its native run as a Linux inspection artifact; download it from the relevant
Actions run. Local output can contain terminal captures and test state; review
files before uploading and retain the build/platform metadata needed to assess
the evidence.

## Release artifacts

Official desktop releases are deliberate SemVer tags, never normal merges to `main`.
[CHANGELOG.md](../CHANGELOG.md) owns What's New; the release workflow copies its
version section into GitHub Release notes and authenticated update metadata.
Native runners package macOS ARM64/Intel signed and notarized DMGs, a Windows x64
per-user EXE installer (currently unsigned), and Linux x64 AppImage/DEB.
All five artifacts must succeed before a draft is staged with SHA256SUMS,
Ed25519-signed update metadata and GitHub provenance attestations. Publication
is manual after review and native acceptance.

[The release guide](releases.md) contains exact version/tag commands,
GitHub environment secrets, one-time Apple setup, download verification and
failure recovery. AI agents must not create/push release tags or create/publish
Neptune releases unless explicitly asked to release that version.
Packaging automation does not replace the native/platform acceptance gates in
[docs/verification.md](verification.md).

Published downloads and AppImage launch instructions are in the
[installation guide](installation.md#downloads). Desktop update preferences are
in the [user guide](usage.md#updates).

## Website

The landing page and downloads for [neptune.rs](https://neptune.rs) live in [`website/`](../website/README.md): a Vercel-hosted Next.js site with server-cached GitHub release discovery. Its window demo follows the app's theme and layout code. It is separate from the Cargo workspace and uses Bun.

## File-location and hint captures

File locations, keyboard hints and the installed-editor setting use a fresh
native window and task-owned storage for each capture:

```sh
NEPTUNE_HINT_CAPTURE="$PWD/artifacts/native-hints.png" \
  cargo test -p neptune-terminal --lib app::tests::capture_file_locations_native --locked -- --ignored --nocapture
```

`NEPTUNE_HINT_SCREEN=explorer` opens the printed location at line 42;
`preferences` shows the searchable setting, `preferences-general` shows General,
and `preferences-menu` opens the installed-editor menu. `palette` shows the
keyboard entry point. `NEPTUNE_HINT_NARROW=1` uses a
640×400 window and `NEPTUNE_HINT_SPLIT=1` adds a second pane.
`NEPTUNE_HINT_SCREEN=link` reviews the modifier-hover underline. Each launch
exits after capture and removes its temporary data root. These captures use real
PTY output and GPU rendering with application events; OS modifier-click,
keyboard entry, the terminal context menu and clipboard ownership need a
separate native input check.

## Subscription usage captures

Review the footer and dropdown with quota fixtures in a real native window:

```sh
NEPTUNE_USAGE_CAPTURE="$PWD/artifacts/native-usage.png" \
  cargo test -p neptune-terminal --lib app::tests::capture_usage_native --locked -- --ignored --nocapture
```

`NEPTUNE_USAGE_SCREEN` selects `open` (default), `closed`, `empty`, `loading` or
`stale`. `NEPTUNE_USAGE_NARROW=1` uses 640×400; `NEPTUNE_USAGE_LIGHT=1` selects
Light and `NEPTUNE_USAGE_SPLIT=1` opens two terminals. Each capture owns fresh
storage and exits after saving the screenshot. The fixtures verify presentation;
use an isolated inspection launch to check real account reads, refresh and input.
