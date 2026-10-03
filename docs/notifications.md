# Terminal notifications

Programs can ask for attention by writing BEL, OSC 9, OSC 99 or OSC 777 to their
Neptune terminal. Neptune rings the pane in amber, shows the latest alert and an
unread count on its workspace row (a collapsed folder sums its workspaces), and
puts a dot on the toolbar bell. The bell and the **Notifications** palette
command open the notification popover, which lists alerts newest first with
their workspace, program and age. Selecting a notification opens its workspace
and pane, including panes hidden by zoom or a collapsed folder. The Up and Down
arrow keys move through the list, Enter opens the highlighted alert and Delete
dismisses it. Focusing a pane or typing in it acknowledges its alerts. Opening
the popover alone does not mark them read. It also offers **Mark all read**,
individual dismissal and **Clear all**.

History is memory-only, capped at 128 entries across the application. Closing or
restarting a pane discards its old session's alerts. Moving a pane carries its
alerts with it. Exited processes retain their alerts until acknowledged or
closed. Notification text is not written to workspace files or diagnostics.

Desktop banners are enabled by default; **Preferences → Notifications → Desktop
banners** or `desktop_notifications = false` disables just OS delivery.
Delivery runs on a bounded background worker, with at most one banner submitted
per second globally. Floods still update the bounded in-app history. OS privacy,
permission, Do Not Disturb and notification-service settings can suppress banners.
The popover reports delivery errors without displaying backend error details.
OS banners do not currently navigate to panes; use the popover to do that.

## Try it

Run one of these in a Neptune shell (POSIX `printf`):

```sh
printf '\033]9;Ready for review\007'
printf '\007'
printf '\033]777;notify;Build complete;Ready for review\033\\'
printf '\033]99;i=build:d=0;Build complete\033\\'
printf '\033]99;i=build:p=body;Ready for review\033\\'
```

In PowerShell:

```powershell
[Console]::Write("$([char]27)]777;notify;Build complete;Ready for review$([char]7)")
```

The process must emit the sequence to the pane's PTY. Output captured by a hook
runner, a log file or a remote server cannot reach Neptune automatically. SSH
works when the remote program emits OSC through its terminal; multiplexers may
need their own passthrough configuration. BEL has no title or body, so it produces
a **Terminal bell** alert; this includes bells from shell line editors and other
programs. A BEL that terminates an OSC does not produce an extra bell alert.
Ordinary output does not produce attention notifications.

## Coding agents

No agent-name detection is required. Any tool that emits these OSCs can use the
same path. Neptune does not edit agent configuration or install hooks for you.
The examples below are integration recipes, not evidence of live agent runs.

### Codex

Codex's interactive CLI enables notifications by default, but emits them only
when it considers the terminal unfocused. Finishing an agent turn is the trigger;
this does not require exiting the Codex process. In Codex CLI 0.160.0, `auto`
recognizes a fixed list of terminals for OSC 9 and uses BEL for Neptune.
Neptune handles that fallback as a generic **Terminal bell** in-app alert and,
when enabled, a native desktop banner. Switching panes/workspaces or leaving or
minimizing the window supplies the focus loss that Codex's default requires.

For message text and notifications even while looking at Codex, run:

```sh
codex -c tui.notifications=true -c 'tui.notification_method="osc9"' -c 'tui.notification_condition="always"'
```

To keep those choices, merge into the `[tui]` section of your Codex user
configuration (`~/.codex/config.toml`, or `$CODEX_HOME/config.toml`):

```toml
[tui]
notifications = true
notification_method = "osc9"
notification_condition = "always"
```

Use `"unfocused"` instead of `"always"` to have Codex emit only when it considers
the terminal unfocused. Explicit `osc9` avoids relying on terminal-name detection.
Disabling `tui.notifications` or filtering out `agent-turn-complete` suppresses
completion alerts. `codex exec` does not run the TUI notification path.

