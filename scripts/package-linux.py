#!/usr/bin/env python3
"""Package a native glibc Linux x64 build as DEB and AppImage."""
import argparse
import importlib.util
import os
from pathlib import Path
import re
import shutil
import subprocess

from release import ROOT, artifact_names, version_info

spec = importlib.util.spec_from_file_location("package_common", ROOT / "scripts/package-common.py")
common = importlib.util.module_from_spec(spec)
spec.loader.exec_module(common)


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def library_package(path):
    # Ubuntu's merged /usr can expose /lib aliases through ldconfig/ldd while
    # dpkg records the /usr/lib path. Query both names before rejecting notices.
    for candidate in dict.fromkeys((path, path.resolve())):
        result = subprocess.run(["dpkg-query", "-S", str(candidate)], capture_output=True, text=True)
        if result.returncode == 0:
            return result.stdout.split(": ", 1)[0].split(":", 1)[0]
    raise ValueError(f"No installed package owns bundled library {path}")


def bundle_libraries(appdir):
    library_dir = appdir / "usr/lib"
    library_dir.mkdir(exist_ok=True)
    # Platform/graphics drivers and glibc belong to the host, with the GBM and
    # Wayland libraries those drivers link: a newer Mesa does not load against
    # older copies. Bundle the other window libraries winit loads at runtime as
    # well as linked transitive dependencies.
    exclude = re.compile(r"^(ld-linux|lib(c|m|dl|rt|pthread|resolv|nss_[^.]+)\.so|lib(GL|EGL|GLX|GLdispatch|vulkan|drm|gbm|wayland-[a-z]+))")
    ldconfig = subprocess.check_output(["/sbin/ldconfig", "-p"], text=True)
    names = ("libxkbcommon.so.0", "libxkbcommon-x11.so.0", "libX11.so.6", "libXcursor.so.1", "libXi.so.6", "libXrandr.so.2", "libXinerama.so.1")
    browser = appdir / "usr/lib/neptune/browser"
    queue = [appdir / "usr/bin/neptune", browser / "neptune-browser", browser / "libcef.so"]
    for name in names:
        matches = re.findall(rf"\s{re.escape(name)} \(.*x86-64.*\) => (\S+)", ldconfig)
        if not matches:
            raise ValueError(f"Missing runtime library {name}")
        queue.append(Path(matches[0]))
    seen = set()
    copyrights = appdir / "usr/share/doc/neptune/system-libraries"
    copyrights.mkdir()
    while queue:
        path = queue.pop()
        if path in seen:
            continue
        seen.add(path)
        if path.name != "neptune" and not path.is_relative_to(browser):
            shutil.copy2(path.resolve(), library_dir / path.name)
            owner = library_package(path)
            copyright_file = Path("/usr/share/doc") / owner / "copyright"
            if not copyright_file.is_file():
                raise ValueError(f"Missing bundled library notice for {owner}")
            shutil.copy2(copyright_file, copyrights / f"{owner}.copyright")
        dependencies = subprocess.check_output(["ldd", str(path)], text=True)
        if "not found" in dependencies:
            raise ValueError(f"Unresolved dependency in {path.name}")
        for name, resolved in re.findall(r"\s(\S+) => (/\S+)", dependencies):
            if not exclude.match(name):
                queue.append(Path(resolved))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--appimagetool", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    args = parser.parse_args()
    match, prerelease = version_info(args.version)
    names = artifact_names(args.version)
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    stage = ROOT / "package/linux"
    stage.mkdir(parents=True)
    (stage / "usr/bin").mkdir(parents=True)
    shutil.copy2(args.binary, stage / "usr/bin/neptune")
    (stage / "usr/bin/neptune").chmod(0o755)
    common.browser_payload(args.binary.parent, stage / "usr/lib/neptune/browser", "linux")
    # dpkg-deb installs this root-owned helper; AppImage uses user namespaces.
    (stage / "usr/lib/neptune/browser/chrome-sandbox").chmod(0o4755)
    desktop = stage / "usr/share/applications/rs.neptune.terminal.desktop"
    desktop.parent.mkdir(parents=True)
    shutil.copy2(ROOT / "packaging/neptune.desktop", desktop)
    for size in (16, 24, 32, 48, 64, 128, 256, 512):
        icon = stage / f"usr/share/icons/hicolor/{size}x{size}/apps/neptune.png"
        icon.parent.mkdir(parents=True)
        shutil.copy2(ROOT / f"assets/icons/neptune-{size}.png", icon)
    common.notices(stage / "usr/share/doc/neptune")
    control = stage / "DEBIAN"
    control.mkdir()
    # Debian sorts ~ prereleases below the stable version; SemVer build metadata
    # is represented as a Debian revision without changing executable version.
    deb_version = ".".join(match[i] for i in (1, 2, 3))
    if prerelease:
        deb_version += "~" + match[4]
    if match[5]:
        deb_version += "+" + match[5]
    (control / "control").write_text(f"Package: neptune\nVersion: {deb_version}\nSection: utils\nPriority: optional\nArchitecture: amd64\nMaintainer: Neptune maintainers <maintainers@neptune.rs>\nHomepage: https://neptune.rs\nInstalled-Size: {sum(p.stat().st_size for p in stage.rglob('*') if p.is_file()) // 1024}\nDepends: libc6 (>= 2.35), libgcc-s1, libstdc++6, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-cursor0, libwayland-egl1, libx11-6, libxcursor1, libxi6, libxrandr2, libxinerama1, libegl1, libvulkan1, libnss3, libnspr4, libatk1.0-0, libatk-bridge2.0-0, libcups2, libxcomposite1, libxdamage1, libxfixes3, libgbm1, libpango-1.0-0, libcairo2, libasound2\nDescription: Native GPU terminal for focused work\n Independent PTY shells and persistent workspace organization.\n")
    run("dpkg-deb", "--root-owner-group", "--build", str(stage), str(dist / names["linux-x64-deb"]))
    (stage / "usr/lib/neptune/browser/chrome-sandbox").chmod(0o755)
    shutil.rmtree(control)
    shutil.copy2(desktop, stage / "neptune.desktop")
    shutil.copy2(ROOT / "assets/icons/neptune-256.png", stage / "neptune.png")
    (stage / ".DirIcon").symlink_to("neptune.png")
    apprun = stage / "AppRun"
    apprun.write_text('#!/bin/sh\nAPPDIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"\nexport LD_LIBRARY_PATH="$APPDIR/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"\nexec "$APPDIR/usr/bin/neptune" "$@"\n')
    apprun.chmod(0o755)
    bundle_libraries(stage)
    # AppImage type2 runtime is MIT; include its pinned license with the payload.
    shutil.copy2(ROOT / "packaging/appimage-runtime-LICENSE", stage / "usr/share/doc/neptune/AppImage-runtime-LICENSE")
    run(str(args.appimagetool.resolve()), "--runtime-file", str(args.runtime.resolve()), "--mksquashfs-opt", "-processors", "--mksquashfs-opt", "2", str(stage), str(dist / names["linux-x64-appimage"]), env={**os.environ, "ARCH": "x86_64", "APPIMAGE_EXTRACT_AND_RUN": "1"})


if __name__ == "__main__":
    main()
