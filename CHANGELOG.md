# Changelog

User-facing release history. Add meaningful changes under **Unreleased** as work
lands. `scripts/release.py prepare` moves them into a dated version section.
The release workflow copies that section verbatim into GitHub's What's New and
the signed desktop update manifest; do not maintain a second release-notes file.
Earlier private drafts were withdrawn before Neptune's first public preview;
their preparation records remain in Git history.

## [Unreleased]

### What's New

- Browser tabs stay signed in. They now open in a profile that keeps cookies
  and site data on this computer; add more profiles from a tab's menu, or
  choose **Private** for a tab that keeps nothing, as every tab did before.
  Set `browser_profile = "private"` to keep that for all of them.
- Cookies can be imported into a browser tab's profile from Chrome, Chromium,
  Edge, Brave, Vivaldi, Opera, Arc and Firefox. Chromium-family browsers on
  Windows and Safari are not supported.
- **Pick an element** in a browser tab copies a description of the element you
  click, with a picture of it, to paste to an agent or an issue.
- **Record the page** in a browser tab saves a WebM video of the page to your
  Videos folder.
- Pages that set no background in a browser tab are now white, as in other
  browsers.
- Browser tabs are sharp on displays scaled above 100%, where pages were
  painted at low resolution and stretched.
- Scrolling a browser tab follows a touchpad directly and moves a mouse wheel's
  usual distance; slow touchpad strokes were being dropped.
- A browser tab shows the page's own icon, and turns while the page loads.
- The pointer changes over links, text and other controls in a browser tab,
  tooltips appear, and a hovered link shows where it leads.
- Restored browser tabs open the page they were on instead of a blank one.
  Private tabs still come back blank.
- Browser tabs now open in the Linux AppImage on Ubuntu 24.04 and later, where
  they reported a stopped browser host. These systems block Chromium's sandbox
  for an AppImage, so previews there run without it and a blank tab says so;
  other systems and the DEB keep the sandbox. The installation guide says how
  to allow it for the AppImage.
- Browser tabs in the Linux AppImage no longer need Chromium's libraries
  installed on the system; the AppImage's own copies serve when they are
  missing.
- The Linux AppImage now starts on systems with current Mesa graphics drivers,
  such as Ubuntu 26.04, Fedora 44 and Arch, where it reported that it could
  not start its renderer. It uses the system's Wayland libraries, which those
  drivers require.

## [0.1.0-rc.5] - 2026-10-10

### What's New

- Preview websites beside your terminal with browser tabs that move and split
  within the same workspace. Open a local dev-server port, navigate and search
  pages, or inspect them with developer tools. Browser painting follows the
  display's refresh rate and pauses for hidden previews; tabs restore blank,
  without saving browsing history or addresses.
- See subscription usage for your local Codex, Claude Code and Cursor accounts
  in the footer. Click it for allowances, reset times, plan names and refresh
  controls; unavailable or stale readings are explained. Checks run in the
  background without starting an agent turn. Cursor Keychain access on macOS
  is optional for each run.
- Select and copy text in the file explorer's previews. Markdown files now
  switch between source and formatted preview, with pictures, working web,
  file and heading links, and Copy and Wrap controls on code blocks.
- Read a pull request without leaving the terminal. Click a pull request's
  number on a terminal's tab and it opens in the new **Pull request** tab of
  the right panel: its title and state, what stands between it and a merge,
  reviewers, labels, description with its pictures and checks under
  **Summary**, everything
  that happened to it under **Timeline**, and its changed files with their
  diffs under **Code**. From there you can merge it or have the host merge it
  by itself, mark it ready or a draft, close, reopen or revert it, edit its
  title and description, change labels and reviewers, comment, react, answer
  and resolve review conversations, mark files as viewed, review it with
  comments on single lines, approve waiting workflow runs, walk a stack,
  check it out in a worktree of its own and hand it to an agent. Each pull request you open is a tab of its own in the panel's strip. Ctrl-click (Command-click on
  macOS) a number to open it in the browser as before. It is read with your
  signed-in GitHub CLI (`gh`), only while its tab is in view, and nothing
  read is saved.
