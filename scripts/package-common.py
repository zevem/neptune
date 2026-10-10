"""Release license payload shared by native installers (stdlib only)."""
import json
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def cef_runtime(binary_directory, platform):
    """Resolve the pinned distribution, rather than whatever DLLs are on PATH."""
    configured = os.environ.get("CEF_PATH")
    candidates = [Path(configured), *sorted(Path(configured).glob(f"154.0.34/cef_{platform}_*"))] if configured else []
    candidates += sorted((binary_directory / "build").glob(f"cef-dll-sys-*/out/cef_{platform}_*"))
    for path in candidates:
        archive = path / "archive.json"
        if archive.is_file() and json.loads(archive.read_text()).get("name", "").startswith("cef_binary_154.0.34+g14c5a08+"):
            return path
    raise ValueError("Build neptune-browser with the locked CEF distribution before packaging")


def browser_payload(binary_directory, destination, platform):
    runtime = cef_runtime(binary_directory, platform)
    destination.mkdir(parents=True, exist_ok=False)
    if platform == "macos":
        framework = runtime / "Chromium Embedded Framework.framework"
        shutil.copytree(framework, destination / framework.name, symlinks=True)
        return
    required = ["icudtl.dat", "resources.pak", "chrome_100_percent.pak", "chrome_200_percent.pak", "v8_context_snapshot.bin", "CREDITS.html"]
    if platform == "linux":
        linked = subprocess.check_output(["readelf", "-d", str(binary_directory / "neptune-browser")], text=True)
        if linked.find("[libcef.so]") < 0 or linked.find("[libcef.so]") > linked.find("[libc.so.6]"):
            raise ValueError("Browser ELF must load CEF before libc for Chromium's close interposer")
        required += ["libcef.so", "chrome-sandbox", "libvk_swiftshader.so", "libvulkan.so.1", "vk_swiftshader_icd.json"]
        shutil.copy2(binary_directory / "neptune-browser", destination / "neptune-browser")
        (destination / "neptune-browser").chmod(0o755)
    elif platform == "windows":
        required += ["libcef.dll", "chrome_elf.dll", "bootstrapc.exe"]
        shutil.copy2(binary_directory / "neptune_browser.dll", destination / "neptune_browser.dll")
        required += [p.name for p in runtime.glob("*.dll") if p.name not in required]
    else:
        raise ValueError("Unknown browser platform")
    for filename in required:
        shutil.copy2(runtime / filename, destination / filename)
    if platform == "linux":
        # The minimal CEF archive still carries large, unmapped debug/symbol
        # sections. Keep the downloaded SDK intact, and ship only its runtime
        # code and dynamic exports in the private installer payload.
        subprocess.run(["strip", "--strip-unneeded", str(destination / "libcef.so")], check=True)
    shutil.copytree(runtime / "locales", destination / "locales")
    shutil.copy2(ROOT / "packaging/cef-LICENSE.txt", destination / "CEF-LICENSE.txt")
    if platform == "windows":
        (destination / "bootstrapc.exe").rename(destination / "neptune_browser.exe")


def notices(destination):
    destination.mkdir(parents=True, exist_ok=True)
    for filename in ("LICENSE", "config.example.toml"):
        shutil.copy2(ROOT / filename, destination / filename)
    fonts = destination / "fonts"
    fonts.mkdir()
    for path in (ROOT / "assets/fonts").glob("*LICENSE*"):
        shutil.copy2(path, fonts / path.name)
    themes = destination / "themes"
    themes.mkdir()
    for filename in ("LICENSE", "CREDITS.md", "README.md", "source.json"):
        shutil.copy2(ROOT / "assets/themes" / filename, themes / filename)
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT))
    output = ["Bundled iTerm2-Color-Schemes: see themes/ for license, authors and provenance.\n"]
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        output.append(f"{package['name']} {package['version']}\nLicense: {package.get('license') or 'see license file'}\n")
        root = Path(package["manifest_path"]).parent
        paths = set()
        if package.get("license_file"):
            paths.add(root / package["license_file"])
        for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "COPYRIGHT*"):
            paths.update(root.glob(pattern))
        for path in sorted(paths):
            if path.is_file():
                output.append(f"\n--- {path.name} ---\n{path.read_text(errors='replace')}\n")
        output.append("\n" + "=" * 72 + "\n")
    (destination / "THIRD-PARTY-NOTICES.txt").write_text("\n".join(output), encoding="utf-8")
