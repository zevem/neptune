import { doc } from "@/lib/site";
import { InView } from "./in-view";
import { PlatformKeys } from "./platform-keys";

function Card({
  title,
  children,
  visual,
  note,
  className = "",
}: {
  title: string;
  children: React.ReactNode;
  visual: React.ReactNode;
  /** Where a measurement came from. Numbers on this page carry one. */
  note?: React.ReactNode;
  className?: string;
}) {
  return (
    <article
      className={`flex flex-col rounded-window bg-chrome p-1.5 shadow-[0_0_0_1px_var(--separator)] ${className}`}
    >
      <div className="relative flex min-h-[220px] items-center justify-center overflow-hidden rounded-pane bg-bg shadow-[inset_0_0_0_1px_var(--separator)]">
        {visual}
      </div>
      <div className="px-3.5 pt-4 pb-3.5">
        <h3 className="text-[15px] font-semibold tracking-[-0.01em] text-fg">{title}</h3>
        <p className="mt-1 text-[13.5px] leading-[1.5] text-secondary">{children}</p>
        {note && <p className="mt-2.5 text-[11.5px] leading-[1.45] text-muted">{note}</p>}
      </div>
    </article>
  );
}

const measured = (
  <a
    href={doc("docs/performance.md")}
    className="underline decoration-separator underline-offset-2 hover:text-secondary"
  >
    How it was measured
  </a>
);

function Node({ name, detail }: { name: string; detail: string }) {
  return (
    <div className="flex min-w-0 flex-col items-center gap-1 rounded-control bg-control px-2.5 py-2.5 text-center sm:px-4">
      <span className="text-[13px] font-medium whitespace-nowrap text-fg">{name}</span>
      <span className="font-mono text-[10.5px] whitespace-nowrap text-muted max-sm:hidden">{detail}</span>
    </div>
  );
}

/** A connector whose dashes travel in the direction the data does. */
function Flow({ vertical = false }: { vertical?: boolean }) {
  return (
    <svg
      viewBox={vertical ? "0 0 8 28" : "0 0 28 8"}
      className={vertical ? "h-7 w-2 shrink-0" : "h-2 w-5 shrink-0 sm:w-7"}
      preserveAspectRatio="none"
      fill="none"
      aria-hidden="true"
    >
      <path
        d={vertical ? "M4 0V28" : "M0 4H28"}
        stroke="var(--accent)"
        strokeWidth="1.5"
        strokeDasharray="3 4"
        strokeLinecap="round"
        vectorEffect="non-scaling-stroke"
        className="flow"
      />
    </svg>
  );
}

function Lane({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-2.5 rounded-pane p-3 shadow-[inset_0_0_0_1px_var(--separator)]">
      <span className="text-[11px] font-medium text-muted">{label}</span>
      <div className="flex items-center gap-1.5 sm:gap-2">{children}</div>
    </div>
  );
}

function Pipeline() {
  return (
    <InView className="flex w-full flex-col items-center gap-0 px-4 py-6 md:flex-row md:justify-center md:gap-2">
      <Lane label="Worker threads">
        <Node name="PTY" detail="portable-pty" />
        <Flow />
        <Node name="Parser" detail="alacritty_terminal" />
        <Flow />
        <Node name="History" detail="bounded" />
      </Lane>
      <span className="md:hidden">
        <Flow vertical />
      </span>
      <span className="hidden md:block">
        <Flow />
      </span>
      <Lane label="Frame">
        <Node name="Snapshot" detail="viewport only" />
        <Flow />
        <Node name="GPU" detail="wgpu" />
      </Lane>
    </InView>
  );
}

function Coalescing() {
  return (
    <div className="flex w-full flex-col gap-3 px-5 py-6">
      <div>
        <div
          className="h-7 rounded-[4px] opacity-80"
          style={{
            background:
              "repeating-linear-gradient(90deg, var(--secondary) 0 1px, transparent 1px 3px)",
          }}
        />
        <p className="mt-1.5 flex justify-between font-mono text-[11px] text-muted">
          <span>terminal revisions</span>
          <span className="text-secondary">450,497</span>
        </p>
      </div>
      <div>
        <div className="flex h-7 justify-between">
          {Array.from({ length: 12 }, (_, index) => (
            <span key={index} className="w-0.5 rounded-full bg-accent" />
          ))}
        </div>
        <p className="mt-1.5 flex justify-between font-mono text-[11px] text-muted">
          <span>repaint requests</span>
          <span className="text-accent">142</span>
        </p>
      </div>
    </div>
  );
}

