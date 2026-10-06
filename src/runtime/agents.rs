//! Scoped CLI adapters and a bounded, metadata-only local hook bridge.
//! All setup and socket/file I/O runs on startup workers or the bridge worker.
use super::agent_remote as remote;
use crate::agent_activity::{Activity, Attention};
use neptune_model::{AgentKind, AgentSession, Attachment, PaneId, PullRequest};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    collections::VecDeque,
    io::{IsTerminal, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use terminal_core::SessionOptions;

type Wake = Arc<dyn Fn() + Send + Sync>;
/// Room for one task, message or reply between agents, escaped.
const MAX_MESSAGE: u64 = 256 * 1024;
/// The longest task, message or reply one agent hands another.
pub(super) const MAX_TEXT: usize = 32 * 1024;
/// Every unread reply of every agent one agent started, escaped.
const MAX_ANSWER: u64 = 32 * 1024 * 1024;
/// Replies an agent has not collected; the oldest give way.
const MAX_OUTBOX: usize = 16;
/// Requests waiting for the next frame.
const MAX_REQUESTS: usize = 64;
/// Keys one call presses for a question a CLI asks before taking its task.
const MAX_KEYS: usize = 8;
/// A turn that follows a handed-over prompt starts within this long of the
/// one before it ending, or was folded into it.
const TURN_GRACE: Duration = Duration::from_secs(4);
/// A CLI that has not taken its task by now is showing a prompt of its own,
/// such as a folder trust question, which only a person answers.
const START_GRACE: Duration = Duration::from_secs(20);
/// A request the agent's own reviewer answers at once is not a wait.
const ASK_HOLD: Duration = Duration::from_millis(1500);
/// Hook input carries tool results; only its few metadata fields are read.
const MAX_HOOK: u64 = 4 * 1024 * 1024;
/// A tool name longer than this is not carried.
const MAX_TOOL: usize = 128;
/// Links waiting for the next frame; the model keeps the most recent per pane.
const MAX_LINKS: usize = 64;
const ENDPOINT: &str = "NEPTUNE_AGENT_ENDPOINT";
const TOKEN: &str = "NEPTUNE_AGENT_TOKEN";
const SHIMS: &str = "NEPTUNE_AGENT_SHIMS";
const RUN: &str = "NEPTUNE_AGENT_RUN";
/// This executable, for hook commands that must read the same at every launch.
const HELPER: &str = "NEPTUNE_AGENT_HELPER";
/// Set for a CLI another agent started: its replies go to that agent.
const SPAWNED: &str = "NEPTUNE_AGENT_SPAWNED";
/// What Neptune's plugin for a CLI runs at each moment, followed by its name.
const HOOK: &str = "NEPTUNE_AGENT_HOOK";
/// Where the plugins are, under the adapters' directory.
const PLUGINS: &str = "plugins";
pub(super) const OPENCODE_PLUGIN: &str = include_str!("agent-opencode.js");
pub(super) const PI_EXTENSION: &str = include_str!("agent-pi.js");
const NOT_TRACKED: &str = "Neptune is not tracking an agent in this terminal.";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Event {
    Open {
        agent: AgentSession,
    },
    Session {
        agent: AgentSession,
    },
    Close,
    PullRequest {
        url: String,
    },
    /// A file for the user to look at, by its absolute path.
    Attachment {
        path: PathBuf,
        #[serde(default)]
        title: Option<String>,
    },
    Activity {
        signal: Signal,
    },
    /// The CLI is gone but its reference is kept, as after a failed resume.
    Stopped,
    /// The CLI started Neptune's tool server.
    Serving,
    /// Start another agent with a task, in a tab beside this one.
    Spawn {
        kind: AgentKind,
        prompt: String,
        cwd: PathBuf,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        effort: Option<String>,
        /// Claude Code's ultracode, or Codex's Ultra effort.
        #[serde(default)]
        ultra: bool,
    },
    /// Open a started agent whose terminal closed again, in a terminal of
    /// its own, to go on with its conversation.
    Reopen {
        agent: u64,
        text: String,
    },
    /// Answer the question a started agent's CLI asks before it takes its
    /// task, as the user said to.
    Press {
        agent: u64,
        keys: Vec<String>,
    },
    /// What became of a `Spawn` or a `Reopen`.
    SpawnResult {
        request: u64,
    },
    /// What the agents this one started are doing, with the replies it has
    /// not read when `take` is set.
    Collect {
        agent: Option<u64>,
        take: bool,
    },
    /// Hand a started agent another prompt.
    Tell {
        agent: u64,
        text: String,
    },
    /// Close a started agent's terminal.
    Dismiss {
        agent: u64,
    },
    /// A started terminal asks for the CLI and task it opens with.
    Launch,
    LaunchFailed {
        reason: String,
    },
    /// A started agent's words for the agent that started it; `done` at the
    /// end of a turn.
    Report {
        text: String,
        done: bool,
    },
}
impl Event {
    /// Answered with an `Answer` instead of an acknowledgement.
    fn is_request(&self) -> bool {
        matches!(
            self,
            Self::Spawn { .. }
                | Self::Reopen { .. }
                | Self::Press { .. }
                | Self::SpawnResult { .. }
                | Self::Collect { .. }
                | Self::Tell { .. }
                | Self::Dismiss { .. }
                | Self::Launch
                | Self::LaunchFailed { .. }
        )
    }
}
/// What Neptune answers a request with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub(super) enum Answer {
    Refused {
        reason: String,
    },
    Done,
    /// The message was handed over; the agent was still at earlier work.
    Told {
        working: bool,
    },
    Queued {
        request: u64,
    },
    Pending,
    Spawned {
        agent: u64,
    },
    Launch {
        kind: AgentKind,
        prompt: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        effort: Option<String>,
        #[serde(default)]
        ultracode: bool,
        /// The conversation of its own that it goes on with.
        #[serde(default)]
        resume: Option<String>,
    },
    Agents {
        agents: Vec<AgentReport>,
    },
}
/// One started agent, as the agent that started it sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct AgentReport {
    pub agent: u64,
    pub kind: AgentKind,
    pub status: Status,
    /// It has replies or a state its starter has not been told.
    pub news: bool,
    pub replies: Vec<String>,
    /// It ended, and its conversation can be opened again.
    #[serde(default)]
    pub reopens: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Status {
    /// Its CLI has not taken its first prompt.
    Starting,
    /// Its CLI stands at a question of its own before taking its task, such
    /// as whether to trust a folder; `screen` is what its terminal shows.
    Asking {
        screen: String,
    },
    Working,
    Idle,
    Waiting {
        attention: Attention,
    },
    Ended {
        reason: String,
    },
}
impl Status {
    pub(super) fn settled(&self) -> bool {
        !matches!(self, Self::Starting | Self::Working)
    }
}
/// What the terminal opened for a started agent begins with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub kind: AgentKind,
    pub prompt: String,
    /// The model its CLI is asked for; the CLI's own choice without one.
    pub model: Option<String>,
    /// How hard its model is asked to think, as its CLI names the level.
    pub effort: Option<String>,
    /// Claude Code runs with ultracode on. Codex's Ultra is an effort.
    pub ultracode: bool,
    /// The closed terminal it ran in before and the conversation it had
    /// there, which it goes on with.
    pub resume: Option<(PaneId, String)>,
}
impl Task {
    #[cfg(test)]
    pub fn new(kind: AgentKind, prompt: &str) -> Self {
        Self {
            kind,
            prompt: prompt.into(),
            model: None,
            effort: None,
            ultracode: false,
            resume: None,
        }
    }
}
/// A key pressed for a question a CLI asks before it takes its task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Enter,
    Escape,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Char(char),
}
impl Press {
    fn parse(key: &str) -> Option<Self> {
        Some(match key.trim().to_ascii_lowercase().as_str() {
            "enter" | "return" => Self::Enter,
            "escape" | "esc" => Self::Escape,
            "tab" => Self::Tab,
            "up" => Self::Up,
            "down" => Self::Down,
            "left" => Self::Left,
            "right" => Self::Right,
            "space" => Self::Char(' '),
            _ => {
                let mut letters = key.trim().chars();
                let letter = letters.next().filter(char::is_ascii_alphanumeric)?;
                if letters.next().is_some() {
                    return None;
                }
                Self::Char(letter)
            }
        })
    }
}
/// What an agent asked of the application, carried out on the next frame.
#[derive(Debug, PartialEq, Eq)]
pub enum AgentRequest {
    Spawn {
        request: u64,
        parent: PaneId,
        generation: u64,
        task: Task,
        cwd: PathBuf,
    },
    /// Keys for the question the CLI in `pane` asks before taking its task.
    Press {
        pane: PaneId,
        generation: u64,
        keys: Vec<Press>,
    },
    /// A prompt for the agent in `pane`, typed for it.
    Tell {
        pane: PaneId,
        generation: u64,
        text: String,
    },
    Close {
        pane: PaneId,
        generation: u64,
        parent: PaneId,
    },
}
/// An agent another agent started, and what passes between the two. Held
/// in memory only: tasks and replies are never saved or logged.
struct Child {
    parent: PaneId,
    kind: AgentKind,
    /// The task its CLI opens with, until its terminal takes it.
    launch: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    ultracode: bool,
    /// The conversation its CLI goes on with, taken along with the task.
    resume: Option<String>,
    /// Its conversation as its CLI last named it, to open it again.
    session: Option<AgentSession>,
    /// What its terminal showed once it stood still before taking its task.
    asking: Option<String>,
    /// Replies its starter has not collected.
    outbox: VecDeque<String>,
    /// Prompts handed over whose turns have not ended.
    owed: u32,
    /// When a prompt was last handed over or a turn last ended.
    marked: Instant,
    /// Its first turn has begun.
    started: bool,
    ended: Option<String>,
    /// The settled state its starter was last told.
    seen: Option<Status>,
}
impl Child {
    fn end(&mut self, reason: &str) {
        if self.ended.is_none() {
            self.ended = Some(reason.into());
            self.launch = None;
        }
    }
    fn task(parent: PaneId, task: Task) -> Self {
        Self {
            parent,
            kind: task.kind,
            launch: Some(task.prompt),
            model: task.model,
            effort: task.effort,
            ultracode: task.ultracode,
            resume: task.resume.map(|resume| resume.1),
            session: None,
            asking: None,
            outbox: VecDeque::new(),
            owed: 1,
            marked: Instant::now(),
            started: false,
            ended: None,
            seen: None,
        }
    }
    /// Its turn began: whatever it asked before taking its task is answered.
    fn start(&mut self) {
        self.started = true;
        self.asking = None;
    }
    /// It ended, and its CLI named a conversation that can be opened again.
    fn reopens(&self) -> bool {
        self.ended.is_some()
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.session_id.is_some())
    }
    fn status(&self, slot: Option<&Slot>, now: Instant) -> Status {
        if let Some(reason) = &self.ended {
            return Status::Ended {
                reason: reason.clone(),
            };
        }
        let Some(slot) = slot else {
            // Its terminal is still being opened while its task waits.
            if self.launch.is_some() {
                return Status::Starting;
            }
            return Status::Ended {
                reason: "its terminal was closed".into(),
            };
        };
        let Some(state) = slot.state().filter(|_| self.started) else {
            return match &self.asking {
                Some(screen) if !self.started => Status::Asking {
                    screen: screen.clone(),
                },
                _ => Status::Starting,
            };
        };
        match state {
            Activity::Working => Status::Working,
            Activity::NeedsInput(attention)
                if now.saturating_duration_since(slot.since) >= ASK_HOLD =>
            {
                Status::Waiting { attention }
            }
            Activity::NeedsInput(_) => Status::Working,
            // The turn of a prompt just handed over has yet to begin.
            Activity::Idle
                if self.owed > 0
                    && now.saturating_duration_since(self.marked.max(slot.since)) < TURN_GRACE =>
            {
                Status::Working
            }
            Activity::Idle => Status::Idle,
        }
    }
}
pub(super) fn label(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude Code",
        AgentKind::Codex => "Codex",
        AgentKind::Opencode => "OpenCode",
        AgentKind::Gemini => "Gemini CLI",
        AgentKind::Pi => "pi",
        AgentKind::Omp => "Oh My Pi",
    }
}
/// A model name as a CLI takes one: a short word, never an option.
fn model_name(value: &str) -> Option<&str> {
    let value = value.trim();
    (value.len() <= 80
        && value.starts_with(|first: char| first.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || "._:/-[]@".contains(letter)))
    .then_some(value)
}
/// The effort a CLI is started with and whether Claude Code runs with
/// ultracode, from what was asked. Each CLI has its own levels, and "ultra"
/// means ultracode for Claude Code and the Ultra effort for Codex.
fn effort_level(
    kind: AgentKind,
    effort: Option<&str>,
    ultra: bool,
) -> Result<(Option<String>, bool), String> {
    let levels: &[&str] = match kind {
        AgentKind::Claude => &["low", "medium", "high", "xhigh", "max"],
        AgentKind::Codex => &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"],
        // Neptune has no way to hand these a task and read their answer.
        AgentKind::Opencode | AgentKind::Gemini | AgentKind::Pi | AgentKind::Omp => {
            return Err(format!(
                "Neptune starts Claude Code and Codex for another agent, not {}.",
                label(kind)
            ));
        }
    };
    let effort = effort
        .map(|effort| effort.trim().to_ascii_lowercase())
        .filter(|effort| !effort.is_empty());
    if let Some(effort) = &effort
        && !levels.contains(&effort.as_str())
    {
        return Err(format!(
            "effort for {} is one of: {}.{}",
            label(kind),
            levels.join(", "),
            if kind == AgentKind::Claude && effort == "ultra" {
                " For ultracode, set ultra to true."
            } else {
                ""
            }
        ));
    }
    Ok(match kind {
        AgentKind::Claude => (effort, ultra),
        AgentKind::Codex if ultra => {
            if effort.as_deref().is_some_and(|effort| effort != "ultra") {
                return Err(
                    "Ultra is Codex's highest effort: give ultra or another effort, not both."
                        .into(),
                );
            }
            (Some("ultra".into()), false)
        }
        _ => (effort, false),
    })
}
/// Keeps the start of text longer than one agent hands another.
fn clip(text: &str) -> String {
    if text.len() <= MAX_TEXT {
        return text.to_owned();
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[Neptune cut this short: it was longer than {} KB.]",
        &text[..end],
        MAX_TEXT / 1024
    )
}
/// What a hook says about the agent's turn. It names the kind of moment and
/// at most a tool; prompts, commands and results never leave the hook process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Signal {
    /// A prompt was submitted.
    Prompt,
    /// A tool is about to run.
    Tool,
    /// A person is asked to allow a tool, answer a question or approve a plan.
    Ask {
        attention: Attention,
        tool: Option<String>,
        subagent: bool,
    },
    ToolDone {
        tool: Option<String>,
        subagent: bool,
    },
    /// A request that names no tool was answered.
    Answered,
    /// The turn ended or was interrupted.
    Done,
}
/// An open agent's activity, and what resolves a request it is waiting on.
struct Tracked {
    state: Activity,
    /// The tool whose completion answers the request.
    tool: Option<String>,
    /// A subagent made the request; its own tool finishing answers it.
    asked_by_subagent: bool,
    /// What a subagent's request interrupted.
    before: Activity,
}
impl Tracked {
    fn idle() -> Self {
        Self {
            state: Activity::Idle,
            tool: None,
            asked_by_subagent: false,
            before: Activity::Idle,
        }
    }
    /// Whether the signal is worth a frame: a change, or a turn boundary that
    /// confirms a state the title may have overruled.
    fn apply(&mut self, signal: Signal) -> bool {
        let waiting = matches!(self.state, Activity::NeedsInput(_));
        let (next, confirmed) = match signal {
            Signal::Prompt => (Activity::Working, true),
            Signal::Done => (Activity::Idle, true),
            // Tools of one batch start together; the request stands until
            // its own tool finishes.
            Signal::Tool if waiting => return false,
            Signal::Tool => (Activity::Working, false),
            // A late notice of a request that is already shown, or was answered.
            Signal::Ask { tool: None, .. } if waiting => return false,
            Signal::Ask {
                attention,
                tool,
                subagent,
            } => {
                if !waiting {
                    self.before = self.state;
                }
                self.tool = tool;
                self.asked_by_subagent = subagent;
                self.state = Activity::NeedsInput(attention);
                return true;
            }
            Signal::ToolDone { tool, subagent } if waiting => {
                // Another tool finishing, or the same tool run by someone
                // else, leaves the request standing.
                if self.tool.is_some()
                    && (subagent != self.asked_by_subagent || (tool.is_some() && self.tool != tool))
                {
                    return false;
                }
                let next = if subagent {
                    self.before
                } else {
                    Activity::Working
                };
                (next, false)
            }
            Signal::ToolDone { subagent: true, .. } => return false,
            Signal::ToolDone { .. } => (Activity::Working, false),
            Signal::Answered if waiting => (Activity::Working, false),
            Signal::Answered => return false,
        };
        let changed = self.state != next;
        self.state = next;
        self.tool = None;
        changed || confirmed
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    token: String,
    run: String,
    event: Event,
}
struct Slot {
    generation: u64,
    token: String,
    run: Option<String>,
    /// The terminal is an SSH connection: its agents run on the host and
    /// report through the terminal itself, never through the listener.
    remote: bool,
    /// The CLI of the open invocation.
    kind: Option<AgentKind>,
    /// The conversation the open invocation is in, as it last named it.
    conversation: Option<AgentSession>,
    /// A prompt was sent in that conversation.
    prompted: bool,
    /// Set while an invocation is open.
    activity: Option<Tracked>,
    /// What the application shows once the terminal's title has corrected
    /// the hooks; a later hook is news again.
    shown: Option<Activity>,
    /// Since when the agent has been in its present state.
    since: Instant,
    wake: Wake,
    _startup: Option<tempfile::TempDir>,
}
impl Slot {
    /// What the open agent is doing, as far as hooks and title say.
    fn state(&self) -> Option<Activity> {
        let reported = self.activity.as_ref()?.state;
        self.run.as_ref().map(|_| self.shown.unwrap_or(reported))
    }
    fn show(&mut self, shown: Option<Activity>, now: Instant) {
        let before = self.state();
        self.shown = shown;
        if self.state() != before {
            self.since = now;
        }
    }
}
#[derive(Default)]
struct Shared {
    slots: BTreeMap<PaneId, Slot>,
    /// The latest reference per pane since the last frame, and whether an
    /// agent left the pane before it: one that leaves and one that opens
    /// between two frames are two changes.
    changes: BTreeMap<PaneId, (u64, Option<AgentSession>, bool)>,
    /// Agents started by agents, by the pane each runs in.
    children: BTreeMap<PaneId, Child>,
    requests: Vec<AgentRequest>,
    /// Spawn requests and what became of each once the application answered.
    /// Each is answered only to the pane that asked.
    spawns: BTreeMap<u64, (PaneId, Option<Result<PaneId, String>>)>,
    next_request: u64,
    /// The latest activity per pane since the last frame; `None` once closed.
    activity: BTreeMap<PaneId, (u64, Option<Activity>)>,
    links: Vec<(PaneId, u64, PullRequest)>,
    attachments: Vec<(PaneId, u64, Attachment)>,
    retired: Vec<tempfile::TempDir>,
}
struct Listener {
    directory: tempfile::TempDir,
    address: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.address);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[derive(Default)]
pub struct AgentBridge {
    listener: Mutex<Option<Listener>>,
    shared: Arc<Mutex<Shared>>,
}
impl AgentBridge {
    /// Called only by a startup worker. Tests and remote sessions opt out.
    pub fn prepare(
        &self,
        pane: PaneId,
        generation: u64,
        options: &mut SessionOptions,
        resume: Option<&AgentSession>,
        spawned_by: Option<PaneId>,
        wake: Wake,
    ) -> std::io::Result<()> {
        #[cfg(not(unix))]
        {
            let _ = (pane, generation, options, resume, spawned_by, wake);
            return Ok(());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut state = self
                .listener
                .lock()
                .map_err(|_| std::io::Error::other("Agent bridge unavailable"))?;
            if state.is_none() {
                let directory = tempfile::Builder::new()
                    .prefix("neptune-agents-")
                    .tempdir()?;
                let executable = std::env::current_exe()?;
                let plugins = directory.path().join(PLUGINS);
                std::fs::create_dir(&plugins)?;
                std::fs::write(plugins.join("opencode.js"), OPENCODE_PLUGIN)?;
                std::fs::write(plugins.join("pi.js"), PI_EXTENSION)?;
                for kind in AgentKind::ALL {
                    let path = directory.path().join(kind.executable());
                    std::fs::write(
                        &path,
                        format!(
                            "#!/bin/sh\nexec {} --agent-run {} \"$@\"\n",
                            quote(&executable.to_string_lossy()),
                            kind.executable()
                        ),
                    )?;
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
                }
                let socket = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
                let address = socket.local_addr()?.to_string();
                let stop = Arc::new(AtomicBool::new(false));
                let stopping = stop.clone();
                let shared = self.shared.clone();
                let worker = thread::Builder::new()
                    .name("neptune-agent-hooks".into())
                    .spawn(move || {
                        for stream in socket.incoming() {
                            if stopping.load(Ordering::Acquire) {
                                break;
                            }
                            let Ok(mut stream) = stream else { break };
                            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                            let mut bytes = Vec::new();
                            if read_message(&mut stream, &mut bytes).is_err() {
                                continue;
                            }
                            let reply = match serde_json::from_slice::<Message>(&bytes) {
                                Ok(message) if message.event.is_request() => {
                                    serde_json::to_vec(&answer(&shared, message))
                                        .unwrap_or_default()
                                }
                                Ok(message) => {
                                    let taken = apply_message(&shared, message);
                                    if taken { b"ok" } else { b"no" }.to_vec()
                                }
                                Err(_) => b"no".to_vec(),
                            };
                            let _ = stream.write_all(&reply);
                        }
                    })?;
                *state = Some(Listener {
                    directory,
                    address,
                    stop,
                    worker: Some(worker),
                });
            }
            let listener = state
                .as_ref()
                .ok_or_else(|| std::io::Error::other("Agent bridge unavailable"))?;
            let nonce = tempfile::Builder::new()
                .rand_bytes(32)
                .tempfile_in(listener.directory.path())?;
            let token = nonce
                .path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let shim_path = listener.directory.path().to_string_lossy().into_owned();
            let startup = tempfile::Builder::new()
                .prefix("pane-")
                .tempdir_in(listener.directory.path())?;
            let path = option_env(options, "PATH").unwrap_or_default();
            options.env.extend([
                (ENDPOINT.into(), listener.address.clone()),
                (TOKEN.into(), token.clone()),
                (SHIMS.into(), shim_path.clone()),
                (RUN.into(), String::new()),
                ("PATH".into(), format!("{shim_path}:{path}")),
            ]);
            // What the shell opens with: a task another agent handed over,
            // a saved agent, or nothing.
            let start = {
                let mut shared = self
                    .shared
                    .lock()
                    .map_err(|_| std::io::Error::other("Agent bridge unavailable"))?;
                let resume = resume.filter(|agent| agent.is_valid());
                let fresh = shared.children.get(&pane).is_some_and(|child| {
                    Some(child.parent) == spawned_by
                        && child.launch.is_some()
                        && child.ended.is_none()
                });
                match (spawned_by, resume) {
                    (Some(_), _) if fresh => Start::Spawned,
                    (Some(parent), Some(agent)) => {
                        shared.children.insert(
                            pane,
                            Child {
                                launch: None,
                                session: Some(agent.clone()),
                                owed: 0,
                                started: true,
                                // At rest when it was saved: no news.
                                seen: Some(Status::Idle),
                                ..Child::task(
                                    parent,
                                    Task {
                                        kind: agent.kind,
                                        prompt: String::new(),
                                        model: None,
                                        effort: None,
                                        ultracode: false,
                                        resume: None,
                                    },
                                )
                            },
                        );
                        Start::Resume(agent, true)
                    }
                    (_, resume) => {
                        if let Some(child) = shared.children.get_mut(&pane) {
                            child.end("its terminal was restarted");
                        }
                        resume.map_or(Start::Shell, |agent| Start::Resume(agent, false))
                    }
                }
            };
            configure_shell(options, startup.path(), &std::env::current_exe()?, start)?;
            let (old, retired) = {
                let mut shared = self
                    .shared
                    .lock()
                    .map_err(|_| std::io::Error::other("Agent bridge unavailable"))?;
                shared.changes.remove(&pane);
                shared.activity.remove(&pane);
                shared.links.retain(|link| link.0 != pane);
                shared.attachments.retain(|file| file.0 != pane);
                shared.requests.retain(|request| !request.targets(pane));
                let old = shared.slots.insert(
                    pane,
                    Slot {
                        generation,
                        token,
                        run: None,
                        remote: false,
                        kind: None,
                        conversation: None,
                        prompted: false,
                        activity: None,
                        shown: None,
                        since: Instant::now(),
                        wake,
                        _startup: Some(startup),
                    },
                );
                (old, std::mem::take(&mut shared.retired))
            };
            drop((old, retired));
            Ok(())
        }
    }
    /// Called only by a startup worker, for a terminal that is an SSH
    /// connection. The host is handed this terminal's report credential and
    /// the script that installs the adapters there, as the second and third
    /// arguments of the bootstrap that ends the client's command line.
    pub fn prepare_remote(
        &self,
        pane: PaneId,
        generation: u64,
        options: &mut SessionOptions,
        wake: Wake,
    ) -> std::io::Result<()> {
        let nonce = tempfile::Builder::new()
            .prefix("")
            .rand_bytes(32)
            .tempfile()?;
        let token = nonce
            .path()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        drop(nonce);
        let Some(command) = options.args.last_mut() else {
            return Ok(());
        };
        command.push_str(&format!(
            " {} {}",
            quote(&token),
            quote(remote::installer())
        ));
        let (old, retired) = {
            let mut shared = self
                .shared
                .lock()
                .map_err(|_| std::io::Error::other("Agent bridge unavailable"))?;
            if let Some(child) = shared.children.get_mut(&pane) {
                child.end("its terminal was restarted");
            }
            shared.changes.remove(&pane);
            shared.activity.remove(&pane);
            shared.links.retain(|link| link.0 != pane);
            shared.requests.retain(|request| !request.targets(pane));
            let old = shared.slots.insert(
                pane,
                Slot {
                    generation,
                    token,
                    run: None,
                    remote: true,
                    kind: None,
                    conversation: None,
                    prompted: false,
                    activity: None,
                    shown: None,
                    since: Instant::now(),
                    wake,
                    _startup: None,
                },
            );
            (old, std::mem::take(&mut shared.retired))
        };
        drop((old, retired));
        Ok(())
    }
    /// A line the terminal in `pane` addressed to the application. It counts
    /// only from an SSH terminal and with that terminal's credential, which
    /// output shown there by accident or by design does not have. Reports
    /// whether it was taken.
    pub fn report(&self, pane: PaneId, generation: u64, line: &str) -> bool {
        let mut fields = line.split(';');
        let (Some(token), Some(run), Some(verb)) = (fields.next(), fields.next(), fields.next())
        else {
            return false;
        };
        let Ok(mut guard) = self.shared.lock() else {
            return false;
        };
        let shared = &mut *guard;
        let Some(slot) = shared
            .slots
            .get_mut(&pane)
            .filter(|slot| slot.remote && slot.generation == generation && slot.token == token)
        else {
            return false;
        };
        if run.is_empty() || run.len() > 64 {
            return false;
        }
        let now = Instant::now();
        let open = slot.run.as_deref() == Some(run);
        match verb {
            "open" => {
                let Some(kind) = fields.next().and_then(|name| kind(name).ok()) else {
                    return false;
                };
                slot.run = Some(run.into());
                slot.kind = Some(kind);
                slot.activity = Some(Tracked::idle());
                slot.shown = None;
                slot.since = now;
                shared
                    .activity
                    .insert(pane, (generation, Some(Activity::Idle)));
            }
            "close" if open => {
                slot.run = None;
                slot.kind = None;
                slot.activity = None;
                slot.shown = None;
                shared.activity.insert(pane, (generation, None));
            }
            // The same fields a local hook reads from its input, and nothing
            // else of it: the host's hook sent only these.
            "hook" if open => {
                let (Some(provider), Some(event)) = (slot.kind, fields.next()) else {
                    return false;
                };
                let mut hook = Hook {
                    hook_event_name: event.into(),
                    ..Hook::default()
                };
                for field in fields {
                    let Some((name, value)) = field.split_once('=') else {
                        continue;
                    };
                    let value = Some(value.to_owned()).filter(|value| value.len() <= MAX_TOOL);
                    match name {
                        "tool_name" => hook.tool_name = value,
                        "agent_id" => hook.agent_id = value,
                        "notification_type" => hook.notification_type = value,
                        "permission_mode" => hook.permission_mode = value,
                        "source" => hook.source = value,
                        "trigger" => hook.trigger = value,
                        // A newer adapter on the host may say more.
                        _ => {}
                    }
                }
                if let Some(signal) = signal(provider, &hook) {
                    shared.signal(pane, signal, now);
                }
            }
            _ => return false,
        }
        true
    }
    pub fn close(&self, pane: PaneId) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.retire(pane);
        }
    }
    pub fn close_generation(&self, pane: PaneId, generation: u64) {
        if let Ok(mut shared) = self.shared.lock()
            && shared
                .slots
                .get(&pane)
                .is_some_and(|slot| slot.generation == generation)
        {
            shared.retire(pane);
        }
    }
    /// The agent each pane turned to since the last call, preceded by `None`
    /// where another left the pane first.
    pub fn drain(&self) -> Vec<(PaneId, u64, Option<AgentSession>)> {
        self.shared
            .lock()
            .map(|mut shared| {
                std::mem::take(&mut shared.changes)
                    .into_iter()
                    .flat_map(|(pane, (generation, agent, left))| {
                        let left = (left && agent.is_some()).then_some((pane, generation, None));
                        left.into_iter().chain([(pane, generation, agent)])
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Stands in for an agent asking, where a test has no CLI to ask.
    #[cfg(test)]
    pub fn request(&self, request: AgentRequest) {
        let mut shared = self.shared.lock().unwrap();
        if let AgentRequest::Spawn {
            request, parent, ..
        } = &request
        {
            shared.spawns.insert(*request, (*parent, None));
        }
        shared.requests.push(request);
    }
    /// What the application answered a spawn request with.
    #[cfg(test)]
    pub fn spawned(&self, request: u64) -> Option<Result<PaneId, String>> {
        self.shared
            .lock()
            .unwrap()
            .spawns
            .get(&request)
            .and_then(|spawn| spawn.1.clone())
    }
    /// What agents asked for since the last call, in arrival order.
    pub fn drain_requests(&self) -> Vec<AgentRequest> {
        self.shared
            .lock()
            .map(|mut shared| std::mem::take(&mut shared.requests))
            .unwrap_or_default()
    }
    /// The task a pane being opened for a started agent begins with. Called
    /// before its session is requested, so its shell finds the task.
    pub fn register_spawn(&self, pane: PaneId, parent: PaneId, task: Task) {
        if let Ok(mut shared) = self.shared.lock() {
            // One that is opened again leaves the record of its closed terminal.
            if let Some((closed, _)) = &task.resume
                && shared
                    .children
                    .get(closed)
                    .is_some_and(|child| child.parent == parent && child.ended.is_some())
            {
                shared.children.remove(closed);
            }
            // Ended agents nobody asked about again give way to new ones.
            let ended: Vec<PaneId> = shared
                .children
                .iter()
                .filter(|(_, child)| child.parent == parent && child.ended.is_some())
                .map(|(pane, _)| *pane)
                .collect();
            for pane in ended.iter().rev().skip(AgentSession::MAX_SPAWNED) {
                shared.children.remove(pane);
            }
            shared.children.insert(pane, Child::task(parent, task));
        }
    }
    /// Started agents whose CLI has not taken its task, by pane and
    /// generation: the application watches whether their terminals stand still.
    pub fn starting(&self) -> Vec<(PaneId, u64)> {
        self.shared
            .lock()
            .map(|shared| {
                shared
                    .children
                    .iter()
                    .filter(|(_, child)| !child.started && child.ended.is_none())
                    .filter_map(|(pane, _)| Some((*pane, shared.slots.get(pane)?.generation)))
                    .collect()
            })
            .unwrap_or_default()
    }
    /// What the terminal of a started agent shows now that it has stood
    /// still without taking its task, or `None` once it moves again. The
    /// agent that started it is told, and tells the user.
    pub fn asking(&self, pane: PaneId, generation: u64, screen: Option<String>) {
        if let Ok(mut shared) = self.shared.lock() {
            let shared = &mut *shared;
            if shared
                .slots
                .get(&pane)
                .is_some_and(|slot| slot.generation == generation)
                && let Some(child) = shared.children.get_mut(&pane)
                && !child.started
                && child.ended.is_none()
                && child.asking != screen
            {
                child.asking = screen;
                child.seen = None;
            }
        }
    }
    /// Forgets an agent whose terminal could not be opened; the request's
    /// answer says why.
    pub fn forget_spawn(&self, pane: PaneId) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.children.remove(&pane);
        }
    }
    /// Answers a spawn request with the pane opened for it, or why none was.
    pub fn spawn_result(&self, request: u64, result: Result<PaneId, String>) {
        if let Ok(mut shared) = self.shared.lock()
            && let Some((_, slot)) = shared.spawns.get_mut(&request)
        {
            *slot = Some(result);
        }
    }
    /// What the application shows for the agent in `pane`, which follows the
    /// terminal's title where no hook reports.
    pub fn observe(&self, pane: PaneId, generation: u64, activity: Activity) {
        if let Ok(mut shared) = self.shared.lock() {
            let shared = &mut *shared;
            if let Some(slot) = shared.slots.get_mut(&pane)
                && slot.generation == generation
                && slot.activity.is_some()
            {
                slot.show(Some(activity), Instant::now());
                if activity == Activity::Working
                    && let Some(child) = shared.children.get_mut(&pane)
                {
                    child.start();
                }
            }
        }
    }
    /// The links the model holds, as (started, starter). An agent whose link
    /// is gone has ended for its starter, and is forgotten once the starter
    /// was told or has left.
    pub fn sync_spawned(&self, links: &[(PaneId, PaneId)]) {
        if let Ok(mut shared) = self.shared.lock() {
            let shared = &mut *shared;
            if shared.children.is_empty() {
                return;
            }
            let slots = &shared.slots;
            shared.children.retain(|pane, child| {
                if links.contains(&(*pane, child.parent)) {
                    return true;
                }
                child.end("its agent exited or its terminal was closed");
                // One that can be opened again is kept for as long as the
                // agent that started it might ask for it.
                let told = matches!(child.seen, Some(Status::Ended { .. }));
                (!told || child.reopens())
                    && slots
                        .get(&child.parent)
                        .is_some_and(|parent| parent.run.is_some())
            });
        }
    }
    /// Whether the agent another agent started in `pane` is still open
    /// there, so that words typed for it reach it and not a shell.
    pub fn takes_prompt(&self, pane: PaneId, generation: u64) -> bool {
        self.shared.lock().is_ok_and(|shared| {
            shared.slots.get(&pane).is_some_and(|slot| {
                // Nothing is typed over a request to a person, however new.
                slot.generation == generation
                    && slot.run.is_some()
                    && !matches!(slot.state(), Some(Activity::NeedsInput(_)))
            }) && shared
                .children
                .get(&pane)
                .is_some_and(|child| child.ended.is_none())
        })
    }
    /// What each agent turned to since the last call; `None` when it left.
    /// An agent on an SSH host comes with its CLI, which the model does not
    /// hold for it.
    pub fn drain_activity(&self) -> Vec<(PaneId, u64, Option<Activity>, Option<AgentKind>)> {
        self.shared
            .lock()
            .map(|mut shared| {
                let shared = &mut *shared;
                std::mem::take(&mut shared.activity)
                    .into_iter()
                    .map(|(pane, (generation, activity))| {
                        let remote = shared
                            .slots
                            .get(&pane)
                            .filter(|slot| slot.remote)
                            .and_then(|slot| slot.kind);
                        (pane, generation, activity, remote)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Pull requests agents linked since the last call, in arrival order.
    pub fn drain_links(&self) -> Vec<(PaneId, u64, PullRequest)> {
        self.shared
            .lock()
            .map(|mut shared| std::mem::take(&mut shared.links))
            .unwrap_or_default()
    }
    /// Files agents attached since the last call, in arrival order.
    pub fn drain_attachments(&self) -> Vec<(PaneId, u64, Attachment)> {
        self.shared
            .lock()
            .map(|mut shared| std::mem::take(&mut shared.attachments))
            .unwrap_or_default()
    }
}
impl AgentRequest {
    fn targets(&self, pane: PaneId) -> bool {
        match self {
            Self::Spawn { parent, .. } => *parent == pane,
            Self::Tell { pane: target, .. }
            | Self::Press { pane: target, .. }
            | Self::Close { pane: target, .. } => *target == pane,
        }
    }
}
impl Shared {
    /// Takes a hook's signal for the agent open in `pane`. `None` without
    /// one; otherwise whether it is news for the application.
    fn signal(&mut self, pane: PaneId, signal: Signal, now: Instant) -> Option<bool> {
        let slot = self.slots.get_mut(&pane)?;
        let before = slot.state();
        let prompt = signal == Signal::Prompt;
        slot.prompted |= prompt;
        let tracked = slot.activity.as_mut()?;
        let news = tracked.apply(signal);
        let state = tracked.state;
        if news {
            // A hook is newer than what the title last corrected.
            slot.shown = None;
            if slot.state() != before {
                slot.since = now;
            }
            self.activity.insert(pane, (slot.generation, Some(state)));
        }
        if prompt && let Some(child) = self.children.get_mut(&pane) {
            child.start();
        }
        Some(news)
    }
    /// Queues the opening of a terminal for an agent `parent` starts, and
    /// names the request its answer is asked for by.
    fn queue_spawn(
        &mut self,
        parent: PaneId,
        generation: u64,
        task: Task,
        cwd: PathBuf,
    ) -> Result<u64, String> {
        let live = self
            .children
            .values()
            .filter(|child| child.parent == parent && child.ended.is_none())
            .count();
        if live >= AgentSession::MAX_SPAWNED {
            return Err(format!(
                "{live} agents this one started are still open, which is the most Neptune keeps. Close one with close_agent first."
            ));
        }
        if self.requests.len() >= MAX_REQUESTS || self.spawns.len() >= MAX_REQUESTS {
            return Err("Neptune is busy; try again in a moment.".into());
        }
        self.next_request += 1;
        let request = self.next_request;
        self.spawns.insert(request, (parent, None));
        self.requests.push(AgentRequest::Spawn {
            request,
            parent,
            generation,
            task,
            cwd,
        });
        Ok(request)
    }
    /// Forgets a pane whose terminal closed or was replaced.
    fn retire(&mut self, pane: PaneId) {
        if let Some(mut slot) = self.slots.remove(&pane)
            && let Some(startup) = slot._startup.take()
        {
            self.retired.push(startup);
        }
        self.changes.remove(&pane);
        self.activity.remove(&pane);
        self.links.retain(|link| link.0 != pane);
        self.attachments.retain(|file| file.0 != pane);
        self.requests.retain(|request| !request.targets(pane));
        self.spawns.retain(|_, spawn| spawn.0 != pane);
        if let Some(child) = self.children.get_mut(&pane) {
            child.end("its terminal was closed");
        }
    }
}
/// What a terminal's shell opens with.
#[cfg(unix)]
enum Start<'a> {
    Shell,
    /// A saved agent, and whether another agent started it.
    Resume(&'a AgentSession, bool),
    /// The task another agent handed over, held by the bridge.
    Spawned,
}
#[cfg(unix)]
fn configure_shell(
    options: &mut SessionOptions,
    directory: &std::path::Path,
    helper: &std::path::Path,
    start: Start,
) -> std::io::Result<()> {
    let default_shell = options.shell.is_none();
    // Configured arguments are the user's: start the shell as given instead
    // of replacing them with Neptune's zsh/bash startup.
    let configured_args = !options.args.is_empty();
    let shell = options
        .shell
        .clone()
        .or_else(|| option_env(options, "SHELL"))
        .unwrap_or_else(|| "/bin/sh".into());
    let name = std::path::Path::new(&shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    // The helper's arguments: always three, for shells started without a
    // startup file of Neptune's.
    let launch: Option<[String; 3]> = match start {
        Start::Shell => None,
        Start::Resume(agent, spawned) => Some([
            "--agent-restore".into(),
            serde_json::to_string(agent)?,
            if spawned { "spawned" } else { "" }.into(),
        ]),
        Start::Spawned => Some(["--agent-spawn".into(), String::new(), String::new()]),
    };
    let mut setup = "export PATH=\"$NEPTUNE_AGENT_SHIMS:$PATH\"\n".to_owned();
    if let Some(launch) = &launch {
        if name == "zsh" {
            // Powerlevel10k's instant prompt keeps stdio off the terminal until the
            // first prompt, later than any startup file; the agent needs it now.
            setup.push_str("(( ${+functions[p10k]} )) && p10k clear-instant-prompt\n");
        }
        // Quoted literal data, never interpolated terminal input. This runs after user startup files.
        setup.push_str(&format!(
            "{} {} {} {}\n",
            quote(&helper.to_string_lossy()),
            launch[0],
            quote(&launch[1]),
            quote(&launch[2])
        ));
    }
    match name {
        "zsh" if !configured_args => {
            let dotdir = tempfile::Builder::new()
                .prefix("zsh-")
                .tempdir_in(directory)?
                .keep();
            let user_dir = option_env(options, "ZDOTDIR");
            options.env.push((
                "NEPTUNE_USER_ZDOTDIR".into(),
                user_dir
                    .clone()
                    .unwrap_or_else(|| option_env(options, "HOME").unwrap_or_default()),
            ));
            options.env.push((
                "NEPTUNE_USER_ZDOTDIR_SET".into(),
                if user_dir.is_some() { "1" } else { "0" }.into(),
            ));
            let restore = "if [[ $NEPTUNE_USER_ZDOTDIR_SET == 1 ]]; then ZDOTDIR=$NEPTUNE_USER_ZDOTDIR; else unset ZDOTDIR; fi\n";
            let quoted_dotdir = quote(&dotdir.to_string_lossy());
            let capture = format!(
                "NEPTUNE_USER_ZDOTDIR=${{ZDOTDIR-$HOME}}\nNEPTUNE_USER_ZDOTDIR_SET=${{+ZDOTDIR}}\nZDOTDIR={quoted_dotdir}\n"
            );
            // Global zshrc (macOS /etc/zshrc) runs while this directory is
            // ZDOTDIR and puts HISTFILE in it; keep history in the user's ZDOTDIR.
            let history = format!(
                "[[ $HISTFILE == {quoted_dotdir}/* ]] && HISTFILE=${{ZDOTDIR-$HOME}}/${{HISTFILE#{quoted_dotdir}/}}\n"
            );
            for file in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
                let mut content = restore.to_owned();
                if file == ".zshrc" {
                    content.push_str(&history);
                }
                content.push_str(&format!(
                    "[[ -r ${{ZDOTDIR-$HOME}}/{file} ]] && source \"${{ZDOTDIR-$HOME}}/{file}\"\n"
                ));
                match file {
                    ".zshrc" => content.push_str(&format!("if [[ ! -o login ]]; then\n{setup}unset NEPTUNE_USER_ZDOTDIR NEPTUNE_USER_ZDOTDIR_SET\nelse\n{capture}fi\n")),
                    ".zlogin" => content.push_str(&format!("{setup}unset NEPTUNE_USER_ZDOTDIR NEPTUNE_USER_ZDOTDIR_SET\n")),
                    _ => content.push_str(&capture),
                }
                std::fs::write(dotdir.join(file), content)?;
            }
            options
                .env
                .push(("ZDOTDIR".into(), dotdir.to_string_lossy().into_owned()));
            options.shell = Some(shell);
            options.args = vec![if default_shell { "-il" } else { "-i" }.into()];
        }
        "bash" if !configured_args => {
            let mut file = tempfile::Builder::new()
                .prefix("bash-")
                .tempfile_in(directory)?;
            let startup = if default_shell {
                "[[ -r /etc/profile ]] && source /etc/profile\nfor f in ~/.bash_profile ~/.bash_login ~/.profile; do if [[ -r $f ]]; then source \"$f\"; break; fi; done\nunset f\n"
            } else {
                "[[ -r ~/.bashrc ]] && source ~/.bashrc\n"
            };
            file.write_all(format!("{startup}{setup}").as_bytes())?;
            let (_, path) = file.keep().map_err(|error| error.error)?;
            options.shell = Some(shell);
            options.args = vec![
                "--rcfile".into(),
                path.to_string_lossy().into_owned(),
                "-i".into(),
            ];
        }
        _ => {
            if let Some(launch) = launch {
                options.shell = Some("/bin/sh".into());
                let args = std::mem::take(&mut options.args);
                options.args = vec![
                    "-c".into(),
                    "\"$1\" \"$2\" \"$3\" \"$4\"; shift 4; exec \"$@\"".into(),
                    "neptune".into(),
                    helper.to_string_lossy().into_owned(),
                ];
                options.args.extend(launch);
                options.args.push(shell);
                options.args.extend(args);
            }
        }
    }
    Ok(())
}
fn read_message(stream: &mut TcpStream, bytes: &mut Vec<u8>) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_millis(200);
    let mut buffer = [0; 4096];
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "Agent event timed out")
            })?;
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        if bytes.len() + count > MAX_MESSAGE as usize {
            return Err(std::io::Error::other("Agent event too large"));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}
fn option_env(options: &SessionOptions, key: &str) -> Option<String> {
    options
        .env
        .iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var(key).ok())
}
/// Reports whether the event was taken for its pane.
fn apply_message(shared: &Mutex<Shared>, message: Message) -> bool {
    let Ok(mut guard) = shared.lock() else {
        return false;
    };
    let shared = &mut *guard;
    let Some((&pane, slot)) = shared
        .slots
        .iter_mut()
        .find(|(_, slot)| !slot.remote && slot.token == message.token)
    else {
        return false;
    };
    let now = Instant::now();
    let (generation, wake) = (slot.generation, slot.wake.clone());
    let open = slot.run.as_ref() == Some(&message.run);
    if let Event::PullRequest { url } = &message.event {
        // Only the invocation that opened in this pane may link to it.
        let link = PullRequest::parse(url).filter(|_| open);
        let Some(link) = link.filter(|_| shared.links.len() < MAX_LINKS) else {
            return false;
        };
        shared.links.push((pane, generation, link));
        drop(guard);
        wake();
        return true;
    }
    if let Event::Attachment { path, title } = &message.event {
        // As for a link: only the invocation that opened in this pane.
        let file = Attachment::new(path.clone(), title.as_deref()).filter(|_| open);
        let Some(file) = file.filter(|_| shared.attachments.len() < MAX_LINKS) else {
            return false;
        };
        shared.attachments.push((pane, generation, file));
        drop(guard);
        wake();
        return true;
    }
    if matches!(message.event, Event::Serving) {
        // Codex starts its tool servers again when it turns to another
        // conversation (`/new`, `/clear`, `/resume`), and names that one only
        // with its first prompt. Between turns of a conversation that had a
        // prompt, the start says the conversation on the tab is over.
        let between = slot.kind == Some(AgentKind::Codex)
            && slot.prompted
            && slot
                .activity
                .as_ref()
                .is_some_and(|tracked| tracked.state == Activity::Idle);
        let Some(agent) = slot.conversation.as_mut().filter(|_| open && between) else {
            return open;
        };
        agent.session_id = None;
        let agent = agent.clone();
        slot.prompted = false;
        shared.links.retain(|link| link.0 != pane);
        shared.attachments.retain(|file| file.0 != pane);
        let left = shared.changes.get(&pane).is_some_and(|change| change.2);
        shared.changes.insert(pane, (generation, Some(agent), left));
        drop(guard);
        wake();
        return true;
    }
    if matches!(message.event, Event::Stopped) {
        if !open {
            return false;
        }
        slot.run = None;
        slot.activity = None;
        slot.shown = None;
        if let Some(child) = shared.children.get_mut(&pane) {
            child.end("its saved session could not be resumed");
            child.session = None;
        }
        shared.activity.insert(pane, (generation, None));
        drop(guard);
        wake();
        return true;
    }
    if let Event::Activity { signal } = message.event {
        // Only the open invocation describes this pane.
        if !open {
            return false;
        }
        let Some(news) = shared.signal(pane, signal, now) else {
            return false;
        };
        if news {
            drop(guard);
            wake();
        }
        return true;
    }
    if let Event::Report { text, done } = message.event {
        // Only an open agent that another started has anyone to tell.
        let Some(child) = shared
            .children
            .get_mut(&pane)
            .filter(|child| open && child.ended.is_none())
        else {
            return false;
        };
        if !text.trim().is_empty() {
            if child.outbox.len() >= MAX_OUTBOX {
                child.outbox.pop_front();
            }
            child.outbox.push_back(clip(&text));
        }
        child.seen = None;
        if !done {
            return true;
        }
        child.owed = child.owed.saturating_sub(1);
        child.marked = now;
        child.start();
        // The turn is over whichever of this and its hook's signal is first.
        let before = slot.state();
        if let Some(tracked) = slot.activity.as_mut() {
            tracked.apply(Signal::Done);
            slot.shown = None;
            if slot.state() != before {
                slot.since = now;
            }
            shared
                .activity
                .insert(pane, (generation, Some(Activity::Idle)));
        }
        drop(guard);
        wake();
        return true;
    }
    let mut reset = None;
    // An invocation that opens over one that never reported leaving ends it.
    let mut left = false;
    let agent = match message.event {
        Event::Open { agent } if agent.is_valid() => {
            left = slot.run.is_some();
            slot.run = Some(message.run);
            slot.kind = Some(agent.kind);
            slot.conversation = Some(agent.clone());
            slot.prompted = false;
            slot.activity = Some(Tracked::idle());
            slot.shown = None;
            slot.since = now;
            reset = Some(Some(Activity::Idle));
            Some(agent)
        }
        Event::Session { agent } if open && agent.is_valid() => {
            // What the conversation before linked and no frame has taken yet
            // stays with it.
            let named = slot.conversation.as_ref();
            // Codex names its conversation again with each prompt.
            if named == Some(&agent) {
                return true;
            }
            if named.is_some_and(|named| {
                named.session_id.is_some() && named.session_id != agent.session_id
            }) {
                shared.links.retain(|link| link.0 != pane);
                shared.attachments.retain(|file| file.0 != pane);
                slot.prompted = false;
            }
            slot.conversation = Some(agent.clone());
            Some(agent)
        }
        Event::Close if open => {
            left = true;
            slot.run = None;
            slot.kind = None;
            slot.conversation = None;
            slot.prompted = false;
            slot.activity = None;
            slot.shown = None;
            reset = Some(None);
            None
        }
        _ => return false,
    };
    if left {
        // The agents it started have no one to answer, and it answers no one.
        shared.children.retain(|_, child| child.parent != pane);
        shared.requests.retain(
            |request| !matches!(request, AgentRequest::Spawn { parent, .. } if *parent == pane),
        );
        shared.spawns.retain(|_, spawn| spawn.0 != pane);
        if let Some(child) = shared.children.get_mut(&pane).filter(|_| agent.is_none()) {
            child.end("its agent exited");
        }
    }
    if let (Some(agent), Some(child)) = (&agent, shared.children.get_mut(&pane))
        && child.ended.is_none()
    {
        child.session = Some(agent.clone());
    }
    // A new conversation in the same run keeps what the agent is doing.
    if let Some(activity) = reset {
        shared.activity.insert(pane, (generation, activity));
    }
    let left = left || shared.changes.get(&pane).is_some_and(|change| change.2);
    shared.changes.insert(pane, (generation, agent, left));
    drop(guard);
    wake();
    true
}
/// The task a started agent opens with, led by who handed it over and how
/// its answer travels back.
fn delegation(parent: AgentKind, prompt: &str) -> String {
    format!(
        "[{} in another Neptune terminal started you and handed you the task below. \
It reads the last message of each of your turns and nothing else you write, so end \
each turn with what you did and what it needs to know.]\n\n{prompt}",
        label(parent)
    )
}
/// Answers a request and wakes the application where it has work to do.
fn answer(shared: &Mutex<Shared>, message: Message) -> Answer {
    let (answer, wake) = respond(shared, message);
    if let Some(wake) = wake {
        wake();
    }
    answer
}
fn respond(shared: &Mutex<Shared>, message: Message) -> (Answer, Option<Wake>) {
    let refused = |reason: String| (Answer::Refused { reason }, None);
    let unknown = |agent: u64| {
        refused(format!(
            "Agent {agent} is not an agent this one started, or it ended and that was already reported."
        ))
    };
    let Ok(mut guard) = shared.lock() else {
        return refused("Neptune is unavailable.".into());
    };
    let shared = &mut *guard;
    let Some((&pane, slot)) = shared
        .slots
        .iter()
        .find(|(_, slot)| !slot.remote && slot.token == message.token)
    else {
        return refused(NOT_TRACKED.into());
    };
    let (generation, wake, kind) = (slot.generation, slot.wake.clone(), slot.kind);
    let open = slot.run.as_ref() == Some(&message.run);
    let now = Instant::now();
    match message.event {
        // A started terminal asks before its CLI, and so any run, exists.
        Event::Launch => {
            let launch = shared
                .children
                .get_mut(&pane)
                .filter(|child| child.ended.is_none())
                .and_then(|child| {
                    let prompt = child.launch.take()?;
                    child.marked = now;
                    Some(Answer::Launch {
                        kind: child.kind,
                        prompt,
                        model: child.model.clone(),
                        effort: child.effort.clone(),
                        ultracode: child.ultracode,
                        resume: child.resume.take(),
                    })
                });
            match launch {
                Some(launch) => (launch, None),
                None => refused("No agent is waiting to start in this terminal.".into()),
            }
        }
        Event::LaunchFailed { reason } => {
            let Some(child) = shared.children.get_mut(&pane) else {
                return refused("No agent was starting in this terminal.".into());
            };
            let reason: String = reason.chars().take(300).collect();
            child.end(&format!("it could not start ({reason})"));
            // The model drops the link along with the agent that never opened.
            shared.changes.insert(pane, (generation, None, false));
            (Answer::Done, Some(wake))
        }
        _ if !open => refused(NOT_TRACKED.into()),
        Event::Spawn {
            kind: wanted,
            prompt,
            cwd,
            model,
            effort,
            ultra,
        } => {
            let Some(kind) = kind else {
                return refused(NOT_TRACKED.into());
            };
            if prompt.trim().is_empty() || prompt.len() > MAX_TEXT || !cwd.is_absolute() {
                return refused("The task or its directory is not usable.".into());
            }
            let model = match model.as_deref().map(model_name) {
                Some(None) => {
                    return refused(
                        "model is not a model name as the agent's CLI takes one, such as opus or gpt-5.1-codex.".into(),
                    );
                }
                Some(Some(model)) => Some(model.to_owned()),
                None => None,
            };
            let (effort, ultracode) = match effort_level(wanted, effort.as_deref(), ultra) {
                Ok(level) => level,
                Err(reason) => return refused(reason),
            };
            let task = Task {
                kind: wanted,
                prompt: delegation(kind, &prompt),
                model,
                effort,
                ultracode,
                resume: None,
            };
            match shared.queue_spawn(pane, generation, task, cwd) {
                Ok(request) => (Answer::Queued { request }, Some(wake)),
                Err(reason) => refused(reason),
            }
        }
        Event::Reopen { agent, text } => {
            let id = PaneId::new(agent);
            let Some(kind) = kind else {
                return refused(NOT_TRACKED.into());
            };
            let Some(child) = shared
                .children
                .get(&id)
                .filter(|child| child.parent == pane)
            else {
                return unknown(agent);
            };
            if child.ended.is_none() {
                return refused(format!(
                    "Agent {agent} is still open; send_agent_message reaches it."
                ));
            }
            let session = child
                .session
                .as_ref()
                .filter(|_| child.reopens())
                .and_then(|session| Some((session.session_id.clone()?, session.cwd.clone())));
            let Some((session, cwd)) = session else {
                return refused(format!(
                    "Agent {agent} left no conversation that can be opened again. Start a new agent with spawn_agent."
                ));
            };
            if !cwd.is_dir() {
                return refused(format!(
                    "Agent {agent} worked in {}, which no longer exists. Start a new agent with spawn_agent.",
                    cwd.display()
                ));
            }
            if text.trim().is_empty() || text.len() > MAX_TEXT {
                return refused("The message is empty or too long.".into());
            }
            let task = Task {
                kind: child.kind,
                prompt: format!(
                    "[{} reopened your terminal, which had been closed, and says:]\n\n{text}",
                    label(kind)
                ),
                model: child.model.clone(),
                effort: child.effort.clone(),
                ultracode: child.ultracode,
                resume: Some((id, session)),
            };
            match shared.queue_spawn(pane, generation, task, cwd) {
                Ok(request) => (Answer::Queued { request }, Some(wake)),
                Err(reason) => refused(reason),
            }
        }
        Event::Press { agent, keys } => {
            let id = PaneId::new(agent);
            let target = shared.slots.get(&id);
            let Some(child) = shared
                .children
                .get_mut(&id)
                .filter(|child| child.parent == pane)
            else {
                return unknown(agent);
            };
            let Some(target) = target
                .filter(|target| matches!(child.status(Some(target), now), Status::Asking { .. }))
            else {
                return refused(format!(
                    "Agent {agent} is not showing a question from before it took its task. Keys are pressed only for those; whatever else it asks is the user's to answer in its terminal."
                ));
            };
            let pressed: Option<Vec<Press>> = keys.iter().map(|key| Press::parse(key)).collect();
            let Some(keys) = pressed.filter(|keys| (1..=MAX_KEYS).contains(&keys.len())) else {
                return refused(format!(
                    "keys are up to {MAX_KEYS} of: enter, escape, tab, space, up, down, left, right, or one letter or digit each."
                ));
            };
            if shared.requests.len() >= MAX_REQUESTS {
                return refused("Neptune is busy; try again in a moment.".into());
            }
            // Its terminal is watched again from here.
            child.asking = None;
            child.seen = None;
            child.marked = now;
            shared.requests.push(AgentRequest::Press {
                pane: id,
                generation: target.generation,
                keys,
            });
            (Answer::Done, Some(wake))
        }
        Event::SpawnResult { request } => {
            match shared.spawns.get(&request).filter(|spawn| spawn.0 == pane) {
                None => refused("Neptune lost track of that request; try again.".into()),
                Some((_, None)) => (Answer::Pending, None),
                Some(_) => match shared.spawns.remove(&request).and_then(|spawn| spawn.1) {
                    Some(Ok(pane)) => (Answer::Spawned { agent: pane.get() }, None),
                    Some(Err(reason)) => refused(reason),
                    None => (Answer::Pending, None),
                },
            }
        }
        Event::Collect { agent, take } => {
            let mut agents = Vec::new();
            for (id, child) in shared
                .children
                .iter_mut()
                .filter(|(id, child)| child.parent == pane && agent.is_none_or(|a| a == id.get()))
            {
                let status = child.status(shared.slots.get(id), now);
                let settled = status.settled()
                    || (status == Status::Starting
                        && now.saturating_duration_since(child.marked) >= START_GRACE);
                let news =
                    !child.outbox.is_empty() || (settled && child.seen.as_ref() != Some(&status));
                let reopens = child.reopens();
                let replies = if take {
                    child.outbox.drain(..).collect()
                } else {
                    Vec::new()
                };
                if !settled {
                    child.seen = None;
                } else if take {
                    child.seen = Some(status.clone());
                    // A prompt folded into the turn before it is not owed one.
                    if status == Status::Idle {
                        child.owed = 0;
                    }
                }
                agents.push(AgentReport {
                    agent: id.get(),
                    kind: child.kind,
                    status,
                    news,
                    replies,
                    reopens,
                });
            }
            match agent {
                Some(agent) if agents.is_empty() => unknown(agent),
                _ => (Answer::Agents { agents }, None),
            }
        }
        Event::Tell { agent, text } => {
            let id = PaneId::new(agent);
            let target = shared.slots.get(&id);
            let Some(child) = shared
                .children
                .get_mut(&id)
                .filter(|child| child.parent == pane)
            else {
                return unknown(agent);
            };
            // A request to a person stands from its first moment for what is
            // typed, though it is reported only once it has stood a while.
            if target.is_some_and(|slot| matches!(slot.state(), Some(Activity::NeedsInput(_)))) {
                return refused(format!(
                    "Agent {agent} is waiting for a person to answer it in its tab, so nothing can be typed for it. Tell the user."
                ));
            }
            let working = match child.status(target, now) {
                Status::Ended { reason } => {
                    return refused(format!("Agent {agent} is no longer running: {reason}."));
                }
                Status::Starting => {
                    return refused(format!(
                        "Agent {agent} has not taken its first task yet. Wait for it with wait_for_agent."
                    ));
                }
                Status::Asking { .. } => {
                    return refused(format!(
                        "Agent {agent} is asking something before it takes its task, so a message would answer that instead. wait_for_agent shows what it asks."
                    ));
                }
                Status::Waiting { .. } => {
                    return refused(format!(
                        "Agent {agent} is waiting for a person to answer it in its tab, so nothing can be typed for it. Tell the user."
                    ));
                }
                Status::Working => true,
                Status::Idle => false,
            };
            let Some(target) = target else {
                return unknown(agent);
            };
            if text.trim().is_empty() || text.len() > MAX_TEXT {
                return refused("The message is empty or too long.".into());
            }
            if shared.requests.len() >= MAX_REQUESTS {
                return refused("Neptune is busy; try again in a moment.".into());
            }
            child.owed = child.owed.saturating_add(1);
            child.marked = now;
            child.seen = None;
            shared.requests.push(AgentRequest::Tell {
                pane: id,
                generation: target.generation,
                text,
            });
            (Answer::Told { working }, Some(wake))
        }
        Event::Dismiss { agent } => {
            let id = PaneId::new(agent);
            let target = shared.slots.get(&id).map(|slot| slot.generation);
            let Some(child) = shared
                .children
                .get_mut(&id)
                .filter(|child| child.parent == pane)
            else {
                return unknown(agent);
            };
            if child.ended.is_some() {
                return refused(format!(
                    "Agent {agent} has already ended; a terminal it left is the user's to close.{}",
                    if child.reopens() {
                        " reopen_agent opens it again with its conversation."
                    } else {
                        ""
                    }
                ));
            }
            let Some(target) = target else {
                return refused(format!(
                    "Agent {agent} is still starting; try again in a moment."
                ));
            };
            child.end("the agent that started it closed it");
            child.seen = Some(child.status(None, now));
            shared.requests.push(AgentRequest::Close {
                pane: id,
                generation: target,
                parent: pane,
            });
            (Answer::Done, Some(wake))
        }
        _ => refused("Neptune does not answer that.".into()),
    }
}
pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn kind(value: &str) -> anyhow::Result<AgentKind> {
    AgentKind::ALL
        .into_iter()
        .find(|kind| kind.executable() == value)
        .ok_or_else(|| anyhow::anyhow!("Unknown agent"))
}
/// Delivers one event and reports whether the bridge took it for this pane.
fn send(event: Event, run: &str) -> anyhow::Result<bool> {
    Ok(exchange(event, run)? == b"ok")
}
/// Makes a request of the application for the agent open in this terminal.
fn ask(event: Event, run: &str) -> Answer {
    exchange(event, run)
        .ok()
        .and_then(|reply| serde_json::from_slice(&reply).ok())
        .unwrap_or(Answer::Refused {
            reason: "Neptune did not answer. This terminal may have been restarted.".into(),
        })
}
fn exchange(event: Event, run: &str) -> anyhow::Result<Vec<u8>> {
    let endpoint: std::net::SocketAddr = std::env::var(ENDPOINT)?.parse()?;
    anyhow::ensure!(endpoint.ip().is_loopback(), "Invalid agent bridge address");
    let message = Message {
        token: std::env::var(TOKEN)?,
        run: run.into(),
        event,
    };
    let mut stream = TcpStream::connect_timeout(&endpoint, Duration::from_millis(200))?;
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.write_all(&serde_json::to_vec(&message)?)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = Vec::new();
    stream.take(MAX_ANSWER).read_to_end(&mut reply)?;
    Ok(reply)
}
/// Private CLI entry points, handled before desktop initialization. Never logs hook input.
pub fn cli(args: &[String]) -> anyhow::Result<Option<i32>> {
    match args.first().map(String::as_str) {
        Some("--agent-hook") => {
            let provider = kind(args.get(1).map(String::as_str).unwrap_or_default())?;
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(MAX_HOOK + 1)
                .read_to_end(&mut bytes)?;
            // Input too large to read whole still marks the moment its hook names.
            let hook = serde_json::from_slice::<Hook>(&bytes)
                .ok()
                .filter(|_| bytes.len() as u64 <= MAX_HOOK)
                .or_else(|| {
                    args.get(2).map(|event| Hook {
                        hook_event_name: event.clone(),
                        ..Hook::default()
                    })
                });
            if let Some(hook) = hook {
                let run = std::env::var(RUN).unwrap_or_default();
                // Codex names a conversation it resumed or began anew with
                // the first prompt; each prompt says which one is open.
                let names = hook.hook_event_name == "SessionStart"
                    || (provider == AgentKind::Codex
                        && hook.hook_event_name == "UserPromptSubmit");
                if names
                    && hook.agent_id.is_none()
                    && let (Some(session_id), Some(cwd)) = (&hook.session_id, &hook.cwd)
                {
                    let agent = AgentSession {
                        kind: provider,
                        session_id: Some(session_id.clone()),
                        cwd: cwd.clone(),
                    };
                    if agent.is_valid() {
                        let _ = send(Event::Session { agent }, &run);
                    }
                }
                // An agent another agent started hands over the last message
                // of its turn; nobody else's words leave the hook process.
                if std::env::var_os(SPAWNED).is_some()
                    && hook.agent_id.is_none()
                    && matches!(
                        hook.hook_event_name.as_str(),
                        "Stop" | "StopFailure" | "Interrupt"
                    )
                {
                    let text = clip(hook.last_assistant_message.as_deref().unwrap_or_default());
                    let _ = send(Event::Report { text, done: true }, &run);
                }
                if let Some(signal) = signal(provider, &hook) {
                    let _ = send(Event::Activity { signal }, &run);
                }
            }
            Ok(Some(0))
        }
        Some("--agent-close") => {
            let failed_resume = args.get(1).is_some_and(|status| status != "0")
                && args.get(2).is_some_and(|resumed| resumed == "1");
            if failed_resume {
                eprintln!(
                    "Neptune: resume failed; the session reference is kept. Restart this terminal for a fresh shell."
                );
                let _ = send(Event::Stopped, &std::env::var(RUN).unwrap_or_default());
            } else {
                let _ = send(Event::Close, &std::env::var(RUN).unwrap_or_default());
            }
            Ok(Some(0))
        }
        Some("--agent-mcp") => {
            struct Bridge(String);
            impl super::agent_mcp::Host for Bridge {
                fn send(&mut self, event: Event) -> bool {
                    send(event, &self.0).unwrap_or(false)
                }
                fn ask(&mut self, event: Event) -> Answer {
                    ask(event, &self.0)
                }
            }
            let run = std::env::var(RUN).unwrap_or_default();
            let _ = send(Event::Serving, &run);
            super::agent_mcp::serve(
                std::io::stdin(),
                std::io::stdout().lock(),
                &mut Bridge(run),
                std::env::var_os(SPAWNED).is_some(),
            )?;
            Ok(Some(0))
        }
        Some("--agent-run") => {
            let provider = kind(args.get(1).map(String::as_str).unwrap_or_default())?;
            Ok(Some(run_agent(provider, &args[2..], None, false, false)?))
        }
        Some("--agent-spawn") => {
            // Shell startup can run this again, as a sourced file does; only
            // the first time finds a task waiting.
            let Answer::Launch {
                kind,
                prompt,
                model,
                effort,
                ultracode,
                resume,
            } = ask(Event::Launch, "")
            else {
                return Ok(Some(0));
            };
            let failed = |reason: String| {
                eprintln!("Neptune: the agent could not start: {reason}");
                let _ = ask(Event::LaunchFailed { reason }, "");
                Ok(Some(0))
            };
            // The same terminal a restored agent needs.
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return failed("its shell's startup left input or output off the terminal".into());
            }
            let resumed = resume.map(|session| AgentSession {
                kind,
                session_id: Some(session),
                cwd: std::env::current_dir().unwrap_or_default(),
            });
            let mut arguments = Vec::new();
            if let Some(agent) = &resumed {
                if !agent.is_valid() {
                    return failed("its conversation could not be found again".into());
                }
                arguments.extend(resume_arguments(agent));
            }
            if let Some(model) = model {
                arguments.extend([
                    match kind {
                        AgentKind::Claude => "--model",
                        _ => "-m",
                    }
                    .to_owned(),
                    model,
                ]);
            }
            if let Some(effort) = effort {
                arguments.extend(match kind {
                    AgentKind::Claude => ["--effort".to_owned(), effort],
                    _ => [
                        "-c".to_owned(),
                        format!("model_reasoning_effort={}", toml::Value::String(effort)),
                    ],
                });
            }
            arguments.push(prompt);
            match run_agent(kind, &arguments, resumed.as_ref(), true, ultracode) {
                Ok(status) => Ok(Some(status)),
                Err(error) => failed(error.to_string()),
            }
        }
        Some("--agent-restore") => {
            let agent: AgentSession = serde_json::from_str(
                args.get(1)
                    .ok_or_else(|| anyhow::anyhow!("Missing resume reference"))?,
            )?;
            anyhow::ensure!(agent.is_valid(), "Invalid resume reference");
            // Startup files can leave stdio redirected. Without the terminal the CLI
            // would run as a batch job; keep the reference and leave the shell quiet.
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return Ok(Some(0));
            }
            let arguments = resume_arguments(&agent);
            let spawned = args.get(2).is_some_and(|mark| mark == "spawned");
            Ok(Some(run_agent(
                agent.kind,
                &arguments,
                Some(&agent),
                spawned,
                false,
            )?))
        }
        _ => Ok(None),
    }
}
/// What a CLI is given to go on with a conversation of its own.
fn resume_arguments(agent: &AgentSession) -> Vec<String> {
    match &agent.session_id {
        Some(id) => vec![
            match agent.kind {
                AgentKind::Claude | AgentKind::Gemini | AgentKind::Omp => "--resume",
                AgentKind::Codex => "resume",
                AgentKind::Opencode | AgentKind::Pi => "--session",
            }
            .into(),
            id.clone(),
        ],
        None => Vec::new(),
    }
}
/// The fields of a hook's input that say what kind of moment it is.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Hook {
    hook_event_name: String,
    session_id: Option<String>,
    cwd: Option<PathBuf>,
    tool_name: Option<String>,
    /// Set for events inside a subagent.
    agent_id: Option<String>,
    notification_type: Option<String>,
    permission_mode: Option<String>,
    source: Option<String>,
    trigger: Option<String>,
    /// What the agent last said, read only for one another agent started.
    last_assistant_message: Option<String>,
}
/// What a hook means for the agent's activity, if anything.
fn signal(provider: AgentKind, hook: &Hook) -> Option<Signal> {
    let subagent = hook.agent_id.is_some();
    let lead = |signal| (!subagent).then_some(signal);
    let tool = hook
        .tool_name
        .clone()
        .filter(|tool| !tool.is_empty() && tool.len() <= MAX_TOOL);
    // Tools that put a question or a plan in front of a person.
    let asks = match (provider, hook.tool_name.as_deref()) {
        (AgentKind::Claude, Some("AskUserQuestion")) => Some(Attention::Question),
        (AgentKind::Claude, Some("ExitPlanMode")) => Some(Attention::Plan),
        (AgentKind::Codex, Some("request_user_input")) => Some(Attention::Question),
        (AgentKind::Opencode, Some("question")) => Some(Attention::Question),
        (AgentKind::Omp, Some("ask")) => Some(Attention::Question),
        _ => None,
    };
    let ask = |attention| {
        Some(Signal::Ask {
            attention,
            tool: tool.clone(),
            subagent,
        })
    };
    let notice = |attention| {
        Some(Signal::Ask {
            attention,
            tool: None,
            subagent,
        })
    };
    match hook.hook_event_name.as_str() {
        // Neptune's own plugins name a session when its first turn begins.
        "SessionStart"
            if matches!(
                provider,
                AgentKind::Opencode | AgentKind::Pi | AgentKind::Omp
            ) =>
        {
            None
        }
        // Compaction restarts the session in the middle of a turn.
        "SessionStart" if hook.source.as_deref() != Some("compact") => lead(Signal::Done),
        "UserPromptSubmit" => lead(Signal::Prompt),
        "PreToolUse" => match asks {
            Some(attention) => ask(attention),
            None => lead(Signal::Tool),
        },
        "PermissionRequest" => match asks {
            Some(attention) => ask(attention),
            // The request is decided without a person when nothing is asked.
            None if hook.permission_mode.as_deref() == Some("bypassPermissions") => None,
            None => ask(Attention::Permission),
        },
        "PostToolUse" | "PostToolUseFailure" => Some(Signal::ToolDone { tool, subagent }),
        "Notification" => match hook.notification_type.as_deref()? {
            "permission_prompt" | "worker_permission_prompt" => notice(Attention::Permission),
            "elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input" => {
                notice(Attention::Input)
            }
            "elicitation_complete" | "elicitation_response" => Some(Signal::Answered),
            _ => None,
        },
        "Elicitation" => notice(Attention::Input),
        "ElicitationResult" => Some(Signal::Answered),
        "Stop" | "StopFailure" | "Interrupt" => lead(Signal::Done),
        // A compaction the person asked for ends without a Stop.
        "PostCompact" if hook.trigger.as_deref() == Some("manual") => lead(Signal::Done),
        _ => None,
    }
}
/// Hook events that say what the agent is doing, beyond `SessionStart`.
/// OpenCode and pi report through a plugin of Neptune's instead, and Gemini
/// CLI through its title alone.
fn activity_hooks(provider: AgentKind) -> &'static [&'static str] {
    match provider {
        AgentKind::Opencode | AgentKind::Gemini | AgentKind::Pi | AgentKind::Omp => &[],
        AgentKind::Claude => &[
            "UserPromptSubmit",
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PostToolUseFailure",
            "Notification",
            "Elicitation",
            "ElicitationResult",
            "Stop",
            "StopFailure",
            "PostCompact",
        ],
        AgentKind::Codex => &[
            "UserPromptSubmit",
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "Stop",
            "Interrupt",
        ],
    }
}
/// Claude Code's `hooks` setting: `command` for the session's start, and
/// `command EVENT` for each event that says what the agent is doing.
pub(super) fn claude_hooks(command: &str) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    hooks.insert(
        "SessionStart".into(),
        serde_json::json!([{"hooks":[{"type":"command","command":command}]}]),
    );
    // In order, so states follow each other as the agent reached them; the
    // command answers in milliseconds and never blocks a tool.
    for event in activity_hooks(AgentKind::Claude) {
        hooks.insert(
            (*event).into(),
            serde_json::json!([{"hooks":[{
                "type":"command","command":format!("{command} {event}"),"timeout":5
            }]}]),
        );
    }
    hooks.into()
}
/// Codex's `-c` overrides for the same hooks.
pub(super) fn codex_hooks(command: &str) -> Vec<String> {
    let start = format!(
        "hooks.SessionStart=[{{hooks=[{{type=\"command\",command={}}}]}}]",
        toml::Value::String(command.into())
    );
    let activity = activity_hooks(AgentKind::Codex).iter().map(|event| {
        // Codex allows an interrupt hook three seconds and says so at every
        // start when asked for more.
        let timeout = if *event == "Interrupt" { 3 } else { 5 };
        format!(
            "hooks.{event}=[{{hooks=[{{type=\"command\",command={},timeout={timeout}}}]}}]",
            toml::Value::String(format!("{command} {event}"))
        )
    });
    std::iter::once(start).chain(activity).collect()
}
/// Whether the program found under a CLI's name is that CLI. `pi` is a name
/// other programs have, and one of those is not given an extension.
fn is_agent(provider: AgentKind, executable: &std::path::Path) -> bool {
    provider != AgentKind::Pi
        || executable
            .canonicalize()
            .is_ok_and(|path| path.to_string_lossy().contains("pi-coding-agent"))
}
/// OpenCode's configuration for one launch with Neptune's plugin added to
/// the plugins it names. `None` when what the user set cannot be added to.
fn opencode_config(existing: Option<&str>, plugin: &str) -> Option<String> {
    let mut config = match existing.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => serde_json::from_str(text).ok()?,
        None => serde_json::json!({}),
    };
    let plugins = config
        .as_object_mut()?
        .entry("plugin")
        .or_insert_with(|| serde_json::json!([]));
    plugins.as_array_mut()?.push(plugin.into());
    Some(config.to_string())
}
fn resolve(provider: AgentKind) -> anyhow::Result<PathBuf> {
    let shims = std::env::var_os(SHIMS).map(PathBuf::from);
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        if shims.as_ref() == Some(&directory) {
            continue;
        }
        let candidate = directory.join(provider.executable());
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if candidate.metadata()?.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "{} is not installed or is not on PATH",
        provider.executable()
    )
}
/// Arguments that make an invocation something other than an interactive
/// session: it is passed through untouched, here and on an SSH host.
pub(super) fn batch_arguments(provider: AgentKind) -> impl Iterator<Item = &'static str> {
    let options: &[&str] = match provider {
        AgentKind::Claude => &["-p", "--print"],
        AgentKind::Codex | AgentKind::Opencode => &[],
        AgentKind::Gemini => &[
            "-p",
            "--prompt",
            "--acp",
            "--experimental-acp",
            "-l",
            "--list-extensions",
            "--list-sessions",
            "--delete-session",
        ],
        AgentKind::Pi => &["-p", "--print", "--mode", "--export", "--list-models"],
        AgentKind::Omp => &["-p", "--print", "--mode", "--export", "--alias"],
    };
    let commands: &[&str] = match provider {
        AgentKind::Claude => &[
            "auth",
            "mcp",
            "plugin",
            "install",
            "update",
            "doctor",
            "setup-token",
            "agents",
            "remote-control",
        ],
        AgentKind::Codex => &[
            "exec",
            "e",
            "review",
            "login",
            "logout",
            "mcp",
            "plugin",
            "app-server",
            "remote-control",
            "completion",
            "update",
            "doctor",
            "sandbox",
            "debug",
            "apply",
            "queue",
            "archive",
            "delete",
            "migrate-rollouts",
            "unarchive",
            "cloud",
            "exec-server",
            "features",
            "help",
            "agents",
        ],
        AgentKind::Opencode => &[
            "run",
            "serve",
            "web",
            "acp",
            "mcp",
            "completion",
            "debug",
            "providers",
            "auth",
            "agent",
            "upgrade",
            "uninstall",
            "models",
            "stats",
            "export",
            "import",
            "github",
            "session",
            "plugin",
            "plug",
            "db",
            // Their terminals are another server's or a pull request's.
            "attach",
            "pr",
        ],
        AgentKind::Gemini => &[
            "mcp",
            "extensions",
            "extension",
            "skills",
            "skill",
            "hooks",
            "hook",
            "gemma",
        ],
        AgentKind::Pi => &[
            "install",
            "remove",
            "uninstall",
            "update",
            "list",
            "config",
            "auth",
            "mcp",
        ],
        AgentKind::Omp => &[
            "acp",
            "agents",
            "auth-broker",
            "auth-gateway",
            "bench",
            "browser-relay",
            "cleanse",
            "commit",
            "completions",
            "compress",
            "config",
            "dry-balance",
            "gallery",
            "gc",
            "grep",
            "grievances",
            "install",
            "join",
            "models",
            "plugin",
            "read",
            "say",
            "search",
            "setup",
            "share",
            "shell",
            "ssh",
            "stats",
            "tiny-models",
            "token",
            "ttsr",
            "update",
            "usage",
            "worktree",
        ],
    };
    [
        "--help",
        "-h",
        "--version",
        "-V",
        "-v",
        "--remote",
        "--remote-control",
    ]
    .into_iter()
    .chain(options.iter().copied())
    .chain(commands.iter().copied())
}
/// Conservative around option-led subcommands too: never relaunch a batch job.
fn interactive(provider: AgentKind, args: &[String]) -> bool {
    !args
        .iter()
        .any(|arg| batch_arguments(provider).any(|word| word == arg))
}
/// `spawned` marks a CLI another agent started, whose replies return to it;
/// `ultracode` starts Claude Code with ultracode on.
fn run_agent(
    provider: AgentKind,
    args: &[String],
    resumed: Option<&AgentSession>,
    spawned: bool,
    ultracode: bool,
) -> anyhow::Result<i32> {
    let executable = resolve(provider)?;
    let mut command = std::process::Command::new(&executable);
    let nested = std::env::var(RUN).is_ok_and(|value| !value.is_empty());
    if nested
        || !is_agent(provider, &executable)
        || !interactive(provider, args)
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || std::env::var(ENDPOINT).is_err()
    {
        // A CLI inside another does not speak for the one that is tracked.
        command.args(args).env_remove(HOOK);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            return Err(command.exec().into());
        }
        #[cfg(not(unix))]
        {
            return Ok(command.status()?.code().unwrap_or(1));
        }
    }
    let invocation = tempfile::Builder::new()
        .prefix("neptune-agent-run-")
        .rand_bytes(32)
        .tempfile()?;
    let run = invocation
        .path()
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    drop(invocation);
    let agent = resumed.cloned().unwrap_or(AgentSession {
        kind: provider,
        session_id: None,
        cwd: std::env::current_dir()?,
    });
    if send(Event::Open { agent }, &run).is_err() {
        eprintln!("Neptune: agent session tracking is unavailable for this launch.");
    }
    let hook = format!(
        "{} --agent-hook {}",
        quote(&std::env::current_exe()?.to_string_lossy()),
        provider.executable()
    );
    let helper = std::env::current_exe()?.to_string_lossy().into_owned();
    let plugin = |name: &str| {
        PathBuf::from(std::env::var_os(SHIMS).unwrap_or_default())
            .join(PLUGINS)
            .join(name)
    };
    // Set for the CLI alone; the supervisor that runs it passes them on.
    let mut environment: Vec<(&str, String)> = Vec::new();
    match provider {
        AgentKind::Opencode => {
            // --pure and its variable turn every plugin off, this one too.
            let pure = args.iter().any(|arg| arg == "--pure")
                || std::env::var_os("OPENCODE_PURE").is_some();
            let config = opencode_config(
                std::env::var("OPENCODE_CONFIG_CONTENT").ok().as_deref(),
                &plugin("opencode.js").to_string_lossy(),
            );
            if let Some(config) = config.filter(|_| !pure) {
                environment.push(("OPENCODE_CONFIG_CONTENT", config));
                environment.push((HOOK, hook.clone()));
            }
        }
        // Oh My Pi grew from pi and takes the same extension.
        AgentKind::Pi | AgentKind::Omp => {
            command.arg("-e").arg(plugin("pi.js"));
            environment.push((HOOK, hook.clone()));
        }
        // Gemini CLI takes hooks only from its settings files, which are the
        // user's. Its title says what it is doing.
        AgentKind::Gemini => {}
        AgentKind::Claude => {
            let mut settings = serde_json::json!({
                "hooks":claude_hooks(&hook),
                "permissions":{"allow":super::agent_mcp::allowed_tools()
                    .map(|tool| format!("mcp__neptune__{tool}"))
                    .collect::<Vec<_>>()},
            });
            // Session-scoped: the user's own setting stands for other launches.
            if ultracode {
                settings["ultracode"] = true.into();
            }
            let servers = serde_json::json!({"mcpServers":{"neptune":{"command":helper,"args":["--agent-mcp"]}}});
            // --mcp-config takes several values; --settings ends its list
            // before the user's own arguments.
            command.args(["--mcp-config", &servers.to_string()]);
            command.args(["--settings", &settings.to_string()]);
        }
        AgentKind::Codex => {
            // Codex asks before it runs a hook it has not seen, and a changed
            // command is one. The helper is named through the environment so
            // the commands stay the same when this executable moves, as a
            // mounted image does at every launch.
            let hook = format!("\"${HELPER}\" --agent-hook {}", provider.executable());
            // Hook processes need this terminal's environment, not a shared daemon's.
            // Older Codex versions still open normally; they can restore the CLI only.
            let supports_local = std::process::Command::new(resolve(provider)?)
                .arg("--help")
                .output()
                .is_ok_and(|output| {
                    output.status.success()
                        && String::from_utf8_lossy(&output.stdout).contains("--no-daemon")
                });
            if supports_local {
                command.arg("--no-daemon");
                for hook in codex_hooks(&hook) {
                    command.args(["-c", &hook]);
                }
                command.args([
                    "-c",
                    &format!(
                        "mcp_servers.neptune.command={}",
                        toml::Value::String(helper.clone())
                    ),
                    "-c",
                    "mcp_servers.neptune.args=[\"--agent-mcp\"]",
                    // Waiting for a started agent outlasts Codex's minute.
                    "-c",
                    &format!(
                        "mcp_servers.neptune.tool_timeout_sec={}",
                        super::agent_mcp::MAX_WAIT.as_secs() + 30
                    ),
                ]);
                // Codex excludes *TOKEN* from the default tool/hook environment.
                // Supply only this pane's bridge metadata as invocation-scoped overrides;
                // leave the user's other environment filters and hook trust intact.
                for (name, value) in [
                    (TOKEN, std::env::var(TOKEN).unwrap_or_default()),
                    (ENDPOINT, std::env::var(ENDPOINT).unwrap_or_default()),
                    (RUN, run.clone()),
                    (HELPER, helper.clone()),
                ]
                .into_iter()
                .chain(spawned.then(|| (SPAWNED, "1".to_owned())))
                {
                    let value = toml::Value::String(value);
                    command.args([
                        "-c",
                        &format!("shell_environment_policy.set.{name}={value}"),
                        "-c",
                        &format!("mcp_servers.neptune.env.{name}={value}"),
                    ]);
                }
            }
        }
    }
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The supervisor catches terminal interrupts while its child retains normal
        // signal handling. A Rust status() parent would die on Ctrl+C even when the
        // interactive agent handles it, leaving a second foreground reader behind.
        let mut supervisor = std::process::Command::new("/bin/sh");
        supervisor.args(["-c", "helper=$1; resumed=$2; shift 2; trap ':' INT QUIT; \"$@\"; result=$?; \"$helper\" --agent-close \"$result\" \"$resumed\"; exit \"$result\"", "neptune-agent"])
            .arg(std::env::current_exe()?).arg(if resumed.is_some() { "1" } else { "0" })
            .arg(command.get_program()).args(command.get_args()).env(RUN, &run).env(HELPER, &helper)
            .envs(environment);
        if spawned {
            supervisor.env(SPAWNED, "1");
        } else {
            supervisor.env_remove(SPAWNED);
        }
        if let Some(agent) = resumed {
            supervisor.current_dir(&agent.cwd);
        }
        Err(supervisor.exec().into())
    }
    #[cfg(not(unix))]
    {
        if let Some(agent) = resumed {
            command.current_dir(&agent.cwd);
        }
        if spawned {
            command.env(SPAWNED, "1");
        }
        let result = command
            .env(RUN, &run)
            .env(HELPER, &helper)
            .envs(environment)
            .status();
        let _ = send(Event::Close, &run);
        Ok(result?.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn agent(id: &str) -> AgentSession {
        AgentSession {
            kind: AgentKind::Claude,
            session_id: Some(id.into()),
            cwd: std::env::temp_dir(),
        }
    }
    const FIRST: &str = "019a1234-5678-7000-8000-123456789abc";
    const SECOND: &str = "019a1234-5678-7000-8000-123456789def";
    fn hook(event: &str) -> Hook {
        Hook {
            hook_event_name: event.into(),
            ..Hook::default()
        }
    }
    fn tool(event: &str, tool: &str) -> Hook {
        Hook {
            tool_name: Some(tool.into()),
            ..hook(event)
        }
    }
    fn ask(attention: Attention, tool: Option<&str>, subagent: bool) -> Signal {
        Signal::Ask {
            attention,
            tool: tool.map(str::to_owned),
            subagent,
        }
    }
    fn done(tool: Option<&str>, subagent: bool) -> Signal {
        Signal::ToolDone {
            tool: tool.map(str::to_owned),
            subagent,
        }
    }
    #[test]
    fn hooks_name_the_moment_and_never_what_was_typed_or_run() {
        use AgentKind::{Claude, Codex};
        // Real payloads carry prompts, commands and results; none is read.
        let payload = serde_json::json!({
            "session_id": FIRST, "cwd": "/tmp", "hook_event_name": "PostToolUse",
            "tool_name": "Bash", "tool_input": {"command": "cat secret"},
            "tool_response": {"stdout": "hunter2"}, "prompt": "my prompt",
            "permission_mode": "default", "transcript_path": "/tmp/t.jsonl",
        });
        let parsed: Hook = serde_json::from_value(payload).unwrap();
        let sent = serde_json::to_string(&signal(Claude, &parsed).unwrap()).unwrap();
        assert_eq!(
            sent,
            r#"{"signal":"tool_done","tool":"Bash","subagent":false}"#
        );

        for provider in [Claude, Codex] {
            assert_eq!(
                signal(provider, &hook("UserPromptSubmit")),
                Some(Signal::Prompt)
            );
            assert_eq!(
                signal(provider, &tool("PreToolUse", "Bash")),
                Some(Signal::Tool)
            );
            assert_eq!(
                signal(provider, &tool("PermissionRequest", "Bash")),
                Some(ask(Attention::Permission, Some("Bash"), false))
            );
            assert_eq!(
                signal(provider, &tool("PostToolUse", "Bash")),
                Some(done(Some("Bash"), false))
            );
            assert_eq!(signal(provider, &hook("Stop")), Some(Signal::Done));
            assert_eq!(signal(provider, &hook("SessionStart")), Some(Signal::Done));
            assert_eq!(signal(provider, &hook("SubagentStop")), None);
            assert_eq!(signal(provider, &hook("SessionEnd")), None);
        }
        assert_eq!(signal(Codex, &hook("Interrupt")), Some(Signal::Done));
        assert_eq!(signal(Claude, &hook("StopFailure")), Some(Signal::Done));
        assert_eq!(
            signal(Claude, &tool("PostToolUseFailure", "Bash")),
            Some(done(Some("Bash"), false))
        );
        // Questions and plans ask through a tool, whatever the permission mode.
        for (provider, name, attention) in [
            (Claude, "AskUserQuestion", Attention::Question),
            (Claude, "ExitPlanMode", Attention::Plan),
            (Codex, "request_user_input", Attention::Question),
        ] {
            for event in ["PreToolUse", "PermissionRequest"] {
                let mut asked = tool(event, name);
                asked.permission_mode = Some("bypassPermissions".into());
                assert_eq!(
                    signal(provider, &asked),
                    Some(ask(attention, Some(name), false)),
                    "{event} {name}"
                );
            }
        }
        // Another agent's question tool is an ordinary tool here.
        assert_eq!(
            signal(Codex, &tool("PreToolUse", "AskUserQuestion")),
            Some(Signal::Tool)
        );
        // Nothing is asked of a person when permissions are bypassed.
        let mut bypassed = tool("PermissionRequest", "Bash");
        bypassed.permission_mode = Some("bypassPermissions".into());
        assert_eq!(signal(Claude, &bypassed), None);
        // A compaction restarts the session mid-turn; one asked for ends quietly.
        let mut compact = hook("SessionStart");
        compact.source = Some("compact".into());
        assert_eq!(signal(Claude, &compact), None);
        let mut manual = hook("PostCompact");
        assert_eq!(signal(Claude, &manual), None);
        manual.trigger = Some("manual".into());
        assert_eq!(signal(Claude, &manual), Some(Signal::Done));
        for (kind, expected) in [
            (
                "permission_prompt",
                Some(ask(Attention::Permission, None, false)),
            ),
            (
                "elicitation_dialog",
                Some(ask(Attention::Input, None, false)),
            ),
            (
                "agent_needs_input",
                Some(ask(Attention::Input, None, false)),
            ),
            ("elicitation_complete", Some(Signal::Answered)),
            ("idle_prompt", None),
            ("auth_success", None),
        ] {
            let mut notice = hook("Notification");
            notice.notification_type = Some(kind.into());
            assert_eq!(signal(Claude, &notice), expected, "{kind}");
        }
        assert_eq!(signal(Claude, &hook("Notification")), None);
        // A subagent's turn is not the pane's; only its requests reach a person.
        let inside = |mut hook: Hook| {
            hook.agent_id = Some("agent-1".into());
            hook
        };
        for event in ["UserPromptSubmit", "Stop", "SessionStart"] {
            assert_eq!(signal(Claude, &inside(hook(event))), None, "{event}");
        }
        assert_eq!(signal(Claude, &inside(tool("PreToolUse", "Bash"))), None);
        assert_eq!(
            signal(Claude, &inside(tool("PermissionRequest", "Bash"))),
            Some(ask(Attention::Permission, Some("Bash"), true))
        );
        assert_eq!(
            signal(Claude, &inside(tool("PostToolUse", "Bash"))),
            Some(done(Some("Bash"), true))
        );
        // A name too long to be a tool's is not carried.
        let long = "x".repeat(MAX_TOOL + 1);
        assert_eq!(
            signal(Claude, &tool("PostToolUse", &long)),
            Some(done(None, false))
        );
    }
    #[test]
    fn activity_follows_the_turn_and_a_request_stands_until_it_is_resolved() {
        use Activity::{Idle, NeedsInput, Working};
        let mut tracked = Tracked::idle();
        let mut step = |signal: Signal, changed: bool, state: Activity| {
            assert_eq!(tracked.apply(signal.clone()), changed, "{signal:?}");
            assert_eq!(tracked.state, state, "{signal:?}");
        };
        step(Signal::Prompt, true, Working);
        step(Signal::Tool, false, Working);
        step(done(Some("Read"), false), false, Working);
        // A turn boundary is reported even when it changes nothing.
        step(Signal::Prompt, true, Working);
        step(
            ask(Attention::Permission, Some("Bash"), false),
            true,
            NeedsInput(Attention::Permission),
        );
        // Tools of the same batch start and finish around the request.
        step(Signal::Tool, false, NeedsInput(Attention::Permission));
        step(
            done(Some("Read"), false),
            false,
            NeedsInput(Attention::Permission),
        );
        // A late notice of the same request does not rename it.
        step(
            ask(Attention::Input, None, false),
            false,
            NeedsInput(Attention::Permission),
        );
        step(done(Some("Bash"), false), true, Working);
        step(Signal::Done, true, Idle);
        step(Signal::Done, true, Idle);
        step(Signal::Answered, false, Idle);
        // A question asked twice, by the tool and by its permission request.
        step(Signal::Prompt, true, Working);
        step(
            ask(Attention::Question, Some("AskUserQuestion"), false),
            true,
            NeedsInput(Attention::Question),
        );
        step(
            ask(Attention::Question, Some("AskUserQuestion"), false),
            true,
            NeedsInput(Attention::Question),
        );
        step(done(Some("AskUserQuestion"), false), true, Working);
        // A request without a tool is answered by any tool finishing, or by word.
        step(
            ask(Attention::Input, None, false),
            true,
            NeedsInput(Attention::Input),
        );
        step(Signal::Answered, true, Working);
        step(
            ask(Attention::Input, None, false),
            true,
            NeedsInput(Attention::Input),
        );
        step(done(None, false), true, Working);
        // Ending the turn clears a request nobody answered.
        step(
            ask(Attention::Plan, Some("ExitPlanMode"), false),
            true,
            NeedsInput(Attention::Plan),
        );
        step(Signal::Done, true, Idle);
        // A subagent working in the background leaves the pane at rest, and
        // its request gives way to what it interrupted.
        step(done(Some("Read"), true), false, Idle);
        step(
            ask(Attention::Permission, Some("Bash"), true),
            true,
            NeedsInput(Attention::Permission),
        );
        step(done(Some("Bash"), true), true, Idle);
        step(Signal::Prompt, true, Working);
        step(
            ask(Attention::Permission, Some("Bash"), true),
            true,
            NeedsInput(Attention::Permission),
        );
        // The same tool finishing for the lead is not the subagent's answer,
        step(
            done(Some("Bash"), false),
            false,
            NeedsInput(Attention::Permission),
        );
        step(done(Some("Bash"), true), true, Working);
        // nor a subagent's the lead's.
        step(
            ask(Attention::Permission, Some("Bash"), false),
            true,
            NeedsInput(Attention::Permission),
        );
        step(
            done(Some("Bash"), true),
            false,
            NeedsInput(Attention::Permission),
        );
        step(done(Some("Bash"), false), true, Working);
    }
    #[test]
    fn activity_is_bound_to_the_open_invocation_and_leaves_with_it() {
        let bridge = AgentBridge::default();
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let wake = woken.clone();
        bridge.shared.lock().unwrap().slots.insert(
            PaneId::new(1),
            Slot {
                generation: 7,
                token: "one".into(),
                run: None,
                remote: false,
                kind: None,
                conversation: None,
                prompted: false,
                activity: None,
                shown: None,
                since: Instant::now(),
                wake: Arc::new(move || {
                    wake.fetch_add(1, Ordering::Relaxed);
                }),
                _startup: None,
            },
        );
        let emit = |token: &str, run: &str, event| {
            apply_message(
                &bridge.shared,
                Message {
                    token: token.into(),
                    run: run.into(),
                    event,
                },
            )
        };
        let activity = |signal| Event::Activity { signal };
        let woke = || woken.swap(0, Ordering::Relaxed);
        // Nothing is open yet.
        assert!(!emit("one", "a", activity(Signal::Prompt)));
        assert!(bridge.drain_activity().is_empty());
        assert!(emit(
            "one",
            "a",
            Event::Open {
                agent: agent(FIRST)
            }
        ));
        assert_eq!(
            bridge.drain_activity(),
            [(PaneId::new(1), 7, Some(Activity::Idle), None)]
        );
        woke();
        // A stranger and another invocation do not describe this pane.
        assert!(!emit("two", "a", activity(Signal::Prompt)));
        assert!(!emit("one", "b", activity(Signal::Prompt)));
        assert_eq!(woke(), 0);
        // Reports between frames coalesce to the latest.
        assert!(emit("one", "a", activity(Signal::Prompt)));
        assert!(emit("one", "a", activity(Signal::Tool)));
        assert_eq!(woke(), 1, "a tool within a working turn wakes nothing");
        assert!(emit(
            "one",
            "a",
            activity(ask(Attention::Question, Some("AskUserQuestion"), false))
        ));
        assert_eq!(
            bridge.drain_activity(),
            [(
                PaneId::new(1),
                7,
                Some(Activity::NeedsInput(Attention::Question)),
                None
            )]
        );
        assert!(bridge.drain_activity().is_empty());
        // A new conversation in the same run keeps what the agent is doing.
        assert!(emit(
            "one",
            "a",
            Event::Session {
                agent: agent(SECOND)
            }
        ));
        assert!(bridge.drain_activity().is_empty());
        assert!(emit("one", "a", Event::Close));
        assert_eq!(bridge.drain_activity(), [(PaneId::new(1), 7, None, None)]);
        assert!(!emit("one", "a", activity(Signal::Done)), "a late hook");
        // Leaving and returning within a frame is a fresh agent at rest.
        assert!(!emit("one", "a", activity(Signal::Prompt)));
        assert!(emit(
            "one",
            "c",
            Event::Open {
                agent: agent(FIRST)
            }
        ));
        assert!(emit("one", "c", activity(Signal::Prompt)));
        assert!(emit("one", "c", Event::Close));
        assert!(emit(
            "one",
            "d",
            Event::Open {
                agent: agent(FIRST)
            }
        ));
        assert_eq!(
            bridge.drain_activity(),
            [(PaneId::new(1), 7, Some(Activity::Idle), None)]
        );
        // A resume that failed keeps the reference but is no running agent.
        bridge.drain();
        assert!(!emit("one", "c", Event::Stopped), "an earlier invocation");
        assert!(emit("one", "d", Event::Stopped));
        assert_eq!(bridge.drain_activity(), [(PaneId::new(1), 7, None, None)]);
        assert!(bridge.drain().is_empty(), "the reference is untouched");
        assert!(!emit("one", "d", activity(Signal::Prompt)));
        assert!(emit(
            "one",
            "e",
            Event::Open {
                agent: agent(FIRST)
            }
        ));
        bridge.drain_activity();
        // What has not reached a frame leaves with its pane.
        assert!(emit("one", "e", activity(Signal::Prompt)));
        bridge.close(PaneId::new(1));
        assert!(bridge.drain_activity().is_empty());
    }
    #[test]
    fn unknown_activity_is_refused_rather_than_guessed() {
        for message in [
            r#"{"token":"t","run":"r","event":{"event":"activity","signal":{"signal":"ask","attention":"plan","tool":null,"subagent":false,"text":"x"}}}"#,
            r#"{"token":"t","run":"r","event":{"event":"activity","signal":{"signal":"typing"}}}"#,
            r#"{"token":"t","run":"r","event":{"event":"activity","signal":{"signal":"ask","attention":"urgent","tool":null,"subagent":false}}}"#,
        ] {
            assert!(
                serde_json::from_str::<Message>(message).is_err(),
                "{message}"
            );
        }
        let message = r#"{"token":"t","run":"r","event":{"event":"activity","signal":{"signal":"ask","attention":"plan","tool":"ExitPlanMode","subagent":false}}}"#;
        assert!(serde_json::from_str::<Message>(message).is_ok());
    }
    #[test]
    fn pull_requests_link_only_to_the_pane_of_the_invocation_that_is_open() {
        let bridge = AgentBridge::default();
        bridge.shared.lock().unwrap().slots.insert(
            PaneId::new(1),
            Slot {
                generation: 7,
                token: "one".into(),
                run: None,
                remote: false,
                kind: None,
                conversation: None,
                prompted: false,
                activity: None,
                shown: None,
                since: Instant::now(),
                wake: Arc::new(|| {}),
                _startup: None,
            },
        );
        let emit = |token: &str, run: &str, event| {
            apply_message(
                &bridge.shared,
                Message {
                    token: token.into(),
                    run: run.into(),
                    event,
                },
            )
        };
        let link = |url: &str| Event::PullRequest { url: url.into() };
        let url = "https://github.com/zevem/neptune/pull/83";
        // No agent is open yet; then a stranger, an exited run and a non-address.
        assert!(!emit("one", "a", link(url)));
        assert!(emit(
            "one",
            "a",
            Event::Open {
                agent: agent(FIRST)
            }
        ));
        assert!(!emit("two", "a", link(url)));
        assert!(!emit("one", "b", link(url)));
        assert!(!emit("one", "a", link("https://github.com/zevem/neptune")));
        assert!(emit("one", "a", link(url)));
        let links = bridge.drain_links();
        assert_eq!(links.len(), 1);
        assert_eq!((links[0].0, links[0].1), (PaneId::new(1), 7));
        assert_eq!(links[0].2.url(), url);
        assert!(bridge.drain_links().is_empty());
        // A file is attached under the same rule, by an absolute path.
        let file = |path: &std::path::Path| Event::Attachment {
            path: path.into(),
            title: Some("After".into()),
        };
        let shot = std::env::temp_dir().join("after.png");
        assert!(!emit("two", "a", file(&shot)));
        assert!(!emit("one", "b", file(&shot)));
        assert!(!emit("one", "a", file(std::path::Path::new("after.png"))));
        assert!(emit("one", "a", file(&shot)));
        let files = bridge.drain_attachments();
        assert_eq!(files.len(), 1);
        assert_eq!((files[0].0, files[0].1), (PaneId::new(1), 7));
        assert_eq!(
            (files[0].2.path(), files[0].2.title()),
            (&*shot, Some("After"))
        );
        // What has not reached a frame stays with its conversation: the same
        // one named again keeps it, another starts without it.
        assert!(emit("one", "a", link(url)));
        assert!(emit("one", "a", file(&shot)));
        let session = |id| Event::Session { agent: agent(id) };
        assert!(emit("one", "a", session(FIRST)));
        assert!(emit("one", "a", session(SECOND)));
        assert!(bridge.drain_links().is_empty());
        assert!(bridge.drain_attachments().is_empty());
        assert!(emit("one", "a", link(url)));
        assert!(emit("one", "a", session(SECOND)));
        assert_eq!(bridge.drain_links().len(), 1);
        // And it leaves with its pane.
        assert!(emit("one", "a", link(url)));
        assert!(emit("one", "a", file(&shot)));
        bridge.close(PaneId::new(1));
        assert!(bridge.drain_links().is_empty());
        assert!(bridge.drain_attachments().is_empty());
    }
    #[test]
    fn codex_ends_a_conversation_by_starting_its_tool_server_between_turns() {
        let bridge = AgentBridge::default();
        bridge.shared.lock().unwrap().slots.insert(
            PaneId::new(1),
            Slot {
                generation: 7,
                token: "one".into(),
                run: None,
                remote: false,
                kind: None,
                conversation: None,
                prompted: false,
                activity: None,
                shown: None,
                since: Instant::now(),
                wake: Arc::new(|| {}),
                _startup: None,
            },
        );
        let emit = |token: &str, run: &str, event| {
            apply_message(
                &bridge.shared,
                Message {
                    token: token.into(),
                    run: run.into(),
                    event,
                },
            )
        };
        let named = |id: &str| AgentSession {
            kind: AgentKind::Codex,
            ..agent(id)
        };
        let link = |url: &str| Event::PullRequest { url: url.into() };
        let url = "https://github.com/zevem/neptune/pull/83";
        assert!(emit(
            "one",
            "a",
            Event::Open {
                agent: named(SECOND)
            }
        ));
        // Codex opens another conversation by starting its tool server
        // again between turns, and names it with the next prompt.
        let turn = |signal| Event::Activity { signal };
        assert!(emit("one", "a", Event::Serving), "before any prompt");
        assert_eq!(bridge.drain(), [(PaneId::new(1), 7, Some(named(SECOND)))]);
        assert!(emit("one", "a", turn(Signal::Prompt)));
        assert!(emit("one", "a", link(url)));
        assert!(emit("one", "a", Event::Serving), "a server started in a turn");
        assert_eq!(bridge.drain_links().len(), 1);
        assert!(emit("one", "a", turn(Signal::Done)));
        assert!(emit("one", "a", link(url)));
        assert!(!emit("one", "b", Event::Serving));
        assert!(emit("one", "a", Event::Serving));
        assert!(bridge.drain_links().is_empty());
        let unnamed = AgentSession {
            session_id: None,
            ..named(SECOND)
        };
        assert_eq!(bridge.drain(), [(PaneId::new(1), 7, Some(unnamed))]);
        assert!(emit("one", "a", Event::Serving));
        assert!(bridge.drain().is_empty());
        // A prompt in the conversation it resumed names it again.
        assert!(emit("one", "a", Event::Session { agent: named(FIRST) }));
        assert!(emit("one", "a", Event::Session { agent: named(FIRST) }));
        assert_eq!(bridge.drain(), [(PaneId::new(1), 7, Some(named(FIRST)))]);
        assert!(bridge.drain().is_empty());
    }
    #[test]
    fn hooks_are_bound_to_pane_generation_and_invocation_and_coalesced() {
        let bridge = AgentBridge::default();
        for (id, token) in [(1, "one"), (2, "two")] {
            bridge.shared.lock().unwrap().slots.insert(
                PaneId::new(id),
                Slot {
                    generation: 7,
                    token: token.into(),
                    run: None,
                    remote: false,
                    kind: None,
                    conversation: None,
                    prompted: false,
                    activity: None,
                    shown: None,
                    since: Instant::now(),
                    wake: Arc::new(|| {}),
                    _startup: None,
                },
            );
        }
        let emit = |token: &str, run: &str, event| {
            apply_message(
                &bridge.shared,
                Message {
                    token: token.into(),
                    run: run.into(),
                    event,
                },
            )
        };
        emit(
            "one",
            "a",
            Event::Open {
                agent: agent(FIRST),
            },
        );
        emit(
            "two",
            "b",
            Event::Open {
                agent: agent(SECOND),
            },
        );
        emit(
            "one",
            "a",
            Event::Session {
                agent: agent(SECOND),
            },
        );
        let changes = bridge.drain();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0], (PaneId::new(1), 7, Some(agent(SECOND))));
        emit("wrong", "a", Event::Close);
        emit("one", "old", Event::Close);
        assert!(bridge.drain().is_empty());
        emit("one", "a", Event::Close);
        emit(
            "one",
            "a",
            Event::Session {
                agent: agent(FIRST),
            },
        );
        assert_eq!(bridge.drain(), vec![(PaneId::new(1), 7, None)]);
        bridge.close_generation(PaneId::new(2), 6);
        emit(
            "two",
            "b",
            Event::Session {
                agent: agent(FIRST),
            },
        );
        assert_eq!(
            bridge.drain(),
            vec![(PaneId::new(2), 7, Some(agent(FIRST)))]
        );
        bridge.close_generation(PaneId::new(2), 7);
        emit(
            "two",
            "b",
            Event::Session {
                agent: agent(FIRST),
            },
        );
        assert!(bridge.drain().is_empty());
    }
    /// A bridge with an unopened terminal for each token, at generation 7.
    fn bridge_with(tokens: &[&str]) -> (AgentBridge, Arc<std::sync::atomic::AtomicUsize>) {
        let bridge = AgentBridge::default();
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for (index, token) in tokens.iter().enumerate() {
            let wake = woken.clone();
            bridge.shared.lock().unwrap().slots.insert(
                PaneId::new(index as u64 + 1),
                Slot {
                    generation: 7,
                    token: (*token).into(),
                    run: None,
                    remote: false,
                    kind: None,
                    conversation: None,
                    prompted: false,
                    activity: None,
                    shown: None,
                    since: Instant::now(),
                    wake: Arc::new(move || {
                        wake.fetch_add(1, Ordering::Relaxed);
                    }),
                    _startup: None,
                },
            );
        }
        (bridge, woken)
    }
    fn message(token: &str, run: &str, event: Event) -> Message {
        Message {
            token: token.into(),
            run: run.into(),
            event,
        }
    }
    fn codex() -> AgentSession {
        AgentSession {
            kind: AgentKind::Codex,
            ..agent(SECOND)
        }
    }
    #[test]
    fn an_agent_starts_another_and_hears_only_from_the_agents_it_started() {
        let ask_signal = ask;
        let (bridge, woken) = bridge_with(&["one", "two", "three"]);
        let (parent, child) = (PaneId::new(1), PaneId::new(2));
        let emit = |token: &str, run: &str, event| {
            apply_message(&bridge.shared, message(token, run, event))
        };
        let ask =
            |token: &str, run: &str, event| answer(&bridge.shared, message(token, run, event));
        let refused = |answer: Answer| matches!(answer, Answer::Refused { .. });
        let spawn = || Event::Spawn {
            kind: AgentKind::Codex,
            prompt: "Build the API".into(),
            cwd: std::env::temp_dir(),
            model: Some("gpt-5.1-codex".into()),
            effort: Some("High".into()),
            ultra: false,
        };
        let collect = |token: &str, run: &str, take| match ask(
            token,
            run,
            Event::Collect { agent: None, take },
        ) {
            Answer::Agents { agents } => agents,
            other => panic!("{other:?}"),
        };
        let one = |take| collect("one", "a", take).remove(0);
        // What the hooks and the title last said is older than any wait.
        let age = |pane: PaneId| {
            let mut shared = bridge.shared.lock().unwrap();
            let past = Instant::now() - Duration::from_secs(30);
            shared.slots.get_mut(&pane).unwrap().since = past;
            shared.children.get_mut(&pane).unwrap().marked = past;
        };

        // Only an open agent starts another, with a task and a directory.
        assert!(refused(ask("one", "a", spawn())));
        assert!(emit(
            "one",
            "a",
            Event::Open {
                agent: agent(FIRST)
            }
        ));
        assert!(refused(ask("one", "b", spawn())));
        assert!(refused(ask(
            "one",
            "a",
            Event::Spawn {
                kind: AgentKind::Codex,
                prompt: " ".into(),
                cwd: std::env::temp_dir(),
                model: None,
                effort: None,
                ultra: false,
            }
        )));
        // A model is a name, never an option for the CLI.
        assert!(refused(ask(
            "one",
            "a",
            Event::Spawn {
                kind: AgentKind::Codex,
                prompt: "x".into(),
                cwd: std::env::temp_dir(),
                model: Some("--dangerously-bypass-approvals-and-sandbox".into()),
                effort: None,
                ultra: false,
            }
        )));
        // Each CLI has its own effort levels; "ultra" is ultracode for one
        // and the highest effort for the other.
        let level = |kind, effort, ultra| effort_level(kind, effort, ultra);
        assert_eq!(
            level(AgentKind::Claude, Some("xhigh"), true),
            Ok((Some("xhigh".into()), true))
        );
        assert_eq!(level(AgentKind::Claude, None, true), Ok((None, true)));
        assert_eq!(
            level(AgentKind::Codex, None, true),
            Ok((Some("ultra".into()), false))
        );
        assert_eq!(
            level(AgentKind::Codex, Some("ultra"), false),
            Ok((Some("ultra".into()), false))
        );
        assert_eq!(level(AgentKind::Codex, Some(" "), false), Ok((None, false)));
        for (kind, effort, ultra) in [
            (AgentKind::Claude, "ultra", false),
            (AgentKind::Claude, "minimal", false),
            (AgentKind::Claude, "--max", false),
            (AgentKind::Codex, "low", true),
            (AgentKind::Codex, "\"; rm", false),
        ] {
            assert!(level(kind, Some(effort), ultra).is_err());
        }
        woken.store(0, Ordering::Relaxed);
        let Answer::Queued { request } = ask("one", "a", spawn()) else {
            panic!("the spawn was not queued");
        };
        assert_eq!(woken.swap(0, Ordering::Relaxed), 1);
        assert_eq!(
            ask("one", "a", Event::SpawnResult { request }),
            Answer::Pending
        );
        let mut requests = bridge.drain_requests();
        let Some(AgentRequest::Spawn {
            parent: asked,
            generation: 7,
            task,
            ..
        }) = requests.pop()
        else {
            panic!("the application was not asked");
        };
        assert_eq!(asked, parent);
        assert_eq!(
            (task.kind, task.model.as_deref(), &task.resume),
            (AgentKind::Codex, Some("gpt-5.1-codex"), &None)
        );
        assert_eq!(
            (task.effort.as_deref(), task.ultracode),
            (Some("high"), false)
        );
        let prompt = task.prompt.clone();
        // The task says who handed it over and how the answer returns.
        assert!(prompt.starts_with("[Claude Code in another Neptune terminal started you"));
        assert!(prompt.ends_with("\n\nBuild the API"));
        bridge.register_spawn(child, parent, task);
        bridge.spawn_result(request, Ok(child));
        assert_eq!(
            ask("one", "a", Event::SpawnResult { request }),
            Answer::Spawned { agent: 2 }
        );
        assert!(refused(ask("one", "a", Event::SpawnResult { request })));

        // Until its CLI takes the task it is starting, and takes no message.
        // One that stays that way is news once: a person has to look.
        assert!(one(false).status == Status::Starting && !one(false).news);
        age(child);
        let stalled = one(true);
        assert!(stalled.news && stalled.status == Status::Starting);
        assert!(!one(false).news);
        let tell = |text: &str| {
            ask(
                "one",
                "a",
                Event::Tell {
                    agent: 2,
                    text: text.into(),
                },
            )
        };
        assert!(refused(tell("hello")));
        assert!(!bridge.takes_prompt(child, 7));
        // What its terminal shows once it stands still is news, and only
        // then are keys pressed for it: a few named ones.
        let press = |keys: &[&str]| {
            ask(
                "one",
                "a",
                Event::Press {
                    agent: 2,
                    keys: keys.iter().map(|key| (*key).to_owned()).collect(),
                },
            )
        };
        assert!(refused(press(&["enter"])));
        bridge.asking(child, 6, Some("stale".into()));
        assert_eq!(one(false).status, Status::Starting);
        bridge.asking(child, 7, Some("Trust this folder?".into()));
        let asking = Status::Asking {
            screen: "Trust this folder?".into(),
        };
        assert!(one(false).news && one(true).status == asking);
        assert!(!one(false).news);
        assert!(refused(tell("hello")));
        assert!(refused(press(&[])));
        assert!(refused(press(&["rm -rf"])));
        assert!(refused(press(&["ctrl+c"])));
        assert_eq!(press(&["1", "Enter"]), Answer::Done);
        assert_eq!(
            bridge.drain_requests(),
            [AgentRequest::Press {
                pane: child,
                generation: 7,
                keys: vec![Press::Char('1'), Press::Enter],
            }]
        );
        assert_eq!(one(false).status, Status::Starting);
        assert_eq!(bridge.starting(), [(child, 7)]);
        // Its terminal takes the task once; a file sourced again finds none.
        assert_eq!(
            ask("two", "", Event::Launch),
            Answer::Launch {
                kind: AgentKind::Codex,
                prompt,
                model: Some("gpt-5.1-codex".into()),
                effort: Some("high".into()),
                ultracode: false,
                resume: None,
            }
        );
        assert!(refused(ask("two", "", Event::Launch)));
        assert!(refused(ask("three", "", Event::Launch)));
        assert!(emit("two", "c", Event::Open { agent: codex() }));
        assert_eq!(one(false).status, Status::Starting);
        let signal = |signal| Event::Activity { signal };
        assert!(emit("two", "c", signal(Signal::Prompt)));
        assert_eq!(one(false).status, Status::Working);
        assert!(bridge.takes_prompt(child, 7) && !bridge.takes_prompt(child, 6));

        // A message for a working agent is typed for it all the same.
        assert_eq!(tell("Also add tests"), Answer::Told { working: true });
        assert_eq!(
            bridge.drain_requests(),
            [AgentRequest::Tell {
                pane: child,
                generation: 7,
                text: "Also add tests".into()
            }]
        );
        // Words before the end of its turn are news while it works on.
        let report = |text: &str, done| {
            emit(
                "two",
                "c",
                Event::Report {
                    text: text.into(),
                    done,
                },
            )
        };
        assert!(report("Which port?", false));
        let peek = one(false);
        assert!(peek.news && peek.replies.is_empty() && peek.status == Status::Working);
        assert_eq!(one(true).replies, ["Which port?"]);
        assert!(!one(false).news);
        // The end of a turn brings its last message. The turn of the message
        // handed over meanwhile is still owed, so the agent is not at rest.
        assert!(report("Built 3 endpoints.", true));
        assert_eq!(one(false).status, Status::Working);
        assert!(emit("two", "c", signal(Signal::Prompt)));
        assert!(report("Tests added.", true));
        let done = one(true);
        assert_eq!(done.status, Status::Idle);
        assert_eq!(done.replies, ["Built 3 endpoints.", "Tests added."]);
        assert!(done.news && !one(false).news);
        // A message folded into the turn before it is not waited for forever.
        assert_eq!(tell("One more thing"), Answer::Told { working: false });
        assert_eq!(bridge.drain_requests().len(), 1);
        assert_eq!(one(false).status, Status::Working);
        age(child);
        let rested = one(true);
        assert!(rested.news && rested.status == Status::Idle);

        // A request to a person is one once it has stood a moment; nothing
        // is typed over it.
        assert!(emit("two", "c", signal(Signal::Prompt)));
        assert!(emit(
            "two",
            "c",
            signal(ask_signal(Attention::Permission, Some("Bash"), false))
        ));
        assert_eq!(one(false).status, Status::Working);
        assert!(refused(tell("hello")) && !bridge.takes_prompt(child, 7));
        age(child);
        assert_eq!(
            one(false).status,
            Status::Waiting {
                attention: Attention::Permission
            }
        );
        assert!(refused(tell("hello")));
        // Where no hook says a wait or a turn ended, the title does.
        bridge.observe(child, 6, Activity::Idle);
        assert!(matches!(one(false).status, Status::Waiting { .. }));
        bridge.observe(child, 7, Activity::Idle);
        assert_eq!(one(false).status, Status::Idle);
        assert!(emit("two", "c", signal(Signal::Prompt)));
        assert_eq!(one(false).status, Status::Working);

        // Another agent neither sees nor reaches it, and it has no agents.
        assert!(emit(
            "three",
            "x",
            Event::Open {
                agent: agent(FIRST)
            }
        ));
        assert!(collect("three", "x", false).is_empty());
        assert!(collect("two", "c", false).is_empty());
        for event in [
            Event::Collect {
                agent: Some(2),
                take: true,
            },
            Event::Tell {
                agent: 2,
                text: "hello".into(),
            },
            Event::Dismiss { agent: 2 },
        ] {
            assert!(refused(ask("three", "x", event)));
        }
        // Nor does an agent nobody started reply to anyone.
        assert!(!emit(
            "three",
            "x",
            Event::Report {
                text: "hello".into(),
                done: true
            }
        ));

        // Its exit is news with what it last said; it is forgotten once told
        // and the model has dropped the link.
        assert!(report("Leaving.", false));
        assert!(emit("two", "c", Event::Close));
        assert!(!bridge.takes_prompt(child, 7));
        bridge.sync_spawned(&[]);
        let ended = one(true);
        assert_eq!(ended.replies, ["Leaving."]);
        assert_eq!(
            ended.status,
            Status::Ended {
                reason: "its agent exited".into()
            }
        );
        assert!(refused(tell("hello")));
        // It is kept while its conversation can be opened again, and opens
        // where it worked, on the model it had.
        bridge.sync_spawned(&[]);
        assert!(ended.reopens && one(false).reopens && !one(false).news);
        assert!(bridge.starting().is_empty());
        let reopen = |agent| {
            ask(
                "one",
                "a",
                Event::Reopen {
                    agent,
                    text: "One more thing".into(),
                },
            )
        };
        assert!(refused(reopen(3)));
        assert!(refused(ask(
            "three",
            "x",
            Event::Reopen {
                agent: 2,
                text: "hello".into()
            }
        )));
        let Answer::Queued { request } = reopen(2) else {
            panic!("the reopening was not queued");
        };
        let Some(AgentRequest::Spawn { task, cwd, .. }) = bridge.drain_requests().pop() else {
            panic!("the application was not asked");
        };
        assert_eq!(cwd, codex().cwd);
        assert_eq!(
            (task.kind, task.model.as_deref(), task.resume.clone()),
            (
                AgentKind::Codex,
                Some("gpt-5.1-codex"),
                Some((child, codex().session_id.unwrap()))
            )
        );
        assert_eq!(task.effort.as_deref(), Some("high"));
        assert!(task.prompt.ends_with("\n\nOne more thing"));
        let again = PaneId::new(3);
        // The third terminal's agent gives way to the one opened again.
        assert!(emit("three", "x", Event::Close));
        bridge.register_spawn(again, parent, task);
        bridge.spawn_result(request, Ok(again));
        assert_eq!(collect("one", "a", false).len(), 1);
        assert!(matches!(
            ask("three", "", Event::Launch),
            Answer::Launch { resume: Some(session), model: Some(_), .. }
                if Some(&session) == codex().session_id.as_ref()
        ));
        assert!(refused(reopen(2)) && refused(reopen(3)));
        bridge.sync_spawned(&[]);
        one(true);
        bridge.sync_spawned(&[]);
        assert!(collect("one", "a", false).is_empty());
        assert!(emit(
            "three",
            "x",
            Event::Open {
                agent: agent(FIRST)
            }
        ));

        // The agent that started it closes one; the application is asked once.
        bridge.register_spawn(child, parent, Task::new(AgentKind::Codex, "again"));
        bridge.sync_spawned(&[(child, parent)]);
        assert_eq!(ask("one", "a", Event::Dismiss { agent: 2 }), Answer::Done);
        assert!(refused(ask("one", "a", Event::Dismiss { agent: 2 })));
        assert_eq!(
            bridge.drain_requests(),
            [AgentRequest::Close {
                pane: child,
                generation: 7,
                parent
            }]
        );
        assert!(!one(false).news);
        bridge.sync_spawned(&[]);
        assert!(collect("one", "a", false).is_empty());

        // One that cannot start says why, and the model is told it left.
        bridge.register_spawn(child, parent, Task::new(AgentKind::Codex, "again"));
        bridge.drain();
        assert_eq!(
            ask(
                "two",
                "",
                Event::LaunchFailed {
                    reason: "codex is not installed".into()
                }
            ),
            Answer::Done
        );
        assert_eq!(bridge.drain(), [(child, 7, None)]);
        assert!(matches!(
            one(true).status,
            Status::Ended { reason } if reason.contains("codex is not installed")
        ));
        bridge.sync_spawned(&[]);

        // No more agents are open at once than the model keeps.
        for pane in 10..10 + AgentSession::MAX_SPAWNED as u64 {
            bridge.register_spawn(PaneId::new(pane), parent, Task::new(AgentKind::Codex, "x"));
        }
        assert!(refused(ask("one", "a", spawn())));
        // When the agent that started them leaves, they answer to no one, and
        // a new agent in its terminal does not inherit them.
        assert!(matches!(ask("three", "x", spawn()), Answer::Queued { .. }));
        assert!(emit("one", "a", Event::Close));
        assert!(emit(
            "one",
            "b",
            Event::Open {
                agent: agent(FIRST)
            }
        ));
        assert!(collect("one", "b", false).is_empty());
        assert_eq!(
            bridge.drain(),
            [(parent, 7, None), (parent, 7, Some(agent(FIRST)))]
        );
        // A terminal that closes takes its pending requests with it.
        bridge.close(PaneId::new(3));
        assert!(bridge.drain_requests().is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn a_restored_link_is_known_before_its_agent_resumes_and_ends_with_a_restart() {
        let bridge = AgentBridge::default();
        let (parent, child) = (PaneId::new(1), PaneId::new(2));
        let prepare = |pane, generation, resume: Option<&AgentSession>, spawned_by| {
            let mut options = SessionOptions {
                shell: Some("/bin/sh".into()),
                ..Default::default()
            };
            bridge
                .prepare(
                    pane,
                    generation,
                    &mut options,
                    resume,
                    spawned_by,
                    Arc::new(|| {}),
                )
                .unwrap();
            options.args
        };
        prepare(parent, 1, Some(&agent(FIRST)), None);
        let args = prepare(child, 1, Some(&codex()), Some(parent));
        assert_eq!(
            (args[4].as_str(), args[6].as_str()),
            ("--agent-restore", "spawned")
        );
        let status = |bridge: &AgentBridge| {
            let shared = bridge.shared.lock().unwrap();
            let record = shared.children.get(&child).unwrap();
            assert_eq!((record.parent, record.kind), (parent, AgentKind::Codex));
            record.status(shared.slots.get(&child), Instant::now())
        };
        assert_eq!(status(&bridge), Status::Starting);
        // A task handed over opens the terminal with its CLI instead.
        bridge.register_spawn(PaneId::new(3), parent, Task::new(AgentKind::Claude, "task"));
        let args = prepare(PaneId::new(3), 1, None, Some(parent));
        assert_eq!(args[4..7], ["--agent-spawn", "", ""]);
        // A link without a saved agent or a task opens a plain shell.
        assert!(prepare(PaneId::new(4), 1, None, Some(parent)).is_empty());
        // Restarting the started terminal ends its agent for its starter.
        assert!(prepare(child, 2, None, None).is_empty());
        assert!(matches!(status(&bridge), Status::Ended { .. }));
    }
    #[test]
    fn only_a_started_agents_last_message_is_read_from_its_hook() {
        let payload = serde_json::json!({
            "hook_event_name": "Stop", "session_id": FIRST, "cwd": "/tmp",
            "last_assistant_message": "Done: 3 endpoints.", "transcript_path": "/tmp/t.jsonl",
        });
        let parsed: Hook = serde_json::from_value(payload).unwrap();
        assert_eq!(
            parsed.last_assistant_message.as_deref(),
            Some("Done: 3 endpoints.")
        );
        // The signal every agent sends still names the moment alone.
        let sent = serde_json::to_string(&signal(AgentKind::Codex, &parsed).unwrap()).unwrap();
        assert_eq!(sent, r#"{"signal":"done"}"#);
        // A reply longer than agents exchange is cut at a character.
        let long = "é".repeat(MAX_TEXT);
        let cut = clip(&long);
        assert!(cut.len() < MAX_TEXT + 100 && cut.contains("Neptune cut this short"));
        assert_eq!(clip("short"), "short");
    }
    fn remote(bridge: &AgentBridge, pane: u64) -> String {
        let mut options = SessionOptions {
            args: vec![
                "-t".into(),
                "--".into(),
                "devbox".into(),
                "sh -c 'x' neptune ''".into(),
            ],
            ..Default::default()
        };
        bridge
            .prepare_remote(PaneId::new(pane), 7, &mut options, Arc::new(|| {}))
            .unwrap();
        let command = options.args.pop().unwrap();
        // The credential and the installer follow the directory as data.
        let rest = command.strip_prefix("sh -c 'x' neptune '' '").unwrap();
        let (token, installer) = rest.split_once("' '").unwrap();
        assert!(token.len() == 32 && token.bytes().all(|b| b.is_ascii_alphanumeric()));
        assert!(installer.contains("_neptune_install_agents"));
        token.to_owned()
    }
    #[test]
    fn an_ssh_terminal_reports_its_agents_through_its_own_credential() {
        let bridge = AgentBridge::default();
        let pane = PaneId::new(1);
        let token = remote(&bridge, 1);
        let other = remote(&bridge, 2);
        assert_ne!(token, other);
        let report = |line: &str| bridge.report(pane, 7, line);
        let drained = || bridge.drain_activity();
        // Output cannot open an agent without this terminal's credential,
        // with another terminal's, or for a generation that is gone.
        assert!(!report("guess;run1;open;claude"));
        assert!(!report(&format!("{other};run1;open;claude")));
        assert!(!bridge.report(pane, 6, &format!("{token};run1;open;claude")));
        assert!(!report(&format!("{token};run1;open;emacs")));
        assert!(!report(&format!("{token};run1;hook;UserPromptSubmit")));
        assert!(drained().is_empty());

        assert!(report(&format!("{token};run1;open;claude")));
        assert_eq!(
            drained(),
            [(pane, 7, Some(Activity::Idle), Some(AgentKind::Claude))]
        );
        // The model is told nothing: the agent is not saved or reopened.
        assert!(bridge.drain().is_empty());
        assert!(report(&format!("{token};run1;hook;UserPromptSubmit")));
        assert_eq!(drained()[0].2, Some(Activity::Working));
        assert!(report(&format!(
            "{token};run1;hook;PermissionRequest;tool_name=Bash;permission_mode=default;later=word"
        )));
        assert_eq!(
            drained()[0].2,
            Some(Activity::NeedsInput(Attention::Permission))
        );
        // The same rules as a local hook: another tool leaves the request,
        // a subagent's tool says nothing, bypassed permissions ask no one.
        assert!(report(&format!(
            "{token};run1;hook;PostToolUse;tool_name=Read"
        )));
        assert!(report(&format!(
            "{token};run1;hook;PostToolUse;tool_name=Bash;agent_id=1"
        )));
        assert!(drained().is_empty());
        assert!(report(&format!(
            "{token};run1;hook;PostToolUse;tool_name=Bash"
        )));
        assert_eq!(drained()[0].2, Some(Activity::Working));
        assert!(report(&format!(
            "{token};run1;hook;PermissionRequest;tool_name=Bash;permission_mode=bypassPermissions"
        )));
        assert!(drained().is_empty());
        assert!(report(&format!(
            "{token};run1;hook;PreToolUse;tool_name=AskUserQuestion"
        )));
        assert_eq!(
            drained()[0].2,
            Some(Activity::NeedsInput(Attention::Question))
        );
        // A run that is not the open one describes nothing and closes nothing.
        assert!(!report(&format!("{token};run0;hook;Stop")));
        assert!(!report(&format!("{token};run0;close")));
        assert!(drained().is_empty());
        assert!(report(&format!("{token};run1;hook;Stop")));
        assert_eq!(drained()[0].2, Some(Activity::Idle));
        assert!(report(&format!("{token};run1;close")));
        assert_eq!(drained(), [(pane, 7, None, None)]);
        assert!(!report(&format!("{token};run1;hook;UserPromptSubmit")));

        // The listener never answers for an SSH terminal, whoever knows its
        // credential, and a local terminal takes no report from its output.
        assert!(!apply_message(
            &bridge.shared,
            message(
                &token,
                "run2",
                Event::Open {
                    agent: agent(FIRST)
                }
            )
        ));
        assert!(matches!(
            answer(&bridge.shared, message(&token, "", Event::Launch)),
            Answer::Refused { .. }
        ));
        let (local, _) = bridge_with(&["local"]);
        assert!(!local.report(PaneId::new(1), 7, "local;run1;open;claude"));
        // A restarted connection has a new credential.
        let again = remote(&bridge, 1);
        assert_ne!(again, token);
        assert!(!report(&format!("{token};run3;open;claude")));
    }
    #[test]
    fn opencode_and_pi_report_through_neptunes_plugins_and_gemini_by_title() {
        // The plugins send only these moments, in the hooks' own form.
        let session = "ses_ef579273dffe5vpZTGF29Kt3yQ";
        assert_eq!(
            signal(AgentKind::Omp, &tool("PermissionRequest", "bash")),
            Some(ask(Attention::Permission, Some("bash"), false))
        );
        assert_eq!(
            signal(AgentKind::Omp, &tool("PreToolUse", "ask")),
            Some(ask(Attention::Question, Some("ask"), false))
        );
        assert_eq!(
            resume_arguments(&AgentSession {
                kind: AgentKind::Omp,
                session_id: Some(FIRST.into()),
                cwd: std::env::temp_dir(),
            }),
            ["--resume", FIRST]
        );
        assert!(!interactive(AgentKind::Omp, &["commit".to_owned()]));
        assert!(interactive(AgentKind::Omp, &["fix it".to_owned()]));
        for provider in [AgentKind::Opencode, AgentKind::Pi, AgentKind::Omp] {
            assert_eq!(signal(provider, &hook("SessionStart")), None);
            assert_eq!(
                signal(provider, &hook("UserPromptSubmit")),
                Some(Signal::Prompt)
            );
            assert_eq!(signal(provider, &hook("Stop")), Some(Signal::Done));
            assert_eq!(
                signal(provider, &hook("Elicitation")),
                Some(ask(Attention::Input, None, false))
            );
            assert_eq!(
                signal(provider, &hook("ElicitationResult")),
                Some(Signal::Answered)
            );
            assert!(activity_hooks(provider).is_empty());
        }
        assert_eq!(
            signal(AgentKind::Opencode, &hook("PermissionRequest")),
            Some(ask(Attention::Permission, None, false))
        );
        assert_eq!(
            signal(AgentKind::Opencode, &tool("PreToolUse", "question")),
            Some(ask(Attention::Question, Some("question"), false))
        );
        assert!(OPENCODE_PLUGIN.contains("NEPTUNE_AGENT_HOOK") && PI_EXTENSION.contains(HOOK));
        // Each CLI is asked for a conversation in its own words.
        let resume = |kind, id: &str| {
            resume_arguments(&AgentSession {
                kind,
                session_id: Some(id.into()),
                cwd: std::env::temp_dir(),
            })
        };
        assert_eq!(resume(AgentKind::Opencode, session), ["--session", session]);
        assert_eq!(resume(AgentKind::Pi, FIRST), ["--session", FIRST]);
        assert_eq!(resume(AgentKind::Gemini, FIRST), ["--resume", FIRST]);
        // The plugin joins the user's own; what cannot be added to is left.
        assert_eq!(
            opencode_config(None, "/n/opencode.js").unwrap(),
            r#"{"plugin":["/n/opencode.js"]}"#
        );
        let merged: serde_json::Value = serde_json::from_str(
            &opencode_config(Some(r#"{"theme":"x","plugin":["mine"]}"#), "/n/opencode.js").unwrap(),
        )
        .unwrap();
        assert_eq!(merged["theme"], "x");
        assert_eq!(
            merged["plugin"],
            serde_json::json!(["mine", "/n/opencode.js"])
        );
        assert_eq!(opencode_config(Some("not json"), "/n/o.js"), None);
        assert_eq!(
            opencode_config(Some(r#"{"plugin":"mine"}"#), "/n/o.js"),
            None
        );
        // Batch jobs and administration are passed through untouched.
        let args = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        for (kind, batch, session) in [
            (AgentKind::Opencode, vec!["run", "x"], vec!["-s", session]),
            (AgentKind::Gemini, vec!["-p", "x"], vec!["-i", "fix it"]),
            (AgentKind::Pi, vec!["--mode", "rpc"], vec!["-c"]),
        ] {
            assert!(!interactive(kind, &args(&batch)), "{kind:?}");
            assert!(interactive(kind, &args(&session)), "{kind:?}");
            assert!(interactive(kind, &[]));
        }
        // Another program named `pi` is not the agent.
        let package = std::env::temp_dir().join("node_modules/pi-coding-agent/dist/cli.js");
        assert!(!is_agent(AgentKind::Pi, std::path::Path::new("/bin/sh")));
        assert!(is_agent(AgentKind::Gemini, std::path::Path::new("/bin/sh")));
        assert!(
            !is_agent(AgentKind::Pi, &package),
            "a missing file is no one's"
        );
        // Only Claude Code and Codex can be started for another agent.
        assert!(effort_level(AgentKind::Gemini, None, false).is_err());
    }
    #[test]
    fn batch_and_administrative_invocations_are_not_restored() {
        for args in [vec!["auth"], vec!["mcp"], vec!["--print"], vec!["--help"]] {
            assert!(!interactive(
                AgentKind::Claude,
                &args.into_iter().map(String::from).collect::<Vec<_>>()
            ));
        }
        assert!(interactive(AgentKind::Codex, &[]));
        assert!(interactive(
            AgentKind::Codex,
            &["resume".into(), FIRST.into()]
        ));
    }
    #[cfg(unix)]
    #[test]
    fn shell_startup_preserves_user_configuration_and_places_scoped_adapters_first() {
        use std::sync::Arc;
        use terminal_core::TerminalSession;
        for shell in ["/bin/bash", "/bin/zsh"] {
            if !std::path::Path::new(shell).is_file() {
                continue;
            }
            let root = tempfile::tempdir().unwrap();
            // Exercise the fixture's user config without host-wide interactive
            // setup (CI's compinit can prompt before the test command is read).
            std::fs::write(root.path().join(".zshenv"), "unsetopt GLOBAL_RCS\n").unwrap();
            std::fs::write(
                root.path().join(".bashrc"),
                "export PATH=/usr/bin:/bin\nexport NEPTUNE_USER_CONFIG=loaded\n",
            )
            .unwrap();
            std::fs::write(
                root.path().join(".zshrc"),
                "export PATH=/usr/bin:/bin\nexport NEPTUNE_USER_CONFIG=loaded\n",
            )
            .unwrap();
            let bridge = AgentBridge::default();
            let mut options = SessionOptions {
                shell: Some(shell.into()),
                cwd: root.path().into(),
                env: vec![
                    ("HOME".into(), root.path().to_string_lossy().into_owned()),
                    ("ZDOTDIR".into(), root.path().to_string_lossy().into_owned()),
                ],
                ..Default::default()
            };
            bridge
                .prepare(PaneId::new(1), 1, &mut options, None, None, Arc::new(|| {}))
                .unwrap();
            let session = TerminalSession::spawn(options, Arc::new(|| {})).unwrap();
            session.write(b"printf 'CONFIG=%s ADAPTER=%s\\n' \"$NEPTUNE_USER_CONFIG\" \"$(command -v claude)\"\r").unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if session.screen_text().contains("CONFIG=loaded ADAPTER=/") {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "shell {shell} startup failed: {}",
                    session.screen_text()
                );
                thread::sleep(Duration::from_millis(10));
            }
            assert!(session.screen_text().contains("neptune-agents-"));
            session.shutdown();
        }
    }
    #[cfg(unix)]
    #[test]
    fn zsh_history_stays_in_the_user_directory() {
        use std::sync::Arc;
        use terminal_core::TerminalSession;
        if !std::path::Path::new("/bin/zsh").is_file() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join(".zshenv"), "unsetopt GLOBAL_RCS\n").unwrap();
        // Stand in for macOS /etc/zshrc, which sets HISTFILE from the startup
        // ZDOTDIR before the user's .zshrc.
        std::fs::write(
            home.join(".zprofile"),
            "HISTFILE=$FIXTURE_STARTUP_DIR/.zsh_history\n",
        )
        .unwrap();
        std::fs::write(home.join(".zshrc"), "PROMPT='NEPTUNE> '\n").unwrap();
        let bridge = AgentBridge::default();
        let mut options = SessionOptions {
            cwd: home.clone(),
            env: vec![
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("SHELL".into(), "/bin/zsh".into()),
            ],
            ..Default::default()
        };
        bridge
            .prepare(PaneId::new(1), 1, &mut options, None, None, Arc::new(|| {}))
            .unwrap();
        let startup = option_env(&options, "ZDOTDIR").unwrap();
        options.env.push(("FIXTURE_STARTUP_DIR".into(), startup));
        let session = TerminalSession::spawn(options, Arc::new(|| {})).unwrap();
        session
            .write(b"print -r -- \"$HISTFILE\" > \"$HOME/histfile\"\r")
            .unwrap();
        let output = home.join("histfile");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&output).is_ok_and(|text| text.ends_with('\n')) {
            assert!(
                std::time::Instant::now() < deadline,
                "zsh startup failed: {}",
                session.screen_text()
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            format!("{}\n", home.join(".zsh_history").display())
        );
        session.shutdown();
    }
    #[cfg(unix)]
    #[test]
    fn zsh_restore_gets_the_terminal_back_from_instant_prompt() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;
        use terminal_core::TerminalSession;
        if !std::path::Path::new("/bin/zsh").is_file() {
            return;
        }
        for login in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("home");
            std::fs::create_dir(&home).unwrap();
            std::fs::write(home.join(".zshenv"), "unsetopt GLOBAL_RCS\n").unwrap();
            // Stand in for Powerlevel10k: stdio is redirected for the rest of startup
            // and returned only by its clear-instant-prompt command.
            std::fs::write(
                home.join(".zshrc"),
                "exec {fd0}<&0 {fd1}>&1 {fd2}>&2 0</dev/null 1>/dev/null 2>&1\np10k() { [[ $# == 1 && $1 == clear-instant-prompt ]] || return 1; exec 0<&$fd0 1>&$fd1 2>&$fd2 }\n",
            )
            .unwrap();
            let helper = root.path().join("helper");
            std::fs::write(
                &helper,
                "#!/bin/sh\nstdio=redirected\n[ -t 0 ] && [ -t 1 ] && [ -t 2 ] && stdio=terminal\necho \"$stdio\" > \"$HOME/restore\"\n",
            )
            .unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
            let mut options = SessionOptions {
                shell: (!login).then(|| "/bin/zsh".into()),
                cwd: home.clone(),
                env: vec![
                    ("HOME".into(), home.to_string_lossy().into_owned()),
                    ("ZDOTDIR".into(), home.to_string_lossy().into_owned()),
                    ("SHELL".into(), "/bin/zsh".into()),
                ],
                ..Default::default()
            };
            configure_shell(
                &mut options,
                root.path(),
                &helper,
                Start::Resume(&agent(FIRST), false),
            )
            .unwrap();
            let session = TerminalSession::spawn(options, Arc::new(|| {})).unwrap();
            let output = home.join("restore");
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !std::fs::read_to_string(&output).is_ok_and(|text| text.ends_with('\n')) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "zsh startup failed: {}",
                    session.screen_text()
                );
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(std::fs::read_to_string(&output).unwrap(), "terminal\n");
            session.shutdown();
        }
    }
    #[cfg(unix)]
    #[test]
    fn configured_shell_arguments_are_kept_and_follow_a_resume() {
        use std::sync::Arc;
        let bridge = AgentBridge::default();
        let mut options = SessionOptions {
            shell: Some("/bin/bash".into()),
            args: vec!["--norc".into(), "-i".into()],
            ..Default::default()
        };
        bridge
            .prepare(PaneId::new(1), 1, &mut options, None, None, Arc::new(|| {}))
            .unwrap();
        assert_eq!(options.shell.as_deref(), Some("/bin/bash"));
        assert_eq!(options.args, ["--norc", "-i"]);

        let mut options = SessionOptions {
            shell: Some("/bin/zsh".into()),
            args: vec!["-l".into()],
            ..Default::default()
        };
        bridge
            .prepare(
                PaneId::new(2),
                1,
                &mut options,
                Some(&agent(FIRST)),
                None,
                Arc::new(|| {}),
            )
            .unwrap();
        assert_eq!(options.shell.as_deref(), Some("/bin/sh"));
        // `shift 4` leaves the shell and its own arguments for `exec "$@"`.
        assert_eq!(options.args[4], "--agent-restore");
        assert_eq!(options.args[6..], ["", "/bin/zsh", "-l"]);
    }
}
