#!/usr/bin/env python3
"""Export Neptune's supplied platform logos and marketing icon.

Requires Pillow (python3 -m pip install Pillow). Native exports preserve each
source's artwork, transparency and padding; no additional macOS padding is added.
"""

from pathlib import Path

from PIL import Image


REPO = Path(__file__).resolve().parents[1]


def main() -> None:
    branding = REPO / "assets/branding"
    app_icon = Image.open(branding / "neptune-logo.png").convert("RGBA")
    macos_icon = Image.open(branding / "neptune-macos-logo.png").convert("RGBA")
    mark = Image.open(branding / "neptune-icon.png").convert("RGBA")
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
    mark.resize((64, 64), Image.Resampling.LANCZOS).save(
        app / "icon.png", optimize=True
    )
    mark.save(app / "favicon.ico", sizes=[(16, 16), (32, 32), (48, 48)])
    # Web clips need an opaque background; use the marketing mark here too.
    touch = Image.new("RGB", (180, 180), "#101012")
    small = mark.resize((180, 180), Image.Resampling.LANCZOS)
    touch.paste(small, mask=small.getchannel("A"))
    touch.save(app / "apple-icon.png", optimize=True)


if __name__ == "__main__":
    main()
