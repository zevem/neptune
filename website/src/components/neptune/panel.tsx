"use client";

// The panel at the window's trailing edge, following `src/ui/panel.rs`: the
// file explorer, the running agents, git's changes and the workspace's
// project as tabs. It slides in below the toolbar and the terminals keep the
// room beside it; a narrow window gives it the stage.

import { useEffect, useRef, useState } from "react";
import { AgentMark, Icon, type IconName } from "../icons";
import { Button, IconButton, Segmented } from "./controls";
import {
  ACTIVITY_NAMES,
  AGENT_NAMES,
  activePane,
  activeWorkspace,
  agentRows,
  waits,
  type Activity,
  type ChangedFile,
  type Dispatch,
  type PanelTab,
  type Project,
  type State,
  type Workspace,
} from "./model";
import { LISTINGS } from "./shell";

const TABS: [PanelTab, string][] = [
  ["files", "Files"],
  ["agents", "Agents"],
  ["changes", "Changes"],
  ["project", "Project"],
];

/** A short age: "now", then whole minutes and hours. */
const age = (minutes: number) =>
  minutes < 1 ? "now" : minutes < 60 ? `${Math.floor(minutes)}m` : `${Math.floor(minutes / 60)}h`;

/** Filled while an agent wants a person, a ring while it works, faint at rest. */
function Mark({ activity }: { activity: Activity }) {
  if (activity === "permission" || activity === "question") {
    return <span className="size-2 shrink-0 rounded-full bg-attention" />;
  }
  if (activity === "working") {
    return (
      <span className="grid size-2 shrink-0 place-items-center rounded-full bg-[color-mix(in_srgb,var(--green)_28%,transparent)]">
        <span className="size-[4.4px] rounded-full bg-ok" />
      </span>
    );
  }
  return <span className="size-[7px] shrink-0 rounded-full border-[1.5px] border-muted" />;
}

function Note({ children }: { children: React.ReactNode }) {
  return (
    <p className="mx-auto mt-7 max-w-[240px] px-3 text-center text-[12px] leading-[1.45] whitespace-pre-line text-muted">
      {children}
    </p>
  );
}

/* Files ------------------------------------------------------------------ */

function Tree({ path, depth, open, toggle }: { path: string; depth: number; open: Set<string>; toggle: (path: string) => void }) {
  const entries = [...(LISTINGS[path] ?? [])].sort(
    (a, b) => Number(b.endsWith("/")) - Number(a.endsWith("/")) || a.localeCompare(b),
  );
  return entries.map((entry) => {
    const folder = entry.endsWith("/");
    const name = folder ? entry.slice(0, -1) : entry;
    const full = `${path}/${name}`;
    const expanded = folder && open.has(full);
    return (
      <div key={full}>
        <button
          type="button"
          aria-expanded={folder ? expanded : undefined}
          onClick={() => folder && toggle(full)}
          className="flex h-[26px] w-full cursor-pointer items-center gap-1.5 rounded-control pr-2 text-left text-[12.5px] text-fg hover:bg-hover"
          style={{ paddingLeft: 6 + depth * 14 }}
        >
          <span className="grid w-2.5 place-items-center text-muted">
            {folder && <Icon name={expanded ? "chevronDown" : "chevronRight"} size={10} />}
          </span>
          <Icon name={folder ? "folder" : "file"} size={14} className={folder ? "text-accent" : "text-secondary"} />
          <span className="truncate">{name}</span>
        </button>
        {expanded && <Tree path={full} depth={depth + 1} open={open} toggle={toggle} />}
      </div>
    );
  });
}

function Files({ state }: { state: State }) {
  const pane = activePane(state);
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const toggle = (path: string) =>
    setOpen((current) => {
      const next = new Set(current);
      if (!next.delete(path)) next.add(path);
      return next;
    });
  if (!pane || pane.remote) return <Note>Files of the focused terminal&apos;s folder are listed here for local terminals.</Note>;
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-[30px] shrink-0 items-center pr-1 pl-3">
        <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium text-fg">
          {pane.cwd === "~" ? "Home" : pane.cwd.split("/").pop()}
        </span>
        <IconButton icon="refresh" label="Refresh" size={14} />
        <IconButton icon="plus" label="New file" size={14} />
      </div>
      <div className="mx-1.5 mt-0.5 flex h-7 shrink-0 items-center gap-[7px] rounded-control bg-control px-2.5 text-[12px] text-muted">
        <Icon name="search" size={13} />
        Search files
      </div>
      <div className="quiet-scroll mt-1.5 min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
        <Tree path={pane.cwd} depth={0} open={open} toggle={toggle} />
      </div>
    </div>
  );
}

