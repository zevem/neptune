# Neptune

Neptune is a native desktop terminal that organizes independent shell sessions into workspaces, tabs and panes.

## Session lifecycle

**Session**:
The shell and child processes that a terminal pane runs, together with that terminal's screen and history.
_Avoid_: Process, shell (when meaning the whole pane's session)

**Close**:
Ending a pane, tab, workspace or one of several windows on purpose, which ends its sessions.
_Avoid_: Detach, quit (when meaning a pane, tab, workspace or window)

**Detach**:
Neptune exiting (by quitting, closing its last window, crashing or updating) while its sessions keep running for the next launch.
_Avoid_: Suspend, background, close

**Session daemon**:
The background process that keeps one data root's sessions running, whether or not Neptune is open.
_Avoid_: Server, host, mux, multiplexer

**Reattach**:
A launch of Neptune picking up the sessions that kept running after a detach.
_Avoid_: Resume, reconnect, restore

**Takeover**:
A newer restoring launch reattaching in place of a Neptune that is still running, which then closes its windows without ending sessions.
_Avoid_: Steal, force attach

**Recovered session**:
A session that kept running but that no saved pane refers to, which Neptune adopts instead of ending.
_Avoid_: Orphan, stray session

**Restore**:
Rebuilding saved workspaces, layout and directories with fresh sessions, used when no detached sessions survived (for example after logout or reboot).
_Avoid_: Reattach, resume
