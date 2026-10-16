"""Supplied Neptune brand artwork is not a replaceable functional UI icon."""

import hashlib
import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "export_logo", Path(__file__).with_name("export-logo.py")
)
export_logo = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(export_logo)

# Supplied originals, retained byte-for-byte from before the Tabler migration.
ORIGINALS = {
    "neptune-icon.png": "38441d8aac422713b26713dd415a6d99e44706258ab90b53bd4c9fd69a3ba2e0",
    "neptune-logo.png": "746024bd52be5ce1578239b85402cc2105fb23c85baf7064a1f9551573acb520",
    "neptune-macos-logo.png": "d03feba08bc2bbfc881877fdd03f482b55bde5fde7565d9656223d7e6590446b",
}


class BrandArtworkTests(unittest.TestCase):
    def test_supplied_brand_artwork_is_unchanged(self):
        for name, expected in ORIGINALS.items():
            with self.subTest(name=name):
                source = ROOT / "assets/branding" / name
                self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), expected)

    def test_platform_and_website_exports_use_supplied_artwork(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            shutil.copytree(ROOT / "assets/branding", root / "assets/branding")
            (root / "website/src/app").mkdir(parents=True)
            with patch.object(export_logo, "REPO", root):
                export_logo.main()
            outputs = list((root / "assets/icons").glob("neptune*"))
            outputs += list((root / "website/public").glob("*"))
            outputs += list((root / "website/src/app").glob("*"))
            self.assertEqual(len(outputs), 15)
            for path in outputs:
                relative = path.relative_to(root)
                with self.subTest(path=str(relative)):
                    self.assertEqual(path.read_bytes(), (ROOT / relative).read_bytes())
            for name in ORIGINALS:
                self.assertEqual(
                    (root / "assets/branding" / name).read_bytes(),
                    (ROOT / "assets/branding" / name).read_bytes(),
                )


if __name__ == "__main__":
    unittest.main()
