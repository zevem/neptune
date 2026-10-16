"""Guard the native provider artwork and render contract restored from 098c515^.

These deliberately pin the original bytes, independently of Git history, so a
future UI-icon migration cannot silently substitute generic provider glyphs.
"""

import hashlib
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
PROVIDER_ASSETS = {
    "claude.svg": "767fed31fe0c29cbc51f8c1c11b1a4531159a10d879ecadb3f5c105400031200",
    "claude.png": "26264ee308ae2e102d53a5ba7a83a6404d0eb66831a039e7eefc58ce3430e5ff",
    "codex.svg": "836a5f380d7bff603f25c373d407680ba2e4a8294f7014d44a1d2218a1632635",
    "codex.png": "dc4d8d717547e8987653435d7381596774127d7c56061c0abdef82e91d11e61d",
    "opencode.svg": "eb2f189c99666daca6d58e2837d7e5a9276bf7158583f1f0c28746421e8de08a",
    "opencode.png": "ce877064642a9ef603fa2fb3f1146192b2ddda1331de002dd6f939b4b0406b00",
    "pi.svg": "22899e952808621a1dbe501f661e696bd16be0ebf1917f5beee3bb6b3b8d82a3",
    "pi.png": "c46129adcd285b9aeac20848b68237e9ac0077221e4f5605bc5c1503a921225e",
}
PROVIDER_RENDER_SHA256 = "70e7a638f685524f3e5628bb5ad31ab29bebbbbbf20da1b3f7784239e5c8347c"


class ProviderIconsTests(unittest.TestCase):
    def test_provider_artwork_matches_original_svg_and_png_bytes(self):
        for name, expected in PROVIDER_ASSETS.items():
            with self.subTest(asset=name):
                asset = ROOT / "assets/icons/providers" / name
                self.assertTrue(asset.is_file(), f"Missing original provider mark: {name}")
                self.assertEqual(hashlib.sha256(asset.read_bytes()).hexdigest(), expected)

    def test_provider_renderer_matches_original_mask_tint_scale_and_cache_contract(self):
        source = (ROOT / "src/icons.rs").read_bytes()
        implementation = re.search(rb"^pub fn paint_provider\(.*?^\}", source, re.M | re.S)
        self.assertIsNotNone(implementation, "Missing native provider renderer")
        self.assertEqual(
            hashlib.sha256(implementation[0]).hexdigest(),
            PROVIDER_RENDER_SHA256,
            "Provider rendering must retain the original masks, tints, resizing, "
            "pixel-scale cache, bounds and unsupported-agent behavior",
        )


if __name__ == "__main__":
    unittest.main()
