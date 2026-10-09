# Resuming coding agents

On Unix desktops, local terminals started by Neptune add temporary `claude`,
`codex`, `opencode`, `gemini`, `pi` and `omp` adapters to their command search path.
Launch any of them normally. When
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
and the ones that report its activity.

OpenCode, pi, Oh My Pi and Gemini CLI have no command hooks that can be added
for one launch, so each is reached its own way:

| CLI | What Neptune adds to the launch | Resumed with |
| --- | --- | --- |
| OpenCode | A plugin of Neptune's, named in `OPENCODE_CONFIG_CONTENT` beside the plugins of your own configuration | `opencode --session ses_…` |
| pi | An extension of Neptune's, as `pi -e`, when the `pi` on the search path resolves into a `pi-coding-agent` package | `pi --session <id>` |
| Oh My Pi | The same extension, as `omp -e` | `omp --resume <id>` |
| Gemini CLI | Nothing: its hooks come only from its settings files, which are yours | Not resumed: the CLI is reopened in its directory |

The plugin and the extension say when a turn begins and ends and when a person
is asked something, and name the session once it has had a turn; a session
without one has nothing to reopen. `opencode --pure`, `OPENCODE_PURE`, or an
`OPENCODE_CONFIG_CONTENT` of your own that is not a JSON object with a list of
plugins, leaves OpenCode without Neptune's plugin: it is listed, but stays
"Idle". On an SSH host any `OPENCODE_CONFIG_CONTENT` of your own does. OpenCode and pi need no trust review for either. Gemini CLI is
followed by its terminal title alone, as [below](#agent-activity).

No integration rewrites shell dotfiles or the providers'
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
Only provider, session ID, directory, the addresses of
[linked pull requests](#linked-pull-requests), the paths and titles of
[attached files](#attached-files), the terminal whose agent
[started an agent](#agents-that-start-agents), the
[worktree made for an agent](#agents-in-worktrees) and the
[project](#projects) a terminal's agent belongs to are saved in workspace
schema 13, which reads versions 1–12; version 10 widened the CLIs a reference
can name, version 12 added the pull requests and files of
[conversations set aside](#a-new-conversation), under their provider and
session ID, and version 13 added projects. Invalid references receive the same
recovery-copy protection as other damaged workspace state. Prompts,
transcripts, arbitrary commands, credentials and permission-bypass flags are
not saved or replayed. The one exception is a [project](#projects), which
keeps the conversation with its lead and its shared context in a folder of
its own, never in workspace state. Transcripts remain owned by the CLI. Launch-only options and temporary environment changes
(such as a different `CODEX_HOME`) must be reapplied by the user's configuration.

This integration is for local interactive CLIs. Batch/print commands and
Windows shells do not participate, and neither does a CLI that one
agent runs from its own shell, as opposed to an agent it
[starts through Neptune](#agents-that-start-agents). An agent in an SSH
workspace is [listed with what it is doing](#agents-on-ssh-hosts) and nothing
more: it is not saved, reopened, given Neptune's tools or started by another
agent. Only Claude Code and Codex can be started by another agent or lead a
[project](#projects). An application update
cannot discover agents launched before its adapters were installed. macOS uses
the Unix adapter but still needs native verification; Linux/X11 is the verified
host for this change.

## Linked pull requests

An agent started through the adapters is also given Neptune's tools. One is
`link_pull_request`, with a pull request's address
(`https://host/owner/repo/pull/N`). Its instructions ask the agent to call it
after it creates a pull request and when it starts work on an existing one,
including every pull request of a stack. The terminal's tab then shows the pull
request's number beside its close control; clicking the number opens the pull
request in the right panel's [Pull request tab](usage.md#pull-requests), and
Ctrl-click (Command-click on macOS) in the default browser. A terminal alone in view has no tab, so the
toolbar shows its numbers after the title. One or two pull requests are shown
side by side, the newest nearest the close control. With more, or where two do
not fit beside the title, one number carries a chevron and opens a list
of them all. A terminal keeps its eight most recent links, and linking the same
pull request again changes nothing.

The link depends on the agent following those instructions: a pull request
created through `gh` or an API is not discovered on its own, and Neptune does
not look up branches. Ask the agent to link a pull request if its number is
missing.

### Pull request status

A linked number also shows where its pull request stands, so that one needing
you can be seen without opening the browser:

| The pull request | Its number |
| --- | --- |
| Open | The accent colour |
| Draft | Grey |
| Merged | Purple, with the merge icon in place of the pull request icon |
| Closed without merging | Dimmed |

While a pull request is open or a draft, two marks follow its number. The
first is for the checks of its last commit taken together: a green check when
they pass, a red cross when one fails and a yellow ring while any is running or
expected; a commit without checks has no mark. The second is a comment icon
with a count, in the attention colour, of the review conversations that are not
resolved; "99+" stands for a hundred or more. A merged or closed pull request
carries neither. Resting the pointer on a number says the same in words, for
example "Open · Checks failing · 2 unresolved comments". The number that
stands for several pull requests is the newest still open or a draft, or the
newest of all once none is; it carries the worst state of their checks and
the sum of their unresolved conversations, and its list says each one's state.
Where a tab is short of room the marks are given up before the number is.

Neptune reads this through the [GitHub CLI](https://cli.github.com) (`gh`), as
the account signed in to it: one request for each host names the linked pull
requests of the workspace in view and asks for their state, the combined state
of their checks and whether each review conversation is resolved. Neptune
holds no token of its own, and nothing it reads is saved. Without `gh`, without
a sign-in for the pull request's host (`gh auth login`, with `--hostname` for
GitHub Enterprise), without a network or for a host that is not GitHub, the
number looks as it did before and its tooltip says the status is unavailable.
Titles, comments and other contents of a pull request are requested only for
the one you open in the [Pull request tab](usage.md#pull-requests), while that
tab is in view.

A state is read when its link first comes into view and again every 15 seconds
while checks are running, every minute while the pull request is otherwise in
review, every ten minutes once it is merged or closed, and every minute after a
lookup that failed. One worker makes one request at a time, for at most 32 pull
requests, with a 20-second limit and a bounded response; the window is drawn
again only when a state changed. Only the workspace in view is read; the last
state of another is kept for an hour, shown again on return and read afresh.

Links belong to the agent's conversation. They return with the agent when
workspaces are restored, and leave the tab when the agent turns to another
conversation or exits, the terminal is restarted or closed, or the workspace
changes SSH hosts; they are back when the conversation is resumed
([below](#a-new-conversation)). Only an address that
names a pull request over HTTPS is accepted; it is saved without credentials,
query or fragment, and nothing else about the pull request is stored.

## Attached files

A path in a terminal does not show you a picture. An agent started through the
adapters is therefore given a second tool, `attach_file`, with a file's `path`
and an optional `title` of a few words. Its instructions ask the agent to call
it for each screenshot, image, recording, report or other file it makes for
you to look at, and whenever you ask it to attach or show one. The path may be
absolute, relative to the agent's directory or begin with `~/`; the file must
exist, and a folder is refused.

The terminal's tab then shows a paperclip and the number of attached files,
before a merged worktree's mark, the count of started agents and the pull
request numbers; a terminal
alone in view has no tab, so the toolbar shows it. Click it for the list,
newest first. Each row has the file's title, or its name, above the folder it
is in, and a small copy of the picture when the file is one (PNG, JPEG, GIF,
WebP or BMP, decided by its contents). A row does this:

| Click | What happens |
| --- | --- |
| A picture's row | The picture at full size over the window. The wheel and the plus and minus keys zoom, 0 fits it again, the arrow keys and the controls at the window's sides step through the terminal's other attached pictures, and a click or Escape closes it. With several pictures, a band at the foot of the window holds a small copy of each, newest first: click one to see it. The folder control in its caption shows the file in the file manager. |
| Another file's row | The file opens with its default application. |
| The folder control | The file manager opens with the file selected. |
| The cross (**Dismiss**) | The file leaves the list. The file itself is not touched. |

When the list holds more than one picture, **View all pictures** under the rows
opens that full view on the newest, to look through them without returning to
the list. **Dismiss all** empties the list. The list stays open while you
dismiss files and closes when you open one.

A terminal keeps its 24 most recent files. Attaching the same path again moves
it to the top with its new title and reads its picture afresh, so an agent can
replace a screenshot it has retaken. A file that was moved or deleted since
stays on the list, marked "No longer there", until you remove it.

Attached files belong to the agent's conversation, like linked pull requests:
they return with the agent when workspaces are restored, leave the tab when
the agent turns to another conversation or exits, the terminal is restarted
or closed, or the workspace changes SSH hosts, and are back when the
conversation is resumed. Only the path and the title are kept and saved; Neptune
does not copy the file, so it shows what is at the path when you open it.
Pictures are read on a worker within fixed limits (64 MB a file, 16,384 pixels
a side), one at a time, for the workspace in view. Paths, titles and pictures
are not logged or sent to diagnostics.

As with pull requests, this depends on the agent following its instructions:
a screenshot it only names in its answer is not attached. Ask it to attach the
file. Only Claude Code and Codex in local terminals are given the tool; an
agent of another kind, or one on an SSH host, has none.

### A new conversation

Neptune tells one conversation from the next by the session ID the CLI
reports, and shows on a tab the pull requests and files of the conversation
its agent is in. When the agent turns to another one without exiting, the
tab's pull requests and attached files are set aside with the conversation
that made them, and the tab shows those of the one it turned to: none for a
new conversation, and its own again for one that is resumed. They also wait
for a conversation whose agent exited or whose terminal was restarted, and
return when a tab's agent resumes it, in that terminal or another. Compacting
a conversation keeps its ID, and so what the tab shows.

| CLI | The tab changes |
| --- | --- |
| Claude Code | At once, on `/clear` and on `/resume` |
| Codex | It empties at once on `/new`, `/clear` and `/resume`; what a resumed session had returns with the first prompt sent in it |

Codex names a session only when a prompt is sent in it. Neptune learns that
it turned to another from its tool server, which Codex starts again for each
session, and that needs Codex's hooks trusted: without them a tab keeps what
it shows until the agent exits.

The 32 conversations set aside most recently are kept, with the same
addresses, paths and titles as on a tab, and are saved with the workspaces.
Closing a terminal while its agent runs, or changing a workspace's SSH host,
drops what its tab showed. The agents a conversation started stay listed
until they are closed or their starter exits.

## Agents in worktrees

Several agents in one checkout edit the same files. **New agent in worktree**
gives an agent a git worktree of its own: a second checkout of the repository,
on its own branch, in its own folder. It is in the command palette, in a
terminal's menu and on Ctrl+Shift+G (Command+G on macOS), for a terminal on
this machine whose directory is in a git repository.

It asks for one thing, the branch. Type a name, choose Claude Code or Codex,
and press Enter. A few words are a name: `fix login` becomes `fix-login`. The
sheet says beneath the field what Create will do before you press it. Neptune
then

- creates the branch from the repository's default branch (the one
  `origin/HEAD` names, as your checkout has it, else `main` or `master`; the
  current commit if there is none), or checks out the branch as it is when it
  already exists;
- adds its worktree beside the repository, in `<repository>.worktrees/<branch>`
  with `/` written as `-`, so the repository's own status and ignore rules do
  not see it;
- opens a tab after the terminal you started from, in that folder, and starts
  the CLI there as if you had typed `claude` or `codex`.

The tab is named by the branch, with the program it runs beside the name; a
terminal alone in view has no tab, so the toolbar names the branch. Tabs and
splits opened from it are ordinary terminals in the same folder. The
repository's own checkout is not touched: its branch, index and files stay as
they were. The sheet opens the CLI the focused terminal runs, or the one you
chose last.

Cleaning up. While a worktree is open in a tab, Neptune looks at its branch
every ten seconds, with git commands that change no branch, index or working
file and take none of the repository's optional locks. Once the branch's work is in the default branch, here or as
`origin/` has it after a fetch, and nothing in the worktree is uncommitted,
the tab shows **Merged**. A merge commit, a fast-forward, a squash and a
rebase all count: the branch's commits are in the default branch, or merging
it there would change nothing. Click **Merged** and confirm: the terminals in
the worktree close, its folder is removed and its local branch is deleted.
Neptune does not fetch, so a pull request merged on a server shows once your
checkout has fetched or pulled it.

**Remove worktree** in the terminal's menu and the command palette does the
same at any time, and says first what would go:

| The worktree has | Removing it |
| --- | --- |
| A merged branch, or no commits the default branch lacks | Removes the folder and deletes the local branch |
| Commits that are not in the default branch | Removes the folder and keeps the branch with its commits |
| Changes that were never committed | Asks to **Discard and remove**; those changes are deleted with the folder |

A branch on a remote is never deleted, and nothing is pushed or fetched.
If git refuses, nothing is closed and the message says why.

The sheet also lists the worktrees Neptune made for this repository, with
**Merged into …** on the finished ones. Click one to start the chosen CLI in it in a new tab, or its bin to remove
it. A worktree can hold several agents, of the same CLI or both: each click
adds a tab beside any that already work there, and a worktree whose tabs you
closed is found here too. Typing the name of a branch that has a worktree
opens another tab in that worktree.

A worktree stays with its terminal when the agent exits or the terminal is
restarted, and returns with it when workspaces are restored, where its agent
is resumed like any other. Connecting the workspace over SSH ends it, since
its terminal then runs elsewhere. A worktree whose folder was deleted outside
Neptune is restored as an ordinary terminal in the workspace's directory.

Limits. Local terminals only: no SSH workspaces. On Windows the tab opens in
the worktree without starting a CLI, because the adapters that start one are
Unix-only. `git` must be on `PATH`. A branch checked out in the repository
itself cannot also have a worktree. Squash and rebase detection needs git
2.38 or later; older versions notice only branches whose commits are in the
default branch. Submodules, and files git ignores such as dependencies or
`.env`, are not copied into a new worktree. An
[agent that another agent starts](#agents-that-start-agents) is still given
its directory by that agent.

## Agents that start agents

An agent started through the adapters can hand work to another: ask Codex to
build a feature's backend and let Claude Code build its frontend, and Codex
starts Claude Code, gives it the task and reads its answer. Claude Code can
start Codex the same way, and either can start more of its own kind.

Neptune's tool server offers seven tools for this:

| Tool | What it does |
| --- | --- |
| `spawn_agent` | Starts `claude` or `codex` with a task, in a directory, on a model, at an effort and in its ultra mode if they are given, and returns the agent's number once it has taken the task |
| `wait_for_agent` | Returns when a started agent has news: a turn ended, it sent a message, it asks something, it needs a person, or it ended |
| `send_agent_message` | Gives a started agent its next prompt |
| `list_agents` | Says what each started agent is doing |
| `close_agent` | Closes a started agent and its terminal |
| `reopen_agent` | Opens a started agent again after its terminal was closed or its CLI exited, with the conversation it had |
| `press_agent_keys` | Types the user's answer to what a started agent's CLI asks before it takes its task |

A started agent runs in a terminal out of view: it has no tab, and the
keyboard and what is in view stay where they were. The starting agent's tab
carries a count of the agents it started, before its pull request numbers; a
terminal alone in view has no tab, so the toolbar carries it. Click the count
for the list, where each agent is named by its CLI and conversation and says
what it is doing, and click an agent to open its terminal. It then has the tab
after the agent that started it, takes the keyboard, and stays a tab like any
other. The count turns to the attention colour while one of them waits for a
person. Started agents are also rows of the [Agents tab](#agent-activity) like
any other, and opening one there gives it its tab too. The sidebar counts a
workspace's tabs, so a terminal out of view is not counted.

Send to background. An opened agent goes back out of view without being
stopped: choose **Send to background** from its tab's menu, from the list
under the count, from a secondary click on its row in the Agents tab, or run
**Send terminal to background** from the command palette (config name
`background-pane`, no default shortcut). Its tab goes, the keyboard passes to
the tab that takes its place, and the agent runs on and stays listed as it
was before you opened it; it is saved and restored that way too. From then on
it is a terminal out of view again in every respect: `close_agent` closes it,
and it closes with its agent if that exits. This is offered only for a
terminal whose agent another agent of the same workspace or a project
started, since nothing lists any other terminal, and not for the last
terminal in view in its workspace. Closing a tab still ends what runs in it.

The started CLI runs interactively, as if you had typed `claude "task"` or
`codex "task"` there: it has its own conversation, tools, permission mode and
settings, and once you open it you can read along, answer it or type to it.
It does not see the conversation of the agent that started it. Neptune puts
one sentence before the task, saying which agent started it and that the last
message of each turn is what that agent reads. That last message is taken from
the CLI's own Stop hook. A started agent is also offered `reply_to_parent`, to
say something before its turn ends. A message from `send_agent_message` is
pasted into the started agent's prompt and submitted; one sent while it works
is taken when it gets to it, as a typed one would be.

Models. `spawn_agent` takes an optional `model`, named as the CLI names it,
and passes it as `claude --model` or `codex -m`: an alias such as `opus`,
`sonnet` or `haiku` or a full model id for Claude Code, a model id for Codex.
Without one the CLI uses its own default. The name is only checked to be a
name and not an option; a model the CLI does not know is the CLI's to refuse.

Effort and ultra. `spawn_agent` also takes an optional `effort` and an
optional `ultra`. The effort is passed as `claude --effort` or as Codex's
`model_reasoning_effort` setting for that launch, and each CLI has its own
levels: `low`, `medium`, `high`, `xhigh` and `max` for Claude Code, and
`minimal` and `ultra` besides those for Codex. A level the chosen CLI does
not have is refused with the list of those it has. `ultra` asks for the CLI's
ultra mode. For Claude Code that is ultracode, its standing multi-agent
workflow orchestration, turned on for that session through the `ultracode`
key of the settings Neptune passes for the launch, at whatever effort was
given. For Codex it is the Ultra effort, so it cannot be combined with another
effort. Both use far more tokens; the tool tells the starting agent to set
them only when you ask.

An agent that is reopened runs on the model, at the effort and in the ultra
mode it was started with. A started agent that returns when workspaces are
restored uses the CLI's defaults again.

Directories and worktrees. A started agent works in the directory of the
agent that started it unless `cwd` names another, which must exist. Neptune
creates no worktree for a started agent: the tools' instructions tell the
starting agent to make one with `git worktree add` and pass its path when both
agents would edit the same files. The started agent's row and tab show the
directory it was given. For an agent you start yourself, Neptune makes and
removes the worktree: see [agents in worktrees](#agents-in-worktrees).

Questions before the task. A CLI can open on a question of its own before it
takes anything: whether to trust a new folder, to review hooks that changed,
to sign in. Nobody is watching a terminal out of view, so Neptune watches for
it. `spawn_agent` returns only once the CLI has taken its task, and a terminal
that has drawn nothing new for four seconds without taking it is reported as
asking: the tool's answer carries the last lines its terminal shows and tells
the starting agent to put the question to you at once. The count turns to the
attention colour and the agent's row reads "Needs your answer". You can open
its terminal from the list and answer there, or tell the starting agent your
answer, which `press_agent_keys` then types: up to eight of enter, escape,
tab, space, the arrow keys and single letters or digits. Keys are pressed
only for a question from before the task, never for a permission request or
anything else a running agent asks. These lines are the only terminal
contents that leave a terminal, and they go to the agent that started it, not
to disk or diagnostics. A terminal you have in view is not read: its question
is reported without its text.

Permissions. Neptune allows Claude Code `wait_for_agent`, `list_agents`,
`reply_to_parent`, `link_pull_request` and `attach_file` for the launch. Starting or
reopening an agent, typing for one, pressing keys for one and closing one
hand work to another CLI or stop it, so `spawn_agent`, `reopen_agent`,
`send_agent_message`, `press_agent_keys` and `close_agent` are decided by
Claude Code's own permission mode and by Codex's approval settings, like any
other tool: an agent that asks before it runs a command asks before it starts
an agent. With permissions bypassed nothing asks, and an agent could press a
key without having been told to; its instructions say not to. The started
agent asks for what its own work needs, in its own terminal; `wait_for_agent`
then reports that it is waiting for a person and what for, and nothing is
typed over the request. Codex is given a five and a half minute limit for
Neptune's tools in place of its one minute, so that a wait of up to five
minutes can finish.

Closing and reopening. `close_agent` closes a started agent with its
terminal. You can also close its tab once you opened it, or it can exit by
itself; `wait_for_agent` reports either once, with anything it had still to
say. While the agent that started it runs, it can bring that agent back with
`reopen_agent` and a message: a new terminal out of view runs
`claude --resume` or `codex resume` for the conversation its CLI last named,
in the directory and with the model, effort and ultra mode it had, and takes
the message as its next prompt. It has a new number from then on. An agent that never named a
conversation, or whose directory is gone, cannot be reopened and a new one is
started instead.

Lifetime. A link lasts while both agents run. It returns with them when
workspaces are restored, out of view where it was, and the restored agents
can go on talking. It ends when the started agent exits or its terminal is
restarted or closed. A started agent that exits in a terminal out of view
takes that terminal with it; one whose terminal has a tab leaves its
shell there. When the starting agent exits or its terminal is closed, the
agents it started keep running, answer to no one, and each gets a tab so that
nothing runs out of view unattended. An agent reaches only the agents it
started, by the number it was given.

Limits. An agent can have eight started agents open at once, and the eight
most recently ended ones are remembered for reopening. An agent you started
can start agents and those can start agents; these last cannot. A task,
message or reply is at most 32 KB, a longer reply is cut there, and the
sixteen most recent unread replies are kept. A terminal out of view has not
been laid out, so its CLI draws at the default size until you open it. A
question before the task is recognised by a terminal standing still, so a CLI
that takes four seconds to draw anything new while starting is reported as
asking, with what it shows, until it goes on. With Codex's hooks untrusted its
replies are not captured: the title still says when a turn ends, and
`reply_to_parent` still delivers. Text a person has typed but not sent in a
started agent's prompt is submitted together with a message that arrives. The
task, the model and the effort are the CLI's arguments, so they are visible in
the process list like any command line. The limits of the first section apply too: no SSH
sessions, Windows shells or bypassed adapters.

Tasks, messages, replies and the lines of a question before the task pass
through the application's memory between the two terminals. They are not saved, logged or sent to diagnostics; a reply
nobody collected is dropped with its link. Only an agent that another agent
or a project's lead started hands over the last message of its turns; every
other agent's hooks send what they did before.

A [project](#projects) is the exception to "not saved". What its lead's
agents report is written to the project's chat as it is handed to the lead,
under the [storage contract](#what-a-project-stores) below. The lines of a
question before the task are still never written, and nothing is logged or
sent to diagnostics. Agents that agents start outside a project stay
memory-only, as this section says.

## Projects

A project gives a workspace a **lead**: a Claude Code or Codex that plans,
starts agents through Neptune and is told what they do. The
[projects guide](projects.md) describes it for the person using it; this
section says how it differs from [agents that start agents](#agents-that-start-agents),
on which it is built.

The lead is not in a terminal. Neptune runs the installed CLI as a process of
its own, in the project's folder, and speaks its machine protocol: Claude
Code's `stream-json` input and output, or `codex app-server`. The lead's only
tools are Neptune's, from the same tool server (`neptune --agent-mcp`) the
adapters give a terminal's agent, under an identity that belongs to the
project instead of to a terminal:

| | A terminal's agent | A project's lead |
| --- | --- | --- |
| Runs in | Its terminal, interactively | A process without a terminal, started on the first message and let go after 30 minutes at rest with no agents |
| Its own tools | All of the CLI's | None for Claude Code (`--tools ""`, no settings sources, `--strict-mcp-config`, permission mode `dontAsk`). For Codex a read-only sandbox without network, approvals `never`, web search and the optional features off, and your other tool servers disabled for that conversation |
| Agents it starts | End their link when it exits, and get tabs | Belong to the project: they stay out of view when the lead's process exits or is replaced, and get tabs when the project is removed |
| Learns of its agents' news | By calling `wait_for_agent` | Neptune collects it and gives it to the lead as a turn; the lead has no waiting tool |
| Told about a paused project | Not applicable | Starting, messaging and reopening agents and adding watches are refused while the project is paused |

The lead is offered thirteen tools, all allowed without asking:

| Tool | What it does |
| --- | --- |
| `spawn_agent` | Starts `claude` or `codex` out of view with a `title` and a task, optionally on a `model` and at an `effort`, in the project's directory, in a `cwd` inside it, or in a git `worktree` Neptune makes for the branch named. Returns once the terminal exists: "Agent N started in …. You will be told when it finishes or needs someone." |
| `send_agent_message` | Gives one of its agents its next prompt, preceded by the decisions recorded since that agent was last told |
| `list_agents` | Says what each of its agents is doing |
| `agent_report` | Returns the whole last report of an agent, which a turn carries only the first 6 KB of |
| `close_agent` | Closes an agent's terminal, or, when a person has it open as a tab, takes the agent out of the project and leaves the terminal open |
| `reopen_agent` | Opens a closed agent again with its conversation, also after Neptune was restarted |
| `read_context` | Reads the index of the project's shared context, or one of its files, 20 KB at a time |
| `write_context` | Replaces `STATUS.md` or adds to a note |
| `record_decision` | Adds a dated entry to `DECISIONS.md` |
| `add_subscription` | Adds a watch: a schedule of at least 15 minutes, which waits for the person's **Allow**, or a pull request, which runs at once |
| `list_subscriptions`, `remove_subscription` | Lists and removes the project's watches |
| `pull_request_status` | Says how a pull request stands: state, checks and unresolved review conversations |

It is not offered `wait_for_agent`, `press_agent_keys`, `link_pull_request`,
`attach_file` or `reply_to_parent`. Watches are called subscriptions in the
tools' names.

An agent the lead starts is a **project agent**. It has the tools of any
[started agent](#agents-that-start-agents), `reply_to_parent` included, which
reaches the lead as news, and two more that Neptune allows Claude Code
without asking:

| Tool | What it does |
| --- | --- |
| `read_context` | As for the lead |
| `add_note` | Adds an entry to `notes/<topic>.md`, signed with the agent's number and title |

Neptune writes the project agent's brief itself, in place of the one sentence
a started agent gets: the person's instructions, the newest decisions, the
top of the status, the index, the folder's path, its worktree if it has one,
and the rule that the last message of each turn is its report. A project
agent may start agents of its own with `spawn_agent`; those are ordinary
started agents, are not members of the project, get none of its context and
cannot start more. A project has at most six agents open and starts at most
forty in a day.

Credentials. A lead gets a token and run of its own for the loopback
channel, and the role `NEPTUNE_AGENT_ROLE=lead`; a project agent's shell has
`NEPTUNE_AGENT_ROLE=member`. The role only chooses which tools the tool
server lists: authority follows the token, and a lead's token is refused once
its process has been replaced. Before either CLI starts, inherited
`NEPTUNE_AGENT_*` variables are removed from its environment and the
adapters' folder from its `PATH`; a Claude Code lead also loses `CLAUDECODE`,
`CLAUDE_CODE_*`, `NODE_OPTIONS` and `ANTHROPIC_API_KEY`, and a Codex lead
`CODEX_SANDBOX*`. A Claude Code lead then has its own four variables in its
process environment, and again in a file readable by your account alone, in
a private temporary folder, named by `--mcp-config` and removed as soon as
the CLI has read it. A Codex lead's process has none of them: they travel
only in the line that opens the conversation, on its standard input, for
Codex to give the tool server. The token is in neither command line.

The lead runs on the model its CLI chooses. `NEPTUNE_LEAD_MODEL` in
Neptune's own environment names another; native checks set it to keep their
runs small, and it is not a setting.

A project agent's brief is the CLI's argument, like any task, so the
instructions and decisions in it are visible in the process list while the
agent starts.

### What a project stores

This is the storage contract. It amends the memory-only rule above for
projects and for nothing else.

| Question | Answer |
| --- | --- |
| What is stored | The person's messages to the lead; the lead's finished replies; the tools the lead used, each with a one-line summary Neptune wrote; agents' reports as handed to the lead; watch titles and instructions; the shared context files; the project's name and directory; the lead's conversation ID; and a record of the project's agents (title, CLI, conversation ID, directory, worktree, pull request addresses, state, last report) |
| Where | Only `projects/<key>/` in the data directory: `chat.jsonl` with one earlier `chat.1.jsonl`, `project.json`, and `context/`. Folders are mode 0700 and files 0600 |
| What workspace state holds | The project's number, folder key, name, workspace, lead CLI and paused flag, and each member terminal's project number. No words of the chat, tasks, watches or context |
| What is never stored | Terminal contents; the lines of a question an agent asks before its task (the chat records that it asked, without the text); credentials; text the lead was still streaming; the lead's reasoning |
| What enters diagnostics | The operation, the project's number, elapsed time and a closed failure kind. Never text, titles, paths or tokens |
| What leaves the machine | Nothing through Neptune. The lead and its agents are the person's own CLIs, which send conversations to their providers and keep their own transcripts |
| How it is deleted | **Remove project…** deletes the folder. **New chat** sets the chat aside as `chat.1.jsonl`. Context files are deleted from their rows or in a file manager |
| What is unchanged | Agents that agents start outside a project stay memory-only |

`project.json` is version 1 and `chat.jsonl` begins with a format line. A
`project.json` of another version, an unreadable or oversize one, or a chat
with another format line makes that project read-only: its lead stays off
and nothing in its folder is written. A damaged `project.json` is copied
aside before it is replaced. Screenshot launches write nothing.

## Agent activity

The right panel's **Agents** tab lists each agent the adapters started and says
whether it is working, idle or waiting for a person. The state comes from the
agent's own hooks, corrected by its terminal title where no hook exists.
OpenCode and pi report through Neptune's plugin and extension instead of
hooks, and Gemini CLI through its title alone.

Hooks. For the launch only, Neptune adds a hook for each of these events;
your own hooks for the same events keep running:

| Agent | Events |
| --- | --- |
| Claude Code | UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, PostToolUseFailure, Notification, Elicitation, ElicitationResult, Stop, StopFailure, PostCompact |
| Codex | UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, Stop, Interrupt |

| Agent | What its plugin reports |
| --- | --- |
| OpenCode | A session turning busy or idle; a permission or a question asked, and answered or dismissed |
| pi | A turn starting and ending; a dialog an extension opens, and its closing |
| Oh My Pi | A turn starting and ending, but for a loop it continues by itself; a tool approval asked and resolved; its `ask` tool starting and finishing |

OpenCode's subagents run in sessions of their own: their turns do not change
the state, their permission requests do. pi asks no permission before a tool
and has no question tool, so it waits for a person only where an extension of
yours opens a dialog (pi 1.0 and later). Oh My Pi's subagents run the
extension in sessions without the terminal: their turns do not change the
state, their approval requests do.

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
The one exception is the last message of a turn of an agent that
[another agent started](#agents-that-start-agents), held in memory for that
agent, or that a [project's lead](#projects) started, handed to the project
and written to its chat.
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

Oh My Pi's title is `π`, a state and the session: `>` at rest, a spinner at
work, `!` when it asks, which correct its reports as Claude Code's and
Codex's titles do.

Gemini CLI's title leads with `◇` when ready, `✦` while working (`⏲` while a
shell command of its own is silent) and `✋` when it needs an action, and
Neptune reads nothing else of it: working shows after two seconds, the other
two at once. With `ui.dynamicWindowTitle` or the title turned off in its
settings, it is listed and stays "Idle". OpenCode's and pi's titles name the
session and carry no state, so nothing corrects their reports: an OpenCode
whose plugin is off, or a pi turn that ends without its end being reported,
keeps the state last reported.

Alerts. Claude Code and Codex ask for attention themselves, through the
terminal, and so does Oh My Pi by default. OpenCode, Gemini CLI and pi do not unless their own notification
settings are turned on, so Neptune raises a
[terminal alert](notifications.md#coding-agents) for them from the state
above: when one comes to rest after working, or starts to wait for a person,
in a terminal that is not the focused one of a focused window. A request that
is dismissed, and an agent that exits, raise nothing.

Limits. With Codex's hooks untrusted, or a Codex without `--no-daemon`, the
title alone is used and states lag by about two seconds. An agent whose title
is disabled or replaced (`CLAUDE_CODE_DISABLE_TERMINAL_TITLE`, a Codex
`terminal_title` without the spinner) can stay "Working" after an interrupt, or
waiting after a refusal, until its next prompt. Answering one part of a
several-part question does not change the state. Background shells and
subagents that outlive a turn are not shown as work unless the title says so.
The limits in the first section (Windows, batch commands, bypassed
adapters) apply here too: those agents are not listed.

### Agents on SSH hosts

An agent you start in a terminal of an SSH workspace is listed like a local
one, under its workspace, with the host and directory in its tooltip. This
needs nothing of Neptune's on the host, and no port or connection besides the
terminal's own.

When the terminal connects, its bootstrap writes the adapters to
`${XDG_CACHE_HOME:-~/.cache}/neptune/agents` on the host, readable by you
alone: a launcher, a hook script, the settings file with Claude Code's hooks,
the plugin and the extension, and one `claude`, `codex`, `opencode`, `gemini`,
`pi` and `omp` command that runs the launcher. They are a few kilobytes of POSIX
shell and JavaScript, written once and replaced when a newer Neptune connects.
Zsh and Bash on the host put that directory first on the search path after
your own startup files have run; Bash is started with a startup file that
reads `/etc/profile` and the first of `~/.bash_profile`, `~/.bash_login` and
`~/.profile`, as a login shell does, though it is not one (`shopt login_shell`
is off and `~/.bash_logout` is not read). Other shells get the directory on
the path they inherit, which their login files may replace. A host where the
directory cannot be written gets its shell without adapters.

The launcher starts the real CLI with the same hooks, plugin or extension as
a local launch, and each reports by writing one line to the terminal:
`OSC 7717 ; neptune ; credential ; run ; …`. It says that a CLI opened or
closed, or names a hook's event with at most its tool name, notification
type, permission mode, source and trigger, and whether a subagent made it.
The hook script takes those words from the first 64 KB of the hook's input
and sends nothing else of it; the same rules as for a local hook then decide
the state. Titles travel the connection as they are, so they correct the
state as they do locally.

The credential is random per connection and is given to the host as an
argument of the bootstrap, so the host's process list shows it to the host's
other users while the shell starts. It keeps output from being taken for a
report: a file you print or a log you replay does not carry this connection's
credential. Someone who can read it and also put text on your terminal could
make a row show a wrong state; a report can do nothing else. Only an SSH
terminal accepts reports, and only for itself.

Limits. Tracking starts with terminals opened after this version: a
connection made before it has no adapters. Inside `tmux` or `screen` on the
host, reports and titles stop at the multiplexer unless it passes them
through, and a session started under an earlier connection has that
connection's credential, so its agents are not listed. Codex asks once per
host to trust the hooks, whose commands differ from the local ones. A report
is written to the terminal between the CLI's own writes; a CLI that was in
the middle of an escape sequence would show a few stray characters, which was
not seen with these CLIs. An SSH session you start yourself by typing `ssh`
in a local terminal is not an SSH workspace and gets no adapters. The host's
CLIs are found on its search path only, as locally.

## Implementation and source review

The pure model validates resume references and accepts generation-tagged
`PaneAgentChanged` commands. Persistence stores a reference on each pane. The
desktop startup workers install private adapters, and a single blocking loopback
listener receives bounded metadata messages. Each pane generation has a random
callback credential, and each CLI invocation has its own identity. Closed or
replaced panes and late hooks from exited invocations cannot overwrite a newer
session. A linked pull request is accepted only from the invocation that is open
in its pane, and reaches the model as a generation-tagged
`PanePullRequestLinked` command; an attached file takes the same path as a
`PaneFileAttached` command, and the tool server checks that the file exists
before it sends the path. A `PaneAgentChanged` that names another session ID
or CLI for an agent still open, or none, moves both lists in the model to a
bounded list of conversations under the ID they were made in, and one that
names an ID on that list moves them back; the bridge drops what the earlier
conversation had queued for a frame. Codex's hooks name a session with its
first prompt, so the bridge also takes the start of Codex's tool server,
between the turns of a conversation that had a prompt, for the end of that
conversation, and reports the agent without a session ID until the next
prompt names one. The state of a linked pull request stays out
of the model: `runtime/pull_requests.rs` owns one worker that runs `gh api
graphql` off the frame and wakes the application when a state changed. Pending updates coalesce per pane; hooks wake the application on change,
so idle integrations do not poll or repaint. Socket reads have byte and total-time
limits. Startup-file creation and cleanup stay on workers.

An SSH terminal has a credential and no channel: terminal-core passes a line
of `OSC 7717 ; neptune` on as an event without reading it, and the bridge
takes it only from the SSH terminal whose credential it carries, for the run
it opened. The listener never answers for such a terminal. The host's
adapters are one script, made from the same hook and batch-argument tables as
the local ones and evaluated by the bootstrap. An agent on a host is held
beside the model with its activity, so nothing about it is saved.

Activity uses the same channel and credentials. The hook process reduces an
event to one of a closed set of signals; the bridge accepts it only from the
invocation open in that pane, reduces signals to a state per pane, coalesces
it between frames and wakes the application when the state changes or a turn
begins or ends. The application holds the state outside the model, drops it
when the pane's generation, lifecycle or agent reference changes, and wakes
itself only for the two-second and 0.4-second deadlines above.

Starting an agent uses the same channel, credentials and startup path. The
tool server's requests are answered over the loopback connection; those that
change the workspace wait for the next frame, where a generation-tagged
`SpawnAgent` command adds a pane that is in no layout, so focus and view do
not move, and the model records which pane's agent started it. Only such a
pane may be outside its workspace's layout: focusing it gives it a tab, and
the model gives it one itself when its link ends while its agent runs, or
closes it when its own agent is gone. The saved layout leaves it out, and it
is restored out of view only together with its link. The task is held by the bridge and taken once
by the new terminal's shell startup, which opens the CLI with it the way a
restored agent is opened; it is never typed into a shell. A message for a
started agent is pasted only while its CLI is open in that pane generation,
and submitted a moment later under the same check. The model ends a link when
either pane loses its agent, and the bridge follows the model each frame. While
a started CLI has not taken its task the application compares its session's
revision twice a second, and reads its screen once when that has not changed
for four seconds; the bridge keeps the record of an ended agent whose CLI named
a conversation for as long as the agent that started it runs.
A wait polls the bridge from the tool server's own process and reads its input
on a second thread, so a cancelled call stops and nothing it had not yet
collected is lost.

A worktree is a field of its pane in the model: the repository, the folder,
the branch and the commit the branch stood at, validated like a resume
reference and opened by an `OpenWorktree` command that adds an ordinary tab
whose saved agent has no conversation yet, so the shell's startup opens the
CLI the way it reopens one without a session ID. One worker thread in
`runtime/worktrees.rs` runs every git command, so no frame waits for a
repository: the sheet's probe, `git worktree add`, the look at open worktrees
and removal. It sleeps while no worktree is open, compares each branch and its
targets with one `git for-each-ref` per look, works out a merge only when one
of them moved, and wakes the application only when an answer changes. Branch
names are checked before git sees them and never begin with a dash; paths and
branch names are not logged or sent to diagnostics.

A project is identity and membership in the model and nothing else there: a
project has a number, a folder key, a name, its workspace, its lead's CLI and
a paused flag, and a pane names the project it belongs to. Such a pane may be
outside the layout as a started agent's may, with the project answering for
it in place of a starting pane. The bridge resolves a caller's token to a
pane or to a project's lead and keeps a project's agents when its lead's
process is replaced. `runtime/project_lead.rs` owns each lead's process and
three threads (reader, writer, error tail) behind a provider-neutral event
type, with one pure driver per CLI tested against recorded exchanges; events
reach the frame through a bounded queue that blocks the CLI when full, and
streamed text wakes the window at most every 50 ms. `runtime/project_store.rs`
owns one worker for every project folder: appends, atomic replacement,
context files, the watch clock and removal. `app/projects.rs` joins them on
the frame without waiting on any of them, and holds user messages and agent
news in a bounded inbox until the lead is idle, so nothing is written into a
running turn. Pure rules for the inbox, the turn envelope, context, schedules
and the saved formats are in `src/projects/`.

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
back through inspection. Its `opencode` and `pi` fixtures need `node`: they
load the plugin and the extension named for the launch and hand them events.
A second instance opens an SSH workspace through a stand-in `ssh` that runs
the host's command on this machine with another home, so the bootstrap, the
installed adapters and the reports through the terminal are the real ones and
the network and a second machine are not. `app::ssh::tests` runs the host's
side on a PTY under Bash and Zsh without the application. `NEPTUNE_EXPLORER_STATE=agents`, `agents-empty` and
`agents-closed` of `app::tests::capture_explorer_native` capture the tab.

`python3 scripts/verify-agent-delegation.py` covers agents that start agents.
Its fixtures start Neptune's tool server as each provider is configured to and
call its tools: one agent starts the other on a named model in a terminal out
of view and reads its reply, sends it a message that arrives as one paste,
sees it wait for a person, closes it and reopens it with its conversation;
the list opens a started agent's terminal in a tab; started agents nest two
deep and reach only their own; a terminal a person closed is reported and
reopened; a fixture that asks before taking its task is reported with what it
shows by the call that started it and answered with the keys given; links
return after a close and reopen, out of view where they were, and no task or
reply is in the saved state.

`python3 scripts/verify-project.py` covers a project from its first message
to its removal in an isolated native app. One `claude` fixture plays both parts: as the lead it
speaks the stream protocol, starts Neptune's real tool server from the file
it was given and calls `spawn_agent` through it; as an agent in a terminal it
fires the injected hooks and answers its task. The script checks the creation
form, the lead's arguments, environment and credential file, the tool card
and roster row, an agent's report returning as a turn of the lead, an agent
that asks before its task under Needs you, pause, stop, what is and is not in
the saved window state, and removal. It goes on to shared context, watches
(with a stand-in `gh`), worktrees in a real temporary repository, a quit in
the middle of a turn and a relaunch, **New chat**, the narrow-window sheet, a
damaged and a later `project.json`, and idle frame counts; it last passed on
2026-10-06. Focused tests:

```sh
cargo test -p neptune-terminal --lib projects:: --locked
cargo test -p neptune-terminal --lib app::projects --locked
cargo test -p neptune-terminal --lib runtime::project_lead --locked
cargo test -p neptune-terminal --lib runtime::project_store --locked
```

`python3 scripts/verify-agent-worktree.py` covers agents in worktrees, in a
repository it makes: a branch named in the sheet becomes a tab in its own
worktree with the fixture CLI started there; a second agent gets another; a
merge is noticed and offered on the tab; removal deletes the folder and the
local branch of the merged one only; uncommitted work is named and cancelling
keeps it; and the worktree returns with its agent after a close and reopen.
`cargo test -p neptune-terminal --lib worktree --locked` runs the git
operations (merge, squash, removal) and the application flow against
temporary repositories.

`python3 scripts/verify-agent-attachments.py` covers attached files the same
way: fixtures attach files through each provider's tool server by absolute
and relative path, a folder and a missing file are refused, the toolbar and
each tab count their own terminal's files, the list reveals a file and opens
another through stand-in launchers, a picture opens at full size and steps to
its neighbour by key and by control, files are removed one at a time and all
at once, and the lists return after a close and reopen, are empty for a new
conversation in the same run, return with an earlier one in the same run or
a later one, and leave with their agent.

`python3 scripts/verify-agent-restore.py` runs deterministic CLI fixtures in an
isolated native app, with fresh storage and a unique inspection endpoint. It
checks independent IDs in one directory, graceful close/reopen, normal exit,
Ctrl+C and narrow-window presentation. Fixtures prove Neptune's contracts without
sending model requests; they do not stand in for installed-provider validation.
Build the inspection targets described in [native verification](verification.md#native-application-verification) first.

`python3 scripts/verify-pull-request-status.py` links pull requests through the
same fixtures, with a stand-in `gh` that answers from a file the script
rewrites. It checks that the real worker asks once per round for the pull
requests in view, reads a change of state without input, and leaves captures of
each state alone in view, on tabs, in the list, in the Light theme and in a
narrow window for review. The stand-in proves Neptune's request and its reading
of the answer, not GitHub's API or an installed `gh`.

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

Agents that start agents were checked on 2026-10-04 in the Linux development
build with `inspection` (Wayland session, XWayland window). The deterministic
native regression passed, with captures reviewed at 1100×700 and 640×440, and
the restore and activity regressions passed again. With the installed **Claude
Code 2.1.289** and **Codex CLI 0.160.0** in an isolated instance at 1200×760,
Claude Code (Haiku, permissions bypassed) started Codex on a named model out
of view and read its reply, closed it, reopened it and got an answer that
recalled the first; started a Claude Code on `haiku`, whose terminal was then
opened from the list and closed by hand, and reopened that one too with its
conversation. A Claude Code started in a folder it had not seen stopped at its
trust question: the `spawn_agent` call that started it returned the
question's text, the starting agent put it to the user and pressed nothing, the count
turned to the attention colour, and after being told "No, exit"
`press_agent_keys` pressed enter and the wait reported the exit. Codex took
its task in a new directory without asking. In a later run that day a
Claude Code started with `model` sonnet, `effort` low and `ultra` showed
"Sonnet with low effort" and its ultracode mark, and a Codex started with
`ultra` showed its model at ultra. That Codex opened on "Hooks need review":
the installed Neptune 0.1.0-rc.3 had been used in between and had saved trust
for its own form of the session-start hook command. The `spawn_agent` call
returned the review screen's text and the starting agent put it to the user. From an earlier build the same day:
both directions of a two-turn exchange, a Codex-made git worktree in which
Claude Code wrote the file it was asked for, a late reply reaching the next
wait, a 90-second wait without a tool timeout, and Claude Code asking before
`spawn_agent` in its default permission mode. Not exercised with the installed
CLIs: restore, a started agent's permission request, Codex as the starting
agent in this version, Codex asking before a tool, and Codex with Neptune's
hooks untrusted; fixtures and unit tests cover the first two. macOS, Windows and native Wayland are unverified.

Agents in worktrees were checked on 2026-10-05 in the Linux development build
with `inspection` (Wayland session, XWayland window) and git 2.53.0.
The deterministic native regression passed, with captures reviewed at
1100×700 and 640×440: the palette command, the sheet empty, with a typed name
and with its list of worktrees, tabs named by their branches, **Merged** on a
tab and on the toolbar of a terminal alone in view, both removal sheets, the
restored workspace and the workspace closing with its last worktree. The CLIs
were fixtures: the installed Claude Code and Codex were not started in a
worktree, though a worktree's CLI is opened by the same shell startup path
that reopens an agent without a session ID. Squash and rebase detection, a
branch that already existed and removal after a folder was deleted by hand
were exercised in unit tests against temporary repositories, not natively.
macOS, Windows and native Wayland are unverified, as is Command+G on macOS.

More agents and SSH workspaces were checked on 2026-10-05 in the Linux
development build with `inspection` (Wayland session, XWayland window), with
captures reviewed at 1100×700 and 640×440. The deterministic native
regression passed: OpenCode through its plugin (turns, a subagent's session,
a permission, a question, "busy" repeated up to the instant of "idle", the
session ID in the saved state), pi and Oh My Pi through the extension
(for Oh My Pi its approvals, `ask` tool, a subagent, a loop it continues and
its title), Gemini CLI through its title, and Claude Code, Codex, pi and
Gemini CLI in an SSH workspace whose host was this machine behind a stand-in
client. In one run the window was not the focused one and the alert Neptune
raises for Gemini CLI showed on its workspace row and the toolbar bell.

With the installed CLIs in an isolated instance at 1500×900, **Gemini CLI
0.62.0**, **pi 1.0.2**, **Oh My Pi 17.3.5** and **OpenCode 1.18.34** were
each listed when started, Gemini CLI at its folder trust question and pi and
Oh My Pi with the extension loaded. OpenCode, the one signed in, ran a turn:
its row went from idle to working and back, took the session's name, and its
session ID was saved. Before the plugin reported changes in order, the same
turn had left the row on "Working": OpenCode says "busy" again 2 ms before
"idle". In a terminal about ten columns wide OpenCode crashed inside Bun and
its row left the list; whether it does so without Neptune's plugin was not
checked. Not exercised: a turn of Gemini CLI, pi or Oh My Pi (none was
signed in; Gemini CLI's working and action-required titles and Oh My Pi's
working and asking titles are read from their sources), a real SSH
connection to another machine, `tmux` on a host, other login shells than
Bash and Zsh, the desktop banner of an alert, macOS and Windows.

Attached files were checked on 2026-10-05 in the Linux development build with
`inspection` (Wayland session, XWayland window). The deterministic native
regression passed, with captures of the toolbar count, the list alone and in
a split, the full view and its neighbour reviewed at 1100×700, and the list
and the full view at 640×440. Focused model, persistence, runtime, widget and
application tests, Clippy for the model and the desktop library and the
architecture boundary check passed. Not exercised: the installed Claude Code
and Codex calling `attach_file` from its instructions alone, a real file
manager and default application (the regression records what would be
launched), macOS, Windows and native Wayland.

**View all pictures** and the band of small copies were checked on 2026-10-06
in the same build and session. The native regression passed, with captures
reviewed of the list's entry, the full view opened from it and after a click
on another copy at 1100×700, and of fifteen pictures at 640×440 with the band
moved to its newest and its oldest. Focused widget and application tests and
Clippy for the desktop library passed. Not exercised: a real pointer on the
band, macOS, Windows and native Wayland.

Links that follow the conversation were checked on 2026-10-06 in the Linux
development build with `inspection` (Wayland session, XWayland window). The
deterministic native regression passed, with captures reviewed of a tab
emptied by a new conversation and filled again by the earlier one at
1100×700. With **Claude Code 2.1.292**, `/clear` emptied a tab of one file
and one pull request and `/resume` of that session brought both back at
once. With **Codex CLI 0.160.1**, `/new` emptied the tab at once and
`/resume` of the earlier session brought both back with the first prompt
sent in it. Focused model, persistence and runtime tests passed. Not
exercised: Codex with untrusted hooks, a conversation resumed in another
terminal with an installed CLI, macOS, Windows and native Wayland.

Projects were checked on 2026-10-05 and 2026-10-06 in the Linux development
build with `inspection` (Wayland session, XWayland window), with a fixture
lead, with the installed **Claude Code 2.1.290** and **2.1.291** as lead and
as agent, and for one turn with **Codex CLI 0.160.1** as lead. What was and
was not seen running is recorded under
[projects verification](verification.md#projects-verification-2026-10-05):
in short, creating a project, turns, agents with worktrees of their own,
reports returning, an agent asking before its task, shared context, watches
read through a stand-in `gh`, pause, stop, a restart with a resumed lead, the
narrow-window sheet and removal were; lead failures, a real `gh`, desktop
banners and a Codex lead's tool calls were not.