/* Agents ----------------------------------------------------------------- */

function Agents({ state, dispatch }: { state: State; dispatch: Dispatch }) {
  const rows = agentRows(state);
  const focused = activeWorkspace(state)?.active;
  if (!rows.length) {
    return (
      <Note>
        {"No agents are running.\nStart a coding agent such as claude, codex or opencode in a terminal and it is listed here."}
      </Note>
    );
  }
  const group = (activity: Activity) =>
    activity === "working" ? "Working" : activity === "idle" ? "Idle" : "Needs input";
  return (
    <div className="quiet-scroll h-full overflow-y-auto pr-2 pb-2 pl-1.5">
      {rows.map(({ pane, agent, workspace }, index) => {
        const heading = group(agent.activity);
        const first = index === 0 || group(rows[index - 1].agent.activity) !== heading;
        const waiting = waits(agent);
        const kind = AGENT_NAMES[agent.kind];
        return (
          <div key={pane.id}>
            {first && (
              <p className="flex h-7 items-center gap-1.5 pt-1.5 pl-2 text-[11.5px] font-medium text-secondary">
                {heading}
                <span className="font-normal text-muted">
                  {rows.filter((row) => group(row.agent.activity) === heading).length}
                </span>
              </p>
            )}
            <button
              type="button"
              aria-label={`Agent ${kind}, ${ACTIVITY_NAMES[agent.activity]}, ${workspace.name}: ${agent.title}`}
              title={`${kind} in ${pane.cwd}`}
              onClick={() => dispatch({ type: "openAgent", pane: pane.id })}
              className={`my-px grid h-11 w-full cursor-pointer grid-cols-[26px_1fr_auto] items-center rounded-[7px] pr-2 text-left hover:bg-hover active:bg-pressed ${
                pane.id === focused ? "bg-fg/6" : ""
              }`}
            >
              <span className="row-span-2 grid place-items-center self-start pt-[11px]">
                <Mark activity={agent.activity} />
              </span>
              <span className="truncate pt-0.5 text-[12.5px] font-medium text-fg">{agent.title}</span>
              <span className="pl-2 text-[11px] text-muted">{age(agent.minutes)}</span>
              <span className="col-span-2 truncate text-[11.5px] text-muted">
                <span className={`font-medium ${waiting ? "text-attention" : "text-secondary"}`}>
                  {ACTIVITY_NAMES[agent.activity]}
                </span>
                {"  ·  "}
                {kind}
                {"  ·  "}
                {workspace.name}
              </span>
            </button>
          </div>
        );
      })}
    </div>
  );
}

/* Changes ---------------------------------------------------------------- */

const STATUS_INK = { M: "text-warn", A: "text-ok", D: "text-danger" } as const;

function Counts({ added, removed }: { added: number; removed: number }) {
  return (
    <span className="flex shrink-0 gap-1.5 text-[11px]">
      {added > 0 && <span className="text-ok">+{added}</span>}
      {removed > 0 && <span className="text-danger">−{removed}</span>}
    </span>
  );
}

