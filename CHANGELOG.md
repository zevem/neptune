# Changelog

User-facing release history. Add meaningful changes under **Unreleased** as work
lands. `scripts/release.py prepare` moves them into a dated version section.
The release workflow copies that section verbatim into GitHub's What's New and
the signed desktop update manifest; do not maintain a second release-notes file.
Earlier private drafts were withdrawn before Neptune's first public preview;
their preparation records remain in Git history.

## [Unreleased]

### What's New

- See every Claude Code and Codex agent running in any workspace in the new
  Agents tab of the right panel: which are working, which are idle and which
  are waiting for you to allow a tool, answer a question or approve a plan.
  Click an agent to go to its terminal. The toolbar's panel button, which now
  opens the file explorer and the agents as tabs, carries a dot while an agent
  waits out of view. Codex asks once to trust the hooks that report this.
- Let Claude Code and Codex work together. Ask one for the other, such as
  "build the backend and let Claude Code handle the frontend", and it starts
  that agent out of view, hands it the task, reads its answer and can keep
  talking to it. The tab of an agent that started others shows how many;
  click the count to list them and open one's terminal in a tab. A started
  agent can be given its own directory, such as a git worktree, a model, an
  effort level and its CLI's ultra mode (ultracode or Ultra).
  One you closed can be opened again with its conversation, and a question a
  started agent's CLI asks before it begins is passed on to you at once.

### Acceptance notes and known limitations

- Saved workspaces now use schema version 9, which records which terminal's
  agent started another's and which of those terminals have no tab. Older
  workspace files still load, but RC3 and
  earlier cannot save over layouts written by this version.

### Fixes

- Run `cargo`, `rustc` and other rustup tools in zsh when Neptune is launched
  from the AppImage. Terminals and the programs Neptune opens no longer inherit
  the AppImage's launcher variables or its bundled libraries.

## [0.1.0-rc.3] - 2026-10-03

### What's New

- Read release notes in a redesigned update sheet, with wrapped lists, a status
  that stays in view while the notes scroll and actions at the trailing edge.
- Paste a screenshot or another clipboard image into a terminal: Neptune saves
  it and pastes its path, which Claude Code and Codex attach as an image.
- Drag files onto a terminal to paste their paths. The terminal that will take
  them is highlighted while the files are held over the window.
- Ctrl+V reaches the program in the terminal when the clipboard holds no text,
  so CLI agents can read a copied image themselves.
- Rest the pointer on the path of a picture in a terminal to preview it beside
  the path, such as a screenshot or chart a CLI agent reports having written.
  Click the preview to see the picture as large as the window allows, and
  scroll to zoom in.
- See the pull requests a CLI agent made on its terminal's tab. Claude Code and
  Codex started in a local Unix terminal can link a pull request to it; click
  its number beside the tab's close control to open it in the browser.
- Browse the focused terminal's folder in a file explorer that slides in from
  the right: toggle it from the toolbar or with Ctrl+Shift+O. It shows hidden
  files, previews text and pictures, searches with a "files to exclude" filter,
  and creates, renames, deletes, reveals and copies the path of files and
  folders.
- Set a default starting directory for a workspace group from its sidebar menu
  or the command palette. Type a path or choose a folder with Browse; new local
  workspaces in the group start there.
- Choose an installed terminal font in Preferences → Text, or enter a custom
  family or PostScript name. Apply it to every terminal without restarting
  shells; unavailable fonts fall back to bundled JetBrains Mono.
- Follow the focused terminal's current directory in the workspace sidebar,
  including after changing directories or switching tabs and splits.
- Receive Codex's background completion bells as terminal alerts, with focus
  reporting kept current while minimized or showing a dialog.
- Keep Linux desktop notification banners visible in GNOME instead of losing
  them immediately after delivery.

### Acceptance notes and known limitations

- This candidate is a private draft for acceptance testing. Full native acceptance
  on every platform remains pending; it is not a production-stable release.
- Saved workspaces now use schema version 8. Older workspace files still load,
  but RC1 and RC2 cannot save over layouts written by RC3. Back up workspace state
  before testing if you need to return to an earlier candidate.
- The reported intermittent native Wayland freeze still needs a capture of the
  blocking state. Earlier XWayland fixes do not establish native Wayland acceptance.
- Windows installers remain unsigned and may show an unverified-publisher or
  SmartScreen warning. Both macOS installers require signing and notarization;
  all installers are covered by signed update metadata and build attestations.
- Linux x64 packages require glibc 2.35 or newer and working host graphics
  drivers. Browser-downloaded AppImages need execute permission before launch;
  enable it in file Properties or run `chmod u+x` on the downloaded file.
- Workspace restoration starts fresh shells and SSH connections; arbitrary
  running commands and process memory are not restored. Coding-agent resumption
  and terminal pull-request links remain limited to supported providers on local
  Unix terminals.
- SSH requires an installed OpenSSH client and a POSIX remote shell. Remote
  directory tracking is integrated for zsh; other shells need OSC 7 integration.
- Kitty graphics and comprehensive complex-script shaping remain unsupported.
  Keypad identity and some keyboard-layout information depend on the window
  toolkit. IME, accessibility and mixed-DPI behavior still need native acceptance.
