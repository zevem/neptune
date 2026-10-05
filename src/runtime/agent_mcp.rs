//! The tools Neptune offers a CLI agent, served over the agent's own stdio
//! pipes. A pull request address leaves this process, and so do the tasks,
//! messages and replies an agent exchanges with the agents it started.
use super::agents::{AgentReport, Answer, Event, MAX_TEXT, Status, label};
use crate::agent_activity::Attention;
use neptune_model::{AgentKind, PullRequest};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{BufRead, Read, Write},
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

/// A task of the longest size, escaped, with room for its envelope.
const MAX_LINE: u64 = 512 * 1024;
/// The longest one call waits for a started agent; the agent calls again.
pub(super) const MAX_WAIT: Duration = Duration::from_secs(300);
const DEFAULT_WAIT: Duration = Duration::from_secs(120);
/// How long the application has to open a started agent's terminal.
const SPAWN_WAIT: Duration = Duration::from_secs(15);
/// How long starting an agent waits to see its CLI take the task or ask
/// something first, so that a question is reported where it was caused.
const START_WAIT: Duration = Duration::from_secs(20);
/// The lead Neptune puts before a task takes part of its room.
const MAX_PROMPT: usize = MAX_TEXT - 1024;

const LINK: &str = "link_pull_request";
const SPAWN: &str = "spawn_agent";
const WAIT: &str = "wait_for_agent";
const TELL: &str = "send_agent_message";
const LIST: &str = "list_agents";
const CLOSE: &str = "close_agent";
const REPLY: &str = "reply_to_parent";
const REOPEN: &str = "reopen_agent";
const PRESS: &str = "press_agent_keys";

const INSTRUCTIONS: &str = "This terminal runs in Neptune, which shows the pull requests you link next to the terminal's tab. \
Call link_pull_request with the pull request's URL right after you create a pull request, and when you start work on an existing one. \
Link every pull request of a stack. Linking the same pull request again is safe.\n\n\
Neptune can also start another coding agent, Claude Code or Codex, in a terminal of its own, so that you can hand it part of the work. \
Use it when the user asks for the other agent or for work to be split between agents. \
spawn_agent starts one with a task, wait_for_agent returns what it answered, send_agent_message continues the conversation with it, \
list_agents shows the agents you started, close_agent closes one when its work is done and reopen_agent brings back one that was closed. \
A started agent does not see this conversation: put everything it needs in its task. \
It works in your directory unless you give it another; when two agents would edit the same files, create a git worktree for it and pass that as cwd. \
Its terminal runs out of view. The user sees each started agent in the list on this terminal's tab and opens its terminal from there. \
When a tool says an agent asks something or waits for a person, tell the user at once, quoting what it asks, before you do anything else: nobody is watching its terminal. \
Before you finish, wait for the agents you started and check their work.";
const SPAWNED_INSTRUCTIONS: &str = "\n\nAnother agent started you from its own Neptune terminal and handed you your task. \
It reads the last message of each of your turns, and nothing else you write: end each turn with what you did and what it needs to know. \
Call reply_to_parent to tell it something before your turn ends.";

/// The application, as the tool server reaches it.
pub(super) trait Host {
    /// Hands over an event and reports whether this terminal took it.
    fn send(&mut self, event: Event) -> bool;
    fn ask(&mut self, event: Event) -> Answer;
}
/// Where a long call rests between looks at a started agent.
pub(super) trait Pause {
    /// Rests up to `duration`. True when the call was cancelled or the agent
    /// closed its end, so nobody is waiting for the answer.
    fn pause(&mut self, call: &Value, duration: Duration) -> bool;
}

fn tool_names() -> impl Iterator<Item = &'static str> {
    [LINK, SPAWN, WAIT, TELL, LIST, CLOSE, REOPEN, PRESS, REPLY].into_iter()
}
/// The tools a launch allows without asking: those that read, link or
/// answer. Starting an agent, typing for one and closing one hand work to
/// another CLI or stop it, so each agent's own permission rules decide them.
pub(super) fn allowed_tools() -> impl Iterator<Item = &'static str> {
    [LINK, WAIT, LIST, REPLY].into_iter()
}
/// What a tool does, for a CLI that decides from it whether to ask first.
fn hints(read_only: bool, destructive: bool, open_world: bool) -> Value {
    json!({
        "readOnlyHint": read_only,
        "destructiveHint": destructive,
        "idempotentHint": read_only,
        "openWorldHint": open_world,
    })
}

