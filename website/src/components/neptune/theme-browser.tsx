"use client";

// The theme catalog, a screen of the Preferences sheet. It follows
// `src/ui/theme_browser.rs`: cards are miniatures of the window in each
// theme's own colours, and only the rows in view are laid out. Custom themes
// and their editor belong to the app and are not rebuilt here.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Icon, type IconName } from "../icons";
import { usePrefs } from "../prefs";
import { Button, IconButton, SearchInput, Segmented } from "./controls";
import { BUILTINS, hex, isDark, lookOf, mix, type Look, type Theme } from "./themes";

const TILE_HEIGHT = 96;
const CARD_HEIGHT = TILE_HEIGHT + 32;
const CARD_GAP = 12;
const LABEL_HEIGHT = 30;

/**
 * A miniature of the window in a theme's own materials: the chrome and one
 * pane of sample text. Illustrative content only, never terminal output.
 */
export function ThemePreview({ look, className = "" }: { look: Look; className?: string }) {
  const [, red, green, yellow, blue, magenta, cyan] = look.ansi.map(hex);
  const fg = hex(look.terminalFg);
  return (
    <span
      aria-hidden="true"
      className={`relative block overflow-hidden rounded-[8px] ${className}`}
      style={{ background: hex(look.chrome) }}
    >
      {[0, 1, 2].map((index) => (
        <span
          key={index}
          className="absolute top-[6.5px] size-1"
          style={{ left: 8 + index * 7, color: hex(mix(look.chrome, look.fg, 0.26)) }}
        >
          <Icon name="circleFilled" size={4} />
        </span>
      ))}
      <span
        className="absolute top-[17px] right-1 bottom-1 left-1 overflow-hidden rounded-[5px] pt-1.5 pl-[9px] font-mono text-[9.5px] leading-[14px] whitespace-pre"
        style={{ background: hex(look.bg), color: fg }}
      >
        <span className="block">
          <span style={{ color: blue }}>~/neptune</span>
          <span style={{ color: magenta }}> main</span>
        </span>
        <span className="block">
          <span style={{ color: green }}>$</span> git status
        </span>
        <span className="block">
          <span style={{ color: yellow }}>M</span> theme.rs
        </span>
        {/* The colours sit along the pane's bottom edge; text takes the rest. */}
        <span className="absolute bottom-[6px] left-[9px] flex gap-1" style={{ background: hex(look.bg) }}>
          {[red, green, yellow, blue, magenta, cyan].map((color, index) => (
            <span key={index} className="size-1.5 rounded-full" style={{ background: color }} />
          ))}
        </span>
      </span>
    </span>
  );
}

type Filter = "all" | "dark" | "light" | "custom" | "favorites";

type Row =
  | { kind: "label"; top: number; title: string; count: number }
  | { kind: "cards"; top: number; themes: Theme[]; pinned: boolean };

function Card({
  theme,
  look,
  selected,
  favorite,
  onUse,
  onStar,
}: {
  theme: Theme;
  look: Look;
  selected: boolean;
  favorite: boolean;
  onUse: () => void;
  onStar: () => void;
}) {
  const star = favorite ? "Remove from favorites" : "Add to favorites";
  return (
    <div className="group/card relative min-w-0 flex-1" style={{ height: CARD_HEIGHT }}>
      <button
        type="button"
        role="radio"
        aria-checked={selected}
        aria-label={`${theme.name} theme`}
        title={theme.name}
        onClick={onUse}
        className="peer absolute inset-0 cursor-pointer rounded-[8px] text-left outline-none"
      >
        <ThemePreview look={look} className="absolute! inset-x-0 top-0 h-24" />
        {/* The ring stands clear of the tile, as the focused pane's does. */}
        <span
          className={`absolute inset-x-0 top-0 h-24 rounded-[8px] ${
            selected
              ? "-inset-x-[3px] -top-[3px] h-[102px] rounded-[11px] shadow-[inset_0_0_0_2px_var(--accent)]"
              : "shadow-[inset_0_0_0_1px_var(--border)] group-hover/card:shadow-[inset_0_0_0_1px_var(--muted)]"
          } [button:focus-visible>&]:ring-[1.5px] [button:focus-visible>&]:ring-accent`}
        />
        <span
          className={`absolute top-[104px] left-0.5 block truncate text-[12.5px] leading-[18px] font-medium group-hover/card:text-fg ${
            selected ? "right-[50px] text-fg" : "right-7 text-secondary"
          }`}
        >
          {theme.name}
        </span>
      </button>
      {selected && (
        <span className="pointer-events-none absolute top-[105px] right-[26px] grid size-4 place-items-center rounded-full bg-accent text-on-accent">
          <Icon name="check" size={9} />
        </span>
      )}
      {/* A favorite's star is always shown, in the accent; any other card
          shows a quiet one on hover or focus. */}
      <button
        type="button"
        role="checkbox"
        aria-checked={favorite}
        aria-label={`Favorite ${theme.name} theme`}
        title={star}
        onClick={onStar}
        className={`absolute top-[102px] -right-px grid size-[22px] cursor-pointer place-items-center rounded-[6px] hover:bg-hover focus-visible:opacity-100 ${
          favorite
            ? "text-accent"
            : "text-secondary opacity-0 group-hover/card:opacity-100 hover:text-fg"
        }`}
      >
        <Icon name="star" size={14} />
      </button>
    </div>
  );
}

