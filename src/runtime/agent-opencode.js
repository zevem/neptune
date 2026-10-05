// Neptune's OpenCode plugin, added to one launch through OPENCODE_CONFIG_CONTENT.
// It tells Neptune what kind of moment the session is at: a turn began or
// ended, a person is asked something. Prompts, tool input and output never
// leave this process; the session's id and directory do, so that Neptune can
// reopen the session.
import { spawn } from "node:child_process";

export const Neptune = async ({ directory }) => {
  const command = process.env.NEPTUNE_AGENT_HOOK;
  if (!command) return {};
  // Sessions a tool started for a subagent; their turns are not the person's.
  const children = new Set();
  let current;
  // OpenCode says "busy" again at every step of a turn, the last time just
  // before "idle": only a change is a moment.
  let busy = false;
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
            hook.stdin.end(JSON.stringify({ hook_event_name: event, cwd: directory, ...fields }));
          } catch {
            ended();
          }
        }),
    );
  };
  const subagent = (session) => (children.has(session) ? { agent_id: session } : {});
  return {
    event: async ({ event }) => {
      const properties = event?.properties ?? {};
      const session = properties.sessionID;
      switch (event?.type) {
        case "session.created":
          if (properties.info?.parentID) children.add(session);
          break;
        case "session.status":
          if (!session || children.has(session)) break;
          if (properties.status?.type === "idle") {
            if (session === current && busy) send("Stop", {});
            if (session === current) busy = false;
          } else if (session !== current || !busy) {
            if (session !== current) {
              current = session;
              send("SessionStart", { session_id: session });
            }
            busy = true;
            send("UserPromptSubmit", {});
          }
          break;
        case "permission.asked":
          send("PermissionRequest", subagent(session));
          break;
        case "question.asked":
          send("PreToolUse", { tool_name: "question", ...subagent(session) });
          break;
        case "permission.replied":
        case "question.replied":
        case "question.rejected":
          send("ElicitationResult", {});
          break;
      }
    },
  };
};
