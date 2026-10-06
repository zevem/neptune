#!/usr/bin/env python3
"""Rasterize the provider marks in assets/icons/providers/ for the tab strip.

Requires PyGObject with librsvg and pycairo. Each SVG becomes a 64-pixel white
mask with the mark centred at its own proportions; Neptune tints it when drawn.
"""

from pathlib import Path

import cairo
import gi

gi.require_version("Rsvg", "2.0")
from gi.repository import Rsvg  # noqa: E402

REPO = Path(__file__).resolve().parents[1]
SIDE = 64


def main() -> None:
    for source in sorted((REPO / "assets/icons/providers").glob("*.svg")):
        handle = Rsvg.Handle.new_from_file(str(source))
        _, _, _, _, has_box, box = handle.get_intrinsic_dimensions()
        if not has_box:
            raise SystemExit(f"{source.name} has no viewBox")
        scale = SIDE / max(box.width, box.height)
        area = Rsvg.Rectangle()
        area.width, area.height = box.width * scale, box.height * scale
        area.x, area.y = (SIDE - area.width) / 2, (SIDE - area.height) / 2
        surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, SIDE, SIDE)
        handle.render_document(cairo.Context(surface), area)
        surface.write_to_png(str(source.with_suffix(".png")))


if __name__ == "__main__":
    main()
