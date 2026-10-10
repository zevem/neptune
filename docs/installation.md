# Installation

Neptune requires a graphical desktop and a working graphics driver. See the
[verification record](verification.md) for platform checks still required before
a production release.

[Project overview](../README.md) · [User guide](usage.md) · [Development guide](development.md)

Run source-build and desktop-integration commands from the repository root.

## Downloads

Download published stable builds at [neptune.rs/download](https://neptune.rs/download)
and prereleases at [neptune.rs/download/beta](https://neptune.rs/download/beta).

GitHub Releases hosts the binaries; the Vercel website caches and verifies release
metadata server-side and always offers all platform/architecture choices.

See [download verification](releases.md#verify-a-download) for checksums, signed
metadata and workflow provenance.

### Linux AppImage

On Linux, a downloaded AppImage needs execute permission before it can run.
In the file's Properties → Permissions, enable execution as a program, or use
the following commands with your downloaded version and location:

```sh
chmod u+x ./Neptune-VERSION-linux-x64.AppImage
./Neptune-VERSION-linux-x64.AppImage
```

Opening the file in Gear Lever shows its management screen; mark the file trusted
and use its launch or integration control. If a desktop double-click does nothing,
run the executable AppImage from a terminal to see the error. The DEB uses the
system package installer and does not need execute permission.

## Build and run

Install a pinned Rust 1.97.1 toolchain (installed automatically by rustup) with Cargo. A graphical desktop and a working graphics driver are required. On Debian/Ubuntu, install the native build dependencies:

```sh
sudo apt-get install pkg-config libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
cargo run --release --locked --bin neptune
```

On macOS, install Xcode Command Line Tools. On Windows, use the MSVC Rust toolchain with the Visual Studio C++ Build Tools and Windows SDK; ConPTY requires Windows 10 version 1809 or later.

```sh
cargo build --release --locked --bin neptune
```

The executable is `target/release/neptune` on Linux/macOS and `target\release\neptune.exe` on Windows.

Website previews also require `cargo build -p neptune-browser --release --locked`
and CMake/Ninja. Keep its pinned CEF runtime with the helper; macOS and Windows
need their bundled helper layout. See [browser development and platform setup](browser.md).
The terminal starts normally when that optional development payload is absent;
opening a browser then shows a recoverable error.

For optional compiler caching and build job limits, see
[development builds](development.md#development-builds).

## Desktop integration

The [Neptune logo and icon exports](../assets/README.md) live in `assets/`. Windows
builds embed the multi-size icon in `neptune.exe`. On macOS, wrap the built
executable in an app bundle to use the icon in Finder and the Dock:

```sh
python3 scripts/package-macos.py
open target/release/Neptune.app
```

Build `neptune-browser` first; the bundle includes Chromium and its helper apps.

Local bundles are unsigned. Official release DMGs require Developer ID signing, Apple notarization and stapling; see [the release guide](releases.md).
On Linux, install the [desktop entry](../packaging/neptune.desktop) and PNG icons
in the standard application/icon locations.

For a per-user Linux install (ensure `~/.local/bin` is on your PATH):

```sh
install -Dm755 target/release/neptune ~/.local/bin/neptune
install -Dm644 packaging/neptune.desktop ~/.local/share/applications/rs.neptune.terminal.desktop
for size in 16 24 32 48 64 128 256 512 1024; do
  install -Dm644 "assets/icons/neptune-${size}.png" "$HOME/.local/share/icons/hicolor/${size}x${size}/apps/neptune.png"
done
```
