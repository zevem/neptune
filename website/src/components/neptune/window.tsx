"use client";

// Neptune's window, rebuilt for the browser from the app's own layout code
// (`src/ui/chrome.rs`, `src/ui/workspace.rs`) and theme tokens. Sizes are the
// app's logical points; the window responds to its own width as the app does:
// the sidebar yields below 820 points and the command field below 760.

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Icon } from "../icons";
import { usePrefs, useMac, type Prefs } from "../prefs";
import { PaneChips } from "./chips";
import { Button, IconButton, Keycaps, Pill, shortcut } from "./controls";
import {
  activeWorkspace,
  arrange,
  dropArea,
  length,
  nextTab,
  ordered,
  panesOf,
  tabsOf,
  spawnedBy,
  unreadIn,
  waits,
  type Action,
  type Box,
  type Close,
  type Destination,
  type Dispatch,
  type Divider,
  type Edge,
  type Group,
  type Pane,
  type Place,
  type State,
  type Store,
  type Workspace,
} from "./model";
import { Palette, commands, matches, type Command } from "./palette";
import { Panel, TabMark } from "./panel";
import { execute } from "./shell";
import {
  ConfirmSheet,
  NameSheet,
  Notifications,
  SettingsSheet,
  SshSheet,
  closing,
  performClose,
} from "./sheets";
import { Terminal } from "./terminal";

const boxStyle = (box: Box): React.CSSProperties => ({
  left: length(box.x),
  top: length(box.y),
  width: length(box.w),
  height: length(box.h),
});

const SLIDE = "duration-[160ms] ease-out";

const ROW_HEIGHT = 40;
/** The distance between the tops of neighbouring workspace rows. */
const ROW_STEP = ROW_HEIGHT + 2;
const GROUP_HEIGHT = 28;
/** The space around a folder: above its workspaces and before the next item. */
const GROUP_GAP = 4;
const PANE_HEADER = 34;

function WindowControls() {
  return (
    <div className="group/lights absolute top-4 left-4 z-30 flex gap-2">
      {(
        [
          ["Close window", "#ff5f57", "M-2.4 -2.4L2.4 2.4M2.4 -2.4L-2.4 2.4"],
          ["Minimize", "#febc2e", "M-3 0H3"],
          ["Maximize", "#28c840", "M-3 0H3M0 -3V3"],
        ] as const
      ).map(([label, color, glyph]) => (
        <span
          key={label}
          title={label}
          className="grid size-3 place-items-center rounded-full shadow-[inset_0_0_0_0.5px_rgb(0_0_0/0.19)]"
          style={{ background: color }}
        >
          <svg
            viewBox="-6 -6 12 12"
            className="size-3 opacity-0 transition-opacity duration-100 group-hover/lights:opacity-100"
            stroke={`color-mix(in srgb, black 62%, ${color})`}
            strokeWidth="1.2"
            strokeLinecap="round"
            aria-hidden="true"
          >
            <path d={glyph} />
          </svg>
        </span>
      ))}
    </div>
  );
}

/** The sidebar's rows and where each rests, folders with their workspaces. */
type SidebarRow =
  | { kind: "workspace"; workspace: Workspace; y: number; index: number; nested: boolean; folded: boolean }
  | { kind: "group"; group: Group; y: number; members: Workspace[] }
  | { kind: "empty"; group: Group; y: number };

function sidebarRows(state: State): { rows: SidebarRow[]; height: number } {
  const rows: SidebarRow[] = [];
  let y = 0;
  let index = 0;
  for (const item of state.order) {
    if (item.kind === "workspace") {
      const workspace = state.workspaces.find((w) => w.id === item.id);
      if (!workspace) continue;
      rows.push({ kind: "workspace", workspace, y, index: index++, nested: false, folded: false });
      y += ROW_STEP;
      continue;
    }
    const group = state.groups.find((g) => g.id === item.id);
    if (!group) continue;
    const members = state.workspaces.filter((w) => w.group === group.id);
    rows.push({ kind: "group", group, y, members });
    y += GROUP_HEIGHT + GROUP_GAP;
    members.forEach((workspace, position) =>
      rows.push({
        kind: "workspace",
        workspace,
        // A collapsed folder's workspaces rest behind its row.
        y: group.collapsed ? y - GROUP_HEIGHT - GROUP_GAP : y + position * ROW_STEP,
        index: index++,
        nested: true,
        folded: group.collapsed,
      }),
    );
    if (!group.collapsed) {
      if (members.length === 0) rows.push({ kind: "empty", group, y });
      y += Math.max(members.length * ROW_STEP - 2, 28) + GROUP_GAP;
    }
  }
  return { rows, height: Math.max(0, y - 2) };
}

function WorkspaceRow({
  row,
  state,
  dispatch,
  mac,
  hinting,
}: {
  row: Extract<SidebarRow, { kind: "workspace" }>;
  state: State;
  dispatch: Dispatch;
  mac: boolean;
  /** The shortcut modifier is held, so rows show the digit that selects them. */
  hinting: boolean;
}) {
  const { workspace } = row;
  const selected = workspace.id === state.active;
  const ids = panesOf(workspace.layout);
  const running = ids.some((id) => state.panes[id]?.status !== "exited");
  const unread = unreadIn(state, ids);
  const menu = state.overlay.kind === "menu" && state.overlay.workspace === workspace.id;
  // A carried terminal can be dropped on any workspace but its own, as long
  // as that workspace is on the same machine.
  const accepts = !selected && !row.folded && workspace.remote === activeWorkspace(state)?.remote;
  const receiving =
    state.drag?.to?.kind === "workspace" && state.drag.to.workspace === workspace.id;
  const hint = hinting && row.index < 9;
  return (
    <div
      data-workspace={workspace.id}
      data-accepts={accepts}
      aria-hidden={row.folded}
      className={`group/row absolute right-0 h-10 rounded-control transition-[top,opacity,background-color] ${SLIDE} ${
        selected ? "bg-pressed" : menu ? "bg-hover" : "hover:bg-hover"
      } ${row.folded ? "pointer-events-none opacity-0" : ""}`}
      style={{ top: row.y, left: row.nested ? 14 : 0 }}
    >
      <span
        className={`pointer-events-none absolute inset-0 rounded-control bg-accent/20 shadow-[inset_0_0_0_1.5px_color-mix(in_srgb,var(--accent)_90%,transparent)] transition-opacity duration-[120ms] ease-out ${
          receiving ? "opacity-100" : "opacity-0"
        }`}
      />
      <button
        type="button"
        tabIndex={row.folded ? -1 : undefined}
        aria-current={selected}
        aria-label={
          unread.length === 0
            ? workspace.name
            : `${workspace.name}, ${unread.length} unread notification${unread.length === 1 ? "" : "s"}`
        }
        onClick={() => dispatch({ type: "selectWorkspace", workspace: workspace.id })}
        onDoubleClick={() =>
          dispatch({
            type: "overlay",
            overlay: {
              kind: "name",
              naming: { kind: "workspace", workspace: workspace.id },
              text: workspace.name,
            },
          })
        }
        className="absolute inset-0 flex cursor-pointer flex-col justify-center rounded-control pt-px pl-[9px] text-left"
        style={{ paddingRight: unread.length ? (unread.length < 10 ? 32 : 38) : 30 }}
      >
        <span className="flex min-w-0 items-center">
          {!running && (
            // Every terminal here has stopped: a red dot leads the name.
            <span className="mr-[5px] size-1.5 shrink-0 rounded-full bg-danger" />
          )}
          <span
            className={`block truncate text-[13px] leading-4 font-medium ${
              selected || unread.length
                ? "text-fg"
                : "text-[color-mix(in_srgb,var(--fg)_45%,var(--secondary))] group-hover/row:text-fg"
            }`}
          >
            {workspace.name}
          </span>
        </span>
        <span className="flex min-w-0 items-center gap-1 text-[11px] leading-4 text-muted">
          {unread.length ? (
            // What the workspace is asking for, until it is read.
            <>
              <Icon name="bell" size={11} className="text-attention" />
              <span className="truncate text-secondary">
                {unread[0].title || unread[0].body || "Notification"}
              </span>
            </>
          ) : workspace.remote ? (
            <>
              <Icon name="globe" size={11} />
              <span className="truncate">{workspace.remote}</span>
            </>
          ) : (
            <>
              <span className="truncate">{workspace.cwd}</span>
              {workspace.branch && (
                // The branch trails the folder; uncommitted work marks it.
                <span className="ml-1 flex min-w-0 items-center gap-[3px]">
                  <Icon name="branch" size={11} />
                  <span className="truncate">{workspace.branch.name}</span>
                  {workspace.branch.dirty && <span className="ml-0.5 size-[5px] shrink-0 rounded-full bg-warn" />}
                </span>
              )}
            </>
          )}
        </span>
      </button>
      {hint && (
        <Keycaps
          chord={mac ? `⌘${row.index + 1}` : `Ctrl⇧${row.index + 1}`}
          className="pointer-events-none absolute top-1 right-[7px] text-secondary"
        />
      )}
      {unread.length > 0 ? (
        <Pill
          count={unread.length}
          className={`pointer-events-none absolute right-2 group-hover/row:opacity-0 ${
            hint ? "bottom-[3px]" : "top-[11px]"
          } ${menu ? "opacity-0" : ""}`}
        />
      ) : (
        ids.length > 1 &&
        !hint && (
          <span
            className={`pointer-events-none absolute top-1/2 right-[5px] grid size-6 -translate-y-1/2 place-items-center text-[11px] font-medium text-muted group-hover/row:opacity-0 ${
              menu ? "opacity-0" : ""
            }`}
          >
            {ids.length}
          </span>
        )
      )}
      <button
        type="button"
        tabIndex={row.folded ? -1 : undefined}
        aria-label={`Actions for ${workspace.name}`}
        aria-expanded={menu}
        onClick={() =>
          dispatch({
            type: "overlay",
            overlay: menu ? { kind: "none" } : { kind: "menu", workspace: workspace.id },
          })
        }
        className={`absolute right-[5px] grid size-6 cursor-pointer place-items-center rounded-[6px] text-secondary hover:bg-pressed hover:text-fg focus-visible:opacity-100 group-hover/row:opacity-100 ${
          hint ? "bottom-0" : "top-1/2 -translate-y-1/2"
        } ${menu ? "bg-pressed opacity-100" : "opacity-0"}`}
      >
        <Icon name="ellipsis" size={14} />
      </button>
    </div>
  );
}