function Nothing({ icon, title, hint }: { icon: IconName; title: string; hint: string }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center pb-6 text-center">
      <Icon name={icon} size={22} className="text-muted" />
      <p className="mt-[13px] text-[13px] leading-[17px] font-medium text-fg">{title}</p>
      <p className="mt-[3px] text-[12px] leading-[17px] text-muted">{hint}</p>
    </div>
  );
}

export function ThemeBrowser({ onBack, onClose }: { onBack: () => void; onClose?: () => void }) {
  const { prefs, set } = usePrefs();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  // The imported palettes arrive on demand; the page does not carry them.
  const [bundled, setBundled] = useState<Theme[] | null>(null);
  const list = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ width: 0, height: 0, top: 0 });
  // Scroll the catalog to the theme in use, once it is laid out.
  const reveal = useRef(true);

  useEffect(() => {
    let live = true;
    void import("./theme-catalog").then(({ BUNDLED }) => {
      if (!live) return;
      setBundled(BUNDLED.map(([name, colors]) => ({ id: `iterm:${name}`, name, colors })));
    });
    return () => {
      live = false;
    };
  }, []);

  useLayoutEffect(() => {
    const element = list.current;
    if (!element) return;
    const measure = () =>
      setView({ width: element.clientWidth, height: element.clientHeight, top: element.scrollTop });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // The column count follows the width, so cards grow with the sheet.
  const inner = Math.max(0, view.width - 40);
  const columns = inner >= 520 ? 3 : inner >= 340 ? 2 : 1;

  const { rows, height, count } = useMemo(() => {
    const text = query.trim().toLowerCase();
    const shown = (theme: Theme) =>
      (filter === "dark"
        ? theme.colors
          ? isDark(theme.colors)
          : theme.id !== "light"
        : filter === "light"
          ? theme.colors
            ? !isDark(theme.colors)
            : theme.id === "light"
          : filter !== "custom") &&
      (!text || theme.name.toLowerCase().includes(text));
    const rows: Row[] = [];
    let top = 0;
    let count = 0;
    const push = (title: string, themes: Theme[], pinned = false) => {
      if (themes.length === 0) return;
      rows.push({ kind: "label", top, title, count: themes.length });
      top += LABEL_HEIGHT;
      for (let index = 0; index < themes.length; index += columns) {
        rows.push({ kind: "cards", top, themes: themes.slice(index, index + columns), pinned });
        top += CARD_HEIGHT;
      }
      count += themes.length;
    };
    // Favorites lead while browsing; a search lists each theme once.
    if (filter === "favorites" || !text) push("Favorites", prefs.favorites.filter(shown), true);
    if (filter !== "favorites") {
      push("Neptune", BUILTINS.filter(shown));
      push("iTerm2 collection", (bundled ?? []).filter(shown));
    }
    return { rows, height: top, count };
  }, [query, filter, columns, bundled, prefs.favorites]);

  useLayoutEffect(() => {
    const element = list.current;
    if (!element || !reveal.current || view.height === 0) return;
    // Imported themes are revealed once they have arrived.
    if (prefs.theme.colors && !bundled) return;
    reveal.current = false;
    const row = rows.find(
      (item) =>
        item.kind === "cards" && !item.pinned && item.themes.some((theme) => theme.id === prefs.theme.id),
    );
    if (!row) return;
    // Centre the row of the theme in use; near the top, keep its heading.
    const offset = row.top - Math.max(4, (view.height - CARD_HEIGHT) / 2);
    element.scrollTop = offset > LABEL_HEIGHT ? offset : 0;
  }, [rows, bundled, prefs.theme, view.height]);

  const star = (theme: Theme) => {
    const starred = prefs.favorites.some((item) => item.id === theme.id);
    set({
      favorites: starred
        ? prefs.favorites.filter((item) => item.id !== theme.id)
        : [...prefs.favorites, theme],
    });
  };
  const changed = (edit: () => void) => {
    edit();
    reveal.current = false;
    if (list.current) list.current.scrollTop = 0;
  };

  return (
    <div className="@container/themes flex size-full flex-col text-fg">
      <div className="flex h-[52px] shrink-0 items-center pr-3 pl-3">
        <IconButton icon="chevronLeft" label="Back to preferences" onClick={onBack} />
        <h3 className="ml-1.5 flex-1 text-[15px] font-semibold text-fg">Themes</h3>
        {onClose && <IconButton icon="close" label="Close preferences" onClick={onClose} />}
      </div>
      {/* Search and filter share a line while there is room for both. */}
      <div className="flex shrink-0 flex-col gap-2.5 px-5 @min-[590px]/themes:flex-row @min-[590px]/themes:items-center">
        <SearchInput
          value={query}
          onChange={(value) => changed(() => setQuery(value))}
          placeholder="Search themes"
          label="Search themes"
          className="@min-[590px]/themes:flex-1"
        />
        <div className="shrink-0 @min-[590px]/themes:w-[364px]">
          <Segmented
            label="Theme filter"
            width="100%"
            value={filter}
            options={[
              ["all", "All"],
              ["dark", "Dark"],
              ["light", "Light"],
              ["custom", "Custom"],
              ["favorites", "Favorites"],
            ]}
            onChange={(value) => changed(() => setFilter(value))}
          />
        </div>
      </div>
      <div
        ref={list}
        role="radiogroup"
        aria-label="Theme"
        onScroll={(event) => {
          const top = event.currentTarget.scrollTop;
          setView((current) => (current.top === top ? current : { ...current, top }));
        }}
        className="quiet-scroll mt-3 flex min-h-0 flex-1 flex-col overflow-y-auto"
      >
        {count === 0 ? (
          filter === "custom" && !query.trim() ? (
            <Nothing icon="pencil" title="No custom themes yet" hint="Create and edit your own in the app." />
          ) : filter === "favorites" && !query.trim() ? (
            <Nothing icon="star" title="No favorites yet" hint="Star a theme to keep it here." />
          ) : bundled || filter === "favorites" ? (
            <Nothing icon="search" title="No themes found" hint="Try another name or filter." />
          ) : null
        ) : (
          <div className="relative mx-5 shrink-0" style={{ height }}>
            {rows
              // Only the rows in view are laid out and painted.
              .filter((row) => row.top + CARD_HEIGHT >= view.top - 160 && row.top <= view.top + view.height + 160)
              .map((row) =>
                row.kind === "label" ? (
                  <p
                    key={`label-${row.title}`}
                    className="absolute inset-x-0 flex items-center gap-[7px] pt-0.5 pl-1 text-[11.5px]"
                    style={{ top: row.top, height: LABEL_HEIGHT }}
                  >
                    <span className="font-medium text-secondary">{row.title}</span>
                    <span className="text-muted">{row.count}</span>
                  </p>
                ) : (
                  <div
                    key={`${row.pinned ? "pin" : "row"}-${row.themes[0].id}`}
                    className="absolute inset-x-0 flex"
                    style={{ top: row.top, gap: CARD_GAP }}
                  >
                    {row.themes.map((theme) => (
                      <Card
                        key={theme.id}
                        theme={theme}
                        look={lookOf(theme, prefs.accent)}
                        selected={theme.id === prefs.theme.id}
                        favorite={prefs.favorites.some((item) => item.id === theme.id)}
                        onUse={() => set({ theme })}
                        onStar={() => star(theme)}
                      />
                    ))}
                    {/* A short last row keeps the cards' width. */}
                    {Array.from({ length: columns - row.themes.length }, (_, index) => (
                      <span key={index} className="min-w-0 flex-1" />
                    ))}
                  </div>
                ),
              )}
          </div>
        )}
      </div>
      <div className="flex h-[60px] shrink-0 items-center justify-end border-t border-separator px-4">
        <Button kind="primary" onClick={onClose ?? onBack}>
          Done
        </Button>
      </div>
    </div>
  );
}
