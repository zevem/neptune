// A small model of Neptune's workspace state for the in-page window. It follows
// the shapes of `crates/neptune-model` and the saved `workspaces.json`: a
// workspace owns a binary layout whose places each hold terminals as tabs, and
// every change is one action.

export type Axis = "vertical" | "horizontal";

export type Layout =
  /** Terminals sharing one place as tabs. Only `shown` is in view. */
  | { kind: "tabs"; panes: number[]; shown: number }
  | {
      kind: "split";
      id: number;
      axis: Axis;
      ratio: number;
      first: Layout;
      second: Layout;
    };

/** Terminal colours: a palette token or an ANSI index. */
export type Tone = "fg" | "secondary" | "muted" | number;
export type Span = { t: string; c?: Tone; b?: boolean };

export type Line =
  /** The prompt's context line: where the shell is. */
  | { k: "ctx"; cwd: string; branch?: string; host?: string }
  /** A command as it was entered. `ok` is the previous command's result. */
  | { k: "cmd"; text: string; ok: boolean }
  | { k: "out"; spans: Span[] };

/** The CLI agents whose marks a terminal's tab shows. */
export type AgentKind = "claude" | "codex";

/** What an agent is doing, as the Agents tab groups it. */
export type Activity = "working" | "idle" | "permission" | "question";

/** A coding agent running in a terminal. */
export interface Agent {
  kind: AgentKind;
  activity: Activity;
  /** What the agent calls its conversation. */
  title: string;
  /** How long it has been doing it, in minutes. */
  minutes: number;
  /** The terminal whose agent started this one. */
  parent?: number;
  /** The workspace it belongs to, which it keeps while it has no tab. */
  workspace: number;
}

/** A pull request linked to a terminal, with what its checks say. */
export interface Pull {
  number: number;
  state: "open" | "draft" | "merged";
  checks: "none" | "pending" | "passing" | "failing";
  /** Unresolved review comments. */
  comments: number;
}

/** A file that differs from the last commit or from the main branch. */
export interface ChangedFile {
  path: string;
  status: "M" | "A" | "D";
  added: number;
  removed: number;
  /** Already committed on the branch, so the working tree no longer lists it. */
  committed?: boolean;
  /** Lines of its diff: a hunk header, context, an addition or a removal. */
  diff: { k: "@" | " " | "+" | "-"; text: string; n?: number }[];
}

export type PanelTab = "files" | "agents" | "changes" | "project";

/** One entry of a project's chat. */
export type ChatEntry =
  /** What the person said. */
  | { k: "user"; text: string }
  /** What the lead said. */
  | { k: "lead"; text: string }
  /** Something Neptune did for the lead, such as starting an agent. */
  | { k: "tool"; text: string }
  /** A report about one of its agents, outlined apart from what was said. */
  | { k: "card"; agent: number; what: string; text: string };

/** Something only the person can clear, pinned above the chat. */
export interface Need {
  agent: number;
  title: string;
  detail: string;
}

/** A lead agent in charge of the work of one workspace. */
export interface Project {
  name: string;
  directory: string;
  /** The lead's CLI and what it runs with. */
  lead: string;
  chat: ChatEntry[];
  /** The terminals of the agents it started. */
  members: number[];
  needs: Need[];
  /** How long the lead's turn has run, in seconds, while it runs. */
  working: number | null;
  /** The message being written, and whether a visitor is writing it. */
  draft: string;
  typed: boolean;
}

export interface Pane {
  id: number;
  title: string;
  cwd: string;
  branch?: string;
  /** SSH destination when the terminal runs on another machine. */
  remote?: string;
  /** The coding agent the terminal runs, if any. */
  agent?: Agent;
  pulls?: Pull[];
  /** Local listeners, as the port chips name them. */
  ports?: number[];
  /** How many files its agent attached to the tab. */
  attached?: number;
  lines: Line[];
  input: string;
  /** A prompt is waiting for input. False while a command is producing output. */
  prompt: boolean;
  /** A program that keeps running, such as `tail -f`, holds the terminal. */
  busy?: boolean;
  /** The previous command succeeded; the prompt reddens when it did not. */
  ok: boolean;
  status: "starting" | "running" | "exited";
}

export interface Workspace {
  id: number;
  name: string;
  cwd: string;
  remote?: string;
  /** The folder group holding it, when it is not at the list's top level. */
  group?: number;
  layout: Layout;
  active: number;
  /** The branch of its folder, and whether work is not committed yet. */
  branch?: { name: string; dirty: boolean };
  /** What git reports for its folder: the files that differ. */
  changes?: ChangedFile[];
}

/** A folder of workspaces in the sidebar. */
export interface Group {
  id: number;
  name: string;
  collapsed: boolean;
}

/** Ungrouped workspaces and folder groups share one ordered list. */
export type SidebarItem = { kind: "workspace"; id: number } | { kind: "group"; id: number };

/** A terminal asking for attention through OSC 9, 99 or 777. */
export interface Alert {
  id: number;
  pane: number;
  title: string;
  body: string;
  unread: boolean;
  /** When it arrived, in milliseconds since the epoch. */
  at: number;
}

/** What a confirmation would close. */
export type Close =
  | { kind: "pane"; pane: number }
  | { kind: "workspace"; workspace: number }
  | { kind: "connection"; workspace: number };

/** What a name sheet names. */
export type Naming =
  | { kind: "workspace"; workspace: number }
  | { kind: "group"; group: number }
  | { kind: "newGroup" };

export type Overlay =
  | { kind: "none" }
  | { kind: "palette"; query: string; selected: number; typed: boolean }
  | { kind: "settings"; themes?: boolean }
  | { kind: "ssh"; workspace: number | null; group?: number; host: string; typed: boolean }
  | { kind: "menu"; workspace: number }
  | { kind: "notifications" }
  | { kind: "confirm"; close: Close }
  | { kind: "name"; naming: Naming; text: string };

