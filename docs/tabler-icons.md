# Tabler icon migration

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
size props and `currentColor` inheritance. Application/site raster formats
are generated from the official `planet` SVG, keeping platform sizes, blue ink,
white native backing, transparent corners and 100-pixel macOS padding.

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

| Old custom icon | Tabler replacement |
| --- | --- |
| Application, Dock, taskbar, site, favicon and marketing planet artwork | `planet`, generated in existing platform formats |
| Claude provider mark | `sparkles`, retaining Claude orange |
| Codex provider mark | `brand-openai`, retaining caller ink |
| OpenCode provider mark | `terminal-2`, retaining caller ink |
| pi provider mark | `math-pi`, retaining caller ink |
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

Tabler 3.49.0 has no exact Claude, OpenCode or pi brand marks, so their closest
semantic glyphs replace the bespoke artwork. `eye` replaces the original
two-eye reaction picture; the reaction still sends the same Eyes value.
The original waved planet brand artwork has no exact upstream match; `planet`
is the closest subject match. Gemini and Omp still have no provider glyph,
matching the previous behavior. No custom icon paths or provider masks remain.

Terminal output/title protocols, keyboard shortcut notation (including command
and arrow keys), diff/math punctuation, fetched browser favicons, identity
initials, charts, color swatches, control geometry and backing/focus surfaces
are data or UI surfaces rather than custom icons and are intentionally retained.

## Maintenance and notices

See the [asset manifest and generator instructions](../assets/icons/tabler/README.md).

`python3 scripts/export-logo.py` regenerates all platform/site raster icons
from the pinned SVG (Pillow, PyGObject/librsvg and pycairo are maintenance-only
requirements). Native packages copy the Tabler MIT notice to
`icons/Tabler-LICENSE.txt`; packaging fails if it is missing.
The [third-party notices](../THIRD-PARTY-NOTICES.md) cover the shared assets
and website package.
