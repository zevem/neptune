"use client";

// Sheets and popovers over the window, following `src/ui/dialogs.rs` and
// `src/ui/notifications.rs`: centred modal sheets over a dimmed window, and a
// notification popover that floats without dimming or resizing anything.

import { useEffect, useRef, useState } from "react";
import { Icon } from "../icons";
import { Button, FIELD, FIELD_FOCUSED, IconButton, Pill } from "./controls";
import {
  owner,
  panesOf,
  validDestination,
  type Close,
  type Dispatch,
  type Naming,
  type State,
} from "./model";
import { Preferences } from "./preferences";

// Workspaces take a stable identity colour from their id.
const IDENTITY = [
  "--blue",
  "--purple",
  "--emerald",
  "--orange",
  "--pink",
  "--indigo",
  "--amber",
  "--crimson",
];
const identity = (id: number) => IDENTITY[(Math.max(1, id) - 1) % IDENTITY.length];

const initial = (name: string) => (name.match(/[\p{L}\p{N}]/u)?.[0] ?? "·").toUpperCase();

const none = { type: "overlay", overlay: { kind: "none" } } as const;

export function Sheet({
  title,
  width,
  children,
}: {
  title: string;
  width: number;
  children: React.ReactNode;
}) {
  return (
    <div
      role="dialog"
      aria-label={title}
      className="absolute top-1/2 left-1/2 flex max-h-[calc(100%-24px)] -translate-1/2 flex-col"
      style={{ width: `min(${width}px, calc(100% - 24px))` }}
    >
      <div className="flex min-h-0 animate-sheet-in flex-col overflow-hidden rounded-sheet border border-edge bg-elevated shadow-sheet">
        {children}
      </div>
    </div>
  );
}

function Footer({ children }: { children: React.ReactNode }) {
  return <div className="mt-1 flex h-[62px] items-center justify-end gap-2 px-5">{children}</div>;
}

export function SshSheet({
  overlay,
  dispatch,
}: {
  overlay: Extract<State["overlay"], { kind: "ssh" }>;
  dispatch: Dispatch;
}) {
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (overlay.typed) input.current?.focus({ preventScroll: true });
  }, [overlay.typed]);
  const close = () => dispatch(none);
  const problem = overlay.host.trim() !== "" && !validDestination(overlay.host);
  const connect = () =>
    dispatch({
      type: "connect",
      workspace: overlay.workspace,
      group: overlay.group,
      destination: overlay.host,
    });
  const title = overlay.workspace === null ? "New SSH workspace" : "Connect over SSH";
  return (
    <Sheet title={title} width={420}>
      <div
        onKeyDown={(event) => {
          if (event.key === "Enter") connect();
          else if (event.key === "Escape") close();
          event.stopPropagation();
        }}
      >
        <h3 className="flex h-[52px] items-center px-5 text-[15px] font-semibold text-fg">
          {title}
        </h3>
        <div className="px-5">
          <div className={`flex h-[30px] items-center px-2.5 ${overlay.typed ? FIELD : FIELD_FOCUSED}`}>
            {overlay.typed ? (
              <input
                ref={input}
                value={overlay.host}
                onChange={(event) =>
                  dispatch({ type: "overlay", overlay: { ...overlay, host: event.target.value } })
                }
                aria-label="SSH host"
                placeholder="user@host"
                spellCheck={false}
                autoCapitalize="none"
                autoComplete="off"
                className="h-full min-w-0 flex-1 bg-transparent text-[13px] text-fg caret-accent outline-none select-text placeholder:text-muted"
              />
            ) : (
              <span className="truncate text-[13px] text-fg">
                {overlay.host || <span className="text-muted">user@host</span>}
                <span className="ml-px inline-block h-[1.15em] w-[1.5px] bg-accent align-[-0.2em]" />
              </span>
            )}
          </div>
          <p className={`mt-3 text-[12px] leading-[1.45] ${problem ? "text-danger" : "text-secondary"}`}>
            {problem
              ? "SSH host must be a destination such as user@host, without spaces or a leading dash"
              : overlay.workspace === null
                ? "Terminals open on the host using your SSH configuration and keys. Sign-in prompts appear in the terminal."
                : "Every terminal in this workspace restarts on the host. Running processes will stop."}
          </p>
        </div>
        <Footer>
          <Button onClick={close}>Cancel</Button>
          <Button kind="primary" onClick={connect}>
            Connect
          </Button>
        </Footer>
      </div>
    </Sheet>
  );
}

const NAMING: Record<Naming["kind"], [title: string, hint: string, verb: string]> = {
  newGroup: ["New workspace group", "Group name", "Create"],
  group: ["Rename workspace group", "Group name", "Save"],
  workspace: ["Rename workspace", "Workspace name", "Save"],
};