/** A folder of workspaces: a disclosure chevron, an outline folder, its name. */
function GroupRow({
  row,
  state,
  dispatch,
}: {
  row: Extract<SidebarRow, { kind: "group" }>;
  state: State;
  dispatch: Dispatch;
}) {
  const { group, members } = row;
  const selected = members.some((workspace) => workspace.id === state.active);
  // Open folders show each workspace's own count; a closed one sums them.
  const unread = group.collapsed
    ? unreadIn(state, members.flatMap((workspace) => panesOf(workspace.layout))).length
    : 0;
  return (
    <div
      className={`group/folder absolute inset-x-0 h-7 rounded-control transition-[top] hover:bg-hover ${SLIDE}`}
      style={{ top: row.y }}
    >
      <button
        type="button"
        aria-expanded={!group.collapsed}
        aria-label={
          unread ? `${group.name}, ${unread} unread notification${unread === 1 ? "" : "s"}` : group.name
        }
        onClick={(event) => {
          // The second press of a double-click renames instead.
          if (event.detail > 1) return;
          dispatch({ type: "collapseGroup", group: group.id, collapsed: !group.collapsed });
        }}
        onDoubleClick={() =>
          dispatch({
            type: "overlay",
            overlay: { kind: "name", naming: { kind: "group", group: group.id }, text: group.name },
          })
        }
        className="absolute inset-0 flex cursor-pointer items-center rounded-control pr-[34px] pl-[7px] text-left"
      >
        <svg
          viewBox="-5 -5 10 10"
          className={`size-2.5 shrink-0 text-muted transition-transform duration-[120ms] ease-out ${
            group.collapsed ? "" : "rotate-90"
          }`}
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="M-2 -4L2 0L-2 4" />
        </svg>
        <Icon
          name="folder"
          size={16}
          className={`ml-[5px] ${selected ? "text-accent" : "text-secondary"}`}
        />
        <span
          className={`ml-1.5 min-w-0 truncate text-[13px] font-medium group-hover/folder:text-fg ${
            selected ? "text-fg" : "text-secondary"
          }`}
        >
          {group.name}
        </span>
      </button>
      <span className="pointer-events-none absolute top-1/2 right-[3px] grid h-7 w-7 -translate-y-1/2 place-items-center text-[11px] font-medium text-muted group-focus-within/folder:opacity-0 group-hover/folder:opacity-0">
        {unread > 0 ? <Pill count={unread} className="-ml-1" /> : members.length > 0 && members.length}
      </span>
      <button
        type="button"
        aria-label={`New workspace in ${group.name}`}
        title={`New workspace in ${group.name}`}
        onClick={() => dispatch({ type: "newWorkspace", group: group.id })}
        className="group/plus absolute top-0 right-[3px] grid size-7 cursor-pointer place-items-center text-secondary opacity-0 transition-opacity duration-[120ms] ease-out hover:text-fg focus-visible:opacity-100 group-hover/folder:opacity-100"
      >
        {/* The target spans the row's height; its highlight sits inside the row. */}
        <span className="grid size-[22px] place-items-center rounded-[6px] group-hover/plus:bg-pressed">
          <Icon name="plus" size={16} />
        </span>
      </button>
    </div>
  );
}

/** A workspace's menu. The hovered row takes the accent, as native menus do. */
function WorkspaceMenu({
  workspace,
  top,
  state,
  dispatch,
  close,
}: {
  workspace: Workspace;
  top: number;
  state: State;
  dispatch: Dispatch;
  close: (target: Close) => void;
}) {
  const row = state.workspaces.filter((other) => other.group === workspace.group);
  const position = row.indexOf(workspace);
  const item = (
    icon: Parameters<typeof Icon>[0]["name"],
    label: string,
    run: () => void,
    destructive = false,
  ) => (
    <button
      type="button"
      role="menuitem"
      onClick={() => {
        dispatch({ type: "overlay", overlay: { kind: "none" } });
        run();
      }}
      className={`flex h-7 w-full cursor-pointer items-center gap-[10px] rounded-[6px] pr-2.5 pl-2 text-left text-[13px] ${
        destructive
          ? "text-danger hover:bg-danger hover:text-on-danger"
          : "text-fg hover:bg-accent hover:text-on-accent"
      }`}
    >
      <Icon name={icon} size={14} />
      {label}
    </button>
  );
  return (
    <div
      role="menu"
      aria-label={`Actions for ${workspace.name}`}
      className="absolute left-[150px] z-40 w-[210px] animate-fade-in rounded-pane border border-edge bg-elevated p-[5px] shadow-popup"
      style={{ top: Math.min(48 + top + 32, 420) }}
    >
      {item("pencil", "Rename…", () =>
        dispatch({
          type: "overlay",
          overlay: {
            kind: "name",
            naming: { kind: "workspace", workspace: workspace.id },
            text: workspace.name,
          },
        }),
      )}
      {position > 0 &&
        item("arrowUp", "Move up", () =>
          dispatch({ type: "moveWorkspace", workspace: workspace.id, by: -1 }),
        )}
      {position + 1 < row.length &&
        item("arrowDown", "Move down", () =>
          dispatch({ type: "moveWorkspace", workspace: workspace.id, by: 1 }),
        )}
      {workspace.remote
        ? item("globe", "Disconnect from SSH", () =>
            close({ kind: "connection", workspace: workspace.id }),
          )
        : item("globe", "Connect over SSH…", () =>
            dispatch({
              type: "overlay",
              overlay: { kind: "ssh", workspace: workspace.id, host: "", typed: true },
            }),
          )}
      <div className="mx-2 my-1 h-px bg-separator" />
      {item("close", "Close workspace", () => close({ kind: "workspace", workspace: workspace.id }), true)}
    </div>
  );
}

