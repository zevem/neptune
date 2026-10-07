import Image from "next/image";

// Neptune's own icon set, ported from `src/icons.rs`: a 24-point grid, a
// 1.5-point stroke that does not scale with the glyph, and rounded ends.

const TAU = Math.PI * 2;
const fixed = (value: number) => Number(value.toFixed(2));
const path = (points: number[][], close = false) =>
  points
    .map(([x, y], index) => `${index ? "L" : "M"}${fixed(x)} ${fixed(y)}`)
    .join("") + (close ? "Z" : "");

const gear = (() => {
  const outline: number[][] = [];
  for (let tooth = 0; tooth < 8; tooth++) {
    for (const [offset, radius] of [
      [0, 7.5],
      [0.2, 9.5],
      [0.55, 9.5],
      [0.75, 7.5],
    ]) {
      const angle = ((tooth + offset) * TAU) / 8;
      outline.push([
        12 + radius * Math.cos(angle),
        12 + radius * Math.sin(angle),
      ]);
    }
  }
  return path(outline, true);
})();

const moon = (() => {
  const crescent: number[][] = [];
  for (let step = 0; step <= 20; step++) {
    const angle = ((-93 - step * 13.2) * Math.PI) / 180;
    crescent.push([12 + 9 * Math.cos(angle), 12 + 9 * Math.sin(angle)]);
  }
  for (let step = 0; step <= 12; step++) {
    const angle = ((65.2 + step * (139.6 / 12)) * Math.PI) / 180;
    crescent.push([18 + 7.125 * Math.cos(angle), 6 + 7.125 * Math.sin(angle)]);
  }
  return path(crescent, true);
})();

const refresh = (() => {
  const ring: number[][] = [];
  for (let step = 0; step <= 24; step++) {
    const angle = ((-60 + step * 12.5) * Math.PI) / 180;
    ring.push([12 + 8 * Math.cos(angle), 12 + 8 * Math.sin(angle)]);
  }
  return path(ring);
})();

const sun = Array.from({ length: 8 }, (_, index) => {
  const angle = (index * TAU) / 8;
  const [x, y] = [Math.cos(angle), Math.sin(angle)];
  return path([
    [12 + x * 7, 12 + y * 7],
    [12 + x * 9.5, 12 + y * 9.5],
  ]);
}).join("");

const bell = (() => {
  // A dome that flares into the rim, closed along the base.
  const bezier = (from: number[], a: number[], b: number[], to: number[]) =>
    Array.from({ length: 8 }, (_, index) => {
      const t = (index + 1) / 8;
      const u = 1 - t;
      return [0, 1].map(
        (i) => u * u * u * from[i] + 3 * u * u * t * a[i] + 3 * u * t * t * b[i] + t * t * t * to[i],
      );
    });
  const outline: number[][] = [];
  for (let step = 0; step <= 12; step++) {
    const angle = Math.PI * (1 + step / 12);
    outline.push([12 + 6 * Math.cos(angle), 8.5 + 6 * Math.sin(angle)]);
  }
  outline.push(...bezier([18, 8.5], [18, 14.5], [19.5, 16], [20.5, 17.5]));
  outline.push([3.5, 17.5]);
  outline.push(...bezier([3.5, 17.5], [4.5, 16], [6, 14.5], [6, 8.5]));
  return path(outline);
})();

const star = path(
  Array.from({ length: 10 }, (_, step) => {
    const angle = ((step * 36 - 90) * Math.PI) / 180;
    const radius = step % 2 === 0 ? 9.5 : 4.4;
    return [12 + radius * Math.cos(angle), 12.9 + radius * Math.sin(angle)];
  }),
  true,
);

const paperclip = (() => {
  // One wire: down the long side, round the foot, up over the head and back
  // down into the loop it holds.
  const arc = (x: number, y: number, radius: number, from: number, to: number) =>
    Array.from({ length: 9 }, (_, step) => {
      const angle = ((from + ((to - from) * step) / 8) * Math.PI) / 180;
      return [x + radius * Math.cos(angle), y + radius * Math.sin(angle)];
    });
  return path([
    [18, 8],
    ...arc(12, 15, 6, 0, 180),
    ...arc(10.25, 7.25, 4.25, 180, 360),
    ...arc(12.25, 14.5, 2.25, 0, 180),
    [10, 9],
  ]);
})();

const frame = <rect x="3" y="4" width="18" height="16" rx="3" />;
const dot = (x: number, y: number, r: number) => (
  <circle key={`${x}-${y}`} cx={x} cy={y} r={r} fill="currentColor" stroke="none" />
);