/** Names a new folder group, or renames a group or a workspace. */
export function NameSheet({
  overlay,
  dispatch,
}: {
  overlay: Extract<State["overlay"], { kind: "name" }>;
  dispatch: Dispatch;
}) {
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus({ preventScroll: true });
    input.current?.select();
  }, []);
  const [title, hint, verb] = NAMING[overlay.naming.kind];
  const close = () => dispatch(none);
  const save = () => {
    const { naming, text: name } = overlay;
    if (naming.kind === "newGroup") dispatch({ type: "newGroup", name });
    else if (naming.kind === "group") dispatch({ type: "renameGroup", group: naming.group, name });
    else dispatch({ type: "renameWorkspace", workspace: naming.workspace, name });
  };
  return (
    <Sheet title={title} width={420}>
      <div
        onKeyDown={(event) => {
          if (event.key === "Enter") save();
          else if (event.key === "Escape") close();
          event.stopPropagation();
        }}
      >
        <h3 className="flex h-[52px] items-center px-5 text-[15px] font-semibold text-fg">
          {title}
        </h3>
        <div className="px-5">
          <div className={`flex h-[30px] items-center px-2.5 ${FIELD}`}>
            <input
              ref={input}
              value={overlay.text}
              onChange={(event) =>
                dispatch({ type: "overlay", overlay: { ...overlay, text: event.target.value } })
              }
              aria-label={hint}
              placeholder={hint}
              spellCheck={false}
              autoComplete="off"
              maxLength={64}
              className="h-full min-w-0 flex-1 bg-transparent text-[13px] text-fg caret-accent outline-none select-text placeholder:text-muted"
            />
          </div>
        </div>
        <Footer>
          <Button onClick={close}>Cancel</Button>
          <Button kind="primary" disabled={!overlay.text.trim()} onClick={save}>
            {verb}
          </Button>
        </Footer>
      </div>
    </Sheet>
  );
}

/** The terminals a close would end, and how many still run a program. */
export function closing(state: State, close: Close): { panes: number[]; running: number } {
  const panes =
    close.kind === "pane"
      ? [close.pane]
      : panesOf(state.workspaces.find((w) => w.id === close.workspace)?.layout ?? { kind: "tabs", panes: [], shown: 0 });
  const running = panes.filter((id) => {
    const pane = state.panes[id];
    return pane?.status === "running" && (pane.busy || !pane.prompt);
  }).length;
  return { panes, running };
}

/** Title, consequence and confirming verb for each close target. */
const CLOSE_COPY: Record<Close["kind"], [string, string, string, string]> = {
  pane: [
    "Close terminal?",
    "Any process running in this terminal will stop.",
    "Close",
    "Closing this terminal will stop the process.",
  ],
  workspace: [
    "Close workspace?",
    "Every terminal in this workspace will close, and running processes will stop.",
    "Close",
    "Closing this workspace will close its terminals and stop their processes.",
  ],
  connection: [
    "Disconnect from SSH?",
    "Every terminal in this workspace restarts as a local shell. Processes running in them will stop.",
    "Disconnect",
    "Disconnecting will stop these connections and restart the terminals as local shells.",
  ],
};

/** Carries out a close that was confirmed, or needed no confirmation. */
export function performClose(dispatch: Dispatch, close: Close) {
  dispatch(none);
  if (close.kind === "pane") dispatch({ type: "closePane", pane: close.pane });
  else if (close.kind === "workspace") dispatch({ type: "closeWorkspace", workspace: close.workspace });
  else dispatch({ type: "disconnect", workspace: close.workspace });
}

export function ConfirmSheet({
  close,
  state,
  dispatch,
}: {
  close: Close;
  state: State;
  dispatch: Dispatch;
}) {
  const [title, consequence, verb, stopping] = CLOSE_COPY[close.kind];
  const { running } = closing(state, close);
  const message =
    running === 0
      ? consequence
      : `${
          close.kind === "pane"
            ? "A process is still running in this terminal."
            : running === 1
              ? "A process is still running in one terminal."
              : `Processes are still running in ${running} terminals.`
        } ${stopping}`;
  const cancel = () => dispatch(none);
  return (
    <Sheet title={title} width={360}>
      <div
        onKeyDown={(event) => {
          // A destructive confirmation is never the Enter default.
          if (event.key === "Escape") cancel();
          event.stopPropagation();
        }}
      >
        <div className="px-[22px] pt-[22px]">
          <h3 className="text-[15px] leading-[1.35] font-semibold text-fg">{title}</h3>
          <p className="mt-1.5 text-[13px] leading-[1.45] text-secondary">{message}</p>
        </div>
        <div className="mt-1.5 flex h-[62px] items-center justify-end gap-2 px-5">
          <Button onClick={cancel}>Cancel</Button>
          <Button kind="destructive" onClick={() => performClose(dispatch, close)}>
            {verb}
          </Button>
        </div>
      </div>
    </Sheet>
  );
}

export function SettingsSheet({ themes, dispatch }: { themes: boolean; dispatch: Dispatch }) {
  return (
    <Sheet title="Preferences" width={680}>
      <div className="h-[min(520px,calc(100cqh-26px))]">
        <Preferences themes={themes} search onClose={() => dispatch(none)} />
      </div>
    </Sheet>
  );
}