function Sidebar({
  state,
  dispatch,
  mac,
  hinting,
}: {
  state: State;
  dispatch: Dispatch;
  mac: boolean;
  hinting: boolean;
}) {
  const { rows, height } = sidebarRows(state);
  return (
    <aside
      aria-label="Workspaces"
      className={`absolute inset-y-0 left-0 hidden w-[216px] -translate-x-full transition-transform @min-[820px]/win:block ${SLIDE} @min-[820px]/win:group-data-[sidebar=open]/win:translate-x-0`}
    >
      <div className="quiet-scroll absolute inset-x-2 top-12 bottom-[50px] overflow-x-hidden overflow-y-auto">
        <div className="relative" style={{ height }}>
          {rows.map((row) =>
            row.kind === "workspace" ? (
              <WorkspaceRow
                key={`w${row.workspace.id}`}
                row={row}
                state={state}
                dispatch={dispatch}
                mac={mac}
                hinting={hinting}
              />
            ) : row.kind === "group" ? (
              <GroupRow key={`g${row.group.id}`} row={row} state={state} dispatch={dispatch} />
            ) : (
              <p
                key={`e${row.group.id}`}
                className="absolute right-0 left-[14px] flex h-7 animate-fade-in items-center pl-[9px] text-[11.5px] text-muted"
                style={{ top: row.y }}
              >
                No workspaces
              </p>
            ),
          )}
        </div>
      </div>
      <div className="absolute inset-x-2 bottom-2 flex h-[30px] items-center">
        <button
          type="button"
          title={`New workspace   ${shortcut(mac, "N")}`}
          onClick={() => dispatch({ type: "newWorkspace" })}
          className="flex h-[30px] min-w-0 flex-1 cursor-pointer items-center gap-[9px] rounded-control pl-2 text-[12.5px] font-medium text-secondary hover:bg-hover hover:text-fg active:bg-pressed"
        >
          <Icon name="plus" size={14} />
          <span className="truncate">New workspace</span>
        </button>
        <IconButton
          icon="settings"
          label="Preferences"
          hint={mac ? "⌘," : "Ctrl+,"}
          className="ml-1.5"
          onClick={() => dispatch({ type: "overlay", overlay: { kind: "settings" } })}
        />
      </div>
    </aside>
  );
}

function SearchField({
  state,
  dispatch,
  className,
}: {
  state: State;
  dispatch: Dispatch;
  className: string;
}) {
  const input = useRef<HTMLInputElement>(null);
  const { query, typed } = state.search;
  useEffect(() => {
    if (typed) input.current?.focus({ preventScroll: true });
  }, [typed]);
  const close = () => dispatch({ type: "search", open: false });
  return (
    <div
      role="search"
      className={`relative flex h-[30px] items-center rounded-control bg-control pr-px pl-[10px] shadow-[inset_0_0_0_1px_var(--accent),0_0_0_3px_color-mix(in_srgb,var(--accent)_28%,transparent)] ${className}`}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          close();
        }
        event.stopPropagation();
      }}
    >
      <Icon name="search" size={13} className="text-muted" />
      {typed ? (
        <input
          ref={input}
          value={query}
          onChange={(event) =>
            dispatch({ type: "search", open: true, query: event.target.value, typed: true })
          }
          aria-label="Terminal search"
          placeholder="Find in scrollback"
          spellCheck={false}
          autoComplete="off"
          className="ml-[7px] h-full min-w-0 flex-1 bg-transparent text-[12.5px] text-fg caret-accent outline-none select-text placeholder:text-muted"
        />
      ) : (
        <span className="ml-[7px] min-w-0 flex-1 truncate text-[12.5px] text-fg">
          {query || <span className="text-muted">Find in scrollback</span>}
          <span className="ml-px inline-block h-[1.15em] w-[1.5px] bg-accent align-[-0.2em]" />
        </span>
      )}
      <IconButton icon="arrowUp" label="Previous match" hint="Shift+Enter" />
      <IconButton icon="arrowDown" label="Next match" hint="Enter" />
      <IconButton icon="close" label="Close search" hint="Esc" onClick={close} />
    </div>
  );
}

