"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Icon } from "../icons";
import { Segmented } from "../neptune/controls";
import { Glow } from "./chrome";
import { CommandPane, Install, PLATFORMS } from "./install";
import {
  PACKAGES,
  downloadPath,
  isMacDesktop,
  recommendPackage,
  type Channel,
  type PackageId,
  type Release,
} from "@/lib/releases";
import { REPO, doc } from "@/lib/site";

type System = "macOS" | "Windows" | "Linux";
const SYSTEMS = [
  ["macOS", "macOS"],
  ["Windows", "Windows"],
  ["Linux", "Linux"],
] as const satisfies readonly (readonly [System, string])[];

const systemOf = (id: PackageId): System =>
  id.startsWith("macos") ? "macOS" : id.startsWith("windows") ? "Windows" : "Linux";

/** Each package by the name it goes by under its platform. */
const NAMES: Record<PackageId, string> = {
  "macos-arm64": "Apple Silicon",
  "macos-x64": "Intel",
  "windows-x64": "Installer",
  "linux-x64-appimage": "AppImage",
  "linux-x64-deb": "DEB package",
};

const NOTES: Record<System, string> = {
  macOS: "Apple menu → About This Mac shows Chip (Apple Silicon) or Processor (Intel).",
  Windows: "The installer is currently unsigned and may show a SmartScreen warning.",
  Linux:
    "Builds target glibc 2.35+ (Ubuntu 22.04+). The AppImage includes window libraries; graphics drivers stay on your system.",
};

const megabytes = (bytes: number) => `${(bytes / 1024 ** 2).toFixed(1)} MB`;

const heading =
  "text-[clamp(30px,4.4vw,44px)] leading-[1.05] font-semibold tracking-[-0.035em] text-balance";
const primary =
  "flex h-10 items-center gap-2 rounded-pane bg-accent pr-4 pl-[18px] text-[14px] font-medium text-on-accent transition-[filter] duration-100 hover:brightness-110 active:brightness-90";
const secondary =
  "flex h-10 cursor-pointer items-center gap-2 rounded-pane bg-control px-4 text-[14px] font-medium text-fg transition-colors duration-100 hover:bg-pressed";
const quietLink =
  "text-secondary underline decoration-separator underline-offset-2 transition-colors duration-100 hover:text-fg";

/** Stable and Beta are separate pages, switched the way the app switches a setting. */
function ChannelSwitch({ channel }: { channel: Channel }) {
  const channels = [
    ["stable", "Stable", "/download"],
    ["beta", "Beta", "/download/beta"],
  ] as const;
  return (
    <nav
      aria-label="Release channel"
      className="relative flex h-8 w-[196px] rounded-control bg-control p-0.5"
    >
      <span
        aria-hidden="true"
        className="absolute top-0.5 bottom-0.5 left-0.5 w-[calc((100%-4px)/2)] rounded-[6px] bg-[var(--thumb)] shadow-[0_1px_0_var(--thumb-shadow)]"
        style={{ transform: channel === "beta" ? "translateX(100%)" : undefined }}
      />
      {channels.map(([id, label, href]) => (
        <Link
          key={id}
          href={href}
          aria-current={channel === id ? "page" : undefined}
          className={`relative flex flex-1 items-center justify-center rounded-[6px] text-[12.5px] font-medium transition-colors duration-100 hover:text-fg ${
            channel === id ? "text-fg" : "text-secondary"
          }`}
        >
          {label}
        </Link>
      ))}
    </nav>
  );
}

