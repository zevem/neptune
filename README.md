# Neptune

<img src="assets/branding/neptune-icon.png" alt="Neptune logo" width="96" height="96">

[neptune.rs](https://neptune.rs) · [Downloads](https://neptune.rs/download) · [User guide](docs/usage.md)

A native Rust terminal for focused work, inspired by cmux and Ghostty. Independent
shell sessions, GPU rendering, and a quiet workspace interface, with no webview.

## Features

- Workspaces, tabs, split panes, and groups with drag-and-drop organization.
- Local shells and SSH workspaces backed by real PTY sessions.
- Scrollback, terminal search, selection/clipboard, and a command palette.
- [Custom keybindings](docs/usage.md#custom-keybindings) configured in TOML.
- 715 built-in themes and custom themes shared by the window and terminal.
- Terminal notifications with optional native desktop banners.
- Saved workspace layouts and directories, plus optional Claude Code, Codex,
  OpenCode, pi and Oh My Pi session resumption on local Unix terminals, where
  an agent's pull requests appear on its tab with their checks, unresolved
  review comments and merged state beside the screenshots and files it
  attaches, and a panel lists which agents are working, idle or
  waiting for you. Either agent can start the other out of view and
  work with it.

## Download

Get [published stable builds](https://neptune.rs/download) or
[prereleases](https://neptune.rs/download/beta). See the
[installation guide](docs/installation.md) for platform packages, AppImage launch
instructions, and download verification.

Linux is the local verification platform. macOS and Windows need native runtime
verification before a production release; see the
[remaining release gates](docs/verification.md#remaining-production-release-gates).

## Build and run

Use the pinned Rust 1.97.1 toolchain through rustup. A graphical desktop and a
working graphics driver are required. On Debian/Ubuntu:

```sh
sudo apt-get install pkg-config libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
cargo run --release --locked --bin neptune
```

See [building from source](docs/installation.md#build-and-run) for macOS and
Windows prerequisites, executable locations, and desktop integration.

### Development builds

Build job limits, optional compiler caching, and scoped verification are in the
[development guide](docs/development.md#development-builds).

## Documentation

| Guide | Contents |
| --- | --- |
| [Installation](docs/installation.md) | Downloads, source builds, and desktop integration |
| [User guide](docs/usage.md) | Workspaces, SSH, shortcuts, preferences, and restoration |
| [Configuration example](config.example.toml) | Supported settings and defaults |
| [Notifications](docs/notifications.md) | Terminal alerts and coding-agent setup |
| [Agent sessions](docs/agent-sessions.md) | Coding-agent session resumption and activity, locally and on SSH hosts |
| [Development](docs/development.md) | Build tooling, native inspection, CI, and artifacts |
| [Architecture](docs/architecture.md) · [Interface design](docs/design.md) | Ownership, terminal engine, renderer, and UI direction |
| [Verification](docs/verification.md) · [Performance](docs/performance.md) | Native evidence, release gates, and reproducible measurements |
| [Releases](docs/releases.md) · [Changelog](CHANGELOG.md) | Packaging, signing, updates, and What's New |
| [Website](website/README.md) | The neptune.rs site and download pages |

## License

Neptune is [MIT licensed](LICENSE). Bundled fonts retain their SIL Open Font
License notices; third-party dependencies and vendored development skills retain
their respective licenses. See [third-party notices](THIRD-PARTY-NOTICES.md) for
skill attribution and the full upstream license texts.
