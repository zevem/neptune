"use client";

// The chips a terminal's tab carries, or the toolbar for a terminal alone in
// its workspace, following `src/ui/ports.rs`, `src/ui/attached.rs` and the
// pull request and started-agent chips of `src/ui/helpers.rs`. In the app's
// order: ports, attached files, started agents, then pull requests nearest
// the close control.

import { Icon } from "../icons";
import { spawnedBy, waits, type Dispatch, type Pane, type Pull, type State } from "./model";

const CHIP =
  "flex h-[18px] shrink-0 cursor-pointer items-center gap-[3px] rounded-[5px] pr-1 pl-[5px] text-[11.5px] font-medium transition-colors duration-100";

function PullChip({ pull }: { pull: Pull }) {
  const merged = pull.state === "merged";
  const ink = merged ? "var(--ansi-5)" : pull.state === "draft" ? "var(--secondary)" : "var(--accent)";
  const checks = { none: "", pending: "running", passing: "passing", failing: "failing" }[pull.checks];
  return (
    <span
      role="link"
      title={`Open pull request #${pull.number}\n${merged ? "Merged" : `Checks ${checks || "not reported"}`}${
        pull.comments ? ` · ${pull.comments} unresolved` : ""
      }`}
      className={`${CHIP} hover:bg-[color-mix(in_srgb,currentColor_14%,transparent)]`}
      style={{ color: ink }}
    >
      <Icon name={merged ? "merged" : "pullRequest"} size={13} />
      <span className="ml-[2px]">{pull.number}</span>
      {!merged && pull.checks === "passing" && <Icon name="check" size={10} className="text-ok" />}
      {!merged && pull.checks === "failing" && <Icon name="close" size={10} className="text-danger" />}
      {!merged && pull.checks === "pending" && (
        <span className="mx-px size-[7.5px] rounded-full border-[1.5px] border-warn" />
      )}
      {!merged && pull.comments > 0 && (
        <span className="ml-0.5 flex items-center gap-[3px] text-attention">
          <Icon name="comment" size={11} />
          {pull.comments}
        </span>
      )}
    </span>
  );
}

/** A count that lists something, with its icon and a chevron. */
function CountChip({
  icon,
  count,
  label,
  attention,
  onClick,
}: {
  icon: "agents" | "paperclip";
  count: number;
  label: string;
  attention?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={(event) => {
        event.stopPropagation();
        onClick?.();
      }}
      className={`${CHIP} ${attention ? "text-attention" : "text-accent"} hover:bg-[color-mix(in_srgb,currentColor_14%,transparent)]`}
    >
      <Icon name={icon} size={13} />
      <span className="ml-[2px]">{count}</span>
      <Icon name="chevronDown" size={10} className="ml-[3px]" />
    </button>
  );
}

export function PaneChips({ pane, state, dispatch }: { pane: Pane; state: State; dispatch: Dispatch }) {
  const started = spawnedBy(state, pane.id);
  const ports = pane.ports ?? [];
  const pulls = pane.pulls ?? [];
  if (!ports.length && !pane.attached && !started.length && !pulls.length) return null;
  return (
    <span className="flex min-w-0 shrink items-center gap-px overflow-hidden">
      {ports.map((port) => (
        <span
          key={port}
          title={`Open http://localhost:${port}`}
          className="mr-[3px] flex h-5 shrink-0 cursor-pointer items-center gap-1 rounded-[6px] bg-control pr-[7px] pl-[5px] text-[11.5px] font-medium text-secondary hover:bg-hover"
        >
          <Icon name="globe" size={12} />:{port}
        </span>
      ))}
      {!!pane.attached && (
        <CountChip
          icon="paperclip"
          count={pane.attached}
          label={pane.attached === 1 ? "1 attached file" : `${pane.attached} attached files`}
        />
      )}
      {started.length > 0 && (
        <CountChip
          icon="agents"
          count={started.length}
          label="Started agents"
          attention={started.some((agent) => waits(agent.agent))}
          // The list's first entry: the agent that waits, or the first started.
          onClick={() =>
            dispatch({
              type: "openAgent",
              pane: (started.find((agent) => waits(agent.agent)) ?? started[0]).id,
            })
          }
        />
      )}
      {pulls.map((pull) => (
        <PullChip key={pull.number} pull={pull} />
      ))}
    </span>
  );
}
