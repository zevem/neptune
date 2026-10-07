//! Codex's app-server protocol as lines in and lines out: JSON-RPC without
//! its version field, a request and its answer matched by a name. Frames are
//! read by key, and what is not known is passed over.
use super::{Driver, Failure, LeadEvent, Outcome, Output, Shown, Spawn, clip};
use serde_json::{Map, Value, json};
use std::{path::PathBuf, process::Command};

const INIT: &str = "init";
const ACCOUNT: &str = "account";
const CONFIG: &str = "config";
const THREAD: &str = "thread";
/// The one tool server a lead has.
const SERVER: &str = "neptune";
/// What Codex would otherwise hand a lead beside Neptune's tools. A lead
/// plans and delegates: it runs nothing, reads nothing and browses nothing.
const FEATURES: [&str; 17] = [
    "shell_tool",
    "unified_exec",
    "code_mode_host",
    "apps",
    "multi_agent",
    "view_image",
    "image_generation",
    "plugins",
    "hooks",
    "tool_suggest",
    "sleep_tool",
    "goals",
    "browser_use",
    "computer_use",
    "skill_search",
    "in_app_browser",
    "memories",
];
/// No tool of a lead waits longer than this for Neptune.
const TOOL_TIMEOUT: u64 = 60;
/// Tool servers of the person's own configuration that are kept off.
const MAX_SERVERS: usize = 64;
/// Tool calls of one turn that have not returned.
const MAX_CALLS: usize = 64;
/// What a provider says of a failure, for the chat's error row.
const MAX_REASON: usize = 300;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Initializing,
    Account,
    Config,
    /// The conversation is asked for, or its tools are still starting.
    Opening,
    Idle,
    Turn,
    /// Start-up failed and was said so; nothing more follows.
    Failed,
}
/// What the CLI last said of Neptune's tool server.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tools {
    Starting,
    Ready,
    Failed,
}
pub(super) struct Codex {
    prompt: String,
    /// The model and the effort the lead is asked for; the CLI's own
    /// choice where there is none.
    asked: Option<String>,
    effort: Option<String>,
    /// The model the conversation runs on, as the CLI named it.
    model: Option<String>,
    neptune: PathBuf,
    dir: PathBuf,
    /// The lead's credential for Neptune's tool server. It is written to the
    /// CLI's input only, never to an argument or its environment.
    env: Vec<(&'static str, String)>,
    /// The conversation to go on with, or the one just begun.
    thread: Option<String>,
    resume: bool,
    phase: Phase,
    plan: Option<String>,
    /// The person's own tool servers, by name. A lead is given none of them.
    servers: Vec<String>,
    /// The conversation answered: its tools decide whether it is ready.
    opened: bool,
    tools: Option<(Option<String>, Tools)>,
    /// The caller's name for the running turn, and the CLI's.
    turn: Option<String>,
    wire: Option<String>,
    turns: u32,
    block: u32,
    /// The message being streamed.
    item: Option<String>,
    calls: Vec<String>,
    interrupts: u32,
    interrupting: bool,
    /// An interrupt was asked for before the CLI named the turn.
    wanted: bool,
    retries: u32,
    /// What the CLI said went wrong in this turn.
    error: Option<Failure>,
    /// When the usage window that was reached ends.
    limit: Option<u64>,
}
impl Codex {
    pub(super) fn new(prompt: &str, model: Option<&str>, effort: Option<&str>) -> Self {
        Self {
            prompt: prompt.to_owned(),
            asked: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
            model: None,
            neptune: PathBuf::new(),
            dir: PathBuf::new(),
            env: Vec::new(),
            thread: None,
            resume: false,
            phase: Phase::Start,
            plan: None,
            servers: Vec::new(),
            opened: false,
            tools: None,
            turn: None,
            wire: None,
            turns: 0,
            block: 0,
            item: None,
            calls: Vec::new(),
            interrupts: 0,
            interrupting: false,
            wanted: false,
            retries: 0,
            error: None,
            limit: None,
        }
    }
    fn fail(&mut self, failure: Failure, out: &mut Output) {
        self.phase = Phase::Failed;
        out.events.push(LeadEvent::Failed(failure));
    }
    /// What the conversation is opened with, every time: what a turn may do
    /// is not kept by the CLI across a resume, and neither is the tool server.
    fn open(&self) -> String {
        let mut servers = Map::new();
        for name in &self.servers {
            servers.insert(name.clone(), json!({"enabled":false}));
        }
        let variables: Map<String, Value> = self
            .env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.as_str().into()))
            .collect();
        servers.insert(
            SERVER.to_owned(),
            json!({
                "command":self.neptune.to_string_lossy(),
                "args":["--agent-mcp"],
                "env":variables,
                "default_tools_approval_mode":"approve",
                "tool_timeout_sec":TOOL_TIMEOUT,
            }),
        );
        let features: Map<String, Value> = FEATURES
            .iter()
            .map(|feature| ((*feature).to_owned(), false.into()))
            .collect();
        let mut config = json!({
            "mcp_servers":servers,
            "features":features,
            "web_search":"disabled",
        });
        let mut params = json!({
            "cwd":self.dir.to_string_lossy(),
            "approvalPolicy":"never",
            "sandbox":"read-only",
        });
        // Said as the conversation is opened and again with every turn:
        // a resumed one does not keep what it ran with.
        if let Some(model) = &self.asked {
            params["model"] = model.as_str().into();
        }
        if let Some(effort) = &self.effort {
            config["model_reasoning_effort"] = effort.as_str().into();
        }
        params["config"] = config;
        let method = match self.thread.as_deref().filter(|_| self.resume) {
            Some(thread) => {
                params["threadId"] = thread.into();
                params["excludeTurns"] = true.into();
                "thread/resume"
            }
            None => {
                params["developerInstructions"] = self.prompt.as_str().into();
                "thread/start"
            }
        };
        json!({"id":THREAD,"method":method,"params":params}).to_string()
    }
    /// Ready once the conversation is there and its tools are: a lead is
    /// not left to talk without them.
    fn settle(&mut self, out: &mut Output) {
        if self.phase != Phase::Opening || !self.opened {
            return;
        }
        let Some((thread, tools)) = &self.tools else {
            return;
        };
        if thread.is_some() && *thread != self.thread {
            return;
        }
        match tools {
            Tools::Starting => {}
            Tools::Ready => {
                self.phase = Phase::Idle;
                out.events.push(LeadEvent::Ready {
                    model: self.model.clone(),
                    account_kind: self.plan.take(),
                });
            }
            Tools::Failed => self.fail(Failure::ToolsUnavailable, out),
        }
    }
    fn response(&mut self, id: &str, frame: &Value, out: &mut Output) {
        let result = &frame["result"];
        let refused = frame["error"]["message"].as_str();
        match (self.phase, id) {
            (Phase::Initializing, INIT) => {
                if !result.is_object() {
                    return self.fail(Failure::Protocol, out);
                }
                self.phase = Phase::Account;
                out.replies
                    .push(json!({"method":"initialized"}).to_string());
                out.replies
                    .push(json!({"id":ACCOUNT,"method":"account/read","params":{}}).to_string());
            }
            (Phase::Account, ACCOUNT) => {
                if !result.is_object() {
                    return self.fail(Failure::Protocol, out);
                }
                let account = &result["account"];
                if account.is_null() && result["requiresOpenaiAuth"] == true {
                    return self.fail(Failure::Auth, out);
                }
                // The plan's name only: who is signed in is never carried.
                self.plan = account["planType"]
                    .as_str()
                    .map(|plan| clip(plan, 64).to_owned());
                self.phase = Phase::Config;
                out.replies.push(
                    json!({"id":CONFIG,"method":"config/read","params":{
                        "cwd":self.dir.to_string_lossy(),
                    }})
                    .to_string(),
                );
            }
            (Phase::Config, CONFIG) => {
                // Only their names are read; what each is started with may
                // hold the person's credentials. Refused, none is known.
                self.servers = result["config"]["mcp_servers"]
                    .as_object()
                    .into_iter()
                    .flat_map(|servers| servers.keys())
                    .filter(|name| *name != SERVER && name.len() <= 128)
                    .take(MAX_SERVERS)
                    .cloned()
                    .collect();
                self.phase = Phase::Opening;
                out.replies.push(self.open());
            }
            (Phase::Opening, THREAD) if !self.opened => {
                let Some(thread) = result["thread"]["id"]
                    .as_str()
                    .filter(|thread| !thread.is_empty() && thread.len() <= 128)
                else {
                    let lost = self.resume
                        && refused.is_some_and(|said| {
                            said.contains("no rollout found") || said.contains("thread not found")
                        });
                    return self.fail(
                        if lost {
                            Failure::SessionLost
                        } else {
                            Failure::Protocol
                        },
                        out,
                    );
                };
                if self.thread.as_deref() != Some(thread) {
                    // A conversation of its own naming: resumed by it later.
                    self.thread = Some(thread.to_owned());
                    out.events.push(LeadEvent::Session {
                        id: thread.to_owned(),
                    });
                }
                self.opened = true;
                self.model = result["model"]
                    .as_str()
                    .map(|model| clip(model, 128).to_owned());
                self.settle(out);
            }
            (Phase::Turn, id) if id == format!("turn-{}", self.turns) => {
                match result["turn"]["id"].as_str() {
                    Some(wire) => self.begun(wire, out),
                    // The turn was not taken: nothing of it runs.
                    None => {
                        let said = refused.unwrap_or("the turn was refused");
                        self.error = Some(Failure::Provider(clip(said, MAX_REASON).to_owned()));
                        self.end("failed", &Value::Null, out);
                    }
                }
            }
            _ => {}
        }
    }
    /// The CLI named the running turn.
    fn begun(&mut self, wire: &str, out: &mut Output) {
        if self.wire.is_none() {
            self.wire = Some(wire.to_owned());
        }
        if std::mem::take(&mut self.wanted) {
            out.replies.extend(self.interrupt());
        }
    }
    /// A request the CLI makes of its host. A lead is granted Neptune's own
    /// tools and nothing else.
    fn request(&mut self, id: &Value, method: &str, params: &Value, out: &mut Output) {
        let result = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                Some(json!({"decision":"decline"}))
            }
            "execCommandApproval" | "applyPatchApproval" => Some(json!({"decision":"denied"})),
            "mcpServer/elicitation/request" => Some(if params["serverName"] == SERVER {
                json!({"action":"accept","content":{}})
            } else {
                json!({"action":"decline"})
            }),
            _ => None,
        };
        out.replies.push(
            match result {
                Some(result) => json!({"id":id,"result":result}),
                None => json!({"id":id,"error":{
                    "code":-32601,
                    "message":"Not available to a project's lead.",
                }}),
            }
            .to_string(),
        );
    }
    fn started(&mut self, item: &Value, out: &mut Output) {
        let Some(id) = item["id"].as_str() else {
            return;
        };
        match item["type"].as_str() {
            // What it says while it works and what it answers are both the
            // lead's words.
            Some("agentMessage") => {
                self.block += 1;
                self.item = Some(id.to_owned());
                out.deltas.push((self.block, String::new()));
            }
            Some("mcpToolCall") if item["server"] == SERVER => {
                let Some(tool) = item["tool"].as_str() else {
                    return;
                };
                if self.calls.len() == MAX_CALLS {
                    self.calls.remove(0);
                }
                self.calls.push(id.to_owned());
                out.events.push(LeadEvent::ToolStarted {
                    call: id.to_owned(),
                    tool: clip(tool, 128).to_owned(),
                    input: super::clip_input(&item["arguments"]),
                });
            }
            _ => {}
        }
    }
    fn completed(&mut self, item: &Value, out: &mut Output) {
        let Some(id) = item["id"].as_str() else {
            return;
        };
        match item["type"].as_str() {
            Some("agentMessage") => {
                if self.item.take().as_deref() != Some(id) {
                    self.block += 1;
                }
                if let Some(text) = item["text"].as_str().filter(|text| !text.is_empty()) {
                    out.events.push(LeadEvent::TextDone {
                        block: self.block,
                        text: clip(text, super::MAX_STREAM).to_owned(),
                    });
                }
            }
            Some("mcpToolCall") => {
                if let Some(at) = self.calls.iter().position(|call| call == id) {
                    out.events.push(LeadEvent::ToolDone {
                        call: self.calls.remove(at),
                        ok: item["status"] == "completed"
                            && item["error"].is_null()
                            && item["result"]["isError"] != true,
                    });
                }
            }
            Some("contextCompaction") => out.events.push(LeadEvent::Compacted),
            _ => {}
        }
    }
    /// The turn is over. A call still open is closed here: the CLI ends an
    /// interrupted turn without a word about the call it was in.
    fn end(&mut self, status: &str, error: &Value, out: &mut Output) {
        let Some(turn) = self.turn.take() else {
            return;
        };
        for call in self.calls.drain(..) {
            out.events.push(LeadEvent::ToolDone { call, ok: false });
        }
        let named = failure(error, self.limit);
        let outcome = match status {
            "completed" => Outcome::Completed,
            "interrupted" => Outcome::Interrupted,
            _ => Outcome::Failed(
                named
                    .or(self.error.take())
                    .unwrap_or(Failure::Provider("error".into())),
            ),
        };
        self.phase = Phase::Idle;
        self.wire = None;
        self.item = None;
        self.error = None;
        self.interrupting = false;
        self.wanted = false;
        out.events.push(LeadEvent::TurnEnded {
            turn,
            outcome,
            cost_usd: None,
        });
    }
    fn notification(&mut self, method: &str, params: &Value, out: &mut Output) {
        // One process holds one conversation; what names another is not it.
        let thread = params["threadId"].as_str();
        if method == "mcpServer/startupStatus/updated" {
            if params["name"] == SERVER && !matches!(self.phase, Phase::Idle | Phase::Turn) {
                let tools = match params["status"].as_str() {
                    Some("ready") => Tools::Ready,
                    Some("failed" | "cancelled") => Tools::Failed,
                    _ => Tools::Starting,
                };
                self.tools = Some((thread.map(str::to_owned), tools));
                self.settle(out);
            }
            return;
        }
        if method == "account/rateLimits/updated" {
            let limits = &params["rateLimits"];
            if limits["rateLimitReachedType"].is_null() {
                return;
            }
            // The window that is full, or the later of two that are.
            self.limit = ["primary", "secondary"]
                .iter()
                .map(|window| &limits[*window])
                .filter(|window| {
                    window["usedPercent"]
                        .as_u64()
                        .is_some_and(|used| used >= 100)
                })
                .filter_map(|window| window["resetsAt"].as_u64())
                .max();
            if self.phase == Phase::Turn {
                out.events.push(LeadEvent::Limit {
                    resets_at: self.limit,
                });
            }
            return;
        }
        // Nothing but its own turn's frames says what a lead did.
        if self.phase != Phase::Turn || (thread.is_some() && thread != self.thread.as_deref()) {
            return;
        }
        if let (Some(wire), Some(named)) = (self.wire.as_deref(), params["turnId"].as_str())
            && wire != named
        {
            return;
        }
        match method {
            "turn/started" => {
                if let Some(wire) = params["turn"]["id"].as_str() {
                    self.begun(wire, out);
                }
            }
            "item/started" => self.started(&params["item"], out),
            "item/agentMessage/delta" => {
                if let (Some(item), Some(text)) =
                    (params["itemId"].as_str(), params["delta"].as_str())
                    && self.item.as_deref() == Some(item)
                {
                    out.deltas.push((self.block, text.to_owned()));
                }
            }
            "item/completed" => self.completed(&params["item"], out),
            "thread/compacted" => out.events.push(LeadEvent::Compacted),
            "error" => {
                let error = &params["error"];
                if params["willRetry"] == true {
                    self.retries += 1;
                    let (attempt, max) = error["message"]
                        .as_str()
                        .and_then(attempts)
                        .unwrap_or((self.retries, 0));
                    out.events.push(LeadEvent::Retry {
                        attempt,
                        max,
                        delay_ms: 0,
                    });
                } else {
                    self.error = failure(error, self.limit);
                }
            }
            "turn/completed" => {
                let turn = &params["turn"];
                if let (Some(wire), Some(named)) = (self.wire.as_deref(), turn["id"].as_str())
                    && wire != named
                {
                    return;
                }
                self.end(
                    turn["status"].as_str().unwrap_or("failed"),
                    &turn["error"],
                    out,
                );
            }
            _ => {}
        }
    }
}
/// "Reconnecting... 2/5" says which try it is, and of how many.
fn attempts(message: &str) -> Option<(u32, u32)> {
    let (attempt, max) = message.split_whitespace().next_back()?.split_once('/')?;
    Some((attempt.parse().ok()?, max.parse().ok()?))
}
/// What went wrong, from the CLI's own name for it where it has one.
fn failure(error: &Value, limit: Option<u64>) -> Option<Failure> {
    if !error.is_object() {
        return None;
    }
    Some(match error["codexErrorInfo"].as_str() {
        Some("usageLimitExceeded") => Failure::Limit { resets_at: limit },
        Some("unauthorized") => Failure::Auth,
        Some("contextWindowExceeded") => Failure::PromptTooLong,
        Some("serverOverloaded") => Failure::Overloaded,
        _ => Failure::Provider(
            clip(error["message"].as_str().unwrap_or("error"), MAX_REASON).to_owned(),
        ),
    })
}
impl Driver for Codex {
    fn config(&self, _: &std::path::Path, _: &super::LeadEnv) -> Option<String> {
        None
    }
    fn command(&mut self, spawn: &Spawn<'_>) -> Command {
        self.neptune = spawn.neptune.to_owned();
        self.dir = spawn.dir.to_owned();
        self.env = spawn.env.vars().into_iter().collect();
        self.thread = spawn.session.map(str::to_owned);
        // A conversation it named exists, whether or not its tools came up.
        self.resume = spawn.session.is_some();
        let mut command = Command::new(spawn.exe);
        command.arg("app-server");
        // An enclosing Codex session would say what this one may do.
        super::scrub(&mut command, std::env::vars_os(), |name| {
            name.starts_with("CODEX_SANDBOX")
        });
        command.current_dir(spawn.dir);
        command
    }
    fn hello(&mut self) -> Vec<String> {
        self.phase = Phase::Initializing;
        vec![
            json!({"id":INIT,"method":"initialize","params":{"clientInfo":{
                "name":"neptune",
                "title":"Neptune",
                "version":env!("CARGO_PKG_VERSION"),
            }}})
            .to_string(),
        ]
    }
    fn turn(&mut self, id: &str, text: &str, pictures: &[Shown]) -> Vec<String> {
        // A turn started while one runs is folded into it without a word.
        let (Phase::Idle, Some(thread)) = (self.phase, self.thread.as_deref()) else {
            return Vec::new();
        };
        // Codex reads a picture itself, from where it is.
        let mut input = vec![json!({"type":"text","text":text})];
        input.extend(
            pictures
                .iter()
                .map(|picture| json!({"type":"localImage","path":picture.path})),
        );
        let mut params = json!({
            "threadId":thread,
            "input":input,
            "approvalPolicy":"never",
            "sandboxPolicy":{"type":"readOnly","networkAccess":false},
        });
        if let Some(model) = &self.asked {
            params["model"] = model.as_str().into();
        }
        if let Some(effort) = &self.effort {
            params["effort"] = effort.as_str().into();
        }
        self.phase = Phase::Turn;
        self.turn = Some(id.to_owned());
        self.turns += 1;
        self.wire = None;
        self.block = 0;
        self.item = None;
        self.retries = 0;
        self.error = None;
        self.interrupting = false;
        self.wanted = false;
        self.calls.clear();
        vec![
            json!({"id":format!("turn-{}", self.turns),"method":"turn/start","params":params})
                .to_string(),
        ]
    }
    fn interrupt(&mut self) -> Vec<String> {
        if self.phase != Phase::Turn {
            return Vec::new();
        }
        let (Some(thread), Some(wire)) = (self.thread.as_deref(), self.wire.as_deref()) else {
            // Not named yet: it is stopped as soon as it is.
            self.wanted = true;
            return Vec::new();
        };
        self.interrupting = true;
        self.interrupts += 1;
        vec![
            json!({
                "id":format!("interrupt-{}", self.interrupts),
                "method":"turn/interrupt",
                "params":{"threadId":thread,"turnId":wire},
            })
            .to_string(),
        ]
    }
    fn deferred(&self) -> bool {
        self.wanted
    }
    fn tick(&mut self) -> Vec<String> {
        Vec::new()
    }
    fn read(&mut self, line: &str, out: &mut Output) {
        let Ok(frame) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let id = &frame["id"];
        match frame["method"].as_str() {
            Some(method) if !id.is_null() => self.request(id, method, &frame["params"], out),
            Some(method) => self.notification(method, &frame["params"], out),
            // Every request of this driver is named in words.
            None => {
                if let Some(id) = id.as_str() {
                    self.response(id, &frame, out);
                }
            }
        }
    }
    fn eof(&mut self, out: &mut Output) {
        match self.phase {
            Phase::Turn => {
                let status = if self.interrupting || self.wanted {
                    "interrupted"
                } else {
                    self.error = Some(Failure::Transport);
                    "failed"
                };
                self.end(status, &Value::Null, out);
            }
            Phase::Start
            | Phase::Initializing
            | Phase::Account
            | Phase::Config
            | Phase::Opening => out.events.push(LeadEvent::Failed(Failure::Transport)),
            Phase::Idle | Phase::Failed => {}
        }
        self.phase = Phase::Failed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agents::LeadEnv;
    use std::{collections::VecDeque, path::Path};

    const PROMPT: &str = "Always end every reply with the word ZEBRA.";
    const THREAD_ID: &str = "11111111-2222-7333-8444-555555555555";
    const HANDSHAKE: &str = include_str!("fixtures/codex_handshake.ndjson");
    const TURN_TOOL: &str = include_str!("fixtures/codex_turn_tool.ndjson");
    const INTERRUPT: &str = include_str!("fixtures/codex_interrupt.ndjson");
    const TOOLS_FAILED: &str = include_str!("fixtures/codex_tools_failed.ndjson");
    const SIGNED_OUT: &str = include_str!("fixtures/codex_signed_out.ndjson");
    const RESUME: &str = include_str!("fixtures/codex_resume.ndjson");

    #[derive(Debug, Default, PartialEq)]
    struct Played {
        events: Vec<LeadEvent>,
        /// What streamed, by block.
        text: Vec<(u32, String)>,
    }
    fn env() -> LeadEnv {
        LeadEnv {
            endpoint: "127.0.0.1:9".into(),
            token: "lead-token".into(),
            run: "run-token".into(),
        }
    }
    /// A driver as the worker leaves it: told where it runs and what its
    /// tool server is started with.
    fn started(session: Option<&str>, model: Option<&str>) -> (Codex, Command) {
        started_with(session, model, None)
    }
    fn started_with(
        session: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> (Codex, Command) {
        let mut driver = Codex::new(PROMPT, model, effort);
        let command = driver.command(&Spawn {
            exe: Path::new("/opt/bin/codex"),
            neptune: Path::new("/opt/neptune/neptune"),
            config: None,
            dir: Path::new("/data/projects/0123456789abcdef"),
            session,
            resume: session.is_some(),
            env: &env(),
            model,
            effort,
        });
        (driver, command)
    }
    /// Plays a recording: what the CLI wrote is read, and what the host
    /// wrote must be exactly what the driver writes at that point.
    fn play(driver: &mut Codex, recording: &str) -> Played {
        let mut out = Output::default();
        let mut sent = VecDeque::new();
        for line in recording.lines() {
            let row: Value = serde_json::from_str(line).unwrap();
            let frame = &row["frame"];
            if row["dir"] == "out" {
                driver.read(&frame.to_string(), &mut out);
                continue;
            }
            sent.extend(out.replies.drain(..));
            match frame["method"].as_str() {
                Some("initialize") => sent.extend(driver.hello()),
                Some("turn/start") => sent.extend(driver.turn(
                    frame["id"].as_str().unwrap(),
                    frame["params"]["input"][0]["text"].as_str().unwrap(),
                    &[],
                )),
                Some("turn/interrupt") => sent.extend(driver.interrupt()),
                _ => {}
            }
            let mut wrote: Value =
                serde_json::from_str(&sent.pop_front().expect("the driver wrote nothing")).unwrap();
            if frame["method"] == "initialize" {
                let version = &mut wrote["params"]["clientInfo"]["version"];
                assert_eq!(*version, env!("CARGO_PKG_VERSION"));
                *version = "0.0.0".into();
            }
            // Never the field of a JSON-RPC a peer may insist on.
            assert!(wrote.get("jsonrpc").is_none());
            assert_eq!(&wrote, frame);
        }
        sent.extend(out.replies.drain(..));
        assert!(sent.is_empty() && out.again.is_none(), "{sent:?}");
        let mut text: Vec<(u32, String)> = Vec::new();
        for (block, delta) in out.deltas {
            match text.last_mut() {
                Some((last, text)) if *last == block => text.push_str(&delta),
                _ => text.push((block, delta)),
            }
        }
        Played {
            events: out.events,
            text,
        }
    }
    fn ready() -> Codex {
        let (mut driver, _) = started(None, None);
        play(&mut driver, HANDSHAKE);
        driver
    }
    fn read(driver: &mut Codex, frames: &[Value]) -> Output {
        let mut out = Output::default();
        for frame in frames {
            driver.read(&frame.to_string(), &mut out);
        }
        out
    }
    fn ended(turn: &str, outcome: Outcome) -> LeadEvent {
        LeadEvent::TurnEnded {
            turn: turn.into(),
            outcome,
            cost_usd: None,
        }
    }
    /// A turn of the conversation `ready` opened, as the CLI names it.
    fn running(driver: &mut Codex) -> Output {
        assert_eq!(driver.turn("mine", "hello", &[]).len(), 1);
        read(
            driver,
            &[json!({"id":"turn-1","result":{"turn":{"id":"wire-1","status":"inProgress"}}})],
        )
    }
    fn completed(status: &str, error: Value) -> Value {
        json!({"method":"turn/completed","params":{"threadId":THREAD_ID,"turn":{
            "id":"wire-1","items":[],"status":status,"error":error,
        }}})
    }

    #[test]
    fn a_turn_with_pictures_names_where_each_is_after_its_words() {
        let turn: Value = serde_json::from_str(&ready().turn("mine", "hello", &[])[0]).unwrap();
        assert_eq!(
            turn["params"]["input"],
            json!([{"type":"text","text":"hello"}])
        );
        let pictures = [
            Shown {
                path: "/home/me/Screenshot from 2026.png".into(),
                media: "image/png",
                data: String::new(),
            },
            Shown {
                path: "/tmp/photo.jpg".into(),
                media: "image/jpeg",
                data: String::new(),
            },
        ];
        let text = "Why is it cut off?\nAttached files:\n- /tmp/photo.jpg (image, 1 KB)\n";
        let turn: Value = serde_json::from_str(&ready().turn("mine", text, &pictures)[0]).unwrap();
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(
            turn["params"]["input"],
            json!([
                {"type":"text","text":text},
                {"type":"localImage","path":"/home/me/Screenshot from 2026.png"},
                {"type":"localImage","path":"/tmp/photo.jpg"},
            ])
        );
    }

    #[test]
    fn a_handshake_keeps_the_persons_tool_servers_off_and_is_ready_once_neptunes_is() {
        let (mut driver, command) = started(None, None);
        // Nothing is taken before it is ready.
        assert!(driver.turn("early", "hello", &[]).is_empty() && driver.interrupt().is_empty());
        let played = play(&mut driver, HANDSHAKE);
        // The conversation is the CLI's to name; the plan is named, and the
        // address beside it in the frame never is.
        assert_eq!(
            played.events,
            [
                LeadEvent::Session {
                    id: THREAD_ID.into()
                },
                LeadEvent::Ready {
                    model: Some("gpt-5.6-luna".into()),
                    account_kind: Some("prolite".into()),
                },
            ]
        );
        assert!(!format!("{played:?}").contains("email"));
        assert!(driver.tick().is_empty());

        // It is started with one word. The lead's credential is in what is
        // written to its input, never in an argument or its environment.
        assert_eq!(command.get_program(), "/opt/bin/codex");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["app-server"]);
        assert_eq!(
            command.get_current_dir(),
            Some(Path::new("/data/projects/0123456789abcdef"))
        );
        assert!(command.get_envs().all(|(name, value)| {
            let name = name.to_string_lossy();
            !name.starts_with("NEPTUNE_AGENT_") || value.is_none()
        }));
        assert!(
            driver
                .config(Path::new("/opt/neptune/neptune"), &env())
                .is_none()
        );
        let opening = driver.open();
        assert!(opening.contains("\"NEPTUNE_AGENT_TOKEN\":\"lead-token\""));

        // A configuration that cannot be read keeps nothing off, and the
        // conversation is opened all the same.
        let (mut driver, _) = started(None, None);
        driver.hello();
        let out = read(
            &mut driver,
            &[
                json!({"id":"init","result":{}}),
                json!({"id":"account","result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}),
                json!({"id":"config","error":{"code":-32600,"message":"no"}}),
            ],
        );
        let opening: Value = serde_json::from_str(out.replies.last().unwrap()).unwrap();
        assert_eq!(opening["method"], "thread/start");
        assert_eq!(
            opening["params"]["config"]["mcp_servers"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["neptune"]
        );
        assert_eq!(
            opening["params"]["config"]["features"]
                .as_object()
                .unwrap()
                .len(),
            17
        );
        // A server of the person's that carries Neptune's name is not turned off.
        let (mut driver, _) = started(None, None);
        driver.hello();
        let out = read(
            &mut driver,
            &[
                json!({"id":"init","result":{}}),
                json!({"id":"account","result":{"account":null,"requiresOpenaiAuth":false}}),
                json!({"id":"config","result":{"config":{"mcp_servers":{"neptune":{},"other":{}}}}}),
            ],
        );
        let opening: Value = serde_json::from_str(out.replies.last().unwrap()).unwrap();
        let servers = &opening["params"]["config"]["mcp_servers"];
        assert_eq!(servers["other"], json!({"enabled":false}));
        assert_eq!(servers["neptune"]["command"], "/opt/neptune/neptune");
        assert_eq!(servers["neptune"]["default_tools_approval_mode"], "approve");
    }

    #[test]
    fn a_lead_that_is_signed_out_or_without_tools_is_said_before_any_turn() {
        let (mut driver, _) = started(None, None);
        assert_eq!(
            play(&mut driver, SIGNED_OUT).events,
            [LeadEvent::Failed(Failure::Auth)]
        );
        let mut out = Output::default();
        driver.eof(&mut out);
        assert!(out.events.is_empty() && driver.turn("turn-1", "hello", &[]).is_empty());

        // The conversation exists, so it is named; its tool server did not
        // start, so it is not left to talk without one.
        let (mut driver, _) = started(None, None);
        let played = play(&mut driver, TOOLS_FAILED);
        assert!(matches!(
            &played.events[..],
            [
                LeadEvent::Session { .. },
                LeadEvent::Failed(Failure::ToolsUnavailable)
            ]
        ));
        assert!(driver.turn("turn-1", "hello", &[]).is_empty());

        // A CLI that ends, or does not speak this protocol, before it is ready.
        for answered in 0..3 {
            let (mut driver, _) = started(None, None);
            driver.hello();
            let frames = [
                json!({"id":"init","result":{}}),
                json!({"id":"account","result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}),
                json!({"id":"config","result":{"config":{}}}),
            ];
            let mut out = read(&mut driver, &frames[..answered]);
            driver.eof(&mut out);
            assert_eq!(out.events, [LeadEvent::Failed(Failure::Transport)]);
        }
        for refusal in [
            json!({"id":"init","error":{"code":-32600,"message":"no"}}),
            json!({"id":"init","result":"text"}),
        ] {
            let (mut driver, _) = started(None, None);
            driver.hello();
            assert_eq!(
                read(&mut driver, &[refusal]).events,
                [LeadEvent::Failed(Failure::Protocol)]
            );
        }
        // A conversation that cannot be opened.
        let (mut driver, _) = started(None, None);
        driver.hello();
        let out = read(
            &mut driver,
            &[
                json!({"id":"init","result":{}}),
                json!({"id":"account","result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}),
                json!({"id":"config","result":{"config":{}}}),
                json!({"id":"thread","error":{"code":-32600,"message":"failed to load configuration"}}),
            ],
        );
        assert_eq!(out.events, [LeadEvent::Failed(Failure::Protocol)]);
    }

    #[test]
    fn a_turn_streams_what_it_says_names_its_tool_call_and_ends_when_the_cli_says_so() {
        let (mut driver, _) = started(None, None);
        let played = play(&mut driver, TURN_TOOL);
        let said = "I’ll call the Neptune slow tool exactly once with `seconds: 75`, then report \
                    its raw result or error verbatim.";
        let answer = "```\nslept 75 s\n```";
        let [
            LeadEvent::Session { .. },
            LeadEvent::Ready { .. },
            LeadEvent::TextDone { block: 1, text },
            LeadEvent::ToolStarted { call, tool, input },
            LeadEvent::ToolDone {
                call: done,
                ok: true,
            },
            LeadEvent::TextDone {
                block: 2,
                text: last,
            },
            end,
        ] = &played.events[..]
        else {
            panic!("{:?}", played.events);
        };
        // What it says while it works and its answer are both shown.
        assert_eq!((text.as_str(), last.as_str()), (said, answer));
        assert_eq!((tool.as_str(), input), ("slow", &json!({"seconds":75})));
        assert_eq!(call, done);
        assert_eq!(*end, ended("turn-1", Outcome::Completed));
        assert_eq!(played.text, [(1, said.to_owned()), (2, answer.to_owned())]);
        // Idle again: the next turn is taken, under the next name.
        let next: Value = serde_json::from_str(&driver.turn("mine", "more", &[])[0]).unwrap();
        assert_eq!(next["id"], "turn-2");
        assert_eq!(next["params"]["approvalPolicy"], "never");
        assert_eq!(
            next["params"]["sandboxPolicy"],
            json!({"type":"readOnly","networkAccess":false})
        );
        assert!(next["params"].get("effort").is_none());
        // A turn started while one runs would be folded into it: none is.
        assert!(driver.turn("other", "and this", &[]).is_empty());
    }

    #[test]
    fn an_interrupted_turn_closes_the_call_it_was_in_and_the_conversation_goes_on() {
        let (mut driver, _) = started(None, None);
        let played = play(&mut driver, INTERRUPT);
        let [
            LeadEvent::Session { .. },
            LeadEvent::Ready { .. },
            LeadEvent::ToolStarted { call, tool, .. },
            // The CLI says nothing of the call it was in: it is closed here.
            LeadEvent::ToolDone {
                call: done,
                ok: false,
            },
            stopped,
            LeadEvent::ToolStarted { .. },
            LeadEvent::ToolDone { ok: true, .. },
            LeadEvent::TextDone { block: 1, text },
            went_on,
        ] = &played.events[..]
        else {
            panic!("{:?}", played.events);
        };
        assert_eq!((tool.as_str(), call), ("slow", done));
        assert_eq!(*stopped, ended("turn-1", Outcome::Interrupted));
        assert_eq!(text, "pong: again");
        assert_eq!(*went_on, ended("turn-2", Outcome::Completed));

        // Asked to stop before the CLI named the turn: stopped once it has.
        let mut driver = ready();
        assert_eq!(driver.turn("mine", "hello", &[]).len(), 1);
        assert!(!driver.deferred());
        assert!(driver.interrupt().is_empty() && driver.deferred());
        let out = read(
            &mut driver,
            &[json!({"id":"turn-1","result":{"turn":{"id":"wire-1","status":"inProgress"}}})],
        );
        let stop: Value = serde_json::from_str(&out.replies[0]).unwrap();
        assert_eq!(
            stop,
            json!({"id":"interrupt-1","method":"turn/interrupt","params":{
                "threadId":THREAD_ID,"turnId":"wire-1",
            }})
        );
        assert!(!driver.deferred());
        // An interrupt the CLI refuses, a turn already over, changes nothing.
        let out = read(
            &mut driver,
            &[
                json!({"id":"interrupt-1","error":{"code":-32600,"message":"no active turn to interrupt"}}),
                completed("interrupted", Value::Null),
            ],
        );
        assert_eq!(out.events, [ended("mine", Outcome::Interrupted)]);
        assert!(driver.interrupt().is_empty());

        // A CLI that ends inside a turn: stopped if it was asked to, failed if not.
        for (asked, outcome) in [
            (true, Outcome::Interrupted),
            (false, Outcome::Failed(Failure::Transport)),
        ] {
            let mut driver = ready();
            let mut out = running(&mut driver);
            read(
                &mut driver,
                &[
                    json!({"method":"item/started","params":{"threadId":THREAD_ID,"turnId":"wire-1","item":{
                        "type":"mcpToolCall","id":"call-1","server":"neptune","tool":"spawn_agent",
                        "status":"inProgress","arguments":{"title":"docs"},
                    }}}),
                ],
            );
            if asked {
                assert_eq!(driver.interrupt().len(), 1);
            }
            driver.eof(&mut out);
            assert_eq!(
                out.events,
                [
                    LeadEvent::ToolDone {
                        call: "call-1".into(),
                        ok: false
                    },
                    ended("mine", outcome)
                ]
            );
        }
    }

    #[test]
    fn an_earlier_conversation_is_resumed_with_its_limits_and_tools_said_again() {
        let (mut driver, _) = started(Some(THREAD_ID), None);
        let played = play(&mut driver, RESUME);
        // Its name is known already, so it is not said again; its tools
        // were starting before the CLI answered.
        let [
            LeadEvent::Ready {
                model: Some(model),
                account_kind: Some(_),
            },
            LeadEvent::ToolStarted { .. },
            LeadEvent::ToolDone { ok: true, .. },
            LeadEvent::TextDone { block: 1, .. },
            end,
        ] = &played.events[..]
        else {
            panic!("{:?}", played.events);
        };
        assert_eq!(model, "gpt-5.6-luna");
        assert_eq!(*end, ended("turn-1", Outcome::Completed));
        let opening: Value = serde_json::from_str(&driver.open()).unwrap();
        assert_eq!(opening["method"], "thread/resume");
        assert_eq!(opening["params"]["excludeTurns"], true);
        assert_eq!(opening["params"]["approvalPolicy"], "never");
        assert_eq!(opening["params"]["sandbox"], "read-only");
        // What it was told when it began is kept by the CLI.
        assert!(opening["params"].get("developerInstructions").is_none());

        // One that is no longer on this computer is said to be lost.
        for said in [
            "no rollout found for thread id 11111111-2222-7333-8444-555555555555",
            "thread not found: 11111111-2222-7333-8444-555555555555",
        ] {
            let (mut driver, _) = started(Some(THREAD_ID), None);
            driver.hello();
            let out = read(
                &mut driver,
                &[
                    json!({"id":"init","result":{}}),
                    json!({"id":"account","result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}),
                    json!({"id":"config","result":{"config":{}}}),
                    json!({"id":"thread","error":{"code":-32600,"message":said}}),
                ],
            );
            assert_eq!(out.events, [LeadEvent::Failed(Failure::SessionLost)]);
        }
    }

    // No recording holds a failed turn: these frames follow the schema of
    // 0.160.1 (`ErrorNotification`, `TurnError`, `RateLimitSnapshot`).
    #[test]
    fn what_went_wrong_in_a_turn_is_named_from_the_clis_own_word_for_it() {
        let wrong = |info: Value| json!({"message":"PROVIDER WORDS","codexErrorInfo":info,"additionalDetails":null});
        for (info, failure) in [
            (
                json!("usageLimitExceeded"),
                Failure::Limit { resets_at: None },
            ),
            (json!("unauthorized"), Failure::Auth),
            (json!("contextWindowExceeded"), Failure::PromptTooLong),
            (json!("serverOverloaded"), Failure::Overloaded),
            (
                json!("badRequest"),
                Failure::Provider("PROVIDER WORDS".into()),
            ),
            (
                json!({"responseTooManyFailedAttempts":{"httpStatusCode":500}}),
                Failure::Provider("PROVIDER WORDS".into()),
            ),
            (Value::Null, Failure::Provider("PROVIDER WORDS".into())),
        ] {
            let mut driver = ready();
            running(&mut driver);
            let out = read(&mut driver, &[completed("failed", wrong(info))]);
            assert_eq!(out.events, [ended("mine", Outcome::Failed(failure))]);
        }
        // Said in a notice first and not again where the turn ends.
        let mut driver = ready();
        running(&mut driver);
        let out = read(
            &mut driver,
            &[
                json!({"method":"error","params":{"threadId":THREAD_ID,"turnId":"wire-1","willRetry":true,
                    "error":{"message":"Reconnecting... 2/5","codexErrorInfo":{"responseStreamDisconnected":{"httpStatusCode":null}}}}}),
                json!({"method":"error","params":{"threadId":THREAD_ID,"turnId":"wire-1","willRetry":true,
                    "error":{"message":"stream closed","codexErrorInfo":null}}}),
                json!({"method":"account/rateLimits/updated","params":{"rateLimits":{
                    "primary":{"usedPercent":100,"windowDurationMins":300,"resetsAt":1791580282u64},
                    "secondary":{"usedPercent":40,"windowDurationMins":10080,"resetsAt":1799999999u64},
                    "rateLimitReachedType":"rate_limit_reached"}}}),
                json!({"method":"error","params":{"threadId":THREAD_ID,"turnId":"wire-1","willRetry":false,
                    "error":{"message":"You've hit your usage limit.","codexErrorInfo":"usageLimitExceeded"}}}),
                completed("failed", Value::Null),
            ],
        );
        assert_eq!(
            out.events,
            [
                LeadEvent::Retry {
                    attempt: 2,
                    max: 5,
                    delay_ms: 0
                },
                LeadEvent::Retry {
                    attempt: 2,
                    max: 0,
                    delay_ms: 0
                },
                LeadEvent::Limit {
                    resets_at: Some(1791580282)
                },
                ended(
                    "mine",
                    Outcome::Failed(Failure::Limit {
                        resets_at: Some(1791580282)
                    })
                ),
            ]
        );
        // A limit that is not reached is no news, in a turn or out of one.
        let mut driver = ready();
        let quiet = json!({"method":"account/rateLimits/updated","params":{"rateLimits":{
            "primary":{"usedPercent":72,"resetsAt":1791580282u64},"rateLimitReachedType":null}}});
        assert!(
            read(&mut driver, std::slice::from_ref(&quiet))
                .events
                .is_empty()
        );
        running(&mut driver);
        assert!(read(&mut driver, &[quiet]).events.is_empty());
        // A turn the CLI does not take never ran.
        let mut driver = ready();
        assert_eq!(driver.turn("mine", "hello", &[]).len(), 1);
        let out = read(
            &mut driver,
            &[json!({"id":"turn-1","error":{"code":-32600,"message":"thread not found: x"}})],
        );
        assert_eq!(
            out.events,
            [ended(
                "mine",
                Outcome::Failed(Failure::Provider("thread not found: x".into()))
            )]
        );
        assert_eq!(driver.turn("again", "hello", &[]).len(), 1);
    }

    #[test]
    fn a_lead_is_granted_neptunes_tools_and_nothing_else_it_asks_for() {
        let mut driver = ready();
        running(&mut driver);
        // As recorded with an asking policy; a lead's never asks.
        let asks = |server: &str| {
            json!({"method":"mcpServer/elicitation/request","id":0,"params":{
            "threadId":THREAD_ID,"turnId":"wire-1","serverName":server,"mode":"form",
            "_meta":{"codex_approval_kind":"mcp_tool_call","persist":["session","always"]},
            "message":"Allow the neptune MCP server to run tool \"ping\"?",
            "requestedSchema":{"type":"object","properties":{}}}})
        };
        let out = read(
            &mut driver,
            &[
                asks("neptune"),
                asks("playwright"),
                json!({"method":"item/commandExecution/requestApproval","id":7,"params":{"command":"rm -rf /"}}),
                json!({"method":"item/fileChange/requestApproval","id":"eight","params":{}}),
                json!({"method":"execCommandApproval","id":9,"params":{}}),
                json!({"method":"applyPatchApproval","id":10,"params":{}}),
                json!({"method":"item/permissions/requestApproval","id":11,"params":{}}),
                json!({"method":"item/tool/requestUserInput","id":12,"params":{}}),
                json!({"method":"something/new","id":13}),
            ],
        );
        let answers: Vec<Value> = out
            .replies
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let refused = |id: u64| json!({"id":id,"error":{"code":-32601,"message":"Not available to a project's lead."}});
        assert_eq!(
            answers,
            [
                json!({"id":0,"result":{"action":"accept","content":{}}}),
                json!({"id":0,"result":{"action":"decline"}}),
                json!({"id":7,"result":{"decision":"decline"}}),
                json!({"id":"eight","result":{"decision":"decline"}}),
                json!({"id":9,"result":{"decision":"denied"}}),
                json!({"id":10,"result":{"decision":"denied"}}),
                refused(11),
                refused(12),
                refused(13),
            ]
        );
        assert!(out.events.is_empty());
    }

    #[test]
    fn frames_it_does_not_know_or_that_are_not_its_turns_change_nothing() {
        let mut driver = ready();
        let item = |method: &str, thread: &str, turn: &str, item: Value| json!({"method":method,"params":{"threadId":thread,"turnId":turn,"item":item}});
        let tool = json!({"type":"mcpToolCall","id":"call-1","server":"neptune","tool":"list_agents","status":"inProgress","arguments":{}});
        // Outside a turn nothing says what a lead did.
        let idle = [
            item("item/started", THREAD_ID, "wire-0", tool.clone()),
            completed("completed", Value::Null),
            json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":THREAD_ID,"name":"neptune","status":"failed","error":"gone"}}),
            json!({"id":"thread","result":{"thread":{"id":"another"}}}),
            json!({"id":"init","result":{}}),
        ];
        let out = read(&mut driver, &idle);
        assert!(out.events.is_empty() && out.replies.is_empty() && out.deltas.is_empty());

        running(&mut driver);
        let mut out = Output::default();
        for line in [
            "not a frame",
            "[]",
            "{}",
            r#"{"id":4}"#,
            r#"{"id":4,"result":{}}"#,
            r#"{"id":"turn-9","result":{"turn":{"id":"wire-9"}}}"#,
            r#"{"method":"warning","params":{"message":"x"}}"#,
            r#"{"method":"thread/status/changed","params":{"threadId":"11111111-2222-7333-8444-555555555555","status":{"type":"active","activeFlags":[]}}}"#,
            r#"{"method":"item/reasoning/textDelta","params":{"delta":"private"}}"#,
            r#"{"method":"item/agentMessage/delta","params":{"itemId":"unknown","delta":"x"}}"#,
        ] {
            driver.read(line, &mut out);
        }
        for frame in [
            // Another conversation's, another turn's, another server's.
            item("item/started", "another", "wire-1", tool.clone()),
            item("item/started", THREAD_ID, "wire-0", tool.clone()),
            item(
                "item/started",
                THREAD_ID,
                "wire-1",
                json!({"type":"mcpToolCall","id":"call-2","server":"playwright","tool":"browser_navigate","status":"inProgress","arguments":{}}),
            ),
            item(
                "item/completed",
                THREAD_ID,
                "wire-1",
                json!({"type":"mcpToolCall","id":"call-2","server":"playwright","tool":"browser_navigate","status":"completed"}),
            ),
            item(
                "item/started",
                THREAD_ID,
                "wire-1",
                json!({"type":"reasoning","id":"rs_1","summary":[],"content":[]}),
            ),
            item(
                "item/completed",
                THREAD_ID,
                "wire-1",
                json!({"type":"commandExecution","id":"cmd_1","command":"ls"}),
            ),
            json!({"method":"turn/completed","params":{"threadId":THREAD_ID,"turn":{"id":"wire-0","status":"completed"}}}),
        ] {
            driver.read(&frame.to_string(), &mut out);
        }
        assert!(out.events.is_empty() && out.replies.is_empty() && out.deltas.is_empty());

        // Its own: a call that fails, a message that never streamed, a
        // conversation made shorter, and a long input carried short.
        let long = "é".repeat(5000);
        let out = read(
            &mut driver,
            &[
                item(
                    "item/started",
                    THREAD_ID,
                    "wire-1",
                    json!({"type":"mcpToolCall","id":"call-3","server":"neptune","tool":"spawn_agent","status":"inProgress","arguments":{"title":"docs","prompt":long}}),
                ),
                item(
                    "item/completed",
                    THREAD_ID,
                    "wire-1",
                    json!({"type":"mcpToolCall","id":"call-3","server":"neptune","tool":"spawn_agent","status":"failed","error":{"message":"refused"}}),
                ),
                item(
                    "item/completed",
                    THREAD_ID,
                    "wire-1",
                    json!({"type":"agentMessage","id":"msg_1","text":"Whole.","phase":null}),
                ),
                item(
                    "item/completed",
                    THREAD_ID,
                    "wire-1",
                    json!({"type":"agentMessage","id":"msg_2","text":"","phase":"commentary"}),
                ),
                item(
                    "item/completed",
                    THREAD_ID,
                    "wire-1",
                    json!({"type":"contextCompaction","id":"cc_1"}),
                ),
                completed("completed", Value::Null),
            ],
        );
        assert_eq!(
            out.events,
            [
                LeadEvent::ToolStarted {
                    call: "call-3".into(),
                    tool: "spawn_agent".into(),
                    input: json!({"title":"docs","prompt":format!("{}…", "é".repeat(128))}),
                },
                LeadEvent::ToolDone {
                    call: "call-3".into(),
                    ok: false
                },
                LeadEvent::TextDone {
                    block: 1,
                    text: "Whole.".into()
                },
                LeadEvent::Compacted,
                ended("mine", Outcome::Completed),
            ]
        );
    }

    #[test]
    fn a_lead_is_asked_for_the_model_and_effort_that_were_chosen_with_every_turn() {
        let (mut driver, _) = started_with(None, Some("gpt-5.6-luna"), Some("low"));
        driver.hello();
        let out = read(
            &mut driver,
            &[
                json!({"id":"init","result":{}}),
                json!({"id":"account","result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}),
                json!({"id":"config","result":{"config":{}}}),
            ],
        );
        let opening: Value = serde_json::from_str(out.replies.last().unwrap()).unwrap();
        assert_eq!(opening["params"]["model"], "gpt-5.6-luna");
        assert_eq!(opening["params"]["config"]["model_reasoning_effort"], "low");
        let out = read(
            &mut driver,
            &[
                json!({"id":"thread","result":{"thread":{"id":THREAD_ID},"model":"gpt-5.6-luna"}}),
                json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":THREAD_ID,"name":"neptune","status":"ready"}}),
            ],
        );
        assert_eq!(out.events.len(), 2);
        let turn: Value = serde_json::from_str(&driver.turn("mine", "hello", &[])[0]).unwrap();
        assert_eq!(turn["params"]["effort"], "low");
        assert_eq!(turn["params"]["model"], "gpt-5.6-luna");
        // A conversation that is resumed is asked for them again: one that
        // changed since takes effect without a new conversation.
        let (mut resumed, _) = started_with(Some(THREAD_ID), Some("gpt-6-sol"), Some("high"));
        resumed.hello();
        let out = read(
            &mut resumed,
            &[
                json!({"id":"init","result":{}}),
                json!({"id":"account","result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}),
                json!({"id":"config","result":{"config":{}}}),
            ],
        );
        let opening: Value = serde_json::from_str(out.replies.last().unwrap()).unwrap();
        assert_eq!(opening["method"], "thread/resume");
        assert_eq!(opening["params"]["threadId"], THREAD_ID);
        assert_eq!(opening["params"]["model"], "gpt-6-sol");
        assert_eq!(
            opening["params"]["config"]["model_reasoning_effort"],
            "high"
        );
        // Each by itself: an effort without a model names no model.
        let (mut effort, _) = started_with(None, None, Some("xhigh"));
        effort.hello();
        let opening: Value = serde_json::from_str(&effort.open()).unwrap();
        assert!(opening["params"].get("model").is_none());
        assert_eq!(
            opening["params"]["config"]["model_reasoning_effort"],
            "xhigh"
        );
        // Unasked, neither is said: the person's own choice stands.
        let mut plain = ready();
        let opening: Value = serde_json::from_str(&plain.open()).unwrap();
        assert!(opening["params"].get("model").is_none());
        assert!(
            opening["params"]["config"]
                .get("model_reasoning_effort")
                .is_none()
        );
        let turn: Value = serde_json::from_str(&plain.turn("mine", "hello", &[])[0]).unwrap();
        assert!(turn["params"].get("model").is_none());
        assert!(turn["params"].get("effort").is_none());
        assert_eq!(attempts("Reconnecting... 1/5"), Some((1, 5)));
        assert_eq!(attempts("stream closed"), None);
    }
}