function Hero({
  channel,
  stableAvailable,
  children,
}: {
  channel: Channel;
  stableAvailable: boolean;
  children: React.ReactNode;
}) {
  return (
    <section
      id="top"
      className="relative mx-auto flex w-full max-w-[1180px] flex-col items-center px-5 pt-12 sm:px-8 sm:pt-16"
    >
      <Glow className="top-[45%] h-[560px]" />
      {stableAvailable && (
        <div className="animate-rise">
          <ChannelSwitch channel={channel} />
        </div>
      )}
      <h1 className="mt-8 max-w-[15ch] animate-rise text-center text-[clamp(40px,6.6vw,72px)] leading-[0.98] font-semibold tracking-[-0.045em] text-balance [animation-delay:40ms]">
        {channel === "beta" ? "Try Neptune Beta." : "Download Neptune."}
      </h1>
      <p className="mt-6 max-w-[34rem] animate-rise text-center text-[clamp(16px,1.6vw,18px)] leading-[1.5] text-secondary [animation-delay:80ms]">
        {channel === "beta"
          ? `Early builds of what comes next. Beta builds may have rough edges${
              stableAvailable ? "; Stable is recommended for everyday work." : "."
            }`
          : "A native terminal for macOS, Windows and Linux. Real shells, GPU rendering and a quiet place to work."}
      </p>
      <div className="mt-8 flex w-full animate-rise flex-col items-center [animation-delay:120ms]">
        {children}
      </div>
    </section>
  );
}

interface Detected {
  pkg: PackageId | null;
  mac: boolean;
}

/** The one download this device most likely wants, or an honest question. */
function Primary({
  channel,
  release,
  detected,
  recommended,
  choose,
}: {
  channel: Channel;
  release: Release;
  detected: Detected | null;
  recommended: PackageId | null;
  choose: (pkg: PackageId) => void;
}) {
  // Holds the space until the browser has said what it runs on.
  if (!detected) return <div className="h-[76px]" aria-hidden="true" />;

  if (recommended) {
    const pkg = PACKAGES.find(({ id }) => id === recommended)!;
    const asset = release.assets.find(({ id }) => id === recommended)!;
    const other = recommended === "macos-arm64" ? "macos-x64" : "macos-arm64";
    return (
      <div className="flex animate-fade-in flex-col items-center">
        <div className="flex flex-wrap items-center justify-center gap-2.5">
          <a className={primary} href={downloadPath(channel, pkg.id, release.tag)}>
            Download for {systemOf(pkg.id)}
            <Icon name="arrowDown" size={14} />
          </a>
          <a className={secondary} href="#all">
            All downloads
          </a>
        </div>
        <p className="mt-4 text-center text-[13px] text-muted">
          Neptune {release.version} for {pkg.label} · {megabytes(asset.size)}
          {systemOf(pkg.id) === "macOS" && (
            <>
              {" · "}
              <button type="button" className={`cursor-pointer ${quietLink}`} onClick={() => choose(other)}>
                {other === "macos-x64" ? "Intel Mac?" : "Apple Silicon Mac?"}
              </button>
            </>
          )}
        </p>
      </div>
    );
  }

  if (detected.mac) {
    return (
      <div className="flex animate-fade-in flex-col items-center">
        <p className="text-[14px] font-medium text-fg">Which chip does your Mac have?</p>
        <div className="mt-3 flex flex-wrap justify-center gap-2.5">
          <button type="button" className={secondary} onClick={() => choose("macos-arm64")}>
            Apple Silicon
          </button>
          <button type="button" className={secondary} onClick={() => choose("macos-x64")}>
            Intel
          </button>
        </div>
        <p className="mt-3 text-center text-[13px] text-muted">{NOTES.macOS}</p>
      </div>
    );
  }

  return (
    <div className="flex animate-fade-in flex-col items-center">
      <a className={primary} href="#all">
        Choose your download
        <Icon name="arrowDown" size={14} />
      </a>
      <p className="mt-4 text-center text-[13px] text-muted">
        Neptune {release.version} for macOS, Windows and Linux
      </p>
    </div>
  );
}