function Toolbar({
  state,
  dispatch,
  mac,
}: {
  state: State;
  dispatch: Dispatch;
  mac: boolean;
}) {
  const workspace = activeWorkspace(state);
  const pane = workspace ? state.panes[workspace.active] : undefined;
  const searching = state.search.open && !!pane;
  const location = pane?.remote ?? pane?.cwd;
  const unread = state.alerts.some((alert) => alert.unread);
  const listing = state.overlay.kind === "notifications";
  const several = !!workspace && panesOf(workspace.layout).length > 1;
  // An agent out of view that waits for a person dots the panel's control.
  const agentsShown = state.panel.open && state.panel.tab === "agents";
  const waiting = !agentsShown && Object.values(state.panes).some((other) => waits(other.agent));
  const palette: Action = {
    type: "overlay",
    overlay: { kind: "palette", query: "", selected: 0, typed: true },
  };
  return (
    <div className="@container/bar absolute inset-x-0 top-0 h-11">
      <div
        className={`flex h-full items-center pr-2 pl-[84px] transition-[padding] @min-[820px]/win:pl-[124px] ${SLIDE} @min-[820px]/win:group-data-[sidebar=open]/win:pl-[14px]`}
      >
        {/* Leading: the workspace, then the focused terminal. */}
        <div
          className={`flex min-w-0 items-baseline gap-2.5 ${
            searching ? "hidden flex-1 basis-0 @min-[760px]/bar:flex" : "flex-1 basis-0"
          }`}
        >
          {workspace && (
            <>
              <span className="truncate text-[13px] font-medium text-fg">{workspace.name}</span>
              {pane && (
                <span className="min-w-[48px] flex-1 truncate text-[12px] text-secondary">
                  {pane.title} — {location}
                </span>
              )}
              {/* A terminal alone in its workspace has no tab to carry them. */}
              {pane && !several && (
                <span className="self-center">
                  <PaneChips pane={pane} state={state} dispatch={dispatch} />
                </span>
              )}
            </>
          )}
        </div>

        {/* Centre: commands, or search while searching. */}
        {searching ? (
          <SearchField
            state={state}
            dispatch={dispatch}
            className="mx-3.5 min-w-0 flex-1 @min-[760px]/bar:w-[clamp(300px,42cqw,440px)] @min-[760px]/bar:flex-none"
          />
        ) : (
          <button
            type="button"
            aria-label="Command palette"
            onClick={() => dispatch(palette)}
            className="mx-3.5 hidden h-7 w-[clamp(210px,32cqw,320px)] shrink-0 cursor-pointer items-center rounded-control bg-control pr-1.5 pl-[10px] text-muted hover:bg-hover active:bg-pressed @min-[760px]/bar:flex"
          >
            <Icon name="search" size={13} />
            <span className="ml-[7px] min-w-0 flex-1 truncate text-left text-[12.5px]">
              Search commands
            </span>
            <Keycaps chord={shortcut(mac, "P")} />
          </button>
        )}

        {/* Trailing: controls for the focused terminal, then the bell. */}
        <div
          className={`flex items-center justify-end gap-0.5 ${
            searching ? "flex-none @min-[760px]/bar:flex-1 @min-[760px]/bar:basis-0" : "flex-1 basis-0"
          }`}
        >
          {state.zoomed && (
            <button
              type="button"
              title={`Show all terminals   ${shortcut(mac, "Enter")}`}
              onClick={() => dispatch({ type: "zoom" })}
              className="mr-1.5 flex h-[22px] shrink-0 animate-fade-in cursor-pointer items-center gap-[5px] rounded-full bg-accent/20 pr-2.5 pl-[9px] text-[11.5px] font-medium text-accent hover:bg-accent/30"
            >
              <Icon name="minimize" size={12} />
              Zoomed
            </button>
          )}
          <IconButton
            icon="command"
            label="Command palette"
            hint={shortcut(mac, "P")}
            className={searching ? "hidden" : "@min-[760px]/bar:hidden"}
            onClick={() => dispatch(palette)}
          />
          {pane && (
            <>
              <IconButton
                icon="search"
                label="Find in terminal"
                hint={shortcut(mac, "F")}
                onClick={() => dispatch({ type: "search", open: true, typed: true })}
              />
              <IconButton
                icon="terminal"
                label="New tab"
                hint={shortcut(mac, "T")}
                onClick={() => dispatch({ type: "newTab", pane: pane.id })}
              />
              <IconButton
                icon="splitVertical"
                label="Split right"
                hint={shortcut(mac, "D")}
                onClick={() => dispatch({ type: "split", pane: pane.id, axis: "vertical" })}
              />
              <IconButton
                icon="splitHorizontal"
                label="Split below"
                hint={shortcut(mac, "E")}
                onClick={() => dispatch({ type: "split", pane: pane.id, axis: "horizontal" })}
              />
            </>
          )}
          {/* The bell stays pressed while its popover is open, and carries a
              dot in the attention colour while any terminal has an unread alert. */}
          <span className="relative shrink-0">
            <IconButton
              icon="bell"
              label="Notifications"
              pressed={listing}
              className={listing ? "[&>span]:bg-pressed" : ""}
              onClick={() =>
                dispatch({
                  type: "overlay",
                  overlay: listing ? { kind: "none" } : { kind: "notifications" },
                })
              }
            />
            <span
              className={`pointer-events-none absolute top-[5.5px] right-[5.5px] size-[7px] rounded-full bg-attention ring-[1.5px] ring-chrome transition-transform duration-[160ms] ease-out ${
                unread ? "scale-100" : "scale-0"
              }`}
            />
          </span>
          <span className="relative shrink-0">
            <IconButton
              icon="panelRight"
              label="Toggle right panel"
              hint={shortcut(mac, "O")}
              pressed={state.panel.open}
              className={state.panel.open ? "[&>span]:bg-pressed" : ""}
              onClick={() => dispatch({ type: "panel" })}
            />
            <span
              className={`pointer-events-none absolute top-[5.5px] right-[5.5px] size-[7px] rounded-full bg-attention ring-[1.5px] ring-chrome transition-transform duration-[160ms] ease-out ${
                waiting ? "scale-100" : "scale-0"
              }`}
            />
          </span>
          {/* "New workspace" moves here while the sidebar is away. */}
          <IconButton
            icon="plus"
            label="New workspace"
            hint={shortcut(mac, "N")}
            className={`@min-[820px]/win:group-data-[sidebar=open]/win:hidden`}
            onClick={() => dispatch({ type: "newWorkspace" })}
          />
        </div>
      </div>
    </div>
  );
}

/** A floating status line with one action, near the pane's bottom edge. */
function StatusCapsule({ pane, dispatch }: { pane: Pane; dispatch: Dispatch }) {
  return (
    <div className="absolute bottom-[17px] left-1/2 flex h-[34px] max-w-[calc(100%-16px)] -translate-x-1/2 animate-fade-in items-center rounded-full border border-edge bg-elevated pr-[5px] pl-[13px] shadow-popup">
      <span className="size-[7px] shrink-0 rounded-full bg-muted" />
      <span className="mr-3 ml-[9px] truncate text-[12px] font-medium text-fg">
        Process exited
      </span>
      <button
        type="button"
        title={pane.remote ? "Connect again   Enter" : "Start a new shell   Enter"}
        onClick={() => dispatch({ type: "restart", pane: pane.id })}
        className="h-6 shrink-0 cursor-pointer rounded-full bg-accent/16 px-2.5 text-[12px] font-medium text-accent hover:bg-accent/26 active:bg-accent/34"
      >
        {pane.remote ? "Reconnect" : "Restart"}
      </button>
    </div>
  );
}

/** One tab of a place. It is also the handle that carries the terminal. */
function Tab({
  pane,
  shown,
  alone,
  focused,
  unread,
  mac,
  dispatch,
  close,
  onCarry,
  state,
}: {
  state: State;
  pane: Pane;
  /** In view in its place. */
  shown: boolean;
  /** The only tab of its place, which reads as a plain title. */
  alone: boolean;
  /** In view in the focused place. */
  focused: boolean;
  unread: boolean;
  mac: boolean;
  dispatch: Dispatch;
  close: (target: Close) => void;
  onCarry: (pane: number, clientX: number, clientY: number) => void;
}) {
  const press = useRef<{ x: number; y: number; carrying: boolean } | null>(null);
  const location = pane.remote ?? pane.cwd.split("/").pop();
  // Chips take the room the folder's name would.
  const chips =
    !!pane.ports?.length || !!pane.attached || !!pane.pulls?.length || spawnedBy(state, pane.id).length > 0;
  return (
    <div
      role="tab"
      aria-selected={shown}
      aria-label={`Terminal tab ${pane.id}`}
      title={`${pane.title}\n${pane.remote ?? pane.cwd}`}
      data-tab={pane.id}
      data-shown={shown}
      onDoubleClick={() => dispatch({ type: "zoom" })}
      onAuxClick={(event) => {
        if (event.button === 1) close({ kind: "pane", pane: pane.id });
      }}
      onPointerDown={(event) => {
        if (event.button !== 0 || (event.target as HTMLElement).closest("button")) return;
        event.stopPropagation();
        dispatch({ type: "focus", pane: pane.id });
        event.currentTarget.setPointerCapture(event.pointerId);
        press.current = { x: event.clientX, y: event.clientY, carrying: false };
      }}
      onPointerMove={(event) => {
        const held = press.current;
        if (!held) return;
        const moved = Math.hypot(event.clientX - held.x, event.clientY - held.y);
        if (!held.carrying && moved < 5) return;
        held.carrying = true;
        onCarry(pane.id, event.clientX, event.clientY);
      }}
      onPointerUp={() => {
        if (press.current?.carrying) dispatch({ type: "release" });
        press.current = null;
      }}
      onPointerCancel={() => {
        if (press.current?.carrying) dispatch({ type: "drag", drag: null });
        press.current = null;
      }}
      className={`group/tab @container/tab relative mr-0.5 flex h-full ${chips ? "max-w-[340px] min-w-[96px]" : "max-w-[218px] min-w-0"} flex-1 cursor-grab items-center rounded-[7px] pt-px pr-[7px] pl-[9px] hover:pr-6 data-[shown=true]:group-hover/pane:pr-6 data-[shown=true]:group-data-[selected=true]/pane:pr-6 ${
        alone ? "" : shown ? "bg-fg/8" : "hover:bg-fg/[0.045]"
      }`}
    >
      {!shown && unread && <span className="mr-[5px] size-1.5 shrink-0 rounded-full bg-attention" />}
      {/* The agent's own mark leads the tab while the terminal runs one. */}
      {pane.agent && <TabMark kind={pane.agent.kind} />}
      <span className={`truncate text-[12px] font-medium ${focused ? "text-fg" : "text-secondary"}`}>
        {pane.title}
      </span>
      {!chips && (
        <span className="ml-[9px] hidden min-w-0 truncate text-[11.5px] text-muted @min-[104px]/tab:block">
          {location}
        </span>
      )}
      <span className="ml-auto flex min-w-0 items-center pl-1.5">
        <PaneChips pane={pane} state={state} dispatch={dispatch} />
      </span>
      <button
        type="button"
        aria-label="Close terminal"
        title={`Close terminal   ${shortcut(mac, "W")}`}
        onClick={() => close({ kind: "pane", pane: pane.id })}
        className={`absolute top-1/2 right-[3px] hidden size-[18px] -translate-y-1/2 cursor-pointer place-items-center rounded-[5px] text-secondary transition-opacity duration-[120ms] ease-out group-hover/tab:opacity-100 hover:bg-fg/10 hover:text-fg focus-visible:opacity-100 @min-[58px]/tab:grid ${
          shown
            ? "opacity-0 group-hover/pane:opacity-100 group-data-[selected=true]/pane:opacity-100"
            : "opacity-0"
        }`}
      >
        <Icon name="close" size={9} />
      </button>
    </div>
  );
}