The separate `notify` setting launches an external command with a JSON argument
for `agent-turn-complete`. It is independent of TUI alerts; it does not itself
create a Neptune notification. To feed Neptune, such a command must write an OSC
to the originating PTY (for example, `/dev/tty` on Unix), rather than to captured
stdout. Neptune does not read Codex transcripts or copy prompt/response contents
into diagnostics. See [official Codex notification configuration](https://learn.chatgpt.com/docs/config-file/config-advanced#notifications).

### Claude Code

Current Claude Code hooks can return a `terminalSequence` JSON field; Claude
writes it to the terminal itself. Save this as `neptune-notify.py`:

```python
import json
print(json.dumps({"terminalSequence": "\u001b]777;notify;Claude Code;Needs your attention\u0007"}))
```

Merge these entries into `.claude/settings.json`, using the script's actual path
and your Python executable (`python3` or `python`):

```json
{
  "hooks": {
    "Notification": [{"hooks": [{"type": "command", "command": "python3 /absolute/path/neptune-notify.py"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "python3 /absolute/path/neptune-notify.py"}]}]
  }
}
```

The terminal sequence requires an interactive Claude session and a version that
supports this field. Hook stdout alone is normally captured; writing raw OSC to
it is insufficient. See [Claude Code's terminal notification hook contract](https://code.claude.com/docs/en/hooks#emit-terminal-notifications).

### OpenCode

For a local OpenCode CLI session, save `.opencode/plugins/neptune-notify.js`:

```javascript
import { openSync, writeSync, closeSync } from "node:fs";

export const NeptuneNotifications = async () => ({
  event: async ({ event }) => {
    if (!["session.idle", "permission.asked"].includes(event.type)) return;
    let fd;
    try {
      fd = openSync(process.platform === "win32" ? "CONOUT$" : "/dev/tty", "w");
      writeSync(fd, "\x1b]777;notify;OpenCode;Needs your attention\x07");
    } catch {
      // A detached server has no originating terminal to notify.
    } finally {
      if (fd !== undefined) closeSync(fd);
    }
  },
});
```

This targets the local server's controlling terminal rather than captured plugin
stdout. A detached server or a client attached from another terminal needs a
client-side OSC integration. See [OpenCode's plugin events](https://opencode.ai/docs/plugins/).

### pi

A pi extension can write OSC directly from its terminal process. For versions
with the `agent_settled` event, save `~/.pi/agent/extensions/neptune-notify.ts`:

```typescript
export default function (pi) {
  pi.on("agent_settled", async () => {
    process.stdout.write("\x1b]777;notify;pi;Ready for input\x07");
  });
}
```

Older pi versions expose `agent_end` instead, which may fire before retries or
queued follow-ups finish. See [pi's notification extension example](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/examples/extensions/notify.ts).

## Protocol scope and limits

- BEL: a generic **Terminal bell** alert, with no inferred message contents.
- OSC 9: plain message; numeric ConEmu subcommands such as `9;4` progress are
  ignored.
- OSC 777: `notify;title;body`; semicolons in the body are retained.
- OSC 99: title/body payloads, identifiers, chunk completion, UTF-8/base64,
  `o=always|unfocused|invisible`, and capability queries. Repeated identifiers
  replace in-app entries only within the same pane generation. `p=close` removes
  the corresponding in-app entry; it does not withdraw an already delivered OS
  banner and is not advertised as a desktop capability.
- Activation/close reports, live-banner queries, custom icons, buttons, sounds,
  urgency and expiry controls are not implemented. Unknown payload kinds are
  ignored. See the [OSC 99 specification](https://sw.kovidgoyal.net/kitty/desktop-notifications/).
- BEL and ST terminators and sequences split across PTY reads work. Incomplete
  messages are limited to 16 per session; each sequence is capped at 8 KiB, each
  decoded title/body at 4 KiB. Control characters are removed. The event queue
  shares the 64-entry/one-MiB clipboard budget and drops oldest entries on
  overflow. Malformed/oversized sequences do not become partial alerts.

Native delivery uses the Linux session notification service, macOS Notification
Center (run the installed `Neptune.app` bundle) and Windows toasts. Windows
registers Neptune’s notification identity under the current user, without admin
rights. On Linux, the background worker keeps one session-bus connection alive
for successive alerts: GNOME removes an application's notifications when its
sending connection closes. Each alert still creates a separate OS notification,
using the packaged `rs.neptune.terminal` desktop identity; OS history limits
apply independently of Neptune's in-app history. A failed delivery lets the next
alert reconnect. Native verification on each OS is required before
claiming verified support; Linux test results do not establish macOS or Windows
behavior.
