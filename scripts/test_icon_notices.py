"""The embedded icon notice must travel with every native package."""

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "package_common", Path(__file__).with_name("package-common.py")
)
common = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(common)


class IconNoticeTests(unittest.TestCase):
    def fixtures(self, root):
        for name in ("LICENSE", "config.example.toml"):
            (root / name).write_text(name)
        (root / "assets/fonts").mkdir(parents=True)
        themes = root / "assets/themes"
        themes.mkdir()
        for name in ("LICENSE", "CREDITS.md", "README.md", "source.json"):
            (themes / name).write_text(name)
        icons = root / "assets/icons/tabler"
        icons.mkdir(parents=True)
        return icons / "LICENSE.txt"

    def test_native_package_retains_the_exact_tabler_mit_notice(self):
        notice = (common.ROOT / "assets/icons/tabler/LICENSE.txt").read_bytes()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixtures(root).write_bytes(notice)
            destination = root / "package"
            with patch.object(common, "ROOT", root), patch.object(
                common.subprocess, "check_output", return_value=b'{"packages": []}'
            ):
                common.notices(destination)
            self.assertEqual((destination / "icons/Tabler-LICENSE.txt").read_bytes(), notice)
            self.assertIn(
                "Bundled Tabler Icons 3.49.0",
                (destination / "THIRD-PARTY-NOTICES.txt").read_text(),
            )

    def test_missing_tabler_notice_prevents_packaging(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixtures(root)
            with patch.object(common, "ROOT", root), self.assertRaises(FileNotFoundError):
                common.notices(root / "package")


if __name__ == "__main__":
    unittest.main()
