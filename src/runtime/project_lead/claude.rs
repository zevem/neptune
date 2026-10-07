//! Claude Code's stream-json protocol as lines in and lines out. Frames are
//! read by key, never by key order, and what is not known is passed over.
use super::{Driver, Failure, LeadEvent, Outcome, Output, Shown, Spawn, clip, shape};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{process::Command, time::Duration};

const INIT: &str = "init-1";
const STATUS: &str = "mcp-1";
/// The one tool server a lead has, and the prefix of every tool it offers.
const SERVER: &str = "neptune";
const TOOLS: &str = "mcp__neptune__";
/// A tool server still connecting is asked about again this much later.
const STATUS_RETRY: Duration = Duration::from_millis(250);
const STATUS_TRIES: u32 = 40;
/// Tool calls of one turn that have not returned.
const MAX_CALLS: usize = 64;
/// What a provider says of a failure, for the chat's error row.
const MAX_REASON: usize = 300;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Initializing,
    Checking,
    Idle,
    Turn,
    /// Start-up failed and was said so; nothing more follows.
    Failed,
}
pub(super) struct Claude {
    prompt: String,
    /// Unique to this opening, so a turn's wire id never repeats in a session.
    seed: String,
    model: Option<String>,
    phase: Phase,
    account: Option<String>,
    checks: u32,
    turn: Option<String>,
    block: u32,
    streaming: bool,
    calls: Vec<String>,
    interrupts: u32,
    interrupting: bool,
    /// The error an `assistant` frame of this turn named.
    error: Option<String>,
    /// When the last refused rate limit window ends.
    limit: Option<u64>,
    /// The process's running cost estimate at the last turn end.
    cost: f64,
}
impl Claude {
    pub(super) fn new(prompt: &str, seed: &str, model: Option<&str>) -> Self {
        Self {
            prompt: prompt.to_owned(),
            seed: seed.to_owned(),
            model: model.map(str::to_owned),
            phase: Phase::Start,
            account: None,
            checks: 0,
            turn: None,
            block: 0,
            streaming: false,
            calls: Vec::new(),
            interrupts: 0,
            interrupting: false,
            error: None,
            limit: None,
            cost: 0.0,
        }
    }
    fn status(&mut self) -> String {
        self.checks += 1;
        json!({"type":"control_request","request_id":STATUS,"request":{"subtype":"mcp_status"}})
            .to_string()
    }
    fn fail(&mut self, failure: Failure, out: &mut Output) {
        self.phase = Phase::Failed;
        out.events.push(LeadEvent::Failed(failure));
    }
    fn response(&mut self, frame: &Value, out: &mut Output) {
        let response = &frame["response"];
        let success = response["subtype"] == "success";
        let body = &response["response"];
        match (self.phase, response["request_id"].as_str()) {
            (Phase::Initializing, Some(INIT)) if !success => self.fail(Failure::Protocol, out),
            (Phase::Initializing, Some(INIT)) => {
                let account = &body["account"];
                // Signed out, the CLI still answers and names no credential.
                if account["tokenSource"] == "none" && account["subscriptionType"].is_null() {
                    return self.fail(Failure::Auth, out);
                }
                // The plan's name only: who is signed in is never carried.
                self.account = account["subscriptionType"]
                    .as_str()
                    .map(|kind| clip(kind, 64).to_owned());
                let wanted = self.model.as_deref().unwrap_or("default");
                if let Some(model) = body["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|model| model["value"] == wanted)
                    .and_then(|model| model["resolvedModel"].as_str())
                {
                    self.model = Some(clip(model, 128).to_owned());
                }
                self.phase = Phase::Checking;
                out.replies.push(self.status());
            }
            (Phase::Checking, Some(STATUS)) => {
                let state = body["mcpServers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|server| server["name"] == SERVER)
                    .and_then(|server| server["status"].as_str());
                match state {
                    Some("connected") if success => {
                        self.phase = Phase::Idle;
                        out.events.push(LeadEvent::Ready {
                            model: self.model.clone(),
                            account_kind: self.account.take(),
                        });
                    }
                    Some("pending") if success && self.checks < STATUS_TRIES => {
                        out.again = Some(STATUS_RETRY)
                    }
                    _ => self.fail(Failure::ToolsUnavailable, out),
                }
            }
            _ => {}
        }
    }
    /// A request the CLI makes of its host. A lead is granted nothing.
    fn request(&mut self, frame: &Value, out: &mut Output) {
        let Some(id) = frame["request_id"].as_str() else {
            return;
        };
        let request = &frame["request"];
        let response = if request["subtype"] == "can_use_tool" {
            json!({"subtype":"success","request_id":id,"response":{
                "behavior":"deny",
                "message":"This tool is not available to a project's lead.",
                "toolUseID":request["tool_use_id"],
            }})
        } else {
            json!({"subtype":"error","request_id":id,"error":"Unsupported request"})
        };
        out.replies
            .push(json!({"type":"control_response","response":response}).to_string());
    }
    fn stream(&mut self, event: &Value, out: &mut Output) {
        match event["type"].as_str() {
            Some("content_block_start") if event["content_block"]["type"] == "text" => {
                self.block += 1;
                self.streaming = true;
                out.deltas.push((self.block, String::new()));
            }
            Some("content_block_delta")
                if self.streaming && event["delta"]["type"] == "text_delta" =>
            {
                if let Some(text) = event["delta"]["text"].as_str() {
                    out.deltas.push((self.block, text.to_owned()));
                }
            }
            _ => {}
        }
    }
    fn assistant(&mut self, frame: &Value, out: &mut Output) {
        if let Some(error) = frame["error"].as_str() {
            // The text beside it restates the failure the result carries.
            self.error = Some(error.to_owned());
            return;
        }
        for block in frame["message"]["content"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => {
                    if !std::mem::take(&mut self.streaming) {
                        self.block += 1;
                    }
                    if let Some(text) = block["text"].as_str().filter(|text| !text.is_empty()) {
                        out.events.push(LeadEvent::TextDone {
                            block: self.block,
                            text: clip(text, super::MAX_STREAM).to_owned(),
                        });
                    }
                }
                Some("tool_use") => {
                    let (Some(call), Some(tool)) = (
                        block["id"].as_str(),
                        block["name"]
                            .as_str()
                            .and_then(|name| name.strip_prefix(TOOLS)),
                    ) else {
                        continue;
                    };
                    if self.calls.len() == MAX_CALLS {
                        self.calls.remove(0);
                    }
                    self.calls.push(call.to_owned());
                    out.events.push(LeadEvent::ToolStarted {
                        call: call.to_owned(),
                        tool: clip(tool, 128).to_owned(),
                        input: super::clip_input(&block["input"]),
                    });
                }
                _ => {}
            }
        }
    }
    fn user(&mut self, frame: &Value, out: &mut Output) {
        for block in frame["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] != "tool_result" {
                continue;
            }
            let Some(at) = self
                .calls
                .iter()
                .position(|call| block["tool_use_id"] == call.as_str())
            else {
                continue;
            };
            out.events.push(LeadEvent::ToolDone {
                call: self.calls.remove(at),
                ok: block["is_error"] != true,
            });
        }
    }
    fn result(&mut self, frame: &Value, out: &mut Output) {
        let Some(turn) = self.turn.take() else {
            return;
        };
        let total = frame["total_cost_usd"].as_f64();
        let cost_usd = total.map(|total| (total - self.cost).max(0.0));
        self.cost = total.unwrap_or(self.cost);
        let outcome = classify(frame, self.error.take().as_deref(), self.limit);
        self.phase = Phase::Idle;
        self.streaming = false;
        self.interrupting = false;
        self.calls.clear();
        out.events.push(LeadEvent::TurnEnded {
            turn,
            outcome,
            cost_usd,
        });
    }
}
/// How a turn ended. An interrupted turn can read as a success or as an
/// error, and a failed request as a success, so the order matters.
pub(super) fn classify(result: &Value, named: Option<&str>, limit: Option<u64>) -> Outcome {
    let reason = result["terminal_reason"].as_str().unwrap_or_default();
    if reason.starts_with("aborted_") {
        return Outcome::Interrupted;
    }
    let status = result["api_error_status"].as_u64();
    if result["subtype"] == "success"
        && result["is_error"] != true
        && result["api_error_status"].is_null()
    {
        return Outcome::Completed;
    }
    Outcome::Failed(match (status, reason, named) {
        (Some(401), ..) | (None, _, Some("authentication_failed")) => Failure::Auth,
        (Some(429), ..) | (_, "blocking_limit", _) | (None, _, Some("rate_limit")) => {
            Failure::Limit { resets_at: limit }
        }
        (Some(529), ..) | (None, _, Some("overloaded")) => Failure::Overloaded,
        (_, "prompt_too_long", _) => Failure::PromptTooLong,
        _ => Failure::Provider(
            clip(
                result["errors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .find(|error| !error.starts_with("[ede_diagnostic]"))
                    .or(result["result"].as_str().filter(|text| !text.is_empty()))
                    .or(result["subtype"].as_str())
                    .unwrap_or("error"),
                MAX_REASON,
            )
            .to_owned(),
        ),
    })
}
/// What an enclosing Claude Code session or a debugger would hand the lead.
fn foreign(name: &str) -> bool {
    name.starts_with("CLAUDE_CODE_")
        || matches!(name, "CLAUDECODE" | "NODE_OPTIONS" | "ANTHROPIC_API_KEY")
}
impl Driver for Claude {
    fn config(&self, neptune: &std::path::Path, env: &super::LeadEnv) -> Option<String> {
        let variables: serde_json::Map<String, Value> = env
            .vars()
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.into()))
            .collect();
        Some(
            json!({"mcpServers":{SERVER:{
                "type":"stdio",
                "command":neptune.to_string_lossy(),
                "args":["--agent-mcp"],
                "env":variables,
            }}})
            .to_string(),
        )
    }
    fn command(&mut self, spawn: &Spawn<'_>) -> Command {
        let mut command = Command::new(spawn.exe);
        command
            .args([
                "--output-format",
                "stream-json",
                "--verbose",
                "--input-format",
                "stream-json",
                "--include-partial-messages",
            ])
            .arg(format!(
                "--{}={}",
                if spawn.resume { "resume" } else { "session-id" },
                spawn.session.unwrap_or_default()
            ))
            .args(["--tools", "", "--mcp-config"])
            .arg(spawn.config.unwrap_or(std::path::Path::new("")))
            .args([
                "--strict-mcp-config",
                "--allowedTools",
                "mcp__neptune__*",
                "--permission-mode",
                "dontAsk",
                "--setting-sources",
                "",
                "--disable-slash-commands",
            ]);
        if let Some(model) = spawn.model {
            command.args(["--model", model]);
        }
        if let Some(effort) = spawn.effort {
            command.args(["--effort", effort]);
        }
        super::scrub(&mut command, std::env::vars_os(), foreign);
        command
            .envs(spawn.env.vars())
            .env("CLAUDE_CODE_ENTRYPOINT", "sdk-ts")
            .current_dir(spawn.dir);
        command
    }
    fn hello(&mut self) -> Vec<String> {
        self.phase = Phase::Initializing;
        vec![
            json!({"type":"control_request","request_id":INIT,"request":{
                "subtype":"initialize",
                "appendSystemPrompt":self.prompt,
            }})
            .to_string(),
        ]
    }
    fn embeds(&self) -> bool {
        true
    }
    fn turn(&mut self, id: &str, text: &str, pictures: &[Shown]) -> Vec<String> {
        if self.phase != Phase::Idle {
            return Vec::new();
        }
        self.phase = Phase::Turn;
        self.turn = Some(id.to_owned());
        self.block = 0;
        self.streaming = false;
        self.interrupting = false;
        self.error = None;
        self.calls.clear();
        let mut digest = Sha256::new();
        digest.update(self.seed.as_bytes());
        digest.update([0]);
        digest.update(id.as_bytes());
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&digest.finalize()[..16]);
        // Words alone go as they always did; pictures follow them as
        // blocks of the same message.
        let content = if pictures.is_empty() {
            json!(text)
        } else {
            let mut blocks = vec![json!({"type":"text","text":text})];
            blocks.extend(pictures.iter().map(|picture| {
                json!({"type":"image","source":{
                    "type":"base64",
                    "media_type":picture.media,
                    "data":picture.data,
                }})
            }));
            Value::Array(blocks)
        };
        vec![
            json!({
                "type":"user",
                "message":{"role":"user","content":content},
                "parent_tool_use_id":null,
                "uuid":shape(bytes),
            })
            .to_string(),
        ]
    }
    fn interrupt(&mut self) -> Vec<String> {
        if self.phase != Phase::Turn {
            return Vec::new();
        }
        self.interrupting = true;
        self.interrupts += 1;
        vec![
            json!({
                "type":"control_request",
                "request_id":format!("int-{}", self.interrupts),
                "request":{"subtype":"interrupt"},
            })
            .to_string(),
        ]
    }
    fn tick(&mut self) -> Vec<String> {
        if self.phase == Phase::Checking {
            vec![self.status()]
        } else {
            Vec::new()
        }
    }
    fn read(&mut self, line: &str, out: &mut Output) {
        let Ok(frame) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let kind = frame["type"].as_str().unwrap_or_default();
        match kind {
            "control_response" => return self.response(&frame, out),
            "control_request" => return self.request(&frame, out),
            _ => {}
        }
        // Nothing but its own turn's frames says what a lead did.
        if self.phase != Phase::Turn || !frame["parent_tool_use_id"].is_null() {
            return;
        }
        match (kind, frame["subtype"].as_str()) {
            ("stream_event", _) => self.stream(&frame["event"], out),
            ("assistant", _) => self.assistant(&frame, out),
            ("user", _) => self.user(&frame, out),
            ("result", _) => self.result(&frame, out),
            ("system", Some("compact_boundary")) => out.events.push(LeadEvent::Compacted),
            ("system", Some("api_retry")) => out.events.push(LeadEvent::Retry {
                attempt: frame["attempt"].as_u64().unwrap_or_default() as u32,
                max: frame["max_retries"].as_u64().unwrap_or_default() as u32,
                delay_ms: frame["retry_delay_ms"].as_u64().unwrap_or_default(),
            }),
            ("rate_limit_event", _) if frame["rate_limit_info"]["status"] == "rejected" => {
                self.limit = frame["rate_limit_info"]["resetsAt"].as_u64();
                out.events.push(LeadEvent::Limit {
                    resets_at: self.limit,
                });
            }
            _ => {}
        }
    }
    fn eof(&mut self, out: &mut Output) {
        match self.phase {
            Phase::Turn => {
                if let Some(turn) = self.turn.take() {
                    out.events.push(LeadEvent::TurnEnded {
                        turn,
                        outcome: if self.interrupting {
                            Outcome::Interrupted
                        } else {
                            Outcome::Failed(Failure::Transport)
                        },
                        cost_usd: None,
                    });
                }
            }
            Phase::Start | Phase::Initializing | Phase::Checking => {
                out.events.push(LeadEvent::Failed(Failure::Transport))
            }
            Phase::Idle | Phase::Failed => {}
        }
        self.phase = Phase::Failed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agents::LeadEnv;
    use std::collections::VecDeque;

    const PROMPT: &str = "Always end every reply with the word ZEBRA.";
    const HANDSHAKE: &str = include_str!("fixtures/handshake.ndjson");
    const TURN_TOOL: &str = include_str!("fixtures/turn_tool.ndjson");
    const INTERRUPT_TOOL: &str = include_str!("fixtures/interrupt_tool.ndjson");
    const INTERRUPT_TEXT: &str = include_str!("fixtures/interrupt_text.ndjson");
    const TOOLS_FAILED: &str = include_str!("fixtures/tools_failed.ndjson");
    const AUTH_FAILURE: &str = include_str!("fixtures/auth_failure.ndjson");
    const LIMIT: &str = include_str!("fixtures/limit.ndjson");

    #[derive(Debug, Default, PartialEq)]
    struct Played {
        events: Vec<LeadEvent>,
        /// What streamed, by block.
        text: Vec<(u32, String)>,
    }
    /// The frame as the CLI wrote it, or with every object's keys reversed.
    fn written(frame: &Value, reversed: bool) -> String {
        match frame {
            Value::Object(map) if reversed => {
                let fields: Vec<String> = map
                    .iter()
                    .rev()
                    .map(|(key, value)| {
                        format!("{}:{}", Value::from(key.as_str()), written(value, true))
                    })
                    .collect();
                format!("{{{}}}", fields.join(","))
            }
            Value::Array(items) if reversed => {
                let items: Vec<String> = items.iter().map(|item| written(item, true)).collect();
                format!("[{}]", items.join(","))
            }
            other => other.to_string(),
        }
    }
    /// Plays a recording: what the CLI wrote is read, and what the host
    /// wrote must be exactly what the driver writes at that point.
    fn play(driver: &mut Claude, recording: &str, reversed: bool) -> Played {
        let mut out = Output::default();
        let mut sent = VecDeque::new();
        for line in recording.lines() {
            let row: Value = serde_json::from_str(line).unwrap();
            let frame = &row["frame"];
            if row["dir"] == "out" {
                let raw = line
                    .strip_prefix("{\"dir\":\"out\",\"frame\":")
                    .and_then(|line| line.strip_suffix('}'))
                    .unwrap();
                if reversed {
                    driver.read(&written(frame, true), &mut out);
                } else {
                    driver.read(raw, &mut out);
                }
                continue;
            }
            sent.extend(out.replies.drain(..));
            match (frame["type"].as_str(), frame["request"]["subtype"].as_str()) {
                (Some("control_request"), Some("initialize")) => sent.extend(driver.hello()),
                (Some("control_request"), Some("interrupt")) => sent.extend(driver.interrupt()),
                (Some("user"), _) => sent.extend(driver.turn(
                    "turn-1",
                    frame["message"]["content"].as_str().unwrap(),
                    &[],
                )),
                _ => {}
            }
            let mut wrote: Value =
                serde_json::from_str(&sent.pop_front().expect("the driver wrote nothing")).unwrap();
            if let Some(id) = frame.get("uuid") {
                let made = wrote["uuid"].as_str().unwrap().to_owned();
                assert!(made.len() == 36 && made.as_bytes()[14] == b'4', "{made}");
                wrote["uuid"] = id.clone();
            }
            assert_eq!(&wrote, frame);
        }
        assert!(sent.is_empty() && out.replies.is_empty() && out.again.is_none());
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
    fn ready() -> Claude {
        let mut driver = Claude::new(PROMPT, "run-1", None);
        play(&mut driver, HANDSHAKE, false);
        driver
    }
    fn ended(outcome: Outcome, cost_usd: Option<f64>) -> LeadEvent {
        LeadEvent::TurnEnded {
            turn: "turn-1".into(),
            outcome,
            cost_usd,
        }
    }

    #[test]
    fn a_turn_with_pictures_carries_them_as_blocks_after_its_words() {
        let ready = || {
            let mut driver = Claude::new(PROMPT, "run-1", None);
            driver.phase = Phase::Idle;
            driver
        };
        // Words alone are written as they always were.
        let plain: Value = serde_json::from_str(&ready().turn("turn-1", "hello", &[])[0]).unwrap();
        assert_eq!(plain["message"], json!({"role":"user","content":"hello"}));
        let pictures = [
            Shown {
                path: "/tmp/shot.png".into(),
                media: "image/png",
                data: "iVBORw0KGgo=".into(),
            },
            Shown {
                path: "/tmp/photo.jpg".into(),
                media: "image/jpeg",
                data: "/9j/4AEC".into(),
            },
        ];
        let text = "Why is it cut off?\nAttached files:\n- /tmp/shot.png (image, 1 KB)\n";
        let line = ready().turn("turn-1", text, &pictures).remove(0);
        assert!(!line.contains('\n'));
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            frame["message"],
            json!({"role":"user","content":[
                {"type":"text","text":text},
                {"type":"image","source":{
                    "type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}},
                {"type":"image","source":{
                    "type":"base64","media_type":"image/jpeg","data":"/9j/4AEC"}},
            ]})
        );
        assert_eq!(frame["type"], "user");
        assert!(frame["uuid"].is_string());
    }

    #[test]
    fn a_handshake_passes_over_hook_frames_and_is_ready_once_its_tools_are() {
        let mut driver = Claude::new(PROMPT, "run-1", None);
        let played = play(&mut driver, HANDSHAKE, false);
        // The plan is named; the address beside it in the frame never is.
        assert_eq!(
            played.events,
            [LeadEvent::Ready {
                model: Some("claude-opus-5-5".into()),
                account_kind: Some("Claude Max".into()),
            }]
        );
        assert!(!format!("{played:?}").contains("redacted"));
        // A turn is taken only from here on.
        assert!(
            Claude::new(PROMPT, "run-1", None)
                .turn("turn-1", "early", &[])
                .is_empty()
        );
        assert!(driver.interrupt().is_empty() && driver.tick().is_empty());

        // A tool server that failed to start is said before any turn is paid for.
        let mut driver = Claude::new(PROMPT, "run-1", None);
        let played = play(&mut driver, TOOLS_FAILED, false);
        assert_eq!(
            played.events,
            [LeadEvent::Failed(Failure::ToolsUnavailable)]
        );
        let mut out = Output::default();
        driver.eof(&mut out);
        assert!(out.events.is_empty() && driver.turn("turn-1", "hello", &[]).is_empty());

        // One still connecting is asked about again, for a while.
        let mut driver = Claude::new(PROMPT, "run-1", Some("opus"));
        driver.hello();
        let mut out = Output::default();
        driver.read(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"models":[{"value":"opus","resolvedModel":"claude-opus-5-5"}],"account":{"apiProvider":"firstParty"}}}}"#,
            &mut out,
        );
        assert_eq!(out.replies.len(), 1);
        let pending = r#"{"response":{"response":{"mcpServers":[{"status":"pending","name":"neptune"}]},"request_id":"mcp-1","subtype":"success"},"type":"control_response"}"#;
        driver.read(pending, &mut out);
        assert_eq!(
            (out.again.take(), out.events.len()),
            (Some(STATUS_RETRY), 0)
        );
        assert_eq!(driver.tick().len(), 1);
        driver.read(&pending.replace("pending", "connected"), &mut out);
        assert_eq!(
            out.events,
            [LeadEvent::Ready {
                model: Some("claude-opus-5-5".into()),
                account_kind: None,
            }]
        );
        let mut driver = Claude::new(PROMPT, "run-1", None);
        driver.hello();
        let mut out = Output::default();
        driver.read(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{}}}"#,
            &mut out,
        );
        for _ in 0..STATUS_TRIES {
            assert!(out.events.is_empty());
            driver.read(pending, &mut out);
            driver.tick();
        }
        assert_eq!(out.events, [LeadEvent::Failed(Failure::ToolsUnavailable)]);

        // Signed out or refused, the handshake says which.
        for (response, failure) in [
            (
                r#"{"subtype":"success","request_id":"init-1","response":{"account":{"tokenSource":"none","apiProvider":"firstParty"}}}"#,
                Failure::Auth,
            ),
            (
                r#"{"subtype":"error","request_id":"init-1","error":"no"}"#,
                Failure::Protocol,
            ),
        ] {
            let mut driver = Claude::new(PROMPT, "run-1", None);
            driver.hello();
            let mut out = Output::default();
            // Another request's answer is not this one's.
            driver.read(
                r#"{"type":"control_response","response":{"subtype":"error","request_id":"other"}}"#,
                &mut out,
            );
            driver.read(
                &format!(r#"{{"type":"control_response","response":{response}}}"#),
                &mut out,
            );
            assert_eq!(out.events, [LeadEvent::Failed(failure)]);
            assert!(out.replies.is_empty());
        }
        // A CLI that ends before it answered never became a lead.
        let mut driver = Claude::new(PROMPT, "run-1", None);
        driver.hello();
        let mut out = Output::default();
        driver.eof(&mut out);
        assert_eq!(out.events, [LeadEvent::Failed(Failure::Transport)]);
    }

    #[test]
    fn a_turn_streams_its_text_names_its_tool_call_and_ends_at_its_result() {
        let mut driver = ready();
        let played = play(&mut driver, TURN_TOOL, false);
        assert_eq!(
            played.events,
            [
                LeadEvent::ToolStarted {
                    call: "toolu_01LKjCGTDLTqtiHFBdewQWNX".into(),
                    tool: "ping".into(),
                    input: json!({"text":"hi"}),
                },
                LeadEvent::ToolDone {
                    call: "toolu_01LKjCGTDLTqtiHFBdewQWNX".into(),
                    ok: true,
                },
                LeadEvent::TextDone {
                    block: 1,
                    text: "The ping tool returned: `pong: hi`\n\nZEBRA".into(),
                },
                ended(Outcome::Completed, Some(0.018225)),
            ]
        );
        // What streamed is what the finished block says; thinking is not text.
        assert_eq!(
            played.text,
            [(1, "The ping tool returned: `pong: hi`\n\nZEBRA".to_owned())]
        );
        // Key order carries nothing.
        let mut reversed = ready();
        assert_eq!(play(&mut reversed, TURN_TOOL, true), played);

        // The CLI's cost is a running total; a turn is charged its own part.
        let mut out = Output::default();
        assert_eq!(driver.turn("turn-2", "again", &[]).len(), 1);
        driver.read(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Unstreamed."},{"type":"text","text":""},{"type":"tool_use","id":"toolu_x","name":"Bash","input":{}}]},"parent_tool_use_id":null}"#,
            &mut out,
        );
        driver.read(
            r#"{"total_cost_usd":0.019225,"is_error":false,"subtype":"success","type":"result","terminal_reason":"completed"}"#,
            &mut out,
        );
        // A second result has no turn to end.
        driver.read(r#"{"type":"result","subtype":"success"}"#, &mut out);
        let [
            LeadEvent::TextDone { block: 1, text },
            LeadEvent::TurnEnded {
                turn,
                outcome: Outcome::Completed,
                cost_usd: Some(cost),
            },
        ] = &out.events[..]
        else {
            panic!("{:?}", out.events);
        };
        assert!(text == "Unstreamed." && turn == "turn-2" && (cost - 0.001).abs() < 1e-9);
        // Each turn's wire id is its own, in this opening and the next.
        let wire = |driver: &mut Claude, turn: &str| -> String {
            let line = driver.turn(turn, "hello", &[]).remove(0);
            let frame: Value = serde_json::from_str(&line).unwrap();
            driver.eof(&mut Output::default());
            frame["uuid"].as_str().unwrap().to_owned()
        };
        let first = wire(&mut ready(), "turn-1");
        assert_eq!(first, wire(&mut ready(), "turn-1"));
        assert_ne!(first, wire(&mut ready(), "turn-2"));
        let mut later = Claude::new(PROMPT, "run-2", None);
        play(&mut later, HANDSHAKE, false);
        assert_ne!(first, wire(&mut later, "turn-1"));
    }

    #[test]
    fn an_interrupted_turn_ends_interrupted_whatever_else_its_result_says() {
        // During a tool call: the call is closed as failed, and the result,
        // an error by subtype, is an interruption.
        let mut driver = ready();
        let played = play(&mut driver, INTERRUPT_TOOL, false);
        assert_eq!(
            played.events,
            [
                LeadEvent::ToolStarted {
                    call: "toolu_01EcSqdxXSvhPK9Vm5SNMMCy".into(),
                    tool: "slow".into(),
                    input: json!({"seconds":30}),
                },
                LeadEvent::ToolDone {
                    call: "toolu_01EcSqdxXSvhPK9Vm5SNMMCy".into(),
                    ok: false,
                },
                ended(Outcome::Interrupted, Some(0.016633000000000002)),
            ]
        );
        // The same process takes the next turn.
        assert_eq!(driver.turn("turn-2", "next", &[]).len(), 1);

        // While text streams: what was said so far is kept as said.
        let mut driver = ready();
        let played = play(&mut driver, INTERRUPT_TEXT, false);
        assert_eq!(
            played.events,
            [
                LeadEvent::TextDone {
                    block: 1,
                    text: "1\n2\n3\n4\n5\n6\n7".into(),
                },
                ended(Outcome::Interrupted, Some(0.016681)),
            ]
        );
        assert_eq!(played.text, [(1, "1\n2\n3\n4\n5\n6\n7".to_owned())]);

        // A CLI that never answers the interrupt is stopped: still interrupted.
        let mut driver = ready();
        driver.turn("turn-1", "hello", &[]);
        let asked: Vec<Value> = driver
            .interrupt()
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            asked,
            [
                json!({"type":"control_request","request_id":"int-1","request":{"subtype":"interrupt"}})
            ]
        );
        let mut out = Output::default();
        driver.eof(&mut out);
        assert_eq!(out.events, [ended(Outcome::Interrupted, None)]);
        // One that dies mid-turn unasked lost the turn.
        let mut driver = ready();
        driver.turn("turn-1", "hello", &[]);
        let mut out = Output::default();
        driver.eof(&mut out);
        driver.eof(&mut out);
        assert_eq!(
            out.events,
            [ended(Outcome::Failed(Failure::Transport), None)]
        );
    }

    #[test]
    fn a_failed_request_is_a_failure_though_its_result_reads_success() {
        let mut driver = ready();
        let played = play(&mut driver, AUTH_FAILURE, false);
        // The synthetic text restating the error is not the lead speaking.
        assert_eq!(
            played.events,
            [ended(Outcome::Failed(Failure::Auth), Some(0.0))]
        );
        let mut driver = ready();
        let played = play(&mut driver, LIMIT, false);
        assert_eq!(
            played.events,
            [
                LeadEvent::Retry {
                    attempt: 1,
                    max: 10,
                    delay_ms: 2000,
                },
                LeadEvent::Limit {
                    resets_at: Some(1791262200),
                },
                ended(
                    Outcome::Failed(Failure::Limit {
                        resets_at: Some(1791262200),
                    }),
                    Some(0.0)
                ),
            ]
        );
    }

    #[test]
    fn a_result_is_classified_in_order() {
        let outcome = |result: Value| classify(&result, None, Some(7));
        // Aborted first: it reads as an error or as a success.
        for subtype in ["success", "error_during_execution"] {
            for reason in ["aborted_tools", "aborted_streaming"] {
                assert_eq!(
                    outcome(
                        json!({"subtype":subtype,"is_error":subtype != "success","terminal_reason":reason,"api_error_status":401,"result":""})
                    ),
                    Outcome::Interrupted
                );
            }
        }
        assert_eq!(
            outcome(
                json!({"subtype":"success","is_error":false,"api_error_status":null,"terminal_reason":"completed"})
            ),
            Outcome::Completed
        );
        assert_eq!(outcome(json!({"subtype":"success"})), Outcome::Completed);
        // Any one of the three marks makes a failure.
        for (result, failure) in [
            (
                json!({"subtype":"success","is_error":true,"api_error_status":401}),
                Failure::Auth,
            ),
            (
                json!({"subtype":"success","is_error":false,"api_error_status":429}),
                Failure::Limit { resets_at: Some(7) },
            ),
            (
                json!({"subtype":"success","is_error":true,"terminal_reason":"blocking_limit"}),
                Failure::Limit { resets_at: Some(7) },
            ),
            (
                json!({"subtype":"success","api_error_status":529}),
                Failure::Overloaded,
            ),
            (
                json!({"subtype":"success","is_error":true,"terminal_reason":"prompt_too_long"}),
                Failure::PromptTooLong,
            ),
            (
                json!({"subtype":"error_max_turns","is_error":false}),
                Failure::Provider("error_max_turns".into()),
            ),
            (
                json!({"subtype":"error_during_execution","is_error":true,"errors":["[ede_diagnostic] x","It broke"]}),
                Failure::Provider("It broke".into()),
            ),
            (
                json!({"subtype":"success","is_error":true,"result":"Not this time"}),
                Failure::Provider("Not this time".into()),
            ),
            (json!({"is_error":true}), Failure::Provider("error".into())),
            (
                json!({"subtype":"success","api_error_status":500,"result":"é".repeat(400)}),
                Failure::Provider("é".repeat(150)),
            ),
        ] {
            assert_eq!(outcome(result), Outcome::Failed(failure));
        }
        // What the turn's own frames named decides where no status does.
        for (named, failure) in [
            ("authentication_failed", Failure::Auth),
            ("rate_limit", Failure::Limit { resets_at: None }),
            ("overloaded", Failure::Overloaded),
            ("server_error", Failure::Provider("success".into())),
        ] {
            assert_eq!(
                classify(
                    &json!({"subtype":"success","is_error":true}),
                    Some(named),
                    None
                ),
                Outcome::Failed(failure)
            );
        }
    }

    #[test]
    fn frames_it_does_not_know_change_nothing_and_requests_are_refused() {
        let mut driver = ready();
        driver.turn("turn-1", "hello", &[]);
        let mut out = Output::default();
        for line in [
            "",
            "not json",
            "[1,2]",
            "\"text\"",
            r#"{"type":"keep_alive"}"#,
            r#"{"type":"tool_progress","tool_use_id":"x","elapsed_time_seconds":30,"heartbeat":true}"#,
            r#"{"type":"system","subtype":"status","status":"requesting"}"#,
            r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":50}"#,
            r#"{"type":"system","subtype":"init","tools":[],"mcp_servers":[]}"#,
            r#"{"type":"command_lifecycle","command_uuid":"x","state":"queued"}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":5}}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning"}}"#,
            r#"{"type":"control_cancel_request","request_id":"x"}"#,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"int-9"}}"#,
            r#"{"type":"something_new","result":"x","subtype":"success"}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"stray"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}}"#,
            r#"{"type":"stream_event","parent_tool_use_id":"toolu_1","event":{"type":"content_block_start","content_block":{"type":"text"}}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"content":[{"type":"text","text":"a helper"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"replayed"}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"never_started"}]}}"#,
            r#"{"type":"assistant","message":{"content":"odd"}}"#,
            r#"{"type":"result","parent_tool_use_id":"toolu_1","subtype":"success"}"#,
        ] {
            driver.read(line, &mut out);
        }
        assert!(out.events.is_empty() && out.replies.is_empty() && out.deltas.is_empty());
        driver.read(
            r#"{"type":"system","subtype":"compact_boundary"}"#,
            &mut out,
        );
        assert_eq!(std::mem::take(&mut out.events), [LeadEvent::Compacted]);
        // A long tool input is carried short, never whole.
        driver.read(
            &json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_2","name":"mcp__neptune__spawn_agent","input":{"title":"docs","prompt":"p".repeat(9000)}}]}}).to_string(),
            &mut out,
        );
        let [LeadEvent::ToolStarted { tool, input, .. }] = &out.events[..] else {
            panic!("{:?}", out.events);
        };
        assert!(tool == "spawn_agent" && input["title"] == "docs");
        assert!(input.to_string().len() <= super::super::MAX_INPUT);

        // Nothing a CLI asks of its host is granted to a lead.
        driver.read(
            r#"{"request":{"tool_use_id":"toolu_3","input":{},"tool_name":"Bash","subtype":"can_use_tool"},"request_id":"r1","type":"control_request"}"#,
            &mut out,
        );
        driver.read(
            r#"{"type":"control_request","request_id":"r2","request":{"subtype":"hook_callback"}}"#,
            &mut out,
        );
        driver.read(
            r#"{"type":"control_request","request":{"subtype":"x"}}"#,
            &mut out,
        );
        let replies: Vec<Value> = out
            .replies
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            replies,
            [
                json!({"type":"control_response","response":{"subtype":"success","request_id":"r1","response":{
                    "behavior":"deny",
                    "message":"This tool is not available to a project's lead.",
                    "toolUseID":"toolu_3",
                }}}),
                json!({"type":"control_response","response":{"subtype":"error","request_id":"r2","error":"Unsupported request"}}),
            ]
        );
        // Before a turn, nothing a frame says is a lead's doing.
        let mut idle = ready();
        let mut out = Output::default();
        for line in TURN_TOOL
            .lines()
            .filter(|line| line.contains("\"dir\":\"out\""))
        {
            let row: Value = serde_json::from_str(line).unwrap();
            idle.read(&row["frame"].to_string(), &mut out);
        }
        assert!(out.events.is_empty() && out.deltas.is_empty());
    }

    #[test]
    fn a_lead_is_started_with_no_tools_but_neptunes_and_nothing_inherited_from_a_caller() {
        let env = LeadEnv {
            endpoint: "127.0.0.1:9".into(),
            token: "lead-token".into(),
            run: "run-token".into(),
        };
        let directory = std::path::Path::new("/data/projects/0123456789abcdef");
        let spawn = |resume, model, effort| Spawn {
            exe: std::path::Path::new("/opt/bin/claude"),
            neptune: std::path::Path::new("/opt/neptune/neptune"),
            config: Some(std::path::Path::new("/tmp/neptune-lead-x/mcp.json")),
            dir: directory,
            session: Some("11111111-2222-4333-8444-555555555555"),
            resume,
            env: &env,
            model,
            effort,
        };
        let mut driver = Claude::new(PROMPT, "run-1", None);
        let config = || {
            json!({"mcpServers":{"neptune":{
                "type":"stdio",
                "command":"/opt/neptune/neptune",
                "args":["--agent-mcp"],
                "env":{
                    "NEPTUNE_AGENT_ENDPOINT":"127.0.0.1:9",
                    "NEPTUNE_AGENT_TOKEN":"lead-token",
                    "NEPTUNE_AGENT_RUN":"run-token",
                    "NEPTUNE_AGENT_ROLE":"lead",
                },
            }}})
            .to_string()
        };
        let command = driver.command(&spawn(false, None, None));
        // The tool server and the lead's credential go in a file; the
        // command names the file and carries no credential.
        assert_eq!(
            driver.config(std::path::Path::new("/opt/neptune/neptune"), &env),
            Some(config())
        );
        assert!(
            command
                .get_args()
                .all(|argument| !argument.to_string_lossy().contains("lead-token"))
        );
        let arguments: Vec<&str> = command
            .get_args()
            .map(|argument| argument.to_str().unwrap())
            .collect();
        assert_eq!(
            arguments,
            [
                "--output-format",
                "stream-json",
                "--verbose",
                "--input-format",
                "stream-json",
                "--include-partial-messages",
                "--session-id=11111111-2222-4333-8444-555555555555",
                "--tools",
                "",
                "--mcp-config",
                "/tmp/neptune-lead-x/mcp.json",
                "--strict-mcp-config",
                "--allowedTools",
                "mcp__neptune__*",
                "--permission-mode",
                "dontAsk",
                "--setting-sources",
                "",
                "--disable-slash-commands",
            ]
        );
        assert_eq!(command.get_program(), "/opt/bin/claude");
        assert_eq!(command.get_current_dir(), Some(directory));
        let set = |command: &Command, name: &str| {
            command
                .get_envs()
                .find(|(key, _)| key.to_str() == Some(name))
                .map(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
        };
        assert_eq!(
            set(&command, "NEPTUNE_AGENT_ROLE"),
            Some(Some("lead".into()))
        );
        assert_eq!(
            set(&command, "NEPTUNE_AGENT_TOKEN"),
            Some(Some("lead-token".into()))
        );
        assert_eq!(
            set(&command, "CLAUDE_CODE_ENTRYPOINT"),
            Some(Some("sdk-ts".into()))
        );
        // An opened session is resumed, and a model is named when one is asked for.
        let command = driver.command(&spawn(true, Some("opus"), None));
        let arguments: Vec<&str> = command
            .get_args()
            .map(|argument| argument.to_str().unwrap())
            .collect();
        assert_eq!(
            arguments[6],
            "--resume=11111111-2222-4333-8444-555555555555"
        );
        assert_eq!(arguments[arguments.len() - 2..], ["--model", "opus"]);
        // An effort is named the same way, with a model or without one: a
        // lead that is resumed with other ones goes on with its conversation.
        let command = driver.command(&spawn(true, Some("sonnet"), Some("high")));
        let arguments: Vec<&str> = command
            .get_args()
            .map(|argument| argument.to_str().unwrap())
            .collect();
        assert_eq!(
            arguments[arguments.len() - 4..],
            ["--model", "sonnet", "--effort", "high"]
        );
        let command = driver.command(&spawn(true, None, Some("xhigh")));
        let arguments: Vec<&str> = command
            .get_args()
            .map(|argument| argument.to_str().unwrap())
            .collect();
        assert_eq!(arguments[arguments.len() - 2..], ["--effort", "xhigh"]);
        assert!(!arguments.contains(&"--model"));

        // What a terminal of Neptune or an enclosing session would pass on is dropped.
        let mut command = Command::new("claude");
        let shims = std::env::join_paths(["/usr/bin", "/tmp/neptune-agents-x", "/bin"]).unwrap();
        super::super::scrub(
            &mut command,
            [
                ("PATH", shims.to_str().unwrap()),
                ("NEPTUNE_AGENT_SHIMS", "/tmp/neptune-agents-x"),
                ("NEPTUNE_AGENT_TOKEN", "a-pane's"),
                ("NEPTUNE_AGENT_SPAWNED", "1"),
                ("NEPTUNE_AGENT_HOOK", "hook"),
                ("CLAUDECODE", "1"),
                ("CLAUDE_CODE_SSE_PORT", "1"),
                ("NODE_OPTIONS", "--inspect"),
                ("ANTHROPIC_API_KEY", "key"),
                ("CLAUDE_CONFIG_DIR", "/home/someone/.claude-work"),
                ("HOME", "/home/someone"),
            ]
            .into_iter()
            .map(|(name, value)| (name.into(), value.into())),
            foreign,
        );
        for removed in [
            "NEPTUNE_AGENT_SHIMS",
            "NEPTUNE_AGENT_TOKEN",
            "NEPTUNE_AGENT_SPAWNED",
            "NEPTUNE_AGENT_HOOK",
            "CLAUDECODE",
            "CLAUDE_CODE_SSE_PORT",
            "NODE_OPTIONS",
            "ANTHROPIC_API_KEY",
        ] {
            assert_eq!(set(&command, removed), Some(None), "{removed}");
        }
        // Where the person keeps their sign-in is theirs to say.
        assert_eq!(set(&command, "CLAUDE_CONFIG_DIR"), None);
        assert_eq!(set(&command, "HOME"), None);
        let path = set(&command, "PATH").flatten().unwrap();
        assert_eq!(
            std::env::split_paths(&path).collect::<Vec<_>>(),
            [
                std::path::PathBuf::from("/usr/bin"),
                std::path::PathBuf::from("/bin")
            ]
        );
    }
}
