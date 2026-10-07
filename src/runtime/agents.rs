//! Scoped CLI adapters and a bounded, metadata-only local hook bridge.
//! All setup and socket/file I/O runs on startup workers or the bridge worker.
use super::agent_remote as remote;
use crate::agent_activity::{Activity, Attention};
use neptune_model::{AgentKind, AgentSession, Attachment, PaneId, Project, ProjectId, PullRequest};
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
/// An answer nobody came back for: the tool that asked gave up or was
/// cancelled. It makes room after this long,
const ABANDONED: Duration = Duration::from_secs(60);
/// and so does a request the application never answered.
const UNANSWERED: Duration = Duration::from_secs(5 * 60);
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
pub(super) const SHIMS: &str = "NEPTUNE_AGENT_SHIMS";
const RUN: &str = "NEPTUNE_AGENT_RUN";
/// This executable, for hook commands that must read the same at every launch.
const HELPER: &str = "NEPTUNE_AGENT_HELPER";
/// Set for a CLI another agent started: its replies go to that agent.
const SPAWNED: &str = "NEPTUNE_AGENT_SPAWNED";
/// What Neptune's plugin for a CLI runs at each moment, followed by its name.
const HOOK: &str = "NEPTUNE_AGENT_HOOK";
/// Set for a CLI that belongs to a project: `lead` or `member`.
const ROLE: &str = "NEPTUNE_AGENT_ROLE";
/// Notes for the application about messages that never reached an agent.
const MAX_NOTES: usize = 64;
/// The longest name a project's lead gives an agent it starts.
pub(super) const MAX_TITLE: usize = 60;
/// Where the plugins are, under the adapters' directory.
const PLUGINS: &str = "plugins";
pub(super) const OPENCODE_PLUGIN: &str = include_str!("agent-opencode.js");
pub(super) const PI_EXTENSION: &str = include_str!("agent-pi.js");
const NOT_TRACKED: &str = "Neptune is not tracking an agent in this terminal.";
/// Said to an agent that asks Claude Code for Codex's highest effort.
const ULTRA_HINT: &str = " For ultracode, set ultra to true.";
const PAUSED: &str = "This project is paused, so nothing is started or sent for it. Tell the user; they resume it in the Project tab.";

