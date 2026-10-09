# Native verification and probes

Prefer `native-harness.py` for integrated native verification: it owns a unique
loopback endpoint, fresh data directory, subprocess handles, logs, artifacts and
cleanup. Build only the required desktop targets:

```sh
cargo build -p neptune-terminal --features inspection --locked --bin neptune --bin neptune-inspect
python3 scripts/native-harness.py --output artifacts/native
```

For final release captures or performance probes, build those targets with
`--release` and pass their paths through `--app`/`--client`. Load the applicable
procedure in [docs/verification.md](../docs/verification.md) for native acceptance
or [docs/performance.md](../docs/performance.md) for CPU/RSS and scaling. Select
only relevant scenarios; use each script's `--help` for current arguments.

## Isolation hazards

- Independently launched test instances need a fresh `--data-root PATH`.
  Inspection-enabled launches also need a unique loopback
  `EGUI_INSPECTION=127.0.0.1:PORT`. Pass that exact endpoint and data root to
  inspection/regression scripts, including relaunches. An explicit `--config`
  must also refer to task-owned storage.
- Reap only captured subprocess handles/PIDs owned by this run. Regression close
  and restore operations may terminate/relaunch the app: verify the endpoint
  belongs to this task before sending them. Never attach mutating tests to the
  developer's working terminal or reuse its saved state.
- OS key/pointer workflows run sequentially because desktop focus is global.
  For `native-smoke.py`/`ui-regression.py`, pass the exact owned `--window-id`;
  do not let title matching select another Neptune window. Prefer stable accessible
  pane/field labels and bounded semantic polling in `inspect-regression.py` over
  coordinates or fixed delays. The old coordinate workflow is diagnostic only.

## Evidence and focused checks

Release scripts and packaging follow [docs/releases.md](../docs/releases.md).
Use `python3 -m unittest discover -s scripts -p test_release.py` for SemVer,
changelog, signed checksums and private-draft failure behavior; use actionlint
for changed workflows. Package only task-owned built executables. Never create
a tag/release to test this pipeline without explicit release authorization.

Linux native updater visuals can be captured from the real app with test-only
release data (no production fixture switch), in fresh task-owned storage:

```sh
NEPTUNE_UPDATE_CAPTURE="$PWD/artifacts/native-update.png" \
  cargo test -p neptune-terminal --lib app::tests::capture_update_native --release --locked -- --ignored --nocapture
```

`NEPTUNE_UPDATE_SCREEN=notification` selects the notification; `preferences`
selects Preferences, `preferences-update` shows it with a release on offer and
`preferences-installed` with one installed and waiting for a restart;
`ready` and `error` review those sheet states, `restart` a verified download
that installs in place and `installed` the state after it has. `NEPTUNE_UPDATE_NOTES` names a
Markdown file to show as the release notes, such as a changelog section.
Preferences opens on Updates; `NEPTUNE_PREFERENCES_PANE` names another pane
(`general`, `appearance`, `text`, `shell`, `notifications`), and
`NEPTUNE_PREFERENCES_SEARCH` opens it on the results for a query.
`NEPTUNE_UPDATE_NARROW=1` uses a 640×400 window. Each launch
exits after its capture and removes its temporary data root. These are visual
fixtures, not proof of an actual published/signed installer download.

The file-drop highlight can be captured the same way, with files held over the
focused terminal as another application's drag would leave them:

```sh
NEPTUNE_DROP_CAPTURE="$PWD/artifacts/native-file-drop.png" \
  cargo test -p neptune-terminal --lib app::tests::capture_file_drop_native --locked -- --ignored --nocapture
```

`NEPTUNE_DROP_SPLIT=1` shows two terminals, `NEPTUNE_DROP_AGENT=1` marks the
focused one as running Claude Code, and `NEPTUNE_DROP_NARROW=1` uses a 640×400
window. It shows the highlight, not an actual drag: a drop from a real file
manager, and where the pointer is during it, need a hand-driven native check on
each of X11 and Wayland.