const GLYPHS = {
  terminal: (
    <>
      <rect x="3" y="5" width="18" height="14" rx="3" />
      <path d="M7 9l3 3-3 3M13 15h4" />
    </>
  ),
  plus: <path d="M12 5v14M5 12h14" />,
  close: <path d="M6 6l12 12M18 6L6 18" />,
  chevronRight: <path d="M9 5l7 7-7 7" />,
  chevronLeft: <path d="M15 5l-7 7 7 7" />,
  chevronDown: <path d="M5 9l7 7 7-7" />,
  bell: (
    <>
      <path d={bell} />
      <path d="M10 20.5l1 1h2l1-1" />
    </>
  ),
  folder: <path d="M3 7v11l1 1h16l1-1V9l-1-1h-8l-2-3H4L3 6v1" />,
  star: <path d={star} />,
  settings: (
    <>
      <path d={gear} />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  search: (
    <>
      <circle cx="10.5" cy="10.5" r="6.5" />
      <path d="M15.3 15.3L20 20" />
    </>
  ),
  sidebar: (
    <>
      {frame}
      <path d="M8.5 4.5v15" />
    </>
  ),
  splitVertical: (
    <>
      {frame}
      <path d="M12 4.5v15" />
    </>
  ),
  splitHorizontal: (
    <>
      {frame}
      <path d="M3.5 12h17" />
    </>
  ),
  grid: (
    <>
      {[4, 14].flatMap((y) =>
        [4, 14].map((x) => (
          <rect key={`${x}-${y}`} x={x} y={y} width="6" height="6" rx="1.5" />
        )),
      )}
    </>
  ),
  check: <path d="M5 12l5 5L20 7" />,
  arrowUpRight: <path d="M6 18L18 6M7 6h11v11" />,
  arrowUp: <path d="M12 19V5M6 11l6-6 6 6" />,
  arrowDown: <path d="M12 5v14M6 13l6 6 6-6" />,
  arrowRight: <path d="M5 12h14M13 6l6 6-6 6" />,
  minus: <path d="M5 12h14" />,
  maximize: <path d="M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5" />,
  minimize: <path d="M4 9h5V4M15 4v5h5M20 15h-5v5M9 20v-5H4" />,
  command: (
    <>
      <rect x="8" y="8" width="8" height="8" />
      {[
        [3, 3],
        [16, 3],
        [3, 16],
        [16, 16],
      ].map(([x, y]) => (
        <rect key={`${x}-${y}`} x={x} y={y} width="5" height="5" rx="2.5" />
      ))}
    </>
  ),
  sun: (
    <>
      <circle cx="12" cy="12" r="4" />
      <path d={sun} />
    </>
  ),
  moon: <path d={moon} />,
  copy: (
    <>
      <rect x="8" y="8" width="12" height="13" rx="2.5" />
      <path d="M15 5V3H4L3 4v11h2" />
    </>
  ),
  ellipsis: <>{[5.5, 12, 18.5].map((x) => dot(x, 12, 1.6))}</>,
  refresh: (
    <>
      <path d={refresh} />
      <path d="M16 2.5l.2 3.1 3.2-.4" />
    </>
  ),
  pencil: <path d="M4 20l1-4.5L16 4.5 19.5 8 8.5 19 4 20zM13.5 7l3.5 3.5" />,
  clipboard: (
    <>
      <rect x="5" y="5" width="14" height="16" rx="2.5" />
      <rect x="9" y="3" width="6" height="4" rx="1.5" />
      <path d="M9 12h6M9 16h4" />
    </>
  ),
  eraser: <path d="M8 19l-4.5-4.5 10-10L20 11l-8 8H8zM8.5 9.5L15 16M12 19h8" />,
  textSize: <path d="M3 19L8 7l5 12M4.8 15h6.4M15 19l3-7 3 7M16.2 16.5h3.6" />,
  swap: <path d="M4 8h16M16 4l4 4-4 4M20 16H4M8 12l-4 4 4 4" />,
  globe: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18" />
      <ellipse cx="12" cy="12" rx="4" ry="9" />
    </>
  ),
  panelRight: (
    <>
      {frame}
      <path d="M15.5 4.5v15" />
    </>
  ),
  pullRequest: (
    <>
      <circle cx="6" cy="6" r="3" />
      <circle cx="18" cy="18" r="3" />
      <path d="M6 9v12M13 6h3l2 2v7" />
    </>
  ),
  merged: (
    <>
      <circle cx="6" cy="6" r="3" />
      <circle cx="18" cy="18" r="3" />
      <path d="M6 9v12M6 9a9 9 0 009 9" />
    </>
  ),
  comment: <path d="M7 17l-4 4V5l2-2h14l2 2v10l-2 2H7z" />,
  branch: (
    <>
      <circle cx="6" cy="5.5" r="2.5" />
      <circle cx="6" cy="18.5" r="2.5" />
      <circle cx="18" cy="7.5" r="2.5" />
      <path d="M6 8v8M18 10v1l-3 3H6" />
    </>
  ),
  paperclip: <path d={paperclip} />,
  agents: (
    <>
      <circle cx="12" cy="5.5" r="2.5" />
      <circle cx="6" cy="18" r="2.5" />
      <circle cx="18" cy="18" r="2.5" />
      <path d="M12 8v4M6 15.5v-2L7.5 12h9l1.5 1.5v2" />
    </>
  ),
  file: <path d="M13.5 3h-7L5 4.5v15L6.5 21h11l1.5-1.5v-11L13.5 3v5.5H19" />,
  warning: (
    <>
      <path d="M12 4l9 15.5H3L12 4zM12 10v4" />
      {dot(12, 16.8, 1)}
    </>
  ),
  // Site-only glyphs, drawn to the same grid and stroke.
  play: <path d="M8 5.5v13l10.5-6.5L8 5.5z" />,
  pause: <path d="M8.5 5.5v13M15.5 5.5v13" />,
  lock: (
    <>
      <rect x="5" y="10.5" width="14" height="9.5" rx="2.5" />
      <path d="M8.5 10.5V8a3.5 3.5 0 017 0v2.5" />
    </>
  ),
} as const;