- Put an agent in charge of a piece of work with Projects. The new Project
  tab of the right panel starts a **lead**, your installed Claude Code or
  Codex, in the workspace in view: tell it what you want done and it plans,
  starts agents in terminals of their own, hears when they finish and reports
  back in a chat. Its agents run out of view and are listed in the tab; click
  one to open its terminal. **Needs you** pins whatever only you can clear,
  such as an agent waiting for a permission. The lead has no file or shell
  tools of its own and cannot answer permission requests. Attach files to a
  message with the paperclip, by dropping them on the chat or by pasting a
  picture: the lead sees pictures and hands other files to its agents by
  path. **Context** keeps
  your instructions and the lead's decisions for every agent it starts, and
  **Watches** wake the lead on a schedule or when a pull request's checks or
  review comments change, only while Neptune is open. Unlike other agent
  features, a project's chat and context are saved on this computer, in a
  folder of its own that **Remove project…** deletes. Needs Claude Code or
  Codex installed and signed in, and a workspace on this computer; not
  available on Windows yet. Saved workspaces move to a newer format, which
  earlier versions of Neptune leave untouched instead of restoring.
- Send an agent's tab back to the background without stopping the agent. Once
  you open the terminal of an agent that a project's lead or another agent
  started, **Send to background** in the tab's menu, in the agent's row menu
  and in the command palette puts the tab away again: the agent runs on out
  of view and stays listed where you opened it from. Closing the tab still
  ends the agent.
- See the new value when you change the terminal font size or window zoom from
  the keyboard or the command palette: a chip under the toolbar shows it for a
  moment, such as "Font size 15 pt" or "Window zoom 110%".
- See which agent a terminal runs from its tab: Claude Code, Codex, OpenCode
  and pi show their own mark before the tab's name.
- Look through everything an agent attached in one go. **View all pictures** in
  a terminal's list of attached files opens them at full size, newest first,
  and a band of small pictures at the foot of the window shows each one:
  click any of them, or keep using the arrow keys.

### Acceptance notes and known limitations

- Full native acceptance on every platform remains pending; this is a release
  candidate for acceptance testing, not a production-stable release.
- Saved workspaces now use schema version 14 for Projects and browser panes.
  Earlier workspace files still load, but RC4 and earlier cannot restore or
  save over layouts written by this version. Back up workspace state before
  testing if you need to return to an earlier candidate. Restoration starts
  fresh shells and SSH connections, and blank browser tabs; running commands,
  process memory, browser addresses and browsing history are not restored.
- Browser previews include a separate sandboxed Chromium runtime, increasing
  installer size. Linux AppImages require working user namespaces for browser
  previews; the DEB includes the sandbox helper for restricted hosts. A browser
  startup failure leaves terminals usable. Browser sandbox, clipboard, IME,
  DPI and installer behavior still need native acceptance on macOS and Windows.
- Projects require Claude Code or Codex installed and signed in, and a local
  workspace; they are not available on Windows yet. Pull request actions require
  the GitHub CLI (`gh`) signed in with the necessary repository permissions.
- Windows installers remain unsigned and may show an unverified-publisher or
  SmartScreen warning. Both macOS installers require signing and notarization;
  all installers are covered by signed update metadata and build attestations.
- Linux x64 packages require glibc 2.35 or newer and working host graphics
  drivers. Browser-downloaded AppImages need execute permission before launch;
  enable it in file Properties or run `chmod u+x` on the downloaded file.
- In-place updates remain unverified on macOS. The reported intermittent native
  Wayland freeze still needs a capture of the blocking state; XWayland checks
  do not establish native Wayland acceptance.
