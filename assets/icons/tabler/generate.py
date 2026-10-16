#!/usr/bin/env python3
"""Regenerate Neptune's embedded Tabler font subset and Rust glyph mapping.

Run with fonttools==4.60.1. The official pinned npm archive is verified before
reading any members; --archive accepts a local copy for offline regeneration.
This maintenance tool is not part of Cargo builds or application startup.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import sys
import tarfile
import urllib.request

import fontTools
from fontTools import subset
from fontTools.ttLib import TTFont


ROOT = Path(__file__).resolve().parent
VERSION = "3.49.0"
FONTTOOLS_VERSION = "4.60.1"
ARCHIVE_URL = (
    "https://registry.npmjs.org/@tabler/icons-webfont/-/"
    "icons-webfont-3.49.0.tgz"
)
ARCHIVE_SHA256 = "48c69c65a5f9013d482a17cab8899a09dcaec50750d1ac7324eb63a061fffb82"
MAX_ARCHIVE_BYTES = 16 * 1024 * 1024
FONT_SOURCES = {
    "outline": (
        "tabler-icons",
        "d291676dc4c28b386552753bf16cf2bef0e93e5543f8e0b5c5d72c0da41ae1bc",
    ),
    "filled": (
        "tabler-icons-filled",
        "08304212f3f43271ee700156044a298a081ad2a24b30e28b4f3346dd2fe7ecf9",
    ),
}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def archive_bytes(path: Path | None) -> bytes:
    if path is not None:
        with path.open("rb") as source:
            data = source.read(MAX_ARCHIVE_BYTES + 1)
    else:
        with urllib.request.urlopen(ARCHIVE_URL, timeout=30) as source:
            data = source.read(MAX_ARCHIVE_BYTES + 1)
    if len(data) > MAX_ARCHIVE_BYTES:
        raise ValueError("Tabler archive exceeds the expected size limit")
    if digest(data) != ARCHIVE_SHA256:
        raise ValueError("Tabler archive SHA-256 does not match the pinned release")
    return data


def member(archive: tarfile.TarFile, name: str) -> bytes:
    source = archive.extractfile(f"package/{name}")
    if source is None:
        raise ValueError(f"Missing archive member: {name}")
    with source:
        return source.read()


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def generate(data: bytes) -> dict[str, bytes]:
    if fontTools.__version__ != FONTTOOLS_VERSION:
        raise ValueError(
            f"Use fonttools=={FONTTOOLS_VERSION}; found {fontTools.__version__}"
        )
    manifest = json.loads((ROOT / "mapping.json").read_text())
    if manifest["version"] != VERSION:
        raise ValueError("SVG mapping version does not match the pinned font release")
    entries = manifest["icons"]
    if len({entry["icon"] for entry in entries}) != len(entries):
        raise ValueError("Duplicate Neptune Icon variant in mapping.json")
    if any(entry["style"] not in FONT_SOURCES for entry in entries):
        raise ValueError("Unsupported icon style in mapping.json")
    source = (ROOT.parents[2] / "src/icons.rs").read_text()
    icon_enum = re.search(r"pub enum Icon \{(.*?)\n\}", source, re.DOTALL)
    if icon_enum is None:
        raise ValueError("Cannot locate Neptune Icon enum")
    variants = re.findall(r"^\s+([A-Za-z][A-Za-z0-9_]*),$", icon_enum[1], re.MULTILINE)
    if set(variants) != {entry["icon"] for entry in entries}:
        raise ValueError("mapping.json must cover every Neptune Icon variant exactly")
    outputs: dict[str, bytes] = {}
    metadata: dict[str, object] = {
        "schema_version": 1,
        "package": "@tabler/icons-webfont",
        "version": VERSION,
        "revision": manifest["revision"],
        "archive": {"url": ARCHIVE_URL, "sha256": ARCHIVE_SHA256},
        "generator": {"file": "generate.py", "fonttools_version": FONTTOOLS_VERSION},
        "viewbox": [0, 0, 24, 24],
        "bearing_policy": "Recalculate bounds and set left side bearings to contour xMin; preserve upstream coordinates and 1000-unit advance.",
        "fonts": {},
    }
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        license_bytes = member(archive, "LICENSE")
        if (
            digest(license_bytes) != manifest["license"]["sha256"]
            or (ROOT / manifest["license"]["file"]).read_bytes() != license_bytes
        ):
            raise ValueError("Font and SVG package licenses do not match")
        for style, (basename, source_hash) in FONT_SOURCES.items():
            source_path = f"dist/fonts/{basename}.ttf"
            source = member(archive, source_path)
            if digest(source) != source_hash:
                raise ValueError(f"Unexpected source font SHA-256: {style}")
            css = member(archive, f"dist/{basename}.css").decode()
            css_mapping = dict(
                re.findall(r'\.ti-([\w-]+):before\s*\{\s*content:\s*"\\([a-f0-9]+)";', css)
            )
            selected = [entry for entry in entries if entry["style"] == style]
            for entry in selected:
                if css_mapping.get(entry["name"]) != entry["unicode"]:
                    raise ValueError(f"Font code point mismatch: {entry['icon']}")
                svg = (ROOT / entry["file"]).read_bytes()
                if digest(svg) != entry["sha256"]:
                    raise ValueError(f"SVG source SHA-256 mismatch: {entry['file']}")
            unicodes = {int(entry["unicode"], 16) for entry in selected}
            font = TTFont(io.BytesIO(source), recalcBBoxes=True, recalcTimestamp=False)
            if font["head"].unitsPerEm != 1000:
                raise ValueError("Unexpected Tabler units per em")
            metrics = font["hhea"]
            if (metrics.ascent, metrics.descent, metrics.lineGap) != (900, -100, 0):
                raise ValueError("Unexpected Tabler viewbox metrics")
            source_cmap = font.getBestCmap()
            if not unicodes.issubset(source_cmap):
                raise ValueError(f"Missing source font glyph: {style}")
            # Snapshot contour coordinates before subsetting. Retained paths
            # must remain identical; only corrected bounds/bearings and unused
            # tables/glyphs may change.
            contours = {
                codepoint: list(font["glyf"][source_cmap[codepoint]].coordinates)
                for codepoint in unicodes
            }
            options = subset.Options()
            options.notdef_glyph = True
            options.notdef_outline = True
            options.recalc_timestamp = False
            options.name_IDs = ["*"]
            options.name_legacy = True
            options.name_languages = ["*"]
            subsetter = subset.Subsetter(options=options)
            subsetter.populate(unicodes=unicodes)
            subsetter.subset(font)
            for glyph_name in font.getGlyphOrder():
                glyph = font["glyf"][glyph_name]
                glyph.recalcBounds(font["glyf"])
                if hasattr(glyph, "xMin"):
                    advance, _ = font["hmtx"][glyph_name]
                    font["hmtx"][glyph_name] = (advance, glyph.xMin)
            output = io.BytesIO()
            font.save(output)
            result = output.getvalue()
            verified = TTFont(io.BytesIO(result), recalcTimestamp=False)
            cmap = verified.getBestCmap()
            if set(cmap) != unicodes:
                raise ValueError(f"Unexpected subset character coverage: {style}")
            for codepoint in unicodes:
                name = cmap[codepoint]
                glyph = verified["glyf"][name]
                if list(glyph.coordinates) != contours[codepoint]:
                    raise ValueError(f"Contour geometry changed: U+{codepoint:04X}")
                if verified["hmtx"][name] != (1000, glyph.xMin):
                    raise ValueError(f"Invalid viewbox bearing: U+{codepoint:04X}")
            outputs[f"{style}.ttf"] = result
            metadata["fonts"][style] = {
                "source_file": source_path,
                "source_sha256": source_hash,
                "source_bytes": len(source),
                "file": f"{style}.ttf",
                "sha256": digest(result),
                "bytes": len(result),
                "glyph_count": len(unicodes),
                "units_per_em": 1000,
                "ascent": 900,
                "descent": -100,
                "line_gap": 0,
            }
    rust = [
        "// Generated by assets/icons/tabler/generate.py. Do not edit.",
        "// Tabler Icons 3.49.0; MIT license in LICENSE.txt.",
        "#[cfg(test)]",
        f"pub(super) const ALL: [Icon; {len(entries)}] = [",
    ]
    rust.extend(f"    Icon::{entry['icon']}," for entry in entries)
    rust.extend([
        "];",
        "",
        "pub(super) fn mapping(icon: Icon) -> (Variant, char) {",
        "    match icon {",
    ])
    for entry in entries:
        rust.append(
            f"        Icon::{entry['icon']} => "
            f"(Variant::{entry['style'].title()}, '\\u{{{entry['unicode']}}}'),"
        )
    rust.extend(["    }", "}", ""])
    outputs["glyphs.rs"] = "\n".join(rust).encode()
    outputs["font-source.json"] = json_bytes(metadata)
    return outputs


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Verify checked-in outputs without changing them")
    parser.add_argument("--archive", type=Path, help="Read an existing pinned npm archive instead of downloading it")
    args = parser.parse_args()
    try:
        outputs = generate(archive_bytes(args.archive))
        mismatches = []
        for name, data in outputs.items():
            destination = ROOT / name
            if args.check:
                if not destination.exists() or destination.read_bytes() != data:
                    mismatches.append(name)
            else:
                destination.write_bytes(data)
        if mismatches:
            raise ValueError(f"Generated output differs: {', '.join(mismatches)}")
        action = "Verified" if args.check else "Generated"
        print(f"{action} {len(outputs)} Tabler outputs with identical upstream contours")
        return 0
    except (OSError, ValueError, KeyError, tarfile.TarError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