export type IconName = keyof typeof GLYPHS;

export function Icon({
  name,
  size = 16,
  className,
}: {
  name: IconName;
  size?: number;
  className?: string;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      vectorEffect="non-scaling-stroke"
      aria-hidden="true"
      className={`shrink-0 [&_*]:[vector-effect:non-scaling-stroke] ${className ?? ""}`}
    >
      {GLYPHS[name]}
    </svg>
  );
}

/** The marketing mark, exported from `assets/branding/neptune-icon.png`. */
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
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="currentColor"
      aria-hidden="true"
      className="shrink-0"
    >
      <path d="M8 0a8 8 0 00-2.53 15.59c.4.07.55-.17.55-.38v-1.33c-2.23.48-2.7-1.07-2.7-1.07-.36-.93-.89-1.17-.89-1.17-.73-.5.05-.49.05-.49.8.06 1.23.83 1.23.83.72 1.22 1.87.87 2.33.66.07-.52.28-.87.5-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.6 7.6 0 014 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.28.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48v2.19c0 .21.15.46.55.38A8 8 0 008 0z" />
    </svg>
  );
}

/**
 * The marks of the CLI agents a terminal's tab can show, from
 * `assets/icons/providers`. They identify those agents only; they are
 * trademarks of their owners and are not covered by Neptune's license.
 */
