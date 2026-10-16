"use client";

import { useState } from "react";
import { Icon } from "../icons";
import { REPO } from "@/lib/site";

const COMMANDS = [
  `git clone ${REPO}`,
  "cd neptune",
  "cargo run --release --locked --bin neptune",
];

export const PLATFORMS = [
  ["Linux", "Developed and verified here", "var(--green)"],
  ["macOS", "Builds and tests in CI. Native verification pending", "var(--yellow)"],
  ["Windows", "Builds and tests in CI. Native verification pending", "var(--yellow)"],
] as const;

/** A terminal pane of commands to copy, as the window draws a quiet shell. */
export function CommandPane({
  commands,
  shell = "zsh",
  cwd = "~",
  note,
}: {
  commands: readonly string[];
  shell?: string;
  cwd?: string;
  /** A comment shown under the commands, never copied with them. */
  note?: React.ReactNode;
}) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(commands.join("\n"));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      // Without clipboard access the commands stay selectable.
    }
  };
  return (
    <div className="rounded-window bg-chrome p-1.5 shadow-window">
      <div className="relative rounded-pane bg-bg shadow-[inset_0_0_0_1px_var(--separator)]">
        <div className="flex h-[30px] items-center justify-between pr-1 pl-3.5">
          <span className="pt-0.5 text-[12px] font-medium text-fg">
            {shell} <span className="ml-1.5 text-[11.5px] font-normal text-muted">{cwd}</span>
          </span>
          <button
            type="button"
            onClick={copy}
            aria-label="Copy the commands"
            className="group/copy flex h-7 cursor-pointer items-center gap-1.5 rounded-[7px] px-2 text-[11.5px] font-medium text-secondary hover:bg-hover hover:text-fg active:bg-pressed"
          >
            <Icon name={copied ? "check" : "copy"} size={13} className={copied ? "text-ok" : ""} />
            <span aria-live="polite">{copied ? "Copied" : "Copy"}</span>
          </button>
        </div>
        <pre className="term overflow-x-auto px-3 pb-3 text-fg [--term-leading:1.7] [--term-size:13.5px] select-text">
          {commands.map((command) => (
            <div key={command} className="whitespace-pre">
              <span className="mr-[1ch] select-none" style={{ color: "var(--ansi-2)" }}>
                $
              </span>
              {command}
            </div>
          ))}
          {note && <div className="mt-[0.5lh] whitespace-pre text-muted">{note}</div>}
        </pre>
      </div>
    </div>
  );
}

export function Install({ intro }: { intro?: React.ReactNode }) {
  return (
    <section
      id="install"
      className="mx-auto grid w-full max-w-[1180px] items-start gap-x-16 gap-y-10 px-5 sm:px-8 lg:grid-cols-[minmax(0,5fr)_minmax(0,7fr)]"
    >
      <div>
        <h2 className="text-[clamp(30px,4.4vw,44px)] leading-[1.05] font-semibold tracking-[-0.035em] text-balance">
          Build it.
        </h2>
        <p className="mt-4 max-w-[26rem] text-[16px] text-secondary">
          {intro ?? (
            <>
              Prefer building from source? With Rust installed, it is three commands.
              Native installers are available on the{" "}
              <a href="/download" className="underline underline-offset-4">download page</a>
              {" "}when a release is published.
            </>
          )}
        </p>
        <ul className="mt-8 space-y-3">
          {PLATFORMS.map(([name, status, color]) => (
            <li key={name} className="flex items-baseline gap-3 text-[13.5px]">
              <span
                className="size-[7px] shrink-0 translate-y-[-1px]"
                style={{ color }}
              >
                <Icon name="circleFilled" size={7} />
              </span>
              <span className="w-[68px] shrink-0 font-medium text-fg">{name}</span>
              <span className="text-secondary">{status}</span>
            </li>
          ))}
        </ul>
      </div>

      <div className="min-w-0">
        <CommandPane commands={COMMANDS} />
        <p className="mt-4 text-[12.5px] leading-[1.5] text-muted">
          Needs a graphical desktop and a working graphics driver.{" "}
          <a
            href={`${REPO}#build-and-run`}
            className="text-secondary underline decoration-separator underline-offset-2 hover:text-fg"
          >
            Build requirements for each platform
          </a>
          .
        </p>
      </div>
    </section>
  );
}
