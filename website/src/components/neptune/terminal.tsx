"use client";

import { memo, useEffect, useLayoutEffect, useRef } from "react";
import { Icon } from "../icons";
import type { Line, Pane, Span, Tone } from "./model";

const useIsomorphicLayoutEffect =
  typeof window === "undefined" ? useEffect : useLayoutEffect;

const tone = (c: Tone | undefined): string | undefined =>
  c === undefined || c === "fg"
    ? undefined
    : typeof c === "number"
      ? `var(--ansi-${c})`
      : `var(--${c})`;

/** Marks search matches, as the renderer tints matching cells. */
function highlighted(text: string, search: string): React.ReactNode {
  if (!search || !text.includes(search)) return text;
  const parts = text.split(search);
  return parts.flatMap((part, index) =>
    index < parts.length - 1 ? [part, <mark key={index}>{search}</mark>] : [part],
  );
}

function Spans({ spans, search }: { spans: Span[]; search: string }) {
  return spans.map((span, index) => (
    <span
      key={index}
      style={{ color: tone(span.c), fontWeight: span.b ? 700 : undefined }}
    >
      {span.icon && <Icon name={span.icon} size={14} className="inline-block align-[-0.15em]" />}
      {highlighted(span.t, search)}
    </span>
  ));
}

/** The official prompt glyph sits in one terminal cell. */
function Chevron({ ok }: { ok: boolean }) {
  return (
    <span
      className="inline-block w-[1ch] text-center"
      style={{ color: `var(--ansi-${ok ? 2 : 1})` }}
    >
      <Icon name="chevronRight" size={14} className="inline-block size-[0.95em] align-[-0.1em]" />
      <span className="sr-only">$</span>
    </span>
  );
}

function Context({ cwd, branch, host }: { cwd: string; branch?: string; host?: string }) {
  return (
    <div className="term-row">
      {host && <span style={{ color: "var(--ansi-5)" }}>{host} </span>}
      <span style={{ color: "var(--ansi-4)", fontWeight: 700 }}>{cwd}</span>
      {branch && <span className="text-muted"> {branch}</span>}
    </div>
  );
}

const Row = memo(function Row({ line, search }: { line: Line; search: string }) {
  if (line.k === "ctx") return <Context {...line} />;
  if (line.k === "cmd") {
    return (
      <div className="term-row">
        <Chevron ok={line.ok} /> {highlighted(line.text, search)}
      </div>
    );
  }
  return (
    <div className="term-row">
      <Spans spans={line.spans} search={search} />
    </div>
  );
});

/**
 * A pane's text. The newest rows stay in view, as a terminal follows its
 * output; the grid follows the font size and line spacing preferences.
 */
export function Terminal({ pane, search }: { pane: Pane; search: string }) {
  const scroller = useRef<HTMLDivElement>(null);

  useIsomorphicLayoutEffect(() => {
    const element = scroller.current;
    if (element) element.scrollTop = element.scrollHeight;
  });

  useEffect(() => {
    const element = scroller.current;
    if (!element) return;
    const pin = () => {
      element.scrollTop = element.scrollHeight;
    };
    const observer = new ResizeObserver(pin);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  return (
    <div ref={scroller} className="term h-full overflow-hidden">
      {pane.lines.map((line, index) => (
        <Row key={index} line={line} search={search} />
      ))}
      {pane.prompt && pane.status === "running" && (
        <>
          <Context cwd={pane.cwd} branch={pane.branch} host={pane.remote} />
          <div className="term-row">
            <Chevron ok={pane.ok} /> {pane.input}
            <span className="cursor" />
          </div>
        </>
      )}
      {!pane.prompt && pane.status === "running" && (
        // A running program holds the cursor at the end of its output.
        <div className="term-row">
          <span className="cursor" />
        </div>
      )}
    </div>
  );
}