/** One place of the layout: its tabs and the terminal in view. */
function PlaceView({
  place,
  state,
  selected,
  multiple,
  search,
  gliding,
  live,
  mac,
  dispatch,
  close,
  onCarry,
}: {
  place: Place;
  state: State;
  /** The place holds the focused terminal. */
  selected: boolean;
  /** The layout holds more than one terminal, so places carry tabs and focus cues. */
  multiple: boolean;
  search: string;
  gliding: boolean;
  /** The window holds the keyboard, so the focused pane's cursor is solid. */
  live: boolean;
  mac: boolean;
  dispatch: Dispatch;
  close: (target: Close) => void;
  /** A tab is dragged: the pointer carries its terminal. */
  onCarry: (pane: number, clientX: number, clientY: number) => void;
}) {
  const pane = state.panes[place.shown];
  if (!pane) return null;
  // The terminal is being carried elsewhere, so its pane recedes.
  const lifted = state.drag?.pane === pane.id;
  const attention = unreadIn(state, [pane.id]).length > 0;
  const control = (
    icon: Parameters<typeof Icon>[0]["name"],
    label: string,
    key: string,
    actions: Action[],
    className: string,
  ) => (
    <IconButton
      icon={icon}
      label={label}
      hint={shortcut(mac, key)}
      className={className}
      onClick={() => actions.forEach(dispatch)}
    />
  );
  const focus: Action = { type: "focus", pane: pane.id };
  const wide = "hidden @min-[260px]/pane:block";
  return (
    <section
      aria-label={`Terminal pane ${pane.id}`}
      data-place={pane.id}
      data-tabs={place.tabs.join(",")}
      data-selected={selected}
      onPointerDown={() => dispatch(focus)}
      className={`group/pane @container/pane absolute animate-pane-in overflow-hidden rounded-pane bg-bg ${
        gliding ? `transition-[left,top,width,height] ${SLIDE}` : ""
      }`}
      style={boxStyle(place.box)}
    >
      {multiple && (
        <header className="absolute inset-x-0 top-0 flex h-[34px] pt-1.5 pr-1 pb-0.5 pl-1.5">
          <div data-tabstrip role="tablist" className="flex h-full min-w-0 flex-1">
            {place.tabs.map((id) => {
              const tab = state.panes[id];
              return (
                tab && (
                  <Tab
                    key={id}
                    state={state}
                    pane={tab}
                    shown={id === place.shown}
                    alone={place.tabs.length === 1}
                    focused={selected && id === place.shown}
                    unread={unreadIn(state, [id]).length > 0}
                    mac={mac}
                    dispatch={dispatch}
                    close={close}
                    onCarry={onCarry}
                  />
                )
              );
            })}
          </div>
          <div
            className={`-my-px ml-1 flex shrink-0 items-center transition-opacity duration-[120ms] ease-out group-hover/pane:opacity-100 ${
              selected ? "opacity-100" : "opacity-0"
            }`}
          >
            {control("plus", "New tab", "T", [{ type: "newTab", pane: pane.id }], "hidden @min-[72px]/pane:block")}
            {control("maximize", "Zoom terminal", "Enter", [focus, { type: "zoom" }], wide)}
            {control("splitVertical", "Split right", "D", [focus, { type: "split", pane: pane.id, axis: "vertical" }], wide)}
            {control("splitHorizontal", "Split below", "E", [focus, { type: "split", pane: pane.id, axis: "horizontal" }], wide)}
          </div>
        </header>
      )}
      <div
        data-focused={selected && live}
        className={`absolute inset-x-3 bottom-2.5 cursor-text ${multiple ? "top-[34px]" : "top-2.5"}`}
      >
        <Terminal pane={pane} search={selected ? search : ""} />
      </div>
      {/* Unfocused panes recede slightly; the focused one carries the accent. */}
      <div
        className={`pointer-events-none absolute inset-0 bg-bg transition-opacity duration-[140ms] ease-out ${
          multiple && !selected ? "opacity-[0.24]" : "opacity-0"
        }`}
      />
      <div className="pointer-events-none absolute inset-0 rounded-pane shadow-[inset_0_0_0_1px_var(--separator)]" />
      <div
        className={`pointer-events-none absolute inset-0 rounded-pane shadow-[inset_0_0_0_1.5px_color-mix(in_srgb,var(--accent)_85%,transparent)] transition-opacity duration-[140ms] ease-out ${
          multiple && selected && !attention ? "opacity-100" : "opacity-0"
        }`}
      />
      {/* An unread alert rings the pane in the attention colour: a crisp edge
          over a glow that fades into the terminal's margin. It takes the focus
          ring's place rather than doubling it; a single pane shows it too. */}
      <div
        className={`pointer-events-none absolute inset-0 rounded-pane shadow-[inset_0_0_0_1.5px_color-mix(in_srgb,var(--attention)_95%,transparent),inset_0_0_7px_1px_color-mix(in_srgb,var(--attention)_30%,transparent)] transition-opacity duration-[160ms] ease-out ${
          attention ? "opacity-100" : "opacity-0"
        }`}
      />
      <div
        className={`pointer-events-none absolute inset-0 rounded-pane bg-chrome transition-opacity duration-[120ms] ease-out ${
          lifted ? "opacity-60" : "opacity-0"
        }`}
      />
      {pane.status === "starting" && (
        <p className="absolute inset-0 flex items-center justify-center gap-2 text-[12.5px] text-muted">
          <svg viewBox="0 0 16 16" className="spinner size-3.5" fill="none" aria-hidden="true">
            <path d="M8 1.5a6.5 6.5 0 106.5 6.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
          {pane.remote ? "Connecting…" : "Starting shell…"}
        </p>
      )}
      {pane.status === "exited" && <StatusCapsule pane={pane} dispatch={dispatch} />}
    </section>
  );
}