const age = (ms: number) => {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return "now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h`;
  return `${Math.floor(seconds / 86_400)}d`;
};

/**
 * Alerts from every terminal, newest first. It floats in the top trailing
 * corner, where messages do, and opening it alone marks nothing read.
 */
export function Notifications({ state, dispatch }: { state: State; dispatch: Dispatch }) {
  const [now, setNow] = useState(() => Date.now());
  // Keeps "now" honest while the popover stays open.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  const unread = state.alerts.filter((alert) => alert.unread).length;
  return (
    <div
      role="dialog"
      aria-label="Notification history"
      className="absolute top-[50px] right-[14px] z-40 flex max-h-[min(520px,calc(100%-64px))] w-[min(360px,calc(100%-28px))] animate-fade-in flex-col overflow-hidden rounded-[12px] border border-edge bg-elevated shadow-popup"
    >
      <div className="flex h-11 shrink-0 items-center border-b border-separator pr-1.5 pl-4">
        <h3 className="text-[13px] font-semibold text-fg">Notifications</h3>
        {unread > 0 && <Pill count={unread} className="ml-2" />}
        {state.alerts.length > 0 && (
          <div className="ml-auto flex">
            <Button kind="quiet" disabled={unread === 0} onClick={() => dispatch({ type: "readAlerts" })}>
              Mark all read
            </Button>
            <Button kind="quiet" onClick={() => dispatch({ type: "clearAlerts" })}>
              Clear all
            </Button>
          </div>
        )}
      </div>
      {state.alerts.length === 0 ? (
        <div className="flex h-[176px] shrink-0 flex-col items-center px-4 text-center">
          <span className="mt-[37px] grid size-[42px] place-items-center rounded-full bg-control text-secondary">
            <Icon name="bell" size={18} />
          </span>
          <p className="mt-[13px] truncate text-[13px] leading-[17px] font-medium text-fg">
            No notifications
          </p>
          <p className="mt-[3px] max-w-full truncate text-[12px] leading-[17px] text-secondary">
            Alerts from your terminals appear here.
          </p>
        </div>
      ) : (
        <div className="quiet-scroll min-h-0 overflow-y-auto p-1.5">
          {state.alerts.map((alert) => {
            const workspace = owner(state, alert.pane);
            const program = state.panes[alert.pane]?.title ?? "";
            const [headline, body] = alert.title
              ? [alert.title, alert.body]
              : [alert.body || "Notification", ""];
            const color = identity(workspace?.id ?? 1);
            return (
              <div
                key={alert.id}
                className="group/alert relative min-h-11 rounded-control hover:bg-hover active:bg-pressed"
              >
                <button
                  type="button"
                  aria-label={`${headline}, ${workspace?.name ?? ""}${alert.unread ? ", unread" : ""}`}
                  onClick={() => dispatch({ type: "openAlert", alert: alert.id })}
                  className="block w-full cursor-pointer rounded-control py-2.5 pr-3 pl-11 text-left"
                >
                  <span
                    className={`line-clamp-2 pr-7 text-[13px] leading-[17px] font-medium ${
                      alert.unread ? "text-fg" : "text-[color-mix(in_srgb,var(--fg)_30%,var(--secondary))]"
                    }`}
                  >
                    {headline}
                  </span>
                  {body && (
                    <span className="mt-0.5 line-clamp-2 text-[12px] leading-4 text-secondary">
                      {body}
                    </span>
                  )}
                  <span className="mt-1 block truncate text-[11px] leading-[14px] text-muted">
                    {program ? `${workspace?.name} · ${program}` : workspace?.name}
                  </span>
                </button>
                <span
                  aria-hidden="true"
                  className="pointer-events-none absolute top-[11px] left-2.5 grid size-6 place-items-center rounded-[7px] text-[11px] font-semibold"
                  style={{
                    background: `color-mix(in srgb, var(${color}) var(--tile-alpha), transparent)`,
                    color: `color-mix(in srgb, black var(--tile-ink-mix), var(${color}))`,
                  }}
                >
                  {initial(workspace?.name ?? "")}
                  {alert.unread && (
                    // The bell's dot, on the workspace that rang it.
                    <Icon name="circleFilled" size={7} className="absolute -top-[2.5px] -right-[2.5px] rounded-full text-attention ring-[1.5px] ring-elevated" />
                  )}
                </span>
                {/* Dismissing takes the time's place while the row is pointed at. */}
                <span className="pointer-events-none absolute top-[10px] right-3 text-[11px] leading-[17px] text-muted group-hover/alert:opacity-0">
                  {age(now - alert.at)}
                </span>
                <IconButton
                  icon="close"
                  label="Dismiss notification"
                  size={12}
                  className="absolute top-1 right-1 opacity-0 group-hover/alert:opacity-100 focus-visible:opacity-100"
                  onClick={() => dispatch({ type: "dismissAlert", alert: alert.id })}
                />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
