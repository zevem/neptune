# Tabler migration: provider and branding correction

This follow-up to `098c515` preserves Tabler 3.49.0 for functional UI icons,
while restoring identity artwork from `098c515^`. Work stays local on
`chore/tabler-icons`: no push, PR, merge, release or deployment.

## Restored identity

- **Claude, Codex, OpenCode and pi:** original SVGs, 64-pixel PNG masks,
  provenance README and `scripts/export-provider-icons.py`. Native
  `paint_provider` and mask caching/resampling retain the original rendering,
  proportions and alpha. Claude keeps `#d97757`; the other marks use caller ink.
  Gemini and Omp still return false without painting.
- **Website providers:** original embedded Claude/Codex paths and viewBoxes,
  sizing/color behavior, and textual terminal banners. Other AI provider marks
  were not present in the original website helper.
- **Neptune branding:** all three supplied PNG originals, eleven native
  PNG/ICO/ICNS exports, and four website logo/favicon/touch-icon copies. These
  18 assets and the original Pillow-only exporter are byte-identical to
  `098c515^`. Running that exporter reproduces all 15 derived files exactly.
- **Tabler cleanup:** remove provider-only glyph mappings and SVGs and the
  unused `planet` glyph; regenerate the native font subset. Ordinary controls
  (including the terminal control, not an OpenCode identity substitute) remain
  Tabler.

## Artwork decisions

The supplied Neptune images are explicitly app identity and external branding
in `assets/README.md`, not UI iconography; restore them rather than using a
stock planet glyph. The old two-eye drawing is the GitHub pull-request Eyes
reaction control (`Emoji::Eyes` → `Icon::Eyes`), not logo/brand artwork. Keep it
on Tabler `eye`, still dispatching the same reaction; no exact two-eye glyph is
available in pinned Tabler 3.49.0.

See [the mapping and exclusions](tabler-icons.md) for the complete migration.

## Verification

Validation is recorded after the final implementation. Host is the connected
Linux computer, Rust 1.97.1 (repository pin), Bun 1.3.14, default desktop
features, Cargo `ci` profile and the repository job limit of two; no job or hook
overrides. No reference-repository changes are part of this follow-up.

| Check | Result |
| --- | --- |
| Original branding regression against `098c515` | **Failed as expected:** all three original-image hashes differ |
| Restored originals and logo exporter regression | **Passed:** two tests; 19 files match `098c515^` before and after regeneration |
| Provider restoration regressions | **Passed after failing before:** native Python regression reports nine expected failures against `098c515`, then two passing tests after restoration; website regression reports five failed tests before, then passes |
| `cargo fmt --all --check` | **Passed** |
| `cargo clippy --locked --all-targets --profile ci -- -D warnings` | **Passed** |
| `cargo test --locked --profile ci` | **Failed:** 849 passed, 11 failed, 9 ignored; all eight icon/provider tests passed |
| `cargo test --locked --profile ci --bin neptune --test agent_resolution --test agent_restore` | **Passed:** two binary and four integration tests |
| `cargo test --locked --profile ci --doc` | **Passed:** zero doctests |
| `cd website && bun run lint` | **Passed** |
| `cd website && bun run test` | **Passed:** 22 tests |
| `cd website && bun run build` | **Passed:** production compile, typecheck and prerender |
| `python3 -m unittest discover -s scripts -p test_brand_artwork.py` | **Passed:** two tests, including all 15 exact derived exports |
| `python3 -m unittest discover -s scripts -p test_provider_icons.py` | **Passed:** two tests |
| `python3 -m unittest discover -s scripts -p test_icon_notices.py` | **Passed:** two tests |
| `uv run --with fonttools==4.60.1 python assets/icons/tabler/generate.py --archive /tmp/neptune-tabler-webfont-3.49.0.tgz --check` | **Passed:** deterministic subset; 75 outline and four filled glyphs |
| `python3 scripts/export-provider-icons.py` plus original byte comparisons | **Passed:** all ten restored provider files and native renderer/helper region match `098c515^` |
| `python3 scripts/check-architecture.py` | **Passed** |
| `git diff --check` | **Passed** |
| Native/browser visual review | **Partial:** fresh captures listed below were viewed; other platforms/states were not exercised |

The 11 Rust failures are the same Git fixture setup failures seen before this
correction: `app::{changes,git,projects,worktrees}` and `runtime::worktrees`.
A fresh standalone `git init --quiet --initial-branch=main` also failed with
“not a git repository” and “ref updates aborted by hook”. Git hooks were not
bypassed or reconfigured; no failing test was skipped, weakened or disabled.
Initial validation caught a reference dereference in a new regression test;
that was fixed before the final passing Clippy and final test run.

## Visual evidence and limits

Fresh native eframe/Wgpu app captures ran in isolated X11/Xvfb with temporary
storage, on the final build. Each ignored capture test passed, and all three
images were viewed:

- `NEPTUNE_DROP_AGENT=1 NEPTUNE_DROP_SPLIT=1` with
  `app::tests::capture_file_drop_native`: Claude mark on a split terminal tab.
- `NEPTUNE_EXPLORER_STATE=agents` with
  `app::tests::capture_explorer_native`: Claude/Codex tabs and Agents panel.
- The same Agents fixture with `NEPTUNE_EXPLORER_NARROW=1`: narrow layout.

Commands used `NEPTUNE_DROP_CAPTURE` / `NEPTUNE_EXPLORER_CAPTURE` for task-owned
output paths, `xvfb-run -a cargo test --locked --profile ci --lib <test> --
--ignored --exact --nocapture`. Captures are
[`native-providers-split.png`](../artifacts/tabler-icons/provider-restore/native-providers-split.png),
[`native-agents.png`](../artifacts/tabler-icons/provider-restore/native-agents.png),
and [`native-agents-narrow.png`](../artifacts/tabler-icons/provider-restore/native-agents-narrow.png).

Actual headless Chrome production-website captures cover Claude and Codex tabs,
original terminal banners and Neptune's navigation logo at 1440px and 430px.
The reduced-motion Agents chapter end-state was staged before actual tab/panel
interactions; these are not proof of active tour animation. No JavaScript
exceptions were observed; computed provider viewBoxes/colors were recorded.
See [`website/`](../artifacts/tabler-icons/provider-restore/website/).

Native OpenCode/pi marks were verified by exact asset/renderer identity and
painting tests, not fresh running-app screenshots. Other native provider-bearing
screens (projects/chat/browser tabs), physical pointer/hover behavior, website
light theme/animation, installed Dock/taskbar icons, hardware-GPU acceleration,
Wayland, macOS and Windows were not visually verified. Existing fixture screens
do not expose every provider/surface; shared-renderer tests and byte identity
are not a claim of full platform visual acceptance.

Detailed command logs and fresh visual evidence are local under
`artifacts/tabler-icons/provider-restore/`; they are not shipped app assets.
Earlier captures under `artifacts/tabler-icons/` show the superseded migration,
not this correction. Its original verification report now points here.