function PackageRow({
  id,
  channel,
  release,
  recommended,
}: {
  id: PackageId;
  channel: Channel;
  release: Release;
  recommended: boolean;
}) {
  const pkg = PACKAGES.find((candidate) => candidate.id === id)!;
  const asset = release.assets.find((candidate) => candidate.id === id)!;
  return (
    <li
      className={`rounded-control p-3 transition-colors duration-150 ${
        recommended
          ? "bg-[color-mix(in_srgb,var(--accent)_9%,transparent)] shadow-[inset_0_0_0_1px_color-mix(in_srgb,var(--accent)_32%,transparent)]"
          : ""
      }`}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <p className="flex flex-wrap items-baseline gap-x-2 text-[14px] font-medium text-fg">
            {NAMES[id]}
            {recommended && <span className="text-[11.5px] text-accent">Recommended</span>}
          </p>
          <p className="mt-0.5 text-[12.5px] text-secondary">
            {pkg.detail} · <span className="whitespace-nowrap">{megabytes(asset.size)}</span>
          </p>
        </div>
        <a
          href={downloadPath(channel, id, release.tag)}
          aria-label={`Download ${pkg.label}`}
          className={`flex h-8 shrink-0 items-center gap-1.5 rounded-control px-3 text-[13px] font-medium transition-[background-color,filter] duration-100 ${
            recommended
              ? "bg-accent text-on-accent hover:brightness-110 active:brightness-90"
              : "bg-control text-fg hover:bg-pressed"
          }`}
        >
          <Icon name="arrowDown" size={13} />
          Download
        </a>
      </div>
      <details className="group mt-2">
        <summary className="flex w-fit cursor-pointer list-none items-center gap-1 rounded-cap text-[11.5px] font-medium text-muted transition-colors duration-100 hover:text-secondary [&::-webkit-details-marker]:hidden">
          <Icon
            name="chevronRight"
            size={11}
            className="transition-transform duration-150 group-open:rotate-90"
          />
          SHA-256
        </summary>
        <code className="mt-1.5 block font-mono text-[11px] leading-[1.55] break-all text-secondary select-text">
          {asset.sha256}
        </code>
      </details>
    </li>
  );
}

function PlatformCard({
  system,
  channel,
  release,
  recommended,
}: {
  system: System;
  channel: Channel;
  release: Release;
  recommended: PackageId | null;
}) {
  const [, status, color] = PLATFORMS.find(([name]) => name === system)!;
  return (
    <article className="flex min-w-0 flex-col rounded-window bg-chrome p-1.5 shadow-[0_0_0_1px_var(--separator)] md:max-lg:last:col-span-2">
      <div className="flex-1 rounded-pane bg-bg p-1.5 shadow-[inset_0_0_0_1px_var(--separator)]">
        <h3 className="px-3 pt-2.5 pb-2 text-[17px] font-semibold tracking-[-0.015em] text-fg">
          {system}
        </h3>
        <ul className="flex flex-col gap-1">
          {PACKAGES.filter(({ id }) => systemOf(id) === system).map(({ id }) => (
            <PackageRow
              key={id}
              id={id}
              channel={channel}
              release={release}
              recommended={id === recommended}
            />
          ))}
        </ul>
      </div>
      <div className="px-3.5 pt-3.5 pb-3 text-[12.5px] leading-[1.5]">
        <p className="flex items-baseline gap-2 text-secondary">
          <span
            className="size-[7px] shrink-0 translate-y-[-1px]"
            style={{ color }}
          >
            <Icon name="circleFilled" size={7} />
          </span>
          {status}
        </p>
        <p className="mt-1.5 text-muted">{NOTES[system]}</p>
      </div>
    </article>
  );
}

