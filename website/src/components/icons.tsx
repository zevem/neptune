import Image from "next/image";
import {
  IconAlertTriangle,
  IconArrowDown,
  IconArrowRight,
  IconArrowUp,
  IconArrowUpRight,
  IconArrowsExchange,
  IconArrowsMaximize,
  IconArrowsMinimize,
  IconBell,
  IconBrandGithub,
  IconBrandOpenai,
  IconCheck,
  IconChevronDown,
  IconChevronLeft,
  IconChevronRight,
  IconCircle,
  IconCircleDot,
  IconCircleFilled,
  IconClipboardText,
  IconCommand,
  IconCopy,
  IconCornerDownRight,
  IconDots,
  IconEraser,
  IconFile,
  IconFolder,
  IconGitBranch,
  IconGitMerge,
  IconGitPullRequest,
  IconHierarchy,
  IconLayoutColumns,
  IconLayoutGrid,
  IconLayoutRows,
  IconLayoutSidebar,
  IconLayoutSidebarRight,
  IconLoader2,
  IconLock,
  IconMessage,
  IconMinus,
  IconMoon,
  IconPaperclip,
  IconPencil,
  IconPlayerPauseFilled,
  IconPlayerPlayFilled,
  IconPlus,
  IconRefresh,
  IconSearch,
  IconSettings,
  IconSparkles,
  IconStar,
  IconSun,
  IconTerminal2,
  IconTextSize,
  IconWorld,
  IconX,
} from "@tabler/icons-react";

// Official Tabler 3.49.0 components only: retain the upstream 24-point grid,
// rounded 2-point strokes and currentColor. Explicit imports are tree-shaken.
export const TABLER_ICONS = {
  terminal: IconTerminal2,
  plus: IconPlus,
  close: IconX,
  chevronRight: IconChevronRight,
  chevronLeft: IconChevronLeft,
  chevronDown: IconChevronDown,
  bell: IconBell,
  folder: IconFolder,
  star: IconStar,
  settings: IconSettings,
  search: IconSearch,
  sidebar: IconLayoutSidebar,
  splitVertical: IconLayoutColumns,
  splitHorizontal: IconLayoutRows,
  grid: IconLayoutGrid,
  check: IconCheck,
  arrowUpRight: IconArrowUpRight,
  arrowUp: IconArrowUp,
  arrowDown: IconArrowDown,
  arrowRight: IconArrowRight,
  minus: IconMinus,
  maximize: IconArrowsMaximize,
  minimize: IconArrowsMinimize,
  command: IconCommand,
  sun: IconSun,
  moon: IconMoon,
  copy: IconCopy,
  ellipsis: IconDots,
  refresh: IconRefresh,
  pencil: IconPencil,
  clipboard: IconClipboardText,
  eraser: IconEraser,
  textSize: IconTextSize,
  swap: IconArrowsExchange,
  globe: IconWorld,
  panelRight: IconLayoutSidebarRight,
  pullRequest: IconGitPullRequest,
  merged: IconGitMerge,
  comment: IconMessage,
  branch: IconGitBranch,
  paperclip: IconPaperclip,
  agents: IconHierarchy,
  file: IconFile,
  warning: IconAlertTriangle,
  play: IconPlayerPlayFilled,
  pause: IconPlayerPauseFilled,
  lock: IconLock,
  circle: IconCircle,
  circleDot: IconCircleDot,
  circleFilled: IconCircleFilled,
  loader: IconLoader2,
  cornerDownRight: IconCornerDownRight,
  sparkles: IconSparkles,
  brandOpenai: IconBrandOpenai,
} as const;

export type IconName = keyof typeof TABLER_ICONS;

export function Icon({
  name,
  size = 16,
  className,
}: {
  name: IconName;
  size?: number;
  className?: string;
}) {
  const Glyph = TABLER_ICONS[name];
  return <Glyph size={size} aria-hidden="true" className={`shrink-0 ${className ?? ""}`} />;
}

/** Tabler's planet, rasterized by `scripts/export-logo.py` with Neptune's colors. */
export function NeptuneMark({ size = 28 }: { size?: number }) {
  return (
    <Image
      src="/neptune-logo.png"
      width={size}
      height={size}
      alt=""
      aria-hidden="true"
      className="shrink-0"
      unoptimized
    />
  );
}

export function GitHubMark({ size = 16 }: { size?: number }) {
  return <IconBrandGithub size={size} aria-hidden="true" className="shrink-0" />;
}

/** Match the native provider glyphs; Tabler 3.49.0 has no brand-claude. */
export function AgentMark({
  kind,
  size = 13,
  className,
}: {
  kind: "claude" | "codex";
  size?: number;
  className?: string;
}) {
  return <Icon name={kind === "claude" ? "sparkles" : "brandOpenai"} size={size} className={className} />;
}