export interface State {
  workspaces: Workspace[];
  groups: Group[];
  /** The sidebar's top level. A group's workspaces follow `workspaces`. */
  order: SidebarItem[];
  panes: Record<number, Pane>;
  /** Newest first. History is bounded, as in the app. */
  alerts: Alert[];
  active: number | null;
  sidebar: boolean;
  zoomed: boolean;
  overlay: Overlay;
  search: { open: boolean; query: string; typed: boolean };
  /** A terminal being carried by its tab to another place. */
  drag: Drag | null;
  /** The last shortcut performed, shown briefly over the window. */
  hud: { keys: string; label: string; n: number } | null;
  /** A value just stepped, such as the font size, shown under the toolbar. */
  level: { text: string; n: number } | null;
  /** The panel at the trailing edge and the tab in view. */
  panel: { open: boolean; tab: PanelTab };
  /** What the Changes tab compares with, and the file whose diff is shown. */
  changes: { scope: "working" | "branch"; file: string | null };
  /** Each workspace's project, by workspace. */
  projects: Record<number, Project>;
  /** The program new local terminals start. */
  shell: string;
  nextId: number;
  /** Workspaces number themselves; their identity colour follows the id. */
  nextWorkspace: number;
}

export type Edge = "left" | "right" | "top" | "bottom";

/** Where a carried terminal lands. */
export type Destination =
  /** In a place of its own against an edge of `pane`'s place. */
  | { kind: "beside"; pane: number; edge: Edge }
  /** Among the tabs of `pane`'s place; an index past the end means last. */
  | { kind: "tab"; pane: number; index: number }
  | { kind: "workspace"; workspace: number };

export interface Drag {
  pane: number;
  /** Where the carried terminal's chip is, as CSS lengths within the stage. */
  x: string;
  y: string;
  to: Destination | null;
  /** The tour is carrying it, so the chip glides between positions. */
  scripted?: boolean;
}

export type Action =
  | { type: "load"; state: State }
  | { type: "drag"; drag: Drag | null }
  | { type: "drop"; pane: number; to: Destination }
  /** Lets go of a carried terminal, dropping it where it is held. */
  | { type: "release" }
  | { type: "split"; pane: number; axis: Axis }
  | { type: "newTab"; pane: number }
  | { type: "closePane"; pane: number }
  | { type: "focus"; pane: number }
  | { type: "focusDirection"; direction: Direction }
  | { type: "ratio"; split: number; ratio: number }
  | { type: "zoom" }
  | { type: "newWorkspace"; name?: string; cwd?: string; branch?: string; group?: number }
  | { type: "connect"; workspace: number | null; group?: number; destination: string }
  | { type: "disconnect"; workspace: number }
  | { type: "selectWorkspace"; workspace: number }
  /** Trades places with the sibling before (-1) or after (1) it. */
  | { type: "moveWorkspace"; workspace: number; by: -1 | 1 }
  | { type: "renameWorkspace"; workspace: number; name: string }
  | { type: "closeWorkspace"; workspace: number }
  | { type: "movePane"; pane: number; workspace: number }
  | { type: "newGroup"; name: string }
  | { type: "renameGroup"; group: number; name: string }
  | { type: "collapseGroup"; group: number; collapsed: boolean }
  /** Removes the folder and keeps its workspaces. */
  | { type: "removeGroup"; group: number }
  | { type: "moveToGroup"; workspace: number; group: number | null }
  | { type: "toggleSidebar" }
  | { type: "overlay"; overlay: Overlay }
  | { type: "search"; open: boolean; query?: string; typed?: boolean }
  | { type: "hud"; keys: string; label: string }
  | { type: "input"; pane: number; input: string }
  | { type: "commit"; pane: number }
  | { type: "print"; pane: number; lines: Line[] }
  /** Rewrites the last row, as a program does when it draws progress. */
  | { type: "amend"; pane: number; line: Line }
  | { type: "ready"; pane: number; ok?: boolean }
  | { type: "patch"; pane: number; patch: Partial<Pane> }
  | { type: "clear"; pane: number }
  | { type: "restart"; pane: number }
  | { type: "notify"; pane: number; title: string; body: string; at: number }
  | { type: "openAlert"; alert: number }
  | { type: "dismissAlert"; alert: number }
  | { type: "readAlerts" }
  | { type: "clearAlerts" }
  | { type: "shell"; name: string }
  | { type: "level"; text: string }
  /** Shows or hides the panel; a tab alone brings that tab into view. */
  | { type: "panel"; open?: boolean; tab?: PanelTab }
  | { type: "changes"; scope?: "working" | "branch"; file?: string | null }
  | { type: "workspacePatch"; workspace: number; patch: Partial<Workspace> }
  /** An agent starts another, out of view: a terminal with no tab. */
  | { type: "spawn"; agent: Agent; cwd: string; lines?: Line[] }
  | { type: "agent"; pane: number; patch: Partial<Agent> }
  /** Opens an agent's terminal, as a tab where it has none. */
  | { type: "openAgent"; pane: number }
  /** Puts an agent's tab away; the agent runs on out of view. */
  | { type: "background"; pane: number }
  | { type: "project"; workspace: number; patch: Partial<Project> }
  | { type: "chat"; workspace: number; entry: ChatEntry };

export type Direction = "left" | "right" | "up" | "down";

export type Dispatch = (action: Action) => void;

/** History is bounded, as in the app; the page keeps far less of it. */
const HISTORY = 240;
/** Alert history is capped across the application. */
const ALERTS = 128;
/** A project's chat keeps a page of entries here. */
const CHAT = 60;