export function AgentMark({ kind, size = 13, className }: { kind: "claude" | "codex"; size?: number; className?: string }) {
  return kind === "claude" ? (
    <svg width={size} height={size} viewBox="0 0 256 257" fill="currentColor" aria-hidden="true" className={`shrink-0 ${className ?? ""}`}>
      <path d="m50.228 170.321 50.357-28.257.843-2.463-.843-1.361h-2.462l-8.426-.518-28.775-.778-24.952-1.037-24.175-1.296-6.092-1.297L0 125.796l.583-3.759 5.12-3.434 7.324.648 16.202 1.101 24.304 1.685 17.629 1.037 26.118 2.722h4.148l.583-1.685-1.426-1.037-1.101-1.037-25.147-17.045-27.22-18.017-14.258-10.37-7.713-5.25-3.888-4.925-1.685-10.758 7-7.713 9.397.649 2.398.648 9.527 7.323 20.35 15.75L94.817 91.9l3.889 3.24 1.555-1.102.195-.777-1.75-2.917-14.453-26.118-15.425-26.572-6.87-11.018-1.814-6.61c-.648-2.723-1.102-4.991-1.102-7.778l7.972-10.823L71.42 0 82.05 1.426l4.472 3.888 6.61 15.101 10.694 23.786 16.591 32.34 4.861 9.592 2.592 8.879.973 2.722h1.685v-1.556l1.36-18.211 2.528-22.36 2.463-28.776.843-8.1 4.018-9.722 7.971-5.25 6.222 2.981 5.12 7.324-.713 4.73-3.046 19.768-5.962 30.98-3.889 20.739h2.268l2.593-2.593 10.499-13.934 17.628-22.036 7.778-8.749 9.073-9.657 5.833-4.601h11.018l8.1 12.055-3.628 12.443-11.342 14.388-9.398 12.184-13.48 18.147-8.426 14.518.778 1.166 2.01-.194 30.46-6.481 16.462-2.982 19.637-3.37 8.88 4.148.971 4.213-3.5 8.62-20.998 5.184-24.628 4.926-36.682 8.685-.454.324.519.648 16.526 1.555 7.065.389h17.304l32.21 2.398 8.426 5.574 5.055 6.805-.843 5.184-12.962 6.611-17.498-4.148-40.83-9.721-14-3.5h-1.944v1.167l11.666 11.406 21.387 19.314 26.767 24.887 1.36 6.157-3.434 4.86-3.63-.518-23.526-17.693-9.073-7.972-20.545-17.304h-1.36v1.814l4.73 6.935 25.017 37.59 1.296 11.536-1.814 3.76-6.481 2.268-7.13-1.297-14.647-20.544-15.1-23.138-12.185-20.739-1.49.843-7.194 77.448-3.37 3.953-7.778 2.981-6.48-4.925-3.436-7.972 3.435-15.749 4.148-20.544 3.37-16.333 3.046-20.285 1.815-6.74-.13-.454-1.49.194-15.295 20.999-23.267 31.433-18.406 19.702-4.407 1.75-7.648-3.954.713-7.064 4.277-6.286 25.47-32.405 15.36-20.092 9.917-11.6-.065-1.686h-.583L44.07 198.125l-12.055 1.555-5.185-4.86.648-7.972 2.463-2.593 20.35-13.999-.064.065Z" />
    </svg>
  ) : (
    <svg width={size} height={size} viewBox="100 100 411 411" fill="currentColor" aria-hidden="true" className={`shrink-0 ${className ?? ""}`}>
      <path fillRule="evenodd" d="M252.794 108.802C289.191 99.0484 326.265 110.305 351.148 135.135C385.113 126.072 422.85 134.862 449.492 161.505C476.136 188.149 484.925 225.888 475.862 259.85V259.854C500.696 284.735 511.95 321.81 502.198 358.207C492.447 394.602 464.161 421.084 430.215 430.217C421.083 464.162 394.603 492.448 358.206 502.199C321.812 511.951 284.734 500.693 259.852 475.864C225.887 484.927 188.15 476.137 161.507 449.495C134.864 422.851 126.073 385.111 135.136 351.149C110.304 326.266 99.0496 289.192 108.801 252.795C118.552 216.4 146.84 189.918 180.784 180.785C189.917 146.841 216.396 118.553 252.794 108.802ZM374.292 407.145C374.292 411.271 372.092 415.086 368.517 417.148L283.723 466.102C302.487 480.585 327.555 486.459 352.217 479.852C386.997 470.532 410.068 439.312 410.555 405.006V317.717C410.555 315.08 409.125 312.621 406.843 311.303L374.292 292.509V407.145ZM251.868 415.897C248.296 417.959 243.893 417.959 240.317 415.897L155.526 366.942C152.366 390.436 159.811 415.08 177.866 433.136H177.863C203.325 458.594 241.896 462.962 271.85 446.232L347.449 402.586C349.735 401.268 351.148 398.8 351.148 396.163V358.579L251.868 415.897ZM368.602 220.628C366.319 219.309 363.474 219.318 361.191 220.637L328.641 239.431L427.921 296.749C431.496 298.811 433.697 302.627 433.697 306.752V404.661C455.622 395.654 473.244 376.881 479.851 352.218C489.169 317.442 473.668 281.85 444.201 264.274L368.602 220.628ZM177.303 206.34C155.377 215.348 137.756 234.122 131.148 258.783C121.832 293.561 137.331 329.153 166.799 346.727L242.398 390.373C244.68 391.692 247.525 391.684 249.807 390.366L282.357 371.572L183.078 314.253C179.504 312.189 177.303 308.375 177.303 304.251V206.34ZM259.849 279.145V331.858L305.5 358.213L351.15 331.858V279.145L305.5 252.789L259.849 279.145ZM327.276 144.9C308.512 130.418 283.445 124.543 258.782 131.15C224.002 140.471 200.931 171.691 200.445 205.995V293.286C200.445 295.923 201.875 298.381 204.158 299.7L236.707 318.493V203.856C236.707 199.731 238.909 195.916 242.483 193.853L327.276 144.9ZM433.137 177.867C407.675 152.407 369.103 148.038 339.149 164.769L263.55 208.415C261.265 209.734 259.852 212.202 259.852 214.838V252.423L359.132 195.105C362.703 193.041 367.108 193.041 370.682 195.105L455.473 244.06C458.635 220.567 451.189 195.922 433.135 177.867H433.137Z" />
    </svg>
  );
}
