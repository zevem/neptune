# Neptune Tabler icon subset

All files in outline/ and filled/ are unmodified official Tabler Icons 3.49.0
SVGs from commit `a4ce1404bc6d24d3c365afe7b258d6bf6f48d62d`. They retain upstream comments,
`currentColor`, the 24×24 viewBox and the rounded 2-point outline strokes.
Playback controls and solid status dots use the official filled style. No
application-specific icon path is generated or traced.

[mapping.json](mapping.json) is the complete native `Icon` mapping, with source
URLs, SHA-256 hashes, upstream icon versions, font code points and explicit
rationales where no exact brand or semantic match exists. The upstream MIT
notice is preserved in [LICENSE.txt](LICENSE.txt).

## Native rendering

The application embeds outline.ttf (75 glyphs, 19,900 bytes) and filled.ttf
(4 glyphs, 1,360 bytes). Dedicated font families do not alter terminal or UI text
fallbacks. Call `icons::register_fonts` when constructing every font definition
set, including asynchronously discovered system fonts, before `ctx.set_fonts`.
Tabler glyphs need no per-frame file access, image decoding or SVG parsing.

The font subsets preserve upstream contour coordinates and the full 24×24
cell. The generator recalculates bounds and corrects left side bearings to the
contour minimum; source fonts contain zero bounding-box minima that otherwise
shift glyphs after subsetting. Every retained glyph keeps its 1000-unit advance,
900/-100 ascent/descent and zero `hhea` line gap. The OS/2 table keeps a
90-unit typographic line gap, which egui adds below the cell.

`paint` centers the full design cell rather than tight ink bounds, preserving
upstream optical offsets. It locates the cell from the glyph's baseline, not
the laid-out row, so that line gap cannot lift the glyph. Physical-pixel raster sizes are rounded and fractional
bounds scale the cached mesh, avoiding new font rasters for every animation
fraction. `paint_rotated` rotates that cell clockwise about its center, without
changing layout or hit targets. Both circle styles have a 20/24 ink diameter;
to preserve a legacy dot's ink diameter, its nominal rectangle is 1.2 times that
diameter. Shapes, hit areas, tooltips, accessible names and interaction state
remain the caller's responsibility.

## Provider marks

