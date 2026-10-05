# User guide

A native Rust terminal for focused work. GPU rendering, real shell sessions, and a quiet workspace interface inspired by cmux and Ghostty. The interface follows a native, Apple-style visual language: a full-height sidebar, a unified toolbar, terminals as rounded content surfaces, and one accent colour for focus. See [the interface direction](design.md).

[Project overview](../README.md) · [Installation](installation.md)

- [Workspaces and terminals](#workspaces-and-terminals)
- [Workspace groups](#workspace-groups)
- [SSH workspaces](#ssh-workspaces)
- [Closing terminals safely](#closing-terminals-safely)
- [Coding agent sessions](#coding-agent-sessions)
- [Notifications](#notifications)
- [Command-line options](#command-line-options)
- [Preferences and themes](#preferences-and-themes)
- [Storage and restoration](#storage-and-restoration)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [Updates](#updates)

## Workspaces and terminals

Workspace navigation, independent shell panes, split layouts, scrollback, terminal search, selection/clipboard, a command palette, and settings are integrated in the native interface. Every action is listed in the command palette with its shortcut; secondary-click a terminal or a workspace for its menu, double-click a workspace to rename it, drag a workspace to reorder the sidebar, and drag the sidebar's edge to resize it. Terminals in a workspace are tabs: several can share one place with one in view, and a split puts places side by side. With several terminals in a workspace, drag one by its tab to rearrange them: drop it on an edge of a place to sit beside it (on an edge of its own place to split it away from its neighbours), on the centre or among the tabs of another place to join them, or on a workspace in the sidebar to move it there with its shell still running. Escape cancels the drag, and the command palette lists "Move terminal to …" for each other workspace. Terminal text uses bundled JetBrains Mono; interface text uses Geist. Font licenses accompany the assets.

Hold Command on macOS or Ctrl on Linux/Windows to reveal small shortcut hints in the top-right of the first nine workspace rows. The hints follow the workspace order and disappear when you release the modifier; `⇧` means Shift. Selecting a workspace with its shortcut or the palette opens its group if it is collapsed.

New workspace opens and selects a fresh shell at `~` immediately. Rename it later by double-clicking its sidebar row or choosing Rename workspace from its menu or the command palette.

The directory beneath a local workspace's name follows its focused terminal. Switching between tabs or split panes updates that directory; inactive workspaces show their last focused terminal's directory. Long paths are shortened to fit the sidebar. SSH workspaces show their host there.

## Workspace groups

Secondary-click the **Workspaces** heading or empty sidebar space for **New workspace**, **New SSH workspace**, and **New workspace group**. A group is a folder containing local or SSH workspaces. Click its name to expand or collapse it; hover the row and click **+** to open a new shell inside it. Secondary-click the folder to create an SSH workspace, rename it, or remove the group. Double-click also renames.

Use **Move to group** in a workspace's menu to organize existing workspaces, or choose **Ungrouped** to move one out. Workspaces can be reordered within their group by dragging or with **Move up** and **Move down**. Drag a group by its folder row to place it at the top, between ungrouped workspaces, or beside another group; its workspaces move with it. Ungrouped workspace rows can also be dragged around groups. Escape cancels the drag, and the new order is saved on release. Group commands are also available in the palette. Collapsing or removing a group keeps its shells running; removing it leaves its workspaces ungrouped in the folder's former position. Names, membership, empty groups, mixed sidebar order, and collapsed state are saved with the workspace organization. Groups do not nest.

Choose **Default directory…** from a group's context menu to set the starting
folder for new local workspaces created in that group. Enter an absolute local
path or `~/folder`, or use **Browse…** to choose a folder in the native file
picker, then **Save**. Canceling the picker keeps your current path. The palette
offers **Set default directory for group …** when the sidebar is hidden. **Use home directory** removes the default.

Existing workspaces and terminals keep their directories. New tabs and splits
always follow the current directory of the terminal they open beside; restarts
and restoration use each terminal's last directory. Moving a workspace between
groups does not change its sessions. Group defaults are saved across launches
and apply to local workspaces; SSH workspaces retain their remote directory
behavior. If a saved group folder becomes unavailable, edit or remove the default
before creating another local workspace there.

## SSH workspaces

A workspace can be connected to another machine over SSH. Every terminal in it, including new splits and restarted terminals, then opens on that host instead of in a local shell. Secondary-click a workspace and choose **Connect over SSH…** to move all of its terminals to a host, or run **New SSH workspace** from the command palette. **Disconnect from SSH** returns the workspace to local shells. Connecting or disconnecting replaces the workspace's terminals, so processes running in them stop. A terminal keeps its session when it is moved, so it can be moved only between workspaces on the same machine.

The host is an OpenSSH destination: `host`, `user@host`, an alias from `~/.ssh/config`, or `ssh://user@host:port`. Neptune runs the system `ssh` client (which must be on your `PATH`) once per terminal, so your SSH configuration, keys and agent apply, and password, passphrase and host-key prompts appear in the terminal. Neptune stores the destination and each terminal's last reported remote directory, never a credential. Each terminal is its own connection; enable `ControlMaster` in your SSH configuration to share one. The remote host needs a POSIX `sh` to launch its login shell; the `shell` setting applies to local terminals only. A terminal whose connection ends offers **Reconnect**.

The first terminal starts in the host's login directory; splitting inherits the source terminal's current remote directory. Reconnecting or reopening the app with workspace restoration enabled opens a fresh SSH shell in each terminal's last reported remote directory. Neptune adds temporary OSC 7 directory reporting to Zsh after loading your normal login configuration, without editing your dotfiles. Other shells need their own OSC 7 integration; until a shell reports a directory, splits and reconnects start in the login directory. If a remembered remote directory no longer exists, the terminal shows the failure instead of silently opening elsewhere. Disconnecting or changing hosts clears the remembered remote directories.

## Closing terminals safely

Preferences has two independent switches, both enabled by default:
**Confirm before closing terminals** always asks before closing; **Warn about
running processes** asks when a terminal has an active job, even if the first
switch is off. They apply to terminals, whole workspaces, SSH disconnects and
quitting Neptune, including terminals in hidden workspaces. One confirmation
covers the whole action; Cancel preserves the sessions. Process checks do not
show a dialog or dim the window until confirmation is needed; Escape can cancel
a pending check.

The process check runs on session workers only when closing is requested. It
uses OS metadata rather than terminal titles or output, detects foreground,
background and stopped child jobs, and recognizes programs that replace the
shell with `exec`. An idle recognized shell can close immediately with the first
switch off. Starting sessions, failed checks and checks taking longer than two
seconds ask before closing. SSH connections always count as active: Neptune
cannot inspect jobs on the remote host. Shell helper processes and unrecognized
shell executables may also trigger a warning. Shell builtins with no child
process and fully detached/reparented jobs cannot reliably be distinguished
from an idle shell; use the always-confirm switch if you need that protection.
Closing hangs up the terminal; detached or signal-ignoring jobs may survive.

## Coding agent sessions

Local Unix terminals can reopen Claude Code, Codex, OpenCode, pi and Oh My Pi
in their original panes when Neptune restarts, using each CLI's own saved session ID;
Gemini CLI is reopened without its conversation. Start `claude`, `codex`,
`opencode`, `pi`, `omp` or `gemini` normally. Codex asks you to review Neptune's hooks before it
can report IDs and what it is doing. Exiting the CLI or restarting its terminal clears the resume
reference; closing Neptune retains it. See [agent sessions](agent-sessions.md)
for setup, exact-session requirements and platform limitations.

The two can work together. Ask one for the other ("build the backend and let
Claude Code handle the frontend", or "start Codex on gpt-5.1-codex at high
effort for this", or "start Claude Code with ultracode")
and it starts the other agent, hands it the task, reads its answer and can
keep talking to it. The started agent runs out of view, without a tab. The tab
of the agent that started others shows how many it started; click the count
for the list and an agent in it to open its terminal, which is a tab from then
on. If a started agent's CLI asks something before it begins, such as whether
to trust a folder, the agent that started it tells you what it asks, and the
count changes colour: answer in its terminal, or tell the first agent your answer.
An agent you closed can be opened again by the agent that started it, with its
conversation. See
[agents that start agents](agent-sessions.md#agents-that-start-agents) for
models, effort levels, worktrees, permissions and limits.

## Notifications

Processes can request attention through BEL, OSC 9, OSC 99 and OSC 777. BEL produces a generic **Terminal bell** alert. Pane rings, sidebar unread badges and the toolbar notification popover keep track of alerts, with optional native desktop banners. See [notifications and agent setup](notifications.md) for Claude Code, Codex, OpenCode, pi and shell examples.

## Command-line options

The examples below assume `neptune` is on your `PATH`. Run the configuration
example from the repository root.

When no saved workspace can be restored, Neptune opens a terminal in your home directory (`~`). Use `--cwd` to choose another starting directory, or `--ssh` to open a workspace on a host. `--command` cannot be combined with `--ssh`, and a startup command is never typed into a terminal that is connecting over SSH.

```sh
neptune --cwd /path/to/project
neptune --ssh user@host
neptune --config config.example.toml
neptune --data-root /path/to/isolated-neptune-data
neptune --command 'printf "hello\n"'
neptune --no-restore
neptune --help
```

## Preferences and themes

[config.example.toml](../config.example.toml) documents the supported settings: 715 themes shared by the window and terminal, custom themes, window zoom, terminal font family, font size and line height, scrollback limit, shell executable and arguments, cursor style/blink, sidebar width, workspace restoration, and the two independent close warnings. Window zoom is available in Preferences under Appearance; changes there or through zoom shortcuts are saved and restored on the next launch, including with workspace restoration disabled. Settings are validated; unknown keys are rejected. Workspace restoration restores directories, split positions, and focused panes, and launches fresh shell processes; a remote workspace opens new SSH connections to its host. Recognized coding agents can resume their provider-owned conversations through saved session references; arbitrary commands and process memory are never serialized.

Preferences → Shell lists the shells found on this computer, as Windows Terminal lists profiles. On Windows it offers Command Prompt, Windows PowerShell, PowerShell 7, WSL distributions, Git Bash, Visual Studio developer prompts and Windows Terminal profiles that have their own command line (such as the Anaconda prompts). On macOS and Linux it offers the shells in `/etc/shells` and common shells on `PATH` or in Homebrew. **System default** keeps the platform's own startup, including a login shell on macOS and Linux; **Custom…** takes any program. A new choice applies to new terminals.

### Themes

Open **Preferences** and choose the theme row under Appearance (or **Browse
themes** in the command palette) to pick one theme for the whole app. Neptune's
Graphite, Dusk and Light themes sit alongside all 712 palettes from the
[iTerm2 collection](https://iterm2colorschemes.com/). Search by name or filter
Dark, Light, Custom and Favorites; each card previews the window and terminal,
and the catalog opens at the theme in use. Choosing a card updates existing and
new terminals, the sidebar, toolbar and dialogs at once.
Star a theme (the star beside its name, or **Add to favorites** in its menu) to
keep it: favorites lead the catalog in the order you starred them, and the
**Favorites** filter shows them alone. Starring does not change the theme in
use. Favorites are saved as `favorite_themes` in `config.toml`.
The collection works offline; [its pinned source and author
credits](../assets/themes/README.md) ship with Neptune. Window surfaces and readable
interface colors are derived from each imported palette.

**New theme** copies the colors in use into an editor, and **Duplicate…** in a
card's menu (the "more" button on a card, or a secondary click) starts from any
other theme. Name it and use the color wells or hex fields for the background,
text, bold text, cursor, selection and 16 ANSI colors; the preview updates as
you edit. **Save theme** saves and applies it. Leaving the editor with unsaved
changes asks before discarding them. A custom theme's menu also offers
**Edit…** and **Delete…**; deletion asks for confirmation, and deleting the
theme in use returns to Graphite. Up to 128 custom themes are saved in
`config.toml`, with names up to 64 characters. Reset to defaults retains your
saved custom themes and favorites; deleting a custom theme removes its star.
Terminal programs can still override terminal colors through escape sequences.
Older settings with a separate `terminal_theme` migrate that selection to the
single `theme` setting on load; the next settings save writes the unified format.

Choose a terminal font in **Preferences → Text → Font family**. Neptune lists installed monospace families and terminal Nerd Fonts with fixed text spacing. **Custom…** reveals a text field: type an installed family name or PostScript name and press Enter to apply it. Names match regardless of ASCII case, and typed names can resolve fonts outside the menu's list. If an exact name is absent, `NF` and `NFM` expand to `Nerd Font` and `Nerd Font Mono`; for example, `MesloLGS NF` can use an installed `MesloLGS Nerd Font`. Preferences identifies the installed family when a shorthand or PostScript name is used. Nerd Fonts with wider icons can have their fixed-pitch flag unset; Neptune accepts them only when their printable ASCII text has uniform advances. An empty or invalid name keeps the current font; closing and reopening Preferences discards unsubmitted text. Applied choices affect every terminal, including existing sessions, without restarting shells. JetBrains Mono is bundled and remains the default. Font choices are saved as `font_family` in the configuration. If a font is unavailable or unreadable, Neptune uses JetBrains Mono and names the fallback in Preferences while keeping the saved choice. Restart Neptune after installing new fonts. Interface text keeps its own font.

## Storage and restoration

Default storage is `~/.config/neptune` on Linux (or `$XDG_CONFIG_HOME/neptune`),
`~/Library/Application Support/rs.Neptune.neptune` on macOS, and
`%APPDATA%\Neptune\neptune\config` on Windows. On the first normal launch,
Neptune moves the previous product's entire settings directory to this location
if Neptune storage does not already exist. Saved files and recovery copies retain
their original bytes; damaged or unsupported state still receives the usual write
protection. A migration failure stops startup and preserves the original files.
Existing Neptune storage takes precedence. Explicit `--data-root` and screenshot
launches bypass migration.

Neptune also remembers the window's size and maximized state when closed. Window state is saved as `window.json` in the data directory, independently of workspace restoration; `--no-restore` and `restore_workspaces = false` only affect workspaces. Use `--size WIDTHxHEIGHT` to override the saved geometry and start with a non-maximized window. Screenshot launches use the default or explicit size and do not save window state.

## Right panel

The **Toggle right panel** button at the trailing end of the toolbar, Ctrl+Shift+O (Command+O on macOS) or the command palette slides a panel in from the right. Its two tabs, **Files** and **Agents**, are chosen by clicking them or with **Show files** and **Show agents** in the command palette, which also open the panel. The panel opens on the tab it last showed. Drag its leading edge to resize it, or double-click the edge for the default width. It starts closed with each launch; its width, its tab and what is typed in it are not saved.

## Agents

The **Agents** tab lists every Claude Code, Codex, OpenCode, Gemini CLI, pi and Oh My Pi agent running in a terminal of any workspace, local or SSH, including workspaces and tabs out of view. Agents are grouped by what they are doing: **Needs input** first (an agent asking to allow a tool, asking a question, waiting for a plan to be approved, or otherwise blocked on you), then **Working**, then **Idle**. Each row shows the agent's name for its conversation, its state, which agent it is, its workspace and how long it has been in that state. Click a row to go to its terminal. While an agent waits for input and the list is not in view, the panel's toolbar button carries a dot, and the Agents tab shows how many are waiting.

An agent is listed while its CLI runs: it appears when its CLI starts and leaves when the CLI exits or its terminal is restarted or closed. In an SSH workspace Neptune installs small adapters in your cache directory on the host to report this through the terminal; see [agents on SSH hosts](agent-sessions.md#agents-on-ssh-hosts). Agents started inside `tmux` on a host, in an `ssh` you typed yourself, on Windows, as batch commands (`claude -p`, `codex exec`) or through an alias or absolute path that bypasses Neptune's adapters are not listed. Codex asks once to trust the hooks that report its activity; until it does, its state is read from its terminal title alone, about two seconds behind. Gemini CLI is always read from its title. See [agent sessions](agent-sessions.md#agent-activity) for how states are detected and where they can lag.

## File explorer

The **Files** tab of the right panel shows the folder of the focused terminal and follows that terminal when it changes directory or when another terminal is focused. Every item is listed, hidden files and folders included, with folders first. Files made, renamed or removed by a program in the terminal appear within about two seconds; **Refresh** reads the folder at once.

Click a folder to open or close it and a file to preview it at the bottom of the panel. Text is shown with line numbers (the first 256 KiB and 5,000 lines), PNG, JPEG, GIF, WebP and BMP pictures are fitted to the preview with their pixel size, and other files say that they have no preview. The preview follows the file as it changes. Drag the divider above the preview to resize it, and use its buttons to open the file with its default application or to close the preview.

**New file** and **New folder** in the panel's header create an item beside the selection, or in the folder shown when nothing is selected; type the name where the item will appear and press Enter. A name with slashes, such as `notes/today.md`, makes the folders on the way. Secondary-click an item for its menu: **New file…** and **New folder…** inside a folder, **Open with default app** for a file, **Reveal in file manager** (Finder on macOS, File Explorer on Windows), **Copy path**, **Copy relative path** (relative to the folder shown), **Rename…** and **Delete…**. Secondary-click empty space for the same creation commands, **Collapse all folders** and **Refresh**. Escape abandons a name being typed; clicking elsewhere uses it. Nothing is ever replaced: pressing Enter on a name that is taken keeps the field open and says so. **Delete…** asks first, then removes the file, or the folder with everything in it, permanently; it does not use the trash. Deleting a link removes the link, not what it points to.

Type in **Search files** to find files and folders under the folder shown. Every word must be in the path and at least one in the name, without regard to case; a word with a slash, such as `app/expl`, is matched against the path alone; `*` and `?` make the text a pattern, such as `*.rs` or `src/**/mod.rs`. The **files to exclude** field appears under the search (the **…** control keeps it in view) and starts as `**/node_modules/**, **/.git/**, **/dist/**, **/target/**, **/build/**`. It takes comma-separated patterns with `*`, `?`, `**` for any folders and `{a,b}` alternatives; a pattern without a slash, such as `*.log`, applies in every folder. Exclusions apply to the search only: the tree always shows everything. Click a file among the results to preview it, or a folder to show it in the tree. A search stops at 2,000 results, and links to folders are listed but not followed. Escape clears the search, then leaves the field.

A workspace connected over SSH has its files on the other machine, so the panel shows no folder for it.

## Keyboard shortcuts

Ctrl-click a web link in a terminal to open it in your default browser; on macOS, use Command-click. Hold the modifier over a link to see its underline and hand cursor. This works with printed HTTP/HTTPS URLs and OSC 8 hyperlinks, including soft-wrapped links and visible scrollback. Ordinary clicks and drags still select text; Shift keeps selecting when a TUI owns the mouse.

Rest the pointer on the path of a picture in a terminal to preview it: when a CLI agent or a command reports a screenshot or chart it wrote, the picture appears beside the path with its file name and pixel size, and goes away when the pointer leaves. Move the pointer onto the preview and click it to see the picture as large as the window allows. There, scroll or pinch to zoom in and out around the pointer, or press + and -; drag a zoomed picture to move it, and press 0 to fit it to the window again. Click or press Escape to return to the terminal. No key is needed, and it works while a TUI owns the mouse. PNG, JPEG, GIF (first frame), WebP and BMP files are shown, recognized by their contents. Absolute paths, `~/` paths, `file://` hyperlinks and paths relative to the directory the agent or shell is in are found, including names with spaces and soft-wrapped paths in visible scrollback. A path that a program breaks across lines itself, a file larger than 64 MiB and a path in a terminal connected over SSH, which names a file on the other machine, show no preview.

Use Ctrl+Shift on Linux/Windows and Command on macOS: T opens a tab beside the focused terminal, N opens a workspace, PageDown/PageUp step through the tabs of the focused place, D splits right, E splits below, W closes the focused pane, F searches, P opens commands, B toggles the sidebar, O toggles the right panel, Enter zooms the focused pane to full size and back, and 1–9 select a workspace by its sidebar position. Ctrl+Tab switches workspaces. Ctrl+Shift+Left/Right/Up/Down focuses the adjacent pane on every platform, including while zoomed; at an outer edge, focus stays put. These moves are also available in the command palette. Escape cancels a terminal drag, or leaves a sheet, a focused search field or a name being typed in the file explorer; otherwise it goes to the shell, as do Tab and unmodified arrow keys. Ctrl+comma opens preferences. Ctrl+plus/minus (Command on macOS) zooms the whole app; Ctrl+equals also zooms in, and Ctrl+0 resets app zoom (Command on macOS). On keyboards where Plus requires Shift, use Ctrl+equals (Command on macOS) for app zoom. Change terminal font size in Preferences or with Ctrl+Shift+plus/minus on Linux/Windows and Command+Shift+plus/minus on macOS; Ctrl+Shift+0 (Command+Shift+0 on macOS) resets it to the default (14 pt). On macOS these font shortcuts follow the active keyboard layout's labeled +, -, and 0 keys, even when Shift produces *, _, or =, as on Latin American keyboards. Use Ctrl+Shift+C/V to copy/paste on Linux/Windows, Command+C/V on macOS. Plain Ctrl+C interrupts the shell; Shift+PageUp/PageDown scrolls history. Hold Shift to select text when a TUI owns the mouse.

Paste also takes a picture: when the clipboard holds a screenshot or another image instead of text, Neptune saves it as a PNG and pastes the file's path. Use the paste shortcut on Linux and Windows, or **Paste** in the terminal's menu or the command palette on any platform. A shell gets the path to use in a command; Claude Code and Codex attach the image a pasted path names. Plain Ctrl+V still goes to the program in the terminal, so an agent that reads the clipboard itself keeps doing so. Neptune keeps the 32 most recent pasted pictures in `pasted-images` beside its settings, readable only by your account, and removes older ones.

Drag files from a file manager or another application onto a terminal to paste their paths, quoted for the shell and separated by spaces. While the files are held over the window, the terminal that will take them is highlighted and says what the drop does; with several terminals in view, that is the one under the pointer on Linux, and the focused one on macOS and Windows or while the files are over the sidebar or toolbar. Dropping on a terminal focuses it. A terminal whose shell has exited, and the window while a sheet or the command palette is open, take no files. Over SSH the pasted path still names a file on this computer.

## Updates

Preferences → Updates controls automatic checks and Stable/Beta channels.
Checks/downloads run off the UI thread; updates require signature/hash verification
and explicit download/install actions. Running shells are never silently closed
or replaced. Beta can advance to a newer stable; neither channel downgrades.

See [installation](installation.md) for downloads and the
[release guide](releases.md#desktop-update-behavior) for update verification
and channel behavior.