- SSH requires an installed OpenSSH client and a POSIX remote shell. Remote
  directory tracking is integrated for zsh; other shells need OSC 7 integration.
- Kitty graphics and comprehensive complex-script shaping remain unsupported.
  Keypad identity and some keyboard-layout information depend on the window
  toolkit. IME, accessibility and mixed-DPI behavior still need native acceptance.
- Automatic updates never downgrade. After this candidate is published, select
  Beta in Preferences → Updates to receive it; private drafts are not offered.

## [0.1.0-rc.4] - 2026-10-05

### What's New

- Customize shortcuts in the config file. Preferences → General → Keyboard
  opens the active config and includes an expandable reference with action
  names, default shortcuts and examples. Menus, buttons and the command palette
  show your configured shortcuts. Restart Neptune after editing the file.
- Open a dev server from its terminal's port chip. Local listeners appear on
  the tab or toolbar; clicking one in an SSH workspace forwards the remote
  port to your computer. A menu lets you stop forwarding or retry a failure.
- Ctrl-click a filename or diagnostic location in a terminal (Command-click
  on macOS) to open it in Neptune's file explorer or your preferred editor.
  Choose the editor in Preferences → General. Use Ctrl+Shift+H
  (Command+Shift+H on macOS) to label visible paths, hashes and URLs, then type
  a label to copy its target without selecting it.
- Keep a row or column of terminals evenly sized as you split, close or move
  them. Double-click a divider to even its row or column, or use **Even terminal
  sizes** in the command palette for the whole workspace. Each workspace now
  holds up to 16 terminals, including tabs and agents without a tab.
- Review what changed without leaving the terminal. The new Changes tab of
  the right panel shows the branch of the focused terminal's folder and the
  files that differ, with a diff of the one you click. Choose **Working tree**
  for work that is not committed yet, or **Branch** for everything the branch
  holds since it left the main one. Each workspace's row in the sidebar now
  shows its branch, with a dot while it has uncommitted changes. Needs Git
  installed.
- See every Claude Code and Codex agent running in any workspace in the new
  Agents tab of the right panel: which are working, which are idle and which
  are waiting for you to allow a tool, answer a question or approve a plan.
  Click an agent to go to its terminal. The toolbar's panel button, which now
  opens the file explorer and the agents as tabs, carries a dot while an agent
  waits out of view. Codex asks once to trust the hooks that report this.
- The Agents tab also lists OpenCode, Gemini CLI, pi and Oh My Pi, and agents
  you start in SSH workspaces. OpenCode, pi and Oh My Pi sessions reopen with
  their conversations like Claude Code's and Codex's. On an SSH host Neptune
  keeps a few small
  adapter scripts in your cache directory and reports through the terminal
  itself, so the host needs nothing installed and no extra connection.
  OpenCode, Gemini CLI and pi do not ask for attention on their own, so
  Neptune alerts you when one of them finishes or starts waiting in a
  terminal you are not looking at.
- Let Claude Code and Codex work together. Ask one for the other, such as
  "build the backend and let Claude Code handle the frontend", and it starts
  that agent out of view, hands it the task, reads its answer and can keep
  talking to it. The tab of an agent that started others shows how many;
  click the count to list them and open one's terminal in a tab. A started
  agent can be given its own directory, such as a git worktree, a model, an
  effort level and its CLI's ultra mode (ultracode or Ultra).
  One you closed can be opened again with its conversation, and a question a
  started agent's CLI asks before it begins is passed on to you at once.
- Update without reinstalling on macOS and with the Linux AppImage. Once an
  update is downloaded and verified, **Restart to update** replaces the app you
  are running and restarts it, with your workspaces restored. There is no disk
  image to drag to Applications and no AppImage to replace by hand. Updating
  *to* this version from an earlier one is still done by hand, one last time.