CLI provider marks are separate from the Tabler UI icon subset. Native terminal
tabs retain the original [bundled provider artwork](../providers/README.md) and
PNG-mask renderer from before the Tabler migration. Masks are resized with
Lanczos3 and cached at each physical-pixel size; Claude retains its orange
(#d97757), and Codex, Opencode and Pi use the caller's ink. Gemini and Omp return
false without painting or changing the caller's fallback.

Provider-only substitute glyphs (`sparkles`, `brand-openai`, `math-pi`) are not
included. `terminal-2` remains the non-provider Terminal UI icon. The unused
`planet` branding glyph is also omitted; all other UI mappings, including the
Eyes reaction's Tabler `eye`, remain unchanged.

The original SVG/PNG bytes and complete provider render contract are pinned by:

```sh
python3 -m unittest discover -s scripts -p test_provider_icons.py
```

Native `icons::tests` additionally cover provider tints, square bounds,
physical-pixel sizing, texture-cache reuse and the no-mark/no-paint contract.

## Complete mapping

| Native variant | Tabler name | Style | Meaning / substitution rationale |
| --- | --- | --- | --- |
| `Icon::Bell` | `bell` | outline | Direct semantic equivalent. |
| `Icon::Terminal` | `terminal-2` | outline | Terminal shell. |
| `Icon::Plus` | `plus` | outline | Direct semantic equivalent. |
| `Icon::Close` | `x` | outline | Direct semantic equivalent. |
| `Icon::ChevronLeft` | `chevron-left` | outline | Direct semantic equivalent. |
| `Icon::ChevronRight` | `chevron-right` | outline | Direct semantic equivalent. |
| `Icon::ChevronDown` | `chevron-down` | outline | Direct semantic equivalent. |
| `Icon::Settings` | `settings` | outline | Direct semantic equivalent. |
| `Icon::Search` | `search` | outline | Direct semantic equivalent. |
| `Icon::Sidebar` | `layout-sidebar` | outline | Direct semantic equivalent. |
| `Icon::PanelRight` | `layout-sidebar-right` | outline | Direct semantic equivalent. |
| `Icon::SplitVertical` | `layout-columns` | outline | A vertical divider creates left/right columns. |
| `Icon::SplitHorizontal` | `layout-rows` | outline | A horizontal divider creates upper/lower rows. |
| `Icon::Folder` | `folder` | outline | Direct semantic equivalent. |
| `Icon::Grid` | `layout-grid` | outline | Direct semantic equivalent. |
| `Icon::Keyboard` | `keyboard` | outline | Direct semantic equivalent. |
| `Icon::Check` | `check` | outline | Direct semantic equivalent. |
| `Icon::ArrowUpRight` | `arrow-up-right` | outline | Direct semantic equivalent. |
| `Icon::ArrowUp` | `arrow-up` | outline | Direct semantic equivalent. |
| `Icon::ArrowDown` | `arrow-down` | outline | Direct semantic equivalent. |
| `Icon::Minus` | `minus` | outline | Direct semantic equivalent. |
| `Icon::Maximize` | `arrows-maximize` | outline | Direct semantic equivalent. |
| `Icon::Command` | `command` | outline | Direct semantic equivalent. |
| `Icon::Sun` | `sun` | outline | Direct semantic equivalent. |
| `Icon::Moon` | `moon` | outline | Direct semantic equivalent. |
| `Icon::Copy` | `copy` | outline | Direct semantic equivalent. |
| `Icon::Code` | `code` | outline | Direct semantic equivalent. |
| `Icon::Wrap` | `text-wrap` | outline | Direct semantic equivalent. |
| `Icon::Ellipsis` | `dots` | outline | Direct semantic equivalent. |
| `Icon::Refresh` | `refresh` | outline | Direct semantic equivalent. |
| `Icon::Pencil` | `pencil` | outline | Direct semantic equivalent. |
| `Icon::Clipboard` | `clipboard-text` | outline | Direct semantic equivalent. |
| `Icon::Warning` | `alert-triangle` | outline | Direct semantic equivalent. |
| `Icon::Minimize` | `arrows-minimize` | outline | Direct semantic equivalent. |
| `Icon::Eraser` | `eraser` | outline | Direct semantic equivalent. |
| `Icon::TextSize` | `text-size` | outline | Direct semantic equivalent. |
| `Icon::Swap` | `arrows-exchange` | outline | Direct semantic equivalent. |
| `Icon::Globe` | `world` | outline | Direct semantic equivalent. |
| `Icon::Star` | `star` | outline | Direct semantic equivalent. |
| `Icon::Image` | `photo` | outline | Direct semantic equivalent. |
| `Icon::PullRequest` | `git-pull-request` | outline | Direct semantic equivalent. |
| `Icon::Merged` | `git-merge` | outline | Direct semantic equivalent. |
| `Icon::Comment` | `message` | outline | Direct semantic equivalent. |
| `Icon::Branch` | `git-branch` | outline | Direct semantic equivalent. |
| `Icon::Agents` | `hierarchy` | outline | Delegated agents form a hierarchy: one parent and its children; no exact multi-agent glyph. |
| `Icon::File` | `file` | outline | Direct semantic equivalent. |
| `Icon::Files` | `files` | outline | Direct semantic equivalent. |
| `Icon::FilePlus` | `file-plus` | outline | Direct semantic equivalent. |
| `Icon::FolderPlus` | `folder-plus` | outline | Direct semantic equivalent. |
| `Icon::Trash` | `trash` | outline | Direct semantic equivalent. |
| `Icon::Paperclip` | `paperclip` | outline | Direct semantic equivalent. |
| `Icon::Pause` | `player-pause` | filled | Direct semantic equivalent. |
| `Icon::Play` | `player-play` | filled | Direct semantic equivalent. |
| `Icon::Stop` | `player-stop` | filled | Direct semantic equivalent. |
| `Icon::Lock` | `lock` | outline | Direct semantic equivalent. |
| `Icon::Pointer` | `pointer` | outline | Direct semantic equivalent. |
| `Icon::Record` | `circle-dot` | outline | Concentric circle-dot represents recording without a bespoke record path. |
| `Icon::Person` | `user` | outline | Direct semantic equivalent. |
| `Icon::ThumbUp` | `thumb-up` | outline | Direct semantic equivalent. |
| `Icon::ThumbDown` | `thumb-down` | outline | Direct semantic equivalent. |
| `Icon::Smile` | `mood-smile` | outline | Direct semantic equivalent. |
| `Icon::Party` | `confetti` | outline | Direct semantic equivalent. |
| `Icon::Unsure` | `mood-confused` | outline | The official confused face preserves the unsure reaction. |
| `Icon::Heart` | `heart` | outline | Direct semantic equivalent. |
| `Icon::Rocket` | `rocket` | outline | Direct semantic equivalent. |
| `Icon::Eyes` | `eye` | outline | GitHub eyes reaction: the official eye is the closest direct looking/watching metaphor; no two-eyes reaction glyph. |
| `Icon::GitHub` | `brand-github` | outline | Direct semantic equivalent. |
| `Icon::ChevronUp` | `chevron-up` | outline | Direct semantic equivalent. |
| `Icon::ArrowRight` | `arrow-right` | outline | Direct semantic equivalent. |
| `Icon::ExternalLink` | `external-link` | outline | Direct semantic equivalent. |
| `Icon::ArrowLeft` | `arrow-left` | outline | Direct semantic equivalent. |
| `Icon::Circle` | `circle` | outline | Direct semantic equivalent. |
| `Icon::CircleFilled` | `circle` | filled | Solid status dot uses the official filled circle rather than custom geometry. |
| `Icon::Loader2` | `loader-2` | outline | Official open circular loader; animation rotates this unmodified glyph around its design-cell center. |
| `Icon::CircleDashed` | `circle-dashed` | outline | Direct semantic equivalent. |
| `Icon::CircleX` | `circle-x` | outline | Direct semantic equivalent. |
| `Icon::CircleCheck` | `circle-check` | outline | Direct semantic equivalent. |
| `Icon::Square` | `square` | outline | Direct semantic equivalent. |
| `Icon::Checkbox` | `checkbox` | outline | Direct semantic equivalent. |

## Reproducible generation

The optional maintenance dependency is pinned; it is not a Cargo/build/runtime
requirement:

```sh
uv run --with fonttools==4.60.1 python assets/icons/tabler/generate.py
uv run --with fonttools==4.60.1 python assets/icons/tabler/generate.py --check
```

The generator verifies the pinned official `@tabler/icons-webfont` 3.49.0 npm
archive, source font hashes and license, every local SVG hash, complete Rust enum
coverage and CSS code point. It checks that subsetting preserves each contour and
advance. [font-source.json](font-source.json) records the verified inputs, exact
output hashes, retained glyph counts and metrics; [glyphs.rs](glyphs.rs) supplies
the exhaustive Rust match. Regeneration is byte-for-byte deterministic.

For offline generation, append `--archive /path/to/icons-webfont-3.49.0.tgz`;
the same archive hash verification applies. SVG source acquisition is separate:
fetch the exact source URL recorded for each entry in mapping.json and verify
its SHA-256 before replacing it. Neither upstream scripts nor downloaded SVGs
are executed. To update the subset, edit the native enum and mapping together,
retain source provenance, regenerate, and rerun the check.

Sources:

- [Pinned Tabler repository](https://github.com/tabler/tabler-icons/tree/a4ce1404bc6d24d3c365afe7b258d6bf6f48d62d)
- [Tabler icon design](https://tabler.io/icons)
- [Pinned official npm package](https://registry.npmjs.org/@tabler/icons-webfont/-/icons-webfont-3.49.0.tgz)