function Diff({ file, dispatch }: { file: ChangedFile; dispatch: Dispatch }) {
  const [name, folder] = split(file.path);
  return (
    <div className="mt-1.5 flex min-h-[96px] flex-1 flex-col overflow-hidden rounded-pane bg-bg">
      <div className="flex h-8 shrink-0 items-center pr-0.5 pl-3">
        <span className={`w-3 text-center text-[11px] font-medium ${STATUS_INK[file.status]}`}>{file.status}</span>
        <span className="ml-[9px] truncate text-[12px] font-medium text-fg">{name}</span>
        <span className="ml-2 min-w-0 flex-1 truncate text-[11px] text-muted">{folder}</span>
        <IconButton
          icon="close"
          label="Close diff"
          size={13}
          onClick={() => dispatch({ type: "changes", file: null })}
        />
      </div>
      <div className="quiet-scroll min-h-0 flex-1 overflow-auto pb-1 font-mono text-[11.5px] leading-[17px]">
        {file.diff.map((line, index) => (
          <div
            key={index}
            className={`flex min-w-max pr-3 ${
              line.k === "+"
                ? "bg-[color-mix(in_srgb,var(--green)_14%,transparent)]"
                : line.k === "-"
                  ? "bg-[color-mix(in_srgb,var(--red)_14%,transparent)]"
                  : line.k === "@"
                    ? "bg-fg/5"
                    : ""
            }`}
          >
            {line.k === "@" ? (
              <span className="pl-[38px] text-muted">{line.text}</span>
            ) : (
              <>
                <span className="w-[26px] shrink-0 pr-1.5 text-right text-muted">{line.n}</span>
                <span
                  className={`w-3 shrink-0 ${line.k === "+" ? "text-ok" : line.k === "-" ? "text-danger" : ""}`}
                >
                  {line.k === "+" ? "+" : line.k === "-" ? "−" : ""}
                </span>
                <span className="whitespace-pre text-fg">{line.text}</span>
              </>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

const split = (path: string) => {
  const at = path.lastIndexOf("/");
  return at < 0 ? [path, ""] : [path.slice(at + 1), path.slice(0, at)];
};

function Changes({ state, dispatch }: { state: State; dispatch: Dispatch }) {
  const workspace = activeWorkspace(state);
  if (!workspace?.branch || workspace.remote) {
    return <Note>The focused terminal&apos;s folder is not in a git repository.</Note>;
  }
  const { scope, file } = state.changes;
  const all = workspace.changes ?? [];
  const files = scope === "working" ? all.filter((item) => !item.committed) : all;
  const ahead = all.some((item) => item.committed) ? 1 : 0;
  const shown = files.find((item) => item.path === file);
  const added = files.reduce((sum, item) => sum + item.added, 0);
  const removed = files.reduce((sum, item) => sum + item.removed, 0);
  const since = scope === "branch" ? " since main" : "";
  return (
    <div className="flex h-full flex-col px-1.5 pb-2">
      <div className="flex h-[30px] shrink-0 items-center pl-1.5">
        <Icon name="branch" size={14} className="text-secondary" />
        <span className="ml-[7px] truncate text-[12.5px] font-medium text-fg">{workspace.branch.name}</span>
        {ahead > 0 && (
          <span className="ml-2 flex items-center gap-0.5 text-[11px] text-muted">
            <Icon name="arrowUp" size={10} />
            {ahead}
          </span>
        )}
        <IconButton icon="refresh" label="Refresh changes" size={14} className="ml-auto" />
      </div>
      <div className="mt-0.5 shrink-0">
        <Segmented
          label="Compare with"
          value={scope}
          width="100%"
          options={[
            ["working", "Working tree"],
            ["branch", "Branch"],
          ]}
          onChange={(value) => dispatch({ type: "changes", scope: value })}
        />
      </div>
      <div className="flex h-[30px] shrink-0 items-center pr-2 pl-1.5 text-[11.5px] text-secondary">
        <span className="min-w-0 flex-1 truncate">
          {files.length === 0
            ? `No changes${since}`
            : `${files.length} file${files.length === 1 ? "" : "s"} changed${since}`}
        </span>
        <Counts added={added} removed={removed} />
      </div>
      {files.length === 0 ? (
        <Note>
          {scope === "working" ? "Nothing has changed since the last commit." : "This branch has nothing that main lacks."}
        </Note>
      ) : (
        <>
          <div className={`quiet-scroll min-h-0 overflow-y-auto ${shown ? "shrink" : "flex-1"}`}>
            {files.map((item) => {
              const [name, folder] = split(item.path);
              const selected = item.path === file;
              return (
                <button
                  key={item.path}
                  type="button"
                  aria-pressed={selected}
                  title={item.path}
                  onClick={() => dispatch({ type: "changes", file: selected ? null : item.path })}
                  className={`flex h-[26px] w-full cursor-pointer items-center rounded-[7px] pr-2 text-left ${
                    selected ? "bg-pressed" : "hover:bg-hover"
                  }`}
                >
                  <span className={`w-[26px] shrink-0 text-center text-[11px] font-medium ${STATUS_INK[item.status]}`}>
                    {item.status}
                  </span>
                  <span className="shrink-0 truncate text-[12.5px] text-fg">{name}</span>
                  <span className="ml-[7px] min-w-0 flex-1 truncate text-[11px] text-muted">{folder}</span>
                  <Counts added={item.added} removed={item.removed} />
                </button>
              );
            })}
          </div>
          {shown && <Diff file={shown} dispatch={dispatch} />}
        </>
      )}
    </div>
  );
}

/* Project ---------------------------------------------------------------- */

function Chat({ project, state }: { project: Project; state: State }) {
  const scroller = useRef<HTMLDivElement>(null);
  // The chat sticks to its newest entry.
  useEffect(() => {
    const element = scroller.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [project.chat.length, project.working]);
  return (
    <div ref={scroller} className="quiet-scroll min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
      {project.chat.length === 0 && (
        <p className="mt-3.5 text-[12px] text-muted">Tell the lead what you want done.</p>
      )}
      {project.chat.map((entry, index) => {
        const previous = project.chat[index - 1];
        const gap = previous && previous.k !== entry.k ? "mt-3" : "mt-1.5";
        switch (entry.k) {
          case "user":
            return (
              <div key={index} className={`flex justify-end ${index ? gap : "mt-3"}`}>
                <p className="max-w-[86%] animate-fade-in rounded-[8px] bg-control px-2.5 py-[7px] text-[12.5px] leading-[1.45] text-fg">
                  {entry.text}
                </p>
              </div>
            );
          case "lead":
            return (
              <p key={index} className={`${gap} animate-fade-in text-[12.5px] leading-[1.5] text-fg`}>
                {entry.text}
              </p>
            );
          case "tool":
            return (
              <p key={index} className="mt-1 flex animate-fade-in items-start gap-1 text-[11.5px] leading-[17px] text-muted">
                <Icon name="chevronRight" size={12} className="mt-[2.5px]" />
                {entry.text}
              </p>
            );
          case "card": {
            const member = state.panes[entry.agent];
            return (
              <div key={index} className={`${gap} animate-fade-in rounded-[8px] px-2.5 pt-2 pb-2.5 shadow-[inset_0_0_0_1px_var(--separator)]`}>
                <p className="text-[12px] font-medium text-fg">
                  Agent {entry.agent} {entry.what}
                </p>
                <p className="mt-px line-clamp-2 text-[12px] leading-[1.45] text-secondary">{entry.text}</p>
                {member && (
                  <span className="mt-2 inline-flex h-[22px] items-center rounded-[6px] bg-control px-2 text-[11.5px] font-medium text-fg">
                    Open terminal
                  </span>
                )}
              </div>
            );
          }
        }
      })}
    </div>
  );
}

function Composer({ project, workspace, dispatch }: { project: Project; workspace: Workspace; dispatch: Dispatch }) {
  const input = useRef<HTMLTextAreaElement>(null);
  const ready = project.draft.trim().length > 0;
  const send = () => {
    const text = project.draft.trim();
    if (!text) return;
    dispatch({ type: "chat", workspace: workspace.id, entry: { k: "user", text } });
    dispatch({ type: "project", workspace: workspace.id, patch: { draft: "", working: 1 } });
    // The page has no lead to ask; it answers the way the app's would begin.
    window.setTimeout(() => {
      dispatch({
        type: "chat",
        workspace: workspace.id,
        entry: {
          k: "lead",
          text: "This window is a demonstration, so no agents start here. In Neptune I would plan the work, start Claude Code or Codex in terminals of their own and report back in this chat.",
        },
      });
      dispatch({ type: "project", workspace: workspace.id, patch: { working: null } });
    }, 1200);
  };
  useEffect(() => {
    if (project.typed) input.current?.focus({ preventScroll: true });
  }, [project.typed]);
  return (
    <div className="shrink-0 px-1.5">
      <div className="relative rounded-control bg-control shadow-[inset_0_0_0_1px_var(--separator)]">
        <textarea
          ref={input}
          rows={2}
          value={project.draft}
          readOnly={!project.typed}
          aria-label="Message the lead"
          placeholder="Message the lead…"
          spellCheck={false}
          onFocus={() => dispatch({ type: "project", workspace: workspace.id, patch: { typed: true } })}
          onChange={(event) =>
            dispatch({ type: "project", workspace: workspace.id, patch: { draft: event.target.value, typed: true } })
          }
          onKeyDown={(event) => {
            event.stopPropagation();
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              send();
            }
          }}
          className="block w-full resize-none bg-transparent py-[7px] pr-[64px] pl-2.5 text-[12.5px] leading-[1.4] text-fg caret-accent outline-none select-text placeholder:text-muted"
        />
        <span className="absolute right-1 bottom-1 flex gap-1">
          <button
            type="button"
            aria-label="Attach files"
            title="Attach files"
            className="grid size-6 cursor-pointer place-items-center rounded-[6px] text-secondary hover:bg-hover hover:text-fg"
          >
            <Icon name="paperclip" size={14} />
          </button>
          <button
            type="button"
            aria-label={project.working !== null && !ready ? "Stop" : "Send"}
            onClick={send}
            className={`grid size-6 cursor-pointer place-items-center rounded-[6px] transition-colors duration-100 ${
              ready || project.working !== null ? "bg-accent text-on-accent" : "bg-pressed text-muted"
            }`}
          >
            {project.working !== null && !ready ? (
              <span className="size-2 rounded-[2px] bg-current" />
            ) : (
              <Icon name="arrowUp" size={13} />
            )}
          </button>
        </span>
      </div>
      <p className="flex h-[18px] items-center justify-between px-1 text-[11px] text-muted">
        <span className="truncate">{project.lead}</span>
        {project.working !== null && (
          <span className="shrink-0 pl-2 font-medium text-secondary">Lead at work · {project.working} s</span>
        )}
      </p>
    </div>
  );
}

function ProjectTab({ state, dispatch }: { state: State; dispatch: Dispatch }) {
  const workspace = activeWorkspace(state);
  if (!workspace) return <Note>Open a workspace to start a project.</Note>;
  if (workspace.remote) return <Note>Projects need a workspace on this computer.</Note>;
  const project = state.projects[workspace.id];
  if (!project) {
    return (
      <div className="flex flex-col items-center px-4 pt-8 text-center">
        <span className="grid size-10 place-items-center rounded-sheet bg-accent/16 text-accent">
          <Icon name="agents" size={20} />
        </span>
        <p className="mt-3.5 text-[13px] font-semibold text-fg">Put an agent in charge</p>
        <p className="mt-1 text-[12px] leading-[1.45] text-secondary">
          A lead plans the work of {workspace.name}, starts agents in terminals of their own and reports back here.
        </p>
        <Button
          kind="primary"
          className="mt-4"
          onClick={() => dispatch({ type: "project", workspace: workspace.id, patch: {} })}
        >
          New project
        </Button>
      </div>
    );
  }
  const members = project.members.flatMap((id) => (state.panes[id]?.agent ? [state.panes[id]] : []));
  return (
    <div className="flex h-full flex-col pb-1">
      <div className="flex h-[30px] shrink-0 items-center pr-1 pl-3">
        <span className="truncate text-[12.5px] font-medium text-fg">{project.name}</span>
        <span className="ml-2 min-w-0 flex-1 truncate text-[11.5px] text-muted">{project.directory}</span>
        <IconButton icon="settings" label="Project details" size={14} />
        <IconButton icon="ellipsis" label="Project menu" size={14} />
      </div>
      {project.needs.map((need) => (
        <div
          key={need.agent}
          className="mx-1.5 mb-1 flex shrink-0 animate-fade-in items-center gap-2 rounded-[8px] bg-[color-mix(in_srgb,var(--attention)_10%,transparent)] py-1.5 pr-1.5 pl-2.5"
        >
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[12px] font-medium text-fg">{need.title}</span>
            <span className="block truncate text-[11.5px] text-secondary">{need.detail}</span>
          </span>
          <button
            type="button"
            onClick={() => dispatch({ type: "openAgent", pane: need.agent })}
            className="h-[22px] shrink-0 cursor-pointer rounded-[6px] bg-accent px-2 text-[11.5px] font-medium text-on-accent hover:brightness-110"
          >
            Open
          </button>
        </div>
      ))}
      {members.length > 0 && (
        <div className="flex h-[26px] shrink-0 items-center gap-1 px-1.5">
          <div className="flex min-w-0 flex-1 gap-1 overflow-hidden">
            {members.map((member) => {
              const agent = member.agent!;
              const waiting = waits(agent);
              return (
                <button
                  key={member.id}
                  type="button"
                  aria-label={`Agent ${member.id}, ${ACTIVITY_NAMES[agent.activity]}: ${agent.title}`}
                  title={`${member.id} ${agent.title}\n${AGENT_NAMES[agent.kind]} · ${ACTIVITY_NAMES[agent.activity]}`}
                  onClick={() => dispatch({ type: "openAgent", pane: member.id })}
                  className={`flex h-[22px] max-w-[130px] min-w-0 shrink cursor-pointer items-center gap-1 rounded-[7px] pr-1.5 pl-[7px] text-[11.5px] font-medium ${
                    waiting
                      ? "bg-[color-mix(in_srgb,var(--attention)_10%,transparent)] text-fg"
                      : "text-secondary hover:bg-fg/[0.045]"
                  }`}
                >
                  <Mark activity={agent.activity} />
                  <span className="ml-0.5">{member.id}</span>
                  <span className="truncate">{agent.title}</span>
                </button>
              );
            })}
          </div>
          <span className="flex h-[22px] shrink-0 items-center gap-1 rounded-[7px] px-1.5 text-[11.5px] font-medium text-secondary">
            <Icon name="agents" size={13} />
            {members.length}
            <Icon name="chevronDown" size={9} />
          </span>
        </div>
      )}
      {(members.length > 0 || project.needs.length > 0) && <div className="mx-1.5 mt-0.5 h-px shrink-0 bg-separator" />}
      <Chat project={project} state={state} />
      <Composer project={project} workspace={workspace} dispatch={dispatch} />
    </div>
  );
}

/* The panel -------------------------------------------------------------- */

export function Panel({ state, dispatch }: { state: State; dispatch: Dispatch }) {
  const { open, tab } = state.panel;
  const waiting = agentRows(state).filter(({ agent }) => waits(agent)).length;
  const workspace = activeWorkspace(state);
  const needs = workspace ? (state.projects[workspace.id]?.needs.length ?? 0) : 0;
  const badges: Partial<Record<PanelTab, number>> = { agents: waiting, project: needs };
  const icon: Record<PanelTab, IconName> = { files: "file", agents: "agents", changes: "branch", project: "agents" };
  return (
    <aside
      aria-label="Right panel"
      inert={!open}
      className={`absolute top-11 right-0 bottom-0 z-[5] w-[300px] bg-chrome transition-transform duration-[160ms] ease-out @max-[560px]/win:w-full ${
        open ? "translate-x-0" : "translate-x-full"
      }`}
    >
      <div role="tablist" className="absolute top-0 right-2 left-1.5 flex h-[34px] gap-1 py-[3px]">
        {TABS.map(([id, label]) => {
          const selected = tab === id;
          const badge = badges[id] ?? 0;
          return (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={selected}
              aria-label={icon[id] && label}
              onClick={() => dispatch({ type: "panel", tab: id, open: true })}
              className={`flex min-w-0 flex-1 cursor-pointer items-center justify-center gap-[5px] rounded-[7px] text-[12px] font-medium transition-colors duration-100 ${
                selected ? "bg-fg/8 text-fg" : "text-secondary hover:bg-fg/[0.045]"
              }`}
            >
              <span className="truncate">{label}</span>
              {badge > 0 && (
                <span className="grid h-[15px] min-w-[15px] place-items-center rounded-full bg-[color-mix(in_srgb,var(--attention)_18%,transparent)] px-1 text-[10.5px] text-attention">
                  {badge}
                </span>
              )}
            </button>
          );
        })}
      </div>
      <div className="absolute inset-x-0 top-[34px] bottom-0">
        {tab === "files" && <Files state={state} />}
        {tab === "agents" && <Agents state={state} dispatch={dispatch} />}
        {tab === "changes" && <Changes state={state} dispatch={dispatch} />}
        {tab === "project" && <ProjectTab state={state} dispatch={dispatch} />}
      </div>
    </aside>
  );
}

/** The agent's mark, for a terminal's tab. */
export function TabMark({ kind }: { kind: "claude" | "codex" }) {
  return <AgentMark kind={kind} size={13} className="mr-[5px] text-secondary" />;
}