fn tools(spawned: bool) -> Vec<Value> {
    let agent_id = json!({
        "type": "integer",
        "description": "The number spawn_agent gave the agent",
    });
    let mut tools = vec![
        json!({
            "name": LINK,
            "annotations": hints(false, false, false),
            "description": "Link a pull request you created or are working on to this Neptune terminal tab, where its number opens it in the browser.",
            "inputSchema": {
                "type": "object",
                "properties": {"url": {
                    "type": "string",
                    "description": "The pull request's address, such as https://github.com/owner/repo/pull/123",
                }},
                "required": ["url"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": SPAWN,
            "annotations": hints(false, false, true),
            "description": "Start another coding agent (Claude Code or Codex) in a Neptune terminal of its own, out of view, and hand it a task. It runs interactively with its own conversation, tools and permissions, and does not see yours. Returns its agent number once it has taken the task, or what its CLI asks first; call wait_for_agent for its reply.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex"],
                        "description": "Which agent to start: claude is Claude Code, codex is Codex",
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The whole task, with everything the agent needs to know: goal, files, constraints, and what to report back. For long context, write a file and name it here.",
                    },
                    "cwd": {
                        "type": "string",
                        "description": "The directory it works in; yours when left out. Give it a git worktree of its own when both of you edit files.",
                    },
                    "effort": {
                        "type": "string",
                        "enum": ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"],
                        "description": "How hard its model thinks. claude takes low, medium, high, xhigh or max; codex takes minimal, low, medium, high, xhigh, max or ultra. Leave out for the CLI's own default; give one when the user asks for an effort level.",
                    },
                    "ultra": {
                        "type": "boolean",
                        "description": "Start it in its ultra mode: for claude, ultracode on (standing multi-agent workflow orchestration, at whatever effort is given); for codex, the Ultra effort. It uses far more tokens, so set it only when the user asks for ultracode or Ultra.",
                    },
                    "model": {
                        "type": "string",
                        "description": "The model it runs on, named as its CLI names it: for claude an alias such as opus, sonnet or haiku, or a full model id; for codex a model id such as gpt-5.1-codex. Leave out for the CLI's own default; give one when the user asks for a model.",
                    },
                },
                "required": ["agent", "prompt"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": WAIT,
            "annotations": hints(true, false, false),
            "description": "Wait until an agent you started has news for you: it finished a turn, sent a message, asks something, needs a person, or ended. Returns what it replied. Without agent_id, waits for whichever of your agents has news first. Returns after timeout_seconds if there is none; call again to keep waiting.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": agent_id,
                    "timeout_seconds": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_WAIT.as_secs(),
                        "description": format!("How long to wait, {} when left out", DEFAULT_WAIT.as_secs()),
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": TELL,
            "annotations": hints(false, false, true),
            "description": "Send a follow-up message to an agent you started, as its next prompt: an answer, a correction or more work. Then call wait_for_agent for its reply.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": agent_id,
                    "message": {"type": "string", "description": "What to tell it"},
                },
                "required": ["agent_id", "message"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": LIST,
            "annotations": hints(true, false, false),
            "description": "List the agents you started and what each is doing, without reading their replies.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        }),
        json!({
            "name": CLOSE,
            "annotations": hints(false, true, false),
            "description": "Close an agent you started, and its terminal, once its work is done and you have read its reply. Its unfinished work stops. reopen_agent brings it back with its conversation if you need it again.",
            "inputSchema": {
                "type": "object",
                "properties": {"agent_id": agent_id},
                "required": ["agent_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": REOPEN,
            "annotations": hints(false, false, true),
            "description": "Open again an agent you started whose terminal was closed, by you or by the user, or whose CLI exited. It goes on with the conversation it had, in the directory and with the model, effort and ultra mode it had, and takes your message as its next prompt. Returns its new agent number; call wait_for_agent for its reply.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": agent_id,
                    "message": {"type": "string", "description": "What it should do now"},
                },
                "required": ["agent_id", "message"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": PRESS,
            "annotations": hints(false, true, true),
            "description": "Press keys in the terminal of an agent you started, to answer what its CLI asks before it takes its task, such as whether to trust a folder or to review hooks. Only for those questions, and only with the answer the user gave you after you told them what it asks: such a question is the user's to decide. Then call wait_for_agent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": agent_id,
                    "keys": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1,
                        "maxItems": 8,
                        "description": "Keys in order, each one of: enter, escape, tab, space, up, down, left, right, or a single letter or digit",
                    },
                },
                "required": ["agent_id", "keys"],
                "additionalProperties": false,
            },
        }),
    ];
    if spawned {
        tools.push(json!({
            "name": REPLY,
            "annotations": hints(false, false, false),
            "description": "Tell the agent that started you something before your turn ends: a question, a finding or progress. The last message of your turn reaches it without this.",
            "inputSchema": {
                "type": "object",
                "properties": {"message": {"type": "string", "description": "What to tell it"}},
                "required": ["message"],
                "additionalProperties": false,
            },
        }));
    }
    tools
}

fn name(report: &AgentReport) -> String {
    format!("Agent {} ({})", report.agent, label(report.kind))
}
fn state(status: &Status) -> String {
    match status {
        Status::Starting => "starting".into(),
        Status::Asking { .. } => {
            "asking something before it takes its task; a person decides".into()
        }
        Status::Working => "working".into(),
        Status::Idle => "idle".into(),
        Status::Waiting { attention } => format!("waiting for a person to {}", asked(*attention)),
        Status::Ended { reason } => format!("ended: {reason}"),
    }
}
/// What a CLI asks before it takes its task, and who answers it.
fn question(name: &str, screen: &str) -> String {
    let shown = if screen.trim().is_empty() {
        "Its terminal is in view of the user.".to_owned()
    } else {
        format!("Its terminal shows:\n\n{screen}\n")
    };
    format!(
        "{name} has not taken its task: its CLI is asking something first, which only the user decides. {shown}\n\
Tell the user now what it asks. They can answer in its terminal, which opens from the agents list on this terminal's tab, or tell you the answer, which press_agent_keys then types. Do not answer for them. Call wait_for_agent afterwards."
    )
}
fn asked(attention: Attention) -> &'static str {
    match attention {
        Attention::Permission => "answer a permission request",
        Attention::Question => "answer a question",
        Attention::Plan => "approve a plan",
        Attention::Input => "give it input",
    }
}
/// What a started agent has to say, for the agent that started it.
fn describe(report: &AgentReport) -> String {
    let replies = report.replies.join("\n\n---\n\n");
    let said = |lead: &str| {
        if replies.is_empty() {
            String::new()
        } else {
            format!(" {lead}\n\n{replies}")
        }
    };
    let name = name(report);
    match &report.status {
        Status::Idle if !replies.is_empty() => {
            format!("{name} finished its turn and replied:\n\n{replies}")
        }
        Status::Idle if report.news => format!(
            "{name} finished its turn without a reply; it may have been interrupted in its terminal."
        ),
        Status::Idle => format!("{name} is idle and has said nothing new."),
        Status::Working => format!("{name} is still working.{}", said("So far it sent:")),
        Status::Starting => format!(
            "{name} has not taken its task yet; its CLI is still starting. If it stays that way, ask the user to open its terminal from the agents list on this terminal's tab."
        ),
        Status::Asking { screen } => question(&name, screen),
        Status::Waiting { attention } => format!(
            "{name} is waiting for a person to {} in its terminal. Tell the user now: they open it from the agents list on this terminal's tab, and it goes on once they answer.{}",
            asked(*attention),
            said("Before that it sent:")
        ),
        Status::Ended { reason } => format!(
            "{name} is no longer running: {reason}.{}{}",
            if report.reopens {
                " reopen_agent opens it again with its conversation."
            } else {
                ""
            },
            said("Before that it replied:")
        ),
    }
}
fn agent_id(arguments: &Value) -> Result<Option<u64>, String> {
    match arguments.get("agent_id") {
        None | Some(Value::Null) => Ok(None),
        Some(id) => id
            .as_u64()
            .filter(|id| *id > 0)
            .map(Some)
            .ok_or_else(|| "agent_id is the number spawn_agent returned.".to_owned()),
    }
}
fn text<'a>(arguments: &'a Value, field: &str, limit: usize) -> Result<&'a str, String> {
    let value = arguments
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default();
    if value.trim().is_empty() {
        Err(format!("{field} is empty."))
    } else if value.len() > limit {
        Err(format!(
            "{field} is longer than {} KB. Write the details to a file and name the file instead.",
            limit / 1024
        ))
    } else {
        Ok(value)
    }
}