export const AGENT_NAMES: Record<AgentKind, string> = { claude: "Claude Code", codex: "Codex" };

/** The state in words, as the Agents tab says it. */
export const ACTIVITY_NAMES: Record<Activity, string> = {
  working: "Working",
  idle: "Idle",
  permission: "Needs permission",
  question: "Asked a question",
};

export const waits = (agent: Agent | undefined) =>
  agent?.activity === "permission" || agent?.activity === "question";

/** A project for a workspace that has none yet: a lead and an empty chat. */
export const newProject = (workspace: Workspace): Project => ({
  name: workspace.name,
  directory: workspace.cwd,
  lead: "Claude Code · Opus · High",
  chat: [],
  members: [],
  needs: [],
  working: null,
  draft: "",
  typed: false,
});

export const out = (text: string, c?: Tone, b?: boolean): Line => ({
  k: "out",
  spans: [{ t: text, c, b }],
});
export const row = (...spans: (Span | string)[]): Line => ({
  k: "out",
  spans: spans.map((span) => (typeof span === "string" ? { t: span } : span)),
});

export const place = (pane: number): Layout => ({ kind: "tabs", panes: [pane], shown: pane });

/** Every pane, including tabs that are not in view. */
export function panesOf(layout: Layout): number[] {
  return layout.kind === "tabs"
    ? layout.panes
    : [...panesOf(layout.first), ...panesOf(layout.second)];
}

/** The tabs sharing a place with `pane`, and which of them is in view. */
export function tabsOf(layout: Layout, pane: number): { panes: number[]; shown: number } | null {
  if (layout.kind === "tabs") return layout.panes.includes(pane) ? layout : null;
  return tabsOf(layout.first, pane) ?? tabsOf(layout.second, pane);
}

/** The tab after or before `pane` in its place, wrapping around. */
export function nextTab(layout: Layout, pane: number, forward: boolean): number | null {
  const tabs = tabsOf(layout, pane)?.panes;
  if (!tabs || tabs.length < 2) return null;
  const step = forward ? 1 : tabs.length - 1;
  return tabs[(tabs.indexOf(pane) + step) % tabs.length];
}

function edit(layout: Layout, target: number, make: (tabs: Extract<Layout, { kind: "tabs" }>) => Layout): Layout {
  if (layout.kind === "tabs") return layout.panes.includes(target) ? make(layout) : layout;
  const first = edit(layout.first, target, make);
  const second = edit(layout.second, target, make);
  return first === layout.first && second === layout.second ? layout : { ...layout, first, second };
}

/** Splits `target`'s place, putting `pane` against the given edge of it. */
function splitAt(layout: Layout, target: number, pane: number, id: number, edge: Edge): Layout {
  const leading = edge === "left" || edge === "top";
  return edit(layout, target, (tabs) => ({
    kind: "split",
    id,
    axis: edge === "left" || edge === "right" ? "vertical" : "horizontal",
    ratio: 0.5,
    first: leading ? place(pane) : tabs,
    second: leading ? tabs : place(pane),
  }));
}

/**
 * Adds `pane` to `target`'s tabs and brings it into view. Without an index it
 * follows `target`; an index past the end means last.
 */
function addTab(layout: Layout, target: number, pane: number, index?: number): Layout {
  return edit(layout, target, (tabs) => {
    const at = Math.min(index ?? tabs.panes.indexOf(target) + 1, tabs.panes.length);
    return { kind: "tabs", panes: [...tabs.panes.slice(0, at), pane, ...tabs.panes.slice(at)], shown: pane };
  });
}

/** Brings `pane` into view in its place. */
const show = (layout: Layout, pane: number): Layout =>
  edit(layout, pane, (tabs) => (tabs.shown === pane ? tabs : { ...tabs, shown: pane }));

/**
 * A place that loses the tab in view shows the one that followed it, or the
 * new last tab. A place that loses its only tab gives its room to its sibling.
 */
function remove(layout: Layout, pane: number): Layout | null {
  if (layout.kind === "tabs") {
    const position = layout.panes.indexOf(pane);
    if (position < 0) return layout;
    const panes = layout.panes.filter((id) => id !== pane);
    if (panes.length === 0) return null;
    const shown = layout.shown === pane ? (panes[position] ?? panes[panes.length - 1]) : layout.shown;
    return { kind: "tabs", panes, shown };
  }
  const first = remove(layout.first, pane);
  const second = remove(layout.second, pane);
  if (!first) return second;
  if (!second) return first;
  return first === layout.first && second === layout.second
    ? layout
    : { ...layout, first, second };
}

/** The same panes in the same places, whatever the ratios and tabs in view. */
function sameArrangement(a: Layout, b: Layout): boolean {
  if (a.kind === "tabs" || b.kind === "tabs") {
    return a.kind === "tabs" && b.kind === "tabs" && a.panes.join() === b.panes.join();
  }
  return a.axis === b.axis && sameArrangement(a.first, b.first) && sameArrangement(a.second, b.second);
}

function setRatio(layout: Layout, split: number, ratio: number): Layout {
  if (layout.kind === "tabs") return layout;
  if (layout.id === split) return { ...layout, ratio };
  return {
    ...layout,
    first: setRatio(layout.first, split, ratio),
    second: setRatio(layout.second, split, ratio),
  };
}

/** A length inside the stage: a share of it plus a fixed offset in pixels. */
export interface Span1D {
  pct: number;
  px: number;
}
export interface Box {
  x: Span1D;
  y: Span1D;
  w: Span1D;
  h: Span1D;
}
export interface Divider extends Box {
  id: number;
  axis: Axis;
  /** The split's own area, which a drag measures against. */
  area: Box;
}

export const GUTTER = 6;

