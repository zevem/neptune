// Neptune's extension for pi and for Oh My Pi, loaded for one launch with
// `-e`. It tells Neptune what kind of moment the session is at: a turn began
// or ended, the person is asked to allow a tool or to answer something. Prompts, tool input and output never leave this
// process; the session's id and directory do, once the session has a message
// and so can be opened again.
import { spawn } from "node:child_process";

export default function (pi) {
  const command = process.env.NEPTUNE_AGENT_HOOK;
  if (!command) return;
  let named;
  // Each report is a process of its own. One starts when the one before it
  // has ended, so that they arrive in the order of the moments they mark.
  let last = Promise.resolve();
  const send = (event, fields) => {
    last = last.then(
      () =>
        new Promise((done) => {
          const limit = setTimeout(done, 5000);
          const ended = () => {
            clearTimeout(limit);
            done();
          };
          try {
            const hook = spawn("sh", ["-c", `${command} ${event}`], {
              stdio: ["pipe", "ignore", "ignore"],
            });
            hook.on("error", ended);
            hook.on("close", ended);
            hook.stdin.on("error", () => {});
            hook.stdin.end(JSON.stringify({ hook_event_name: event, ...fields }));
          } catch {
            ended();
          }
        }),
    );
  };
  // Each CLI has events the other lacks; one that is unknown is left out.
  const on = (name, handler) => {
    try {
      pi.on(name, handler);
    } catch {}
  };
  // A subagent runs in a session of its own, without the terminal: its turns
  // are not the person's, its requests to them are.
  const subagent = (ctx) => ctx?.hasUI === false;
  on("agent_start", (_event, ctx) => {
    if (subagent(ctx)) return;
    try {
      const session = ctx?.sessionManager?.getSessionId?.();
      if (session && session !== named) {
        named = session;
        send("SessionStart", { session_id: session, cwd: ctx.cwd });
      }
    } catch {}
    working = true;
    send("UserPromptSubmit", {});
  });
  // Both mark the end of a turn where pi has both; one report says it.
  let working = false;
  const rest = () => {
    if (working) send("Stop", {});
    working = false;
  };
  // Oh My Pi ends a loop it is about to continue by itself, as on a retry.
  on("agent_end", (event, ctx) => {
    if (!event?.willContinue && !subagent(ctx)) rest();
  });
  on("agent_settled", (_event, ctx) => {
    if (!subagent(ctx)) rest();
  });
  on("ui_prompt_start", () => send("Elicitation", {}));
  on("ui_prompt_end", () => send("ElicitationResult", {}));
  // Oh My Pi asks before a tool, and has a tool that asks a question.
  const tool = (name) => (typeof name === "string" && /^[\w.:-]{1,64}$/.test(name) ? name : undefined);
  on("tool_approval_requested", (event, ctx) =>
    send("PermissionRequest", {
      tool_name: tool(event?.toolName),
      ...(subagent(ctx) ? { agent_id: "subagent" } : {}),
    }),
  );
  on("tool_approval_resolved", () => send("ElicitationResult", {}));
  on("tool_execution_start", (event, ctx) => {
    if (event?.toolName === "ask" && !subagent(ctx)) send("PreToolUse", { tool_name: "ask" });
  });
  on("tool_execution_end", (event, ctx) => {
    if (event?.toolName === "ask" && !subagent(ctx)) send("PostToolUse", { tool_name: "ask" });
  });
}
