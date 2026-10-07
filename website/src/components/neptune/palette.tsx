"use client";

// The command palette: every action, searchable, with its shortcut. The list
// follows `src/ui/palette.rs`.

import { useEffect, useRef } from "react";
import { Icon, type IconName } from "../icons";
import { WINDOW_ZOOM, type Prefs } from "../prefs";
import { Keycaps, editShortcut, shortcut } from "./controls";
import {
  activeWorkspace,
  adjacent,
  nextTab,
  ordered,
  panesOf,
  type Action,
  type Layout,
  type Close,
  type Direction,
  type State,
} from "./model";
import { BUILTINS } from "./themes";

export interface Command {
  group: string;
  icon: IconName;
  title: string;
  chord: string;
  key: string;
  run: () => void;
}

export function commands(
  state: State,
  prefs: Prefs,
  mac: boolean,
  dispatch: (action: Action) => void,
  setPrefs: (patch: Partial<Prefs>) => void,
  /** Closes a target, asking first when Preferences says to. */
  close: (target: Close) => void,
): Command[] {
  const list: Command[] = [];
  const push = (group: string, icon: IconName, title: string, chord: string, run: () => void) =>
    list.push({ group, icon, title, chord, key: `${group}:${title}:${list.length}`, run });
  const add = (
    group: string,
    icon: IconName,
    title: string,
    chord: string,
    ...actions: Action[]
  ) => push(group, icon, title, chord, () => actions.forEach(dispatch));
  const workspace = activeWorkspace(state);
  const pane = workspace?.active;

  // Terminal commands exist only with a focused terminal.
  if (workspace && pane !== undefined) {
    add("Terminal", "plus", "New tab", shortcut(mac, "T"), { type: "newTab", pane });
    add("Terminal", "splitVertical", "Split right", shortcut(mac, "D"), {
      type: "split",
      pane,
      axis: "vertical",
    });
    add("Terminal", "splitHorizontal", "Split below", shortcut(mac, "E"), {
      type: "split",
      pane,
      axis: "horizontal",
    });
    add(
      "Terminal",
      state.zoomed ? "minimize" : "maximize",
      state.zoomed ? "Show all terminals" : "Zoom terminal",
      shortcut(mac, "Enter"),
      { type: "zoom" },
    );
    add("Terminal", "search", "Find in terminal", shortcut(mac, "F"), {
      type: "search",
      open: true,
      typed: true,
    });
    add("Terminal", "eraser", "Clear scrollback", "", { type: "clear", pane });
    // An agent's tab can be put away; the agent runs on, listed in the panel.
    if (state.panes[pane]?.agent && panesOf(workspace.layout).length > 1) {
      add("Terminal", "minus", "Send to background", "", { type: "background", pane });
    }
    if (workspace.layout.kind === "split") {
      push("Terminal", "grid", "Even terminal sizes", "", () => {
        const even = (layout: Layout) => {
          if (layout.kind === "tabs") return;
          dispatch({ type: "ratio", split: layout.id, ratio: 0.5 });
          even(layout.first);
          even(layout.second);
        };
        even(workspace.layout);
      });
    }
    add("Terminal", "refresh", "Restart terminal", "", { type: "restart", pane });
    push("Terminal", "close", "Close terminal", shortcut(mac, "W"), () =>
      close({ kind: "pane", pane }),
    );
    for (const [forward, title, key] of [
      [true, "Next tab", "PgDn"],
      [false, "Previous tab", "PgUp"],
    ] as const) {
      const target = nextTab(workspace.layout, pane, forward);
      if (target !== null) {
        add("Terminal", "terminal", title, shortcut(mac, key), { type: "focus", pane: target });
      }
    }
    const moves: [Direction, string, string][] = [
      ["left", "Focus pane to the left", "←"],
      ["right", "Focus pane to the right", "→"],
      ["up", "Focus pane above", "↑"],
      ["down", "Focus pane below", "↓"],
    ];
    for (const [direction, title, arrow] of moves) {
      if (!state.zoomed && adjacent(workspace.layout, pane, direction) !== null) {
        add("Terminal", "grid", title, `Ctrl+Shift+${arrow}`, {
          type: "focusDirection",
          direction,
        });
      }
    }
  }

  add("Workspace", "plus", "New workspace", shortcut(mac, "N"), { type: "newWorkspace" });
  add("Workspace", "globe", "New SSH workspace", "", {
    type: "overlay",
    overlay: { kind: "ssh", workspace: null, host: "", typed: true },
  });
  add("Workspace", "folder", "New workspace group", "", {
    type: "overlay",
    overlay: { kind: "name", naming: { kind: "newGroup" }, text: "" },
  });
  for (const group of state.groups) {
    add("Groups", "folder", `New workspace in ${group.name}`, "", {
      type: "newWorkspace",
      group: group.id,
    });
    add("Groups", "folder", `New SSH workspace in ${group.name}`, "", {
      type: "overlay",
      overlay: { kind: "ssh", workspace: null, group: group.id, host: "", typed: true },
    });
    add("Groups", "folder", `Rename group ${group.name}`, "", {
      type: "overlay",
      overlay: { kind: "name", naming: { kind: "group", group: group.id }, text: group.name },
    });
    add("Groups", "folder", `${group.collapsed ? "Expand" : "Collapse"} group ${group.name}`, "", {
      type: "collapseGroup",
      group: group.id,
      collapsed: !group.collapsed,
    });
    add("Groups", "folder", `Remove group ${group.name} (keep workspaces)`, "", {
      type: "removeGroup",
      group: group.id,
    });
    if (workspace && workspace.group !== group.id) {
      add("Groups", "folder", `Move workspace to ${group.name}`, "", {
        type: "moveToGroup",
        workspace: workspace.id,
        group: group.id,
      });
    }
  }
  if (workspace?.group !== undefined) {
    add("Groups", "grid", "Move workspace out of group", "", {
      type: "moveToGroup",
      workspace: workspace.id,
      group: null,
    });
  }
  if (workspace) {
    add("Workspace", "pencil", "Rename workspace", "", {
      type: "overlay",
      overlay: {
        kind: "name",
        naming: { kind: "workspace", workspace: workspace.id },
        text: workspace.name,
      },
    });
    if (workspace.remote) {
      push("Workspace", "globe", "Disconnect workspace from SSH", "", () =>
        close({ kind: "connection", workspace: workspace.id }),
      );
    } else {
      add("Workspace", "globe", "Connect workspace over SSH", "", {
        type: "overlay",
        overlay: { kind: "ssh", workspace: workspace.id, host: "", typed: true },
      });
    }
    push("Workspace", "close", "Close workspace", "", () =>
      close({ kind: "workspace", workspace: workspace.id }),
    );
    // Reordering by keyboard, among the workspaces it shares a folder with.
    const row = state.workspaces.filter((other) => other.group === workspace.group);
    const index = row.indexOf(workspace);
    if (index > 0) {
      add("Workspace", "arrowUp", "Move workspace up", "", {
        type: "moveWorkspace",
        workspace: workspace.id,
        by: -1,
      });
    }
    if (index + 1 < row.length) {
      add("Workspace", "arrowDown", "Move workspace down", "", {
        type: "moveWorkspace",
        workspace: workspace.id,
        by: 1,
      });
    }
  }
  ordered(state).forEach((other, index) => {
    if (other.id === state.active) return;
    add(
      "Go to",
      "arrowUpRight",
      `Go to ${other.name}`,
      index < 9 ? shortcut(mac, String(index + 1)) : "",
      { type: "selectWorkspace", workspace: other.id },
    );
  });
  if (workspace && pane !== undefined) {
    // A terminal keeps its session, so it moves only within its machine.
    for (const other of ordered(state)) {
      if (other.id === workspace.id || other.remote !== workspace.remote) continue;
      add("Move to", "arrowUpRight", `Move terminal to ${other.name}`, "", {
        type: "movePane",
        pane,
        workspace: other.id,
      });
    }
  }
  add("View", "sidebar", "Toggle sidebar", shortcut(mac, "B"), { type: "toggleSidebar" });
  add("View", "panelRight", "Toggle right panel", shortcut(mac, "O"), { type: "panel" });
  for (const [tab, title, icon] of [
    ["files", "Show files", "file"],
    ["agents", "Show agents", "agents"],
    ["changes", "Show changes", "branch"],
    ["project", "Show project", "agents"],
  ] as const) {
    add("View", icon, title, "", { type: "panel", open: true, tab });
  }
  add("View", "bell", "Notifications", "", {
    type: "overlay",
    overlay: { kind: "notifications" },
  });
  add("View", "settings", "Preferences", editShortcut(mac, ","), {
    type: "overlay",
    overlay: { kind: "settings" },
  });
  const zoom = (by: number) =>
    Math.min(WINDOW_ZOOM.max, Math.max(WINDOW_ZOOM.min, Math.round((prefs.windowZoom + by) * 100) / 100));
  for (const [title, key, value] of [
    ["Zoom app in", "+", zoom(0.1)],
    ["Zoom app out", "-", zoom(-0.1)],
    ["Reset app zoom", "0", 1],
  ] as const) {
    push("View", "textSize", title, editShortcut(mac, key), () => {
      setPrefs({ windowZoom: value });
      dispatch({ type: "level", text: `Window zoom ${Math.round(value * 100)}%` });
    });
  }
  // A stepped value is shown for a moment under the toolbar.
  for (const [title, value] of [
    ["Increase font size", Math.min(32, prefs.fontSize + 1)],
    ["Decrease font size", Math.max(9, prefs.fontSize - 1)],
    ["Reset font size", 14],
  ] as const) {
    push("View", "textSize", title, "", () => {
      setPrefs({ fontSize: value });
      dispatch({ type: "level", text: `Font size ${value} pt` });
    });
  }
  add("Appearance", "settings", "Browse themes", "", {
    type: "overlay",
    overlay: { kind: "settings", themes: true },
  });
  for (const theme of BUILTINS) {
    if (theme.id === prefs.theme.id) continue;
    push("Appearance", theme.id === "light" ? "sun" : "moon", `Use ${theme.name} theme`, "", () =>
      setPrefs({ theme }),
    );
  }
  return list;
}

