#!/usr/bin/env python3
"""Developer ID sign, notarize/staple app, then sign/notarize/staple DMG.

Credentials are read only from the GitHub environment and cleaned in finally.
No ad-hoc signing, deprecated altool, or unsigned fallback.
"""
import argparse
import base64
import importlib.util
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile

from release import ROOT, artifact_names


def run(*args):
    result = subprocess.run(args, capture_output=True)
    if result.returncode:
        # Never include command arguments (keychain/password) in exceptions.
        raise RuntimeError(f"{Path(args[0]).name} {args[1]} failed (exit {result.returncode})")
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--platform", choices=("macos-arm64", "macos-x64"), required=True)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    required = ("MACOS_CERTIFICATE_P12_BASE64", "MACOS_CERTIFICATE_PASSWORD", "MACOS_SIGN_IDENTITY", "APPLE_API_KEY_P8_BASE64", "APPLE_API_KEY_ID", "APPLE_API_ISSUER_ID")
    if any(not os.environ.get(name) for name in required):
        raise ValueError("Missing macOS release credentials; see docs/releases.md")
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        certificate = root / "developer-id.p12"
        certificate.write_bytes(base64.b64decode(os.environ["MACOS_CERTIFICATE_P12_BASE64"], validate=True))
        certificate.chmod(0o600)
        api_key = root / "AuthKey.p8"
        api_key.write_bytes(base64.b64decode(os.environ["APPLE_API_KEY_P8_BASE64"], validate=True))
        api_key.chmod(0o600)
        keychain = root / "release.keychain-db"
        password = secrets.token_urlsafe(32)
        original_keychains = [line.strip().strip('"') for line in run("security", "list-keychains", "-d", "user").decode().splitlines()]
        try:
            run("security", "create-keychain", "-p", password, str(keychain))
            run("security", "set-keychain-settings", "-lut", "21600", str(keychain))
            run("security", "unlock-keychain", "-p", password, str(keychain))
            run("security", "import", str(certificate), "-k", str(keychain), "-P", os.environ["MACOS_CERTIFICATE_PASSWORD"], "-T", "/usr/bin/codesign")
            run("security", "set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", password, str(keychain))
            run("security", "list-keychains", "-d", "user", "-s", str(keychain), *original_keychains)
            stage = root / "volume"
            stage.mkdir()
            app = stage / "Neptune.app"
            run("python3", str(ROOT / "scripts/package-macos.py"), "--binary", str(args.binary), "--output", str(app))
            spec = importlib.util.spec_from_file_location("common", ROOT / "scripts/package-common.py")
            common = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(common)
            common.notices(app / "Contents/Resources/licenses")
            identity = os.environ["MACOS_SIGN_IDENTITY"]
            def sign_browser(path, entitlements=False):
                arguments = ["codesign", "--force", "--timestamp", "--options", "runtime", "--keychain", str(keychain), "--sign", identity]
                if entitlements:
                    arguments += ["--entitlements", str(ROOT / "packaging/browser-entitlements.plist")]
                run(*arguments, str(path))
            frameworks = app / "Contents/Frameworks"
            framework = frameworks / "Chromium Embedded Framework.framework"
            for library in sorted(framework.rglob("*.dylib")):
                if not library.is_symlink():
                    sign_browser(library)
            sign_browser(framework)
            for helper in sorted(frameworks.glob("*.app")):
                for executable in (helper / "Contents/MacOS").iterdir():
                    sign_browser(executable, True)
                sign_browser(helper, True)
            sign_browser(app / "Contents/MacOS/neptune-browser", True)
            run("codesign", "--force", "--timestamp", "--options", "runtime", "--keychain", str(keychain), "--sign", identity, str(app / "Contents/MacOS/neptune"))
            run("codesign", "--force", "--timestamp", "--options", "runtime", "--keychain", str(keychain), "--sign", identity, str(app))
            run("codesign", "--verify", "--deep", "--strict", str(app))
            archive = root / "Neptune.zip"
            run("ditto", "-c", "-k", "--keepParent", str(app), str(archive))

            def notarize(path):
                response = json.loads(run("xcrun", "notarytool", "submit", str(path), "--key", str(api_key), "--key-id", os.environ["APPLE_API_KEY_ID"], "--issuer", os.environ["APPLE_API_ISSUER_ID"], "--wait", "--timeout", "30m", "--output-format", "json"))
                if response.get("status") != "Accepted":
                    raise RuntimeError(f"Apple notarization was not accepted; submission {response.get('id', 'unknown')}. Retrieve the notarytool log.")

            notarize(archive)
            run("xcrun", "stapler", "staple", str(app))
            run("xcrun", "stapler", "validate", str(app))
            run("spctl", "--assess", "--type", "execute", "--verbose=2", str(app))
            (stage / "Applications").symlink_to("/Applications")
            dmg = dist / artifact_names(args.version)[args.platform]
            run("hdiutil", "create", "-volname", "Neptune", "-srcfolder", str(stage), "-format", "UDZO", "-ov", str(dmg))
            run("codesign", "--timestamp", "--keychain", str(keychain), "--sign", identity, str(dmg))
            notarize(dmg)
            run("xcrun", "stapler", "staple", str(dmg))
            run("xcrun", "stapler", "validate", str(dmg))
            run("spctl", "--assess", "--type", "open", "--context", "context:primary-signature", "--verbose=2", str(dmg))
        finally:
            subprocess.run(["security", "list-keychains", "-d", "user", "-s", *original_keychains], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["security", "delete-keychain", str(keychain)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
