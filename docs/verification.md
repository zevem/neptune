# Verification and release readiness

Neptune has a running native Linux implementation with real PTY sessions, automated
engine and application tests, and iterative native screenshot review. Linux is
the local verification platform. macOS and Windows build/test jobs are configured
in CI; their existence does not establish a successful CI run or desktop runtime
verification. A production release requires the remaining gates below.

Historical captures and raw reports under `artifacts/` retain the branding,
paths and executable names of the build they actually measured. They are
evidence of those builds; Neptune's rename does not rewrite past evidence.

## Local proof and CI coverage

Local changes use the narrowest meaningful affected crate/target checks and
focused behavior tests. See [desktop guidance](../src/AGENTS.md),
[model guidance](../crates/neptune-model/AGENTS.md),
[terminal guidance](../crates/terminal-core/AGENTS.md) and
[script guidance](../scripts/AGENTS.md) for scoped commands. Cross-boundary work
checks each changed seam. Documentation-only edits need reference/consistency
review rather than Rust builds.

**Do not run repo-wide checks locally unless explicitly requested. CI owns the
full suite.** The commands below document full-suite reproduction for such a
request; they are not a local handoff checklist. The actual CI matrix and commands
are maintained in [ci.yml](../.github/workflows/ci.yml).

Unix PTY tests require `/bin/sh` and `zsh`. Install `zsh` on Linux if it is
absent; the multiline prompt regression uses an isolated shell configuration.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
python3 scripts/check-architecture.py
python3 scripts/check-architecture.py --self-test
cargo build --release --locked --bin neptune
```

Rust 1.97.1 is pinned by `rust-toolchain.toml` and is the declared minimum for all
packages. CI tests default and inspection configurations on that baseline on
Linux, macOS (Apple Silicon) and Windows. Apple Silicon is the primary macOS
target; native Intel macOS build/test coverage runs on release tags through
[release.yml](../.github/workflows/release.yml).
Latest stable gets an all-target/all-feature compile
check rather than a second copy of the Linux test suite. Formatting and
architecture checks precede compilation; documentation/evidence-only PRs skip
the build matrix while retaining successful required statuses. The `ci` Cargo
profile disables optimization/debug symbols, and dependency caches are saved on
failure. Linux native inspection/restoration reuses the matrix job's compiled
dependencies instead of starting a separate cold GUI build. These checks are
correctness evidence; performance measurements still require release builds.
Current coverage includes:

| Boundary | What is exercised |
| --- | --- |
| Real Unix PTY | Input, parser replies, resize, exit status, idle shutdown, escalation of a shell ignoring SIGHUP, input/grid limits, bracketed paste, synchronized-output timeout, and real-zsh multiline prompt/input preservation |
| Resize and backpressure | Native resize waits for terminal-grid access; a rejected resize preserves existing grid dimensions when the input queue is full |
| Terminal input | Ctrl combinations, application cursor mode, modified/function keys, Unicode text ownership, Kitty key flags/repeat/release, SGR/legacy mouse coordinates, and motion modes |
| Renderer cache | A real quiet shell and headless egui frames retain galley identity when cells are unchanged; one changed row rebuilds once; wide/combining Unicode search highlights use terminal columns; a display-scale change rebuilds glyph layouts at unchanged grid dimensions and subsequent frames reuse them |
| Custom controls | A right-aligned titlebar icon retains its widget identity across hover/press/release and clicks once; hiding a neighboring control preserves the focused action, so Enter cannot activate a different button; switches, segmented choices, sliders and steppers change once per interaction and respect their bounds; a new dialog gives its first field the keyboard once visible; command-palette entries capture their pane and filter by query; dragging a sidebar row moves its workspace once on release, scrolls a long list from its edge, leaves a click selecting, and is cancelled by Escape without passing the key on |
| Configuration and layout | Resource validation, serialization, atomic replacement, failed/concurrent writes, saved split validation, split removal, and Unicode-safe labels |
| Pure application controller | Targeted commands, workspace order/identity and reordering, closing before/at/after focus, stable split identities, tabs (opening, focus bringing a tab into view, closing, reordering, joining and leaving a place), pane moves within and between workspaces (rearrange, unchanged drops, arrival as a tab, last-pane removal, capacity and atomic rejection), failed/stale startup, restart at capacity, fake-runtime effects and dirty/save acknowledgements |
| Persistence compatibility and scheduling | Unversioned fixture migration, independent versioned DTOs, invalid/missing directory recovery, corrupt-file preservation, unsupported/unreadable write protection, coalescing, destination generations, slow/failing storage, bounded flush and ephemeral isolation |
| Architecture | Forbidden model dependencies (including target tables/aliases), desktop backend/lock leaks and production renderer live-session leaks; checker self-tests verify rejection |
| Asynchronous bootstrap | Bounded pending actions replay onto restored state; preference changes coalesce; loader failure preserves storage; launch commands retain their original pane/generation and are delivered once to a real PTY despite focus changes |

Renderer tests include synthetic project-owned snapshots across platforms and a
smaller real-PTY integration set. They do not submit GPU work. Unix PTY fixtures do not run on Windows. Dedicated ConPTY
fixtures are provided for Windows and require execution there; they are not
covered by a Linux test result.

### Shell prompt resize coverage

The engine observes OSC 133 semantic primary-prompt markers alongside the VT
parser. On resize it clears an identified redrawable active prompt before the
shell redraws it, preserving command output and history. Output markers end that
active region; continuation/right-prompt markers, `redraw=0`, and alternate-screen
content retain their separate semantics.

The automated real-zsh fixture uses an isolated `.zshrc` with OSC 133 hooks and
a synthetic multiline/right-aligned prompt. It fills the 10,000-row history cap,
types a long unsubmitted command, resizes between 100 and 33 columns and between
eight and four rows, then verifies prompt count, the typed buffer, and executed
command/history preservation. This targets actual ZLE redraw behavior without
depending on a developer's theme installation.

Local interactive screenshots additionally exercise the development host's real
zsh configuration. Its `.zshrc` sources `~/powerlevel10k/powerlevel10k.zsh-theme`
and `~/.p10k.zsh`, and contains existing BridgeSpace `precmd`/`preexec` hooks that
emit OSC 133 A/C markers for interactive shells. The final native workflow
preserved an actual 330-byte typed payload across 900×640 and 640×480 windows,
then verified its exact successful execution without clearing the terminal.

That observation verifies this sourced configuration; it does not mean Neptune
installs shell-integration hooks. OSC 133 coverage does not establish universal
resize behavior for every shell, theme, asynchronous-output pattern, or
shell-integration implementation.

## Native application verification

Build the release executable before measuring or accepting final screenshots.
Capture the application's native framebuffer with a one-shot launch:

```sh
target/release/neptune --data-root /tmp/neptune-capture-UNIQUE --no-restore --screenshot artifacts/native-main.png
```

Replace `/tmp/neptune-capture-UNIQUE` with a fresh task-owned directory. This mode
captures after three seconds and exits without saving workspace state.
It is useful for the default view; interactive states need a running application.
The preferred repeatable UI workflow is
[`native-harness.py`](../scripts/native-harness.py), which owns a unique endpoint,
fresh `--data-root`, process, logs, artifacts and cleanup. It invokes the semantic
[`inspect-regression.py`](../scripts/inspect-regression.py) workflow and carries
the same endpoint/data directory through restoration. Widgets expose stable
accessible pane and field labels. Build the optional developer tools and run:

```sh
cargo build -p neptune-terminal --locked --features inspection --bin neptune --bin neptune-inspect
python3 scripts/native-harness.py --output artifacts/native
```

Each run writes `run.json`, `app.log`, `ui-regression.json` and captures beneath
its unique output directory. Native readiness and semantic conditions use bounded
polling. The full shell workflow currently requires a POSIX shell; OS key/pointer
injection remains in the explicitly Linux/X11 adapter.

For an independently managed manual launch, supply a fresh `--data-root` and
unique loopback `EGUI_INSPECTION=HOST:PORT` to Neptune, then carry both to the script.
Any explicit `--config` must use task-owned storage too. Track the process at
spawn and stop only that process; do not find processes to kill by name/path.
`--config` or `--no-restore` alone does not isolate workspace writes:

```sh
python3 scripts/inspect-regression.py --addr HOST:PORT --data-root PATH \
  --output artifacts/native-regression --restore
