import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { GitHubMark, Icon, NeptuneMark, TABLER_ICONS, type IconName } from "../src/components/icons";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const mapping: Record<IconName, string> = {
  terminal: "terminal-2", plus: "plus", close: "x",
  chevronRight: "chevron-right", chevronLeft: "chevron-left", chevronDown: "chevron-down",
  bell: "bell", folder: "folder", star: "star", settings: "settings", search: "search",
  sidebar: "layout-sidebar", splitVertical: "layout-columns", splitHorizontal: "layout-rows",
  grid: "layout-grid", check: "check", arrowUpRight: "arrow-up-right", arrowUp: "arrow-up",
  arrowDown: "arrow-down", arrowRight: "arrow-right", minus: "minus",
  maximize: "arrows-maximize", minimize: "arrows-minimize", command: "command",
  sun: "sun", moon: "moon", copy: "copy", ellipsis: "dots", refresh: "refresh",
  pencil: "pencil", clipboard: "clipboard-text", eraser: "eraser", textSize: "text-size",
  swap: "arrows-exchange", globe: "world", panelRight: "layout-sidebar-right",
  pullRequest: "git-pull-request", merged: "git-merge", comment: "message", branch: "git-branch",
  paperclip: "paperclip", agents: "hierarchy", file: "file", warning: "alert-triangle",
  play: "player-play-filled", pause: "player-pause-filled", lock: "lock",
  circle: "circle", circleDot: "circle-dot", circleFilled: "circle-filled",
  loader: "loader-2", cornerDownRight: "corner-down-right",
};

const root = fileURLToPath(new URL("..", import.meta.url));

test("the official Tabler package is exactly pinned to 3.49.0", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const installed = JSON.parse(readFileSync(join(root, "node_modules/@tabler/icons-react/package.json"), "utf8"));
  assert.equal(pkg.dependencies["@tabler/icons-react"], "3.49.0");
  assert.equal(installed.version, "3.49.0");
});

test("every shared glyph uses its official mapping and preserves size, classes and currentColor", () => {
  assert.deepEqual(Object.keys(TABLER_ICONS).sort(), Object.keys(mapping).sort());
  for (const name of Object.keys(mapping) as IconName[]) {
    for (const size of [8, 13, 16, 24, 40]) {
      const markup = renderToStaticMarkup(createElement(Icon, { name, size, className: "text-accent hover:text-fg" }));
      assert.ok(markup.includes(`tabler-icon-${mapping[name]}`));
      assert.ok(markup.includes(`width="${size}" height="${size}"`));
      assert.ok(markup.includes('viewBox="0 0 24 24"'));
      assert.ok(markup.includes('aria-hidden="true"'));
      assert.ok(markup.includes("currentColor"));
      assert.ok(markup.includes("shrink-0 text-accent hover:text-fg"));
      assert.equal(markup, renderToStaticMarkup(createElement(TABLER_ICONS[name], {
        size, "aria-hidden": "true", className: "shrink-0 text-accent hover:text-fg",
      })));
      if (!name.endsWith("Filled") && name !== "play" && name !== "pause") {
        assert.ok(markup.includes('stroke-width="2"'));
      }
    }
  }
});

test("GitHub uses the official glyph and Neptune retains the supplied raster mark", () => {
  assert.ok(renderToStaticMarkup(createElement(GitHubMark, { size: 15 })).includes("tabler-icon-brand-github"));
  const mark = renderToStaticMarkup(createElement(NeptuneMark, { size: 24 }));
  assert.ok(mark.includes('width="24" height="24"'));
  assert.ok(mark.includes('src="/neptune-logo.png"'));
});

function sourceFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? sourceFiles(path) : /\.tsx?$/.test(path) ? [path] : [];
  });
}

test("website functional icons have no custom SVG paths or Unicode drawings", () => {
  for (const path of sourceFiles(join(root, "src"))) {
    const source = readFileSync(path, "utf8");
    // Provider trademarks are original artwork, separately pinned by providers.test.
    // The other remaining SVG paths are an architecture connector and CPU chart.
    const functionalSource = path.endsWith("/icons.tsx")
      ? source.split("/**\n * The marks of the CLI agents")[0]
      : source;
    if (!path.endsWith("/site/internals.tsx")) assert.doesNotMatch(functionalSource, /<svg|<path\b/);
    assert.doesNotMatch(source, /[●⎿›•✔]/u);
    if (!path.endsWith("/neptune/shell.ts") && !path.endsWith("/neptune/tour.ts")) {
      assert.doesNotMatch(source, /✻/u);
    }
    assert.ok(!source.includes("GLYPHS"));
  }
});