function Idle() {
  return (
    <div className="flex w-full flex-col px-5 py-6">
      <p className="text-[52px] leading-none font-semibold tracking-[-0.04em] text-fg tabular-nums">
        0.5<span className="text-[26px] tracking-normal text-muted">%</span>
      </p>
      <p className="mt-1.5 text-[12.5px] text-secondary">of one core, with a quiet shell open</p>
      <svg
        viewBox="0 0 300 36"
        className="mt-5 h-9 w-full"
        preserveAspectRatio="none"
        fill="none"
        aria-hidden="true"
      >
        <path
          d="M0 30H96l4-22 5 22H212l3-8 4 8H300"
          stroke="var(--accent)"
          strokeWidth="1.5"
          strokeLinejoin="round"
          strokeLinecap="round"
          vectorEffect="non-scaling-stroke"
        />
        <path d="M0 35.5H300" stroke="var(--separator)" vectorEffect="non-scaling-stroke" />
      </svg>
    </div>
  );
}

const SAVED = [
  ['"name"', '"api"'],
  ['"cwd"', '"/home/you/code/api"'],
  ['"axis"', '"vertical"'],
  ['"ratio"', "0.5"],
  ['"active"', "3"],
] as const;

function Saved() {
  return (
    <div className="w-full self-stretch px-4 py-4">
      <p className="mb-2 text-[11px] font-medium text-muted">workspaces.json</p>
      <div className="font-mono text-[12px] leading-[1.6]" role="img" aria-label="A saved workspace: its name, folder and split layout">
        {SAVED.map(([key, value]) => (
          <div key={key}>
            <span style={{ color: "var(--ansi-4)" }}>{key}</span>
            <span className="text-muted">: </span>
            <span style={{ color: `var(--ansi-${value.startsWith('"') ? 2 : 3})` }}>{value}</span>
          </div>
        ))}
        <div className="mt-2 text-muted">
          <span className="line-through decoration-danger/70">&quot;commands&quot;</span>{" "}
          <span className="line-through decoration-danger/70">&quot;memory&quot;</span>{" "}
          <span className="line-through decoration-danger/70">&quot;credentials&quot;</span>
        </div>
      </div>
    </div>
  );
}

const KEYS = [
  ["T", "New workspace"],
  ["D", "Split right"],
  ["E", "Split below"],
  ["F", "Find"],
  ["P", "Commands"],
  ["B", "Sidebar"],
] as const;

function Keys() {
  return (
    <div className="flex w-full flex-col gap-3.5 px-5 py-5">
      <p className="flex items-center gap-1.5 text-[11px] font-medium text-muted">
        <PlatformKeys name="" />
        <span>with</span>
      </p>
      <div className="grid grid-cols-3 gap-x-2 gap-y-3">
        {KEYS.map(([key, label]) => (
          <div key={key} className="flex min-w-0 flex-col items-center gap-1.5">
            <kbd className="grid size-10 place-items-center rounded-pane bg-control font-sans text-[15px] font-medium text-fg shadow-[inset_0_-1px_0_var(--separator)]">
              {key}
            </kbd>
            <span className="max-w-full truncate text-[11.5px] text-secondary">{label}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

export function Internals() {
  return (
    <section id="internals" className="mx-auto w-full max-w-[1180px] px-5 sm:px-8">
      <h2 className="text-[clamp(30px,4.4vw,44px)] leading-[1.05] font-semibold tracking-[-0.035em] text-balance">
        Native all the way down.
      </h2>
      <p className="mt-4 max-w-[30rem] text-[16px] text-secondary">
        Rust, a real PTY and a GPU renderer. No webview.
      </p>

      <div className="mt-10 grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        <Card
          title="Parsing stays off the UI thread"
          visual={<Pipeline />}
          className="md:col-span-2"
        >
          PTY reading and terminal parsing run on workers, with bounded queues and history.
          A frame draws an owned snapshot of the visible rows, without locks or live sessions.
        </Card>
        <Card
          title="Quiet terminals sleep"
          visual={<Idle />}
          note={
            <>
              Release build on Linux, one quiet zsh, 20-second sample. A single local
              observation. {measured}.
            </>
          }
        >
          A settled window does not repaint. It wakes for output, input and cursor
          deadlines.
        </Card>
        <Card
          title="Bursts become frames"
          visual={<Coalescing />}
          note={
            <>
              64 MiB through a real PTY with a simulated 120 Hz consumer. One local run on
              Linux. {measured}.
            </>
          }
        >
          Output notifications coalesce until the renderer catches up, so a flood of small
          reads does not become a flood of redraws.
        </Card>
        <Card title="Terminals keep running after quit" visual={<Saved />}>
          On macOS and Linux, shells and programs keep running when Neptune quits, and
          the next launch reattaches them. Only the layout is saved: never commands,
          process memory or terminal contents.
        </Card>
        <Card title="The shell keeps its keys" visual={<Keys />}>
          Tab, arrows and Escape go to the terminal. Every action has a shortcut and a
          place in the command palette.
        </Card>
      </div>
    </section>
  );
}