/// Runs one tool. `None` when the call was cancelled while it waited.
fn call(
    tool: &str,
    arguments: &Value,
    id: &Value,
    host: &mut dyn Host,
    rest: &mut dyn Pause,
) -> Option<Result<String, String>> {
    let refusal = |answer: Answer| match answer {
        Answer::Refused { reason } => reason,
        _ => "Neptune gave an answer this tool does not know.".to_owned(),
    };
    Some(match tool {
        LINK => {
            let url = arguments
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match PullRequest::parse(url) {
                None => Err(
                    "Not linked: expected a pull request address such as https://github.com/owner/repo/pull/123".to_owned(),
                ),
                Some(pull_request)
                    if host.send(Event::PullRequest {
                        url: pull_request.url().into(),
                    }) =>
                {
                    Ok(format!("Linked {} to this terminal tab.", pull_request.label()))
                }
                Some(_) => {
                    Err("Not linked: Neptune is not tracking an agent in this terminal.".to_owned())
                }
            }
        }
        SPAWN => (|| {
            let kind = match arguments.get("agent").and_then(Value::as_str) {
                Some("claude") => AgentKind::Claude,
                Some("codex") => AgentKind::Codex,
                _ => return Some(Err("agent is \"claude\" or \"codex\".".to_owned())),
            };
            let prompt = match text(arguments, "prompt", MAX_PROMPT) {
                Ok(prompt) => prompt.to_owned(),
                Err(error) => return Some(Err(error)),
            };
            let cwd = arguments
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.trim().is_empty());
            let cwd = std::env::current_dir()
                .map(|here| here.join(cwd.unwrap_or_default()))
                .and_then(|path| path.canonicalize())
                .ok()
                .filter(|path| path.is_dir());
            let Some(cwd) = cwd else {
                return Some(Err(
                    "cwd is not a directory that exists. Create it first, for example with git worktree add.".to_owned(),
                ));
            };
            let model = arguments
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_owned);
            let effort = arguments
                .get("effort")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|effort| !effort.is_empty())
                .map(str::to_owned);
            let ultra = arguments.get("ultra").and_then(Value::as_bool) == Some(true);
            let request = match host.ask(Event::Spawn {
                kind,
                prompt,
                cwd: cwd.clone(),
                model: model.clone(),
                effort: effort.clone(),
                ultra,
            }) {
                Answer::Queued { request } => request,
                other => return Some(Err(refusal(other))),
            };
            let agent = match opened(request, id, host, rest)? {
                Ok(agent) => agent,
                Err(error) => return Some(Err(error)),
            };
            let mode = match (kind, ultra) {
                (AgentKind::Claude, true) => " with ultracode on",
                (AgentKind::Codex, true) => " at ultra effort",
                _ => "",
            };
            let started = format!(
                "Started {} as agent {agent}{}{}{mode}, working in {}",
                label(kind),
                model.map_or(String::new(), |model| format!(" on {model}")),
                effort
                    .filter(|effort| !(ultra && kind == AgentKind::Codex && effort == "ultra"))
                    .map_or(String::new(), |effort| format!(" at {effort} effort")),
                cwd.display()
            );
            taken(agent, &started, id, host, rest)
        })()?,
        REOPEN => (|| {
            let agent = match agent_id(arguments) {
                Ok(Some(agent)) => agent,
                Ok(None) => return Some(Err("agent_id is required.".to_owned())),
                Err(error) => return Some(Err(error)),
            };
            let message = match text(arguments, "message", MAX_PROMPT) {
                Ok(message) => message.to_owned(),
                Err(error) => return Some(Err(error)),
            };
            let request = match host.ask(Event::Reopen {
                agent,
                text: message,
            }) {
                Answer::Queued { request } => request,
                other => return Some(Err(refusal(other))),
            };
            let reopened = match opened(request, id, host, rest)? {
                Ok(agent) => agent,
                Err(error) => return Some(Err(error)),
            };
            let started = format!(
                "Reopened agent {agent} with its conversation. It is agent {reopened} from now on"
            );
            taken(reopened, &started, id, host, rest)
        })()?,
        PRESS => (|| {
            let agent = agent_id(arguments)?.ok_or("agent_id is required.")?;
            let keys: Vec<String> = arguments
                .get("keys")
                .and_then(Value::as_array)
                .map(|keys| {
                    keys.iter()
                        .map(|key| key.as_str().unwrap_or_default().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            match host.ask(Event::Press { agent, keys }) {
                Answer::Done => Ok(format!(
                    "Pressed in agent {agent}'s terminal. Call wait_for_agent to see whether it took its task or asks more."
                )),
                other => Err(refusal(other)),
            }
        })(),
        WAIT => (|| {
            let agent = match agent_id(arguments) {
                Ok(agent) => agent,
                Err(error) => return Some(Err(error)),
            };
            let wait = arguments
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_WAIT, |seconds| {
                    Duration::from_secs(seconds).min(MAX_WAIT)
                });
            let started = Instant::now();
            loop {
                let agents = match host.ask(Event::Collect { agent, take: false }) {
                    Answer::Agents { agents } => agents,
                    other => return Some(Err(refusal(other))),
                };
                if agents.is_empty() {
                    return Some(Ok(
                        "You have not started any agents that are still open.".to_owned()
                    ));
                }
                // Nothing more comes from agents that have all ended.
                let over = agents
                    .iter()
                    .all(|report| matches!(report.status, Status::Ended { .. }));
                // Something the caller has not heard: a turn that ended, a
                // message, a request to a person or an exit. An agent at rest
                // may still have work in the background that ends later.
                let ready = agents.iter().any(|report| report.news);
                let waited = started.elapsed();
                if ready || over || waited >= wait {
                    let agents = match host.ask(Event::Collect { agent, take: true }) {
                        Answer::Agents { agents } => agents,
                        other => return Some(Err(refusal(other))),
                    };
                    let mut said: Vec<String> = agents.iter().map(describe).collect();
                    if !ready && !over {
                        said.push(format!(
                            "Nothing new after {} seconds. Call wait_for_agent again to keep waiting.",
                            waited.as_secs()
                        ));
                    }
                    return Some(Ok(said.join("\n\n")));
                }
                // Quick while a short task may be ending, then once a second.
                let rest_for = if waited < Duration::from_secs(5) {
                    Duration::from_millis(250)
                } else {
                    Duration::from_secs(1)
                };
                if rest.pause(id, rest_for.min(wait - waited)) {
                    return None;
                }
            }
        })()?,
        TELL => (|| {
            let agent = agent_id(arguments)?.ok_or("agent_id is required.")?;
            let message = text(arguments, "message", MAX_TEXT)?.to_owned();
            match host.ask(Event::Tell {
                agent,
                text: message,
            }) {
                Answer::Told { working } => Ok(format!(
                    "Sent to agent {agent}.{} Call wait_for_agent for its reply.",
                    if working {
                        " It was still working, so it takes the message when it gets to it."
                    } else {
                        ""
                    }
                )),
                other => Err(refusal(other)),
            }
        })(),
        LIST => match host.ask(Event::Collect {
            agent: None,
            take: false,
        }) {
            Answer::Agents { agents } if agents.is_empty() => {
                Ok("You have not started any agents that are still open.".to_owned())
            }
            Answer::Agents { agents } => Ok(agents
                .iter()
                .map(|report| {
                    format!(
                        "{}: {}{}",
                        name(report),
                        state(&report.status),
                        if report.news {
                            "; it has news for wait_for_agent"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")),
            other => Err(refusal(other)),
        },
        CLOSE => (|| {
            let agent = agent_id(arguments)?.ok_or("agent_id is required.")?;
            match host.ask(Event::Dismiss { agent }) {
                Answer::Done => Ok(format!(
                    "Closed agent {agent} and its terminal. reopen_agent brings it back with its conversation if you need it again."
                )),
                other => Err(refusal(other)),
            }
        })(),
        REPLY => text(arguments, "message", MAX_TEXT).and_then(|message| {
            if host.send(Event::Report {
                text: message.to_owned(),
                done: false,
            }) {
                Ok("Delivered to the agent that started you.".to_owned())
            } else {
                Err("Not delivered: no agent that started this one is still open.".to_owned())
            }
        }),
        _ => return Some(Err(format!("Neptune has no tool named {tool}."))),
    })
}

/// Waits for the application to open the terminal a request asked for.
/// `None` when the call was cancelled meanwhile.
fn opened(
    request: u64,
    id: &Value,
    host: &mut dyn Host,
    rest: &mut dyn Pause,
) -> Option<Result<u64, String>> {
    let deadline = Instant::now() + SPAWN_WAIT;
    loop {
        match host.ask(Event::SpawnResult { request }) {
            Answer::Spawned { agent } => return Some(Ok(agent)),
            Answer::Pending if Instant::now() < deadline => {
                if rest.pause(id, Duration::from_millis(50)) {
                    return None;
                }
            }
            Answer::Pending => {
                return Some(Err(
                    "Neptune did not open the terminal in time. Call list_agents to see whether the agent started before trying again.".to_owned(),
                ));
            }
            Answer::Refused { reason } => return Some(Err(reason)),
            _ => {
                return Some(Err(
                    "Neptune gave an answer this tool does not know.".to_owned()
                ));
            }
        }
    }
}
/// Waits for a started agent's CLI to take its task, so that whatever it
/// asks first is reported by the call that caused it. `started` leads the
/// answer. `None` when the call was cancelled meanwhile.
fn taken(
    agent: u64,
    started: &str,
    id: &Value,
    host: &mut dyn Host,
    rest: &mut dyn Pause,
) -> Option<Result<String, String>> {
    let after = format!(
        "It cannot see this conversation, and its terminal runs out of view: the user opens it from the agents list on this terminal's tab. Call wait_for_agent with agent_id {agent} to read its reply."
    );
    let deadline = Instant::now() + START_WAIT;
    loop {
        let look = Event::Collect {
            agent: Some(agent),
            take: false,
        };
        let status = match host.ask(look) {
            Answer::Agents { agents } => agents.into_iter().next().map(|report| report.status),
            _ => None,
        };
        match status {
            Some(Status::Starting) if Instant::now() < deadline => {
                if rest.pause(id, Duration::from_millis(250)) {
                    return None;
                }
            }
            Some(Status::Starting) | None => {
                return Some(Ok(format!(
                    "{started}. Its CLI is still starting and has not taken its task yet. {after}"
                )));
            }
            Some(status @ (Status::Asking { .. } | Status::Ended { .. })) => {
                // Told here, so it is not news again to the next wait.
                host.ask(Event::Collect {
                    agent: Some(agent),
                    take: true,
                });
                return Some(match status {
                    Status::Ended { reason } => {
                        Err(format!("{started}, but it is no longer running: {reason}."))
                    }
                    Status::Asking { screen } => Ok(format!(
                        "{started}. {}",
                        question(&format!("Agent {agent}"), &screen)
                    )),
                    _ => Ok(format!("{started}. {after}")),
                });
            }
            Some(_) => return Some(Ok(format!("{started}. It has taken its task. {after}"))),
        }
    }
}

/// Answers one JSON-RPC message; notifications have no answer, and neither
/// has a call cancelled while it waited. `spawned` adds what an agent started
/// by another agent is offered.
pub(super) fn respond(
    message: &Value,
    host: &mut dyn Host,
    spawned: bool,
    rest: &mut dyn Pause,
) -> Option<Value> {
    let id = message.get("id").filter(|id| !id.is_null())?.clone();
    let result = match message.get("method").and_then(Value::as_str) {
        Some("initialize") => json!({
            "protocolVersion": message
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "neptune", "version": env!("CARGO_PKG_VERSION")},
            "instructions": format!(
                "{INSTRUCTIONS}{}",
                if spawned { SPAWNED_INSTRUCTIONS } else { "" }
            ),
        }),
        Some("ping") => json!({}),
        Some("tools/list") => json!({"tools": tools(spawned)}),
        Some("tools/call") => {
            let tool = message
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let known = tool_names().any(|name| name == tool) && (spawned || tool != REPLY);
            if !known {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32602, "message": format!("Unknown tool: {tool}")},
                }));
            }
            let arguments = message
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or(Value::Null);
            let (text, failed) = match call(tool, &arguments, &id, host, rest)? {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            json!({"content": [{"type": "text", "text": text}], "isError": failed})
        }
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "Method not found"},
            }));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// Messages read ahead of the call being answered.