/** Every word of the query must appear in the title or its group. */
export function matches(command: Command, query: string): boolean {
  const haystack = `${command.title} ${command.group}`.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word));
}

export function Palette({
  list,
  query,
  selected,
  typed,
  onQuery,
  onSelect,
  onRun,
  onClose,
}: {
  list: Command[];
  query: string;
  selected: number;
  /** A visitor opened it, so the field takes the keyboard. */
  typed: boolean;
  onQuery: (query: string) => void;
  onSelect: (index: number) => void;
  onRun: (command: Command) => void;
  onClose: () => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const rows = useRef<HTMLDivElement>(null);
  const grouped = query.trim() === "";
  const index = Math.min(selected, Math.max(0, list.length - 1));

  useEffect(() => {
    if (typed) input.current?.focus({ preventScroll: true });
  }, [typed]);

  // The highlighted row stays in view as the keyboard moves it.
  useEffect(() => {
    const row = rows.current?.querySelector<HTMLElement>(`[data-index="${index}"]`);
    const list = rows.current;
    if (!row || !list) return;
    if (row.offsetTop < list.scrollTop + 32) list.scrollTop = Math.max(0, row.offsetTop - 32);
    const bottom = row.offsetTop + row.offsetHeight;
    if (bottom > list.scrollTop + list.clientHeight - 6) {
      list.scrollTop = bottom - list.clientHeight + 6;
    }
  }, [index]);

  return (
    <div
      role="dialog"
      aria-label="Commands"
      className="absolute top-[clamp(24px,14%,110px)] left-1/2 flex max-h-[calc(100%-36px)] w-[min(560px,calc(100%-24px))] -translate-x-1/2 flex-col overflow-hidden rounded-sheet border border-edge bg-elevated shadow-sheet"
      onKeyDown={(event) => {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          if (!list.length) return;
          const step = event.key === "ArrowDown" ? 1 : -1;
          onSelect((index + step + list.length) % list.length);
        } else if (event.key === "Enter") {
          event.preventDefault();
          if (list[index]) onRun(list[index]);
        } else if (event.key === "Escape") {
          event.preventDefault();
          onClose();
        }
        event.stopPropagation();
      }}
    >
      <label className="flex h-[52px] shrink-0 items-center border-b border-separator pr-4 pl-4 text-secondary">
        <Icon name="search" size={16} />
        <span className="sr-only">Command search</span>
        {typed ? (
          <input
            ref={input}
            value={query}
            onChange={(event) => onQuery(event.target.value)}
            placeholder="Search commands"
            spellCheck={false}
            autoComplete="off"
            className="ml-3 h-full min-w-0 flex-1 bg-transparent text-[15px] text-fg caret-accent outline-none select-text placeholder:text-muted"
          />
        ) : (
          <span className="ml-3 flex-1 truncate text-[15px] text-fg">
            {query || <span className="text-muted">Search commands</span>}
            {query && <span className="ml-px inline-block h-[1.1em] w-[1.5px] bg-accent align-[-0.18em]" />}
          </span>
        )}
      </label>
      <div
        ref={rows}
        className="quiet-scroll relative max-h-[clamp(120px,calc(100cqh-260px),372px)] min-h-0 overflow-y-auto py-1.5"
      >
        {list.length === 0 && (
          <p className="grid h-16 place-items-center text-[13px] text-muted">
            No matching commands
          </p>
        )}
        {list.map((command, position) => {
          const heading =
            grouped && command.group !== list[position - 1]?.group;
          const active = position === index;
          return (
            <div key={command.key}>
              {heading && (
                <p className="flex h-[26px] items-end px-[18px] pb-1 text-[11px] font-medium text-muted">
                  {command.group}
                </p>
              )}
              <button
                type="button"
                data-index={position}
                onPointerMove={() => !active && onSelect(position)}
                onClick={() => onRun(command)}
                className={`mx-1.5 flex h-9 w-[calc(100%-12px)] cursor-pointer items-center gap-3 rounded-control pr-[9px] pl-3 text-left text-[13px] ${
                  active ? "bg-accent text-on-accent" : "text-fg"
                }`}
              >
                <Icon
                  name={command.icon}
                  size={15}
                  className={active ? "" : "text-secondary"}
                />
                <span className="min-w-0 flex-1 truncate">{command.title}</span>
                <Keycaps chord={command.chord} className={active ? "" : "text-muted"} />
              </button>
            </div>
          );
        })}
      </div>
      <div className="flex h-8 shrink-0 items-center justify-end gap-3 border-t border-separator px-3.5 text-[11px] text-muted">
        {[
          ["↑+↓", "Navigate"],
          ["Enter", "Run"],
          ["Esc", "Close"],
        ].map(([keys, label]) => (
          <span key={label} className="flex items-center gap-1.5">
            <Keycaps chord={keys} />
            {label}
          </span>
        ))}
      </div>
    </div>
  );
}
