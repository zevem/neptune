# Resuming coding agents

On Unix desktops, local terminals started by Neptune add temporary `claude` and
`codex` adapters to their command search path. Launch either CLI normally. When
workspace restoration is enabled, closing and reopening Neptune opens each
remembered agent in its original pane and asks it to resume its recorded session
ID. Several panes can use the same directory without selecting each other's
latest conversation. There is no `--last` or directory-history guessing.

For Codex, review and trust Neptune's hooks when Codex asks: **SessionStart**,
and the six that report [what the agent is doing](#agent-activity). Each
command is `"$NEPTUNE_AGENT_HELPER" --agent-hook codex`, followed by the event's
name for the six; Neptune sets that variable to its own executable for the
launch. The commands read the same at every launch, so Codex asks once, not
again when Neptune is updated or runs from a different path. Existing
hooks remain installed; Neptune does not bypass their trust checks. If the hooks
were skipped, trust them in `/hooks` and restart Codex to capture subsequent
sessions. Codex versions exposing `--no-daemon` run locally so hooks belong to
this terminal, independently of a shared server's environment. Older versions
still open normally, but automatic exact-session capture is unavailable.

Claude Code receives invocation-scoped hooks through `--settings`: SessionStart
and the ones that report its activity. Neither integration rewrites shell dotfiles or the providers'
global settings. Bash and Zsh startup adapters load the user's configuration
before adding the command adapters; other Unix shells, and a shell configured
with its own arguments, use the inherited PATH.
Aliases, functions, absolute executable paths, another shell that replaces PATH,
and user-supplied hook/settings overrides can bypass capture.

If an agent has not reported a usable ID (including disabled or untrusted hooks, or an untouched Codex prompt),
Neptune reopens that CLI at its recorded directory. It cannot guarantee the same
conversation in that case. Exact resumption also requires the provider's local
session history to remain available. An unsuccessful resume keeps the saved
reference and leaves a shell available; **Restart terminal** clears the reference.
Closing an agent normally clears its reference. Closing a pane or workspace
removes it; changing SSH hosts clears it; cancelling a close leaves it intact.
A split starts a fresh shell. Hidden workspaces still receive hook updates.

A restored agent needs the terminal. [Powerlevel10k's instant prompt](https://github.com/romkatv/powerlevel10k#how-do-i-configure-instant-prompt) redirects
stdin to `/dev/null` and captures stdout/stderr until the first prompt. Resuming
Claude inside that window selects its non-interactive mode, which can report
`No deferred tool marker found` and trigger Powerlevel10k's initialization warning.
The Zsh adapter ends instant prompt with `p10k clear-instant-prompt` before
resuming. If other startup configuration leaves input or output redirected,
Neptune does not start the agent as a batch command: the shell opens and the
saved reference is kept.

`--command` takes precedence in its target pane and starts a shell there.
`--no-restore` and `restore_workspaces = false` retain their existing meaning.
Neptune restores new processes, not unfinished tool execution or process memory.
Only provider, session ID, directory and the addresses of
[linked pull requests](#linked-pull-requests) are saved in workspace schema 8,
which reads versions 1–7. Invalid references receive the same recovery-copy protection
as other damaged workspace state. Prompts, transcripts, arbitrary commands,
credentials and permission-bypass flags are not saved or replayed. Transcripts
remain owned by the CLI. Launch-only options and temporary environment changes
(such as a different `CODEX_HOME`) must be reapplied by the user's configuration.

This integration is for local interactive CLIs. Batch/print commands, nested
agents, SSH sessions and Windows shells do not participate. An application update
cannot discover agents launched before its adapters were installed. macOS uses
the Unix adapter but still needs native verification; Linux/X11 is the verified
host for this change.

## Linked pull requests

An agent started through the adapters is also given one Neptune tool,
`link_pull_request`, with a pull request's address
(`https://host/owner/repo/pull/N`). Its instructions ask the agent to call it
after it creates a pull request and when it starts work on an existing one,
including every pull request of a stack. The terminal's tab then shows the pull
request's number beside its close control; clicking the number opens the pull
request in the default browser. A terminal alone in view has no tab, so the
toolbar shows its numbers after the title. One or two pull requests are shown
side by side, the newest nearest the close control. With more, or where two do
not fit beside the title, the newest number carries a chevron and opens a list
of them all. A terminal keeps its eight most recent links, and linking the same
pull request again changes nothing.

The link depends on the agent following those instructions: a pull request
created through `gh` or an API is not discovered on its own, and Neptune does
not look up branches or pull request status. Ask the agent to link a pull
request if its number is missing.

Links belong to the agent's run in that terminal. They return with the agent
when workspaces are restored, and leave when the agent exits, the terminal is
restarted or closed, or the workspace changes SSH hosts. Only an address that
names a pull request over HTTPS is accepted; it is saved without credentials,
query or fragment, and nothing else about the pull request is read or stored.

Claude Code receives the tool as an invocation-scoped server through
`--mcp-config`, and permission for that one tool through `--settings`; its other
servers and permissions are unchanged. Codex receives it through
`-c mcp_servers.neptune…` overrides, on versions that expose `--no-daemon`, and
may ask before the first call according to its approval settings. The server is
the Neptune executable followed by `--agent-mcp`. A server of your own named
`neptune` is replaced for that launch.

## Agent activity

The right panel's **Agents** tab lists each agent the adapters started and says
whether it is working, idle or waiting for a person. The state comes from the
agent's own hooks, corrected by its terminal title where no hook exists.

Hooks. For the launch only, Neptune adds a hook for each of these events;
your own hooks for the same events keep running:

| Agent | Events |
| --- | --- |
| Claude Code | UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, PostToolUseFailure, Notification, Elicitation, ElicitationResult, Stop, StopFailure, PostCompact |
| Codex | UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, Stop, Interrupt |

A submitted prompt or a starting tool means working. A permission request, the
question tools (`AskUserQuestion`, `request_user_input`), plan approval
(`ExitPlanMode`) and an input request from a tool server mean waiting, until
that tool finishes or the turn ends. The end of a turn, a failed turn, an
interrupt Codex reports and a compaction you asked for mean idle. A subagent's
own tools do not change the state; its permission request does, and gives way
to the earlier state once answered. A permission request made while Claude
Code bypasses permissions is not a wait, and a wait shorter than 0.4 seconds
(an automatic reviewer's) is not shown.

Each hook runs the Neptune executable, which reads the event's input, sends at
most the kind of moment and the tool's name to the application over the pane's
private loopback channel, prints nothing and exits 0, so it never decides
anything for the agent. Prompts, commands, tool input and results, and the
transcript are not sent, logged or saved; the state itself is not saved either.
The hooks run in order with a five-second limit (three for Codex's Interrupt,
the most Codex allows) and normally take a few milliseconds; input over 4 MB is not parsed and only its event is used.

Titles. Neither CLI reports every ending. Claude Code runs no hook when a turn
is interrupted, or when a question, plan or permission request is dismissed or
refused. Both CLIs keep the terminal title current: Claude Code leads it with a
spinner while busy and `✳` otherwise; Codex shows a spinner while working and
`Action Required` while blocked. Once a title has contradicted the hooks for
two seconds it wins: a working agent whose title has gone calm is idle, and an
idle or waiting agent whose title shows work is working. `Action Required`
means waiting at once, and work that follows it means working at once. Calm counts only after the title has shown work in that
run, so a title without a spinner cannot hide work. A waiting Claude Code
shows the same `✳` as an idle one, so there the keys you type decide: Escape
in that terminal ends the wait after two seconds without a hook, and Enter or
a digit does so for a permission request or a plan when the title stays calm.

Limits. With Codex's hooks untrusted, or a Codex without `--no-daemon`, the
title alone is used and states lag by about two seconds. An agent whose title
is disabled or replaced (`CLAUDE_CODE_DISABLE_TERMINAL_TITLE`, a Codex
`terminal_title` without the spinner) can stay "Working" after an interrupt, or
waiting after a refusal, until its next prompt. Answering one part of a
several-part question does not change the state. Background shells and
subagents that outlive a turn are not shown as work unless the title says so.
The limits in the first section (SSH, Windows, batch commands, bypassed
adapters) apply here too: those agents are not listed.

## Implementation and source review

The pure model validates resume references and accepts generation-tagged
`PaneAgentChanged` commands. Persistence stores a reference on each pane. The
desktop startup workers install private adapters, and a single blocking loopback
listener receives bounded metadata messages. Each pane generation has a random
callback credential, and each CLI invocation has its own identity. Closed or
replaced panes and late hooks from exited invocations cannot overwrite a newer
session. A linked pull request is accepted only from the invocation that is open
in its pane, and reaches the model as a generation-tagged
`PanePullRequestLinked` command. Pending updates coalesce per pane; hooks wake the application on change,
so idle integrations do not poll or repaint. Socket reads have byte and total-time
limits. Startup-file creation and cleanup stay on workers.

Activity uses the same channel and credentials. The hook process reduces an
event to one of a closed set of signals; the bridge accepts it only from the
invocation open in that pane, reduces signals to a state per pane, coalesces
it between frames and wakes the application when the state changes or a turn
begins or ends. The application holds the state outside the model, drops it
when the pane's generation, lifecycle or agent reference changes, and wakes
itself only for the two-second and 0.4-second deadlines above.

A small POSIX supervisor preserves foreground signal handling and reports normal
CLI exit. Resumption runs as part of shell startup with quoted arguments, never
by typing commands into a terminal that could still be showing another prompt.
No backend terminal-engine types or transcript readers were added.

Reviewed against the installed **Codex CLI 0.160.0** and **Claude Code 2.1.287**,
and the providers' documentation on 2026-10-02:

- [Codex CLI](https://learn.chatgpt.com/docs/codex/cli) and local `codex resume --help` describe targeted resumption.
- [Codex hooks](https://learn.chatgpt.com/docs/hooks) describes SessionStart metadata, additive hook sources and trust review.
- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference) describes `--resume` and invocation settings.
- [Claude Code hooks](https://code.claude.com/docs/en/hooks) describes session-start metadata and lifecycle changes.

## Focused verification

The startup regression covers both login and non-login Zsh with redirected
stdio. The helper integration test independently redirects stdin and stdout on
a real PTY for both providers and asserts that no CLI runs or prints output:

```sh
cargo test -p neptune-terminal --lib runtime::agents --locked
cargo test -p neptune-terminal --test agent_restore --locked
```

`python3 scripts/verify-agent-activity.py` does the same for the Agents tab:
its fixtures read the hooks injected for them, fire them with realistic input
(including a 6 MB tool result) and set terminal titles, and the rows are read
back through inspection. `NEPTUNE_EXPLORER_STATE=agents`, `agents-empty` and
`agents-closed` of `app::tests::capture_explorer_native` capture the tab.

`python3 scripts/verify-agent-restore.py` runs deterministic CLI fixtures in an
isolated native app, with fresh storage and a unique inspection endpoint. It
checks independent IDs in one directory, graceful close/reopen, normal exit,
Ctrl+C and narrow-window presentation. Fixtures prove Neptune's contracts without
sending model requests; they do not stand in for installed-provider validation.
Build the inspection targets described in [native verification](verification.md#native-application-verification) first.

The implementation was also checked in the running Linux/X11 app with the two
installed CLIs above. Each received a no-tools request to reply with `OK`; both
conversations and their exact IDs returned after a graceful close and reopen.
The development build with `inspection` was reviewed at 1100×700 and 640×440.
The deterministic native regression additionally verifies normal agent exit and
Ctrl+C. These checks establish Linux behavior, not macOS/Windows readiness or
performance. Focused model, persistence, runtime and application tests, desktop
library Clippy and the architecture boundary check passed.

Pull request linking was checked on 2026-10-03 in the Linux/X11 development
build with `inspection`, at 1100×700 and 640×440. The deterministic native
regression starts the tool server as each provider is configured to, and covers
links per terminal, repeats and non-addresses, the toolbar and tab numbers,
the list of a terminal with several, their handoff to the browser launcher,
reopen, and agent exit. With the
installed **Claude Code 2.1.288**, the tool's instructions alone led it to link
a pull request it was told it had created; **Codex CLI 0.160.0** linked one when
asked to call the tool. A prompt given as a launch argument reached Claude Code
before the tool server had connected, and that pull request was not linked.
macOS and Windows are unverified.

The Powerlevel10k warning was separately reproduced on 2026-10-02 in the native
Linux/X11 app using the user's unmodified Zsh configuration and Claude Code
2.1.288. The old build printed both the initialization warning and the deferred
tool marker error. The fixed development build with `inspection` reopened the
same session through both login and non-login Zsh, displayed its earlier
conversation, and answered new requests without either warning. Captures were
reviewed at 1100×700 and 640×440. Normal Claude exit returned to the shell and
cleared the saved reference. Both regression tests were also observed failing
with their respective protections removed, then passing with the fix restored.

Agent activity was checked on 2026-10-04 in the Linux/X11 development build
with `inspection` (Wayland session, XWayland window), at 1100×700, 1000×680 and
640×440. The deterministic native regression passed for both providers. With
the installed **Claude Code 2.1.289** in an isolated instance, the row went
from idle to working on a prompt, to "Asked a question" when `AskUserQuestion`
ran, back to working when answered and to idle at the end of the turn; Escape
on a question and Escape during generation, which fire no hook, each returned
it to idle two seconds later; `/exit` removed the row. With **Codex CLI
0.160.0**, declining the hook review left the title to report working and idle
about two seconds late; after "Trust all and continue" the hooks reported a
turn and an interrupt. Codex's permission and question prompts, Claude Code's
permission and plan prompts, subagents, tool-server input requests and
compaction were exercised only through fixtures and unit tests. macOS and
Windows are unverified.