struct Inbox {
    messages: Receiver<Value>,
    waiting: VecDeque<Value>,
    closed: bool,
}
impl Pause for Inbox {
    fn pause(&mut self, call: &Value, duration: Duration) -> bool {
        match self.messages.recv_timeout(duration) {
            Ok(message) => {
                let cancels = message.get("method").and_then(Value::as_str)
                    == Some("notifications/cancelled")
                    && message.pointer("/params/requestId") == Some(call);
                if cancels {
                    return true;
                }
                // A cancellation of a call still queued takes it out of the queue.
                if message.get("method").and_then(Value::as_str) == Some("notifications/cancelled")
                    && let Some(id) = message.pointer("/params/requestId")
                {
                    self.waiting.retain(|queued| queued.get("id") != Some(id));
                } else {
                    self.waiting.push_back(message);
                }
                false
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
                true
            }
        }
    }
}

/// Serves newline-delimited messages until the agent closes its end. Input is
/// read on a thread of its own, so a call that waits still hears that it was
/// cancelled.
pub(super) fn serve(
    input: impl Read + Send + 'static,
    mut output: impl Write,
    host: &mut dyn Host,
    spawned: bool,
) -> std::io::Result<()> {
    let (sender, messages) = std::sync::mpsc::sync_channel(64);
    std::thread::Builder::new()
        .name("neptune-agent-mcp".into())
        .spawn(move || {
            let mut input = std::io::BufReader::new(input);
            let mut line = Vec::new();
            loop {
                line.clear();
                match (&mut input).take(MAX_LINE).read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                if line.last() != Some(&b'\n') && line.len() as u64 == MAX_LINE {
                    // An oversized message is dropped whole, not read as several.
                    let mut rest = Vec::new();
                    while (&mut input)
                        .take(MAX_LINE)
                        .read_until(b'\n', &mut rest)
                        .is_ok_and(|read| read > 0)
                        && rest.last() != Some(&b'\n')
                    {
                        rest.clear();
                    }
                    continue;
                }
                if let Ok(message) = serde_json::from_slice::<Value>(&line)
                    && sender.send(message).is_err()
                {
                    return;
                }
            }
        })?;
    let mut inbox = Inbox {
        messages,
        waiting: VecDeque::new(),
        closed: false,
    };
    loop {
        let message = match inbox.waiting.pop_front() {
            Some(message) => message,
            None if inbox.closed => return Ok(()),
            None => match inbox.messages.recv() {
                Ok(message) => message,
                Err(_) => return Ok(()),
            },
        };
        if let Some(answer) = respond(&message, host, spawned, &mut inbox) {
            serde_json::to_writer(&mut output, &answer)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Takes the first link only, and answers requests from a script.
    #[derive(Default)]
    struct Fake {
        linked: Vec<String>,
        asked: Vec<Event>,
        answers: VecDeque<Answer>,
        reported: Vec<String>,
    }
    impl Host for Fake {
        fn send(&mut self, event: Event) -> bool {
            match event {
                Event::PullRequest { url } => {
                    self.linked.push(url);
                    self.linked.len() == 1
                }
                Event::Report { text, done: false } => {
                    self.reported.push(text);
                    true
                }
                _ => false,
            }
        }
        fn ask(&mut self, event: Event) -> Answer {
            self.asked.push(event);
            self.answers.pop_front().unwrap_or(Answer::Refused {
                reason: "unscripted".into(),
            })
        }
    }
    struct NoRest;
    impl Pause for NoRest {
        fn pause(&mut self, _: &Value, _: Duration) -> bool {
            false
        }
    }
    fn call_message(id: u64, tool: &str, arguments: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":arguments}})
    }
    fn run(host: &mut Fake, spawned: bool, tool: &str, arguments: Value) -> (String, bool) {
        let answer = respond(
            &call_message(1, tool, arguments),
            host,
            spawned,
            &mut NoRest,
        )
        .unwrap();
        (
            answer["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            answer["result"]["isError"] == true,
        )
    }
    fn report(agent: u64, status: Status, news: bool, replies: &[&str]) -> AgentReport {
        AgentReport {
            agent,
            kind: AgentKind::Codex,
            status,
            news,
            replies: replies.iter().map(|reply| (*reply).to_owned()).collect(),
            reopens: false,
        }
    }

    #[test]
    fn the_server_introduces_its_tool_and_links_only_pull_request_addresses() {
        let mut host = Fake::default();
        let mut output = Vec::new();
        let call = |id: u64, url: &str| call_message(id, LINK, json!({"url": url})).to_string();
        let oversized = format!(
            "{{\"id\":9,\"method\":\"ping\",\"pad\":\"{}\"}}",
            "x".repeat(MAX_LINE as usize + 10)
        );
        let input = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}).to_string(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}).to_string(),
            call(3, "https://github.com/zevem/neptune/pull/83/files"),
            call(4, "https://example.com/not/a/pr"),
            oversized,
            "not json".into(),
            call(5, "https://github.com/zevem/neptune/pull/7"),
            json!({"jsonrpc":"2.0","id":6,"method":"resources/list"}).to_string(),
            call_message(7, REPLY, json!({"message": "hello"})).to_string(),
        ]
        .join("\n");
        serve(
            Cursor::new(input.into_bytes()),
            &mut output,
            &mut host,
            false,
        )
        .unwrap();
        let answers: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let ids: Vec<_> = answers
            .iter()
            .map(|answer| answer["id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(answers[0]["result"]["protocolVersion"], "2024-11-05");
        let instructions = answers[0]["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains(LINK) && instructions.contains(SPAWN));
        assert!(!instructions.contains(REPLY));
        let names: Vec<_> = answers[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, [LINK, SPAWN, WAIT, TELL, LIST, CLOSE, REOPEN, PRESS]);
        assert_eq!(answers[2]["result"]["isError"], false);
        assert_eq!(
            answers[2]["result"]["content"][0]["text"],
            "Linked zevem/neptune#83 to this terminal tab."
        );
        assert_eq!(answers[3]["result"]["isError"], true);
        assert_eq!(answers[4]["result"]["isError"], true);
        assert_eq!(answers[5]["error"]["code"], -32601);
        // An agent nobody started has no one to reply to.
        assert_eq!(answers[6]["error"]["code"], -32602);
        assert_eq!(
            host.linked,
            [
                "https://github.com/zevem/neptune/pull/83",
                "https://github.com/zevem/neptune/pull/7"
            ]
        );
    }

    #[test]
    fn an_agent_starts_another_waits_for_it_and_carries_on_the_conversation() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().canonicalize().unwrap();
        let mut host = Fake::default();
        // The application answers a frame after it is asked.
        host.answers.extend([
            Answer::Queued { request: 4 },
            Answer::Pending,
            Answer::Spawned { agent: 12 },
            // It answers once the CLI has taken its task.
            Answer::Agents {
                agents: vec![report(12, Status::Starting, false, &[])],
            },
            Answer::Agents {
                agents: vec![report(12, Status::Working, false, &[])],
            },
        ]);
        let (said, failed) = run(
            &mut host,
            false,
            SPAWN,
            json!({"agent": "codex", "prompt": "Build the API", "cwd": cwd}),
        );
        assert!(
            !failed
                && said.contains("Started Codex as agent 12")
                && said.contains("It has taken its task"),
            "{said}"
        );
        assert!(host.answers.is_empty());
        assert!(matches!(
            &host.asked[0],
            Event::Spawn { kind: AgentKind::Codex, prompt, cwd: sent, model: None, .. } if prompt == "Build the API" && *sent == cwd
        ));
        // What its CLI asks before taking the task is said at once, with
        // what its terminal shows, and is taken so it is not news twice.
        let asking = Answer::Agents {
            agents: vec![report(
                13,
                Status::Asking {
                    screen: "Do you trust this folder?".into(),
                },
                true,
                &[],
            )],
        };
        host.answers.extend([
            Answer::Queued { request: 5 },
            Answer::Spawned { agent: 13 },
            asking.clone(),
            asking,
        ]);
        let (said, failed) = run(
            &mut host,
            false,
            SPAWN,
            json!({"agent": "claude", "prompt": "x", "cwd": cwd, "model": " opus ", "effort": "xhigh", "ultra": true}),
        );
        assert!(
            !failed
                && said.starts_with(
                    "Started Claude Code as agent 13 on opus at xhigh effort with ultracode on"
                )
                && said.contains("Do you trust this folder?")
                && said.contains(PRESS),
            "{said}"
        );
        assert!(host.asked.iter().any(
            |event| matches!(event, Event::Spawn { model: Some(model), effort: Some(effort), ultra: true, .. } if model == "opus" && effort == "xhigh")
        ));
        assert!(matches!(
            host.asked.last(),
            Some(Event::Collect {
                agent: Some(13),
                take: true
            })
        ));
        host.answers.push_back(Answer::Done);
        let (said, failed) = run(
            &mut host,
            false,
            PRESS,
            json!({"agent_id": 13, "keys": ["1", "enter"]}),
        );
        assert!(!failed && said.contains("Pressed"), "{said}");
        assert!(matches!(
            host.asked.last(),
            Some(Event::Press { agent: 13, keys }) if *keys == ["1", "enter"]
        ));
        // One that was closed is opened again under a new number.
        host.answers.extend([
            Answer::Queued { request: 6 },
            Answer::Spawned { agent: 14 },
            Answer::Agents {
                agents: vec![report(14, Status::Working, false, &[])],
            },
        ]);
        let (said, failed) = run(
            &mut host,
            false,
            REOPEN,
            json!({"agent_id": 13, "message": "One more thing"}),
        );
        assert!(
            !failed && said.starts_with("Reopened agent 13") && said.contains("agent 14"),
            "{said}"
        );
        assert!(run(&mut host, false, REOPEN, json!({"agent_id": 13})).1);
        // Nothing unusable reaches the application.
        for arguments in [
            json!({"agent": "gemini", "prompt": "x"}),
            json!({"agent": "claude", "prompt": "  "}),
            json!({"agent": "claude", "prompt": "x".repeat(MAX_PROMPT + 1)}),
            json!({"agent": "claude", "prompt": "x", "cwd": cwd.join("missing")}),
        ] {
            let asked = host.asked.len();
            assert!(run(&mut host, false, SPAWN, arguments).1);
            assert_eq!(host.asked.len(), asked);
        }
        host.answers.push_back(Answer::Refused {
            reason: "Total pane limit reached".into(),
        });
        let refused = run(
            &mut host,
            false,
            SPAWN,
            json!({"agent": "claude", "prompt": "x"}),
        );
        assert_eq!(refused, ("Total pane limit reached".into(), true));

        // Waiting looks without taking until the agent rests, then takes once.
        host.asked.clear();
        host.answers.extend([
            Answer::Agents {
                agents: vec![report(12, Status::Working, false, &[])],
            },
            Answer::Agents {
                agents: vec![report(12, Status::Idle, true, &[])],
            },
            Answer::Agents {
                agents: vec![report(12, Status::Idle, true, &["Done: 3 endpoints."])],
            },
        ]);
        let (said, failed) = run(&mut host, false, WAIT, json!({"agent_id": 12}));
        assert!(!failed);
        assert_eq!(
            said,
            "Agent 12 (Codex) finished its turn and replied:\n\nDone: 3 endpoints."
        );
        let takes: Vec<bool> = host
            .asked
            .iter()
            .map(|event| {
                matches!(
                    event,
                    Event::Collect {
                        agent: Some(12),
                        take: true
                    }
                )
            })
            .collect();
        assert_eq!(takes, [false, false, true]);

        // A timeout says so, and what each agent is doing.
        host.answers.extend([
            Answer::Agents {
                agents: vec![report(12, Status::Working, false, &[])],
            },
            Answer::Agents {
                agents: vec![report(12, Status::Working, false, &[])],
            },
        ]);
        let (said, _) = run(&mut host, false, WAIT, json!({"timeout_seconds": 0}));
        assert!(
            said.starts_with("Agent 12 (Codex) is still working."),
            "{said}"
        );
        assert!(said.contains("Call wait_for_agent again"), "{said}");
        // Of several agents, one needing a person is news while another works.
        let waiting = Status::Waiting {
            attention: Attention::Permission,
        };
        let both = Answer::Agents {
            agents: vec![
                report(12, Status::Working, false, &[]),
                report(13, waiting, true, &[]),
            ],
        };
        host.answers.extend([both.clone(), both]);
        let (said, _) = run(&mut host, false, WAIT, json!({}));
        assert!(
            said.contains(
                "Agent 13 (Codex) is waiting for a person to answer a permission request"
            )
        );
        assert!(!said.contains("Call wait_for_agent again"));
        // An agent at rest that was already heard is waited on like any other.
        for _ in 0..2 {
            host.answers.push_back(Answer::Agents {
                agents: vec![report(12, Status::Idle, false, &[])],
            });
        }
        let (said, _) = run(
            &mut host,
            false,
            WAIT,
            json!({"agent_id": 12, "timeout_seconds": 0}),
        );
        assert!(
            said.starts_with("Agent 12 (Codex) is idle and has said nothing new."),
            "{said}"
        );
        assert!(said.contains("Call wait_for_agent again"), "{said}");
        // Agents that have all ended are not waited for.
        let ended = Answer::Agents {
            agents: vec![AgentReport {
                reopens: true,
                ..report(
                    12,
                    Status::Ended {
                        reason: "its terminal was closed".into(),
                    },
                    false,
                    &[],
                )
            }],
        };
        host.answers.extend([ended.clone(), ended]);
        let (said, _) = run(&mut host, false, WAIT, json!({}));
        assert!(
            said.contains("its terminal was closed") && said.contains(REOPEN),
            "{said}"
        );
        assert!(!said.contains("Call wait_for_agent again"), "{said}");
        host.answers
            .push_back(Answer::Agents { agents: Vec::new() });
        assert!(
            run(&mut host, false, WAIT, json!({}))
                .0
                .contains("not started any agents")
        );

        host.answers.push_back(Answer::Told { working: true });
        let (said, failed) = run(
            &mut host,
            false,
            TELL,
            json!({"agent_id": 12, "message": "Also add tests"}),
        );
        assert!(!failed && said.contains("still working"), "{said}");
        assert!(
            run(
                &mut host,
                false,
                TELL,
                json!({"agent_id": 12, "message": ""})
            )
            .1
        );
        assert!(run(&mut host, false, TELL, json!({"message": "x"})).1);
        host.answers.push_back(Answer::Agents {
            agents: vec![report(12, Status::Idle, false, &[])],
        });
        assert_eq!(
            run(&mut host, false, LIST, json!({})).0,
            "Agent 12 (Codex): idle"
        );
        host.answers.push_back(Answer::Done);
        assert_eq!(
            run(&mut host, false, CLOSE, json!({"agent_id": 12})),
            (
                "Closed agent 12 and its terminal. reopen_agent brings it back with its conversation if you need it again."
                    .into(),
                false
            )
        );

        // A started agent is told so, and can speak before its turn ends.
        let started = respond(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
            &mut host,
            true,
            &mut NoRest,
        )
        .unwrap();
        assert!(
            started["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains(REPLY)
        );
        let (said, failed) = run(&mut host, true, REPLY, json!({"message": "Which port?"}));
        assert!(!failed, "{said}");
        assert_eq!(host.reported, ["Which port?"]);
    }

    #[test]
    fn a_cancelled_wait_gives_no_answer_and_keeps_what_was_read_ahead() {
        let (sender, messages) = std::sync::mpsc::sync_channel(8);
        let mut inbox = Inbox {
            messages,
            waiting: VecDeque::new(),
            closed: false,
        };
        let mut host = Fake::default();
        for _ in 0..2 {
            host.answers.push_back(Answer::Agents {
                agents: vec![report(12, Status::Working, false, &[])],
            });
        }
        sender
            .send(json!({"jsonrpc":"2.0","id":8,"method":"ping"}))
            .unwrap();
        sender
            .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}))
            .unwrap();
        let waited = respond(
            &call_message(1, WAIT, json!({"agent_id": 12})),
            &mut host,
            false,
            &mut inbox,
        );
        assert_eq!(waited, None);
        // Nothing was taken from the agent, and the ping is still answered.
        assert!(
            host.asked
                .iter()
                .all(|event| matches!(event, Event::Collect { take: false, .. }))
        );
        assert_eq!(inbox.waiting.len(), 1);
        // A call cancelled while it was queued behind a wait never runs.
        let (queue, messages) = std::sync::mpsc::sync_channel(8);
        let mut queued = Inbox {
            messages,
            waiting: VecDeque::new(),
            closed: false,
        };
        queue
            .send(call_message(5, CLOSE, json!({"agent_id": 12})))
            .unwrap();
        queue
            .send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":5}}))
            .unwrap();
        assert!(!queued.pause(&json!(4), Duration::ZERO));
        assert!(!queued.pause(&json!(4), Duration::ZERO));
        assert!(queued.waiting.is_empty());
        // The agent closing its end stops a wait as well.
        drop(sender);
        host.answers.push_back(Answer::Agents {
            agents: vec![report(12, Status::Working, false, &[])],
        });
        let message = call_message(2, WAIT, json!({"agent_id": 12}));
        assert_eq!(respond(&message, &mut host, false, &mut inbox), None);
        assert!(inbox.closed);
    }
}
