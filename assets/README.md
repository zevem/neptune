# Assets

Tabler icon sources, generated platform artwork and bundled fonts live here.
All icon shapes come from pinned official Tabler Icons 3.49.0 assets; the
[manifest and regeneration notes](icons/tabler/README.md) record their provenance.
The [migration mapping](../docs/tabler-icons.md) lists every replaced icon and
explains the few approximate matches.

| Generated artwork | Use |
| --- | --- |
| [`branding/neptune-logo.png`](branding/neptune-logo.png) | Blue Tabler `planet` on a white rounded tile, for Linux/Windows app/taskbar icons |
| [`branding/neptune-macos-logo.png`](branding/neptune-macos-logo.png) | Same tile with the existing 100-pixel macOS Finder/Dock padding |
| [`branding/neptune-icon.png`](branding/neptune-icon.png) | Blue Tabler `planet` on transparency, for website, metadata and README |

- `icons/` contains Linux/Windows PNGs from 16 to 1024 pixels and a Windows ICO
  with sizes through 256 pixels, exported from `neptune-logo.png`. The macOS ICNS
  has Retina sizes through 1024 pixels, exported from `neptune-macos-logo.png`
  without adding padding.
- [`icons/tabler/`](icons/tabler/README.md) contains the native UI font subsets,
  official SVGs, exhaustive glyph mapping and MIT notice. Provider indicators
  use these glyphs too; no separate provider masks are decoded.
- [`fonts/`](fonts/README.md) contains the bundled typefaces and their licenses.

Regenerate the icons and website copies from the repository root:

```sh
# Requires Pillow, PyGObject with librsvg, and pycairo.
python3 scripts/export-logo.py
```

Linux/Windows native windows load `icons/neptune-256.png`; Windows builds embed
`icons/neptune.ico` in the executable. The running macOS Dock icon loads
`branding/neptune-macos-logo.png`, preserving the same artwork and padding as
the bundle's `icons/neptune.icns`, so opening the app does not enlarge the icon;
see [`package-macos.py`](../scripts/package-macos.py). Linux installs the PNGs in
the `hicolor` icon theme under the name `neptune`.

The website copies use `neptune-icon.png` for navigation, footer, favicon, touch
icon and sharing metadata. The touch icon composites the mark onto the website's
opaque dark background. The logo belongs to app identity and external branding;
it is not drawn inside the terminal interface or its website demo.