```

The harness requires Python 3 and the inspection binaries. It checks native
field values, persisted workspace/theme state, independent interactive terminal
bodies, shell command markers, delivery of a literal Tab to the foreground
process with the terminal keeping keyboard focus, the terminal context menu and
its dismissal, sidebar resizing and reset from its edge, the Preferences content
viewport, search, and close/cancel behavior. It captures right/below splits, the terminal menu, Graphite/Light
preferences, and full-size/narrow windows. The harness verifies restoration by default and
cleans up its owned process. `--no-restore-check` skips that part. A manually
managed regression without `--restore` leaves its application running after
cancelling close.

Accept a run only when its `ui-regression.json` has `status: "passed"` and each
recorded check passed. A failed or interrupted run remains diagnostic evidence.
Record the exact executable/build, window size, desktop scale, and host alongside
the screenshots. Developer inspection proves application event routing and
native rendering; use the normal release build for performance measurements.

### Operating-system input smoke tests

[`native-smoke.py`](../scripts/native-smoke.py) complements inspection by sending
real X11 keyboard/pointer events and capturing the desktop window. It requires
Python 3, Pillow, libX11, and libXtst, and never starts the application. Launch
your isolated Neptune instance through X11/XWayland (unset `WAYLAND_DISPLAY` only
for that child), identify its exact window ID, and pass it to every invocation:

```sh
python3 scripts/native-smoke.py --window-id 0xWINDOW info
python3 scripts/native-smoke.py --window-id 0xWINDOW key ctrl+shift+t
python3 scripts/native-smoke.py --window-id 0xWINDOW snapshot artifacts/native-keyboard.png
```

Replace `0xWINDOW` with the owned window ID, not a title match. Run OS-input
workflows sequentially; desktop focus is global across tasks.

The older coordinate-based `ui-regression.py` is retained for diagnostics;
its modal offsets require review after layout changes. Prefer the semantic
inspection workflow for repeatable UI regression. X11 automation does not
verify the Wayland input path.

### Normal-release runtime evidence

The normal Linux release was also exercised with native X11/XTest input at 2×
display scale. The run opened `nvim --clean -R README.md`, paged with Ctrl+F,
moved with Down, resized from 1180×760 to 900×640 logical points, and quit with
`:q`. The alternate screen returned to the existing shell, and a subsequent
`printf` command produced a file whose exact sentinel content was checked.
The [full-size TUI](../artifacts/native-nvim.png),
[resized TUI](../artifacts/native-nvim-resized.png), and
[returned shell](../artifacts/native-tui-return.png) captures show aligned grid
content, preserved native status lines, and clean font/icon rendering.

The [final normal-release workspace](../artifacts/native-final.png) combines a
read-only Neovim session, a ready shell after `ls src`, and actual parser-benchmark
output. Its three-pane hierarchy, consistent header spacing, and restrained
lavender marker clearly identify the focused top-right shell without competing
with terminal content. This capture passed the final designer review.

A separate normal-release Wayland run at 1.5× scale used the Vulkan/NVIDIA
renderer and displayed real shell output from ten passing terminal-core tests.
Its [native capture](../artifacts/native-wayland.png) verifies launch, terminal
output, and rendering at that scale. This smoke test does not establish complete
Wayland keyboard, pointer, clipboard, or IME coverage; the broader platform and
desktop-integration gates below remain open.

### Optional native inspection

The developer client also supports ad hoc native screenshots, logical-coordinate
input, Unicode text injection, and AccessKit tree inspection. With the application
running as above, use another terminal:

```sh
target/debug/neptune-inspect --addr HOST:PORT info
target/debug/neptune-inspect --addr HOST:PORT tree
target/debug/neptune-inspect --addr HOST:PORT settle
target/debug/neptune-inspect --addr HOST:PORT screenshot artifacts/inspection/main.png
```

Inspection screenshots use one pixel per logical point. X11 window captures use
native desktop pixels, so their dimensions differ at high display scale. The
normal release binary does not include the inspection listener. Injected events
verify application routing; they do not replace operating-system keyboard,
clipboard, IME, or screen-reader checks.

## Architecture refactor verification: 2026-09-30

On Linux with Rust 1.97.1, the completed refactor passes **125 default-feature
Rust tests** and **127 inspection-feature Rust tests**. Formatting and both
Clippy configurations pass with warnings denied. Architecture checking and its
three rejection self-tests pass, along with the Python harness/metric tests.
The Rust totals comprise 14 pure-model tests, 61 desktop tests, 37 terminal-core
tests, 13 Unix PTY tests and, with inspection enabled, two client tests.
The [default test log](../artifacts/architecture-accepted/20261001T012656Z-6ff804df/test-default.log)
and [inspection test log](../artifacts/architecture-accepted/20261001T012656Z-6ff804df/test-inspection.log)
retain the final results.

The final optimized inspection build passed **18 native semantic checks** and
produced **23 screenshots** in the
[accepted native run](../artifacts/architecture-accepted/20261001T012656Z-6ff804df/ui-regression.json).
The [run metadata](../artifacts/architecture-accepted/20261001T012656Z-6ff804df/run.json)
records compiler, platform, binary/lockfile hashes, endpoint and isolated state.
The workflow exercised independent shell processes, Ctrl+C, theme/preferences,
search ownership, close cancellation, graceful save/relaunch restoration and
exact preservation/execution of a 330-byte typed payload through two resizes.
Fresh three-pane and narrow-window captures were visually inspected. Events
came through native egui inspection; this is separate from OS keyboard/IME
coverage.

The ordinary release build also rendered a native screenshot, executed an exact
launch-command marker, left ephemeral workspace storage untouched and ignored
`EGUI_INSPECTION` (no listener), using both a controlled `/bin/sh` configuration
and the host's default zsh. See the
[ordinary-build record](../artifacts/architecture-accepted/20261001T012656Z-6ff804df/normal-release.json)
and [zsh record](../artifacts/architecture-accepted/20261001T012656Z-6ff804df/normal-zsh-release.json).
An initial host-zsh attempt contained an extra character before the command;
that capture is retained as `normal-zsh-first-attempt.png`. The same invocation,
three additional native launches and a direct core-session probe subsequently
passed without code changes. Its origin was not established; deterministic
automated launch-command coverage uses isolated controlled shells.

The optimized scaling build passed **19 applicable scenarios** across
1/8/32/64 panes, with the one-pane hidden-output case marked inapplicable.
Every accepted case confirmed complete session teardown. Output cases include
startup markers, parsed-byte verification and an activity audit; the 64-pane
several-output case was rerun after its first producers ended too soon.
The [accepted scenario summary](../artifacts/architecture-scale-accepted/accepted-summary.json)
links the original and replacement records. See [performance.md](performance.md)
for workload and measurement limits. These runs preceded the final failure-only
diagnostic additions; their recorded executable hashes identify the measured
build independently of the final ordinary executable.

After the final diagnostic/error-handling changes, the rebuilt ordinary release
passed [two targeted native smokes](../artifacts/architecture-accepted/20261001T012656Z-6ff804df/final-smokes/report.json):
successful exact command execution and an unavailable-shell failure. The latter
verified a visible error, a structured `Spawn` category with pane/generation,
complete teardown and no execution of the pending launch command. Both confirmed
inspection was disabled and ephemeral storage remained unchanged.

## Interface rebuild verification: 2026-09-30

The interface was rebuilt on this date; [design.md](design.md) records the
direction and the corrections made during review. Evidence is under
[`artifacts/ui-rebuild`](../artifacts/ui-rebuild).

- **Focused tests.** The desktop library suite passes 85 tests on Linux with
  Rust 1.97.1 (`cargo test -p neptune-terminal --lib --locked`), and the
  inspection client's two tests pass. Clippy with warnings denied passes for the
  desktop package's targets with and without `inspection`; rustfmt and the
  architecture boundary check pass. The workspace-wide suite, `neptune-model` and
  `terminal-core` tests were not rerun locally; neither crate changed.
- **Native regression.** The optimized inspection build passed 24 semantic
  checks and produced 25 captures in the
  [accepted run](../artifacts/ui-rebuild/20261001T034328Z-2d49bbe2/ui-regression.json);
  its [metadata](../artifacts/ui-rebuild/20261001T034328Z-2d49bbe2/run.json)
  records the binary and lockfile hashes. The run used X11 through the harness.
  New checks cover Tab delivery with the terminal keeping focus, the terminal
  menu and its dismissal, and sidebar resize and reset. An
  [earlier run](../artifacts/ui-rebuild/20261001T033954Z-4724214b/ui-regression.json)
  failed at the sidebar reset: the handle accepted only an exact double-click,
  and the toolkit counted the clicks as a triple because another click had just
  happened. Both resize handles now accept either. That run is retained as
  diagnostic evidence.
- **Captures.** The run's captures and separate manual captures of states the
  harness does not reach were inspected. Capture 17 was taken while two
  restored shells were still drawing their startup prompt; a resize trace of a
  separate restoration showed both resizes of every pane within 34 ms of launch
  and none later, and capture 18 shows the settled prompts.
- **Native Wayland.** The ordinary release build rendered
  [a one-shot capture](../artifacts/ui-rebuild/wayland-release.png) at 1.5×
  scale with transparent window corners, and was observed idle; see
  [performance.md](performance.md#native-wayland-idle-after-the-interface-rebuild).
  Wayland keyboard, pointer, clipboard and IME behaviour were not exercised.

Not verified: macOS and Windows builds and appearance (the Windows target is
not installed locally; CI owns those jobs), operating-system keyboard injection,
clipboard, screen-reader navigation of the new controls, and an input method
that is actively composing.

## SSH workspace verification: 2026-10-01

Remote workspaces were exercised on the Linux development host (GNOME Wayland
session, NVIDIA/Vulkan, OpenSSH 10.2p1) with a debug build that had the
`inspection` feature enabled.

- **Headless contracts.** Model tests cover destination validation, inherited
  destinations for new, split, restarted and restored panes, replacement of
  every pane when a workspace connects or disconnects, and atomic refusal.
  Persistence tests cover the version 2 round trip, lossless reading of
  version 1 and skipping an unusable saved destination. Desktop tests cover the
  client's argument vector, the sheet, palette and confirmation flow, the
  deferred startup path, the startup-command guard, and a real PTY spawn of a
  stand-in client proving that a directory reported by the host never replaces
  a pane's local directory.
- **Real client and server.** A task-owned OpenSSH server listened on loopback
  with throwaway keys; the review launch put a pass-through `ssh` first on
  `PATH` that only added `-F` with a task-owned client configuration. Through
  the X11 inspection path, connecting a two-pane workspace from its menu
  produced two accepted public-key logins, a split a third, and "Reconnect"
  after `exit` another; `SSH_CONNECTION` was set in the panes. Relaunching
  restored the three-pane layout with three new logins. Disconnecting returned
  all three panes to local shells in their original directory and removed the
  destination from saved state.
- **States reviewed from fresh captures.** Both workspace menus, the connect
  and new-workspace sheets (empty, unusable host, Graphite and Light), the
  palette commands, connected single/split panes, an ended connection, the
  disconnect confirmation, a missing client, and 900×640 and 640×480 windows.
  The review found that Enter on an unusable host left the field without the
  keyboard; the sheet now returns focus to it.
- **Wayland.** A one-shot `--ssh` capture at 1.5× scale rendered the connected
  workspace, and an idle `--diagnostics` sample settled at one to two frames a
  second for both a remote and a local workspace.

Not established: a host on another machine or a slow or lossy network, password,
passphrase and host-key prompts, agent forwarding and `ControlMaster` sharing,
and any behaviour on macOS or Windows, where the system client and ConPTY path
are unexercised. The standard isolated regression harness passed with the
version 2 state file.

## SSH split directory regression: 2026-10-01

Focused desktop regressions now cover splitting from the source pane's OSC 7
directory, both split axes, a directory change while that source is unfocused,
and keeping remote paths out of local saved directories. A real Zsh bootstrap
fixture checks login-file ordering, custom `ZDOTDIR`, temporary-file cleanup,
quoted paths with spaces/apostrophes/percent signs/Unicode, and a missing remote
directory. A terminal-core PTY regression checks that local process polling
cannot overwrite the separate shell directory report. The affected desktop and
terminal-core library Clippy checks passed with warnings denied.

The final debug build with `inspection` was exercised on this Linux Dell through
X11 with a task-owned loopback OpenSSH server and the host's actual Zsh startup
configuration. Right and below splits both returned
`/home/biggabo/Documents/Projects/leyfind/leyfind` from `pwd`. Another split
inherited a subsequent directory containing spaces, an apostrophe, `%`, and
Unicode. Five fresh native captures were visually reviewed, including the
900×640 window; saved pane directories remained local, and the application
closed gracefully. This establishes the Linux/OpenSSH/Zsh behavior, not remote
shell integration on other shells or native macOS/Windows behavior.

## Projects verification: 2026-10-05

[Projects](projects.md) were built in slices. The first was exercised in the
running application on 2026-10-05 and the whole on 2026-10-06; this record
keeps apart what each pass saw and what no pass has seen.

Host and build for every native run: Linux 7.0.0-38, GNOME Wayland session
with the window **forced onto X11** (`WAYLAND_DISPLAY` removed for the child),
NVIDIA RTX 4060 with Vulkan, Rust 1.97.1, development profile with the
`inspection` feature, Claude Code 2.1.290. Input was sent through the
inspection protocol, not the operating system.

**Seen running, with a fixture lead.** `python3 scripts/verify-project.py`
passed twice on the build of that day. A stand-in `claude` plays the lead
over the stream protocol and starts Neptune's real tool server, and plays an
agent in a terminal. The script asserted, through the accessibility tree and
the fixture's log:

- the Project tab and its creation form, the project folder (mode 0700) and
  the goal becoming the first message;
- the lead's working directory, arguments and environment; a credential file
  of mode 0600 in a 0700 folder that is gone once the lead is ready; no token
  on the command line;
- a streamed reply, the "Started agent" card, the roster row, no new tab, and
  the agent's report returning as a row and as a turn of the lead;
- an agent that asks before its task pinned under Needs you, its terminal
  opened from there and answered, with one row and one turn of the lead for
  one question;
- pause and resume saved, a paused project starting nothing, Stop ending a
  streamed turn with the same lead process going on;
- no chat words, titles, tasks or replies in `workspaces.json` or the log;
- removal: confirmation, agents given tabs, the lead's process ended.

**Seen running, with the installed Claude Code as lead and agent** (two short
runs on a small model): the lead's arguments as read from `/proc`, including
`--mcp-config` as a file path; a reply about five seconds after the goal; an
agent started on the person's word; Claude Code's folder trust question
surfacing as a Needs-you row and a turn of the lead; the terminal opened from
the row and answered; the agent's report and the lead's follow-up within
seconds; nothing left running or in the temporary folder after closing. Two
defects found this way were fixed and seen fixed: one unanswered trust
question producing repeated rows and turns, and the credential file outliving
the application.

Captures of the running application were reviewed at 1180×760 for the
creation form, a chat with the agent list, Needs you, a streaming reply with
Stop, the project's menu (then only **Rename…** and **Remove project…**),
the removal sheet and the state after removal; the first three were also
captured at 640×480, where the panel still fits. Idle frame counts with `--diagnostics` on X11 were the same with and
without a resting project (two an interval).

**Final native pass: 2026-10-06.** Same host and build settings, Claude Code
2.1.291 and Codex CLI 0.160.1. `python3 scripts/verify-project.py` was extended
and passed twice in a row on the final build (about 2 min 45 s a run). It still
plays lead and agents with a stand-in `claude` that starts Neptune's real tool
server, reads pull requests through the stand-in `gh` of
`verify-pull-request-status.py`, and works in a real temporary git repository.
Beyond the list above it asserted:

- the lead's `close_agent` on an agent whose terminal the person had opened:
  the tab and its CLI stay, the agent leaves the list;
- a paused refusal saying why in its card;
- `record_decision` and `write_context` by the lead, the instructions editor
  saved, `add_note` by an agent, all as files of mode 0600 listed under
  Context, and the next agent's brief holding instructions, decision and
  status; the lead told of the changed instructions;
- two agents started with `worktree`, each on its own branch in its own
  checkout beside the repository;
- Watches: the add form refusing five minutes, a schedule added and run with
  **Run now**, a pull request added in the form, one linked by an agent
  followed without being asked, first sight silent, failing checks waking
  the lead once for each, **Fix CI** on the pull request's own row and on
  the newest row about the agent, pressed on the former with its words
  reaching the agent's terminal, rows that say "Next in … min" and "Last ran
  …", a schedule proposed by the lead waiting for **Allow**;
- a quit in the middle of a streamed turn and a relaunch on the same data:
  the chat with one notice of the cut turn, the agents listed again and their
  CLIs resumed, no turn of the lead for agents that only opened again, the
  lead started with `--resume` and its earlier conversation for the next
  message, and a schedule made two runs late (by editing its saved time)
  running once, half a minute after opening;
- **New chat** keeping the earlier chat in the folder and starting a lead
  that is handed the decisions and instructions;
- **Remove project…** deleting the folder and leaving worktrees in place;
- no chat, context or watch words in `workspaces.json` or the log, at three
  points;
- in a second data root: the sheet at 640×400 with the window at 150 %, its
  list of agents folded and unfolded, a message sent from it, the sheet becoming the tab with its draft when the
  window is widened; a damaged `project.json` copied to
  `project-recovery-*.json` once before it is replaced; a `project.json` of a
  later version shown read-only, its Watches saying they are off, starting
  no lead and leaving every file byte for byte;
- idle: with a resting project and a schedule ahead, 1.0 frames a second with
  the chat in view and 1.5 with the watches, over 12 seconds each with no
  inspection request (X11, `--diagnostics`).

**With the installed CLIs.** One run with Claude Code as lead on a small
model in a new git repository: a plan, then on approval two agents in
worktrees of their own, a recorded decision, both folder trust questions
surfacing as Needs-you rows and answered in the opened terminals, both
reports returning and the lead's summary; after a quit and relaunch the lead
ran with `--resume` and answered from the earlier conversation. One turn with
Codex as lead ("READY"), picked in the creation form.

Defects found by this pass and fixed, each with a test:

- After every relaunch, each restored agent of a project was reported as
  having finished a turn once its CLI had opened again, and the lead took a
  turn about it unasked. Seen with the installed Claude Code; the script now
  delays the restored shells to show it.
- A paused refusal's card lacked its reason when the lead's turn began and
  was refused within one frame.
- A watch's second line was cut off at the panel's width, so how a pull
  request stands could not be read; it wraps now.

Captures of the final build were reviewed at 1180×760 for every state above
and at 640×480 for the creation form, the chat with the agent list, Needs
you, Context, Watches, the allow card, a result row, the restored chat and
the read-only project.

**Still not seen running.**

| Area | Unverified natively |
| --- | --- |
| Durable chat | The "Not everything is saved" row, **Reveal project folder**, the reopen offer for a closed workspace's project, **Load earlier**, rotation of a large chat |
| Shared context | A file's **Open**, **Reveal** and **Delete…**; an installed CLI calling `read_context`, `write_context` or `add_note` (the installed Claude Code called `record_decision`) |
| Watches | A schedule firing at its own time without a restart, pausing and deleting a watch, declining a proposal, a pull request read through the real `gh`, **Address comments**, a merged pull request or worktree, desktop banners |
| Codex as lead | A tool call, an interrupt, a resume or a failure with the installed Codex |
| Lead failures | A usage limit, a signed-out CLI, a retry, an interrupted real turn, three leads at once, the automatic pause |
| Agents | A project agent's permission request with an installed CLI; Codex as a project agent; a restored agent that takes longer than its start grace to report |

Limits seen in that pass were taken up afterwards and seen in the same run
on the changed build: the follow-ups on the pull request's row, an agent's
report drawn with its Markdown, the list of agents scrolling in two fifths
of the room with its bar in view at 640×480, relative times and the
read-only wording in Watches, plain primary buttons where a form cannot be
sent, and the sheet at the smallest window with 150 % zoom, which names the
project in its title row and keeps about four lines of chat while the list
of agents is folded. Left as it is: unfolded there, the list takes a row's
height and the chat keeps one line; by its layout test only, the same holds
while something is under Needs you. **Address comments** was shown and not
pressed.

Also not established: native Wayland (appearance, focus, idle repainting,
input), macOS and Windows (not compiled there for this change; on Windows the
tab only says projects are unavailable), an input method composing in the
chat field or any other project field, the Ctrl+Shift+C/V shortcuts while a
project field has the keyboard, operating-system keyboard input in general,
screen-reader navigation of the tab, and idle cost while the Context segment
re-reads its folder every two seconds.

## Historical visual acceptance

The following acceptance record predates the architecture refactor. It is useful
historical evidence, not acceptance of the current build. Fresh harness artifacts
are required after the changes described in the architecture review appendix.

On 2026-09-30, the final native inspection workflow passed **18 semantic checks**
and produced **23 fresh screenshots**. Its
[machine-readable report](../artifacts/final-review/ui-regression.json) records
workspace creation, three distinct shell processes, interruption, preferences,
responsive focus/search, close/cancel, restoration, and typed-input preservation.
The final designer review accepted the single/three-pane hierarchy, both theme
forms, contained narrow scrolling, vector search controls, focus markers, and
clean restored state. Earlier failed captures remain diagnostic evidence rather
than acceptance results.

For ordinary UI changes, review fresh native captures of affected states after
the final build, including narrow-window behavior. The full release acceptance
matrix (or an explicitly requested full design review) covers:

- A quiet single pane and a populated three-pane workspace.
- Workspace creation, preferences in Graphite and Light, search, and close
  confirmation with cancellation.
- 1180×760, 900×640, and 640×480 logical windows at the recorded display scale.

Inspect terminal dominance, focused-pane clarity, icon alignment, text contrast,
path truncation, overlay padding, field/action alignment, and narrow-window
clipping. Check typography, whitespace, color balance, density, and visual
character as one system. Resolve defects, rebuild, and recapture affected states
before accepting them. The design choices and corrections are recorded in
[design.md](design.md).

## Remaining production release gates

Record these results for the exact release candidate under the mandatory
[RC workflow](releases.md#release-candidate-policy). An accepted RC is required
before preparing stable artifacts; verify the final installers before publication.

| Gate | Required evidence |
| --- | --- |
| Native platforms | Successful release builds and interactive desktop runs on Linux Wayland/X11, Windows ConPTY, and macOS; real shell/TUI input, resize/reflow, alternate screen, mouse, clipboard, and lifecycle verification on each |
| Text and desktop integration | Mixed DPI/display changes, non-US layouts and AltGr, IME/composition, installed/missing CJK and symbol fallbacks, complex scripts, accessibility navigation with a screen reader, and host window controls |
| Sustained behavior | Long output runs, bounded maximum-history memory, many independent panes, hidden-session output, rapid split/resize/close, exited processes, and visible error recovery |
| Performance | Repeated release measurements of PTY-to-screen throughput, keyboard-to-display latency, frame-time percentiles, startup, split-pane scaling, focused/unfocused idle with both blink settings, CPU/RSS, and GPU memory |
| Distribution | Test downloaded installers on clean machines; verify dependency/font notices, signed metadata, checksums/attestations, desktop integration and macOS signing/notarization. Verify the explicit unsigned Windows limitation, native Stable/Beta download/handoff behavior, and no automatic downgrades. Follow [the release guide](releases.md); Windows signing is deferred to SignPath OSS by product decision. |

The parser benchmark and Linux CPU/RSS probe are documented in
[performance.md](performance.md). They do not measure GPU presentation or prove
comparative speed. Publish explicit workload, build, hardware, sample conditions,
and distributions for performance claims.

Current scope excludes Kitty graphics and comprehensive complex-script shaping.
Keyboard encoding also inherits egui's missing keypad identity, lock-state, and
some physical/layout information. Workspace restoration launches new shells, or
new SSH connections for a remote workspace; it does not restore live processes. Those boundaries must remain explicit in release
notes until implementation and native verification expand them.