export const length = (span: Span1D) => `calc(${span.pct}% + ${span.px}px)`;
const FULL: Box = {
  x: { pct: 0, px: 0 },
  y: { pct: 0, px: 0 },
  w: { pct: 100, px: 0 },
  h: { pct: 100, px: 0 },
};

/** One place of the layout: its tabs, the one in view and where it sits. */
export interface Place {
  tabs: number[];
  shown: number;
  box: Box;
}

/**
 * Where each place and divider sits. Lengths stay symbolic so the browser
 * resolves them, and panes glide when a ratio changes. `panes` holds the
 * place of every terminal in view.
 */
export function arrange(layout: Layout): {
  places: Place[];
  panes: Map<number, Box>;
  dividers: Divider[];
} {
  const places: Place[] = [];
  const panes = new Map<number, Box>();
  const dividers: Divider[] = [];
  const half = GUTTER / 2;
  const visit = (node: Layout, box: Box) => {
    if (node.kind === "tabs") {
      places.push({ tabs: node.panes, shown: node.shown, box });
      panes.set(node.shown, box);
      return;
    }
    const vertical = node.axis === "vertical";
    const start = vertical ? box.x : box.y;
    const length = vertical ? box.w : box.h;
    const r = node.ratio;
    const cut: Span1D = {
      pct: start.pct + length.pct * r,
      px: start.px + length.px * r,
    };
    const a: Span1D = { pct: length.pct * r, px: length.px * r - half };
    const b: Span1D = {
      pct: length.pct * (1 - r),
      px: length.px * (1 - r) - half,
    };
    const after: Span1D = { pct: cut.pct, px: cut.px + half };
    const gap: Span1D = { pct: cut.pct, px: cut.px - half };
    const thickness: Span1D = { pct: 0, px: GUTTER };
    if (vertical) {
      dividers.push({ id: node.id, axis: node.axis, area: box, ...box, x: gap, w: thickness });
      visit(node.first, { ...box, w: a });
      visit(node.second, { ...box, x: after, w: b });
    } else {
      dividers.push({ id: node.id, axis: node.axis, area: box, ...box, y: gap, h: thickness });
      visit(node.first, { ...box, h: a });
      visit(node.second, { ...box, y: after, h: b });
    }
  };
  visit(layout, FULL);
  return { places, panes, dividers };
}

/** The area a carried terminal would take: half of a place, or all of it. */
export function dropArea(box: Box, to: Destination): Box {
  if (to.kind !== "beside") return box;
  const half = (span: Span1D): Span1D => ({ pct: span.pct / 2, px: span.px / 2 - GUTTER / 2 });
  const middle = (start: Span1D, span: Span1D): Span1D => ({
    pct: start.pct + span.pct / 2,
    px: start.px + span.px / 2 + GUTTER / 2,
  });
  switch (to.edge) {
    case "left":
      return { ...box, w: half(box.w) };
    case "right":
      return { ...box, x: middle(box.x, box.w), w: half(box.w) };
    case "top":
      return { ...box, h: half(box.h) };
    case "bottom":
      return { ...box, y: middle(box.y, box.h), h: half(box.h) };
  }
}

/** The pane in view nearest to `pane`'s place in a direction, by geometry. */
export function adjacent(
  layout: Layout,
  pane: number,
  direction: Direction,
): number | null {
  const unit = (span: Span1D) => span.pct / 100;
  const { panes } = arrange(layout);
  const origin = tabsOf(layout, pane)?.shown;
  const from = origin === undefined ? undefined : panes.get(origin);
  if (!from) return null;
  const rect = (box: Box) => ({
    l: unit(box.x),
    t: unit(box.y),
    r: unit(box.x) + unit(box.w),
    b: unit(box.y) + unit(box.h),
  });
  const a = rect(from);
  let best: { id: number; overlap: number } | null = null;
  for (const [id, box] of panes) {
    if (id === origin) continue;
    const b = rect(box);
    const touches =
      direction === "left"
        ? Math.abs(b.r - a.l) < 1e-6
        : direction === "right"
          ? Math.abs(b.l - a.r) < 1e-6
          : direction === "up"
            ? Math.abs(b.b - a.t) < 1e-6
            : Math.abs(b.t - a.b) < 1e-6;
    if (!touches) continue;
    const overlap =
      direction === "left" || direction === "right"
        ? Math.min(a.b, b.b) - Math.max(a.t, b.t)
        : Math.min(a.r, b.r) - Math.max(a.l, b.l);
    if (overlap > 1e-6 && (!best || overlap > best.overlap)) best = { id, overlap };
  }
  return best?.id ?? null;
}

/**
 * Workspaces in the sidebar's order, including those of collapsed folders.
 * It decides workspace navigation and the shortcut each one answers to.
 */
export function ordered(state: State): Workspace[] {
  return state.order.flatMap((item) =>
    state.workspaces.filter((workspace) =>
      item.kind === "workspace" ? workspace.id === item.id : workspace.group === item.id,
    ),
  );
}

export const activeWorkspace = (state: State): Workspace | undefined =>
  state.workspaces.find((workspace) => workspace.id === state.active);

export const activePane = (state: State): Pane | undefined => {
  const workspace = activeWorkspace(state);
  return workspace ? state.panes[workspace.active] : undefined;
};