/** The gutter between panes is also the split handle. */
function DividerHandle({
  divider,
  stage,
  dispatch,
  onDrag,
}: {
  divider: Divider;
  stage: React.RefObject<HTMLDivElement | null>;
  dispatch: Dispatch;
  onDrag: (dragging: boolean) => void;
}) {
  const [dragging, setDragging] = useState(false);
  const vertical = divider.axis === "vertical";
  const move = (event: React.PointerEvent) => {
    const element = stage.current;
    if (!element) return;
    const rect = element.getBoundingClientRect();
    const scale = rect.width / element.offsetWidth || 1;
    const start = vertical ? divider.area.x : divider.area.y;
    const length = vertical ? divider.area.w : divider.area.h;
    const size = vertical ? rect.width : rect.height;
    const origin = vertical ? rect.left : rect.top;
    const from = (start.pct / 100) * size + start.px * scale;
    const span = (length.pct / 100) * size + length.px * scale;
    const pointer = vertical ? event.clientX : event.clientY;
    dispatch({ type: "ratio", split: divider.id, ratio: (pointer - origin - from) / span });
  };
  return (
    <div
      role="separator"
      aria-orientation={vertical ? "vertical" : "horizontal"}
      aria-label="Resize split"
      className={`group/divider absolute z-10 touch-none ${
        vertical ? "-mx-0.5 cursor-col-resize px-0.5" : "-my-0.5 cursor-row-resize py-0.5"
      }`}
      style={{ ...boxStyle(divider), boxSizing: "content-box" }}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        setDragging(true);
        onDrag(true);
      }}
      onPointerMove={(event) => dragging && move(event)}
      onPointerUp={() => {
        setDragging(false);
        onDrag(false);
      }}
      onPointerCancel={() => {
        setDragging(false);
        onDrag(false);
      }}
      onDoubleClick={() => dispatch({ type: "ratio", split: divider.id, ratio: 0.5 })}
    >
      <span
        className={`absolute top-1/2 left-1/2 -translate-1/2 rounded-[2px] transition-opacity duration-[120ms] ease-out group-hover/divider:opacity-100 ${
          vertical ? "h-10 max-h-[calc(100%-16px)] w-[3px]" : "h-[3px] w-10 max-w-[calc(100%-16px)]"
        } ${dragging ? "bg-accent opacity-100" : "bg-muted opacity-0"}`}
      />
    </div>
  );
}

function Stage({
  state,
  dispatch,
  mac,
  onGrab,
  live,
  close,
}: {
  state: State;
  dispatch: Dispatch;
  mac: boolean;
  onGrab: () => void;
  live: boolean;
  close: (target: Close) => void;
}) {
  const stage = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  // Where a tab dropped on a header would land between the tabs there.
  const [insertion, setInsertion] = useState<{ x: number; y: number; height: number } | null>(null);
  const workspace = activeWorkspace(state);
  // The panel takes the trailing edge; a narrow window gives it the stage.
  const className = `absolute top-11 right-1.5 bottom-1.5 left-1.5 transition-[left,right] ${SLIDE} @min-[820px]/win:group-data-[sidebar=open]/win:left-0 @min-[561px]/win:group-data-[panel=open]/win:right-[300px]`;
  if (!workspace) {
    return (
      <div className={className}>
        {/* A calm placeholder for a window without a workspace. */}
        <div className="grid size-full place-items-center rounded-pane bg-bg shadow-[inset_0_0_0_1px_var(--separator)]">
          <div className="flex flex-col items-center text-center">
            <span className="grid size-[52px] place-items-center rounded-sheet bg-accent/16 text-accent">
              <Icon name="terminal" size={24} />
            </span>
            <p className="mt-5 text-[15px] font-semibold text-fg">No open workspaces</p>
            <p className="mt-1 text-[12.5px] text-secondary">
              Start a shell in your home directory.
            </p>
            <Button kind="primary" className="mt-5" onClick={() => dispatch({ type: "newWorkspace" })}>
              New workspace
            </Button>
          </div>
        </div>
      </div>
    );
  }
  const arranged = arrange(workspace.layout);
  const multiple = panesOf(workspace.layout).length > 1;
  // Zooming shows one place, with its tabs.
  const focused = tabsOf(workspace.layout, workspace.active);
  const places: Place[] =
    state.zoomed && focused
      ? [{ tabs: focused.panes, shown: focused.shown, box: arrange({ kind: "tabs", ...focused }).places[0].box }]
      : arranged.places;
  const drag = state.drag;
  const target =
    drag?.to && drag.to.kind !== "workspace"
      ? places.find((place) => place.shown === (drag.to as { pane: number }).pane)?.box
      : undefined;
  const between = drag?.to?.kind === "tab" && drag.to.index !== Number.MAX_SAFE_INTEGER;

  // Where the carried terminal would land: against the nearest edge of the
  // place under the pointer, among that place's tabs when near its centre or
  // on its header, or in a workspace of the sidebar.
  const carry = (pane: number, clientX: number, clientY: number) => {
    const element = stage.current;
    if (!element) return;
    if (!drag) onGrab();
    const bounds = element.getBoundingClientRect();
    const scale = bounds.width / element.offsetWidth || 1;
    const inside = (rect: DOMRect) =>
      clientX >= rect.left && clientX <= rect.right && clientY >= rect.top && clientY <= rect.bottom;
    let to: Destination | null = null;
    let line: typeof insertion = null;
    for (const node of element.querySelectorAll<HTMLElement>("[data-place]")) {
      const rect = node.getBoundingClientRect();
      const tabs = (node.dataset.tabs ?? "").split(",").map(Number);
      // A terminal alone in its place is already where it would land.
      if (!inside(rect) || (tabs.length === 1 && tabs[0] === pane)) continue;
      const shown = Number(node.dataset.place);
      const strip = node.querySelector<HTMLElement>("[data-tabstrip]")?.getBoundingClientRect();
      if (strip && clientY < rect.top + PANE_HEADER * scale) {
        // On the tabs: between the two under the pointer.
        const others = [...node.querySelectorAll<HTMLElement>("[data-tab]")]
          .filter((tab) => Number(tab.dataset.tab) !== pane)
          .map((tab) => tab.getBoundingClientRect());
        const index = others.filter((slot) => (slot.left + slot.right) / 2 < clientX).length;
        const x = others[index]
          ? others[index].left - scale
          : (others[others.length - 1]?.right ?? strip.left) + scale;
        to = { kind: "tab", pane: shown, index };
        line = {
          x: (x - bounds.left) / scale,
          y: (strip.top - bounds.top) / scale + 2,
          height: strip.height / scale - 4,
        };
        continue;
      }
      const x = (clientX - rect.left) / Math.max(1, rect.width);
      const y = (clientY - rect.top) / Math.max(1, rect.height);
      const edges: [number, Edge][] = [
        [x, "left"],
        [1 - x, "right"],
        [y, "top"],
        [1 - y, "bottom"],
      ];
      const [distance, edge] = edges.reduce((near, next) => (next[0] < near[0] ? next : near));
      // A tab of this place can leave for an edge, not join where it is.
      to =
        distance <= 0.3
          ? { kind: "beside", pane: shown, edge }
          : tabs.includes(pane)
            ? null
            : { kind: "tab", pane: shown, index: Number.MAX_SAFE_INTEGER };
    }
    const rows = element
      .closest("[role=application]")
      ?.querySelectorAll<HTMLElement>("[data-workspace][data-accepts=true]");
    for (const row of rows ?? []) {
      if (inside(row.getBoundingClientRect())) {
        to = { kind: "workspace", workspace: Number(row.dataset.workspace) };
        line = null;
      }
    }
    setInsertion(line);
    dispatch({
      type: "drag",
      drag: {
        pane,
        x: `${(clientX - bounds.left) / scale}px`,
        y: `${(clientY - bounds.top) / scale}px`,
        to,
      },
    });
  };

  return (
    <div ref={stage} className={className}>
      {places.map((place) => (
        <PlaceView
          key={place.tabs.join(",")}
          place={place}
          state={state}
          selected={place.tabs.includes(workspace.active)}
          multiple={multiple}
          search={state.search.open ? state.search.query : ""}
          gliding={!dragging}
          live={live}
          mac={mac}
          dispatch={dispatch}
          close={close}
          onCarry={carry}
        />
      ))}
      {multiple &&
        !state.zoomed &&
        arranged.dividers.map((divider) => (
          <DividerHandle
            key={divider.id}
            divider={divider}
            stage={stage}
            dispatch={dispatch}
            onDrag={(active) => {
              if (active) onGrab();
              setDragging(active);
            }}
          />
        ))}
      {/* The area a drop would take. It glides between areas as the pointer moves. */}
      {drag?.to && target && !between && (
        <div
          className="pointer-events-none absolute z-20 animate-fade-in rounded-pane bg-accent/20 shadow-[inset_0_0_0_1.5px_color-mix(in_srgb,var(--accent)_90%,transparent)] transition-[left,top,width,height] duration-[120ms] ease-out"
          style={boxStyle(dropArea(target, drag.to))}
        />
      )}
      {/* On a header, an insertion line marks the place between the tabs. */}
      {drag && between && insertion && (
        <div
          className="pointer-events-none absolute z-20 w-[3px] animate-fade-in rounded-full bg-accent transition-[left] duration-[120ms] ease-out"
          style={{ left: insertion.x - 1.5, top: insertion.y, height: insertion.height }}
        />
      )}
      {/* A chip with the terminal's name follows the pointer. */}
      {drag && (
        <div
          className={`pointer-events-none absolute z-30 flex h-7 animate-fade-in items-center gap-[7px] rounded-full border border-edge bg-elevated pr-3 pl-[9px] text-[12px] font-medium whitespace-nowrap text-fg shadow-popup ${
            drag.scripted ? "transition-[left,top] duration-[600ms] ease-out" : ""
          }`}
          style={{ left: `calc(${drag.x} + 14px)`, top: `calc(${drag.y} + 16px)` }}
        >
          <Icon name="terminal" size={13} className="text-secondary" />
          {state.panes[drag.pane]?.title}
        </div>
      )}
    </div>
  );
}

