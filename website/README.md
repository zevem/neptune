# Neptune website

The landing page and release downloads for [Neptune](https://neptune.rs): Next.js
with server-cached GitHub release discovery, styled with Tailwind CSS. It uses
[Bun](https://bun.sh) for dependencies and scripts and is hosted on Vercel.

```sh
bun install --frozen-lockfile
bun run dev      # http://localhost:3000
bun run lint
bun run test
bun run build    # Next.js output in .next/
bun run start    # production server
```

`/download` prefers published stable releases and shows Beta until the first
stable release is published. `/download/beta` resolves the newest published
prerelease. The Stable/Beta choice appears once a verified stable release is
available. Server-only discovery verifies signed metadata,
caches success for five minutes and empty/failure states for one minute. Native
binaries remain on GitHub. All five packages, architecture/format, version,
notes, release/source and hashes stay visible. Ambiguous Mac architectures get
explicit Apple Silicon/Intel choices; unsupported platforms get no guessed binary.
See [the release guide](../docs/releases.md#website-downloads) for routing,
optional server read-token configuration, trust keys and troubleshooting.

## Staying consistent with the app

The page rebuilds Neptune's window in the browser instead of showing screenshots,
so it follows the app's own sources:

- `src/app/globals.css` carries the palette, radii and motion of
  [`src/theme.rs`](../src/theme.rs) and [`docs/design.md`](../docs/design.md).
- `src/components/icons.tsx` maps the shared icon names to official
  `@tabler/icons-react` **3.49.0** components, matching the native Tabler subset
  in [`src/icons.rs`](../src/icons.rs). Explicit imports keep the bundle limited
  to used glyphs; icons retain the upstream 24-point grid, 2-point rounded
  strokes, `currentColor`, caller sizes and theme/hover classes. Playback uses
  the official filled variants. GitHub uses `brand-github`, Codex
  `brand-openai`, and Claude `sparkles` (there is no `brand-claude` in 3.49.0).
- `src/components/neptune/` mirrors the chrome, panes, palette and sheets under
  [`src/ui/`](../src/ui), and a small model of
  [`crates/neptune-model`](../crates/neptune-model).

When the app's look or behaviour changes, update the matching file here.

Website branding uses the official Tabler 3.49.0 `planet` glyph, stored as
[`assets/branding/neptune-icon.png`](../assets/branding/neptune-icon.png), with
Neptune's existing colors, backing and export sizes.
Run `python3 scripts/export-logo.py` from the repository root to refresh the
navigation/footer logo, favicon and touch icon. The same logo is used in sharing
metadata. Native app icons use the separate platform logos documented in
[`assets/README.md`](../assets/README.md). Keep the window demo free of logos, as
in the desktop app.

The window answers a few typed commands through a stand-in shell
(`src/components/neptune/shell.ts`). It demonstrates the interface; it is not
Neptune's terminal engine.

## Claims

The page states only what the repository documents. Measurements come from
[`docs/performance.md`](../docs/performance.md) and are shown with their host and
scope; platform status follows the [README](../README.md). It makes no
comparison with other terminals. Keep new copy to the same standard.

## Hosting

The canonical domain is `https://neptune.rs`; metadata and sharing links use it.
Configure Vercel with `website/` as the project root and `main` as the production
branch. [`vercel.json`](vercel.json) selects the Next.js preset, frozen Bun
install, `bun run build` and `.next` output. It enables automatic deployments
only for pushes to `main`, including merged pull requests. Other branches do
not create preview deployments; the `**` rule covers branch names containing `/`.

Server routes and caching require a Next.js runtime. The public updater trust
key is embedded from `../packaging/update-public-key.hex` at build time. Enable
**Include source files outside of the Root Directory in the Build Step** in
Vercel's Root Directory settings. No domain change is performed merely by
preparing release code.