/// Who started an agent and hears from it: the agent in a terminal, or a
/// project, whose lead has no terminal.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Starter {
    Pane(PaneId),
    Project(ProjectId),
}
impl From<PaneId> for Starter {
    fn from(pane: PaneId) -> Self {
        Self::Pane(pane)
    }
}
impl From<ProjectId> for Starter {
    fn from(project: ProjectId) -> Self {
        Self::Project(project)
    }
}
impl PartialEq<PaneId> for Starter {
    fn eq(&self, pane: &PaneId) -> bool {
        *self == Self::Pane(*pane)
    }
}
/// Which tools and words the tool server offers the CLI it serves. Read from
/// the environment, so it describes and never authorizes: what a caller may
/// do follows from its credential alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Role {
    /// An agent a person started in a terminal.
    Agent,
    /// An agent another agent started.
    Spawned,
    /// An agent a project's lead started.
    Member,
    /// A project's lead, which has no terminal.
    Lead,
}
impl Role {
    pub(super) fn from_env() -> Self {
        Self::named(
            std::env::var(ROLE).ok().as_deref(),
            std::env::var_os(SPAWNED).is_some(),
        )
    }
    /// A project's word for it comes first; anything else it might say is
    /// no role, and leaves what the terminal was started as.
    fn named(role: Option<&str>, spawned: bool) -> Self {
        match role {
            Some("lead") => Self::Lead,
            Some("member") => Self::Member,
            _ if spawned => Self::Spawned,
            _ => Self::Agent,
        }
    }
    /// Its replies go to whoever started it.
    fn reports(self) -> bool {
        matches!(self, Self::Spawned | Self::Member)
    }
    /// The word a restored terminal's helper is told it by.
    fn mark(self) -> &'static str {
        match self {
            Self::Spawned => "spawned",
            Self::Member => "member",
            Self::Agent | Self::Lead => "",
        }
    }
}
/// What a project asks of the application through its lead or its agents,
/// answered with text once a frame has carried it out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectCall {
    /// The whole last reply of one of the project's agents.
    Report {
        agent: u64,
    },
    /// A file of the project's context from `offset`, or its index.
    ReadContext {
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        offset: u64,
    },
    /// The lead writes its status, or adds to notes.
    WriteContext {
        path: String,
        content: String,
        #[serde(default)]
        append: Option<bool>,
    },
    /// The lead records what the user approved.
    RecordDecision {
        decision: String,
        #[serde(default)]
        why: Option<String>,
    },
    /// The lead or one of its agents adds to the notes on a topic.
    AddNote {
        topic: String,
        content: String,
    },
    /// The lead adds a watch: a schedule it proposes to the user, or a pull
    /// request it follows.
    AddSubscription {
        title: String,
        trigger: WatchTrigger,
        #[serde(default)]
        instruction: String,
    },
    ListSubscriptions,
    RemoveSubscription {
        id: u64,
    },
    /// Where a pull request stands, or every one the project follows.
    PullRequestStatus {
        #[serde(default)]
        url: Option<String>,
    },
}
/// What sets a watch off, as a lead names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum WatchTrigger {
    Schedule {
        every_minutes: u32,
    },
    /// The address of a pull request, or "linked" for every one the
    /// project's agents linked.
    PullRequest(String),
}
/// What the application is told about a project outside its agents' replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectNote {
    /// A message for this agent was never typed: its prompt did not take it
    /// in time.
    Undelivered { agent: u64 },
    /// Its lead asked to start or tell an agent while the project is
    /// paused, and was refused.
    Paused,
}
/// What a lead's process is started with to reach the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeadEnv {
    pub endpoint: String,
    pub token: String,
    pub run: String,
}
impl LeadEnv {
    /// The variables its tool server reads, by name.
    pub fn vars(&self) -> [(&'static str, String); 4] {
        [
            (ENDPOINT, self.endpoint.clone()),
            (TOKEN, self.token.clone()),
            (RUN, self.run.clone()),
            (ROLE, "lead".into()),
        ]
    }
    /// Every variable of the bridge, for a process that must not inherit
    /// another caller's.
    pub fn names() -> [&'static str; 8] {
        [ENDPOINT, TOKEN, RUN, ROLE, SPAWNED, HOOK, SHIMS, HELPER]
    }
}

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
        /// What a project's lead calls the agent.
        #[serde(default)]
        title: Option<String>,
        /// The branch whose worktree a project's agent works in. Neptune
        /// makes the worktree, or finds the one the branch has.
        #[serde(default)]
        worktree: Option<String>,
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
    /// Something a project asks of the application.
    Project {
        call: ProjectCall,
    },
    /// What became of a `Project` call.
    CallResult {
        request: u64,
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
                | Self::Project { .. }
                | Self::CallResult { .. }
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
        /// What it was started with beyond its CLI's own choices, in words
        /// that follow "started", such as " on opus at high effort".
        #[serde(default)]
        with: String,
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
        /// A project's lead started it.
        #[serde(default)]
        project: bool,
    },
    Agents {
        agents: Vec<AgentReport>,
    },
    /// What a `Project` call came to.
    Text {
        text: String,
    },
}
/// One started agent, as whoever started it sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentReport {
    pub agent: u64,
    pub kind: AgentKind,
    pub status: Status,
    /// It has replies or a state its starter has not been told.
    pub news: bool,
    pub replies: Vec<String>,
    /// It ended, and its conversation can be opened again.
    #[serde(default)]
    pub reopens: bool,
    /// What a project's lead called it.
    #[serde(default)]
    pub title: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
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
    pub fn settled(&self) -> bool {
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
    /// What a project's lead calls it; an agent's own agents have no name.
    pub title: Option<String>,
    /// The branch whose worktree it works in, which the application makes
    /// before its terminal. Only a project's lead asks for one.
    pub worktree: Option<String>,
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
            title: None,
            worktree: None,
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
    /// `generation` is the asking terminal's, or the opening of a project's
    /// lead that asked. A project's own directory is an empty `cwd`.
    Spawn {
        request: u64,
        parent: Starter,
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
        parent: Starter,
    },
    /// A call of `project`, answered through `AgentBridge::call_result`.
    Project {
        request: u64,
        project: ProjectId,
        from: Starter,
        call: ProjectCall,
    },
}
/// An agent another agent started, and what passes between the two. Held
/// in memory only: tasks and replies are never saved or logged.
struct Child {
    parent: Starter,
    kind: AgentKind,
    title: Option<String>,
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
    /// The reply its last turn ended with, for a project to read whole.
    last: Option<String>,
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
    fn task(parent: Starter, task: Task) -> Self {
        Self {
            parent,
            kind: task.kind,
            title: task.title,
            launch: Some(task.prompt),
            model: task.model,
            effort: task.effort,
            ultracode: task.ultracode,
            resume: task.resume.map(|resume| resume.1),
            session: None,
            asking: None,
            outbox: VecDeque::new(),
            last: None,
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
    /// When its status changes by the clock alone: a request that has stood
    /// long enough to be one, a turn that did not follow a prompt, a CLI that
    /// never took its task.
    fn hold(&self, slot: Option<&Slot>, now: Instant) -> Option<Instant> {
        if self.ended.is_some() {
            return None;
        }
        let state = slot.and_then(|slot| Some((slot, slot.state().filter(|_| self.started)?)));
        let deadline = match state {
            None if self.asking.is_some() || (slot.is_none() && self.launch.is_none()) => {
                return None;
            }
            None => self.marked + START_GRACE,
            Some((slot, Activity::NeedsInput(_))) => slot.since + ASK_HOLD,
            Some((slot, Activity::Idle)) if self.owed > 0 => {
                self.marked.max(slot.since) + TURN_GRACE
            }
            Some(_) => return None,
        };
        (deadline > now).then_some(deadline)
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
/// The effort levels of a CLI Neptune starts for another agent; none for a
/// CLI it does not start.
pub fn effort_levels(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => &["low", "medium", "high", "xhigh", "max"],
        AgentKind::Codex => &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"],
        AgentKind::Opencode | AgentKind::Gemini | AgentKind::Pi | AgentKind::Omp => &[],
    }
}
/// What an agent was started with beyond its CLI's own choices, in words
/// that follow "started": its model, its effort and its ultra mode.
pub fn started_with(
    kind: AgentKind,
    model: Option<&str>,
    effort: Option<&str>,
    ultracode: bool,
) -> String {
    format!(
        "{}{}{}",
        model.map_or(String::new(), |model| format!(" on {model}")),
        effort.map_or(String::new(), |effort| format!(" at {effort} effort")),
        if ultracode && kind == AgentKind::Claude {
            " with ultracode on"
        } else {
            ""
        }
    )
}
/// The effort a CLI is started with and whether Claude Code runs with
/// ultracode, from what was asked. Each CLI has its own levels, and "ultra"
/// means ultracode for Claude Code and the Ultra effort for Codex.
pub fn effort_level(
    kind: AgentKind,
    effort: Option<&str>,
    ultra: bool,
) -> Result<(Option<String>, bool), String> {
    let levels = effort_levels(kind);
    // Neptune has no way to hand the others a task and read their answer.
    if levels.is_empty() {
        return Err(format!(
            "Neptune starts Claude Code and Codex for another agent, not {}.",
            label(kind)
        ));
    }
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
                ULTRA_HINT
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
/// A project's lead: a process Neptune runs without a terminal, which
/// reaches the application with a credential of its own.
struct Lead {
    token: String,
    /// The process this opening stands for; an earlier one's is refused.
    run: String,
    /// Which opening this is, as a terminal's generation is.
    epoch: u64,
    kind: AgentKind,
    wake: Wake,
}
/// Something asked of the application: who asked, what it came to once
/// the application answered, and when it was asked or answered.
type Asked<T> = (Starter, Option<Result<T, String>>, Instant);
#[derive(Default)]
struct Shared {
    slots: BTreeMap<PaneId, Slot>,
    /// Open leads, by project.
    leads: BTreeMap<ProjectId, Lead>,
    next_epoch: u64,
    /// The projects the model holds, and whether each is paused. Their
    /// agents are kept while they are here, whether or not a lead is open.
    projects: BTreeMap<ProjectId, bool>,
    /// Project calls and what each came to, answered only to who asked,
    /// with when each was asked or answered.
    calls: BTreeMap<u64, Asked<String>>,
    notes: Vec<(ProjectId, ProjectNote)>,
    /// The latest reference per pane since the last frame, and whether an
    /// agent left the pane before it: one that leaves and one that opens
    /// between two frames are two changes.
    changes: BTreeMap<PaneId, (u64, Option<AgentSession>, bool)>,
    /// Agents started by agents or by projects, by the pane each runs in.
    children: BTreeMap<PaneId, Child>,
    requests: Vec<AgentRequest>,
    /// Spawn requests and what became of each once the application answered.
    /// Each is answered only to who asked; with when it was asked or answered.
    spawns: BTreeMap<u64, Asked<PaneId>>,
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
    /// The listener every local caller reports to, started on first use.
    /// It binds a socket and writes the adapters, so only workers call it.
    #[cfg(unix)]
    fn listening(&self) -> std::io::Result<std::sync::MutexGuard<'_, Option<Listener>>> {
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
                                serde_json::to_vec(&answer(&shared, message)).unwrap_or_default()
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
        Ok(state)
    }
    /// Opens a project's lead to the application and names what its process
    /// is started with. Called by the worker that starts the process. An
    /// earlier opening of the same lead is closed: its credential and run
    /// are refused from here on, and the project's agents stay.
    pub fn open_lead(
        &self,
        project: ProjectId,
        kind: AgentKind,
        wake: Wake,
    ) -> std::io::Result<LeadEnv> {
        // Where no bridge listens there is nothing for a lead to reach.
        #[cfg(not(unix))]
        let env: std::io::Result<LeadEnv> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Agent bridge unavailable",
        ));
        #[cfg(unix)]
        let env = self.listening().and_then(|state| {
            let listener = state
                .as_ref()
                .ok_or_else(|| std::io::Error::other("Agent bridge unavailable"))?;
            let name = |prefix: &str| -> std::io::Result<String> {
                let nonce = tempfile::Builder::new()
                    .prefix(prefix)
                    .rand_bytes(32)
                    .tempfile_in(listener.directory.path())?;
                Ok(nonce
                    .path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned())
            };
            Ok(LeadEnv {
                endpoint: listener.address.clone(),
                token: name("lead-")?,
                run: name("run-")?,
            })
        });
        let env = env?;
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| std::io::Error::other("Agent bridge unavailable"))?;
        shared.leave(project);
        shared.next_epoch += 1;
        let epoch = shared.next_epoch;
        shared.leads.insert(
            project,
            Lead {
                token: env.token.clone(),
                run: env.run.clone(),
                epoch,
                kind,
                wake,
            },
        );
        Ok(env)
    }
    /// The lead's process is gone or is being stopped. What it asked and was
    /// not yet answered goes with it; the agents the project started stay.
    pub fn close_lead(&self, project: ProjectId) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.leave(project);
        }
    }
    /// The projects the model holds, and whether each is paused. A project
    /// that is gone takes its lead and what the bridge knew of its agents
    /// with it; their terminals are the model's to reveal.
    pub fn sync_projects(&self, projects: &[(ProjectId, bool)]) {
        if let Ok(mut shared) = self.shared.lock() {
            let shared = &mut *shared;
            if shared.projects.is_empty() && projects.is_empty() && shared.leads.is_empty() {
                return;
            }
            let gone: Vec<ProjectId> = shared
                .projects
                .keys()
                .chain(shared.leads.keys())
                .filter(|id| !projects.iter().any(|(project, _)| project == *id))
                .copied()
                .collect();
            for project in gone {
                shared.leave(project);
                shared
                    .children
                    .retain(|_, child| child.parent != Starter::Project(project));
                shared.notes.retain(|note| note.0 != project);
            }
            shared.projects = projects.iter().copied().collect();
        }
    }
    /// What the agents of `project` are doing, with the replies nobody read
    /// when `take` is set, and when to look again although nothing woke the
    /// application: the earliest moment one's state changes by the clock.
    pub fn collect_project(
        &self,
        project: ProjectId,
        take: bool,
    ) -> (Vec<AgentReport>, Option<Instant>) {
        let Ok(mut shared) = self.shared.lock() else {
            return (Vec::new(), None);
        };
        let now = Instant::now();
        let starter = Starter::Project(project);
        let agents = shared.collect(starter, None, take, now);
        let deadline = shared
            .children
            .iter()
            .filter(|(_, child)| child.parent == starter)
            .filter_map(|(pane, child)| child.hold(shared.slots.get(pane), now))
            .min();
        (agents, deadline)
    }
    /// The whole reply the last turn of a project's agent ended with.
    pub fn last_report(&self, project: ProjectId, agent: u64) -> Result<String, String> {
        let shared = self
            .shared
            .lock()
            .map_err(|_| "Neptune is unavailable.".to_owned())?;
        let child = shared
            .children
            .get(&PaneId::new(agent))
            .filter(|child| child.parent == Starter::Project(project))
            .ok_or_else(|| format!("Agent {agent} is not an agent of this project."))?;
        child
            .last
            .clone()
            .ok_or_else(|| format!("Agent {agent} has not ended a turn with a reply yet."))
    }
    /// Takes a message the person has for the agent `agent` of `project`,
    /// typed for it on the next frame as its lead's are. `Err` says why it
    /// cannot take one now, in words for the lead; a paused project sends
    /// nothing.
    pub fn tell(&self, project: ProjectId, agent: u64, text: String) -> Result<(), String> {
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| "Neptune is unavailable.".to_owned())?;
        if shared.projects.get(&project) == Some(&true) {
            return Err(PAUSED.into());
        }
        shared
            .tell(Starter::Project(project), agent, text, Instant::now())
            .map(|_| ())
    }
    /// Answers a project call with its text, or why it was refused.
    pub fn call_result(&self, request: u64, result: Result<String, String>) {
        if let Ok(mut shared) = self.shared.lock()
            && let Some((_, slot, at)) = shared.calls.get_mut(&request)
        {
            *slot = Some(result.map(|text| clip(&text)));
            *at = Instant::now();
        }
    }
    /// A message for the agent in `pane` was given up: its prompt did not
    /// take the paste in time, or took it and turned to a person before it
    /// was submitted. A project is told, since nobody waits on the agent to
    /// notice; an agent that started another sees it at rest.
    pub fn undelivered(&self, pane: PaneId, generation: u64) {
        if let Ok(mut shared) = self.shared.lock() {
            let shared = &mut *shared;
            if shared
                .slots
                .get(&pane)
                .is_some_and(|slot| slot.generation == generation)
                && let Some(child) = shared.children.get_mut(&pane)
                && let Starter::Project(project) = child.parent
            {
                // No turn follows a prompt that was never typed.
                child.owed = child.owed.saturating_sub(1);
                child.seen = None;
                if shared.notes.len() < MAX_NOTES {
                    shared
                        .notes
                        .push((project, ProjectNote::Undelivered { agent: pane.get() }));
                }
            }
        }
    }
    /// Gives back what `project` kept of an agent from an earlier run: the
    /// name its lead called it and the reply its last turn ended with, for
    /// one whose terminal was restored; and for one whose terminal is gone,
    /// with `closed` its conversation, a record that lets its lead open it
    /// again, with what it ran with. What the bridge already knows of the
    /// agent is left as it is.
    pub fn remember(
        &self,
        project: ProjectId,
        agent: u64,
        title: Option<&str>,
        last: Option<&str>,
        closed: Option<&AgentSession>,
        (model, effort, ultracode): (Option<&str>, Option<&str>, bool),
    ) {
        let Ok(mut shared) = self.shared.lock() else {
            return;
        };
        let shared = &mut *shared;
        let parent = Starter::Project(project);
        if !shared.projects.contains_key(&project) {
            return;
        }
        let pane = PaneId::new(agent);
        if let Some(child) = shared.children.get_mut(&pane) {
            if child.parent == parent {
                if child.title.is_none() {
                    child.title = title.map(str::to_owned);
                }
                if child.last.is_none() {
                    child.last = last.map(clip);
                }
            }
            return;
        }
        let Some(session) = closed.filter(|session| {
            session.session_id.is_some() && session.is_valid() && !shared.slots.contains_key(&pane)
        }) else {
            return;
        };
        let ended = shared
            .children
            .values()
            .filter(|child| child.parent == parent && child.ended.is_some())
            .count();
        if ended >= AgentSession::MAX_SPAWNED {
            return;
        }
        let reason = "its terminal was closed";
        shared.children.insert(
            pane,
            Child {
                launch: None,
                session: Some(session.clone()),
                last: last.map(clip),
                owed: 0,
                started: true,
                ended: Some(reason.into()),
                // Its lead heard of the end when it happened.
                seen: Some(Status::Ended {
                    reason: reason.into(),
                }),
                ..Child::task(
                    parent,
                    Task {
                        kind: session.kind,
                        prompt: String::new(),
                        // What it ran with, which it runs with again once
                        // its lead opens it.
                        model: model.map(str::to_owned),
                        effort: effort.map(str::to_owned),
                        ultracode,
                        resume: None,
                        title: title.map(str::to_owned),
                        worktree: None,
                    },
                )
            },
        );
    }
    /// What projects are told besides their agents' replies, in arrival order.
    pub fn drain_project_notes(&self) -> Vec<(ProjectId, ProjectNote)> {
        self.shared
            .lock()
            .map(|mut shared| std::mem::take(&mut shared.notes))
            .unwrap_or_default()
    }
    /// Called only by a startup worker. Tests and remote sessions opt out.
    pub fn prepare(
        &self,
        pane: PaneId,
        generation: u64,
        options: &mut SessionOptions,
        resume: Option<&AgentSession>,
        spawned_by: Option<Starter>,
        wake: Wake,
    ) -> std::io::Result<()> {
        #[cfg(not(unix))]
        {
            let _ = (pane, generation, options, resume, spawned_by, wake);
            return Ok(());
        }
        #[cfg(unix)]
        {
            let state = self.listening()?;
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
                        let role = match parent {
                            Starter::Pane(_) => Role::Spawned,
                            Starter::Project(_) => Role::Member,
                        };
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
                                        title: None,
                                        worktree: None,
                                    },
                                )
                            },
                        );
                        Start::Resume(agent, role)
                    }
                    (_, resume) => {
                        if let Some(child) = shared.children.get_mut(&pane) {
                            child.end("its terminal was restarted");
                        }
                        resume.map_or(Start::Shell, |agent| Start::Resume(agent, Role::Agent))
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
        match &request {
            AgentRequest::Spawn {
                request, parent, ..
            } => {
                shared
                    .spawns
                    .insert(*request, (*parent, None, Instant::now()));
            }
            AgentRequest::Project { request, from, .. } => {
                shared.calls.insert(*request, (*from, None, Instant::now()));
            }
            _ => {}
        }
        shared.requests.push(request);
    }
    /// A reply of the agent started in `pane`, as the end of a turn leaves
    /// one for whoever started it.
    #[cfg(test)]
    pub fn replied(&self, pane: PaneId, reply: &str) {
        if let Some(child) = self.shared.lock().unwrap().children.get_mut(&pane) {
            child.outbox.push_back(reply.to_owned());
        }
    }
    /// Stands in for the CLI of the agent started in `pane` ending a turn
    /// with `reply`, where a test has no CLI: it is at rest from then on.
    /// Its terminal's start must have been prepared, which a worker does.
    #[cfg(test)]
    pub fn rested(&self, pane: PaneId, generation: u64, reply: &str) {
        let mut shared = self.shared.lock().unwrap();
        let shared = &mut *shared;
        let slot = shared.slots.entry(pane).or_insert_with(|| Slot {
            generation,
            token: format!("rested-{pane}"),
            run: None,
            remote: false,
            kind: None,
            activity: None,
            shown: None,
            since: Instant::now(),
            wake: Arc::new(|| {}),
            _startup: None,
        });
        slot.run = Some("rested".into());
        slot.activity = Some(Tracked::idle());
        if let Some(child) = shared.children.get_mut(&pane) {
            child.start();
            child.owed = 0;
            child.outbox.push_back(reply.to_owned());
            child.last = Some(reply.to_owned());
        }
    }
    /// The task the agent started in `pane` opens with, while its terminal
    /// has not taken it.
    #[cfg(test)]
    pub fn task(&self, pane: PaneId) -> Option<String> {
        self.shared
            .lock()
            .unwrap()
            .children
            .get(&pane)
            .and_then(|child| child.launch.clone())
    }
    /// A lead asks for an agent, as its tool server does. Returns why it
    /// was refused, if it was.
    #[cfg(all(test, unix))]
    pub fn lead_spawns(&self, lead: &LeadEnv, title: &str) -> Option<String> {
        let event = Event::Spawn {
            kind: AgentKind::Claude,
            prompt: "task".into(),
            cwd: PathBuf::new(),
            model: None,
            effort: None,
            ultra: false,
            title: Some(title.into()),
            worktree: None,
        };
        let message = Message {
            token: lead.token.clone(),
            run: lead.run.clone(),
            event,
        };
        match answer(&self.shared, message) {
            Answer::Refused { reason } => Some(reason),
            _ => None,
        }
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
    /// What the agent in `pane` was started with, as its starter is told.
    #[cfg(test)]
    pub fn started_as(&self, pane: PaneId) -> Option<String> {
        self.shared
            .lock()
            .unwrap()
            .children
            .get(&pane)
            .map(|child| {
                started_with(
                    child.kind,
                    child.model.as_deref(),
                    child.effort.as_deref(),
                    child.ultracode,
                )
            })
    }
    /// What the application answered a project call with.
    #[cfg(test)]
    pub fn called(&self, request: u64) -> Option<Result<String, String>> {
        self.shared
            .lock()
            .unwrap()
            .calls
            .get(&request)
            .and_then(|call| call.1.clone())
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
    pub fn register_spawn(&self, pane: PaneId, parent: impl Into<Starter>, task: Task) {
        let parent = parent.into();
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
    ///
    /// A project's agent goes on asking while its terminal moves: the
    /// person opening it, or moving in what it asks, redraws it, and the
    /// row that sent them there must not come and go meanwhile. It asks
    /// until it takes its task or ends.
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
                && (screen.is_some() || matches!(child.parent, Starter::Pane(_)))
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
            && let Some((_, slot, at)) = shared.spawns.get_mut(&request)
        {
            *slot = Some(result);
            *at = Instant::now();
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
    /// was told or has left. A project is its agents' starter for as long as
    /// it exists, whether or not its lead is running.
    pub fn sync_spawned(&self, links: &[(PaneId, Starter)]) {
        if let Ok(mut shared) = self.shared.lock() {
            let shared = &mut *shared;
            if shared.children.is_empty() {
                return;
            }
            let (slots, projects) = (&shared.slots, &shared.projects);
            shared.children.retain(|pane, child| {
                if links.contains(&(*pane, child.parent)) {
                    return true;
                }
                child.end("its agent exited or its terminal was closed");
                // One that can be opened again is kept for as long as the
                // agent that started it might ask for it.
                let told = matches!(child.seen, Some(Status::Ended { .. }));
                (!told || child.reopens())
                    && match child.parent {
                        Starter::Pane(parent) => slots
                            .get(&parent)
                            .is_some_and(|parent| parent.run.is_some()),
                        Starter::Project(project) => projects.contains_key(&project),
                    }
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
            Self::Spawn { parent, .. } | Self::Project { from: parent, .. } => *parent == pane,
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
    /// Takes a prompt for the agent `starter` started under the number
    /// `agent`, to be typed for it on the next frame. Returns whether it is
    /// in a turn, which the prompt then waits for; `Err` says why nothing
    /// is typed.
    fn tell(
        &mut self,
        starter: Starter,
        agent: u64,
        text: String,
        now: Instant,
    ) -> Result<bool, String> {
        // A project is told what becomes of its agents; an agent asks.
        let (wait, shows) = match starter {
            Starter::Pane(_) => (
                "Wait for it with wait_for_agent.",
                "wait_for_agent shows what it asks.",
            ),
            Starter::Project(_) => (
                "You will be told when it has.",
                "You are told what it asks.",
            ),
        };
        let id = PaneId::new(agent);
        let target = self.slots.get(&id);
        let Some(child) = self
            .children
            .get_mut(&id)
            .filter(|child| child.parent == starter)
        else {
            return Err(unknown_agent(agent));
        };
        // A request to a person stands from its first moment for what is
        // typed, though it is reported only once it has stood a while.
        if target.is_some_and(|slot| matches!(slot.state(), Some(Activity::NeedsInput(_)))) {
            return Err(format!(
                "Agent {agent} is waiting for a person to answer it in its tab, so nothing can be typed for it. Tell the user."
            ));
        }
        let working = match child.status(target, now) {
            Status::Ended { reason } => {
                return Err(format!("Agent {agent} is no longer running: {reason}."));
            }
            Status::Starting => {
                return Err(format!(
                    "Agent {agent} has not taken its first task yet. {wait}"
                ));
            }
            Status::Asking { .. } => {
                return Err(format!(
                    "Agent {agent} is asking something before it takes its task, so a message would answer that instead. {shows}"
                ));
            }
            Status::Waiting { .. } => {
                return Err(format!(
                    "Agent {agent} is waiting for a person to answer it in its tab, so nothing can be typed for it. Tell the user."
                ));
            }
            Status::Working => true,
            Status::Idle => false,
        };
        let Some(target) = target else {
            return Err(unknown_agent(agent));
        };
        if text.trim().is_empty() || text.len() > MAX_TEXT {
            return Err("The message is empty or too long.".into());
        }
        if self.requests.len() >= MAX_REQUESTS {
            return Err("Neptune is busy; try again in a moment.".into());
        }
        child.owed = child.owed.saturating_add(1);
        child.marked = now;
        child.seen = None;
        self.requests.push(AgentRequest::Tell {
            pane: id,
            generation: target.generation,
            text,
        });
        Ok(working)
    }
    /// Queues the opening of a terminal for an agent `parent` starts, and
    /// names the request its answer is asked for by.
    fn queue_spawn(
        &mut self,
        parent: Starter,
        generation: u64,
        task: Task,
        cwd: PathBuf,
    ) -> Result<u64, String> {
        let live = self
            .children
            .values()
            .filter(|child| child.parent == parent && child.ended.is_none())
            .count();
        match parent {
            Starter::Pane(_) if live >= AgentSession::MAX_SPAWNED => {
                return Err(format!(
                    "{live} agents this one started are still open, which is the most Neptune keeps. Close one with close_agent first."
                ));
            }
            Starter::Project(_) if live >= Project::MAX_AGENTS => {
                return Err(format!(
                    "{live} agents of this project are still open, which is the most Neptune keeps for one. Close one with close_agent first."
                ));
            }
            _ => {}
        }
        let now = Instant::now();
        self.evict(now);
        if self.requests.len() >= MAX_REQUESTS || self.spawns.len() >= MAX_REQUESTS {
            return Err("Neptune is busy; try again in a moment.".into());
        }
        self.next_request += 1;
        let request = self.next_request;
        self.spawns.insert(request, (parent, None, now));
        self.requests.push(AgentRequest::Spawn {
            request,
            parent,
            generation,
            task,
            cwd,
        });
        Ok(request)
    }
    /// Makes room where a tool gave up on its request or was cancelled:
    /// nobody comes back for those answers, and they must not keep later
    /// requests out.
    fn evict(&mut self, now: Instant) {
        fn stale<T>(result: &Option<T>, at: Instant, now: Instant) -> bool {
            let kept = if result.is_some() {
                ABANDONED
            } else {
                UNANSWERED
            };
            now.saturating_duration_since(at) >= kept
        }
        self.spawns
            .retain(|_, spawn| !stale(&spawn.1, spawn.2, now));
        self.calls.retain(|_, call| !stale(&call.1, call.2, now));
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
        self.calls.retain(|_, call| call.0 != pane);
        if let Some(child) = self.children.get_mut(&pane) {
            child.end("its terminal was closed");
        }
    }
    /// Closes the lead of `project`, with what it asked and was not answered.
    fn leave(&mut self, project: ProjectId) {
        let starter = Starter::Project(project);
        self.leads.remove(&project);
        self.requests.retain(|request| {
            !matches!(
                request,
                AgentRequest::Spawn { parent, .. } | AgentRequest::Project { from: parent, .. }
                    if *parent == starter
            )
        });
        self.spawns.retain(|_, spawn| spawn.0 != starter);
        self.calls.retain(|_, call| call.0 != starter);
    }
    /// What the agents `starter` started are doing, with the replies it has
    /// not read when `take` is set. Only `agent` when one is named.
    fn collect(
        &mut self,
        starter: Starter,
        agent: Option<u64>,
        take: bool,
        now: Instant,
    ) -> Vec<AgentReport> {
        let mut agents = Vec::new();
        for (id, child) in self
            .children
            .iter_mut()
            .filter(|(id, child)| child.parent == starter && agent.is_none_or(|a| a == id.get()))
        {
            let status = child.status(self.slots.get(id), now);
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
                // One that is still starting has done nothing yet: an agent
                // restored at rest is not news when its CLI has opened.
                if status != Status::Starting {
                    child.seen = None;
                }
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
                title: child.title.clone(),
            });
        }
        agents
    }
}
/// What a terminal's shell opens with.
#[cfg(unix)]
enum Start<'a> {
    Shell,
    /// A saved agent, and who started it.
    Resume(&'a AgentSession, Role),
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
        Start::Resume(agent, role) => Some([
            "--agent-restore".into(),
            serde_json::to_string(agent)?,
            role.mark().into(),
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
            let text = clip(&text);
            if done {
                child.last = Some(text.clone());
            }
            child.outbox.push_back(text);
        }
        child.seen = None;
        if !done {
            // An agent that started it asks for its words; a project is
            // handed them, so the application looks now.
            if matches!(child.parent, Starter::Project(_)) {
                drop(guard);
                wake();
            }
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
            slot.activity = Some(Tracked::idle());
            slot.shown = None;
            slot.since = now;
            reset = Some(Some(Activity::Idle));
            Some(agent)
        }
        Event::Session { agent } if open && agent.is_valid() => Some(agent),
        Event::Close if open => {
            left = true;
            slot.run = None;
            slot.kind = None;
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
        shared.requests.retain(|request| {
            !matches!(
                request,
                AgentRequest::Spawn { parent, .. } | AgentRequest::Project { from: parent, .. }
                    if *parent == pane
            )
        });
        shared.spawns.retain(|_, spawn| spawn.0 != pane);
        shared.calls.retain(|_, call| call.0 != pane);
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
/// its answer travels back. A project's agent is briefed by the application
/// instead, which knows what the project keeps: its task goes on as it is.
fn delegation(parent: Starter, kind: AgentKind, prompt: &str) -> String {
    let who = match parent {
        Starter::Pane(_) => format!("{} in another Neptune terminal", label(kind)),
        Starter::Project(_) => return prompt.to_owned(),
    };
    format!(
        "[{who} started you and handed you the task below. \
It reads the last message of each of your turns and nothing else you write, so end \
each turn with what you did and what it needs to know.]\n\n{prompt}"
    )
}
fn unknown_agent(agent: u64) -> String {
    format!(
        "Agent {agent} is not an agent this one started, or it ended and that was already reported."
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
    let unknown = |agent: u64| refused(unknown_agent(agent));
    let Ok(mut guard) = shared.lock() else {
        return refused("Neptune is unavailable.".into());
    };
    let shared = &mut *guard;
    // A terminal's credential first, then a lead's: the one that asks is who
    // its credential says, whatever else it claims.
    let slot = shared
        .slots
        .iter()
        .find(|(_, slot)| !slot.remote && slot.token == message.token);
    let (starter, generation, wake, kind, open) = if let Some((&pane, slot)) = slot {
        (
            Starter::Pane(pane),
            slot.generation,
            slot.wake.clone(),
            slot.kind,
            slot.run.as_ref() == Some(&message.run),
        )
    } else if let Some((&project, lead)) = shared
        .leads
        .iter()
        .find(|(_, lead)| lead.token == message.token)
    {
        (
            Starter::Project(project),
            lead.epoch,
            lead.wake.clone(),
            Some(lead.kind),
            lead.run == message.run,
        )
    } else {
        return refused(NOT_TRACKED.into());
    };
    // A paused project starts nothing and sends nothing; it still looks.
    let paused = matches!(
        starter,
        Starter::Project(project) if shared.projects.get(&project) == Some(&true)
    );
    // The terminal that asks, where it is one.
    let terminal = match starter {
        Starter::Pane(pane) => Some(pane),
        Starter::Project(_) => None,
    };
    let now = Instant::now();
    match message.event {
        // A started terminal asks before its CLI, and so any run, exists.
        Event::Launch => {
            let Some(pane) = terminal else {
                return refused(NOT_TRACKED.into());
            };
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
                        project: matches!(child.parent, Starter::Project(_)),
                    })
                });
            match launch {
                Some(launch) => (launch, None),
                None => refused("No agent is waiting to start in this terminal.".into()),
            }
        }
        Event::LaunchFailed { reason } => {
            let Some(pane) = terminal else {
                return refused(NOT_TRACKED.into());
            };
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
        Event::Spawn { .. } | Event::Reopen { .. } | Event::Tell { .. } if paused => {
            // The chat says why, beside what the lead makes of it.
            if let Starter::Project(project) = starter
                && shared.notes.len() < MAX_NOTES
            {
                shared.notes.push((project, ProjectNote::Paused));
            }
            refused(PAUSED.into())
        }
        Event::Spawn {
            kind: wanted,
            prompt,
            cwd,
            model,
            effort,
            ultra,
            title,
            worktree,
        } => {
            let Some(kind) = kind else {
                return refused(NOT_TRACKED.into());
            };
            // Neptune makes a worktree in a project's repository, off the
            // frame, and starts the agent in it. An agent that starts
            // another names a directory it made itself.
            let worktree = match (terminal, worktree) {
                (_, None) => None,
                (Some(_), Some(_)) => {
                    return refused(
                        "Only a project's lead names a worktree. Create one with git worktree add and pass it as cwd.".into(),
                    );
                }
                (None, Some(_)) if !cwd.as_os_str().is_empty() => {
                    return refused(
                        "Give worktree or cwd, not both: Neptune makes the worktree beside the project's repository.".into(),
                    );
                }
                (None, Some(branch)) => match super::worktrees::branch_name(&branch) {
                    Ok(name) if name == branch => Some(name),
                    _ => {
                        return refused(
                                "worktree is a branch name git takes, without spaces, such as fix-login.".into(),
                            );
                    }
                },
            };
            // A project's own directory is the application's to name.
            let here = terminal.is_none() && cwd.as_os_str().is_empty();
            if prompt.trim().is_empty() || prompt.len() > MAX_TEXT || !(here || cwd.is_absolute()) {
                return refused("The task or its directory is not usable.".into());
            }
            // Only a project names its agents, in a few plain words.
            let title = match (terminal, title) {
                (Some(_), _) => None,
                (None, Some(title))
                    if !title.trim().is_empty()
                        && title.chars().count() <= MAX_TITLE
                        && !title.chars().any(char::is_control) =>
                {
                    Some(title.trim().to_owned())
                }
                (None, _) => {
                    return refused(format!(
                        "title is a name of up to {MAX_TITLE} characters on one line."
                    ));
                }
            };
            let model = match model.as_deref().map(crate::projects::settings::model_name) {
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
                prompt: delegation(starter, kind, &prompt),
                model,
                effort,
                ultracode,
                resume: None,
                title,
                worktree,
            };
            match shared.queue_spawn(starter, generation, task, cwd) {
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
                .filter(|child| child.parent == starter)
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
                    match starter {
                        Starter::Pane(_) => label(kind),
                        Starter::Project(_) => "The lead of your Neptune project",
                    }
                ),
                model: child.model.clone(),
                effort: child.effort.clone(),
                ultracode: child.ultracode,
                resume: Some((id, session)),
                title: child.title.clone(),
                worktree: None,
            };
            match shared.queue_spawn(starter, generation, task, cwd) {
                Ok(request) => (Answer::Queued { request }, Some(wake)),
                Err(reason) => refused(reason),
            }
        }
        // What a CLI asks before its task is a person's to answer; a lead
        // tells the user and presses nothing.
        Event::Press { .. } if terminal.is_none() => {
            refused("Neptune does not answer that.".into())
        }
        Event::Press { agent, keys } => {
            let id = PaneId::new(agent);
            let target = shared.slots.get(&id);
            let Some(child) = shared
                .children
                .get_mut(&id)
                .filter(|child| child.parent == starter)
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
            match shared
                .spawns
                .get(&request)
                .filter(|spawn| spawn.0 == starter)
            {
                None => refused("Neptune lost track of that request; try again.".into()),
                Some((_, None, _)) => (Answer::Pending, None),
                Some(_) => match shared.spawns.remove(&request).and_then(|spawn| spawn.1) {
                    Some(Ok(pane)) => {
                        let with = shared.children.get(&pane).map_or(String::new(), |child| {
                            started_with(
                                child.kind,
                                child.model.as_deref(),
                                child.effort.as_deref(),
                                child.ultracode,
                            )
                        });
                        (
                            Answer::Spawned {
                                agent: pane.get(),
                                with,
                            },
                            None,
                        )
                    }
                    Some(Err(reason)) => refused(reason),
                    None => (Answer::Pending, None),
                },
            }
        }
        Event::Collect { agent, take } => {
            // A project's replies are taken by the application, which hands
            // them to its lead; the lead itself only looks.
            let take = take && terminal.is_some();
            let agents = shared.collect(starter, agent, take, now);
            match agent {
                Some(agent) if agents.is_empty() => unknown(agent),
                _ => (Answer::Agents { agents }, None),
            }
        }
        Event::Tell { agent, text } => match shared.tell(starter, agent, text, now) {
            Ok(working) => (Answer::Told { working }, Some(wake)),
            Err(reason) => refused(reason),
        },
        Event::Dismiss { agent } => {
            let id = PaneId::new(agent);
            let target = shared.slots.get(&id).map(|slot| slot.generation);
            let Some(child) = shared
                .children
                .get_mut(&id)
                .filter(|child| child.parent == starter)
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
            child.end(match starter {
                Starter::Pane(_) => "the agent that started it closed it",
                Starter::Project(_) => "the project's lead closed it",
            });
            child.seen = Some(child.status(None, now));
            shared.requests.push(AgentRequest::Close {
                pane: id,
                generation: target,
                parent: starter,
            });
            (Answer::Done, Some(wake))
        }
        Event::Project { call } => {
            // The project of the lead that asks, or of the agent it started.
            let project = match starter {
                Starter::Project(project) => Some(project),
                Starter::Pane(pane) => shared
                    .children
                    .get(&pane)
                    .filter(|child| child.ended.is_none())
                    .and_then(|child| match child.parent {
                        Starter::Project(project) => Some(project),
                        Starter::Pane(_) => None,
                    }),
            };
            let Some(project) = project else {
                return refused("This terminal's agent does not belong to a project.".into());
            };
            shared.evict(now);
            if shared.requests.len() >= MAX_REQUESTS || shared.calls.len() >= MAX_REQUESTS {
                return refused("Neptune is busy; try again in a moment.".into());
            }
            shared.next_request += 1;
            let request = shared.next_request;
            shared.calls.insert(request, (starter, None, now));
            shared.requests.push(AgentRequest::Project {
                request,
                project,
                from: starter,
                call,
            });
            (Answer::Queued { request }, Some(wake))
        }
        Event::CallResult { request } => {
            match shared.calls.get(&request).filter(|call| call.0 == starter) {
                None => refused("Neptune lost track of that request; try again.".into()),
                Some((_, None, _)) => (Answer::Pending, None),
                Some(_) => match shared.calls.remove(&request).and_then(|call| call.1) {
                    Some(Ok(text)) => (Answer::Text { text }, None),
                    Some(Err(reason)) => refused(reason),
                    None => (Answer::Pending, None),
                },
            }
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
                if hook.hook_event_name == "SessionStart"
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
                if Role::from_env().reports()
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
            super::agent_mcp::serve(
                std::io::stdin(),
                std::io::stdout().lock(),
                &mut Bridge(std::env::var(RUN).unwrap_or_default()),
                Role::from_env(),
            )?;
            Ok(Some(0))
        }
        Some("--agent-run") => {
            let provider = kind(args.get(1).map(String::as_str).unwrap_or_default())?;
            Ok(Some(run_agent(
                provider,
                &args[2..],
                None,
                Role::Agent,
                false,
            )?))
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
                project,
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
            let role = if project { Role::Member } else { Role::Spawned };
            match run_agent(kind, &arguments, resumed.as_ref(), role, ultracode) {
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
            let role = [Role::Spawned, Role::Member]
                .into_iter()
                .find(|role| args.get(2).is_some_and(|mark| mark == role.mark()))
                .unwrap_or(Role::Agent);
            Ok(Some(run_agent(
                agent.kind,
                &arguments,
                Some(&agent),
                role,
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
    resolve_in(
        provider,
        &std::env::var_os("PATH").unwrap_or_default(),
        std::env::var_os(SHIMS).map(PathBuf::from).as_deref(),
    )
}
/// The CLI itself in one of these folders, never the adapter in `shims`.
pub(super) fn resolve_in(
    provider: AgentKind,
    path: &std::ffi::OsStr,
    shims: Option<&std::path::Path>,
) -> anyhow::Result<PathBuf> {
    for directory in std::env::split_paths(path) {
        if shims == Some(directory.as_path()) {
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
/// `role` marks a CLI another agent or a project started, whose replies
/// return to it; `ultracode` starts Claude Code with ultracode on.
fn run_agent(
    provider: AgentKind,
    args: &[String],
    resumed: Option<&AgentSession>,
    role: Role,
    ultracode: bool,
) -> anyhow::Result<i32> {
    let spawned = role.reports();
    let member = role == Role::Member;
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
                .chain(member.then(|| (ROLE, "member".to_owned())))
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
        if member {
            supervisor.env(ROLE, "member");
        } else {
            supervisor.env_remove(ROLE);
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
        if member {
            command.env(ROLE, "member");
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
        // What has not reached a frame leaves with its pane.
        assert!(emit("one", "a", link(url)));
        assert!(emit("one", "a", file(&shot)));
        bridge.close(PaneId::new(1));
        assert!(bridge.drain_links().is_empty());
        assert!(bridge.drain_attachments().is_empty());
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
            title: None,
            worktree: None,
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
                title: None,
                worktree: None,
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
                title: None,
                worktree: None,
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
        // The answer says what it was started with, as its task holds it.
        assert_eq!(
            ask("one", "a", Event::SpawnResult { request }),
            Answer::Spawned {
                agent: 2,
                with: " on gpt-5.1-codex at high effort".into(),
            }
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
                project: false,
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
        bridge.sync_spawned(&[(child, parent.into())]);
        assert_eq!(ask("one", "a", Event::Dismiss { agent: 2 }), Answer::Done);
        assert!(refused(ask("one", "a", Event::Dismiss { agent: 2 })));
        assert_eq!(
            bridge.drain_requests(),
            [AgentRequest::Close {
                pane: child,
                generation: 7,
                parent: parent.into()
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
        let args = prepare(child, 1, Some(&codex()), Some(parent.into()));
        assert_eq!(
            (args[4].as_str(), args[6].as_str()),
            ("--agent-restore", "spawned")
        );
        let status = |bridge: &AgentBridge| {
            let shared = bridge.shared.lock().unwrap();
            let record = shared.children.get(&child).unwrap();
            assert_eq!(
                (record.parent, record.kind),
                (parent.into(), AgentKind::Codex)
            );
            record.status(shared.slots.get(&child), Instant::now())
        };
        assert_eq!(status(&bridge), Status::Starting);
        // A task handed over opens the terminal with its CLI instead.
        bridge.register_spawn(PaneId::new(3), parent, Task::new(AgentKind::Claude, "task"));
        let args = prepare(PaneId::new(3), 1, None, Some(parent.into()));
        assert_eq!(args[4..7], ["--agent-spawn", "", ""]);
        // A link without a saved agent or a task opens a plain shell.
        assert!(prepare(PaneId::new(4), 1, None, Some(parent.into())).is_empty());
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
    #[test]
    fn a_projects_words_cross_the_wire_and_its_role_is_read_from_what_its_cli_was_given() {
        // What a lead's tool server sends is what the application reads.
        let call = ProjectCall::Report { agent: 12 };
        let sent = serde_json::to_string(&message("t", "r", Event::Project { call: call.clone() }))
            .unwrap();
        assert_eq!(
            sent,
            r#"{"token":"t","run":"r","event":{"event":"project","call":{"call":"report","agent":12}}}"#
        );
        let read: Message = serde_json::from_str(&sent).unwrap();
        assert!(matches!(&read.event, Event::Project { call: read } if *read == call));
        assert!(read.event.is_request());
        let result = r#"{"token":"t","run":"r","event":{"event":"call_result","request":4}}"#;
        let read: Message = serde_json::from_str(result).unwrap();
        assert!(matches!(read.event, Event::CallResult { request: 4 }) && read.event.is_request());
        // A call Neptune does not know, or one with more than it names, is
        // refused instead of guessed.
        for event in [
            r#"{"event":"project","call":{"call":"erase"}}"#,
            r#"{"event":"project","call":{"call":"report","agent":1,"path":"/"}}"#,
            r#"{"event":"project"}"#,
        ] {
            assert!(serde_json::from_str::<Event>(event).is_err(), "{event}");
        }
        // What a project keeps is asked for and added to by name.
        for (call, wire) in [
            (
                ProjectCall::ReadContext {
                    path: None,
                    offset: 0,
                },
                r#"{"call":"read_context","path":null,"offset":0}"#,
            ),
            (
                ProjectCall::WriteContext {
                    path: "STATUS.md".into(),
                    content: "Goal".into(),
                    append: Some(true),
                },
                r#"{"call":"write_context","path":"STATUS.md","content":"Goal","append":true}"#,
            ),
            (
                ProjectCall::RecordDecision {
                    decision: "Use JWT".into(),
                    why: None,
                },
                r#"{"call":"record_decision","decision":"Use JWT","why":null}"#,
            ),
            (
                ProjectCall::AddNote {
                    topic: "auth".into(),
                    content: "Found".into(),
                },
                r#"{"call":"add_note","topic":"auth","content":"Found"}"#,
            ),
            // A project's watches, as its lead asks for them.
            (
                ProjectCall::AddSubscription {
                    title: "Nightly".into(),
                    trigger: WatchTrigger::Schedule { every_minutes: 60 },
                    instruction: "Check the build".into(),
                },
                r#"{"call":"add_subscription","title":"Nightly","trigger":{"schedule":{"every_minutes":60}},"instruction":"Check the build"}"#,
            ),
            (
                ProjectCall::AddSubscription {
                    title: "PR".into(),
                    trigger: WatchTrigger::PullRequest("linked".into()),
                    instruction: String::new(),
                },
                r#"{"call":"add_subscription","title":"PR","trigger":{"pull_request":"linked"},"instruction":""}"#,
            ),
            (
                ProjectCall::ListSubscriptions,
                r#"{"call":"list_subscriptions"}"#,
            ),
            (
                ProjectCall::RemoveSubscription { id: 3 },
                r#"{"call":"remove_subscription","id":3}"#,
            ),
            (
                ProjectCall::PullRequestStatus { url: None },
                r#"{"call":"pull_request_status","url":null}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&call).unwrap(), wire);
            assert_eq!(serde_json::from_str::<ProjectCall>(wire).unwrap(), call);
        }
        assert_eq!(
            serde_json::from_str::<ProjectCall>(r#"{"call":"read_context"}"#).unwrap(),
            ProjectCall::ReadContext {
                path: None,
                offset: 0
            }
        );
        // Nothing replaces a decision: there is no such call to make.
        for call in [
            r#"{"call":"record_decision","decision":"x","replace":true}"#,
            r#"{"call":"write_decisions","content":"x"}"#,
            r#"{"call":"add_note","topic":"t","content":"x","append":false}"#,
            // A schedule by the clock is not something a lead can ask for,
            // nor can it allow its own.
            r#"{"call":"add_subscription","title":"t","trigger":{"schedule":{"daily_at":"09:00"}}}"#,
            r#"{"call":"add_subscription","title":"t","trigger":{"schedule":{"every_minutes":60}},"allowed":true}"#,
            r#"{"call":"add_subscription","title":"t","trigger":{"webhook":"https://x"}}"#,
        ] {
            assert!(serde_json::from_str::<ProjectCall>(call).is_err(), "{call}");
        }
        // An agent's own task carries no title, and its terminal no project.
        let spawn = r#"{"event":"spawn","kind":"codex","prompt":"x","cwd":"/tmp"}"#;
        assert!(matches!(
            serde_json::from_str::<Event>(spawn).unwrap(),
            Event::Spawn { title: None, .. }
        ));
        let titled = r#"{"event":"spawn","kind":"codex","prompt":"x","cwd":"","title":"api"}"#;
        assert!(matches!(
            serde_json::from_str::<Event>(titled).unwrap(),
            Event::Spawn { title: Some(title), cwd, .. } if title == "api" && cwd.as_os_str().is_empty()
        ));
        let launch = r#"{"answer":"launch","kind":"codex","prompt":"x"}"#;
        assert!(matches!(
            serde_json::from_str::<Answer>(launch).unwrap(),
            Answer::Launch { project: false, .. }
        ));
        let text = Answer::Text { text: "all".into() };
        assert_eq!(
            serde_json::to_string(&text).unwrap(),
            r#"{"answer":"text","text":"all"}"#
        );
        let report =
            r#"{"agent":2,"kind":"codex","status":{"status":"idle"},"news":false,"replies":[]}"#;
        assert_eq!(
            serde_json::from_str::<AgentReport>(report).unwrap().title,
            None
        );

        // A project's word decides the role; any other word is none.
        for (role, spawned, is) in [
            (None, false, Role::Agent),
            (None, true, Role::Spawned),
            (Some("member"), true, Role::Member),
            (Some("member"), false, Role::Member),
            (Some("lead"), false, Role::Lead),
            (Some("lead"), true, Role::Lead),
            (Some("owner"), true, Role::Spawned),
            (Some(""), false, Role::Agent),
        ] {
            assert_eq!(Role::named(role, spawned), is, "{role:?} {spawned}");
        }
        // Only an agent someone started hands its replies on, and a restored
        // terminal is told which by a word of its own.
        assert!(Role::Spawned.reports() && Role::Member.reports());
        assert!(!Role::Agent.reports() && !Role::Lead.reports());
        assert_eq!(
            [Role::Agent, Role::Spawned, Role::Member, Role::Lead].map(Role::mark),
            ["", "spawned", "member", ""]
        );
    }
    /// An unopened terminal at generation 7, as the application starts one.
    #[cfg(unix)]
    fn terminal(bridge: &AgentBridge, pane: PaneId, token: &str) {
        terminal_waking(bridge, pane, token, Arc::new(|| {}));
    }
    #[cfg(unix)]
    fn terminal_waking(bridge: &AgentBridge, pane: PaneId, token: &str, wake: Wake) {
        bridge.shared.lock().unwrap().slots.insert(
            pane,
            Slot {
                generation: 7,
                token: token.into(),
                run: None,
                remote: false,
                kind: None,
                activity: None,
                shown: None,
                since: Instant::now(),
                wake,
                _startup: None,
            },
        );
    }
    /// A lead's task for an agent, as its tool server sends it.
    #[cfg(unix)]
    fn lead_spawn(title: Option<&str>) -> Event {
        Event::Spawn {
            kind: AgentKind::Codex,
            prompt: "Build the API".into(),
            cwd: PathBuf::new(),
            model: None,
            effort: None,
            ultra: false,
            title: title.map(str::to_owned),
            worktree: None,
        }
    }
    #[cfg(unix)]
    #[test]
    fn a_projects_lead_starts_agents_and_the_application_collects_what_they_say() {
        let bridge = AgentBridge::default();
        let (project, other, child) = (ProjectId::new(3), ProjectId::new(4), PaneId::new(2));
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let wake = || -> Wake {
            let woken = woken.clone();
            Arc::new(move || {
                woken.fetch_add(1, Ordering::Relaxed);
            })
        };
        let env = bridge
            .open_lead(project, AgentKind::Claude, wake())
            .unwrap();
        assert!(env.vars().contains(&(ROLE, "lead".into())));
        assert!(env.vars().contains(&(TOKEN, env.token.clone())));
        let lead = |event| answer(&bridge.shared, message(&env.token, &env.run, event));
        let kid = |run: &str, event| apply_message(&bridge.shared, message("kid", run, event));
        let refused = |answer: Answer| matches!(answer, Answer::Refused { .. });

        // A lead names each agent; its own directory is the application's to
        // fill in, and the request says which project asks.
        assert!(refused(lead(lead_spawn(None))));
        assert!(refused(lead(lead_spawn(Some("two\nlines")))));
        assert!(refused(lead(lead_spawn(Some(&"x".repeat(MAX_TITLE + 1))))));
        let Answer::Queued { request } = lead(lead_spawn(Some(" auth-refactor "))) else {
            panic!("the spawn was not queued");
        };
        assert_eq!(woken.swap(0, Ordering::Relaxed), 1);
        let Some(AgentRequest::Spawn {
            parent, task, cwd, ..
        }) = bridge.drain_requests().pop()
        else {
            panic!("the application was not asked");
        };
        assert_eq!(parent, Starter::Project(project));
        assert_eq!(cwd, PathBuf::new());
        assert_eq!(task.title.as_deref(), Some("auth-refactor"));
        // The application writes the brief of a project's agent, from what
        // the project keeps; the bridge hands the task on as it came.
        assert_eq!(task.prompt, "Build the API");
        terminal_waking(&bridge, child, "kid", wake());
        bridge.register_spawn(child, parent, task);
        bridge.spawn_result(request, Ok(child));
        assert_eq!(
            lead(Event::SpawnResult { request }),
            Answer::Spawned {
                agent: 2,
                with: String::new(),
            }
        );
        // Until its CLI takes the task it is starting, for no longer than a
        // CLI is given; a message would be typed into its shell, and the
        // lead is not pointed at a tool it does not have.
        let (starting, deadline) = bridge.collect_project(project, false);
        assert_eq!(starting[0].status, Status::Starting);
        let wait = deadline.unwrap().saturating_duration_since(Instant::now());
        assert!(wait <= START_GRACE && wait > START_GRACE / 2);
        assert!(matches!(
            lead(Event::Tell { agent: 2, text: "Hurry".into() }),
            Answer::Refused { reason }
                if reason.contains("You will be told") && !reason.contains("wait_for_agent")
        ));
        // Its terminal learns that a project started it.
        assert!(matches!(
            answer(&bridge.shared, message("kid", "", Event::Launch)),
            Answer::Launch { project: true, .. }
        ));
        assert!(kid("c", Event::Open { agent: codex() }));
        let signal = |signal| Event::Activity { signal };
        assert!(kid("c", signal(Signal::Prompt)));

        // Words before the end of a turn wake the application: nobody polls.
        woken.store(0, Ordering::Relaxed);
        let report = |text: &str, done| {
            kid(
                "c",
                Event::Report {
                    text: text.into(),
                    done,
                },
            )
        };
        assert!(report("Which port?", false));
        assert_eq!(woken.swap(0, Ordering::Relaxed), 1);
        assert!(report("Built 3 endpoints.", true));
        // The lead looks, and never takes: the application hands replies on.
        let looked = match lead(Event::Collect {
            agent: None,
            take: true,
        }) {
            Answer::Agents { agents } => agents,
            other => panic!("{other:?}"),
        };
        assert!(looked[0].news && looked[0].replies.is_empty());
        assert_eq!(looked[0].title.as_deref(), Some("auth-refactor"));
        assert!(bridge.collect_project(other, true).0.is_empty());
        let (peeked, _) = bridge.collect_project(project, false);
        assert!(peeked[0].news && peeked[0].replies.is_empty());
        let (taken, deadline) = bridge.collect_project(project, true);
        assert_eq!(
            (taken[0].agent, &taken[0].status, taken[0].kind),
            (2, &Status::Idle, AgentKind::Codex)
        );
        assert_eq!(taken[0].replies, ["Which port?", "Built 3 endpoints."]);
        assert_eq!(deadline, None);
        assert!(!bridge.collect_project(project, false).0[0].news);

        // The whole last reply is read through a call the application answers.
        assert_eq!(
            bridge.last_report(project, 2),
            Ok("Built 3 endpoints.".into())
        );
        assert!(bridge.last_report(other, 2).is_err());
        let call = ProjectCall::Report { agent: 2 };
        let Answer::Queued { request } = lead(Event::Project { call: call.clone() }) else {
            panic!("the call was not queued");
        };
        assert_eq!(
            bridge.drain_requests(),
            [AgentRequest::Project {
                request,
                project,
                from: Starter::Project(project),
                call: call.clone(),
            }]
        );
        assert_eq!(lead(Event::CallResult { request }), Answer::Pending);
        bridge.call_result(request, bridge.last_report(project, 2));
        assert_eq!(
            lead(Event::CallResult { request }),
            Answer::Text {
                text: "Built 3 endpoints.".into()
            }
        );
        assert!(refused(lead(Event::CallResult { request })));
        // An agent of the project may ask as well; anyone else has no project.
        assert!(matches!(
            answer(
                &bridge.shared,
                message("kid", "c", Event::Project { call: call.clone() })
            ),
            Answer::Queued { .. }
        ));
        assert!(matches!(
            bridge.drain_requests().pop(),
            Some(AgentRequest::Project { from, .. }) if from == child
        ));
        terminal(&bridge, PaneId::new(9), "stranger");
        let stranger = |event| answer(&bridge.shared, message("stranger", "x", event));
        assert!(apply_message(
            &bridge.shared,
            message(
                "stranger",
                "x",
                Event::Open {
                    agent: agent(FIRST)
                }
            )
        ));
        assert!(refused(stranger(Event::Project { call })));
        assert!(refused(stranger(Event::Dismiss { agent: 2 })));

        // A message is typed by the application; while its turn is owed the
        // agent is not at rest, and the application is told when to look
        // again. One that was never typed is said so instead of lost.
        let tell = Event::Tell {
            agent: 2,
            text: "Also add tests".into(),
        };
        assert_eq!(lead(tell), Answer::Told { working: false });
        assert_eq!(bridge.drain_requests().len(), 1);
        let (waiting, deadline) = bridge.collect_project(project, true);
        assert_eq!(waiting[0].status, Status::Working);
        let wait = deadline.unwrap().saturating_duration_since(Instant::now());
        assert!(wait <= TURN_GRACE && wait > TURN_GRACE / 2);
        bridge.undelivered(child, 6);
        assert!(bridge.drain_project_notes().is_empty());
        bridge.undelivered(child, 7);
        assert_eq!(
            bridge.drain_project_notes(),
            [(project, ProjectNote::Undelivered { agent: 2 })]
        );
        let (rested, deadline) = bridge.collect_project(project, true);
        assert_eq!((&rested[0].status, deadline), (&Status::Idle, None));
        // A request to a person is one once it has stood a moment.
        assert!(kid("c", signal(Signal::Prompt)));
        assert!(kid(
            "c",
            signal(ask(Attention::Permission, Some("Bash"), false))
        ));
        let (asked, deadline) = bridge.collect_project(project, true);
        assert_eq!(asked[0].status, Status::Working);
        assert!(deadline.unwrap().saturating_duration_since(Instant::now()) <= ASK_HOLD);
        // A lead presses no keys and takes no terminal's place.
        assert!(refused(lead(Event::Press {
            agent: 2,
            keys: vec!["enter".into()],
        })));
        assert!(refused(lead(Event::Launch)));
        // It closes an agent of its project, as the application is asked to.
        assert_eq!(lead(Event::Dismiss { agent: 2 }), Answer::Done);
        assert_eq!(
            bridge.drain_requests(),
            [AgentRequest::Close {
                pane: child,
                generation: 7,
                parent: Starter::Project(project),
            }]
        );
    }
    #[cfg(unix)]
    #[test]
    fn a_lead_names_a_branch_for_a_worktree_and_what_nobody_came_back_for_makes_room() {
        let bridge = AgentBridge::default();
        let (project, child) = (ProjectId::new(3), PaneId::new(2));
        let env = bridge
            .open_lead(project, AgentKind::Claude, Arc::new(|| {}))
            .unwrap();
        bridge.sync_projects(&[(project, false)]);
        let lead = |event| answer(&bridge.shared, message(&env.token, &env.run, event));
        let why = |answer: Answer| match answer {
            Answer::Refused { reason } => reason,
            other => panic!("{other:?}"),
        };
        let spawn = |worktree: Option<&str>, cwd: &str| Event::Spawn {
            kind: AgentKind::Codex,
            prompt: "Fix the login".into(),
            cwd: cwd.into(),
            model: None,
            effort: None,
            ultra: false,
            title: Some("login".into()),
            worktree: worktree.map(str::to_owned),
        };
        // The branch travels with the task; the application has git make
        // its worktree before the terminal.
        let Answer::Queued { request } = lead(spawn(Some("fix/login"), "")) else {
            panic!("the spawn was not queued");
        };
        let Some(AgentRequest::Spawn { task, cwd, .. }) = bridge.drain_requests().pop() else {
            panic!("the application was not asked");
        };
        assert_eq!(
            (task.worktree.as_deref(), cwd),
            (Some("fix/login"), PathBuf::new())
        );
        // A name git would not take, or one that would have to be changed
        // to be one, never reaches git; nor does a branch beside a directory.
        for branch in ["", "two words", "-D", "a..b", "x.lock", "HEAD", "feat/"] {
            let reason = why(lead(spawn(Some(branch), "")));
            assert!(reason.contains("branch name"), "{branch}: {reason}");
        }
        assert!(why(lead(spawn(Some("fix-login"), "/repo"))).contains("not both"));
        assert!(bridge.drain_requests().is_empty());
        // An agent that starts another makes its own worktree.
        terminal(&bridge, PaneId::new(9), "parent");
        assert!(apply_message(
            &bridge.shared,
            message("parent", "r", Event::Open { agent: codex() })
        ));
        let asked = answer(
            &bridge.shared,
            message("parent", "r", spawn(Some("fix-login"), "/repo")),
        );
        assert!(why(asked).contains("Only a project's lead"));
        // Without the word it reads as before.
        let wire = |more: &str| {
            let text = format!(r#"{{"event":"spawn","kind":"codex","prompt":"p","cwd":""{more}}}"#);
            serde_json::from_str::<Event>(&text).unwrap()
        };
        assert!(matches!(wire(""), Event::Spawn { worktree: None, .. }));
        assert!(matches!(
            wire(r#","worktree":"fix-login""#),
            Event::Spawn { worktree: Some(branch), .. } if branch == "fix-login"
        ));

        // What a project's agent asks before its task stands while its
        // terminal moves: the person opening it redraws it, and the row that
        // sent them there stays. It ends when the task is taken.
        terminal(&bridge, child, "kid");
        bridge.register_spawn(child, project, task);
        bridge.spawn_result(request, Ok(child));
        assert_eq!(
            lead(Event::SpawnResult { request }),
            Answer::Spawned {
                agent: 2,
                with: String::new(),
            }
        );
        assert!(matches!(
            answer(&bridge.shared, message("kid", "", Event::Launch)),
            Answer::Launch { .. }
        ));
        let status = || bridge.collect_project(project, false).0[0].status.clone();
        let asking = |screen: &str| Status::Asking {
            screen: screen.into(),
        };
        bridge.asking(child, 7, Some("Trust this folder?".into()));
        assert_eq!(status(), asking("Trust this folder?"));
        bridge.asking(child, 7, None);
        assert_eq!(status(), asking("Trust this folder?"));
        bridge.asking(child, 7, Some(String::new()));
        assert_eq!(status(), asking(""));
        let kid = |event| apply_message(&bridge.shared, message("kid", "c", event));
        assert!(kid(Event::Open { agent: codex() }));
        assert!(kid(Event::Activity {
            signal: Signal::Prompt
        }));
        assert_eq!(status(), Status::Working);

        // The person's own message for an agent goes the way its lead's do,
        // and a paused project sends none.
        assert!(kid(Event::Report {
            text: "Done.".into(),
            done: true
        }));
        assert!(kid(Event::Activity {
            signal: done(None, false)
        }));
        bridge.collect_project(project, true);
        assert_eq!(bridge.tell(project, 2, "Fix CI".into()), Ok(()));
        assert_eq!(
            bridge.drain_requests(),
            [AgentRequest::Tell {
                pane: child,
                generation: 7,
                text: "Fix CI".into(),
            }]
        );
        assert!(bridge.tell(project, 5, "Fix CI".into()).is_err());
        assert!(bridge.tell(ProjectId::new(8), 2, "Fix CI".into()).is_err());
        bridge.sync_projects(&[(project, true)]);
        assert_eq!(bridge.tell(project, 2, "Fix CI".into()), Err(PAUSED.into()));
        assert!(bridge.drain_requests().is_empty());
        bridge.sync_projects(&[(project, false)]);

        // A tool that gave up leaves its answer behind. Those make room
        // after a while, so they cannot keep later calls out; one that is
        // still waited for stays.
        let call = || Event::Project {
            call: ProjectCall::Report { agent: 2 },
        };
        let mut requests = Vec::new();
        for _ in 0..MAX_REQUESTS {
            let Answer::Queued { request } = lead(call()) else {
                panic!("a call was not queued");
            };
            bridge.call_result(request, Ok("said".into()));
            bridge.drain_requests();
            requests.push(request);
        }
        assert!(why(lead(call())).contains("busy"));
        {
            let mut shared = bridge.shared.lock().unwrap();
            // One the application has not answered is waited for longer.
            shared.calls.get_mut(&requests[0]).unwrap().1 = None;
            shared
                .spawns
                .insert(900, (project.into(), Some(Ok(child)), Instant::now()));
            shared.evict(Instant::now() + ABANDONED / 2);
            assert_eq!((shared.calls.len(), shared.spawns.len()), (MAX_REQUESTS, 1));
            let later = Instant::now() + ABANDONED;
            shared.evict(later);
            assert_eq!((shared.calls.len(), shared.spawns.len()), (1, 0));
            shared.evict(later + UNANSWERED);
            assert!(shared.calls.is_empty());
        }
        assert!(matches!(lead(call()), Answer::Queued { .. }));
        assert!(
            why(lead(Event::CallResult {
                request: requests[1]
            }))
            .contains("lost track")
        );
    }
    #[cfg(unix)]
    #[test]
    fn a_projects_agents_outlive_its_lead_and_an_earlier_lead_is_refused() {
        let bridge = AgentBridge::default();
        let (project, child) = (ProjectId::new(3), PaneId::new(2));
        let open = || {
            bridge
                .open_lead(project, AgentKind::Claude, Arc::new(|| {}))
                .unwrap()
        };
        let ask =
            |token: &str, run: &str, event| answer(&bridge.shared, message(token, run, event));
        let look = || Event::Collect {
            agent: None,
            take: false,
        };
        let refused = |answer: Answer| matches!(answer, Answer::Refused { .. });
        bridge.sync_projects(&[(project, false)]);
        let first = open();
        terminal(&bridge, child, "kid");
        bridge.register_spawn(
            child,
            Starter::Project(project),
            Task::new(AgentKind::Codex, "x"),
        );
        let link = [(child, Starter::Project(project))];
        bridge.sync_spawned(&link);
        assert!(matches!(
            ask(&first.token, &first.run, look()),
            Answer::Agents { agents } if agents.len() == 1
        ));
        // A request the lead's process did not live to hear answered goes
        // with it; the agents it started do not.
        let Answer::Queued { request } = ask(&first.token, &first.run, lead_spawn(Some("second")))
        else {
            panic!("the spawn was not queued");
        };
        bridge.close_lead(project);
        assert!(bridge.drain_requests().is_empty());
        assert!(refused(ask(&first.token, &first.run, look())));
        bridge.sync_spawned(&link);
        assert_eq!(bridge.collect_project(project, false).0.len(), 1);

        // Opened again it is another process: the earlier one's tool server
        // is refused, with either credential, and the agents are still there.
        let second = open();
        assert!(second.token != first.token && second.run != first.run);
        assert!(refused(ask(&first.token, &first.run, look())));
        assert!(refused(ask(&second.token, &first.run, look())));
        assert!(refused(ask(&first.token, &second.run, look())));
        assert!(refused(ask(
            &second.token,
            &second.run,
            Event::SpawnResult { request }
        )));
        assert!(matches!(
            ask(&second.token, &second.run, look()),
            Answer::Agents { agents } if agents.len() == 1 && agents[0].agent == 2
        ));
        // Opening over one that never closed ends the earlier one the same.
        let third = open();
        assert!(refused(ask(&second.token, &second.run, look())));
        assert!(matches!(
            ask(&third.token, &third.run, look()),
            Answer::Agents { agents } if agents.len() == 1
        ));

        // An agent that left is kept until the project was told, and for as
        // long as its conversation can be opened again, lead or no lead.
        assert!(apply_message(
            &bridge.shared,
            message("kid", "c", Event::Open { agent: codex() })
        ));
        bridge.close_lead(project);
        bridge.sync_spawned(&[]);
        bridge.sync_spawned(&[]);
        let (ended, _) = bridge.collect_project(project, true);
        assert!(matches!(ended[0].status, Status::Ended { .. }) && ended[0].reopens);
        bridge.sync_spawned(&[]);
        assert_eq!(bridge.collect_project(project, false).0.len(), 1);
        // A project that is gone takes its lead and its agents' records.
        let fourth = open();
        bridge.sync_projects(&[]);
        assert!(refused(ask(&fourth.token, &fourth.run, look())));
        assert!(bridge.collect_project(project, false).0.is_empty());

        // A saved agent of a project returns as one, before any lead opens.
        let mut options = SessionOptions {
            shell: Some("/bin/sh".into()),
            ..Default::default()
        };
        bridge
            .prepare(
                child,
                1,
                &mut options,
                Some(&codex()),
                Some(Starter::Project(project)),
                Arc::new(|| {}),
            )
            .unwrap();
        assert_eq!(
            (options.args[4].as_str(), options.args[6].as_str()),
            ("--agent-restore", "member")
        );
        bridge.sync_projects(&[(project, false)]);
        bridge.sync_spawned(&link);
        let (restored, _) = bridge.collect_project(project, false);
        assert_eq!(restored.len(), 1);
        assert!(!restored[0].news);
        // Its CLI takes a moment to open the conversation again. That it
        // then rests is how it was left: nothing for its lead to hear.
        assert_eq!(restored[0].status, Status::Starting);
        let token = bridge.shared.lock().unwrap().slots[&child].token.clone();
        assert!(apply_message(
            &bridge.shared,
            message(&token, "again", Event::Open { agent: codex() })
        ));
        let (rested, _) = bridge.collect_project(project, true);
        assert_eq!(rested[0].status, Status::Idle);
        assert!(!rested[0].news, "a restored agent at rest was news");
    }
    #[cfg(unix)]
    #[test]
    fn what_a_project_kept_of_its_agents_names_them_again_and_reopens_a_closed_one() {
        // A new run: the bridge knows nothing of the project's agents.
        let bridge = AgentBridge::default();
        let (project, restored, closed) = (ProjectId::new(3), PaneId::new(2), PaneId::new(5));
        let (first, second) = (agent(FIRST), agent(SECOND));
        // Nothing is kept for a project the model does not hold.
        bridge.remember(
            project,
            5,
            Some("docs"),
            Some("Docs done."),
            Some(&second),
            (None, None, false),
        );
        bridge.sync_projects(&[(project, false)]);
        assert!(bridge.collect_project(project, false).0.is_empty());

        // A saved agent's terminal returns under its project, without the
        // name its lead gave it; the project gives the name and the reply
        // of its last turn back.
        let mut options = SessionOptions {
            shell: Some("/bin/sh".into()),
            ..Default::default()
        };
        bridge
            .prepare(
                restored,
                1,
                &mut options,
                Some(&first),
                Some(Starter::Project(project)),
                Arc::new(|| {}),
            )
            .unwrap();
        let link = [(restored, Starter::Project(project))];
        bridge.sync_spawned(&link);
        assert_eq!(bridge.collect_project(project, false).0[0].title, None);
        assert!(bridge.last_report(project, 2).is_err());
        bridge.remember(
            project,
            2,
            Some("auth"),
            Some("PR #214 is up."),
            None,
            (None, None, false),
        );
        // One whose terminal was closed is known again by its conversation,
        // with what it ran with.
        bridge.remember(
            project,
            5,
            Some("docs"),
            Some("Docs done."),
            Some(&second),
            (Some("sonnet"), Some("high"), true),
        );
        bridge.sync_spawned(&link);
        bridge.sync_spawned(&link);
        let (agents, deadline) = bridge.collect_project(project, true);
        // Only the restored one is waited for, until its CLI is back.
        assert!(deadline.is_some_and(|at| at <= Instant::now() + START_GRACE));
        let seen: Vec<_> = agents
            .iter()
            .map(|report| {
                (
                    report.agent,
                    report.title.as_deref(),
                    matches!(report.status, Status::Ended { .. }),
                    report.reopens,
                    report.news,
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (2, Some("auth"), false, false, false),
                (5, Some("docs"), true, true, false)
            ],
            "named again, and nothing of it is news"
        );
        assert_eq!(bridge.last_report(project, 2).unwrap(), "PR #214 is up.");
        assert_eq!(bridge.last_report(project, 5).unwrap(), "Docs done.");
        // What the bridge knows already is not replaced.
        bridge.remember(
            project,
            2,
            Some("other"),
            Some("other"),
            Some(&second),
            (None, None, false),
        );
        bridge.remember(
            project,
            5,
            Some("other"),
            Some("other"),
            Some(&first),
            (None, None, false),
        );
        assert_eq!(bridge.last_report(project, 2).unwrap(), "PR #214 is up.");
        assert_eq!(bridge.last_report(project, 5).unwrap(), "Docs done.");
        // Without a conversation there is nothing to open again, and a
        // terminal that is open is not a closed agent.
        let lost = AgentSession {
            session_id: None,
            ..agent(FIRST)
        };
        bridge.remember(
            project,
            6,
            Some("lost"),
            None,
            Some(&lost),
            (None, None, false),
        );
        bridge.remember(project, 7, Some("gone"), None, None, (None, None, false));
        terminal(&bridge, PaneId::new(8), "shell");
        bridge.remember(
            project,
            8,
            Some("taken"),
            None,
            Some(&second),
            (None, None, false),
        );
        assert_eq!(bridge.collect_project(project, false).0.len(), 2);

        // The project's lead opens the closed one again: it goes on in the
        // conversation it had, where it worked, under the name it had.
        let env = bridge
            .open_lead(project, AgentKind::Claude, Arc::new(|| {}))
            .unwrap();
        let lead = |event| answer(&bridge.shared, message(&env.token, &env.run, event));
        assert!(matches!(
            lead(Event::Reopen {
                agent: 2,
                text: "more".into()
            }),
            Answer::Refused { reason } if reason.contains("still open")
        ));
        let Answer::Queued { request } = lead(Event::Reopen {
            agent: 5,
            text: "Add a changelog entry.".into(),
        }) else {
            panic!("the closed agent was not opened again");
        };
        let requests = bridge.drain_requests();
        let [
            AgentRequest::Spawn {
                request: asked,
                parent,
                task,
                cwd,
                ..
            },
        ] = &requests[..]
        else {
            panic!("{requests:?}");
        };
        assert_eq!((*asked, *parent), (request, Starter::Project(project)));
        assert_eq!(cwd, &second.cwd);
        assert_eq!(task.resume, Some((closed, SECOND.to_owned())));
        assert_eq!(
            (task.kind, task.title.as_deref()),
            (AgentKind::Claude, Some("docs"))
        );
        assert!(task.prompt.ends_with("\n\nAdd a changelog entry."));
        // It runs with what it ran with before Neptune was closed.
        assert_eq!(
            (
                task.model.as_deref(),
                task.effort.as_deref(),
                task.ultracode
            ),
            (Some("sonnet"), Some("high"), true)
        );
        // Opened in a new terminal, the record of the closed one is gone.
        let reopened = PaneId::new(9);
        let Some(AgentRequest::Spawn { task, .. }) = requests.into_iter().next() else {
            unreachable!();
        };
        bridge.register_spawn(reopened, Starter::Project(project), task);
        let (agents, _) = bridge.collect_project(project, false);
        let numbers: Vec<u64> = agents.iter().map(|report| report.agent).collect();
        assert_eq!(numbers, [2, 9]);
        assert_eq!(agents[1].title.as_deref(), Some("docs"));
        // No more closed agents are kept than a lead can use.
        for n in 20..40 {
            bridge.remember(project, n, None, None, Some(&second), (None, None, false));
        }
        assert_eq!(
            bridge.collect_project(project, false).0.len(),
            2 + AgentSession::MAX_SPAWNED
        );
    }
    #[cfg(unix)]
    #[test]
    fn a_paused_or_full_project_starts_and_sends_nothing() {
        let bridge = AgentBridge::default();
        let (project, child) = (ProjectId::new(3), PaneId::new(2));
        let env = bridge
            .open_lead(project, AgentKind::Claude, Arc::new(|| {}))
            .unwrap();
        let lead = |event| answer(&bridge.shared, message(&env.token, &env.run, event));
        let paused =
            |answer: Answer| matches!(answer, Answer::Refused { reason } if reason == PAUSED);
        terminal(&bridge, child, "kid");
        bridge.register_spawn(
            child,
            Starter::Project(project),
            Task::new(AgentKind::Codex, "x"),
        );
        bridge.sync_projects(&[(project, true)]);
        assert!(paused(lead(lead_spawn(Some("more")))));
        assert!(paused(lead(Event::Tell {
            agent: 2,
            text: "hello".into()
        })));
        assert!(paused(lead(Event::Reopen {
            agent: 2,
            text: "hello".into()
        })));
        assert!(bridge.drain_requests().is_empty());
        // The application hears of each refusal, for the project's chat.
        assert_eq!(
            bridge.drain_project_notes(),
            [(project, ProjectNote::Paused); 3]
        );
        // It still sees its agents and may close one.
        assert!(matches!(
            lead(Event::Collect { agent: None, take: false }),
            Answer::Agents { agents } if agents.len() == 1
        ));
        assert_eq!(lead(Event::Dismiss { agent: 2 }), Answer::Done);
        bridge.drain_requests();

        // Resumed, it starts agents up to the most a project keeps open.
        bridge.sync_projects(&[(project, false)]);
        for pane in 10..10 + Project::MAX_AGENTS as u64 {
            assert!(matches!(
                lead(lead_spawn(Some("more"))),
                Answer::Queued { .. }
            ));
            bridge.register_spawn(
                PaneId::new(pane),
                Starter::Project(project),
                Task::new(AgentKind::Codex, "x"),
            );
        }
        assert!(matches!(
            lead(lead_spawn(Some("one too many"))),
            Answer::Refused { reason } if reason.contains("the most Neptune keeps")
        ));
        // Its tool takes ultra, so what it is refused says how to ask for it.
        let ultra = Event::Spawn {
            kind: AgentKind::Claude,
            prompt: "x".into(),
            cwd: PathBuf::new(),
            model: None,
            effort: Some("ultra".into()),
            ultra: false,
            title: Some("deep".into()),
            worktree: None,
        };
        assert!(matches!(
            lead(ultra),
            Answer::Refused { reason }
                if reason.starts_with("effort for Claude Code") && reason.contains("ultra to true")
        ));
        // Asked for by name, a lead's agent starts in its ultra mode:
        // ultracode for Claude Code at whatever effort, the Ultra effort for
        // Codex, which no other effort goes with.
        bridge.close(PaneId::new(10));
        let ultra = |kind, effort: Option<&str>| Event::Spawn {
            kind,
            prompt: "x".into(),
            cwd: PathBuf::new(),
            model: None,
            effort: effort.map(str::to_owned),
            ultra: true,
            title: Some("deep".into()),
            worktree: None,
        };
        let asked = |event| match lead(event) {
            Answer::Queued { .. } => match bridge.drain_requests().pop() {
                Some(AgentRequest::Spawn { task, .. }) => Ok((task.effort, task.ultracode)),
                _ => panic!("the application was not asked"),
            },
            Answer::Refused { reason } => Err(reason),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            asked(ultra(AgentKind::Claude, Some("high"))),
            Ok((Some("high".into()), true))
        );
        assert_eq!(
            asked(ultra(AgentKind::Codex, None)),
            Ok((Some("ultra".into()), false))
        );
        assert!(
            asked(ultra(AgentKind::Codex, Some("low")))
                .unwrap_err()
                .contains("not both")
        );
        // Another starter's agents are not counted against the project's.
        terminal(&bridge, PaneId::new(1), "one");
        assert!(apply_message(
            &bridge.shared,
            message(
                "one",
                "a",
                Event::Open {
                    agent: agent(FIRST)
                }
            )
        ));
        let spawn = Event::Spawn {
            kind: AgentKind::Codex,
            prompt: "x".into(),
            cwd: std::env::temp_dir(),
            model: None,
            effort: None,
            ultra: false,
            title: Some("not a lead's".into()),
            worktree: None,
        };
        assert!(matches!(
            answer(&bridge.shared, message("one", "a", spawn)),
            Answer::Queued { .. }
        ));
        // Only a project names its agents.
        assert!(matches!(
            bridge.drain_requests().pop(),
            Some(AgentRequest::Spawn { parent, task, .. })
                if parent == PaneId::new(1) && task.title.is_none()
        ));
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
                Start::Resume(&agent(FIRST), Role::Agent),
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