/** The shortcut just performed, shown the way a screen recording would. */
function Hud({ hud, mac }: { hud: NonNullable<State["hud"]>; mac: boolean }) {
  return (
    <div className="hud pointer-events-none absolute bottom-6 left-1/2 z-50 -translate-x-1/2">
      <div
        key={hud.n}
        className="flex h-[38px] animate-hud-in items-center gap-2.5 rounded-full border border-edge bg-elevated pr-4 pl-2.5 text-[12.5px] font-medium whitespace-nowrap text-fg shadow-popup"
      >
        <Keycaps chord={shortcut(mac, hud.keys)} large className="text-secondary" />
        {hud.label}
      </div>
    </div>
  );
}

export function NeptuneWindow({
  store,
  onTakeover,
  touring,
}: {
  store: Store;
  /** A visitor used the window, so a scripted tour should let go of it. */
  onTakeover: () => void;
  /** A scripted tour is typing, as a focused window would be typed in. */
  touring: boolean;
}) {
  const state = useSyncExternalStore(store.subscribe, store.get, store.get);
  const dispatch = store.dispatch;
  const { prefs, set: setPrefs } = usePrefs();
  const mac = useMac();
  const root = useRef<HTMLDivElement>(null);
  const [focused, setFocused] = useState(false);
  // The shortcut modifier is held, so workspace rows show their digits.
  const [hinting, setHinting] = useState(false);
  const overlay = state.overlay;
  const workspace = activeWorkspace(state);

  // A terminal that is starting becomes ready shortly after.
  const starting = Object.values(state.panes)
    .filter((pane) => pane.status === "starting")
    .map((pane) => pane.id)
    .join(",");
  useEffect(() => {
    if (!starting) return;
    const timer = window.setTimeout(() => {
      for (const id of starting.split(",")) {
        if (store.get().panes[Number(id)]?.status === "starting") {
          store.dispatch({ type: "ready", pane: Number(id) });
        }
      }
    }, 1300);
    return () => window.clearTimeout(timer);
  }, [starting, store]);

  // New terminals start the shell chosen in Preferences.
  const program = shellName(prefs);
  useEffect(() => {
    store.dispatch({ type: "shell", name: program });
  }, [program, store]);

  // Closing asks first, unless Preferences says not to. A running process
  // is warned about either way.
  const close = (target: Close) => {
    const { running } = closing(store.get(), target);
    if (prefs.confirmClose || (prefs.warnProcesses && running > 0)) {
      dispatch({ type: "overlay", overlay: { kind: "confirm", close: target } });
    } else {
      performClose(dispatch, target);
    }
  };

  const list: Command[] =
    overlay.kind === "palette"
      ? commands(state, prefs, mac, dispatch, setPrefs, close).filter((command) =>
          matches(command, overlay.query),
        )
      : [];

  const perform = (key: string, label: string, ...actions: Action[]) => {
    dispatch({ type: "hud", keys: key, label });
    actions.forEach(dispatch);
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    const target = event.target as HTMLElement;
    const pane = workspace ? state.panes[workspace.active] : undefined;
    const chord = mac ? event.metaKey : event.ctrlKey && event.shiftKey;
    setHinting(mac ? event.metaKey : event.ctrlKey);
    // A sheet owns its keys; shortcuts wait until it is gone.
    const modal = overlay.kind !== "none" && overlay.kind !== "menu" && overlay.kind !== "notifications";
    if (chord && !event.altKey && !modal) {
      const key = event.key.length === 1 ? event.key.toUpperCase() : event.key;
      const open: Action = {
        type: "overlay",
        overlay: { kind: "palette", query: "", selected: 0, typed: true },
      };
      const tab = (forward: boolean) => {
        const next = workspace && nextTab(workspace.layout, workspace.active, forward);
        return next == null
          ? undefined
          : () =>
              perform(forward ? "PgDn" : "PgUp", forward ? "Next tab" : "Previous tab", {
                type: "focus",
                pane: next,
              });
      };
      const direction = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" } as const;
      const bound: Record<string, (() => void) | undefined> = {
        P: () => perform("P", "Command palette", open),
        B: () => perform("B", "Toggle sidebar", { type: "toggleSidebar" }),
        O: () => perform("O", "Toggle right panel", { type: "panel" }),
        N: () => perform("N", "New workspace", { type: "newWorkspace" }),
        // Without a terminal to join, a new tab is a new workspace.
        T: () =>
          pane
            ? perform("T", "New tab", { type: "newTab", pane: pane.id })
            : perform("T", "New workspace", { type: "newWorkspace" }),
        PageDown: tab(true),
        PageUp: tab(false),
        ...(pane && {
          D: () => perform("D", "Split right", { type: "split", pane: pane.id, axis: "vertical" }),
          E: () => perform("E", "Split below", { type: "split", pane: pane.id, axis: "horizontal" }),
          F: () => perform("F", "Find in terminal", { type: "search", open: true, typed: true }),
          W: () => close({ kind: "pane", pane: pane.id }),
          Enter: () =>
            perform("Enter", state.zoomed ? "Show all terminals" : "Zoom terminal", { type: "zoom" }),
        }),
      };
      const digit = /^Digit[1-9]$/.test(event.code) ? Number(event.code.slice(5)) : 0;
      const other = digit ? ordered(state)[digit - 1] : undefined;
      if (other) {
        event.preventDefault();
        dispatch({ type: "selectWorkspace", workspace: other.id });
        return;
      }
      if (bound[key]) {
        event.preventDefault();
        bound[key]();
        return;
      }
      if (event.ctrlKey && event.shiftKey && event.key in direction) {
        event.preventDefault();
        dispatch({ type: "focusDirection", direction: direction[event.key as keyof typeof direction] });
        return;
      }
    }
    if (state.drag) {
      // Escape puts a carried terminal back; the shell does not receive it.
      if (event.key === "Escape") dispatch({ type: "drag", drag: null });
      return;
    }
    if (overlay.kind !== "none") {
      if (event.key === "Escape") dispatch({ type: "overlay", overlay: { kind: "none" } });
      return;
    }
    // A focused control keeps its own activation keys.
    if (target.closest("button, input") && (event.key === "Enter" || event.key === " ")) return;
    if (event.key === "Tab" || event.metaKey || event.altKey || !pane) return;
    if (event.key === "Escape") {
      root.current?.blur();
      return;
    }
    if (pane.status === "exited") {
      if (event.key === "Enter") dispatch({ type: "restart", pane: pane.id });
      return;
    }
    if (pane.status !== "running") return;
    if (event.ctrlKey) {
      const key = event.key.toLowerCase();
      if (key === "c") {
        event.preventDefault();
        if (pane.prompt) {
          dispatch({ type: "input", pane: pane.id, input: `${pane.input}^C` });
          dispatch({ type: "commit", pane: pane.id });
        } else {
          dispatch({ type: "print", pane: pane.id, lines: [{ k: "out", spans: [{ t: "^C" }] }] });
        }
        dispatch({ type: "ready", pane: pane.id, ok: false });
      } else if (key === "l") {
        event.preventDefault();
        dispatch({ type: "clear", pane: pane.id });
      }
      return;
    }
    if (!pane.prompt) return;
    if (event.key === "Enter") {
      event.preventDefault();
      execute({ dispatch, setPrefs }, pane);
    } else if (event.key === "Backspace") {
      event.preventDefault();
      dispatch({ type: "input", pane: pane.id, input: pane.input.slice(0, -1) });
    } else if (event.key.length === 1) {
      event.preventDefault();
      dispatch({ type: "input", pane: pane.id, input: pane.input + event.key });
    }
  };

  const menu = overlay.kind === "menu" ? overlay.workspace : null;
  const menuRow =
    menu === null
      ? undefined
      : sidebarRows(state).rows.find((row) => row.kind === "workspace" && row.workspace.id === menu);
  // Window zoom scales terminal text and chrome together; the window keeps
  // its place and lays itself out in the room that leaves.
  const zoom = prefs.windowZoom;

  return (
    <div
      ref={root}
      role="application"
      aria-label="Neptune, an interactive demonstration"
      aria-roledescription="terminal window"
      tabIndex={0}
      data-sidebar={state.sidebar ? "open" : "closed"}
      data-panel={state.panel.open ? "open" : "closed"}
      className="group/win relative overflow-hidden rounded-window bg-chrome text-[13px] leading-normal shadow-window outline-none select-none [container:win_/_size]"
      style={{ zoom, width: `${100 / zoom}%`, height: `${100 / zoom}%` }}
      onFocus={() => setFocused(true)}
      onBlur={(event) => {
        if (event.currentTarget.contains(event.relatedTarget)) return;
        setFocused(false);
        setHinting(false);
      }}
      onKeyDownCapture={onTakeover}
      onClickCapture={onTakeover}
      onKeyDown={onKeyDown}
      onKeyUp={(event) => setHinting(mac ? event.metaKey : event.ctrlKey)}
      onClick={(event) => {
        // The terminal holds the keyboard, so a pressed control hands it back.
        const target = event.target as HTMLElement;
        if (store.get().overlay.kind === "none" && !target.closest("input")) {
          root.current?.focus({ preventScroll: true });
        }
      }}
    >
      <Sidebar state={state} dispatch={dispatch} mac={mac} hinting={hinting} />
      <div className={`absolute inset-y-0 right-0 left-0 transition-[left] ${SLIDE} @min-[820px]/win:group-data-[sidebar=open]/win:left-[216px]`}>
        <Toolbar state={state} dispatch={dispatch} mac={mac} />
        <Panel state={state} dispatch={dispatch} />
        <Stage
          state={state}
          dispatch={dispatch}
          mac={mac}
          onGrab={onTakeover}
          live={touring || focused}
          close={close}
        />
      </div>
      <WindowControls />
      <IconButton
        icon="sidebar"
        label="Toggle sidebar"
        hint={shortcut(mac, "B")}
        className={`absolute top-2 left-[80px] z-30 hidden transition-[left] @min-[820px]/win:block ${SLIDE} @min-[820px]/win:group-data-[sidebar=open]/win:left-[180px]`}
        onClick={() => dispatch({ type: "toggleSidebar" })}
      />

      {(overlay.kind === "menu" || overlay.kind === "notifications") && (
        <>
          <div
            className="absolute inset-0 z-30"
            onClick={() => dispatch({ type: "overlay", overlay: { kind: "none" } })}
          />
          {menuRow?.kind === "workspace" && (
            <WorkspaceMenu
              workspace={menuRow.workspace}
              top={menuRow.y}
              state={state}
              dispatch={dispatch}
              close={close}
            />
          )}
          {overlay.kind === "notifications" && <Notifications state={state} dispatch={dispatch} />}
        </>
      )}

      {(overlay.kind === "palette" ||
        overlay.kind === "ssh" ||
        overlay.kind === "settings" ||
        overlay.kind === "confirm" ||
        overlay.kind === "name") && (
        <div className="absolute inset-0 z-40">
          {/* The dim follows the window's rounded shape. */}
          <div
            className="absolute inset-0 animate-fade-in rounded-window bg-scrim"
            onClick={() => dispatch({ type: "overlay", overlay: { kind: "none" } })}
          />
          {overlay.kind === "palette" && (
            <Palette
              list={list}
              query={overlay.query}
              selected={overlay.selected}
              typed={overlay.typed}
              onQuery={(query) =>
                dispatch({ type: "overlay", overlay: { ...overlay, query, selected: 0 } })
              }
              onSelect={(selected) =>
                dispatch({ type: "overlay", overlay: { ...overlay, selected } })
              }
              onRun={(command) => {
                // Close first: the command itself may open another overlay.
                dispatch({ type: "overlay", overlay: { kind: "none" } });
                command.run();
                if (store.get().overlay.kind === "none") root.current?.focus({ preventScroll: true });
              }}
              onClose={() => dispatch({ type: "overlay", overlay: { kind: "none" } })}
            />
          )}
          {overlay.kind === "ssh" && <SshSheet overlay={overlay} dispatch={dispatch} />}
          {overlay.kind === "name" && <NameSheet overlay={overlay} dispatch={dispatch} />}
          {overlay.kind === "confirm" && (
            <ConfirmSheet close={overlay.close} state={state} dispatch={dispatch} />
          )}
          {overlay.kind === "settings" && (
            <SettingsSheet themes={overlay.themes ?? false} dispatch={dispatch} />
          )}
        </div>
      )}

      {state.hud && <Hud hud={state.hud} mac={mac} />}
      {state.level && (
        // A stepped value, such as the font size, shown for a moment.
        <div
          key={state.level.n}
          className="hud pointer-events-none absolute top-[52px] left-1/2 z-50 flex h-7 -translate-x-1/2 animate-hud-in items-center rounded-full border border-edge bg-elevated px-3 text-[12px] font-medium whitespace-nowrap text-fg shadow-popup"
        >
          {state.level.text}
        </div>
      )}
    </div>
  );
}

/** The program new terminals start: the platform default, or the one chosen. */
export const shellName = (prefs: Prefs) =>
  prefs.shell?.trim().split(/[\\/]/).pop()?.replace(/\.exe$/i, "") || "zsh";