The preview of a picture whose path is under the pointer is captured likewise,
with a shell printing the path of a generated chart and the pointer held on it:

```sh
NEPTUNE_PREVIEW_CAPTURE="$PWD/artifacts/native-image-preview.png" \
  cargo test -p neptune-terminal --lib app::tests::capture_image_preview_native --locked -- --ignored --nocapture
```

`NEPTUNE_PREVIEW_BOTTOM=1` prints the path at the bottom of the terminal, where
the preview opens above it, `NEPTUNE_PREVIEW_OPEN=1` clicks the preview for the
full view, `NEPTUNE_PREVIEW_ZOOM=1` also zooms it in with the keyboard, `NEPTUNE_PREVIEW_NARROW=1` uses a 640×400 window,
and `NEPTUNE_PREVIEW_IMAGE` names a picture of your own by absolute path. The
pointer is placed by the test: resting a real pointer, and leaving the path,
need a hand-driven native check.

The pull request tab is captured likewise, with a pull request written in
the test in place of GitHub:

```sh
NEPTUNE_PR_CAPTURE="$PWD/artifacts/native-pull-request.png" \
  cargo test -p neptune-terminal --lib app::pull_request::tests::capture_pull_request_native --locked -- --ignored --nocapture
```

`NEPTUNE_PR_STATE` names what is shown: `summary` (the default), `ready`,
`merged`, `draft`, `conflicts`, `checks`, `confirm`, `close`, `problem`,
`timeline`, `code`, `diff`, `comment`, `review`, `reading`, `missing`,
`signed-out`, `linked`, `empty`, `split` (the diff side by side), `line` (a
comment being written under a line of the diff) or `drive`. `drive` presses a number on the
toolbar, opens the comment field, types, sends with the command key and Enter
and leaves a second comment with Escape, through the application's own input
path, and fails unless each step did what it should. `press` does the same for a
label and a reviewer from their lists, the description and a comment rewritten
in place, a reaction, an answer to a review conversation and its reopening,
hiding whitespace, a comment written under a line of a diff and sent with a
review, and a pill's cross. `NEPTUNE_PR_NARROW=1` uses a 640×400 window,
`NEPTUNE_PR_WIDTH` sets the panel's width and `NEPTUNE_PR_THEME` names a theme.
`NEPTUNE_PR_LIVE` names a pull request by its address and reads it with the
signed-in GitHub CLI instead; a capture never changes a pull request. Pressing
a real pointer and keyboard and every change sent to a host need a hand-driven
native check. `NEPTUNE_PR_LIVE=<address of a merged pull request> cargo test -p
neptune-terminal --lib runtime::pull_request::tests::the_github_cli --locked --
--ignored` reads it with the signed-in GitHub CLI, marks one of its files as
viewed and unmarks it (which only that account sees) and checks that a change
the host must refuse comes back in the host's words.

The integrated shell regression needs a POSIX shell. X11 injection verifies
Linux/X11 input, not Wayland, macOS or Windows input. Inspection events verify
application routing; clipboard, IME and real OS keyboard behavior need native
checks. A passing interaction report still needs visual inspection of captures.

Output measurements require producer-start markers and parsed-byte evidence
before sampling, activity throughout the sample and complete session teardown.
Use release builds and distinguish parser throughput, frame CPU, process CPU/RSS,
inspection roundtrip latency and GPU/input-to-display performance. Record actual
host, features and build metadata; retain failed runs as diagnostic evidence.

For changes to the corresponding tool, choose one:

```sh
python3 -m unittest discover -s scripts -p test_native_harness.py
python3 -m unittest discover -s scripts -p test_measure_scale.py
python3 scripts/check-architecture.py --self-test
```

The architecture checker is a lightweight boundary proof for dependency/public
seam changes. Its self-tests are needed when changing the checker, not for every
Rust change. Do not launch native/resource scenarios for instruction-only edits.
