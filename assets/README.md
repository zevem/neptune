# Assets

Project artwork, designs and bundled fonts live here.

The supplied originals have distinct uses:

| Source | Supplied name | Use |
| --- | --- | --- |
| [`branding/neptune-logo.png`](branding/neptune-logo.png) | Logo.png | Linux and Windows app/taskbar icons |
| [`branding/neptune-macos-logo.png`](branding/neptune-macos-logo.png) | MacOS Logo.png | macOS Finder/Dock icon, with its supplied padding preserved |
| [`branding/neptune-icon.png`](branding/neptune-icon.png) | Icon.png | Website, marketing, sharing metadata and README |

- `icons/` contains Linux/Windows PNGs from 16 to 1024 pixels and a Windows ICO
  with sizes through 256 pixels, exported from `neptune-logo.png`. The macOS ICNS
  has Retina sizes through 1024 pixels, exported from `neptune-macos-logo.png`
  without adding padding.
- [`icons/providers/`](icons/providers/README.md) contains the marks of the CLI
  agents a terminal tab can show.
- [`fonts/`](fonts/README.md) contains the bundled typefaces and their licenses.

Regenerate the icons and website copies from the repository root:

```sh
python3 -m pip install Pillow
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