- See where an agent's pull request stands on its tab. The linked number now
  shows whether its checks pass, fail or are still running and how many review
  comments are unresolved, and changes colour and icon once the pull request is
  merged. It updates on its own, through the GitHub CLI (`gh`) you are signed
  in to; without it the number looks as before.
- Give an agent a git worktree of its own with **New agent in worktree**, in
  the command palette, a terminal's menu or on Ctrl+Shift+G (Command+G on
  macOS). Name a branch and press Enter: Neptune creates the branch, checks it
  out in a folder beside the repository and opens a tab there with Claude Code
  or Codex started. The tab is named by the branch and shows **Merged** once
  its work is in the default branch; click it to remove the folder and the
  local branch. **Remove worktree** says what would be lost before it removes
  anything, and keeps a branch that has unmerged commits.
- Ask Claude Code or Codex to attach its screenshots, or any file it wants
  you to look at, and they appear on its tab. Click the paperclip for the
  list: each file with its picture, where it has one. Click a picture to see
  it at full size and step through the others with the arrow keys; click any
  other file to open it. Each row also shows the file in your file manager or
  dismisses it from the list, leaving the file where it is.

### Acceptance notes and known limitations

- Full native acceptance on every platform remains pending; this is a release
  candidate, not a production-stable release.
- In-place updates are verified on Linux only by replacing and restarting a
  test AppImage; the macOS bundle replacement has not yet run on a Mac.
- Saved workspaces now use schema version 11, which records which terminal's
  agent started another's, which of those terminals have no tab, the git
  worktree made for a terminal's agent, attached files, and sessions of OpenCode,
  Gemini CLI, pi and Oh My Pi. Older workspace files still load, but RC3 and
  earlier cannot save over layouts written by this version. Back up workspace
  state before testing if you need to return to an earlier candidate. Older
  builds also reject workspaces containing more than 12 terminals.
- Agents in worktrees are for local terminals with `git` on `PATH`. Neptune
  does not fetch, so a pull request merged on a server shows as merged after
  your checkout fetches or pulls it. On Windows the tab opens in the worktree
  without starting a CLI.
- Port discovery requires the host's process/socket tools. Windows SSH port
  discovery and forwarding need noninteractive key or SSH-agent authentication;
  remote discovery requires a Linux or Unix host. Port chips identify TCP
  listeners, which may not be HTTP services.
- The reported intermittent native Wayland freeze still needs a capture of the
  blocking state. Earlier XWayland fixes do not establish native Wayland acceptance.
- Windows installers remain unsigned and may show an unverified-publisher or
  SmartScreen warning. Both macOS installers require signing and notarization;
  all installers are covered by signed update metadata and build attestations.
- Linux x64 packages require glibc 2.35 or newer and working host graphics
  drivers. Browser-downloaded AppImages need execute permission before launch;
  enable it in file Properties or run `chmod u+x` on the downloaded file.
- Workspace restoration starts fresh shells and SSH connections; arbitrary
  running commands and process memory are not restored. Coding-agent features
  remain limited to the supported providers and platform paths.
- SSH requires an installed OpenSSH client and a POSIX remote shell. Remote
  directory tracking is integrated for zsh; other shells need OSC 7 integration.
- Kitty graphics and comprehensive complex-script shaping remain unsupported.
  Keypad identity and some keyboard-layout information depend on the window
  toolkit. IME, accessibility and mixed-DPI behavior still need native acceptance.
- This release candidate is available through the Beta channel. Automatic
  updates never downgrade; select Beta in Preferences → Updates to receive previews.

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

- Full native acceptance on every platform remains pending; this is a release
  candidate, not a production-stable release.
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
- This release candidate is available through the Beta channel. Automatic
  updates never downgrade; select Beta in Preferences → Updates to receive previews.

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

- Full native acceptance on every platform remains pending; this is a release
  candidate, not a production-stable release.
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
- This release candidate is available through the Beta channel. Automatic
  updates never downgrade; select Beta in Preferences → Updates to receive previews.

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
