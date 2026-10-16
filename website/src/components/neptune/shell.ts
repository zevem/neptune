// A stand-in shell for the in-page window. It answers a handful of commands so
// a visitor can type; it is a demonstration, not Neptune's terminal engine.

import { ACCENTS, type Accent, type Prefs } from "../prefs";
import { BUILTINS } from "./themes";
import { AGENT_NAMES, out, row, type Action, type Line, type Pane } from "./model";

export interface ShellEnv {
  dispatch: (action: Action) => void;
  setPrefs: (patch: Partial<Prefs>) => void;
}

const dir = (name: string) => ({ t: name, c: 4, b: true });

export const LISTINGS: Record<string, string[]> = {
  "~": ["code/", "notes/"],
  "~/code": ["api/", "neptune/"],
  "~/code/neptune": [
    "assets/",
    "crates/",
    "docs/",
    "src/",
    "Cargo.toml",
    "README.md",
    "config.example.toml",
  ],
  "~/code/neptune/crates": ["neptune-model/", "terminal-core/"],
  "~/code/api": ["src/", "package.json", "bun.lock"],
  "~/notes": ["ideas.md", "today.md"],
};

/** `neptune --help`, as printed by `src/main.rs`. */
export const NEPTUNE_HELP = [
  "Neptune — a native GPU terminal",
  "",
  "Usage: neptune [OPTIONS]",
  "  --cwd PATH         Open a workspace at PATH",
  "  --ssh DESTINATION  Open a workspace whose terminals run on an SSH host",
  "  --config PATH      Use a TOML configuration",
  "  --data-root PATH   Isolate settings and saved workspace/window state",
  "  --command COMMAND  Run a command in the first terminal",
  "  --no-restore       Start without saved workspaces",
  "  --size WIDTHxHEIGHT Override saved window size and maximized state",
  "  --screenshot PATH  Capture the native window after 3 seconds and exit",
  "  --diagnostics      Print renderer and display details",
  "  --version",
  "  --help",
];

/** `cargo test -p neptune-model -q`: the model crate has 33 tests. */
export const TEST_COUNT = 33;
export const CARGO_TEST: Line[] = [
  out(""),
  out(`running ${TEST_COUNT} tests`),
  out(".".repeat(TEST_COUNT)),
  row("test result: ", { t: "ok", c: 2 }, `. ${TEST_COUNT} passed; 0 failed`),
];

/** Commits from this repository's history. */
export const GIT_LOG: Line[] = [
  ["237a86b", "ci: let cancelled runs stop expensive checks"],
  ["b615026", "ci: use latest GitHub-hosted runners"],
  ["321b005", "feat(sidebar): reorder workspaces by dragging"],
  ["59716df", "docs(agents): define pull request rules"],
  ["ec15039", "Fix CI failures and reduce build minutes"],
].map(([hash, subject]) => row({ t: hash, c: 3 }, ` ${subject}`));

const HELP: Line[] = [
  out("This window is a demonstration. It answers:"),
  row({ t: "  ls  cd  pwd  echo  clear  exit", c: 6 }),
  row({ t: "  cargo test   git log   git status", c: 6 }),
  row({ t: "  neptune --help  neptune --version", c: 6 }),
  row({ t: "  claude   codex", c: 6 }),
  row({ t: "  theme dusk   accent pink", c: 6 }),
  row({ t: "  printf '\\e]9;Build finished\\a'", c: 6 }),
];

/**
 * The attention request a `printf` argument carries, if any: OSC 9 with a
 * message, or OSC 777 with a title and a body.
 */
function notification(text: string): { title: string; body: string } | null {
  const escape = String.raw`(?:\\e|\\033|\\x1b)`;
  const match = new RegExp(
    String.raw`${escape}\](9|777;notify);(.*?)(?:\\a|\\007|${escape}\\\\)`,
    "i",
  ).exec(text);
  if (!match) return null;
  const [title, ...body] = match[2].split(";");
  return match[1] === "9" ? { title: "", body: match[2] } : { title, body: body.join(";") };
}

function resolve(cwd: string, target: string): string {
  if (!target) return cwd;
  if (target === "~") return "~";
  const parts = target.startsWith("~") ? ["~"] : cwd.split("/");
  for (const part of target.replace(/^~\/?/, "").split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (parts.length > 1) parts.pop();
    } else {
      parts.push(part);
    }
  }
  return parts.join("/");
}