- Private drafts are excluded from website downloads and automatic updates.
  Automatic updates never downgrade; install this candidate manually for testing.

## [0.1.0-rc.2] - 2026-10-03

### What's New

- Keep several terminals as tabs within each split. Open a tab with
  Ctrl+Shift+T, step through tabs with Ctrl+Shift+PageDown/PageUp, and drag tabs
  between splits or workspaces. Ctrl+Shift+N opens a new workspace.
- Find settings in reorganized Preferences, with dedicated General, Appearance,
  Text, Shell, Notifications and Updates panes and searchable controls.
- Fit more workspaces in the sidebar with shorter rows and tighter group spacing.
- Avoid multi-second GPU presentation stalls observed after idle on Linux with
  NVIDIA graphics under XWayland.
- Restore Claude Code sessions after Powerlevel10k's instant prompt releases
  the terminal, avoiding zsh startup warnings and redirected terminal I/O.
- Avoid briefly flashing the close-confirmation overlay when closing a terminal.
- Show the latest published Beta download on the website until the first stable
  release is available, with a live preview reflecting the current desktop UI.

### Acceptance notes and known limitations

- This candidate is a private draft for acceptance testing. Full native acceptance
  on every platform remains pending; it is not a production-stable release.
- Saved workspaces now use schema version 7. Older workspace files still load,
  but RC1 cannot save over layouts written by RC2. Back up workspace state before
  testing if you need to return to RC1.
- The reported intermittent native Wayland freeze still needs a capture of the
  blocking state. The XWayland fix does not establish native Wayland acceptance.
- Windows installers remain unsigned and may show an unverified-publisher or
  SmartScreen warning. Both macOS installers require signing and notarization;
  all installers are covered by signed update metadata and build attestations.
- Linux x64 packages require glibc 2.35 or newer and working host graphics
  drivers. Browser-downloaded AppImages need execute permission before launch;
  enable it in file Properties or run `chmod u+x` on the downloaded file.
- Workspace restoration starts fresh shells and SSH connections; arbitrary
  running commands and process memory are not restored. Coding-agent resumption
  remains limited to supported provider sessions on local Unix terminals.
- SSH requires an installed OpenSSH client and a POSIX remote shell. Remote
  directory tracking is integrated for zsh; other shells need OSC 7 integration.
- Kitty graphics and comprehensive complex-script shaping remain unsupported.
  Keypad identity and some keyboard-layout information depend on the window
  toolkit. IME, accessibility and mixed-DPI behavior still need native acceptance.
- Private drafts are excluded from website downloads and automatic updates.
  Automatic updates never downgrade; install this candidate manually for testing.

## [0.1.0-rc.1] - 2026-10-02

### What's New

- Meet Neptune's first public preview: a native Rust terminal with GPU rendering
  and independent shell sessions, without a browser or webview.
- Organize terminals into workspaces and groups, split and rearrange panes,
  search scrollback, and find actions in the command palette.
- Reopen saved layouts and directories with fresh shells. Close confirmations
  help protect running work, including terminals in hidden workspaces.
- Connect entire workspaces through your system's OpenSSH client. New splits
  and restored connections use each terminal's reported remote directory.
- Choose from 715 bundled themes, save favorites, and create custom palettes.
  Adjust terminal typography and save your preferred window zoom and size.
- Pick installed shells in Preferences, including PowerShell, WSL, Git Bash and
  Windows Terminal profiles on Windows, or use a custom program.
- Receive terminal alerts through OSC 9, 99 and 777, with pane highlights,
  workspace badges, a notification popover and optional desktop banners.
- Reopen supported Claude Code and Codex conversations in their original local
  Unix panes using the providers' saved session IDs.
- Choose Stable or Beta updates and download verified installers while your
  shells keep running. Native packages include checksums, signed update metadata
  and GitHub build attestations.
- Include fixes for macOS Dock icon sizing, recovery after minimizing and zsh
  history persistence, plus Windows 11 window corners and terminal directories
  when splitting or restoring workspaces.

### Known limitations

- This is a public release candidate. Full native acceptance on every platform
  remains pending; it is not a production-stable release.
- Windows installers are unsigned. Windows may show an unverified-publisher or
  SmartScreen warning.
- Linux x64 packages target glibc 2.35 or newer and need working host graphics
  drivers. Browser-downloaded AppImages need execute permission before launch;
  enable it in file Properties or run `chmod u+x` on the downloaded file.
- Workspace restoration starts fresh shells and SSH connections. Arbitrary
  running commands and process memory are not restored. Coding-agent resumption
  is limited to supported provider sessions on local Unix terminals.
- SSH requires an installed OpenSSH client and a POSIX remote shell. Remote
  directory tracking is integrated for zsh; other shells need OSC 7 integration.
- Kitty graphics and comprehensive complex-script shaping are not supported.
  Keypad identity and some keyboard-layout information depend on the window
  toolkit. IME, accessibility and mixed-DPI behavior still need native acceptance.
- Release candidates are offered through the Beta channel; Stable waits for an
  accepted stable release. If you installed a withdrawn higher-version private
  draft, install this RC manually: automatic updates never downgrade.
