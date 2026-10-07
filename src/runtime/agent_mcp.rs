//! The tools Neptune offers a CLI agent or a project's lead, served over the
//! CLI's own stdio pipes. A pull request address leaves this process, and so
//! do the tasks, messages and replies exchanged with started agents.
use super::agents::{
    AgentReport, Answer, Event, MAX_TEXT, MAX_TITLE, ProjectCall, Role, Status, WatchTrigger, label,
};
use crate::agent_activity::Attention;
use neptune_model::{AgentKind, Attachment, PullRequest};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{BufRead, Read, Write},
    path::PathBuf,
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
/// How long a lead's call waits where git makes a worktree first.
const WORKTREE_WAIT: Duration = Duration::from_secs(30);
/// How long starting an agent waits to see its CLI take the task or ask
/// something first, so that a question is reported where it was caused.
const START_WAIT: Duration = Duration::from_secs(20);
/// The lead Neptune puts before a task takes part of its room.
const MAX_PROMPT: usize = MAX_TEXT - 1024;
/// The longest task a project's lead hands an agent.
const LEAD_PROMPT: usize = 24 * 1024;

const LINK: &str = "link_pull_request";
const ATTACH: &str = "attach_file";
const SPAWN: &str = "spawn_agent";
const WAIT: &str = "wait_for_agent";
const TELL: &str = "send_agent_message";
const LIST: &str = "list_agents";
const CLOSE: &str = "close_agent";
const REPLY: &str = "reply_to_parent";
const REOPEN: &str = "reopen_agent";
const PRESS: &str = "press_agent_keys";
const REPORT: &str = "agent_report";
const READ: &str = "read_context";
const WRITE: &str = "write_context";
const DECIDE: &str = "record_decision";
const NOTE: &str = "add_note";
const WATCH: &str = "add_subscription";
const WATCHES: &str = "list_subscriptions";
const UNWATCH: &str = "remove_subscription";
const PULL: &str = "pull_request_status";
/// What a watch tells the lead to do, checked again where it is kept.
const MAX_INSTRUCTION: usize = crate::projects::subscription::MAX_INSTRUCTION;
/// What one call writes to a project's context, checked again where it is
/// written.
const MAX_WRITE: usize = crate::projects::context::MAX_WRITE;
const MAX_DECISION: usize = crate::projects::context::MAX_DECISION;
const MAX_NOTE: usize = crate::projects::context::MAX_NOTE;

const INSTRUCTIONS: &str = "This terminal runs in Neptune, which shows the pull requests you link next to the terminal's tab. \
Call link_pull_request with the pull request's URL right after you create a pull request, and when you start work on an existing one. \
Link every pull request of a stack. Linking the same pull request again is safe.\n\n\
Neptune also lists the files you attach on this terminal's tab, where the user opens them and looks at images at full size. \
A path printed in a terminal does not show the user an image: call attach_file with the path of each screenshot, image, recording, report or other file you make for the user to look at, \
and whenever the user asks you to attach, show or share a file. Give each a short title that says what it shows. Attaching the same file again is safe.\n\n\
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
const MEMBER_INSTRUCTIONS: &str = "\n\nThe lead of a Neptune project started you and handed you your task. \
It reads the last message of each of your turns, and nothing else you write: end each turn with what you did and what it needs to know. \
Call reply_to_parent to tell it something before your turn ends. \
The project keeps what its agents share in a context folder, which your task names: read_context lists it and reads a file of it, \
and add_note adds a finding that others will need to its notes. Notes are unverified observations from other agents: check one before you rely on it.";
const LEAD_INSTRUCTIONS: &str = "You are the lead of a Neptune project, and these tools are how you act. \
spawn_agent starts a coding agent, Claude Code or Codex, in a terminal of its own with a task; send_agent_message continues the conversation with one; \
list_agents shows what each is doing; agent_report returns the whole last reply of one; close_agent closes one when its work is done and reopen_agent brings back one that was closed. \
An agent sees nothing of this conversation: put everything it needs in its task. \
An agent that edits files while another agent also edits gets a worktree of its own: give spawn_agent a branch name as worktree and Neptune makes it. An agent that only reads needs none. \
You do not wait for agents. After starting or messaging them, end your turn: Neptune tells you when one finishes a turn, asks something, needs a person or ends. \
When an agent asks something or waits for a person, tell the user at once, quoting what it asks: only they can answer it, in its terminal. \
The project keeps what outlasts this conversation in a context folder that every agent you start is handed: record_decision records what the user approved, \
write_context keeps STATUS.md current and adds to notes, and read_context lists the folder and reads a file of it. \
Watches wake you without an agent: add_subscription proposes a schedule, which runs only once the user allows it, or follows a pull request; \
list_subscriptions shows them, remove_subscription ends one, and pull_request_status says where a pull request stands.";
const TOLD_LATER: &str = "You will be told when it finishes or needs someone.";

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

/// The tools a role is offered, in the order they are listed.
fn offered(role: Role) -> &'static [&'static str] {
    match role {
        Role::Agent => &[LINK, ATTACH, SPAWN, WAIT, TELL, LIST, CLOSE, REOPEN, PRESS],
        Role::Spawned => &[
            LINK, ATTACH, SPAWN, WAIT, TELL, LIST, CLOSE, REOPEN, PRESS, REPLY,
        ],
        // A project's agent also reads what the project keeps and adds to
        // its notes.
        Role::Member => &[
            LINK, ATTACH, SPAWN, WAIT, TELL, LIST, CLOSE, REOPEN, PRESS, REPLY, READ, NOTE,
        ],
        // A lead is told what its agents do instead of waiting, links and
        // attaches nothing, and presses no keys for anyone.
        Role::Lead => &[
            SPAWN, TELL, LIST, REPORT, CLOSE, REOPEN, READ, WRITE, DECIDE, WATCH, WATCHES, UNWATCH,
            PULL,
        ],
    }
}
fn instructions(role: Role) -> String {
    match role {
        Role::Agent => INSTRUCTIONS.to_owned(),
        Role::Spawned => format!("{INSTRUCTIONS}{SPAWNED_INSTRUCTIONS}"),
        Role::Member => format!("{INSTRUCTIONS}{MEMBER_INSTRUCTIONS}"),
        Role::Lead => LEAD_INSTRUCTIONS.to_owned(),
    }
}
/// The tools a launch allows without asking: those that read, link, attach
/// or answer, and a project's agent adding to its project's notes, which
/// Neptune keeps inside the project's own folder. Starting an agent, typing
/// for one and closing one hand work to another CLI or stop it, so each
/// agent's own permission rules decide them.
pub(super) fn allowed_tools() -> impl Iterator<Item = &'static str> {
    [LINK, ATTACH, WAIT, LIST, REPLY, READ, NOTE].into_iter()
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

