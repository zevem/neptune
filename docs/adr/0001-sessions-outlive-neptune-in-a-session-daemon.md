---
status: accepted
---

# Sessions outlive Neptune in a session daemon

Neptune used to end every session when it exited and restore workspaces with fresh shells. We reversed that, following Rex's persistent sessions: quitting, closing the last window, a crash or an update now detaches, and the next launch reattaches to the same running sessions. Closing a pane, tab, workspace or one of several windows still ends its sessions. Each data root gets one session daemon, a private mode of the `neptune` binary that Neptune starts on demand. It owns every PTY and runs the authoritative terminal engine, so programs keep getting answers to terminal queries while Neptune is closed. Neptune keeps a replica of each terminal: on attach it receives a snapshot, then the raw output. Rendering, selection and search stay local. The daemon knows only sessions. Workspaces, tabs, splits and windows stay in Neptune's model and `workspaces.json`, where each saved pane records its session. Terminal contents stay in memory and are never written to disk. Nothing survives a reboot or logout; those launches fall back to restoring fresh shells. Linux and macOS come first, and Windows keeps today's behaviour until its ConPTY path is built and verified.

## Considered options

- **Thin client (WezTerm's mux).** Only the daemon would hold the terminal, and Neptune would fetch the screen every frame. That leaves one copy of the state, but every frame, scroll, search and selection crosses a process boundary, and it rewrites terminal-core's in-process seam.
- **PTY holder only (iTerm2, dtach).** The daemon would keep the PTY and nothing else. Output produced while Neptune is closed would be lost or need logging, programs that query the terminal would stall, and the screen would have to be saved to disk, which breaks Neptune's privacy rule.
- **Writing idle terminals to disk (Rex's parking).** Rejected for the same privacy reason. Memory stays within the existing scrollback budgets.

## Consequences

- Output is parsed twice, and history memory roughly doubles while Neptune is attached.
- Saved panes gain a session reference, which needs a new workspace state version.
- Persistence is on by default and can be turned off. It requires workspace restoration. It first merged off by default; the default was turned on before its performance gate passed, with the idle-wakeup and output-CPU costs tracked in #177 and #178.
- After an update, an old session daemon keeps serving its existing sessions until they end, and new sessions start in a daemon of the new version. The daemon protocol is versioned, and every release must still attach to daemons from earlier releases. We rejected handing PTYs over to a new daemon: the shells would remain children of the old process, so their exit statuses would be lost.