/** A workspace is named after its host when remote: `user@host` → `host`. */
export function remoteLabel(destination: string): string {
  const host = destination.replace(/^ssh:\/\//, "").split("@").pop() ?? "";
  return host || destination;
}

/** An OpenSSH destination: no spaces, no leading dash. */
export function validDestination(text: string): boolean {
  const destination = text.trim();
  return (
    destination.length > 0 &&
    destination.length <= 255 &&
    !destination.startsWith("-") &&
    !/[\s\p{Cc}]/u.test(destination)
  );
}

function shell(id: number, from: Partial<Pane>, program = "zsh"): Pane {
  return {
    id,
    title: from.remote ? "ssh" : program,
    cwd: from.cwd ?? "~",
    branch: from.branch,
    remote: from.remote,
    lines: [],
    input: "",
    prompt: !from.remote,
    ok: true,
    status: from.remote ? "starting" : "running",
  };
}

function editWorkspace(
  state: State,
  id: number,
  edit: (workspace: Workspace) => Workspace,
): State {
  return {
    ...state,
    workspaces: state.workspaces.map((workspace) =>
      workspace.id === id ? edit(workspace) : workspace,
    ),
  };
}

function editPane(state: State, id: number, edit: (pane: Pane) => Pane): State {
  const pane = state.panes[id];
  return pane ? { ...state, panes: { ...state.panes, [id]: edit(pane) } } : state;
}

export const owner = (state: State, pane: number): Workspace | undefined =>
  state.workspaces.find((workspace) => panesOf(workspace.layout).includes(pane));

/** Unread alerts of the given panes. */
export const unreadIn = (state: State, panes: number[]): Alert[] =>
  state.alerts.filter((alert) => alert.unread && panes.includes(alert.pane));

/** Focusing a pane or typing in it acknowledges its alerts. */
function acknowledge(state: State, pane: number): State {
  return state.alerts.some((alert) => alert.unread && alert.pane === pane)
    ? {
        ...state,
        alerts: state.alerts.map((alert) =>
          alert.pane === pane ? { ...alert, unread: false } : alert,
        ),
      }
    : state;
}

/** Closing or restarting a terminal discards its session's alerts. */
const forget = (state: State, panes: number[]): State =>
  state.alerts.some((alert) => panes.includes(alert.pane))
    ? { ...state, alerts: state.alerts.filter((alert) => !panes.includes(alert.pane)) }
    : state;

/** The workspaces that are reordered together: a folder's, or the top level's. */
const siblings = (state: State, workspace: Workspace): Workspace[] =>
  state.workspaces.filter((other) => other.group === workspace.group);

/** Selecting a workspace reveals it when its folder is collapsed. */
function reveal(state: State, id: number | null): State {
  const group = state.workspaces.find((workspace) => workspace.id === id)?.group;
  return state.groups.some((item) => item.id === group && item.collapsed)
    ? {
        ...state,
        groups: state.groups.map((item) =>
          item.id === group ? { ...item, collapsed: false } : item,
        ),
      }
    : state;
}

function removeWorkspace(state: State, id: number, keep: number[] = []): State {
  const index = state.workspaces.findIndex((workspace) => workspace.id === id);
  if (index < 0) return state;
  const closed = panesOf(state.workspaces[index].layout).filter((pane) => !keep.includes(pane));
  const panes = { ...state.panes };
  for (const pane of closed) delete panes[pane];
  const workspaces = state.workspaces.filter((workspace) => workspace.id !== id);
  const next = workspaces[Math.min(index, workspaces.length - 1)];
  const active = state.active === id ? (next?.id ?? null) : state.active;
  return reveal(
    forget(
      {
        ...state,
        workspaces,
        order: state.order.filter((item) => item.kind !== "workspace" || item.id !== id),
        panes,
        active,
        zoomed: state.active === id ? false : state.zoomed,
      },
      closed,
    ),
    active,
  );
}

/** Takes a terminal out of its workspace's layout; the session is untouched. */
function detach(state: State, id: number): State {
  const workspace = owner(state, id);
  if (!workspace) return state;
  const layout = remove(workspace.layout, id);
  // A workspace's last terminal takes the workspace with it.
  if (!layout) return removeWorkspace(state, workspace.id, [id]);
  const remaining = panesOf(layout);
  const next = editWorkspace(state, workspace.id, (current) => ({
    ...current,
    layout,
    // The place it left shows its next tab; a place that is gone hands focus on.
    active:
      current.active !== id
        ? current.active
        : (tabsOf(workspace.layout, id)?.panes.length ?? 1) > 1
          ? tabsOf(layout, tabsOf(workspace.layout, id)!.panes.find((tab) => tab !== id)!)!.shown
          : remaining[remaining.length - 1],
  }));
  return { ...next, zoomed: remaining.length > 1 && next.zoomed };
}

function removePane(state: State, id: number): State {
  const next = detach(state, id);
  if (next === state) return state;
  const panes = { ...next.panes };
  delete panes[id];
  return forget({ ...next, panes }, [id]);
}

/** Opens a terminal beside `source`: as a tab, or in a place of its own. */
function addPane(state: State, source: number, axis: Axis | null): State {
  const workspace = owner(state, source);
  const from = state.panes[source];
  if (!workspace || !from) return state;
  const pane = state.nextId;
  const layout =
    axis === null
      ? addTab(workspace.layout, source, pane)
      : splitAt(workspace.layout, source, pane, state.nextId + 1, axis === "vertical" ? "right" : "bottom");
  return {
    ...editWorkspace(state, workspace.id, (current) => ({ ...current, layout, active: pane })),
    // A new terminal starts in the source's directory, on its machine.
    panes: { ...state.panes, [pane]: shell(pane, from, state.shell) },
    active: workspace.id,
    zoomed: axis === null ? state.zoomed : false,
    nextId: state.nextId + 2,
  };
}

export function reduce(state: State, action: Action): State {
  switch (action.type) {
    case "load":
      return action.state;

    case "drag":
      return { ...state, drag: action.drag };

    case "release": {
      const drag = state.drag;
      if (!drag) return state;
      return drag.to
        ? reduce(state, { type: "drop", pane: drag.pane, to: drag.to })
        : { ...state, drag: null };
    }

    case "drop": {
      const { pane, to } = action;
      const settled = { ...state, drag: null };
      if (to.kind === "workspace") {
        return reduce(settled, { type: "movePane", pane, workspace: to.workspace });
      }
      const workspace = owner(state, pane);
      if (!workspace || owner(state, to.pane) !== workspace) return settled;
      const focused = (layout: Layout) =>
        editWorkspace(settled, workspace.id, (current) => ({ ...current, layout, active: pane }));
      // A terminal placed relative to itself is placed relative to the tabs
      // it shares a place with. Alone there, it is already where it would land.
      const target =
        to.pane === pane
          ? tabsOf(workspace.layout, pane)?.panes.find((tab) => tab !== pane)
          : to.pane;
      const without = remove(workspace.layout, pane);
      if (target === undefined || !without) return focused(show(workspace.layout, pane));
      const layout =
        to.kind === "beside"
          ? splitAt(without, target, pane, state.nextId, to.edge)
          : addTab(without, target, pane, to.index);
      // Dropped where it already was: keep the split and its ratio.
      if (sameArrangement(layout, workspace.layout)) return focused(show(workspace.layout, pane));
      return { ...focused(layout), nextId: state.nextId + 1 };
    }

    case "split":
      return addPane(state, action.pane, action.axis);

    case "newTab":
      return addPane(state, action.pane, null);

    case "closePane":
      return removePane(state, action.pane);

    case "focus": {
      const workspace = owner(state, action.pane);
      if (!workspace) return state;
      const read = acknowledge(state, action.pane);
      if (workspace.active === action.pane && tabsOf(workspace.layout, action.pane)?.shown === action.pane) {
        return read;
      }
      return editWorkspace(read, workspace.id, (current) => ({
        ...current,
        layout: show(current.layout, action.pane),
        active: action.pane,
      }));
    }

    case "focusDirection": {
      const workspace = activeWorkspace(state);
      if (!workspace) return state;
      const target = adjacent(workspace.layout, workspace.active, action.direction);
      return target === null ? state : reduce(state, { type: "focus", pane: target });
    }

    case "ratio": {
      const ratio = Math.min(0.9, Math.max(0.1, action.ratio));
      return {
        ...state,
        workspaces: state.workspaces.map((workspace) => ({
          ...workspace,
          layout: setRatio(workspace.layout, action.split, ratio),
        })),
      };
    }

    case "zoom": {
      const workspace = activeWorkspace(state);
      if (!workspace) return state;
      const several = workspace.layout.kind === "split";
      return { ...state, zoomed: several && !state.zoomed };
    }

    case "newWorkspace": {
      const id = state.nextWorkspace;
      const pane = state.nextId;
      const cwd = action.cwd ?? "~";
      return reveal(
        {
          ...state,
          workspaces: [
            ...state.workspaces,
            {
              id,
              // Named after its folder; the home directory is "Home".
              name: action.name ?? (cwd === "~" ? "Home" : (cwd.split("/").pop() ?? cwd)),
              cwd,
              group: action.group,
              layout: place(pane),
              active: pane,
            },
          ],
          order:
            action.group === undefined ? [...state.order, { kind: "workspace", id }] : state.order,
          panes: {
            ...state.panes,
            [pane]: shell(pane, { cwd, branch: action.branch }, state.shell),
          },
          active: id,
          zoomed: false,
          overlay: { kind: "none" },
          nextId: state.nextId + 1,
          nextWorkspace: state.nextWorkspace + 1,
        },
        id,
      );
    }

    case "connect": {
      const destination = action.destination.trim();
      if (!validDestination(destination)) return state;
      if (action.workspace === null) {
        const id = state.nextWorkspace;
        const pane = state.nextId;
        return reveal(
          {
            ...state,
            workspaces: [
              ...state.workspaces,
              {
                id,
                name: remoteLabel(destination),
                cwd: "~",
                remote: destination,
                group: action.group,
                layout: place(pane),
                active: pane,
              },
            ],
            order:
              action.group === undefined
                ? [...state.order, { kind: "workspace", id }]
                : state.order,
            panes: { ...state.panes, [pane]: shell(pane, { cwd: "~", remote: destination }) },
            active: id,
            zoomed: false,
            overlay: { kind: "none" },
            nextId: state.nextId + 1,
            nextWorkspace: state.nextWorkspace + 1,
          },
          id,
        );
      }
      // Connecting an existing workspace restarts its terminals on the host.
      const workspace = state.workspaces.find((w) => w.id === action.workspace);
      if (!workspace) return state;
      const ids = panesOf(workspace.layout);
      const panes = { ...state.panes };
      for (const pane of ids) panes[pane] = shell(pane, { cwd: "~", remote: destination });
      return forget(
        {
          ...editWorkspace(state, workspace.id, (current) => ({
            ...current,
            remote: destination,
          })),
          panes,
          overlay: { kind: "none" },
        },
        ids,
      );
    }

    case "disconnect": {
      const workspace = state.workspaces.find((w) => w.id === action.workspace);
      if (!workspace) return state;
      const ids = panesOf(workspace.layout);
      const panes = { ...state.panes };
      for (const pane of ids) panes[pane] = shell(pane, { cwd: workspace.cwd }, state.shell);
      return forget(
        {
          ...editWorkspace(state, workspace.id, (current) => ({
            ...current,
            remote: undefined,
          })),
          panes,
          overlay: { kind: "none" },
        },
        ids,
      );
    }

    case "selectWorkspace":
      return !state.workspaces.some((workspace) => workspace.id === action.workspace)
        ? state
        : state.active === action.workspace
          ? reveal(state, action.workspace)
          : reveal({ ...state, active: action.workspace, zoomed: false }, action.workspace);

    case "moveWorkspace": {
      const workspace = state.workspaces.find((w) => w.id === action.workspace);
      if (!workspace) return state;
      const row = siblings(state, workspace);
      const other = row[row.indexOf(workspace) + action.by];
      if (!other) return state;
      const trade = <T,>(list: T[], a: number, b: number) => {
        const next = [...list];
        [next[a], next[b]] = [next[b], next[a]];
        return next;
      };
      const at = (id: number) => state.order.findIndex((item) => item.kind === "workspace" && item.id === id);
      return {
        ...state,
        workspaces: trade(
          state.workspaces,
          state.workspaces.indexOf(workspace),
          state.workspaces.indexOf(other),
        ),
        // Ungrouped workspaces also hold places in the sidebar's top level.
        order: workspace.group === undefined ? trade(state.order, at(workspace.id), at(other.id)) : state.order,
      };
    }

    case "renameWorkspace": {
      const name = action.name.trim();
      return name
        ? {
            ...editWorkspace(state, action.workspace, (current) => ({ ...current, name })),
            overlay: { kind: "none" },
          }
        : state;
    }

    case "closeWorkspace":
      return { ...removeWorkspace(state, action.workspace), overlay: { kind: "none" } };

    case "movePane": {
      const source = owner(state, action.pane);
      const target = state.workspaces.find((w) => w.id === action.workspace);
      // A terminal keeps its session, so it stays on its machine.
      if (!source || !target || source.id === target.id || source.remote !== target.remote) {
        return state;
      }
      const last = panesOf(source.layout).length === 1;
      const moved = editWorkspace(detach(state, action.pane), target.id, (current) => ({
        ...current,
        // It joins the tabs of the workspace's focused terminal, as the last.
        layout: addTab(current.layout, current.active, action.pane, Number.MAX_SAFE_INTEGER),
        active: action.pane,
      }));
      // The view stays where it was; a workspace's last terminal is followed.
      return last ? reveal({ ...moved, active: target.id }, target.id) : moved;
    }

    case "newGroup": {
      const name = action.name.trim();
      if (!name) return state;
      const id = Math.max(0, ...state.groups.map((group) => group.id)) + 1;
      return {
        ...state,
        groups: [...state.groups, { id, name, collapsed: false }],
        order: [...state.order, { kind: "group", id }],
        overlay: { kind: "none" },
      };
    }

    case "renameGroup": {
      const name = action.name.trim();
      return name
        ? {
            ...state,
            groups: state.groups.map((group) =>
              group.id === action.group ? { ...group, name } : group,
            ),
            overlay: { kind: "none" },
          }
        : state;
    }

    case "collapseGroup":
      return {
        ...state,
        groups: state.groups.map((group) =>
          group.id === action.group ? { ...group, collapsed: action.collapsed } : group,
        ),
      };

    case "removeGroup": {
      const members = state.workspaces.filter((workspace) => workspace.group === action.group);
      return {
        ...state,
        groups: state.groups.filter((group) => group.id !== action.group),
        workspaces: state.workspaces.map((workspace) =>
          workspace.group === action.group ? { ...workspace, group: undefined } : workspace,
        ),
        // Its workspaces take the folder's place in the list.
        order: state.order.flatMap((item): SidebarItem[] =>
          item.kind === "group" && item.id === action.group
            ? members.map((workspace) => ({ kind: "workspace", id: workspace.id }))
            : [item],
        ),
      };
    }

    case "moveToGroup": {
      const workspace = state.workspaces.find((w) => w.id === action.workspace);
      const group = action.group ?? undefined;
      if (!workspace || workspace.group === group) return state;
      const order = state.order.filter(
        (item) => item.kind !== "workspace" || item.id !== workspace.id,
      );
      return reveal(
        {
          ...editWorkspace(state, workspace.id, (current) => ({ ...current, group })),
          order: group === undefined ? [...order, { kind: "workspace", id: workspace.id }] : order,
        },
        state.active,
      );
    }

    case "toggleSidebar":
      return { ...state, sidebar: !state.sidebar };

    case "overlay":
      // An opening sheet cancels a carried terminal.
      return { ...state, overlay: action.overlay, drag: action.overlay.kind === "none" ? state.drag : null };

    case "search":
      return {
        ...state,
        search: {
          open: action.open,
          query: action.query ?? (action.open ? state.search.query : ""),
          typed: action.typed ?? false,
        },
      };

    case "hud":
      return {
        ...state,
        hud: { keys: action.keys, label: action.label, n: (state.hud?.n ?? 0) + 1 },
      };

    case "input":
      return editPane(acknowledge(state, action.pane), action.pane, (pane) => ({
        ...pane,
        input: action.input,
      }));

    case "commit":
      return editPane(state, action.pane, (pane) => ({
        ...pane,
        lines: [
          ...pane.lines,
          { k: "ctx", cwd: pane.cwd, branch: pane.branch, host: pane.remote } as Line,
          { k: "cmd", text: pane.input, ok: pane.ok } as Line,
        ].slice(-HISTORY),
        input: "",
        prompt: false,
      }));

    case "print":
      return editPane(state, action.pane, (pane) => ({
        ...pane,
        lines: [...pane.lines, ...action.lines].slice(-HISTORY),
      }));

    case "amend":
      return editPane(state, action.pane, (pane) => ({
        ...pane,
        lines: [...pane.lines.slice(0, -1), action.line],
      }));

    case "ready":
      return editPane(state, action.pane, (pane) => ({
        ...pane,
        prompt: true,
        busy: false,
        status: "running",
        ok: action.ok ?? true,
      }));

    case "patch":
      return editPane(state, action.pane, (pane) => ({ ...pane, ...action.patch }));

    case "clear":
      return editPane(state, action.pane, (pane) => ({ ...pane, lines: [] }));

    case "restart":
      return editPane(forget(state, [action.pane]), action.pane, (pane) =>
        shell(pane.id, { cwd: pane.cwd, branch: pane.branch, remote: pane.remote }, state.shell),
      );

    case "notify":
      return state.panes[action.pane]
        ? {
            ...state,
            alerts: [
              {
                id: Math.max(0, ...state.alerts.map((alert) => alert.id)) + 1,
                pane: action.pane,
                title: action.title,
                body: action.body,
                unread: true,
                at: action.at,
              },
              ...state.alerts,
            ].slice(0, ALERTS),
          }
        : state;

    case "openAlert": {
      // Opens the alert's workspace and pane, even one hidden by zoom.
      const alert = state.alerts.find((item) => item.id === action.alert);
      const workspace = alert && owner(state, alert.pane);
      if (!alert || !workspace) return state;
      const selected = reduce(state, { type: "selectWorkspace", workspace: workspace.id });
      return {
        ...reduce(selected, { type: "focus", pane: alert.pane }),
        zoomed: false,
        overlay: { kind: "none" },
      };
    }

    case "dismissAlert":
      return { ...state, alerts: state.alerts.filter((alert) => alert.id !== action.alert) };

    case "readAlerts":
      return { ...state, alerts: state.alerts.map((alert) => ({ ...alert, unread: false })) };

    case "clearAlerts":
      return { ...state, alerts: [] };

    case "shell":
      return state.shell === action.name ? state : { ...state, shell: action.name };

    case "level":
      return { ...state, level: { text: action.text, n: (state.level?.n ?? 0) + 1 } };

    case "panel": {
      const tab = action.tab ?? state.panel.tab;
      const open = action.open ?? (action.tab ? true : !state.panel.open);
      return { ...state, panel: { open, tab } };
    }

    case "changes":
      return {
        ...state,
        changes: {
          scope: action.scope ?? state.changes.scope,
          // A new comparison lists other files; its diff starts closed.
          file: action.file !== undefined ? action.file : action.scope ? null : state.changes.file,
        },
      };

    case "workspacePatch":
      return editWorkspace(state, action.workspace, (workspace) => ({ ...workspace, ...action.patch }));

    case "spawn": {
      const pane = state.nextId;
      return {
        ...state,
        panes: {
          ...state.panes,
          [pane]: {
            id: pane,
            title: action.agent.title,
            cwd: action.cwd,
            lines: action.lines ?? [],
            input: "",
            prompt: false,
            busy: true,
            ok: true,
            status: "running",
            agent: action.agent,
          },
        },
        nextId: state.nextId + 1,
      };
    }

    case "agent":
      return editPane(state, action.pane, (pane) =>
        pane.agent ? { ...pane, agent: { ...pane.agent, ...action.patch } } : pane,
      );

    case "openAgent": {
      const pane = state.panes[action.pane];
      if (!pane?.agent) return state;
      const home = owner(state, action.pane);
      if (home) {
        const selected = reduce(state, { type: "selectWorkspace", workspace: home.id });
        return { ...reduce(selected, { type: "focus", pane: action.pane }), zoomed: false };
      }
      const workspace = state.workspaces.find((w) => w.id === pane.agent!.workspace);
      if (!workspace) return state;
      // Beside the agent that started it where that one has a tab.
      const parent = pane.agent.parent;
      const beside = parent !== undefined && panesOf(workspace.layout).includes(parent) ? parent : workspace.active;
      return reveal(
        {
          ...editWorkspace(state, workspace.id, (current) => ({
            ...current,
            layout: addTab(current.layout, beside, action.pane, Number.MAX_SAFE_INTEGER),
            active: action.pane,
          })),
          active: workspace.id,
          zoomed: false,
        },
        workspace.id,
      );
    }

    case "background": {
      const workspace = owner(state, action.pane);
      // A workspace keeps at least one terminal in view.
      if (!workspace || !state.panes[action.pane]?.agent || panesOf(workspace.layout).length < 2) {
        return state;
      }
      return reduce(detach(state, action.pane), {
        type: "agent",
        pane: action.pane,
        patch: { workspace: workspace.id },
      });
    }

    case "project": {
      const workspace = state.workspaces.find((w) => w.id === action.workspace);
      const current = state.projects[action.workspace] ?? (workspace && newProject(workspace));
      if (!current) return state;
      return {
        ...state,
        projects: { ...state.projects, [action.workspace]: { ...current, ...action.patch } },
      };
    }

    case "chat": {
      const current = state.projects[action.workspace];
      if (!current) return state;
      return {
        ...state,
        projects: {
          ...state.projects,
          [action.workspace]: { ...current, chat: [...current.chat, action.entry].slice(-CHAT) },
        },
      };
    }
  }
}

/** Agents in any workspace, those waiting for a person first, then working, then idle. */
export function agentRows(state: State): { pane: Pane; agent: Agent; workspace: Workspace }[] {
  const order: Record<Activity, number> = { permission: 0, question: 0, working: 1, idle: 2 };
  return Object.values(state.panes)
    .flatMap((pane) => {
      if (!pane.agent) return [];
      const workspace = owner(state, pane.id) ?? state.workspaces.find((w) => w.id === pane.agent!.workspace);
      return workspace ? [{ pane, agent: pane.agent, workspace }] : [];
    })
    .sort((a, b) => order[a.agent.activity] - order[b.agent.activity]);
}

/** The agents a terminal's agent started. */
export const spawnedBy = (state: State, pane: number): Pane[] =>
  Object.values(state.panes).filter((other) => other.agent?.parent === pane);

/** A minimal external store, so scripted steps read each change at once. */
export class Store {
  private listeners = new Set<() => void>();
  constructor(private state: State) {}
  get = (): State => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  dispatch = (action: Action) => {
    const next = reduce(this.state, action);
    if (next === this.state) return;
    this.state = next;
    for (const listener of this.listeners) listener();
  };
}