fn tools(role: Role) -> Vec<Value> {
    if role == Role::Lead {
        return lead_tools();
    }
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
            "name": ATTACH,
            "annotations": hints(false, false, false),
            "description": "Attach a file to this Neptune terminal tab for the user to look at: a screenshot or other image, a recording, a report, a log, a document. The tab lists the attached files, shows images at full size, opens a file with its application and shows it in the file manager. Attach what you made for the user to review, and what the user asks you to attach or show. Call it once for each file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file's path on this machine, absolute or relative to your working directory",
                    },
                    "title": {
                        "type": "string",
                        "description": "A few words saying what the file shows, such as \"After: the settings dialog\"; the file's name is shown without one",
                    },
                },
                "required": ["path"],
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
    if offered(role).contains(&REPLY) {
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
    if offered(role).contains(&READ) {
        tools.push(read_tool());
    }
    if offered(role).contains(&NOTE) {
        tools.push(json!({
            "name": NOTE,
            "annotations": hints(false, false, false),
            "description": "Add a finding that other agents of the project will need to the project's notes on a topic: a fact about the code, a trap, a command that works. It is added to notes/<topic>.md with your name and the time; notes are only ever added to. Say what you observed and how, since readers take a note as unverified.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "A short name of lower-case letters, digits and hyphens, such as auth-tokens. Use a topic from the index where one fits.",
                    },
                    "content": {"type": "string", "description": "The finding, in a few lines"},
                },
                "required": ["topic", "content"],
                "additionalProperties": false,
            },
        }));
    }
    tools
}
/// Reading what a project keeps, as its lead and its agents are offered it.
fn read_tool() -> Value {
    json!({
        "name": READ,
        "annotations": hints(true, false, false),
        "description": "Read what the project keeps for its agents. Without a path, the index: every file with its first heading, size, last author and date. With a path from the index, such as STATUS.md, DECISIONS.md, INSTRUCTIONS.md or notes/<topic>.md, that file. A long file comes in parts; the answer says which offset reads on. Notes are unverified observations from other agents.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "A file named in the index; the index itself when left out"},
                "offset": {"type": "integer", "minimum": 0, "description": "Where to read on in a long file, as an earlier answer said"},
            },
            "additionalProperties": false,
        },
    })
}
/// What a project's lead is offered. Every one is allowed when its CLI is
/// started; what a tool may do is decided where the application answers.
fn lead_tools() -> Vec<Value> {
    let agent_id = json!({
        "type": "integer",
        "description": "The number spawn_agent gave the agent",
    });
    vec![
        json!({
            "name": SPAWN,
            "annotations": hints(false, false, true),
            "description": "Start a coding agent (Claude Code or Codex) for this project in a Neptune terminal of its own, out of view, and hand it a task. It runs interactively with its own conversation, tools and permissions, and sees nothing of this conversation. Returns its agent number as soon as its terminal is open. Neptune tells you when it finishes a turn, asks something, needs a person or ends, so end your turn instead of waiting.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex"],
                        "description": "Which agent to start: claude is Claude Code, codex is Codex",
                    },
                    "title": {
                        "type": "string",
                        "maxLength": MAX_TITLE,
                        "description": "A few words naming the task, such as auth-refactor; the user sees it beside the agent",
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The whole task, with everything the agent needs to know: goal, files, constraints, what counts as done, and what to report back.",
                    },
                    "cwd": {
                        "type": "string",
                        "description": "The absolute path of the directory it works in: the project's directory or one inside it. The project's directory when left out.",
                    },
                    "worktree": {
                        "type": "string",
                        "description": "A branch name, such as fix-login. Neptune makes a git worktree of that branch beside the project's repository, or finds the one the branch has, and the agent works there. Give one to every agent that edits files while another agent also edits; an agent that only reads needs none. Not together with cwd.",
                    },
                    "model": {
                        "type": "string",
                        "description": "The model it runs on, named as its CLI names it: for claude an alias such as opus, sonnet or haiku, or a full model id; for codex a model id such as gpt-5.1-codex. Leave out for the CLI's own default; give one when the user asks for a model.",
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
                },
                "required": ["agent", "title", "prompt"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": TELL,
            "annotations": hints(false, false, true),
            "description": "Send a follow-up message to one of the project's agents, as its next prompt: an answer, a correction or more work. Neptune tells you when it replies.",
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
            "description": "List the project's agents and what each is doing, without reading their replies.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        }),
        json!({
            "name": REPORT,
            "annotations": hints(true, false, false),
            "description": "Read the whole reply the last turn of one of the project's agents ended with, when what you were told of it was cut short.",
            "inputSchema": {
                "type": "object",
                "properties": {"agent_id": agent_id},
                "required": ["agent_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": CLOSE,
            "annotations": hints(false, true, false),
            "description": "Close one of the project's agents, and its terminal, once its work is done. Its unfinished work stops; what it wrote to disk stays. reopen_agent brings it back with its conversation if you need it again.",
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
            "description": "Open again one of the project's agents whose terminal was closed, by you or by the user, or whose CLI exited. It goes on with the conversation it had, in the directory and with the model, effort and ultra mode it had, and takes your message as its next prompt. Returns its new agent number.",
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
        read_tool(),
        json!({
            "name": WRITE,
            "annotations": hints(false, false, false),
            "description": "Write to what the project keeps. STATUS.md is yours: the goal, what is in progress, what is next, what is done, and links; it is replaced by what you pass (the one before is kept), or added to with append. notes/<topic>.md takes a finding worth keeping and is only ever added to, with your name and the time. INSTRUCTIONS.md is the user's, DECISIONS.md takes record_decision, and INDEX.md is written by Neptune.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "STATUS.md, or notes/<topic>.md with a topic of lower-case letters, digits and hyphens"},
                    "content": {"type": "string", "description": "What to write, as Markdown"},
                    "append": {"type": "boolean", "description": "Add to STATUS.md instead of replacing it. Notes are always added to."},
                },
                "required": ["path", "content"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": DECIDE,
            "annotations": hints(false, false, false),
            "description": "Record a decision the user approved, at once: a plan they agreed to, a choice between options, a constraint, something ruled out. It is added to DECISIONS.md with the time and is handed to every agent you start from then on. A decision is never changed; one that is overturned gets a new entry that says so.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "decision": {"type": "string", "description": "What was decided, in a sentence or two"},
                    "why": {"type": "string", "description": "The reason, where there is one"},
                },
                "required": ["decision"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": WATCH,
            "annotations": hints(false, false, false),
            "description": "Add a watch that wakes you without an agent. A schedule runs every so many minutes while Neptune is open and hands you its instruction each time; it is only proposed here, and does nothing until the user allows it in the chat, so tell them you proposed it. A pull request watch wakes you when the pull request's checks fail or pass, when it is merged or closed, or when more review comments are open; it starts at once. Neptune already follows every pull request your agents link.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "maxLength": 80, "description": "A few words the user will recognise it by"},
                    "trigger": {
                        "type": "object",
                        "description": "Either {\"schedule\": {\"every_minutes\": N}} with N of 15 or more, or {\"pull_request\": \"<address>\"}, or {\"pull_request\": \"linked\"} for every pull request the project's agents linked",
                        "properties": {
                            "schedule": {
                                "type": "object",
                                "properties": {"every_minutes": {"type": "integer", "minimum": 15, "maximum": 10080}},
                                "required": ["every_minutes"],
                                "additionalProperties": false,
                            },
                            "pull_request": {"type": "string"},
                        },
                        "additionalProperties": false,
                    },
                    "instruction": {"type": "string", "description": "What you should do each time it fires, written so that it stands alone on a later day. Required for a schedule."},
                },
                "required": ["title", "trigger"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": WATCHES,
            "annotations": hints(true, false, false),
            "description": "List the project's watches: each one's number, what sets it off, when it runs next or how its pull request stands, and whether it is paused or still waits for the user to allow it.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        }),
        json!({
            "name": UNWATCH,
            "annotations": hints(false, true, false),
            "description": "End one of the project's watches, by the number list_subscriptions gives it.",
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "integer", "description": "The watch's number"}},
                "required": ["id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": PULL,
            "annotations": hints(true, false, true),
            "description": "Where a pull request stands as Neptune last read it through the user's GitHub CLI: open, draft, merged or closed, its checks, and how many review comments are unresolved. Without an address, every pull request the project's agents linked or a watch follows. \"checking\" means it is being read: ask again in a moment. \"unavailable\" means it could not be read. It never returns what anyone wrote on the pull request.",
            "inputSchema": {
                "type": "object",
                "properties": {"url": {"type": "string", "description": "The address of one pull request"}},
                "additionalProperties": false,
            },
        }),
    ]
}

/// Asks the application for a file of the project's context, or its index.
fn read_context(
    arguments: &Value,
    id: &Value,
    host: &mut dyn Host,
    rest: &mut dyn Pause,
) -> Option<Result<String, String>> {
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned);
    let offset = match arguments.get("offset") {
        None | Some(Value::Null) => 0,
        Some(offset) => match offset.as_u64() {
            Some(offset) => offset,
            None => return Some(Err("offset is a number of bytes, zero or more.".to_owned())),
        },
    };
    project_call(ProjectCall::ReadContext { path, offset }, id, host, rest)
}
/// Hands the application a call of a project and waits for what it came to.
fn project_call(
    call: ProjectCall,
    id: &Value,
    host: &mut dyn Host,
    rest: &mut dyn Pause,
) -> Option<Result<String, String>> {
    match host.ask(Event::Project { call }) {
        Answer::Queued { request } => answered(request, id, host, rest),
        Answer::Refused { reason } => Some(Err(reason)),
        _ => Some(Err(
            "Neptune gave an answer this tool does not know.".to_owned()
        )),
    }
}
/// A finding for the notes on a topic, from the lead or one of its agents.
fn add_note(
    arguments: &Value,
    id: &Value,
    host: &mut dyn Host,
    rest: &mut dyn Pause,
) -> Option<Result<String, String>> {
    let topic = match text(arguments, "topic", 128) {
        Ok(topic) => topic.trim().to_owned(),
        Err(error) => return Some(Err(error)),
    };
    let content = match text(arguments, "content", MAX_NOTE) {
        Ok(content) => content.to_owned(),
        Err(error) => return Some(Err(error)),
    };
    project_call(ProjectCall::AddNote { topic, content }, id, host, rest)
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

/// The file an agent names, which must be there to be looked at. A relative
/// path is the agent's own, read from this process's directory, and `~` is
/// the user's home.
fn attachment(path: &str, title: Option<&str>) -> Result<Attachment, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("path names no file.".to_owned());
    }
    let home = path
        .strip_prefix("~/")
        .and_then(|rest| Some(directories::BaseDirs::new()?.home_dir().join(rest)));
    let path = home.unwrap_or_else(|| PathBuf::from(path));
    let path =
        std::path::absolute(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    match std::fs::metadata(&path) {
        Err(error) => Err(format!("{}: {error}", path.display())),
        Ok(metadata) if metadata.is_dir() => Err(format!(
            "{} is a folder; attach the files in it one at a time.",
            path.display()
        )),
        Ok(_) => Attachment::new(path.clone(), title)
            .ok_or_else(|| format!("{} is not a path Neptune can keep.", path.display())),
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
        ATTACH => {
            let title = arguments.get("title").and_then(Value::as_str);
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match attachment(path, title) {
                Err(reason) => Err(format!("Not attached: {reason}")),
                Ok(file)
                    if host.send(Event::Attachment {
                        path: file.path().into(),
                        title: file.title().map(str::to_owned),
                    }) =>
                {
                    Ok(format!(
                        "Attached {} to this terminal tab, where the user can open it.",
                        file.name()
                    ))
                }
                Ok(_) => Err(
                    "Not attached: Neptune is not tracking an agent in this terminal.".to_owned(),
                ),
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
                title: None,
                worktree: None,
            }) {
                Answer::Queued { request } => request,
                other => return Some(Err(refusal(other))),
            };
            let agent = match opened(request, id, host, rest, SPAWN_WAIT)? {
                Ok((agent, _)) => agent,
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
            let reopened = match opened(request, id, host, rest, SPAWN_WAIT)? {
                Ok((agent, _)) => agent,
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
        READ => read_context(arguments, id, host, rest)?,
        NOTE => add_note(arguments, id, host, rest)?,
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

/// Runs one tool for a project's lead. Nothing here waits for an agent: the
/// application tells the lead what becomes of each. `None` when the call was
/// cancelled while the application had yet to answer.
fn lead_call(
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
    let agent = || agent_id(arguments)?.ok_or_else(|| "agent_id is required.".to_owned());
    Some(match tool {
        SPAWN => (|| {
            let kind = match arguments.get("agent").and_then(Value::as_str) {
                Some("claude") => AgentKind::Claude,
                Some("codex") => AgentKind::Codex,
                _ => return Some(Err("agent is \"claude\" or \"codex\".".to_owned())),
            };
            let title = match text(arguments, "title", MAX_TEXT) {
                Ok(title)
                    if title.chars().count() <= MAX_TITLE
                        && !title.chars().any(char::is_control) =>
                {
                    title.trim().to_owned()
                }
                Ok(_) => {
                    return Some(Err(format!(
                        "title is a name of up to {MAX_TITLE} characters on one line."
                    )));
                }
                Err(error) => return Some(Err(error)),
            };
            let prompt = match text(arguments, "prompt", LEAD_PROMPT) {
                Ok(prompt) => prompt.to_owned(),
                Err(error) => return Some(Err(error)),
            };
            // The lead runs outside the project's directory, so only the
            // application knows where "here" is: an empty path asks for it.
            let cwd = match arguments
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|cwd| !cwd.is_empty())
            {
                None => PathBuf::new(),
                Some(cwd) => {
                    let cwd = PathBuf::from(cwd);
                    if !cwd.is_absolute() || !cwd.is_dir() {
                        return Some(Err(
                            "cwd is not the absolute path of a directory that exists.".to_owned(),
                        ));
                    }
                    cwd
                }
            };
            let word = |field: &str| {
                arguments
                    .get(field)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|word| !word.is_empty())
                    .map(str::to_owned)
            };
            let worktree = word("worktree");
            let request = match host.ask(Event::Spawn {
                kind,
                prompt,
                cwd: cwd.clone(),
                model: word("model"),
                effort: word("effort"),
                ultra: arguments.get("ultra").and_then(Value::as_bool) == Some(true),
                title: Some(title),
                worktree: worktree.clone(),
            }) {
                Answer::Queued { request } => request,
                other => return Some(Err(refusal(other))),
            };
            // Git makes the worktree before the terminal is opened.
            let wait = match worktree {
                Some(_) => WORKTREE_WAIT,
                None => SPAWN_WAIT,
            };
            // What it runs with is what Neptune started it with: the
            // user's settings for the project win over what was asked here.
            Some(opened(request, id, host, rest, wait)?.map(|(agent, with)| {
                let place = match &worktree {
                    Some(branch) => format!("a worktree of its own, on branch {branch}"),
                    None if cwd.as_os_str().is_empty() => "the project's directory".to_owned(),
                    None => cwd.display().to_string(),
                };
                format!("Agent {agent} started{with} in {place}. {TOLD_LATER}")
            }))
        })()?,
        REOPEN => (|| {
            let agent = match agent() {
                Ok(agent) => agent,
                Err(error) => return Some(Err(error)),
            };
            let message = match text(arguments, "message", LEAD_PROMPT) {
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
            Some(opened(request, id, host, rest, SPAWN_WAIT)?.map(|(reopened, _)| {
                format!(
                    "Reopened agent {agent} with its conversation. It is agent {reopened} from now on. {TOLD_LATER}"
                )
            }))
        })()?,
        TELL => (|| {
            let agent = agent()?;
            let message = text(arguments, "message", MAX_TEXT)?.to_owned();
            match host.ask(Event::Tell {
                agent,
                text: message,
            }) {
                Answer::Told { working } => Ok(format!(
                    "Sent to agent {agent}.{} You will be told when it replies.",
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
                Ok("The project has no agents that are open or can be reopened.".to_owned())
            }
            Answer::Agents { agents } => Ok(agents
                .iter()
                .map(|report| {
                    format!(
                        "{}{}: {}{}",
                        name(report),
                        report
                            .title
                            .as_ref()
                            .map_or(String::new(), |title| format!(" \"{title}\"")),
                        state(&report.status),
                        if report.reopens {
                            "; reopen_agent opens it again"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")),
            other => Err(refusal(other)),
        },
        REPORT => (|| {
            let agent = match agent() {
                Ok(agent) => agent,
                Err(error) => return Some(Err(error)),
            };
            project_call(ProjectCall::Report { agent }, id, host, rest)
        })()?,
        READ => read_context(arguments, id, host, rest)?,
        WRITE => (|| {
            let path = match text(arguments, "path", 256) {
                Ok(path) => path.trim().to_owned(),
                Err(error) => return Some(Err(error)),
            };
            let content = match text(arguments, "content", MAX_WRITE) {
                Ok(content) => content.to_owned(),
                Err(error) => return Some(Err(error)),
            };
            let append = match arguments.get("append") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(append)) => Some(*append),
                Some(_) => return Some(Err("append is true or false.".to_owned())),
            };
            let call = ProjectCall::WriteContext {
                path,
                content,
                append,
            };
            project_call(call, id, host, rest)
        })()?,
        DECIDE => (|| {
            let decision = match text(arguments, "decision", MAX_DECISION) {
                Ok(decision) => decision.to_owned(),
                Err(error) => return Some(Err(error)),
            };
            let why = match arguments.get("why").and_then(Value::as_str).map(str::trim) {
                None | Some("") => None,
                Some(why) if why.len() > MAX_DECISION => {
                    return Some(Err(format!(
                        "why is longer than {} KB. Keep the reason to a sentence or two.",
                        MAX_DECISION / 1024
                    )));
                }
                Some(why) => Some(why.to_owned()),
            };
            project_call(
                ProjectCall::RecordDecision { decision, why },
                id,
                host,
                rest,
            )
        })()?,
        CLOSE => (|| {
            let agent = agent()?;
            match host.ask(Event::Dismiss { agent }) {
                // A terminal the user opened as a tab is theirs to close.
                Answer::Done => Ok(format!(
                    "Agent {agent} is no longer an agent of this project. Its terminal was closed, unless the user had opened it as a tab: that one stays open for them. Its worktree, if it had one, stays on disk."
                )),
                other => Err(refusal(other)),
            }
        })(),
        WATCH => (|| {
            let title = match text(arguments, "title", MAX_TEXT) {
                Ok(title) => title.trim().to_owned(),
                Err(error) => return Some(Err(error)),
            };
            const SHAPE: &str = "trigger is {\"schedule\": {\"every_minutes\": N}} or {\"pull_request\": \"<address>\"} or {\"pull_request\": \"linked\"}.";
            let trigger = arguments.get("trigger").and_then(Value::as_object);
            let trigger = match trigger.map(|trigger| {
                (
                    trigger.len(),
                    trigger.get("schedule"),
                    trigger.get("pull_request"),
                )
            }) {
                Some((1, Some(schedule), None)) => {
                    if schedule.get("daily_at").is_some() {
                        return Some(Err(
                            "A watch runs every so many minutes, not at a time of day: give every_minutes.".to_owned(),
                        ));
                    }
                    let every = schedule
                        .as_object()
                        .filter(|schedule| schedule.len() == 1)
                        .and_then(|schedule| schedule.get("every_minutes"))
                        .and_then(Value::as_u64)
                        .and_then(|every| u32::try_from(every).ok());
                    match every {
                        Some(every_minutes) => WatchTrigger::Schedule { every_minutes },
                        None => {
                            return Some(Err(
                                "every_minutes is a whole number of minutes, 15 or more."
                                    .to_owned(),
                            ));
                        }
                    }
                }
                Some((1, None, Some(Value::String(link)))) if !link.trim().is_empty() => {
                    WatchTrigger::PullRequest(link.trim().to_owned())
                }
                _ => return Some(Err(SHAPE.to_owned())),
            };
            let instruction = match arguments.get("instruction") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(instruction)) if instruction.len() <= MAX_INSTRUCTION => {
                    instruction.trim().to_owned()
                }
                Some(Value::String(_)) => {
                    return Some(Err(format!(
                        "instruction is longer than {} KB. Keep it to what must be done each time.",
                        MAX_INSTRUCTION / 1024
                    )));
                }
                Some(_) => return Some(Err("instruction is text.".to_owned())),
            };
            let call = ProjectCall::AddSubscription {
                title,
                trigger,
                instruction,
            };
            project_call(call, id, host, rest)
        })()?,
        WATCHES => project_call(ProjectCall::ListSubscriptions, id, host, rest)?,
        UNWATCH => match arguments.get("id").and_then(Value::as_u64) {
            Some(watch) => project_call(
                ProjectCall::RemoveSubscription { id: watch },
                id,
                host,
                rest,
            )?,
            None => Err("id is the number list_subscriptions gives the watch.".to_owned()),
        },
        PULL => (|| {
            let url = match arguments.get("url") {
                None | Some(Value::Null) => None,
                Some(Value::String(url)) if url.len() <= 512 => {
                    Some(url.trim().to_owned()).filter(|url| !url.is_empty())
                }
                Some(_) => return Some(Err("url is the address of a pull request.".to_owned())),
            };
            project_call(ProjectCall::PullRequestStatus { url }, id, host, rest)
        })()?,
        _ => return Some(Err(format!("Neptune has no tool named {tool}."))),
    })
}

/// Waits for the application to answer a project call. `None` when the call
/// was cancelled meanwhile.
fn answered(
    request: u64,
    id: &Value,
    host: &mut dyn Host,
    rest: &mut dyn Pause,
) -> Option<Result<String, String>> {
    let deadline = Instant::now() + SPAWN_WAIT;
    loop {
        match host.ask(Event::CallResult { request }) {
            Answer::Text { text } => return Some(Ok(text)),
            Answer::Pending if Instant::now() < deadline => {
                if rest.pause(id, Duration::from_millis(50)) {
                    return None;
                }
            }
            Answer::Pending => {
                return Some(Err("Neptune did not answer in time; try again.".to_owned()));
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
/// Waits for the application to open the terminal a request asked for.
/// `None` when the call was cancelled meanwhile.
fn opened(
    request: u64,
    id: &Value,
    host: &mut dyn Host,
    rest: &mut dyn Pause,
    wait: Duration,
) -> Option<Result<(u64, String), String>> {
    let deadline = Instant::now() + wait;
    loop {
        match host.ask(Event::SpawnResult { request }) {
            Answer::Spawned { agent, with } => return Some(Ok((agent, with))),
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
/// has a call cancelled while it waited. `role` decides which tools and
/// words are offered.
pub(super) fn respond(
    message: &Value,
    host: &mut dyn Host,
    role: Role,
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
            "instructions": instructions(role),
        }),
        Some("ping") => json!({}),
        Some("tools/list") => json!({"tools": tools(role)}),
        Some("tools/call") => {
            let tool = message
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !offered(role).contains(&tool) {
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
            let result = if role == Role::Lead {
                lead_call(tool, &arguments, &id, host, rest)
            } else {
                call(tool, &arguments, &id, host, rest)
            };
            let (text, failed) = match result? {
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
    role: Role,
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
        if let Some(answer) = respond(&message, host, role, &mut inbox) {
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
        attached: Vec<(PathBuf, Option<String>)>,
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
                Event::Attachment { path, title } => {
                    self.attached.push((path, title));
                    true
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
        let role = if spawned { Role::Spawned } else { Role::Agent };
        run_as(host, role, tool, arguments)
    }
    fn run_as(host: &mut Fake, role: Role, tool: &str, arguments: Value) -> (String, bool) {
        let answer = respond(&call_message(1, tool, arguments), host, role, &mut NoRest).unwrap();
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
            title: None,
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
            Role::Agent,
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
        assert!(instructions.contains(ATTACH));
        assert!(!instructions.contains(REPLY));
        let names: Vec<_> = answers[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [LINK, ATTACH, SPAWN, WAIT, TELL, LIST, CLOSE, REOPEN, PRESS]
        );
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
    fn files_are_attached_by_path_when_they_exist() {
        let directory = tempfile::tempdir().unwrap();
        let shot = directory.path().join("after shot.png");
        std::fs::write(&shot, "picture").unwrap();
        let mut host = Fake::default();
        let attach = |host: &mut Fake, arguments| run(host, false, ATTACH, arguments);
        let (text, error) = attach(
            &mut host,
            json!({"path": shot.to_str().unwrap(), "title": "After:\nthe dialog"}),
        );
        assert!(!error, "{text}");
        assert!(text.contains("after shot.png"), "{text}");
        // A folder, a missing file and no path at all are refused, with why.
        for (path, reason) in [
            (directory.path().to_str().unwrap(), "is a folder"),
            (
                directory.path().join("gone.png").to_str().unwrap(),
                "gone.png",
            ),
            ("  ", "names no file"),
        ] {
            let (text, error) = attach(&mut host, json!({"path": path}));
            assert!(error && text.starts_with("Not attached: "), "{text}");
            assert!(text.contains(reason), "{text}");
        }
        assert_eq!(
            host.attached,
            [(shot, Some("After: the dialog".to_owned()))]
        );
        // A relative path is absolute by the time it leaves this process.
        let relative = attachment("Cargo.toml", None).unwrap();
        assert!(relative.path().is_absolute() && relative.title().is_none());
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
            Answer::Spawned {
                agent: 12,
                with: String::new(),
            },
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
            Answer::Spawned {
                agent: 13,
                with: String::new(),
            },
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
            Answer::Spawned {
                agent: 14,
                with: String::new(),
            },
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
            Role::Spawned,
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
    fn a_lead_is_offered_what_a_lead_does_and_is_never_made_to_wait() {
        let names = |role| -> Vec<String> {
            tools(role)
                .iter()
                .map(|tool| tool["name"].as_str().unwrap().to_owned())
                .collect()
        };
        // What is listed is what is answered, for every role.
        for role in [Role::Agent, Role::Spawned, Role::Member, Role::Lead] {
            assert_eq!(names(role), offered(role), "{role:?}");
        }
        // A project's agent has what any started agent has, and reads and
        // adds to what its project keeps.
        let mut member = names(Role::Spawned);
        member.extend([READ.to_owned(), NOTE.to_owned()]);
        assert_eq!(names(Role::Member), member);
        for role in [Role::Agent, Role::Spawned] {
            for tool in [READ, NOTE, WRITE, DECIDE] {
                assert!(!offered(role).contains(&tool), "{role:?} {tool}");
            }
        }
        assert!(!offered(Role::Member).contains(&WRITE));
        assert!(!offered(Role::Member).contains(&DECIDE));
        assert_eq!(
            names(Role::Lead),
            [
                SPAWN, TELL, LIST, REPORT, CLOSE, REOPEN, READ, WRITE, DECIDE, WATCH, WATCHES,
                UNWATCH, PULL
            ]
        );
        // Watches are a lead's: no agent adds, lists or ends one.
        for role in [Role::Agent, Role::Spawned, Role::Member] {
            for tool in [WATCH, WATCHES, UNWATCH, PULL] {
                assert!(!offered(role).contains(&tool), "{role:?} {tool}");
            }
        }
        // What a project's agent reads and notes is allowed as it starts.
        let allowed: Vec<&str> = allowed_tools().collect();
        assert!(allowed.contains(&READ) && allowed.contains(&NOTE));
        assert!(!allowed.contains(&WRITE) && !allowed.contains(&DECIDE));
        let spawn = &tools(Role::Lead)[0]["inputSchema"];
        assert_eq!(spawn["required"], json!(["agent", "title", "prompt"]));
        let mut fields: Vec<&str> = spawn["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "agent", "cwd", "effort", "model", "prompt", "title", "ultra", "worktree"
            ]
        );
        // The lead is told when an agent gets a worktree of its own.
        assert!(
            instructions(Role::Lead).contains("a branch name as worktree")
                && spawn["properties"]["worktree"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("an agent that only reads needs none")
        );
        for tool in tools(Role::Lead) {
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert!(!tool["description"].as_str().unwrap().contains(WAIT));
        }
        let mut host = Fake::default();
        let hello = |host: &mut Fake, role| {
            respond(
                &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
                host,
                role,
                &mut NoRest,
            )
            .unwrap()["result"]["instructions"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let lead = hello(&mut host, Role::Lead);
        assert!(lead.contains(SPAWN) && lead.contains(REPORT));
        for absent in [WAIT, PRESS, LINK, ATTACH, REPLY] {
            assert!(!lead.contains(absent), "{absent}");
        }
        let member = hello(&mut host, Role::Member);
        assert!(member.contains(REPLY) && member.contains("lead of a Neptune project"));
        // What a lead is not offered it cannot call either.
        for tool in [WAIT, PRESS, LINK, ATTACH, REPLY] {
            let answer = respond(
                &call_message(1, tool, json!({})),
                &mut host,
                Role::Lead,
                &mut NoRest,
            )
            .unwrap();
            assert_eq!(answer["error"]["code"], -32602, "{tool}");
        }
        assert!(host.asked.is_empty());
        let answer = respond(
            &call_message(1, REPORT, json!({"agent_id": 1})),
            &mut host,
            Role::Agent,
            &mut NoRest,
        )
        .unwrap();
        assert_eq!(answer["error"]["code"], -32602);

        // Starting an agent returns once its terminal exists, and looks at
        // nothing after that: the application tells the lead the rest.
        let lead = |host: &mut Fake, tool, arguments| run_as(host, Role::Lead, tool, arguments);
        host.answers.extend([
            Answer::Queued { request: 4 },
            Answer::Pending,
            Answer::Spawned {
                agent: 12,
                with: String::new(),
            },
        ]);
        let task = json!({"agent": "codex", "title": " api ", "prompt": "Build the API"});
        assert_eq!(
            lead(&mut host, SPAWN, task),
            (
                "Agent 12 started in the project's directory. You will be told when it finishes or needs someone."
                    .into(),
                false
            )
        );
        assert!(host.answers.is_empty());
        assert!(matches!(
            &host.asked[..],
            [
                Event::Spawn { kind: AgentKind::Codex, prompt, cwd, ultra: false, title: Some(title), .. },
                Event::SpawnResult { request: 4 },
                Event::SpawnResult { request: 4 },
            ] if prompt == "Build the API" && cwd.as_os_str().is_empty() && title == "api"
        ));
        let directory = tempfile::tempdir().unwrap();
        host.answers.extend([
            Answer::Queued { request: 5 },
            Answer::Spawned {
                agent: 13,
                // What Neptune started it with, which the user's settings
                // for the project decide before what was asked here.
                with: " on sonnet at high effort with ultracode on".into(),
            },
        ]);
        let (said, failed) = lead(
            &mut host,
            SPAWN,
            json!({"agent": "claude", "title": "docs", "prompt": "x", "cwd": directory.path(), "model": " opus ", "effort": "high", "ultra": true}),
        );
        assert!(!failed, "{said}");
        assert_eq!(
            said,
            format!(
                "Agent 13 started on sonnet at high effort with ultracode on in {}. You will be told when it finishes or needs someone.",
                directory.path().display()
            )
        );
        assert!(host.asked.iter().any(
            |event| matches!(event, Event::Spawn { model: Some(model), effort: Some(effort), ultra: true, cwd, .. } if model == "opus" && effort == "high" && cwd == directory.path())
        ));
        // A branch asks for a worktree, which Neptune makes: the call says
        // where the agent works, and waits longer for git than for a terminal.
        host.asked.clear();
        host.answers.extend([
            Answer::Queued { request: 7 },
            Answer::Spawned {
                agent: 15,
                with: String::new(),
            },
        ]);
        let (said, failed) = lead(
            &mut host,
            SPAWN,
            json!({"agent": "codex", "title": "login", "prompt": "x", "worktree": " fix-login "}),
        );
        assert_eq!(
            (said.as_str(), failed),
            (
                "Agent 15 started in a worktree of its own, on branch fix-login. You will be \
                 told when it finishes or needs someone.",
                false
            )
        );
        assert!(host.asked.iter().any(|event| matches!(
            event,
            Event::Spawn { worktree: Some(branch), cwd, .. }
                if branch == "fix-login" && cwd.as_os_str().is_empty()
        )));
        assert!(WORKTREE_WAIT > SPAWN_WAIT);
        // Nothing unusable reaches the application.
        for arguments in [
            json!({"agent": "codex", "prompt": "x"}),
            json!({"agent": "codex", "title": "t".repeat(MAX_TITLE + 1), "prompt": "x"}),
            json!({"agent": "codex", "title": "two\nlines", "prompt": "x"}),
            json!({"agent": "gemini", "title": "t", "prompt": "x"}),
            json!({"agent": "codex", "title": "t", "prompt": "x".repeat(LEAD_PROMPT + 1)}),
            json!({"agent": "codex", "title": "t", "prompt": "x", "cwd": "relative"}),
            json!({"agent": "codex", "title": "t", "prompt": "x", "cwd": directory.path().join("missing")}),
        ] {
            let asked = host.asked.len();
            assert!(lead(&mut host, SPAWN, arguments).1);
            assert_eq!(host.asked.len(), asked);
        }
        // What the application refuses is said as it said it.
        host.answers.push_back(Answer::Refused {
            reason: "This project is paused.".into(),
        });
        assert_eq!(
            lead(
                &mut host,
                SPAWN,
                json!({"agent": "codex", "title": "t", "prompt": "x"})
            ),
            ("This project is paused.".into(), true)
        );

        host.asked.clear();
        host.answers.extend([
            Answer::Queued { request: 6 },
            Answer::Spawned {
                agent: 14,
                with: String::new(),
            },
        ]);
        let (said, failed) = lead(
            &mut host,
            REOPEN,
            json!({"agent_id": 13, "message": "One more thing"}),
        );
        assert!(
            !failed && said.starts_with("Reopened agent 13") && said.contains("agent 14"),
            "{said}"
        );
        host.answers.push_back(Answer::Told { working: false });
        let (said, failed) = lead(
            &mut host,
            TELL,
            json!({"agent_id": 12, "message": "Also add tests"}),
        );
        assert_eq!(
            (said.as_str(), failed),
            ("Sent to agent 12. You will be told when it replies.", false)
        );
        host.answers.push_back(Answer::Agents {
            agents: vec![
                AgentReport {
                    title: Some("api".into()),
                    ..report(12, Status::Working, true, &[])
                },
                AgentReport {
                    reopens: true,
                    ..report(
                        13,
                        Status::Ended {
                            reason: "its terminal was closed".into(),
                        },
                        false,
                        &[],
                    )
                },
            ],
        });
        assert_eq!(
            lead(&mut host, LIST, json!({})).0,
            "Agent 12 (Codex) \"api\": working\nAgent 13 (Codex): ended: its terminal was closed; reopen_agent opens it again"
        );
        host.answers.push_back(Answer::Done);
        // Closing says what may stay: a terminal the user opened is theirs.
        let (said, failed) = lead(&mut host, CLOSE, json!({"agent_id": 12}));
        assert!(
            !failed && said.contains("unless the user had opened it as a tab"),
            "{said}"
        );
        // None of it looked at an agent to wait for it or took its replies.
        assert!(!host.asked.iter().any(|event| matches!(
            event,
            Event::Collect { agent: Some(_), .. } | Event::Collect { take: true, .. }
        )));

        // A whole reply is asked of the application and read once answered.
        host.asked.clear();
        host.answers.extend([
            Answer::Queued { request: 9 },
            Answer::Pending,
            Answer::Text {
                text: "Done: 3 endpoints.".into(),
            },
        ]);
        assert_eq!(
            lead(&mut host, REPORT, json!({"agent_id": 12})),
            ("Done: 3 endpoints.".into(), false)
        );
        assert!(matches!(
            &host.asked[..],
            [
                Event::Project {
                    call: ProjectCall::Report { agent: 12 }
                },
                Event::CallResult { request: 9 },
                Event::CallResult { request: 9 },
            ]
        ));
        assert!(lead(&mut host, REPORT, json!({})).1);
        host.answers.extend([
            Answer::Queued { request: 10 },
            Answer::Refused {
                reason: "Agent 12 has not ended a turn with a reply yet.".into(),
            },
        ]);
        assert_eq!(
            lead(&mut host, REPORT, json!({"agent_id": 12})),
            (
                "Agent 12 has not ended a turn with a reply yet.".into(),
                true
            )
        );
    }

    #[test]
    fn a_lead_adds_lists_and_ends_watches_and_asks_where_a_pull_request_stands() {
        let mut host = Fake::default();
        let text = |text: &str| Answer::Text { text: text.into() };
        let ask = |host: &mut Fake, tool, arguments, answer: Answer| {
            host.asked.clear();
            host.answers
                .extend([Answer::Queued { request: 5 }, Answer::Pending, answer]);
            let said = run_as(host, Role::Lead, tool, arguments);
            assert!(host.answers.is_empty(), "{tool} left an answer unread");
            let call = match host.asked.first() {
                Some(Event::Project { call }) => call.clone(),
                other => panic!("{tool} asked {other:?}"),
            };
            (said, call)
        };
        // A schedule goes to the application as asked; what comes back says
        // that the user has yet to allow it.
        let proposed = "Watch 3 \"Nightly\" is proposed — the user must allow it.";
        assert_eq!(
            ask(
                &mut host,
                WATCH,
                json!({"title": " Nightly ", "trigger": {"schedule": {"every_minutes": 60}},
                       "instruction": " Check the build. "}),
                text(proposed),
            ),
            (
                (proposed.into(), false),
                ProjectCall::AddSubscription {
                    title: "Nightly".into(),
                    trigger: WatchTrigger::Schedule { every_minutes: 60 },
                    instruction: "Check the build.".into(),
                }
            )
        );
        for (trigger, link) in [
            (json!({"pull_request": "linked"}), "linked"),
            (
                json!({"pull_request": " https://github.com/a/b/pull/7 "}),
                "https://github.com/a/b/pull/7",
            ),
        ] {
            let (_, call) = ask(
                &mut host,
                WATCH,
                json!({"title": "PR", "trigger": trigger}),
                text("ok"),
            );
            assert_eq!(
                call,
                ProjectCall::AddSubscription {
                    title: "PR".into(),
                    trigger: WatchTrigger::PullRequest(link.into()),
                    instruction: String::new(),
                }
            );
        }
        assert_eq!(
            ask(&mut host, WATCHES, json!({}), text("Watch 3 …")),
            (("Watch 3 …".into(), false), ProjectCall::ListSubscriptions)
        );
        assert_eq!(
            ask(&mut host, UNWATCH, json!({"id": 3}), text("Removed.")).1,
            ProjectCall::RemoveSubscription { id: 3 }
        );
        assert_eq!(
            ask(&mut host, PULL, json!({}), text("checking")).1,
            ProjectCall::PullRequestStatus { url: None }
        );
        assert_eq!(
            ask(
                &mut host,
                PULL,
                json!({"url": "https://github.com/a/b/pull/7"}),
                text("open")
            )
            .1,
            ProjectCall::PullRequestStatus {
                url: Some("https://github.com/a/b/pull/7".into())
            }
        );
        // A refusal of the application is the tool's error, in its words.
        host.asked.clear();
        host.answers.extend([
            Answer::Queued { request: 6 },
            Answer::Refused {
                reason: "This project is paused.".into(),
            },
        ]);
        assert_eq!(
            run_as(
                &mut host,
                Role::Lead,
                WATCH,
                json!({"title": "t", "trigger": {"schedule": {"every_minutes": 15}}, "instruction": "x"}),
            ),
            ("This project is paused.".into(), true)
        );
        // What is not a watch is refused before the application is asked.
        host.asked.clear();
        for (tool, arguments, why) in [
            (
                WATCH,
                json!({"trigger": {"schedule": {"every_minutes": 60}}}),
                "title is empty",
            ),
            (WATCH, json!({"title": "t"}), "trigger is"),
            (WATCH, json!({"title": "t", "trigger": {}}), "trigger is"),
            (
                WATCH,
                json!({"title": "t", "trigger": {"pull_request": ""}}),
                "trigger is",
            ),
            (
                WATCH,
                json!({"title": "t", "trigger": {"webhook": "https://x"}}),
                "trigger is",
            ),
            (
                WATCH,
                json!({"title": "t", "trigger": {"schedule": {"every_minutes": 60}, "pull_request": "linked"}}),
                "trigger is",
            ),
            (
                WATCH,
                json!({"title": "t", "trigger": {"schedule": {"daily_at": "09:00"}}}),
                "not at a time of day",
            ),
            (
                WATCH,
                json!({"title": "t", "trigger": {"schedule": {"every_minutes": "often"}}}),
                "whole number of minutes",
            ),
            (
                WATCH,
                json!({"title": "t", "trigger": {"schedule": {"every_minutes": 60}},
                       "instruction": "i".repeat(MAX_INSTRUCTION + 1)}),
                "longer than 2 KB",
            ),
            (UNWATCH, json!({}), "id is the number"),
            (UNWATCH, json!({"id": "three"}), "id is the number"),
            (PULL, json!({"url": 7}), "url is the address"),
        ] {
            let (said, failed) = run_as(&mut host, Role::Lead, tool, arguments);
            assert!(failed && said.contains(why), "{tool}: {said}");
        }
        assert!(host.asked.is_empty());
        // No agent is offered these, and one that calls one anyway reaches
        // nothing.
        for role in [Role::Agent, Role::Spawned, Role::Member] {
            let answer = respond(
                &call_message(1, WATCHES, json!({})),
                &mut host,
                role,
                &mut NoRest,
            )
            .unwrap();
            assert!(answer.get("result").is_none(), "{role:?}: {answer}");
        }
        assert!(host.asked.is_empty());
    }

    #[test]
    fn what_a_project_keeps_is_read_and_added_to_through_the_application() {
        let mut host = Fake::default();
        let text = |text: &str| Answer::Text { text: text.into() };
        let ask = |host: &mut Fake, role, tool, arguments, answers: Vec<Answer>| {
            host.asked.clear();
            host.answers.extend(answers);
            let said = run_as(host, role, tool, arguments);
            assert!(host.answers.is_empty(), "{tool} left an answer unread");
            let call = match host.asked.first() {
                Some(Event::Project { call }) => Some(call.clone()),
                _ => None,
            };
            // Whatever was asked is waited for under the number it was given.
            assert!(
                host.asked
                    .iter()
                    .skip(1)
                    .all(|event| matches!(event, Event::CallResult { request: 3 }))
            );
            (said, call)
        };
        let queued = |answer: Answer| vec![Answer::Queued { request: 3 }, Answer::Pending, answer];

        // The lead reads the index, then a file from where an answer said.
        let (said, call) = ask(
            &mut host,
            Role::Lead,
            READ,
            json!({}),
            queued(text("# Context index")),
        );
        assert_eq!(said, ("# Context index".into(), false));
        assert_eq!(
            call,
            Some(ProjectCall::ReadContext {
                path: None,
                offset: 0
            })
        );
        let (_, call) = ask(
            &mut host,
            Role::Lead,
            READ,
            json!({"path": " notes/auth.md ", "offset": 20480}),
            queued(text("…")),
        );
        assert_eq!(
            call,
            Some(ProjectCall::ReadContext {
                path: Some("notes/auth.md".into()),
                offset: 20480
            })
        );
        // It keeps its status and records what was approved.
        let (said, call) = ask(
            &mut host,
            Role::Lead,
            WRITE,
            json!({"path": "STATUS.md", "content": "Goal: split checkout.", "append": true}),
            queued(text("Added to STATUS.md.")),
        );
        assert_eq!(said, ("Added to STATUS.md.".into(), false));
        assert_eq!(
            call,
            Some(ProjectCall::WriteContext {
                path: "STATUS.md".into(),
                content: "Goal: split checkout.".into(),
                append: Some(true),
            })
        );
        let (said, call) = ask(
            &mut host,
            Role::Lead,
            DECIDE,
            json!({"decision": "Use JWT.", "why": " "}),
            queued(text("Recorded in DECISIONS.md.")),
        );
        assert_eq!(said, ("Recorded in DECISIONS.md.".into(), false));
        assert_eq!(
            call,
            Some(ProjectCall::RecordDecision {
                decision: "Use JWT.".into(),
                why: None,
            })
        );
        // What the application refuses is said as it said it.
        let (said, _) = ask(
            &mut host,
            Role::Lead,
            WRITE,
            json!({"path": "DECISIONS.md", "content": "x"}),
            vec![
                Answer::Queued { request: 3 },
                Answer::Refused {
                    reason: "DECISIONS.md is only added to, with record_decision.".into(),
                },
            ],
        );
        assert_eq!(
            said,
            (
                "DECISIONS.md is only added to, with record_decision.".into(),
                true
            )
        );

        // One of its agents reads the same, and adds to the notes.
        let (said, call) = ask(
            &mut host,
            Role::Member,
            NOTE,
            json!({"topic": "auth", "content": "Tokens expire after an hour."}),
            queued(text("Added to notes/auth.md.")),
        );
        assert_eq!(said, ("Added to notes/auth.md.".into(), false));
        assert_eq!(
            call,
            Some(ProjectCall::AddNote {
                topic: "auth".into(),
                content: "Tokens expire after an hour.".into(),
            })
        );
        let (said, call) = ask(
            &mut host,
            Role::Member,
            READ,
            json!({"path": "STATUS.md"}),
            queued(text("Goal")),
        );
        assert_eq!(said, ("Goal".into(), false));
        assert!(matches!(call, Some(ProjectCall::ReadContext { .. })));

        // Nothing unusable reaches the application.
        for (role, tool, arguments) in [
            (Role::Lead, READ, json!({"offset": -1})),
            (Role::Lead, READ, json!({"offset": "far"})),
            (Role::Lead, WRITE, json!({"path": "STATUS.md"})),
            (Role::Lead, WRITE, json!({"content": "x"})),
            (
                Role::Lead,
                WRITE,
                json!({"path": "STATUS.md", "content": "x".repeat(MAX_WRITE + 1)}),
            ),
            (
                Role::Lead,
                WRITE,
                json!({"path": "STATUS.md", "content": "x", "append": "yes"}),
            ),
            (Role::Lead, DECIDE, json!({"decision": " "})),
            (
                Role::Lead,
                DECIDE,
                json!({"decision": "x".repeat(MAX_DECISION + 1)}),
            ),
            (
                Role::Lead,
                DECIDE,
                json!({"decision": "x", "why": "y".repeat(MAX_DECISION + 1)}),
            ),
            (Role::Member, NOTE, json!({"topic": "auth"})),
            (Role::Member, NOTE, json!({"content": "x"})),
            (
                Role::Member,
                NOTE,
                json!({"topic": "auth", "content": "x".repeat(MAX_NOTE + 1)}),
            ),
        ] {
            let (said, call) = ask(&mut host, role, tool, arguments.clone(), Vec::new());
            assert!(said.1 && call.is_none(), "{tool} {arguments}: {said:?}");
        }
        // Only the lead writes the status and records decisions, and nobody
        // else reads or notes at all: the tools are not theirs to call.
        for (role, tool) in [
            (Role::Member, WRITE),
            (Role::Member, DECIDE),
            (Role::Lead, NOTE),
            (Role::Spawned, READ),
            (Role::Spawned, NOTE),
            (Role::Agent, READ),
            (Role::Agent, NOTE),
        ] {
            host.asked.clear();
            let answer = respond(
                &call_message(1, tool, json!({"topic": "t", "content": "c"})),
                &mut host,
                role,
                &mut NoRest,
            )
            .unwrap();
            assert_eq!(answer["error"]["code"], -32602, "{role:?} {tool}");
            assert!(host.asked.is_empty());
        }
        let hello = |role| {
            respond(
                &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
                &mut Fake::default(),
                role,
                &mut NoRest,
            )
            .unwrap()["result"]["instructions"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        for tool in [READ, WRITE, DECIDE] {
            assert!(hello(Role::Lead).contains(tool), "{tool}");
        }
        assert!(hello(Role::Member).contains(READ) && hello(Role::Member).contains(NOTE));
        assert!(hello(Role::Member).contains("unverified"));
        assert!(!hello(Role::Spawned).contains(READ) && !hello(Role::Agent).contains(NOTE));
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
            Role::Agent,
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
        assert_eq!(respond(&message, &mut host, Role::Agent, &mut inbox), None);
        assert!(inbox.closed);
    }
}
