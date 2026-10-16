import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { AgentMark } from "../src/components/icons";
import { TabMark } from "../src/components/neptune/panel";
import { Store, type Action, type AgentKind, type Line, type Pane, type Span } from "../src/components/neptune/model";
import { execute } from "../src/components/neptune/shell";
import { Terminal } from "../src/components/neptune/terminal";
import { BASE, CHAPTERS } from "../src/components/neptune/tour";

// Pin the original provider artwork from before 098c515, not a generic icon
// library's approximation. The SVG path and viewBox together define each mark.
const providers = {
  claude: {
    viewBox: "0 0 256 257",
    pathSha256: "8dd102a1fd65491d5dd1e5ae66fd583edd9df8b291157e09ac7bbec7e02bca1c",
    banner: "✻ ",
    tone: 3,
    color: "var(--ansi-3)",
  },
  codex: {
    viewBox: "100 100 411 411",
    pathSha256: "f137248b6c4ca3c558149ccfeae259e5a2e3406cbb4e54e0529ce7febf723146",
    banner: ">_ ",
    tone: "muted",
    color: "var(--muted)",
  },
} as const;

function banner(lines: Line[], title: string): Span[] {
  const line = lines.find((line) => line.k === "out" && line.spans.some((span) => span.t === title));
  assert.ok(line?.k === "out", `missing ${title} banner`);
  return line.spans;
}

function assertBanner(lines: Line[], kind: AgentKind, title: string, pane: Pane) {
  const expected = providers[kind];
  const spans = banner(lines, title);
  assert.deepEqual(spans, [{ t: expected.banner, c: expected.tone }, { t: title, b: true }]);
  const markup = renderToStaticMarkup(createElement(Terminal, {
    pane: { ...pane, lines: [{ k: "out", spans }], prompt: false, status: "exited" },
    search: "",
  }));
  const text = expected.banner.replace(">", "&gt;");
  assert.ok(markup.includes(`<span style="color:${expected.color}">${text}</span>`));
  assert.ok(!markup.includes("tabler-icon"), "provider banner is terminal text, not a UI icon");
}

for (const kind of Object.keys(providers) as AgentKind[]) {
  test(`${kind} retains its original filled silhouette, viewport and caller color`, () => {
    const expected = providers[kind];
    for (const size of [8, 13, 16, 24, 40]) {
      const markup = renderToStaticMarkup(createElement(AgentMark, {
        kind, size, className: "text-secondary hover:text-fg",
      }));
      assert.ok(markup.includes(`width="${size}" height="${size}"`));
      assert.ok(markup.includes(`viewBox="${expected.viewBox}"`));
      assert.ok(markup.includes('fill="currentColor"'));
      assert.ok(markup.includes('aria-hidden="true"'));
      assert.ok(markup.includes('class="shrink-0 text-secondary hover:text-fg"'));
      assert.doesNotMatch(markup, /stroke=|tabler-icon/);
      const paths = [...markup.matchAll(/<path\b[^>]*\bd="([^"]+)"/g)];
      assert.equal(paths.length, 1);
      assert.equal(createHash("sha256").update(paths[0][1]).digest("hex"), expected.pathSha256);
      assert.equal(markup.includes('fill-rule="evenodd"'), kind === "codex");
    }
    const defaultMark = renderToStaticMarkup(createElement(AgentMark, { kind }));
    assert.ok(defaultMark.includes('width="13" height="13"'));
    const tabMark = renderToStaticMarkup(createElement(TabMark, { kind }));
    assert.ok(tabMark.includes('class="shrink-0 mr-[5px] text-secondary"'));
    assert.ok(tabMark.includes(`viewBox="${expected.viewBox}"`));
  });

  test(`${kind} shell banner retains the original terminal variant and color`, () => {
    const pane = { ...BASE.panes[1], input: kind };
    const actions: Action[] = [];
    execute({ dispatch: (action) => actions.push(action), setPrefs: () => {} }, pane);
    const lines = actions.flatMap((action) => action.type === "print" ? action.lines : []);
    assertBanner(lines, kind, kind === "claude" ? "Claude Code" : "Codex", pane);
    assert.ok(actions.some((action) => action.type === "patch" && action.patch.agent?.kind === kind));
  });
}

test("the agents tour retains both original provider banners at wide and compact sizes", async () => {
  const chapter = CHAPTERS.find((chapter) => chapter.id === "agents")!;
  for (const compact of [false, true]) {
    const store = new Store(structuredClone(BASE));
    const lines: Line[] = [];
    await chapter.run({
      d: (action) => {
        if (action.type === "print" || action.type === "spawn") lines.push(...(action.lines ?? []));
        store.dispatch(action);
      },
      get: store.get,
      wait: async () => {},
      compact,
    });
    assertBanner(lines, "claude", "Claude Code", BASE.panes[1]);
    assertBanner(lines, "codex", "OpenAI Codex", BASE.panes[1]);
  }
});
