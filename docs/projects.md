# Projects

A project puts one coding agent in charge of a piece of work in a workspace.
That agent is the **lead**. You talk to it in the **Project** tab of the right
panel; it plans the work, starts other agents in terminals of their own, is
told when they finish or need someone, and reports back to you. The agents it
starts are ordinary Claude Code or Codex sessions in ordinary terminals: you
can open any of them, read along and type.

[User guide](usage.md#projects) · [Agent sessions](agent-sessions.md#projects) ·
[What was verified](verification.md#projects-verification-2026-10-05)

- [Requirements](#requirements)
- [Start a project](#start-a-project)
- [Talking to the lead](#talking-to-the-lead)
- [Settings](#settings)
- [Agents](#agents)
- [Needs you](#needs-you)
- [Shared context](#shared-context)
- [Watches](#watches)
- [Pause, stop and remove](#pause-stop-and-remove)
- [What the lead can and cannot do](#what-the-lead-can-and-cannot-do)
- [Limits](#limits)
- [What is stored, and how to delete it](#what-is-stored-and-how-to-delete-it)
- [After a restart](#after-a-restart)
- [Troubleshooting](#troubleshooting)

## Requirements

- **Claude Code or Codex, installed and signed in.** The lead is your own
  installed CLI, run by Neptune without a terminal. Neptune looks for `claude`
  and `codex` on `PATH`, in the usual per-user install folders and, failing
  those, through your login shell. It holds no account or key of its own.
- **Linux or macOS.** On Windows the tab says "Projects are not available on
  this system yet", because the adapters that connect an agent to Neptune are
  Unix-only. Linux under X11 is where projects were checked in the running
  application; macOS and native Wayland are
  [unverified](verification.md#projects-verification-2026-10-05), and so is a
  Codex lead beyond recorded exchanges.
- **A workspace on this computer.** A workspace connected over SSH shows
  "Projects run on this computer", and a workspace that has a project cannot
  be connected over SSH until the project is removed.
- `git` on `PATH` for agents in [worktrees](#worktrees), and the
  [GitHub CLI](agent-sessions.md#pull-request-status) signed in for pull
  request watches and status.

A workspace has at most one project, and a window at most eight.

## Start a project

1. Open the right panel on **Project**: click the tab, run **New project
   here** or **Show project** from the command palette, or bind
   [`show-project`](usage.md#custom-keybindings).
2. Type what you want done in the field under "New project". The folder
   named over the field is the one the terminal in front is in, as its shell
   last reported it, and it follows that terminal while you `cd`; it is the
   folder the panel's Changes and Files tabs show. A terminal whose shell
   reports no directory leaves the workspace's own.
3. Press Enter, or the arrow in the field.

The line under the field names the lead, such as "Claude Code ›". That is
all most projects need. Pressing it unfolds the lead's options: which CLI
leads, where both are installed, and its **Model** and **Effort**. Left at
"Default", the CLI's own settings apply; both can be changed later in the
project's [settings](#settings), the CLI cannot.

Neptune names the project after the workspace, makes its folder, and sends
what you typed as the first message. **The project's directory is settled
then and does not change**: it is where its agents start unless the lead
names a directory inside it, where worktrees are made from, and what the lead
is told. It no longer follows the terminal or the workspace. The tab shows
it beside the project's name, and **Reveal** beside "Folder" in the
project's [settings](#settings) opens it. The lead's process starts then, not
before; "Starting Claude Code…" shows under the chat until it answers.
**Rename…** in the project's "⋯" menu changes the name.

The lead is asked to propose a plan and wait for your go before it starts the
first agents of a new goal, and to treat a message that names what to start
("launch an agent that checks the tech stack") as that go. That is an instruction to the lead, not something
Neptune enforces; the [limits](#limits) are what is enforced.

If a project worked in the directory the form names and its workspace was
closed, the form lists it under "Earlier in this folder" with **Reopen**,
which brings it back with its chat, context and settings. It goes on in that
same directory.

In a window too narrow for the right panel (below 460 points, which a 640
pixel window reaches at 150% zoom), the same commands open the project in a
sheet over the terminals instead, whose title row is the project's name and
controls.

### What the tab shows

A project is a conversation, and the tab shows little else:

- **Its name and folder**, with two controls: **Project details** and the
  "⋯" menu.
- **What needs you**, only while something does; see [Needs you](#needs-you).
- **A strip of its agents**, once it has any: one row with a pill for each,
  pressed like a tab; see [Agents](#agents).
- **The chat** and the field you write in.
- **Who leads**: the lead's CLI with its model and effort under the field,
  which opens the settings when pressed.

**Project details** replaces the chat with three parts, **Context**,
**Watches** and **Settings**, and opens on the one you last looked at. The
chevron beside them, or the same control again, returns to the chat.

## Talking to the lead

Type in "Message the lead…". Enter sends and Shift+Enter starts a new line.
Escape gives the keyboard back to the terminal and keeps what you typed.

While the lead works, the line under the field reads "Lead at work" with the
time taken, and the lead's text appears as it is written. A message sent then
is held ("Queued · 1") and delivered when the turn ends: Neptune never writes
into a running turn. With the field empty, the send control becomes **Stop**,
which ends the turn and leaves the lead ready for the next message.

The chat shows more than the two of you:

| Row | Written by | Says |
| --- | --- | --- |
| Your messages and the lead's replies | You and the lead | The conversation: yours in a filled bubble at the trailing edge, the lead's as plain text with its Markdown drawn: headings, nested and task lists, quotes, tables, rules and code. The copy control at the end of a reply copies it as it was written. |
| "Worked for 4m 52s ›" | Neptune | The work of a turn the lead finished: what it said on the way to its reply and what Neptune did for it, folded so that the reply is read alone. Press the row to unfold the work in place, in the order it happened, and again to fold it. The row is absent where a turn was its reply and nothing else. |
| Tool card, such as "Started agent 12 · Claude Code · auth-refactor" | Neptune | What Neptune actually did for the lead: started, messaged, closed or reopened an agent, recorded a decision, added a watch. It is part of its turn's work, and folded with it once the turn has ended. A request Neptune refused says so, with the reason where there is one, and is never folded. |
| Agent row, such as "“auth-refactor” finished its turn" | Neptune | An agent's news, with the first two lines of its last message and **Open terminal** and **Changes**. Press the headline of a longer report to unfold all of it in place, and again to fold it. |
| Watch, pull request and worktree rows | Neptune | A watch that fired, a pull request whose state changed, a worktree whose branch was merged. |
| Notice | Neptune | Things such as "You stopped the lead." or a failed turn. |

While a turn runs, everything of it is shown as it comes. It folds when the
turn ends with a reply: the reply is the last thing the lead wrote in the
turn, and the copy control copies that alone. A turn that you stopped, that
failed, that Neptune closed in the middle of, or that ended without a word
from the lead has no reply to set apart, so all of it stays in view. An
agent's news, a proposed watch and a notice are never folded, whenever they
arrive. Which turns you unfolded is not kept when Neptune restarts.

The cards come from what Neptune executed, not from what the lead says it
did, so a mismatch between the two is visible. A link in a reply or an
unfolded report opens when you press it, and shows where it leads under
the pointer first: a web address in your browser, a file given by its full
path with its application. Nothing else a text names is opened, and a
picture is shown as a link, never loaded. When agents have news, Neptune
gathers it for a moment (two seconds of quiet, ten at most) and gives it to
the lead as one turn, without a message from you.

"⋯" → **New chat** starts the lead on a fresh conversation. The earlier chat
is kept as one file with the project's saved files, and the lead is handed the shared
context and the list of agents again.

### Attaching files

A message can carry up to 10 files, with or without words. There are three
ways to add them:

- the **paperclip** in the field opens the file picker, for one file or several;
- **drop** files from a file manager or another application on the chat,
  which is highlighted and says "Drop to attach for the lead" while they are
  held over it. On macOS and Windows, where Neptune is not told where files
  are held, they go to the message while its field has the keyboard and to
  the focused terminal otherwise;
- **paste** while the field has the keyboard and the clipboard holds a
  picture, such as a screenshot. Neptune saves it as a PNG among its
  [pasted pictures](usage.md#keyboard-shortcuts), where it keeps the 32 most recent. Text on the clipboard is
  pasted as text.

Each file waits as a chip over the field, with a control that takes it off
again, and is sent with the message. In the chat the chips stay under your
words; click one to open the file with its default application.

What the lead gets depends on the file:

| File | The lead |
| --- | --- |
| A PNG, JPEG, GIF or WebP picture of at most 3.75 MB | Sees the picture, and is told its path |
| Any other file | Is told its path, kind and size. It cannot open the file: when what the file holds matters, it gives the path to an agent, which can |

Files are not copied: the lead and its agents are given the path where the
file is, so a file you move or delete before an agent reads it is gone for
them too, and the chat's chip then opens nothing. A picture that cannot be
shown when its turn begins, because it has gone, is not a picture after all,
or would make more than 15 MB of pictures in one turn, is left out, and the
lead is told so. A larger picture, a folder and a file that cannot be read
are refused when you attach them, with the reason. A picture you attach is
sent to the lead's provider as part of the conversation, like your words.

## Settings

**Project details** → **Settings** holds what the project's lead and the agents it
starts run with. Everything there is kept with the project and survives a
restart.

**Lead.** A model and an effort for the lead's own CLI; "Default" leaves each
to the CLI. Models with a name in the list are offered, and any other model
id is typed after choosing **Custom…** and applied with Enter; a text no CLI
would take as a model name is said to be none and not used. A Claude Code lead takes the efforts low, medium, high,
xhigh and max; a Codex lead minimal, low, medium, high, xhigh and max. Neither
is offered an ultra mode: Codex's Ultra effort is reasoning with the CLI's own
automatic delegation and Claude Code's ultracode is its own multi-agent
orchestration, while a lead has no tools but Neptune's and delegates through
them, where you see every agent. The lead's CLI itself stays the one the
project was made with.

A change is used from the lead's next turn and keeps the conversation. A
turn that is running is never interrupted for it. Before the next turn the
lead's process is let go and started again in the same conversation with the
new model and effort (Claude Code with `--resume`, Codex by resuming its
thread, which is also told the model and effort with every turn), so that
turn takes a few seconds longer to begin; until then the card reads
"Changed. The lead takes this up with its next turn." The line under the
chat's field names the model you chose, or where you chose none the model
the lead's CLI reported the last time it started.

**Agents, for each CLI.** A model and an effort for the Claude Code agents
the lead starts and for the Codex ones, and for Claude Code agents whether
ultracode is on. Each is "Lead decides" until you set it. A value you set is
used for every agent of that CLI the lead starts from then on and **wins over
what the lead asks for**; a value left at "Lead decides" is the lead's to
choose for each agent, with `spawn_agent`'s `model`, `effort` and `ultra`.
Codex's Ultra is one of its effort levels. Ultracode and Ultra use far more
tokens. The lead is told what is set at the head of every turn, and the
answer to `spawn_agent` says what the agent was actually started with.
Agents that already run keep what they were started with, and so does one
the lead reopens.

You can also simply ask the lead in the chat, for example for "a Claude Code
agent with ultracode" or "a Codex agent at Ultra effort".

**Project.** Its name, with **Rename…**, and its folder, the directory it
works in, with **Reveal**.

## Agents

The lead starts agents with Neptune's tools. Each is a Claude Code or Codex
session in a terminal **out of view**: it has no tab until you open it, and
the keyboard stays where it was. A strip of one row over the chat has a
**pill** for each: a mark for its state, its number and its name. Agents that
need you come first, in the tint of what needs you, then those at work, then
the rest. The strip stays one row however many agents there are: pills
narrow down to a mark and a number, and past that the strip scrolls
sideways, fading at the edge that has more. Rest the pointer on a pill to
read its CLI, its branch, what it is doing and for how long.

The control at the strip's end, with the count of agents, opens the **list
of all of them** in a popover over the chat; the chat does not move or
shrink. Its first line says how they stand, such as "2 working · 1 ready for
review", and so does the control's tooltip. Escape, a click outside it, a
second click on the control or opening an agent closes it. **Show project
agents** in the command palette opens it too. An agent is in one of six
states:

| State | Means |
| --- | --- |
| Starting | Its CLI has not taken its task yet |
| Working | A turn is running |
| Needs you | It waits for a person; see [Needs you](#needs-you) |
| Ready for review | Its turn ended with a report you have not looked at |
| Idle | At rest |
| Ended | Its CLI exited or its terminal closed |

A row of the list is two lines: the agent's mark, number, name and state,
then its CLI, the branch when it has a worktree, and its linked pull requests
with their [status](agent-sessions.md#pull-request-status), which open their
pages. Click a pill, or a row of the list, to open the agent's terminal: it
becomes a tab, takes the keyboard and stays a tab until you send it back.
The pill of an agent whose terminal is a tab is filled, as the tab in view
is. "Ready for review" clears once that terminal is the one in front.
Project agents are also rows of the panel's [Agents tab](usage.md#agents).

To get an agent's tab out of the way without stopping the agent, choose
**Send to background** from the tab's menu or from a secondary click on the
agent's pill, or run **Send terminal to background** from the command palette.
The tab goes, the agent runs on out of view, and its pill stays as it was
before you opened it. Closing the tab instead ends the agent. The last
terminal in view in a workspace cannot be sent to the background.

Under an agent's report in the chat:

- **Open terminal** shows its terminal.
- **Changes** shows its terminal and the panel's Changes tab for its folder.
- **Fix CI** appears when a pull request it linked has failing checks, and
  **Address comments** when one has unresolved review conversations. Each
  sends the agent a fixed request and adds a card saying that you asked.
  The newest card about a watched pull request offers the same two while
  the agent that linked it is at rest.

An agent asks for permissions in its own terminal, under its own permission
settings. The lead cannot answer for it.

### Worktrees

When two agents would edit the same files, the lead gives each a git worktree
by naming a branch. Neptune creates it the way
[New agent in worktree](agent-sessions.md#agents-in-worktrees) does, in
`<repository>.worktrees/<branch>`, and starts the agent there. Removing a
worktree stays with you: closing an agent or removing a project leaves
worktrees and branches on disk, for **Remove worktree** or the **Merged**
chip to clean up.

### Closing

When the lead closes an agent whose terminal is out of view, never opened or
sent back to the background, its terminal closes. If you have it open as a
tab, the terminal stays open and the agent only leaves the project. An agent whose CLI exits leaves the project too. The lead can reopen
a closed agent with the conversation it had.

## Needs you

Rows pinned over the chat say what only you can clear, each with the one
thing to do about it. They are there only while something waits. While there
are any, the Project tab shows their count, and the panel's toolbar
button carries a dot when the tab is not in view. Both follow the project of
the workspace in view. Whichever workspace it is in, a project that starts to
need you raises one [desktop banner](notifications.md#coding-agents) named
after it, unless you are looking at its tab.

| Row | Cause | What to do |
| --- | --- | --- |
| An agent, with "Needs permission", "Asked a question", "Plan needs approval", "Needs input" or "Needs your answer" | The agent's CLI waits for a person | **Open** its terminal and answer there |
| The lead needs you | The lead could not start or go on; see [Troubleshooting](#troubleshooting) | Fix the cause, then **Try again** |
| Paused after 20 turns without you / after 30 turns in an hour | Neptune paused the project so it does not run on by itself | Look at the chat, then **Resume** |
| This project is read-only | Its saved files are from a newer Neptune or could not be read | See [Troubleshooting](#troubleshooting) |
| Not everything is saved | Neptune could not write the project's saved files | Check that the disk has room |

The lead is told when an agent waits for a person, and its instructions are
to tell you and stop.

## Shared context

**Project details** → **Context** shows what the project keeps between conversations:
a few Markdown files in the project's folder, outside your repository, so
every worktree sees the same copy.

| File | Written by | Rule |
| --- | --- | --- |
| `INSTRUCTIONS.md` | You, in the field at the top of Context | How you want agents to work here. At most 16 KB. **Save** and **Revert** appear once you change it. |
| `DECISIONS.md` | The lead, through `record_decision` | Dated entries, only added to. Past 64 KB the oldest move to `decisions-archive.md`. |
| `STATUS.md` | The lead | A summary of at most 8 KB that it replaces as work moves. The version before is kept as `STATUS.prev.md`; one replaced within two minutes counts as a draft and is not. |
| `notes/<topic>.md` | The lead and its agents | Entries only added to, each signed with who wrote it. 64 KB a file. |
| `INDEX.md` | Neptune | A generated list of the files above. |

The folder holds at most 64 files and 4 MB. Entry times are UTC.

Who is handed what:

- **A new agent** gets your instructions, the newest decisions, the top of
  the status, the index and the folder's path before its task. Files are read
  from disk when the brief is built, so an edit you made outside Neptune is
  in the next brief.
- **A later message to an agent** is preceded by the decisions recorded since
  it was last told.
- **The lead** gets everything on a new conversation and after its CLI
  compacts the conversation, and otherwise only what changed.
- **Agents that a project agent starts itself** get none of it.

Each file's row opens it in its default application; its "⋯" menu has
**Reveal in file manager** and **Delete…**, and **Reveal context folder**
under the list shows where they all are. Notes are what agents observed,
not checked facts, and the brief says so.

Two limits of these rules. Neptune's tools enforce who writes what, but an
agent is given the folder's path and has file tools of its own, so nothing
stops it from editing a file there directly; its brief tells it not to. And
a brief is part of the command line that starts the agent's CLI, like any
task [one agent gives another](agent-sessions.md#agents-that-start-agents),
so your instructions and recent decisions are visible in this computer's
process list while that agent starts.

## Watches

A watch wakes the lead when something happens, with an instruction for what
to do then. They are under **Project details** → **Watches**.

| Watch | Fires | Notes |
| --- | --- | --- |
| Agent updates | When an agent finishes, needs you or ends | Always on; not listed among the watches |
| Schedule | Every N minutes, from 15 minutes to a week | One the lead proposes does nothing until you press **Allow** on its card or row |
| Pull request | When its checks start failing or pass, when it is merged or closed, and when more review conversations are unresolved | Added automatically for each pull request a project agent links; ends itself once the pull request is merged or closed |

**Watches run only while Neptune is open.** Nothing runs in the background
after you quit. A schedule that came due while Neptune was closed runs once,
about 30 seconds after the next launch, and says how many runs it missed; it
never runs early and missed runs are not made up one by one. A pull request
that changed while Neptune was closed is reported once.

A row's "⋯" menu has **Pause** (or **Resume**), **Run now** and
**Delete…**; a watch the lead proposed has **Allow** and **Decline** in its
row instead. **Add watch** makes one yourself: a name, how often it runs
(from every 15 minutes to every week, or **Custom…** for a number of
minutes) or the address of a pull request, and what the lead should do. A run that is due while the previous one is still waiting or
running is skipped, not stacked. A row says how long until its next run and
how long ago its last one was; the card of a run in the chat gives its due
time in UTC. A pull request
event tells the lead states and counts only, never titles or comment text.
Reading pull requests uses `gh` as described under
[pull request status](agent-sessions.md#pull-request-status); without it a
pull request watch never fires.

## Pause, stop and remove

| Action | Where | Effect |
| --- | --- | --- |
| **Pause project** | The "⋯" menu, or the command palette | Holds what happens by itself: turns about agent news, watch runs, and the lead starting, messaging or reopening agents or adding watches. Your own messages still reach the lead, and agents finish the turn they are in. Saved across restarts. |
| **Resume project** | The same places, **Resume** on the "Paused" row over the chat, or **Resume** on an automatic pause | Releases what was held |
| **Stop** | The send control while the lead works and the field is empty | Ends the lead's turn. Agents are not stopped. |
| Close an agent | Its tab's close control once opened, or ask the lead | Ends that agent |
| **Remove project…** | The "⋯" menu | After a confirmation: the lead stops and the project's saved files are deleted with its chat, context and watches. Its agents keep running and each gets a tab. Worktrees, branches and what the CLIs keep of their own conversations are not touched. |

Closing a workspace takes its project out of the window and keeps its
saved files; the creation form offers it again while the terminal in
front is in the directory the project worked in.

## What the lead can and cannot do

The lead works only through thirteen tools Neptune gives it:

| It can | With |
| --- | --- |
| Start an agent with a task, in the project's directory or a worktree, on a model, at an effort and in its ultra mode where it names them and your [settings](#settings) leave them open | `spawn_agent` |
| Message, list, close and reopen its agents, and read an agent's last report | `send_agent_message`, `list_agents`, `close_agent`, `reopen_agent`, `agent_report` |
| Read the shared context, record decisions, replace the status, add notes | `read_context`, `record_decision`, `write_context` |
| Add, list and remove watches, and ask how a pull request stands | `add_subscription`, `list_subscriptions`, `remove_subscription`, `pull_request_status` |

It cannot:

- read or edit your files, or run commands. It sees a picture you
  [attach](#attaching-files) to a message, and nothing else of a file but its
  path. A Claude Code lead runs with no
  built-in tools, none of your settings, hooks or other tool servers, and a
  permission mode that refuses whatever is not already allowed. A Codex lead
  runs in a read-only sandbox without network access, with approvals set to
  never, web search off, and your other Codex tool servers turned off for it;
- answer an agent's permission request, or type into an agent's terminal
  while it waits for a person;
- start work in a directory outside the project's;
- write your instructions, or rewrite or delete recorded decisions;
- turn on a schedule it proposed; that needs your **Allow**;
- remove a worktree, or remove the project.

The lead runs in the project's own folder, not in your repository, so the
repository's `CLAUDE.md` or `AGENTS.md` does not reach it; the agents it
starts work in the repository and read them as usual. What an agent reports
is handed to the lead marked as another program's output, not as your
instruction. These are instructions and settings for a language model, and a
model can still describe things it did not do, which is why the cards are
written by Neptune.

A lead is your CLI talking to its provider, as in a terminal: its
conversation goes to that provider under your account, and each agent it
starts is a full session of its own. The "⋯" menu shows a **Usage estimate**
for a Claude Code lead's recent turns, as Claude Code reports it; a Codex lead
reports none, and agents' usage is not counted.

## Limits

| Limit | Value |
| --- | --- |
| Projects in a window | 8, one per workspace |
| Agents a project has open at once | 6, within the 16 terminals of a workspace |
| Agents a project starts in a day | 40 |
| Agents a project agent can start itself | Allowed; those cannot start more |
| Lead processes running at once | 3; a lead at rest for 30 minutes with no agents is let go and resumed when needed |
| Turns the lead takes by itself | 30 in an hour, or 20 in a row without a message from you, then the project pauses |
| A message to the lead | 32 KB, and 10 [attached files](#attaching-files) |
| A picture shown to the lead | 3.75 MB; 15 MB of pictures in one turn |
| An agent's report as handed to the lead | 6 KB; the lead can ask for the whole of it |
| Watches in a project | 16 |
| Runs of one schedule in a day | 12 |
| Wakes for one pull request in a day | 10 |
| Shortest and longest schedule | 15 minutes, one week |
| Chat kept in view | The newest 400 entries; **Load earlier** reads more |

## What is stored, and how to delete it

Other agent features of Neptune keep conversations in memory only. A project
is the exception: its chat and context are written to disk so that they
survive a restart.

| | |
| --- | --- |
| **Where** | One folder per project, `projects/<key>/` in Neptune's [data directory](usage.md#storage-and-restoration), readable only by your account. **Reveal saved files** in the "⋯" menu shows it. This is not the project's folder, which is the directory it works in. |
| **`chat.jsonl`** | Your messages with the path and size of each file attached to them (never what a file holds), the lead's finished replies, the tool cards, agents' reports as they were handed to the lead, watch and pull request rows, notices. At 4 MB it becomes `chat.1.jsonl`, replacing the one before, and a new file begins. **New chat** does the same. |
| **`project.json`** | The project's name and the directory it works in, your [settings](#settings) for its lead and its agents, the lead's conversation ID, a record of up to 64 agents (title, CLI, conversation ID, directory, worktree, pull request addresses, state, last report up to 8 KB, and the model, effort and ultracode it was started with), the watches with their instructions, and the day's counts. |
| **`context/`** | The [shared context](#shared-context) files. |
| **`workspaces.json`** | Only the project's identity: its number, folder key, name, workspace, lead CLI and paused flag, and which terminals are its agents. No text of the chat, tasks, watches or context. |
| **Never stored** | Terminal contents; the lines of a question an agent asks before taking its task (they go to the lead and are not written down); credentials; text the lead was still writing; the lead's reasoning. |
| **Diagnostics** | The operation, the project's number and a fixed kind of failure. Never text, titles, paths or tokens. |
| **Leaves the computer** | Nothing through Neptune. The lead and the agents are your CLIs: they send their conversations to their providers and keep their own transcripts, as in any terminal. |

To delete:

- **Remove project…** deletes the whole folder.
- **New chat** sets the current chat aside as `chat.1.jsonl`; delete that
  file among the project's saved files to be rid of it.
- **Delete…** on a context file's row deletes that file.
- A project whose workspace you closed keeps its folder. Reopen it and remove
  it, or delete `projects/<key>/` yourself while Neptune is closed.
- The CLIs' own transcripts are theirs: see Claude Code's and Codex's own
  storage.

Neptune protects these files the way it protects saved workspaces. A
`project.json` from a newer version, or one that cannot be read, makes the
project read-only instead of being replaced; a damaged one is copied to
`project-recovery-….json` before the next save replaces it; a `chat.jsonl`
this version does not recognise is never appended to. One project's trouble
does not stop another's saving. Screenshot launches write nothing.

## After a restart

- The project, its paused flag and which terminals are its agents come back
  with the workspace; its directory and settings are read from its folder.
  A project whose folder does not say where it worked, such as one whose
  saved state was damaged, goes on in its workspace's directory. Agents are reopened with their conversations like any
  [restored agent](agent-sessions.md).
- The lead is not started at launch. It resumes its conversation the next
  time there is something to send: your message, or news that had not
  reached it, which is delivered once and marked as from before the restart.
- A reply the lead was writing when Neptune closed is lost; the chat says
  "Neptune closed while the lead was replying." A message of yours the lead
  had not taken is not sent again by itself; the chat says so. Its attached
  files are still listed under it.
- Due schedules run once, as [above](#watches).
- The turn counts behind the automatic pause start again from zero.
  "Ready for review" is not kept; such an agent reads "Idle".

## Troubleshooting

Each of these appears as a row over the chat that needs you, a notice in the chat, or both.

| What you see | Cause | What to do |
| --- | --- | --- |
| "No lead installed" under the field, which starts nothing | Neither `claude` nor `codex` was found | Install one and check that it runs in a terminal. The form looks again within a minute. |
| "Claude Code isn't installed or isn't on PATH." (or Codex) | The lead's CLI is gone or not where Neptune looks | Install it or fix `PATH`, then **Try again** |
| "Claude Code isn't signed in." / "Codex isn't signed in." | The CLI has no account | Run `claude` and sign in, or `codex login`, in a terminal, then **Try again** |
| "… could not reach Neptune's tools, so the lead was not started." | The CLI did not connect to Neptune's tool server | **Try again**. If it persists, update the CLI. |
| "…'s usage limit was reached." | The provider's limit for your account | Wait: with a known reset time the lead goes on by itself; otherwise **Try again** later. Messages wait meanwhile. |
| "…'s service is overloaded right now." | The provider refused the turn | Send your message again in a moment |
| "The conversation has grown too long for the lead to continue." | The lead's conversation no longer fits its model | "⋯" → **New chat**. The shared context carries over. |
| "The lead's earlier conversation is no longer on this computer. It starts a new one." | The CLI's own transcript was deleted | Nothing; the lead is handed the shared context again |
| "… stopped unexpectedly." / "Neptune and this version of … did not understand each other." | The CLI exited, or speaks a protocol this Neptune does not know | **Try again**. If it repeats, update Neptune or the CLI; the notice quotes what the CLI printed. |
| "The lead stopped before it finished its turn." | The lead's process ended mid-turn | Send the message again |
| "Paused after 20 turns without you" / "… 30 turns in an hour" | The automatic pause | Read what happened, then **Resume** |
| "This project is read-only" | `project.json` or `chat.jsonl` was saved by a newer Neptune, or could not be read | Update Neptune, or use **Reveal saved files** to look at the files. Nothing in the folder is changed meanwhile, and its watches are off. |
| "Not everything is saved" | A write to the project's saved files failed | Free disk space or fix the folder's permissions. The row stays until Neptune restarts. |
| "Waiting for another project's lead: three run at a time." | Three other leads are busy | Wait for one to finish its turn |
| An agent stays "Needs you" | Its CLI asks something: folder trust, a permission, a question | **Open** and answer in its terminal |
| "Could not start an agent: …" card | The reason follows: 6 agents open, 40 started today, the workspace's 16 terminals, the project paused, a directory outside the project, or what git said about a worktree | Close an agent, resume the project, or tell the lead what to change |
| "The lead's … request was refused: the project is paused" | The lead acted while paused | **Resume project** |
| A pull request watch never fires | `gh` is missing or not signed in for the host | See [pull request status](agent-sessions.md#pull-request-status) |
| A schedule did not run overnight | Neptune was closed | Watches run only while Neptune is open |