/** Runs what the pane's prompt holds and leaves a fresh prompt behind. */
export function execute(env: ShellEnv, pane: Pane) {
  const { dispatch } = env;
  const id = pane.id;
  const text = pane.input.trim();
  dispatch({ type: "commit", pane: id });
  const print = (...lines: Line[]) => dispatch({ type: "print", pane: id, lines });
  const [name, ...args] = text.split(/\s+/);
  const rest = args.join(" ");
  let ok = true;

  switch (name) {
    case "":
      break;
    case "help":
      print(...HELP);
      break;
    case "clear":
      dispatch({ type: "clear", pane: id });
      break;
    case "pwd":
      print(out(pane.remote ? "/home/" + (pane.remote.split("@")[0] || "user") : pane.cwd.replace("~", "/home/you")));
      break;
    case "whoami":
      print(out(pane.remote?.includes("@") ? pane.remote.split("@")[0] : "you"));
      break;
    case "echo":
      print(out(rest.replace(/^(['"])(.*)\1$/, "$2")));
      break;
    case "date":
      print(out(new Date().toString()));
      break;
    case "printf": {
      const text = rest.replace(/^(['"])(.*)\1$/, "$2");
      const alert = notification(text);
      // A program asks for attention through its terminal; the rest is text.
      if (alert) dispatch({ type: "notify", pane: id, ...alert, at: Date.now() });
      else if (text) print(out(text.replace(/\\n$/, "")));
      break;
    }
    case "ls": {
      const entries = pane.remote ? [] : (LISTINGS[resolve(pane.cwd, args.find((a) => !a.startsWith("-")) ?? "")] ?? []);
      if (entries.length) {
        print(
          row(
            ...entries.flatMap((entry, index) => [
              entry.endsWith("/") ? dir(entry.slice(0, -1)) : entry,
              index < entries.length - 1 ? "  " : "",
            ]),
          ),
        );
      }
      break;
    }
    case "cd": {
      if (pane.remote) break;
      const next = resolve(pane.cwd, args[0] || "~");
      if (next in LISTINGS) {
        dispatch({
          type: "patch",
          pane: id,
          patch: {
            cwd: next,
            branch: next.startsWith("~/code/") ? "main" : undefined,
          },
        });
      } else {
        print(out(`cd: no such file or directory: ${args[0]}`));
        ok = false;
      }
      break;
    }
    case "exit":
      // The pane stays, stating plainly that its process ended.
      dispatch({ type: "patch", pane: id, patch: { status: "exited", prompt: false } });
      return;
    case "claude":
    case "codex": {
      // The tab takes the agent's mark and the Agents tab lists it.
      const kind = name as "claude" | "codex";
      dispatch({
        type: "patch",
        pane: id,
        patch: {
          agent: { kind, activity: "idle", title: AGENT_NAMES[kind], minutes: 0, workspace: 0 },
        },
      });
      print(
        out(""),
        row({ t: kind === "claude" ? "✻ " : ">_ ", c: kind === "claude" ? 3 : "muted" }, { t: AGENT_NAMES[kind], b: true }),
        out(""),
        out(`In Neptune, ${AGENT_NAMES[kind]} runs here: its tab carries its mark, its pull`),
        out("requests and files, and the Agents tab says when it waits for you."),
        row({ t: "This window is a demonstration, so no agent starts.", c: "muted" }),
      );
      break;
    }
    case "neptune":
      if (args[0] === "--version" || args[0] === "-V") print(out("Neptune 0.1.0"));
      else print(...NEPTUNE_HELP.map((line) => out(line)));
      break;
    case "cargo":
      if (args[0] === "test") print(...CARGO_TEST);
      else {
        print(row({ t: "error", c: 1, b: true }, ": this demonstration only runs `cargo test`"));
        ok = false;
      }
      break;
    case "git":
      if (args[0] === "log") print(...GIT_LOG);
      else if (args[0] === "status") {
        print(out("On branch main"), out("nothing to commit, working tree clean"));
      } else {
        print(out("usage: git log | git status"));
        ok = false;
      }
      break;
    case "theme":
      if (BUILTINS.some((theme) => theme.id === args[0])) {
        env.setPrefs({ theme: BUILTINS.find((theme) => theme.id === args[0]) });
      } else {
        print(out(`usage: theme ${BUILTINS.map((theme) => theme.id).join("|")}`));
        ok = false;
      }
      break;
    case "accent":
      if ((ACCENTS as readonly string[]).includes(args[0])) {
        env.setPrefs({ accent: args[0] as Accent });
      } else {
        print(out(`usage: accent ${ACCENTS.join("|")}`));
        ok = false;
      }
      break;
    default:
      print(out(`${pane.remote ? "bash" : pane.title}: command not found: ${name}`));
      ok = false;
  }
  dispatch({ type: "ready", pane: id, ok });
}
