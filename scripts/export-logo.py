#!/usr/bin/env python3
"""Export Neptune's Tabler planet application and marketing icons.

Requires Pillow, PyGObject (librsvg) and pycairo. Uses the pinned, unmodified
Tabler SVG in assets/icons/tabler; no network access or custom glyph paths.
The blue ink, white application tile, transparent corners and 100-pixel macOS
padding remain platform-specific. Website touch icons retain their dark backing.
"""

import io
from pathlib import Path

import cairo
import gi
from PIL import Image, ImageDraw

gi.require_version("Rsvg", "2.0")
from gi.repository import Rsvg  # noqa: E402


REPO = Path(__file__).resolve().parents[1]
INK = "#1464ed"
SIZE = 1024


def planet(size: int) -> Image.Image:
    source = (REPO / "assets/icons/tabler/outline/planet.svg").read_bytes()
    handle = Rsvg.Handle.new_from_data(source.replace(b"currentColor", INK.encode()))
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, size, size)
    viewport = Rsvg.Rectangle()
    viewport.width = viewport.height = size
    handle.render_document(cairo.Context(surface), viewport)
    output = io.BytesIO()
    surface.write_to_png(output)
    output.seek(0)
    return Image.open(output).convert("RGBA")


def main() -> None:
    branding = REPO / "assets/branding"
    branding.mkdir(parents=True, exist_ok=True)
    mark = planet(SIZE)
    # The tile is a backing surface, not an additional icon. Draw at double
    # resolution so its rounded transparent corners downsample cleanly.
    tile = Image.new("RGBA", (SIZE * 2, SIZE * 2))
    ImageDraw.Draw(tile).rounded_rectangle(
        (0, 0, SIZE * 2 - 1, SIZE * 2 - 1), radius=560, fill="white"
    )
    app_icon = tile.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    glyph = planet(912)
    app_icon.alpha_composite(glyph, (56, 56))
    macos_icon = Image.new("RGBA", (SIZE, SIZE))
    macos_icon.alpha_composite(app_icon.resize((824, 824), Image.Resampling.LANCZOS), (100, 100))
    for filename, artwork in (
        ("neptune-logo.png", app_icon),
        ("neptune-macos-logo.png", macos_icon),
        ("neptune-icon.png", mark),
    ):
        artwork.save(branding / filename, optimize=True)

    icons = REPO / "assets/icons"
    icons.mkdir(parents=True, exist_ok=True)
    sizes = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
    for size in sizes:
        app_icon.resize((size, size), Image.Resampling.LANCZOS).save(
            icons / f"neptune-{size}.png", optimize=True
        )
    app_icon.save(icons / "neptune.ico", sizes=[(s, s) for s in sizes if s <= 256])
    macos_icon.save(icons / "neptune.icns")

    public = REPO / "website/public"
    public.mkdir(parents=True, exist_ok=True)
    mark.resize((256, 256), Image.Resampling.LANCZOS).save(
        public / "neptune-logo.png", optimize=True
    )
    app = REPO / "website/src/app"
    mark.resize((64, 64), Image.Resampling.LANCZOS).save(app / "icon.png", optimize=True)
    mark.save(app / "favicon.ico", sizes=[(16, 16), (32, 32), (48, 48)])
    touch = Image.new("RGB", (180, 180), "#101012")
    small = mark.resize((180, 180), Image.Resampling.LANCZOS)
    touch.paste(small, mask=small.getchannel("A"))
    touch.save(app / "apple-icon.png", optimize=True)


if __name__ == "__main__":
    main()