/** Splits a changelog section into headings, list items and paragraphs. */
function parseNotes(notes: string) {
  const blocks: { kind: "heading" | "item" | "text"; text: string }[] = [];
  let open: (typeof blocks)[number] | null = null;
  for (const raw of notes.split("\n")) {
    const line = raw.trim();
    if (!line) {
      open = null;
    } else if (line.startsWith("#")) {
      open = null;
      const text = line.replace(/^#+\s*/, "");
      // The section's own title is the heading beside it.
      if (text.toLowerCase() !== "what's new") blocks.push({ kind: "heading", text });
    } else if (/^[-*] /.test(line)) {
      open = { kind: "item", text: line.slice(2) };
      blocks.push(open);
    } else if (open) {
      // A wrapped line continues its item or paragraph.
      open.text += ` ${line}`;
    } else {
      open = { kind: "text", text: line };
      blocks.push(open);
    }
  }
  return blocks;
}

/** Text with its `code` spans set in the terminal face. */
function Inline({ text }: { text: string }) {
  return text.split("`").map((part, index) =>
    index % 2 ? (
      <code key={index} className="rounded-cap bg-control px-1 py-px font-mono text-[0.88em] text-fg">
        {part}
      </code>
    ) : (
      part
    ),
  );
}

function Notes({ notes }: { notes: string }) {
  const blocks = parseNotes(notes);
  const out: React.ReactNode[] = [];
  for (let index = 0; index < blocks.length; ) {
    const block = blocks[index];
    if (block.kind === "item") {
      const items = [];
      while (blocks[index]?.kind === "item") items.push(blocks[index++]);
      out.push(
        <ul key={out.length} className="space-y-3">
          {items.map((item, key) => (
            <li key={key} className="relative pl-4">
              <span className="absolute top-[0.68em] left-0.5 size-[5px] rounded-full bg-accent" />
              <Inline text={item.text} />
            </li>
          ))}
        </ul>,
      );
      continue;
    }
    out.push(
      block.kind === "heading" ? (
        <h3 key={out.length} className="pt-3 text-[13px] font-semibold tracking-[-0.005em] text-fg">
          {block.text}
        </h3>
      ) : (
        <p key={out.length}>
          <Inline text={block.text} />
        </p>
      ),
    );
    index++;
  }
  return <div className="space-y-4 text-[14px] leading-[1.6] text-secondary">{out}</div>;
}

function LinkList({ links }: { links: readonly (readonly [string, string, "arrowUpRight" | "arrowDown"])[] }) {
  return (
    <ul className="mt-8 space-y-3">
      {links.map(([label, href, icon]) => (
        <li key={label}>
          <a
            href={href}
            className="inline-flex items-center gap-2 text-[13.5px] font-medium text-fg transition-colors duration-100 hover:text-accent"
          >
            {label}
            <Icon name={icon} size={13} className="text-muted" />
          </a>
        </li>
      ))}
    </ul>
  );
}

const twoColumns =
  "mx-auto grid w-full max-w-[1180px] items-start gap-x-16 gap-y-10 px-5 sm:px-8 lg:grid-cols-[minmax(0,5fr)_minmax(0,7fr)]";

function WhatsNew({ release }: { release: Release }) {
  return (
    <section id="whats-new" className={twoColumns}>
      <div className="lg:sticky lg:top-24">
        <h2 className={heading}>What&apos;s new.</h2>
        <p className="mt-4 max-w-[26rem] text-[16px] text-secondary">
          Neptune {release.version}
          {release.prerelease ? ", a beta release" : ""}. Notes from the changelog shipped
          with this build.
        </p>
        <LinkList
          links={[
            ["GitHub release", release.url, "arrowUpRight"],
            ["Source for this build", release.sourceUrl, "arrowUpRight"],
          ]}
        />
      </div>
      <div className="min-w-0 rounded-window bg-chrome p-1.5 shadow-window">
        <div className="rounded-pane bg-bg px-5 py-5 shadow-[inset_0_0_0_1px_var(--separator)] sm:px-7 sm:py-6">
          <Notes notes={release.notes} />
        </div>
      </div>
    </section>
  );
}

/** The provenance check, wrapped with the shell's own line continuation. */
const attest = (file: string, continuation: string) =>
  [
    `gh attestation verify ${file}`,
    "--repo zevem/neptune",
    "--signer-workflow zevem/neptune/.github/workflows/release.yml",
  ].join(` ${continuation}\n    `);

function Verify({ release, recommended }: { release: Release; recommended: PackageId | null }) {
  const [picked, setPicked] = useState<System | null>(null);
  const system = picked ?? (recommended ? systemOf(recommended) : "macOS");
  const id =
    recommended && systemOf(recommended) === system
      ? recommended
      : PACKAGES.find((pkg) => systemOf(pkg.id) === system)!.id;
  const asset = release.assets.find((candidate) => candidate.id === id)!;
  const file = asset.name;
  const pane =
    system === "Windows"
      ? {
          shell: "pwsh",
          cwd: "~\\Downloads",
          commands: [
            `Get-FileHash .\\${file} -Algorithm SHA256`,
            attest(`.\\${file}`, "`"),
          ],
          note: `# expect\n# ${asset.sha256.toUpperCase()}`,
        }
      : system === "macOS"
        ? {
            shell: "zsh",
            cwd: "~/Downloads",
            commands: [`shasum -a 256 ${file}`, attest(`./${file}`, "\\")],
            note: `# expect\n# ${asset.sha256}`,
          }
        : {
            shell: "bash",
            cwd: "~/Downloads",
            commands: [
              "sha256sum --ignore-missing -c SHA256SUMS",
              attest(`./${file}`, "\\"),
            ],
            note: `# expect ${file}: OK`,
          };
  return (
    <section id="verify" className={twoColumns}>
      <div>
        <h2 className={heading}>Check it.</h2>
        <p className="mt-4 max-w-[26rem] text-[16px] text-secondary">
          A checksum shows the download arrived intact. An attestation shows it was built
          from this repository by the release workflow.
        </p>
        <LinkList
          links={[
            ["SHA256SUMS", release.checksumsUrl, "arrowDown"],
            ["How to verify a download", doc("docs/releases.md#verify-a-download"), "arrowUpRight"],
          ]}
        />
      </div>
      <div className="min-w-0">
        <div className="mb-3 flex items-center justify-between gap-4">
          <Segmented label="Platform" value={system} options={SYSTEMS} onChange={setPicked} width={240} />
          <span className="truncate font-mono text-[11.5px] text-muted max-sm:hidden">{file}</span>
        </div>
        <CommandPane shell={pane.shell} cwd={pane.cwd} commands={pane.commands} note={pane.note} />
        <p className="mt-4 text-[12.5px] leading-[1.5] text-muted">
          The attestation check uses the{" "}
          <a href="https://cli.github.com" className={quietLink}>
            GitHub CLI
          </a>
          .
        </p>
      </div>
    </section>
  );
}

function Empty({
  channel,
  stableAvailable,
  unavailable,
}: {
  channel: Channel;
  stableAvailable: boolean;
  unavailable: boolean;
}) {
  const page = channel === "beta" ? "/download/beta" : "/download";
  const [title, text, action] = unavailable
    ? [
        "Downloads are temporarily unavailable",
        "Release details could not be loaded and verified. Try again shortly, or visit GitHub Releases.",
        <a key="retry" className={primary} href={page}>
          Try again
          <Icon name="refresh" size={14} />
        </a>,
      ]
    : channel === "beta" && stableAvailable
      ? [
          "No beta release published yet",
          "Beta builds appear here once they are published. Stable is the recommended download.",
          <Link key="stable" className={primary} href="/download">
            Get Stable
            <Icon name="arrowRight" size={14} />
          </Link>,
        ]
      : [
          `No ${channel} release published yet`,
          "Releases appear here once they are published. You can build Neptune from source today.",
          <a key="build" className={primary} href="#install">
            Build from source
            <Icon name="arrowDown" size={14} />
          </a>,
        ];
  return (
    <div role="status" className="mt-4 w-full max-w-[560px] rounded-window bg-chrome p-1.5 shadow-window">
      <div className="flex flex-col items-center rounded-pane bg-bg px-6 pt-10 pb-8 text-center shadow-[inset_0_0_0_1px_var(--separator)]">
        <span className="grid size-11 place-items-center rounded-pane bg-control text-secondary">
          <Icon name={unavailable ? "globe" : "terminal"} size={20} />
        </span>
        <h2 className="mt-5 text-[19px] font-semibold tracking-[-0.02em] text-fg">{title}</h2>
        <p className="mt-2 max-w-[24rem] text-[14px] leading-[1.55] text-secondary">{text}</p>
        <div className="mt-7 flex flex-wrap justify-center gap-2.5">
          {action}
          <a className={secondary} href={`${REPO}/releases`}>
            GitHub Releases
            <Icon name="arrowUpRight" size={14} />
          </a>
        </div>
      </div>
    </div>
  );
}

export function Downloads({
  channel,
  stableAvailable,
  release,
  unavailable,
}: {
  channel: Channel;
  stableAvailable: boolean;
  release: Release | null;
  unavailable: boolean;
}) {
  const [detected, setDetected] = useState<Detected | null>(null);
  const [chosen, setChosen] = useState<PackageId | null>(null);
  useEffect(() => {
    let active = true;
    async function detect() {
      const ua = navigator.userAgent;
      const data = (
        navigator as Navigator & {
          userAgentData?: {
            getHighEntropyValues(
              hints: string[],
            ): Promise<{ architecture?: string; bitness?: string }>;
          };
        }
      ).userAgentData;
      const hints = await data
        ?.getHighEntropyValues(["architecture", "bitness"])
        .catch(() => undefined);
      if (active) {
        setDetected({
          mac: isMacDesktop(ua, navigator.maxTouchPoints),
          pkg: recommendPackage(ua, hints?.architecture, hints?.bitness, navigator.maxTouchPoints),
        });
      }
    }
    void detect();
    return () => {
      active = false;
    };
  }, []);
  const recommended = chosen ?? detected?.pkg ?? null;

  if (!release) {
    return (
      <>
        <Hero channel={channel} stableAvailable={stableAvailable}>
          <Empty channel={channel} stableAvailable={stableAvailable} unavailable={unavailable} />
        </Hero>
        <div className="mt-32 sm:mt-40">
          <Install
            intro={
              <>
                Neptune builds from source on macOS, Windows and Linux. With Rust installed,
                it is three commands.
              </>
            }
          />
        </div>
      </>
    );
  }

  return (
    <>
      <Hero channel={channel} stableAvailable={stableAvailable}>
        <Primary
          channel={channel}
          release={release}
          detected={detected}
          recommended={recommended}
          choose={setChosen}
        />
      </Hero>
      <section
        id="all"
        aria-label="All downloads"
        className="mx-auto mt-14 w-full max-w-[1180px] animate-rise px-5 [animation-delay:180ms] sm:mt-16 sm:px-8"
      >
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {SYSTEMS.map(([system]) => (
            <PlatformCard
              key={system}
              system={system}
              channel={channel}
              release={release}
              recommended={recommended}
            />
          ))}
        </div>
        <p className="mt-6 text-center text-[12.5px] text-muted">
          Every package needs a graphical desktop and a working graphics driver.
        </p>
      </section>
      <div className="mt-32 flex flex-col gap-32 sm:mt-40 sm:gap-40">
        <WhatsNew release={release} />
        <Verify release={release} recommended={recommended} />
      </div>
    </>
  );
}

/** The page's shape while release details load, so nothing jumps when they arrive. */
export function DownloadsLoading({ channel }: { channel: Channel }) {
  return (
    <>
      <Hero channel={channel} stableAvailable={false}>
        <div role="status" className="flex h-[76px] flex-col items-center">
          <div className="h-10 w-[300px] max-w-full rounded-pane bg-control" />
          <span className="sr-only">Loading downloads</span>
        </div>
      </Hero>
      <div className="mx-auto mt-14 grid w-full max-w-[1180px] gap-4 px-5 sm:mt-16 sm:px-8 md:grid-cols-2 lg:grid-cols-3">
        {SYSTEMS.map(([system]) => (
          <div key={system} className="h-[300px] rounded-window bg-chrome shadow-[0_0_0_1px_var(--separator)]" />
        ))}
      </div>
    </>
  );
}
