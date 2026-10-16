# Tabler icon migration

Provider/branding exclusions below correct the original `098c515` migration.
See the [restoration verification](tabler-icons-verification.md).

## Reference approach

The ai-video-editor checkout was reviewed read-only before implementing this
migration: its `src/icons.rs`, `src/theme.rs` and
`assets/icons/tabler/{README.md,generate.py,mapping.json,font-source.json}`.
Neptune follows its pinned **Tabler Icons 3.49.0** approach:

- Unmodified official SVGs, source URLs, SHA-256 hashes and the MIT notice.
- Small outline/filled TrueType subsets from `@tabler/icons-webfont` 3.49.0,
  embedded with `include_bytes!`; no extra Rust package or runtime SVG decoder.
- Exhaustive semantic `Icon` → variant/code-point mapping in generated Rust.
- Dedicated named icon font families, never text/terminal fallbacks. Central
  font construction preserves them through async system-font selection/reset.
- Upstream 24-point cells, corrected bearings and unchanged contours; rendering
  quantizes the raster size to physical pixels and scales the mesh for smooth
  fractional-size animation. Widgets still own color, hit targets, focus, hover
  surfaces and tooltips. Rotating glyphs keep the cell center fixed.
- Optional pinned `fonttools==4.60.1` maintenance generator and `--check`;
  normal Cargo builds require neither Python, npm nor network access for icons.

The website uses explicit, tree-shakeable `@tabler/icons-react` **3.49.0**
imports behind its existing semantic helper, with the same glyph mapping,
size props and `currentColor` inheritance. Provider marks and Neptune app/site
branding are deliberate exclusions: their original assets and rendering from
before `098c515` are restored, not approximated with Tabler glyphs.

## Complete original native icon mapping

Website names shared with these controls use the same Tabler icon.

| Old custom icon | Tabler icon |
| --- | --- |
| Bell | `bell` |
| Terminal | `terminal-2` |
| Plus | `plus` |
| Close | `x` |
| ChevronLeft | `chevron-left` |
| ChevronRight | `chevron-right` |
| ChevronDown | `chevron-down` |
| Settings | `settings` |
| Search | `search` |
| Sidebar | `layout-sidebar` |
| PanelRight | `layout-sidebar-right` |
| SplitVertical | `layout-columns` |
| SplitHorizontal | `layout-rows` |
| Folder | `folder` |
| Grid | `layout-grid` |
| Keyboard | `keyboard` |
| Check | `check` |
| ArrowUpRight | `arrow-up-right` |
| ArrowUp | `arrow-up` |
| ArrowDown | `arrow-down` |
| Minus | `minus` |
| Maximize | `arrows-maximize` |
| Command | `command` |
| Sun | `sun` |
| Moon | `moon` |
| Copy | `copy` |
| Code | `code` |
| Wrap | `text-wrap` |
| Ellipsis | `dots` |
| Refresh | `refresh` |
| Pencil | `pencil` |
| Clipboard | `clipboard-text` |
| Warning | `alert-triangle` |
| Minimize | `arrows-minimize` |
| Eraser | `eraser` |
| TextSize | `text-size` |
| Swap | `arrows-exchange` |
| Globe | `world` |
| Star | `star` |
| Image | `photo` |
| PullRequest | `git-pull-request` |
| Merged | `git-merge` |
| Comment | `message` |
| Branch | `git-branch` |
| Agents | `hierarchy` |
| File | `file` |
| Files | `files` |
| FilePlus | `file-plus` |
| FolderPlus | `folder-plus` |
| Trash | `trash` |
| Paperclip | `paperclip` |
| Pause | `player-pause` (filled) |
| Play | `player-play` (filled) |
| Stop | `player-stop` (filled) |
| Lock | `lock` |
| Pointer | `pointer` |
| Record | `circle-dot` |
| Person | `user` |
| ThumbUp | `thumb-up` |
| ThumbDown | `thumb-down` |
| Smile | `mood-smile` |
| Party | `confetti` |
| Unsure | `mood-confused` |
| Heart | `heart` |
| Rocket | `rocket` |
| Eyes | `eye` |

## Marks and icons outside the original enum

| Old custom icon / artwork | Replacement or preserved original |
| --- | --- |
| Application, Dock, taskbar, site, favicon and marketing artwork | Original supplied Neptune brand artwork, restored byte-for-byte |
| Claude provider mark | Original SVG/PNG mask and rendering; Claude orange |
| Codex provider mark | Original SVG/PNG mask and rendering; caller ink |
| OpenCode provider mark | Original SVG/PNG mask and rendering; caller ink |
| pi provider mark | Original SVG/PNG mask and rendering; caller ink |
| Website GitHub silhouette | `brand-github` |
| Window traffic-light close/minimize/maximize strokes | `x` / `minus` / `plus`; colored button surfaces retained |
| Animated folder disclosure chevron | Rotated `chevron-right`; existing openness animation retained |
| Browser loading arc and egui spinners | Rotating `loader-2` in the existing bounds/color |
| Native status rings and dots | `circle` outline/filled, with existing backing surfaces and colors |
| Website working-agent concentric mark | `circle-dot` |
| PR branch-direction text arrow | `arrow-left`, preserving its text-layout slot |
| Viewed-file text checkmark | `check`, preserving the toggle and accessible label |
| Markdown task checkbox geometry | `square` plus `check`, preserving the two-tone checked state |
| Website right arrow | `arrow-right` |
| Website terminal prompt elbow | `corner-down-right` |

## Matches and deliberate exclusions

Provider identity must remain exactly as before this migration. The original
Claude, Codex, OpenCode and pi SVGs, raster masks, rasterizer and native cached
texture renderer are retained. The website retains its original Claude/Codex
embedded masks, colors, sizing and provider call sites. Gemini and Omp still
have no native provider glyph, matching the previous behavior. No AI provider
mark is replaced by a generic Tabler glyph. Provider-only glyphs and the unused
`planet` glyph have been removed from the native subset.

The supplied Neptune artwork is explicitly documented in [the asset guide](../assets/README.md)
as app identity and external branding, not terminal UI. Restore all three
originals plus their native platform, website and favicon exports, with the
original Pillow-only exporter; do not reinterpret them as Tabler `planet`.

The old two-eye drawing belongs to the GitHub pull-request reaction control
(`Emoji::Eyes` → `Icon::Eyes`), not app/provider branding. It therefore stays on
Tabler `eye`; it sends the same Eyes reaction. Tabler 3.49.0 has no exact
two-eye reaction glyph, so this remains an approximate UI-icon match.

Terminal output/title protocols, keyboard shortcut notation (including command
and arrow keys), diff/math punctuation, fetched browser favicons, identity
initials, charts, color swatches, control geometry and backing/focus surfaces
are data or UI surfaces rather than custom icons and are intentionally retained.

## Maintenance and notices

See the [asset manifest and generator instructions](../assets/icons/tabler/README.md).

`python3 scripts/export-logo.py` regenerates platform/site raster copies from
the supplied original PNG artwork (Pillow only).
`python3 scripts/export-provider-icons.py` regenerates the original provider
masks; see [provider provenance](../assets/icons/providers/README.md).
Native packages copy the Tabler MIT notice to
`icons/Tabler-LICENSE.txt`; packaging fails if it is missing.
The [third-party notices](../THIRD-PARTY-NOTICES.md) cover the shared assets
and website package.
