//! Projects in the application: each one's lead, chat and inbox, and what
//! passes between them and the model once a frame. Processes and folders
//! belong to workers; a frame only takes what they have ready.
//!
//! A project's chat and what it knows of its agents are kept in its folder
//! by the store's worker. Every entry of a chat goes through `Run::say`,
//! which queues it for writing; a project is read back the first time it is
//! looked at, spoken to or has agents to answer for.
//!
//! What a project's agents share, its context folder, is read and written by
//! the same worker. The rules of who writes what are applied here, before a
//! request is handed over; each answer carries what the folder holds, which
//! is what leads are told of it and what the tab lists.
//!
//! A project's watches wake its lead without an agent. The store's worker
//! keeps their clock and says when one is due; whether it then fires is
//! decided here, where the caps and the pause are known. Pull requests are
//! read by their own watcher: what it knows is compared here with what the
//! lead was last told.
use super::App;
use crate::{
    agent_activity::Activity,
    projects::{
        context::{self, Digest, Place, Recipient, Sent, Writer},
        inbox::{AgentEvent, Fire, Guard, Inbox, Message, Next, What},
        prompt::{self, Doing},
        registry::{Counters, LeadRef, Registry, Saved, Task as Kept, TaskState},
        schedule,
        settings::{self, AgentSettings, LeadSettings, Settings},
        subscription::{self, Subscription, Trigger, Why},
        transcript::{self, Ended, Entry, MAX_ATTACHMENTS, Origin, Source, Transcript, clip},
    },
    runtime::{
        agents::{
            self, AgentReport, ProjectCall, ProjectNote, Starter, Status, Task, WatchTrigger,
        },
        project_lead::{
            self, Failure, Launch, Lead, LeadEvent, Mcp, Outcome, Refused, SessionRef,
            State as LeadState, Stream, Wake,
        },
        project_store::{self, ContextOp, Loaded, Out, Protection, Reply, Store},
        pull_requests::{self, Lookup},
    },
    ui::{
        self, OverlayState,
        project::{
            Body, Catalog, Choice, Choices, Event, File, FollowUp, Member, Need, Remedy, WatchAsk,
        },
    },
};
use eframe::egui;
use neptune_model::{
    AgentKind, AgentSession, Command, PaneId, Project, ProjectId, ProjectKey, PullRequest,
    WorkspaceId, Worktree,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Leads that run at once. Another project's lead starts when one rests.
const MAX_LEADS: usize = 3;
/// A lead with nothing to do and no agent at work is let go after this long;
/// its session is resumed when it is next needed.
const IDLE: Duration = Duration::from_secs(30 * 60);
/// What Neptune did for leads that no frame has written down yet.
const MAX_DONE: usize = 64;
/// One message to a lead.
const MAX_MESSAGE: usize = 32 * 1024;
/// What a lead's CLI said as it failed, as the chat quotes it.
const MAX_SAID: usize = 300;
/// Entries that may wait for the store's worker before a project takes no
/// more from its lead and its agents. Nothing is dropped: they wait where
/// they are, and their CLI waits with them.
const MAX_UNWRITTEN: usize = 64;
/// How soon a project looks again when the store's worker had no room.
const RETRY: Duration = Duration::from_millis(50);
/// What was asked of the store about projects' context and is not answered.
const MAX_TICKETS: usize = 256;
/// How often the Context part of the tab reads the folder while in view.
const LOOK_EVERY: Duration = Duration::from_secs(2);
/// How long a turn waits for the folder to be read before it goes without.
const LOOK_WAIT: Duration = Duration::from_secs(1);
/// What a card of the chat quotes of a decision.
const MAX_CARD: usize = 120;
/// Pull requests a lead asked about that nothing else follows, and how long
/// each is read for.
const MAX_ASKED: usize = 8;
const ASKED_FOR: Duration = Duration::from_secs(10 * 60);
/// What is kept of what a lead said in a turn, to tell an update from
/// "Nothing to report."
const MAX_HEARD: usize = 128;
/// The model every lead is asked for, whatever its project's settings say.
/// Unset, the settings decide; set by native checks to keep them cheap.
const MODEL: &str = "NEPTUNE_LEAD_MODEL";

/// A lead as the application drives it. The process behind it is the
/// runtime's; tests put a script in its place.
pub(super) trait LeadHandle {
    fn state(&self) -> LeadState;
    fn send_turn(&self, id: &str, text: &str, pictures: &[PathBuf]) -> Result<(), Refused>;
    fn interrupt(&self);
    fn try_events(&self) -> Vec<LeadEvent>;
    fn revision(&self) -> u64;
    fn stream(&self) -> Stream;
    fn last_error(&self) -> Option<String>;
}
impl LeadHandle for Lead {
    fn state(&self) -> LeadState {
        Lead::state(self)
    }
    fn send_turn(&self, id: &str, text: &str, pictures: &[PathBuf]) -> Result<(), Refused> {
        Lead::send_turn(self, id, text, pictures)
    }
    fn interrupt(&self) {
        Lead::interrupt(self)
    }
    fn try_events(&self) -> Vec<LeadEvent> {
        Lead::try_events(self)
    }
    fn revision(&self) -> u64 {
        Lead::revision(self)
    }
    fn stream(&self) -> Stream {
        Lead::stream(self)
    }
    fn last_error(&self) -> Option<String> {
        Lead::last_error(self)
    }
}
pub(super) type StartLead = Box<dyn FnMut(Launch, Wake) -> Box<dyn LeadHandle>>;

/// Something Neptune carried out, or could not, because a lead asked.
pub(super) enum Done {
    Started {
        agent: PaneId,
        kind: AgentKind,
        title: Option<String>,
        /// It was opened again, not started.
        again: bool,
        /// Where it works,
        cwd: PathBuf,
        /// and the branch of the worktree that is, where it has one.
        branch: Option<String>,
        /// The closed terminal whose conversation it goes on with.
        resumed: Option<PaneId>,
        /// The model, effort and ultracode it was started with.
        with: (Option<String>, Option<String>, bool),
    },
    NotStarted {
        again: bool,
        reason: String,
    },
    /// A message was taken to be typed for it.
    Told(PaneId),
    Closed(PaneId),
    /// The lead closed an agent whose terminal the person had given a tab:
    /// it left the project and its terminal stays.
    Released(PaneId),
}
impl Done {
    /// The tool of the lead that asked for it.
    fn tool(&self) -> &'static str {
        match self {
            Self::Started { again: true, .. } | Self::NotStarted { again: true, .. } => {
                "reopen_agent"
            }
            Self::Started { .. } | Self::NotStarted { .. } => "spawn_agent",
            Self::Told(_) => "send_agent_message",
            Self::Closed(_) | Self::Released(_) => "close_agent",
        }
    }
    fn summary(&self) -> String {
        match self {
            Self::Started {
                agent,
                kind,
                title,
                again,
                branch,
                ..
            } => {
                let verb = if *again { "Reopened" } else { "Started" };
                let kind = ui::agents::kind_name(*kind);
                let mut summary = match title {
                    Some(title) => format!("{verb} agent {agent} · {kind} · {title}"),
                    None => format!("{verb} agent {agent} · {kind}"),
                };
                if let Some(branch) = branch {
                    summary.push_str(&format!(" · in its own worktree, on {branch}"));
                }
                summary
            }
            Self::NotStarted {
                again: true,
                reason,
            } => {
                format!("Could not reopen an agent: {reason}")
            }
            Self::NotStarted { reason, .. } => format!("Could not start an agent: {reason}"),
            Self::Told(agent) => format!("Took a message for agent {agent}"),
            Self::Closed(agent) => format!("Closed agent {agent}"),
            Self::Released(agent) => format!(
                "Agent {agent} left the project. Its terminal stays open: you had opened it \
                 as a tab"
            ),
        }
    }
}

/// An agent a project's lead asked for, held until the project's context was
/// read: its brief opens with what the project keeps.
pub(super) struct Briefing {
    pub request: u64,
    pub project: ProjectId,
    pub generation: u64,
    pub task: Task,
    pub cwd: PathBuf,
}
/// Who waits for an answer of the store about a project's context.
enum Waiting {
    /// Nobody: only what the folder holds was asked for.
    Look,
    /// A tool's call, answered through the bridge. `card` is what the chat
    /// says once it is carried out, by the tool that asked.
    Call {
        request: u64,
        card: Option<(&'static str, String)>,
    },
    Brief(Box<Briefing>),
    /// The person, who is told if what they asked for failed.
    Person,
}

/// A call of the lead that Neptune has not carried out yet.
struct Pending {
    call: String,
    tool: String,
    line: String,
}
/// The turn the lead is in.
struct Running {
    id: String,
    /// The last block whose text is whole.
    block: u32,
    tools: Vec<Pending>,
    /// The newest entry of the chat when the turn was put together, where
    /// the turn carried events.
    through: Option<u64>,
    /// Who began it.
    origin: Origin,
    /// The watches whose fires it carries.
    fires: Vec<u64>,
    /// The beginning of what the lead said in it.
    said: String,
    /// When it began.
    since: Instant,
}
#[derive(Default)]
struct Session {
    id: Option<String>,
    /// A handshake succeeded in it once: it is resumed from then on.
    opened: bool,
}
/// How far a project has been read back from its folder.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Loading {
    No,
    Asked,
    Done,
}

/// One project while the application runs.
struct Run {
    key: ProjectKey,
    lead: Option<Box<dyn LeadHandle>>,
    session: Session,
    chat: Transcript,
    inbox: Inbox,
    /// Its agents as the bridge last described them.
    reports: Vec<AgentReport>,
    /// The agents its lead was told ask something before their task, and
    /// have not taken it since.
    asked: BTreeSet<u64>,
    /// The agents that ended a turn with a report the person has not
    /// opened their terminal since: ready for review.
    unread: BTreeSet<u64>,
    /// Why its lead does not go on without the person.
    trouble: Option<Failure>,
    /// A usage limit that ends by itself then.
    limited: Option<Instant>,
    /// Why Neptune paused it.
    guard: Option<Guard>,
    turn: Option<Running>,
    /// The newest entry when a turn was last put together.
    taken: Option<u64>,
    stream: Stream,
    /// A line under the chat, such as a retry under way.
    status: Option<String>,
    /// Since when its lead has had nothing to do.
    idle: Instant,
    loading: Loading,
    /// Why nothing of it is written and its lead stays off.
    protected: Option<Protection>,
    /// What waits to be written to its chat, oldest first.
    out: VecDeque<Out>,
    /// The agents it started, as its folder remembers them.
    tasks: Registry,
    counters: Counters,
    /// Its watches, as its folder keeps them.
    subscriptions: Vec<Subscription>,
    /// When the store's clock was last told each of its schedules is due.
    armed: Vec<(u64, u64)>,
    /// The schedules that came due while Neptune was closed: when each
    /// runs, once, and how many runs it missed.
    late: BTreeMap<u64, (u64, u64)>,
    /// Something of it needed the person when that was last looked at.
    needed: bool,
    /// What it is called and which CLI leads it, as last seen in the model,
    /// and between them where it works: the directory it was made in, which
    /// its folder keeps. Empty until that is known.
    named: (String, PathBuf, AgentKind),
    /// What the person chose for its lead and for the agents it starts.
    settings: Settings,
    /// What its lead's process was started with. A lead at rest that was
    /// started with something else is let go before its next turn and
    /// resumed with what is chosen now.
    launched: Option<LeadSettings>,
    /// The model its lead runs on, as its CLI named it.
    model: Option<String>,
    /// Its state changed since the store was last handed it.
    dirty: bool,
    /// Its folder holds entries older than the chat does here,
    more: bool,
    /// and they were asked for.
    fetching: bool,
    /// The closed agents it remembers were named to the bridge.
    reminded: bool,
    /// Requests of its lead that were refused because it is paused, which
    /// no card names yet.
    refused: u8,
    /// Something of it could not be written.
    unsaved: bool,
    /// What its context folder held when it was last read,
    context: Digest,
    /// and when that was.
    read: Option<Instant>,
    /// Since when it is being read again,
    looking: Option<Instant>,
    /// and something happened since that its lead's next turn should know.
    stale: bool,
    /// What its lead was last handed of the folder.
    sent: Option<Sent>,
    /// What the turn being handed over says of it, kept once the turn begins.
    offered: Option<Sent>,
    /// Its lead's next turn carries all of it: the conversation is new, or
    /// its CLI summarised it.
    full: bool,
    /// The newest decision each of its agents was told, by the agent's number.
    heard: BTreeMap<u64, u64>,
    /// When its last entry was written: each gets a second of its own.
    stamped: u64,
}
impl Run {
    fn new(key: ProjectKey, now: Instant) -> Self {
        Self {
            key,
            lead: None,
            session: Session::default(),
            chat: Transcript::default(),
            inbox: Inbox::default(),
            reports: Vec::new(),
            asked: BTreeSet::new(),
            unread: BTreeSet::new(),
            trouble: None,
            limited: None,
            guard: None,
            turn: None,
            taken: None,
            stream: Stream::default(),
            status: None,
            idle: now,
            loading: Loading::No,
            protected: None,
            out: VecDeque::new(),
            tasks: Registry::default(),
            counters: Counters::default(),
            subscriptions: Vec::new(),
            armed: Vec::new(),
            late: BTreeMap::new(),
            needed: false,
            named: (String::new(), PathBuf::new(), AgentKind::Claude),
            settings: Settings::default(),
            launched: None,
            model: None,
            dirty: false,
            more: false,
            fetching: false,
            reminded: false,
            refused: 0,
            unsaved: false,
            context: Digest::default(),
            read: None,
            looking: None,
            stale: false,
            sent: None,
            offered: None,
            full: true,
            heard: BTreeMap::new(),
            stamped: 0,
        }
    }
    /// Takes what the store read of its context folder.
    fn context(&mut self, digest: Digest, now: Instant) {
        self.stamped = self.stamped.max(digest.newest());
        if self.read.is_none() && !self.full {
            // A lead that goes on with an earlier conversation was handed
            // the folder there; only what changes from now is news.
            self.sent = Some(Sent::of(&digest));
        }
        self.read = Some(now);
        self.context = digest;
    }
    /// The time a new entry of its context is written under.
    fn stamp(&mut self) -> u64 {
        self.stamped = unix_now().max(self.stamped + 1);
        self.stamped
    }
    /// What its lead's next turn says of the folder, and what the lead will
    /// have been handed once that turn begins.
    fn context_lines(&mut self) -> String {
        if self.read.is_none() {
            // Not read yet: said at the first turn after it was.
            self.offered = None;
            return String::new();
        }
        self.offered = Some(Sent::of(&self.context));
        match self.sent.filter(|_| !self.full) {
            Some(sent) => context::lead(Recipient::LeadTurn, &self.context, &sent),
            None => context::lead(Recipient::LeadNew, &self.context, &Sent::default()),
        }
    }
    /// What Neptune did for the lead's call of `tool`, in place of the line
    /// that said the lead had asked.
    fn card(&mut self, tool: &str, summary: String) {
        self.settled(tool);
        self.say(Entry::Tool {
            name: tool.into(),
            summary,
            ok: true,
        });
    }
    /// The lead's call of `tool` was carried out: the line that said it had
    /// asked goes.
    fn settled(&mut self, tool: &str) {
        if let Some(running) = &mut self.turn
            && let Some(at) = running
                .tools
                .iter()
                .position(|pending| pending.tool == tool)
        {
            running.tools.remove(at);
        }
    }
    /// When the store's clock is to say that each of its schedules is due.
    /// One that was missed while Neptune was closed has its own time.
    fn times(&self) -> Vec<(u64, u64)> {
        if self.protected.is_some() || self.loading != Loading::Done {
            return Vec::new();
        }
        self.subscriptions
            .iter()
            .filter(|watch| watch.runs() && watch.minutes().is_some())
            .filter_map(|watch| {
                let at = self.late.get(&watch.id).map(|late| late.0).or(watch.next)?;
                Some((watch.id, at))
            })
            .collect()
    }
    /// Gives each schedule that came due while Neptune was closed one run,
    /// a moment after Neptune opened at `opened`. `ahead` counts the late
    /// runs placed so far, of every project.
    fn make_up(&mut self, opened: SystemTime, at: SystemTime, ahead: &mut u64) {
        if self.protected.is_some() {
            return;
        }
        let due = self
            .subscriptions
            .iter()
            .filter(|watch| watch.runs())
            // One whose last fire was read back from the chat and waits for
            // the lead has its run already: a second would say it twice.
            .filter(|watch| !self.inbox.awaits(watch.id))
            .filter_map(|watch| Some((watch.id, watch.minutes()?, watch.next?)));
        for late in schedule::late(due, opened, at, *ahead) {
            self.late.insert(late.id, (late.at, late.runs));
            *ahead += 1;
        }
    }
    /// An earlier fire of `watch` waits for the lead, or the lead is at it.
    fn firing(&self, watch: u64) -> bool {
        self.inbox.awaits(watch)
            || self
                .turn
                .as_ref()
                .is_some_and(|turn| turn.fires.contains(&watch))
    }
    /// What a watch has to say: a row of the chat, and words for the lead's
    /// next turn. False, and nothing said, where it cannot wait for the
    /// lead: an earlier fire of the same watch does, or too much else.
    fn fire(&mut self, row: Entry, watch: Option<u64>, told: String, now: Instant) -> bool {
        if !self.inbox.push_fire(Fire { watch, text: told }, now) {
            return false;
        }
        self.say(row);
        self.stale = true;
        true
    }
    /// The agent that linked the pull request at `url`, with what its lead
    /// called it.
    fn owner(&self, url: &str) -> Option<(u64, String)> {
        self.tasks
            .tasks()
            .iter()
            .rev()
            .find(|task| {
                task.pull_requests
                    .iter()
                    .any(|linked| linked.eq_ignore_ascii_case(url))
            })
            .map(|task| (task.n, task.title.clone()))
    }
    /// Adds an entry to the chat, and to what the store is handed to write.
    fn say(&mut self, entry: Entry) -> u64 {
        let seq = self.chat.push(entry, unix_now());
        if self.protected.is_none()
            && let Some(record) = self.chat.records().back()
        {
            self.out.push_back(Out::Entry(record.clone()));
        }
        seq
    }
    /// Takes what the store read of the project. What was cut short when
    /// Neptune last closed is said once: a turn without an end, words of
    /// the user no turn took, and what its agents did that the lead was
    /// never told, which waits for the lead again.
    ///
    /// Where it works is what its folder says. A folder that does not say,
    /// such as one whose state was lost, leaves it `fallback`: its
    /// workspace's directory.
    fn loaded(&mut self, loaded: Loaded, now: Instant, fallback: &Path) {
        let lead = self.named.2;
        // What was said before it was read back follows what was read.
        let early: Vec<Entry> = self
            .chat
            .records()
            .iter()
            .map(|record| record.entry.clone())
            .collect();
        self.out.clear();
        self.chat = Transcript::restore(loaded.records, loaded.last);
        self.more = loaded.more;
        self.protected = loaded.protection;
        self.loading = Loading::Done;
        // A lead that never took a turn in this chat has been handed
        // nothing of what the project keeps.
        self.full = !self.more
            && !self
                .chat
                .records()
                .iter()
                .any(|record| matches!(record.entry, Entry::Turn { .. }));
        // Its file is made, or made anew, with what is known of it now.
        let kept = loaded
            .saved
            .as_ref()
            .map(|saved| saved.directory.clone())
            .filter(|directory| directory.is_absolute());
        self.dirty = kept.is_none()
            || loaded
                .saved
                .as_ref()
                .is_none_or(|saved| saved.name != self.named.0);
        self.named.1 = kept.unwrap_or_else(|| fallback.to_path_buf());
        if let Some(saved) = loaded.saved {
            if let Some(kept) = saved.lead.filter(|kept| kept.kind == lead) {
                self.session = Session {
                    opened: kept.opened && kept.session.is_some(),
                    id: kept.session,
                };
            }
            self.tasks = Registry::restore(saved.tasks);
            self.counters = saved.counters;
            self.subscriptions = saved.subscriptions;
            self.settings = saved.settings;
        }
        if self.protected.is_some() {
            // Shown as far as it was read; nothing is added to it, and
            // nothing waits for a lead that stays off.
            self.inbox = Inbox::default();
            return;
        }
        if loaded.recovered {
            self.notice(
                "What this project had saved about its lead and agents was damaged. A copy \
                 is in the project folder, and the project goes on without it; the chat is \
                 as it was.",
            );
        }
        let recovery = transcript::recover(self.chat.records());
        if let Some(turn) = recovery.unfinished {
            self.say(Entry::End {
                turn,
                outcome: Ended::Failed,
                cost: None,
            });
            self.notice(transcript::CLOSED_MID_TURN);
        }
        for record in recovery.undelivered {
            let Entry::Event {
                source,
                agent,
                what,
                text,
            } = record.entry
            else {
                continue;
            };
            match (source, agent) {
                (Source::Agent, Some(agent)) => {
                    let mut event = AgentEvent::new(agent, None, What::Past(what));
                    if !text.is_empty() {
                        event.replies.push(text);
                    }
                    self.inbox.push_event(event, now);
                }
                (Source::Agent, None) => {}
                // What a watch said waits under its watch, so that the same
                // watch firing again as Neptune opens is not told twice.
                (source, about) => {
                    let watch = about.filter(|_| source != Source::Worktree);
                    self.inbox.push_fire(
                        Fire {
                            watch,
                            text: prompt::past(&what, &text),
                        },
                        now,
                    );
                }
            }
        }
        // A fire whose turn Neptune was closed in, or that the chat no longer
        // holds, is not one the lead is still at.
        for watch in &mut self.subscriptions {
            if let Some(last) = &mut watch.last
                && last.outcome == subscription::Outcome::Waiting
                && !self.inbox.awaits(watch.id)
            {
                last.outcome = subscription::Outcome::Stopped;
                self.dirty = true;
            }
        }
        if recovery.unsent {
            self.notice(transcript::CLOSED_UNSENT);
        }
        for entry in early {
            self.say(entry);
        }
    }
    /// What its folder keeps beside the chat.
    fn saved(&self) -> Saved {
        Saved {
            name: self.named.0.clone(),
            directory: self.named.1.clone(),
            lead: Some(LeadRef {
                kind: self.named.2,
                session: self.session.id.clone(),
                opened: self.session.opened,
            }),
            tasks: self.tasks.tasks().to_vec(),
            subscriptions: self.subscriptions.clone(),
            counters: self.counters.clone(),
            settings: self.settings.clone(),
            ..Saved::default()
        }
    }
    /// Begins a new chat with a lead that remembers nothing of this one.
    /// The chat so far stays in the folder as the earlier one.
    fn new_chat(&mut self) {
        self.lead = None;
        self.session = Session::default();
        self.inbox = Inbox::default();
        self.turn = None;
        self.taken = None;
        self.stream = Stream::default();
        self.status = None;
        self.asked.clear();
        self.refused = 0;
        self.guard = None;
        self.sent = None;
        self.offered = None;
        self.full = true;
        // Fires that waited for the lead went with the chat they were in.
        for watch in &mut self.subscriptions {
            if let Some(last) = &mut watch.last
                && last.outcome == subscription::Outcome::Waiting
            {
                last.outcome = subscription::Outcome::Stopped;
            }
        }
        // A limit is the provider's and outlasts the chat.
        if !matches!(self.trouble, Some(Failure::Limit { .. })) {
            self.trouble = None;
        }
        if self.protected.is_none() {
            self.out.push_back(Out::Archive);
        }
        self.chat.clear();
        self.more = false;
        self.fetching = false;
        self.dirty = true;
        self.notice(
            "New chat. The earlier one is kept with the project's saved files; the lead starts without \
             it and is told which agents the project has.",
        );
    }
    /// Nothing was said in the chat yet, by the person, the lead or an
    /// agent. Setting it aside would put it in the place of the earlier
    /// chat, which is the one worth keeping.
    fn unbegun(&self) -> bool {
        !self.more
            && self
                .chat
                .records()
                .iter()
                .all(|record| matches!(record.entry, Entry::Notice { .. }))
    }
    fn notice(&mut self, text: impl Into<String>) {
        self.say(Entry::Notice { text: text.into() });
    }
    /// News of an agent: a row of the chat, and words for the lead's next
    /// turn. What a terminal showed goes to the lead and is not kept.
    fn event(&mut self, event: AgentEvent, now: Instant) {
        let text: Vec<&str> = event.replies.iter().map(String::as_str).collect();
        let text = clip(&text.join("\n\n"), prompt::MAX_REPORT).to_owned();
        let what = match &event.title {
            Some(title) => format!("“{title}” {}", event.what.words()),
            None => event.what.words(),
        };
        self.say(Entry::Event {
            source: Source::Agent,
            agent: Some(event.agent),
            what,
            text,
        });
        self.inbox.push_event(event, now);
        self.stale = true;
    }
    /// Writes down what Neptune did, in place of the line that said the
    /// lead had asked.
    fn did(&mut self, done: Done) {
        if let Done::Started {
            agent,
            kind,
            title,
            again,
            cwd,
            resumed,
            with,
            ..
        } = &done
        {
            let now = unix_now();
            if !again {
                self.counters.spawned(now);
            }
            self.tasks.started(
                Kept {
                    n: agent.get(),
                    title: title.clone().unwrap_or_default(),
                    kind: *kind,
                    session: None,
                    cwd: cwd.clone(),
                    worktree: None,
                    pull_requests: Vec::new(),
                    state: TaskState::Starting,
                    last_report: None,
                    started: now,
                    ended: None,
                    merged: false,
                    model: with.0.clone(),
                    effort: with.1.clone(),
                    ultracode: with.2,
                },
                resumed.map(PaneId::get),
            );
            self.dirty = true;
        }
        // One that left with its terminal open is not opened again beside
        // the one that still runs.
        if let Done::Released(agent) = &done
            && let Some(task) = self.tasks.get_mut(agent.get())
        {
            self.dirty |= task.session.take().is_some();
        }
        if let Some(running) = &mut self.turn
            && let Some(at) = running
                .tools
                .iter()
                .position(|pending| pending.tool == done.tool())
        {
            running.tools.remove(at);
        }
        self.say(Entry::Tool {
            name: done.tool().into(),
            summary: done.summary(),
            ok: !matches!(done, Done::NotStarted { .. }),
        });
    }
    /// Whether what the bridge says of an agent is news for the lead.
    /// What a terminal shows before its task changes as the person opens
    /// it and answers: the lead was told that it waits for them, and has
    /// nothing new to hear until the task is taken.
    fn hears(&mut self, report: &AgentReport) -> bool {
        let before_task = matches!(report.status, Status::Asking { .. } | Status::Starting);
        if !before_task {
            self.asked.remove(&report.agent);
        }
        if !report.news {
            return false;
        }
        if before_task && report.replies.is_empty() {
            if self.asked.contains(&report.agent) {
                return false;
            }
            if matches!(report.status, Status::Asking { .. }) {
                self.asked.insert(report.agent);
            }
        }
        true
    }
    /// The newest decision `agent` was told. One started in an earlier run
    /// was handed what had been decided by then.
    fn heard_by(&self, agent: u64) -> u64 {
        self.heard
            .get(&agent)
            .copied()
            .or_else(|| self.tasks.get(agent).map(|task| task.started))
            .unwrap_or_else(|| self.context.newest())
    }
    /// The lead has nothing running and nothing handed to it.
    fn resting(&self) -> bool {
        self.turn.is_none() && !self.inbox.sending()
    }
    fn held(&self) -> bool {
        self.trouble.is_some() || self.protected.is_some()
    }
    /// What only the person can clear, beside its agents that wait.
    fn stuck(&self) -> usize {
        usize::from(self.trouble.is_some())
            + usize::from(self.guard.is_some())
            + usize::from(self.protected.is_some())
            + usize::from(self.unsaved)
    }
    /// The agents that wait for a person, by their terminal.
    fn waiting(&self) -> impl Iterator<Item = &AgentReport> {
        self.reports.iter().filter(|report| {
            matches!(
                report.status,
                Status::Waiting { .. } | Status::Asking { .. }
            )
        })
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// What the lead's CLI is called in words to the person.
fn lead_name(kind: AgentKind) -> &'static str {
    ui::agents::kind_name(kind)
}
/// What went wrong with a lead, and what the person can do about it.
fn trouble_text(
    failure: &Failure,
    kind: AgentKind,
    now: Instant,
    until: Option<Instant>,
) -> String {
    let name = lead_name(kind);
    let command = match kind {
        AgentKind::Codex => "codex",
        _ => "claude",
    };
    match failure {
        Failure::NotInstalled => format!(
            "{name} isn't installed or isn't on PATH. Install it, check that `{command}` runs \
             in a terminal, then try again."
        ),
        Failure::Auth if kind == AgentKind::Codex => {
            format!("{name} isn't signed in. Run `codex login` in a terminal, then try again.")
        }
        Failure::Auth => {
            format!(
                "{name} isn't signed in. Run `{command}` in a terminal and sign in, then try again."
            )
        }
        Failure::ToolsUnavailable => format!(
            "{name} could not reach Neptune's tools, so the lead was not started. Try again."
        ),
        Failure::Limit { .. } => match until.map(|until| until.saturating_duration_since(now)) {
            Some(left) if !left.is_zero() => format!(
                "{name}'s usage limit was reached. It resets in {}; the lead goes on then.",
                span(left)
            ),
            _ => format!("{name}'s usage limit was reached. Try again once it has reset."),
        },
        Failure::Overloaded => {
            format!(
                "{name}'s service is overloaded right now. Send your message again in a moment."
            )
        }
        Failure::PromptTooLong => {
            "The conversation has grown too long for the lead to continue.".to_owned()
        }
        Failure::SessionLost => {
            "The lead's earlier conversation is no longer on this computer. It starts a new \
             one."
                .to_owned()
        }
        Failure::Transport => format!("{name} stopped unexpectedly."),
        Failure::Protocol => {
            format!("Neptune and this version of {name} did not understand each other.")
        }
        Failure::Provider(said) => format!("The lead's turn failed: {said}"),
    }
}
/// "2 h 10 min", "5 min", "less than a minute".
fn span(left: Duration) -> String {
    let minutes = left.as_secs().div_ceil(60);
    match (minutes / 60, minutes % 60) {
        (0, 0) => "less than a minute".into(),
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// What the lead is doing, said quietly until Neptune has done it.
fn pending_line(tool: &str, input: &serde_json::Value) -> String {
    let agent = input["agent_id"].as_u64();
    let numbered = |verb: &str| match agent {
        Some(agent) => format!("{verb} agent {agent}…"),
        None => format!("{verb} an agent…"),
    };
    match tool {
        "spawn_agent" => match input["title"].as_str().filter(|title| !title.is_empty()) {
            Some(title) => format!("Starting agent “{}”…", clip(title, 60)),
            None => "Starting an agent…".into(),
        },
        "send_agent_message" => numbered("Messaging"),
        "reopen_agent" => numbered("Reopening"),
        "close_agent" => numbered("Closing"),
        "agent_report" => numbered("Reading the reply of"),
        "list_agents" => "Looking at its agents…".into(),
        "read_context" => "Reading what the project keeps…".into(),
        "write_context" => "Writing to what the project keeps…".into(),
        "record_decision" => "Recording a decision…".into(),
        "add_subscription" => "Adding a watch…".into(),
        "list_subscriptions" => "Looking at its watches…".into(),
        "remove_subscription" => "Removing a watch…".into(),
        "pull_request_status" => "Asking how a pull request stands…".into(),
        other => format!("Using {other}…"),
    }
}

fn doing(status: &Status) -> Doing {
    match status {
        Status::Starting => Doing::Starting,
        Status::Asking { .. } => Doing::Asking,
        Status::Working => Doing::Working,
        Status::Idle => Doing::Idle,
        Status::Waiting { attention } => Doing::NeedsPerson(*attention),
        Status::Ended { .. } => Doing::Ended,
    }
}
fn doing_words(doing: Doing) -> &'static str {
    use crate::agent_activity::Attention;
    match doing {
        Doing::Starting => "Starting",
        Doing::Working => "Working",
        Doing::Idle => "Idle",
        Doing::NeedsPerson(Attention::Permission) => "Needs permission",
        Doing::NeedsPerson(Attention::Question) => "Asked a question",
        Doing::NeedsPerson(Attention::Plan) => "Plan needs approval",
        Doing::NeedsPerson(Attention::Input) => "Needs input",
        Doing::Asking => "Needs your answer",
        Doing::Ended => "Ended",
    }
}
/// What an agent's report is news of, if anything the lead should hear.
fn what(report: &AgentReport) -> What {
    match &report.status {
        Status::Idle => What::Finished,
        Status::Waiting { attention } => What::NeedsPerson(*attention),
        Status::Asking { .. } => What::Asking,
        Status::Ended { reason } => What::Ended(reason.clone()),
        Status::Starting => What::Stalled,
        Status::Working => What::Progress,
    }
}

/// An agent of a project as both the tab and the lead's turn describe it.
struct Row {
    pane: PaneId,
    generation: u64,
    kind: AgentKind,
    title: String,
    doing: Doing,
    elapsed: Option<Duration>,
    /// The branch of its worktree, where it works in one.
    branch: Option<String>,
    /// The pull requests it linked, with what is known of each.
    pull_requests: Vec<ui::helpers::LinkedPullRequest>,
    /// Its last turn ended with a report nobody has opened its terminal
    /// since.
    ready: bool,
}
/// What the roster says an agent is doing: one of six words, whatever
/// exactly it waits for.
fn state_word(doing: Doing, ready: bool) -> &'static str {
    match doing {
        Doing::Starting => "Starting",
        Doing::Working => "Working",
        Doing::NeedsPerson(_) | Doing::Asking => "Needs you",
        Doing::Idle if ready => "Ready for review",
        Doing::Idle => "Idle",
        Doing::Ended => "Ended",
    }
}
/// What the person has an agent told about its pull request, in fixed words.
fn follow_up(ask: FollowUp, link: &PullRequest, unresolved: u8) -> String {
    match ask {
        FollowUp::Checks => format!(
            "The checks of your pull request {} are failing. Find out why, fix it, push the \
             fix, and report what was wrong and what you changed.",
            link.url()
        ),
        FollowUp::Comments => format!(
            "Your pull request {} has {unresolved} unresolved review {}. Read each one, \
             address it, push, and report what you changed and anything you did not change, \
             with the reason.",
            link.url(),
            if unresolved == 1 {
                "comment"
            } else {
                "comments"
            }
        ),
    }
}

/// The newest pull request among `links` that `ask` is for: one in review
/// whose checks fail, or with comments nobody resolved, and how many.
fn needed(links: &[ui::helpers::LinkedPullRequest], ask: FollowUp) -> Option<(&PullRequest, u8)> {
    links.iter().rev().find_map(|linked| {
        let status = linked.lookup.status()?;
        let wanted = match ask {
            FollowUp::Checks => status.checks() == pull_requests::Checks::Failing,
            FollowUp::Comments => status.unresolved() > 0,
        };
        wanted.then(|| (&linked.link, status.unresolved()))
    })
}

/// How long a CLI that was not found is taken to be missing before it is
/// looked for again.
const LOOK_AGAIN: Duration = Duration::from_secs(60);
/// Said where settings are changed before a project is read back, or of
/// one that is read-only.
const SETTINGS_LOCKED: &str = "This project's settings cannot be changed right now.";
/// Looks for the CLIs that can lead, off the frame.
type FindLeads = Box<dyn Fn(Wake) -> Receiver<Vec<AgentKind>>>;
/// The CLIs installed here that can lead a project, for the form that
/// offers them. Nothing of it is saved.
struct Installed {
    found: Option<Vec<AgentKind>>,
    looking: Option<Receiver<Vec<AgentKind>>>,
    looked: Option<Instant>,
    find: FindLeads,
}
impl Installed {
    fn new() -> Self {
        Self {
            // A test's form offers both without looking on its host.
            found: cfg!(test).then(|| project_lead::LEADS.to_vec()),
            looking: None,
            looked: None,
            find: Box::new(project_lead::installed),
        }
    }
    /// Whether to look: never yet, or a while ago with one still missing.
    fn due(&self, now: Instant) -> bool {
        self.looking.is_none()
            && match &self.found {
                None => true,
                Some(found) => {
                    found.len() < project_lead::LEADS.len()
                        && self
                            .looked
                            .is_none_or(|at| now.saturating_duration_since(at) >= LOOK_AGAIN)
                }
            }
    }
    /// Takes the answer when it waits. Never waits for it.
    fn poll(&mut self, now: Instant) {
        let Some(looking) = &self.looking else {
            return;
        };
        match looking.try_recv() {
            Ok(found) => self.found = Some(found),
            Err(TryRecvError::Empty) => return,
            // Nobody looked: each start says for itself what is missing.
            Err(TryRecvError::Disconnected) => {
                self.found
                    .get_or_insert_with(|| project_lead::LEADS.to_vec());
            }
        }
        self.looking = None;
        self.looked = Some(now);
    }
}
/// A project being made: its folder first, off the frame.
struct Creating {
    workspace: WorkspaceId,
    name: String,
    goal: String,
    lead: AgentKind,
    /// Where it works: the directory of the terminal in front when the
    /// person asked for it.
    directory: PathBuf,
    settings: LeadSettings,
}

pub(super) struct Projects {
    runs: BTreeMap<ProjectId, Run>,
    /// The projects' folders, `projects` under the data root.
    store: Store,
    /// Projects whose workspace was closed: their folders stay and are
    /// offered again where they worked.
    kept: Vec<project_store::Kept>,
    /// The projects that were open when `kept` was last asked for.
    kept_for: Option<Vec<ProjectKey>>,
    /// Projects the person removed: their folders are deleted once their
    /// runs are let go.
    removing: BTreeSet<ProjectKey>,
    /// Folders to delete and chats to finish writing that the worker had
    /// no room for.
    removals: Vec<ProjectKey>,
    leaving: Vec<(ProjectKey, VecDeque<Out>)>,
    /// This application, which serves a lead's tools.
    exe: Option<PathBuf>,
    model: Option<String>,
    /// What the settings offer, the same for every project.
    catalog: Catalog,
    start: StartLead,
    installed: Installed,
    creating: Option<Creating>,
    done: Vec<(ProjectId, Done)>,
    turns: u64,
    /// What was asked about projects' context, by the number it was asked
    /// under.
    tickets: BTreeMap<u64, Waiting>,
    ticket: u64,
    /// When Neptune started: what watches missed before then is made up
    /// for a moment after it.
    opened: SystemTime,
    /// The late runs placed so far, each a little after the one before.
    late: u64,
    /// Pull requests a lead asked about that nothing else follows.
    asked: Vec<(PullRequest, Instant)>,
    /// The agents a message of the person's is on its way to, until the
    /// frame that types it.
    own: Vec<PaneId>,
    /// What the person was told through the desktop, by project.
    #[cfg(test)]
    banners: Vec<(ProjectId, &'static str)>,
}
/// A pull request as a watch remembers it.
fn seen_of(status: pull_requests::Status) -> subscription::Seen {
    use pull_requests::{Checks, State};
    use subscription::{Checks as Kept, Review};
    subscription::Seen {
        state: match status.state {
            State::Open => Review::Open,
            State::Draft => Review::Draft,
            State::Merged => Review::Merged,
            State::Closed => Review::Closed,
        },
        checks: match status.checks {
            Checks::None => Kept::None,
            Checks::Passing => Kept::Passing,
            Checks::Pending => Kept::Pending,
            Checks::Failing => Kept::Failing,
        },
        unresolved: status.unresolved,
    }
}
/// What the settings of a project offer: for each CLI that can lead, the
/// models that have a name here and the effort levels it takes, as a lead
/// and as an agent a lead starts.
fn catalog() -> Catalog {
    let choice = |value: &'static str| Choice {
        value,
        label: settings::label(value),
    };
    let of = |kind: AgentKind, efforts: Vec<&'static str>| Choices {
        kind,
        models: settings::models(kind).iter().copied().map(choice).collect(),
        efforts: efforts.into_iter().map(choice).collect(),
    };
    Catalog {
        lead: project_lead::LEADS
            .iter()
            .map(|kind| of(*kind, project_lead::effort_levels(*kind)))
            .collect(),
        agents: project_lead::LEADS
            .iter()
            .map(|kind| of(*kind, agents::effort_levels(*kind).to_vec()))
            .collect(),
    }
}
/// A model as the person gave it: none for nothing, and an error for what
/// no CLI takes as a model's name.
fn chosen_model(kind: AgentKind, model: Option<String>) -> Result<Option<String>, String> {
    match model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        None => Ok(None),
        Some(model) => settings::model_name(model)
            .map(|model| Some(model.to_owned()))
            .ok_or_else(|| format!("That is not a model name {} takes.", lead_name(kind))),
    }
}
/// What the person asked a lead of `kind` to run with, as its CLI takes it.
fn chosen_lead(
    kind: AgentKind,
    model: Option<String>,
    effort: Option<String>,
) -> Result<LeadSettings, String> {
    let effort = effort
        .map(|effort| effort.trim().to_ascii_lowercase())
        .filter(|effort| !effort.is_empty());
    let levels = project_lead::effort_levels(kind);
    if let Some(effort) = &effort
        && !levels.contains(&effort.as_str())
    {
        return Err(format!(
            "A {} lead's effort is one of: {}.",
            lead_name(kind),
            levels.join(", ")
        ));
    }
    Ok(LeadSettings {
        model: chosen_model(kind, model)?,
        effort,
    })
}
/// What the person asked the agents of `kind` to run with, by the rules an
/// agent's start is held to.
fn chosen_agents(
    kind: AgentKind,
    model: Option<String>,
    effort: Option<String>,
    ultracode: Option<bool>,
) -> Result<AgentSettings, String> {
    let (effort, _) = agents::effort_level(kind, effort.as_deref(), false)?;
    Ok(AgentSettings {
        model: chosen_model(kind, model)?,
        effort,
        // Codex has no such switch: its Ultra is an effort.
        ultracode: ultracode.filter(|_| kind == AgentKind::Claude),
    })
}
/// The lead said only that there is nothing to say.
fn nothing_to_report(said: &str) -> bool {
    said.trim()
        .trim_end_matches('.')
        .eq_ignore_ascii_case("nothing to report")
}
impl Projects {
    /// The pull requests projects follow beyond those of the workspace in
    /// view: what their watches name, what their agents linked, and what a
    /// lead asked about a moment ago.
    pub fn links(&self, model: &neptune_model::Model) -> Vec<PullRequest> {
        let mut links: Vec<PullRequest> = Vec::new();
        for (id, run) in &self.runs {
            links.extend(
                model
                    .project_agents(*id)
                    .flat_map(|pane| pane.pull_requests().iter().cloned()),
            );
            links.extend(run.subscriptions.iter().filter_map(Subscription::link));
        }
        links.extend(self.asked.iter().map(|(link, _)| link.clone()));
        links
    }
    /// `ephemeral` is a run that saves nothing, such as a capture.
    pub fn new(root: PathBuf, ephemeral: bool) -> Self {
        Self {
            runs: BTreeMap::new(),
            store: Store::new(root, ephemeral),
            kept: Vec::new(),
            kept_for: None,
            removing: BTreeSet::new(),
            removals: Vec::new(),
            leaving: Vec::new(),
            exe: std::env::current_exe().ok(),
            model: std::env::var(MODEL).ok().filter(|model| !model.is_empty()),
            catalog: catalog(),
            start: Box::new(|launch, wake| Box::new(Lead::start(launch, wake))),
            installed: Installed::new(),
            creating: None,
            done: Vec::new(),
            turns: 0,
            tickets: BTreeMap::new(),
            ticket: 0,
            opened: SystemTime::now(),
            late: 0,
            asked: Vec::new(),
            own: Vec::new(),
            #[cfg(test)]
            banners: Vec::new(),
        }
    }
    /// Asks the store's worker something about the context of the project
    /// named `key`. Handed back where the worker has no room for it.
    fn ask(&mut self, key: &ProjectKey, op: ContextOp, waiting: Waiting) -> Result<(), Waiting> {
        if self.tickets.len() >= MAX_TICKETS {
            return Err(waiting);
        }
        self.ticket += 1;
        if !self.store.context(key, self.ticket, op) {
            return Err(waiting);
        }
        self.tickets.insert(self.ticket, waiting);
        Ok(())
    }
    /// Where the project named `key` keeps what its agents share.
    pub fn context_folder(&self, key: &ProjectKey) -> PathBuf {
        project_store::folder(self.store.root(), key).join(context::FOLDER)
    }
    /// Holds an agent a lead asked for until its project's context was read
    /// from its folder, so that what the user changed there by hand is in
    /// the brief. Handed back where it cannot wait: it is then briefed from
    /// what was read last.
    pub fn brief_after_reading(&mut self, briefing: Box<Briefing>) -> Result<(), Box<Briefing>> {
        let Some(key) = self.runs.get(&briefing.project).map(|run| run.key.clone()) else {
            return Err(briefing);
        };
        self.ask(&key, ContextOp::Look, Waiting::Brief(briefing))
            .map_err(|waiting| match waiting {
                Waiting::Brief(briefing) => briefing,
                _ => unreachable!("what was handed over is handed back"),
            })
    }
    /// The task of an agent of `project` as it is handed over: a new agent's
    /// opens with what the project keeps, which is in `folder`; one opened
    /// again is told what was decided since it was last told.
    /// `worktree` is the one git made for a new agent.
    pub fn brief(
        &self,
        project: ProjectId,
        folder: &std::path::Path,
        task: &Task,
        worktree: Option<&Worktree>,
    ) -> String {
        let run = self.runs.get(&project);
        let nothing = Digest::default();
        let digest = run.map_or(&nothing, |run| &run.context);
        match &task.resume {
            None => context::preamble(
                digest,
                folder,
                worktree.map(|worktree| (worktree.branch.as_str(), worktree.path.as_path())),
                &task.prompt,
            ),
            Some((before, _)) => {
                let since = run.map_or(0, |run| run.heard_by(before.get()));
                format!("{}{}", context::later(digest, since), task.prompt)
            }
        }
    }
    /// The worktree the closed agent `before` of `project` worked in, where
    /// `cwd`, the directory it is opened in again, is still that worktree's.
    pub fn worktree_of(
        &self,
        project: ProjectId,
        before: PaneId,
        cwd: &std::path::Path,
    ) -> Option<Worktree> {
        self.runs
            .get(&project)?
            .tasks
            .get(before.get())?
            .worktree
            .clone()
            .filter(|worktree| worktree.path == cwd)
    }
    /// Whether the message being typed for the agent in `pane` is one the
    /// person asked for, which the chat names by itself. Asked once.
    pub fn asked_by_person(&mut self, pane: PaneId) -> bool {
        match self.own.iter().position(|asked| *asked == pane) {
            Some(at) => {
                self.own.remove(at);
                true
            }
            None => false,
        }
    }
    /// The agent in `agent` was handed everything `project` has decided.
    pub fn briefed(&mut self, project: ProjectId, agent: PaneId) {
        if let Some(run) = self.runs.get_mut(&project) {
            run.heard.insert(agent.get(), run.context.newest());
        }
    }
    /// A later message of the lead to its agent `agent`, led by what the
    /// project decided since that agent was last told.
    pub fn told(&mut self, project: ProjectId, agent: PaneId, text: String) -> String {
        let Some(run) = self.runs.get_mut(&project) else {
            return text;
        };
        let since = run.heard_by(agent.get());
        let lead = context::later(&run.context, since);
        run.heard
            .insert(agent.get(), run.context.newest().max(since));
        format!("{lead}{text}")
    }
    /// Projects whose store does its work as its results are taken, so a
    /// test sees each result on the poll after its cause.
    #[cfg(test)]
    fn inline(root: PathBuf) -> Self {
        let mut projects = Self::new(root.clone(), false);
        projects.store = Store::inline(root);
        projects
    }
    /// Notes what Neptune did for a project's lead, for its chat.
    pub fn record(&mut self, project: ProjectId, done: Done) {
        if self.done.len() < MAX_DONE {
            self.done.push((project, done));
        }
    }
    /// Lets every lead go: each one's worker ends its process. What the
    /// projects have to save is written before this returns, within a
    /// moment. Called as the application closes, never from a frame.
    pub fn shutdown(&mut self) {
        for run in self.runs.values_mut() {
            run.lead = None;
            if run.loading != Loading::Done || run.protected.is_some() {
                continue;
            }
            self.store.write(&run.key, &mut run.out);
            if std::mem::take(&mut run.dirty) {
                self.store.save(&run.key, run.saved());
            }
        }
        for (key, out) in &mut self.leaving {
            self.store.write(key, out);
        }
        self.store.flush(Duration::from_secs(2));
    }
    /// The reply the last turn of `agent` ended with, as `project` kept it.
    pub fn report(&self, project: ProjectId, agent: u64) -> Option<String> {
        self.runs
            .get(&project)?
            .tasks
            .get(agent)?
            .last_report
            .clone()
    }
    /// Whether `project` may start another agent today.
    pub fn may_spawn(&self, project: ProjectId) -> bool {
        self.runs
            .get(&project)
            .is_none_or(|run| run.counters.may_spawn(unix_now()))
    }
    /// Looks for the CLIs that can lead while the form that offers them is
    /// in view, off the frame.
    /// Where `project` works, once that is known: the directory it was
    /// made in.
    pub fn directory(&self, project: ProjectId) -> Option<&Path> {
        self.runs
            .get(&project)
            .map(|run| run.named.1.as_path())
            .filter(|directory| !directory.as_os_str().is_empty())
    }
    /// Gives a new agent of `project` what the person set for the agents
    /// of its CLI. Each value that is set replaces what the lead asked for.
    pub fn agent_settings(&self, project: ProjectId, task: &mut Task) {
        let Some(set) = self
            .runs
            .get(&project)
            .and_then(|run| run.settings.agents(task.kind))
        else {
            return;
        };
        (task.model, task.effort, task.ultracode) = set.over(
            task.kind,
            task.model.take(),
            task.effort.take(),
            task.ultracode,
        );
    }
    /// What a lead of `kind` is started with: the project's settings, or
    /// the model a check names for every lead, which is then asked to
    /// think little as well where its CLI takes that with the model.
    fn lead_settings(&self, kind: AgentKind, chosen: &LeadSettings) -> LeadSettings {
        match &self.model {
            Some(model) => LeadSettings {
                model: Some(model.clone()),
                effort: (kind == AgentKind::Codex).then(|| "low".to_owned()),
            },
            None => chosen.clone(),
        }
    }
    pub fn look_for_leads(&mut self, ctx: &egui::Context, panel: &Panel) {
        let installed = &mut self.installed;
        if matches!(panel.body, PanelBody::Empty { .. } | PanelBody::Project(_))
            && installed.due(Instant::now())
        {
            let ctx = ctx.clone();
            installed.looking = Some((installed.find)(Arc::new(move || ctx.request_repaint())));
        }
    }
    /// Whether the lead of `project` has a turn under way, or one about to
    /// be sent.
    pub fn lead_busy(&self, project: ProjectId) -> bool {
        self.runs
            .get(&project)
            .is_some_and(|run| run.turn.is_some() || run.inbox.sending())
    }
    /// The parts of the tab that borrow a project's own state.
    pub fn view<'a>(&'a self, panel: &'a Panel, shown: usize) -> Body<'a> {
        match &panel.body {
            PanelBody::Nothing => Body::Nothing,
            PanelBody::Remote => Body::Remote,
            PanelBody::Unsupported => Body::Unsupported,
            PanelBody::Empty {
                workspace,
                directory,
                starting,
                reopen,
            } => Body::Empty {
                workspace: *workspace,
                directory,
                starting: *starting,
                reopen: reopen.as_deref(),
                leads: self.installed.found.as_deref(),
                catalog: &self.catalog,
            },
            PanelBody::Project(project) => {
                let Shown {
                    id,
                    name,
                    directory,
                    lead,
                    paused,
                    needs,
                    members,
                    open,
                    follow,
                    pulls,
                    thinking,
                    elapsed,
                    usage,
                    pending,
                    files,
                    watches,
                    proposed,
                } = &**project;
                let Some(run) = self.runs.get(id) else {
                    return Body::Nothing;
                };
                Body::Project(Box::new(ui::project::Project {
                    id: *id,
                    name,
                    directory,
                    directory_path: &run.named.1,
                    lead: *lead,
                    lead_model: run.model.as_deref(),
                    settings: &run.settings,
                    // Its process was started with something else than is
                    // chosen now, and is let go before its next turn.
                    pending: run
                        .launched
                        .as_ref()
                        .is_some_and(|with| *with != self.lead_settings(*lead, &run.settings.lead)),
                    catalog: &self.catalog,
                    leads: self.installed.found.as_deref(),
                    paused: *paused,
                    needs,
                    members,
                    chat: ui::chat::Chat {
                        project: *id,
                        proposed,
                        pulls,
                        records: run.chat.records(),
                        shown,
                        members: open,
                        follow,
                        thinking: *thinking,
                        pending,
                        // What streams is shown until its block is whole.
                        streaming: run
                            .turn
                            .as_ref()
                            .filter(|turn| run.stream.block > turn.block)
                            .map(|_| run.stream.text.as_str())
                            .filter(|text| !text.is_empty()),
                        status: run.status.as_deref(),
                        more: run.more && !run.fetching,
                    },
                    busy: run.turn.is_some() || run.inbox.sending(),
                    elapsed: *elapsed,
                    usage: usage.as_deref(),
                    queued: run.inbox.queued(),
                    locked: run.protected.is_some(),
                    context: ui::project::Context {
                        files,
                        instructions: &run.context.instructions,
                        read: run.read.is_some(),
                    },
                    watches,
                }))
            }
        }
    }
}

/// What the tab shows that is gathered from the model each frame.
pub(super) struct Panel {
    body: PanelBody,
}
enum PanelBody {
    Nothing,
    Remote,
    Unsupported,
    Empty {
        workspace: WorkspaceId,
        directory: String,
        starting: bool,
        /// A project that worked here before, by name.
        reopen: Option<String>,
    },
    Project(Box<Shown>),
}
/// A project as the tab shows it.
struct Shown {
    id: ProjectId,
    name: String,
    /// Where it works, as the person knows it.
    directory: String,
    lead: AgentKind,
    paused: bool,
    needs: Vec<Need>,
    members: Vec<Member>,
    open: Vec<(PaneId, u64)>,
    /// The agents at rest whose pull request has failing checks, or
    /// comments nobody resolved.
    follow: Vec<(PaneId, bool, bool)>,
    /// The watches of pull requests one of those agents owns, with what
    /// that pull request needs.
    pulls: Vec<(u64, PaneId, bool, bool)>,
    /// Its lead's turn has begun and nothing of it has come yet.
    thinking: bool,
    /// How long its lead's turn has run.
    elapsed: Option<Duration>,
    /// What the turns of the chat in memory cost, as their CLI put it.
    usage: Option<String>,
    pending: Vec<String>,
    /// What its context folder holds, the longest untouched first.
    files: Vec<File>,
    watches: Vec<ui::project::Watch>,
    /// The watches its lead proposed that wait for a yes or a no.
    proposed: Vec<u64>,
}
impl Panel {
    pub fn has_agents(&self) -> bool {
        matches!(&self.body, PanelBody::Project(shown) if !shown.members.is_empty())
    }
    /// Its lead is in a turn, whose time the tab counts.
    pub fn running(&self) -> bool {
        matches!(&self.body, PanelBody::Project(shown) if shown.elapsed.is_some())
    }
}

/// Where a project made in `workspace` now would work: the directory of the
/// terminal in front, as its shell last reported it, which is the folder
/// the panel's Changes and Files show. The workspace's own directory where
/// that terminal names none. Read from the model: nothing is looked up.
fn focused_directory(workspace: &neptune_model::Workspace) -> &Path {
    workspace
        .pane(workspace.active())
        .map(|pane| pane.cwd())
        .filter(|directory| directory.is_absolute())
        .unwrap_or(workspace.cwd())
}

impl App {
    /// Where `project` works: the directory it was made in. Until its
    /// folder is read back, and where the folder does not say, that is its
    /// workspace's directory.
    pub(super) fn project_home(&self, project: ProjectId) -> Option<PathBuf> {
        if let Some(directory) = self.projects.directory(project) {
            return Some(directory.to_path_buf());
        }
        let model = self.controller.model();
        model
            .project(project)
            .and_then(|project| model.workspace(project.workspace()))
            .map(|workspace| workspace.cwd().to_path_buf())
    }

    pub(super) fn active_project(&self) -> Option<&Project> {
        let model = self.controller.model();
        model
            .active_workspace()
            .and_then(|workspace| model.project_of(workspace))
    }

    /// The agents of `project` that have a terminal, oldest first.
    fn project_rows(&self, project: ProjectId, now: Instant) -> Vec<Row> {
        let run = self.projects.runs.get(&project);
        let reports = run.map_or(&[][..], |run| &run.reports);
        self.controller
            .model()
            .project_agents(project)
            .map(|pane| {
                let report = reports
                    .iter()
                    .find(|report| report.agent == pane.id().get());
                let seen = self.agents.get(pane.id());
                let kind = report
                    .map(|report| report.kind)
                    .or(pane.agent().map(|agent| agent.kind))
                    .unwrap_or(AgentKind::Claude);
                Row {
                    pane: pane.id(),
                    generation: pane.generation(),
                    kind,
                    // After a restart the project's folder remembers what
                    // its lead called the agent.
                    title: report
                        .and_then(|report| report.title.clone())
                        .or_else(|| {
                            let kept = run?.tasks.get(pane.id().get())?;
                            (!kept.title.is_empty()).then(|| kept.title.clone())
                        })
                        .unwrap_or_else(|| ui::agents::kind_name(kind).to_owned()),
                    // The bridge knows a started agent best; one restored
                    // from an earlier run is known by what it reports.
                    doing: match (report, seen) {
                        (Some(report), _) => doing(&report.status),
                        (None, Some((Activity::Working, _))) => Doing::Working,
                        (None, Some((Activity::Idle, _))) => Doing::Idle,
                        (None, Some((Activity::NeedsInput(attention), _))) => {
                            Doing::NeedsPerson(attention)
                        }
                        (None, None) => Doing::Starting,
                    },
                    elapsed: seen.map(|(_, since)| now.saturating_duration_since(since)),
                    branch: pane.worktree().map(|worktree| worktree.branch.clone()),
                    pull_requests: pane
                        .pull_requests()
                        .iter()
                        .map(|link| ui::helpers::LinkedPullRequest {
                            link: link.clone(),
                            lookup: self.pull_requests.lookup(link),
                        })
                        .collect(),
                    ready: run.is_some_and(|run| run.unread.contains(&pane.id().get())),
                }
            })
            .collect()
    }

    /// How much the project of the workspace in view needs the person for.
    pub(super) fn project_needs(&self) -> usize {
        self.active_project()
            .and_then(|project| self.projects.runs.get(&project.id()))
            .map_or(0, |run| run.waiting().count() + run.stuck())
    }

    /// What the tab shows for the workspace in view.
    pub(super) fn project_panel(&self, shown: bool) -> Panel {
        let model = self.controller.model();
        let workspace = model
            .active_workspace()
            .and_then(|id| model.workspace(id))
            .filter(|_| shown);
        let Some(workspace) = workspace else {
            return Panel {
                body: PanelBody::Nothing,
            };
        };
        let Some(project) = model.project_of(workspace.id()) else {
            let body = if workspace.remote().is_some() {
                PanelBody::Remote
            } else if cfg!(not(unix)) {
                // The bridge a lead's tools go through listens on Unix only.
                PanelBody::Unsupported
            } else {
                PanelBody::Empty {
                    workspace: workspace.id(),
                    directory: ui::helpers::path_label(focused_directory(workspace), 40),
                    starting: self
                        .projects
                        .creating
                        .as_ref()
                        .is_some_and(|creating| creating.workspace == workspace.id()),
                    reopen: self
                        .projects
                        .kept
                        .iter()
                        .find(|kept| kept.directory == focused_directory(workspace))
                        .map(|kept| kept.name.clone()),
                }
            };
            return Panel { body };
        };
        let Some(run) = self.projects.runs.get(&project.id()) else {
            return Panel {
                body: PanelBody::Nothing,
            };
        };
        let now = Instant::now();
        let mut rows = self.project_rows(project.id(), now);
        // Those that wait for the person lead the roster, then those with
        // something to look at; each group in the order they were started.
        rows.sort_by_key(|row| match row.doing {
            Doing::NeedsPerson(_) | Doing::Asking => 0,
            Doing::Idle if row.ready => 1,
            _ => 2,
        });
        let mut needs = Vec::new();
        for row in &rows {
            let detail = match row.doing {
                Doing::NeedsPerson(_) | Doing::Asking => doing_words(row.doing),
                _ => continue,
            };
            needs.push(Need {
                title: format!(
                    "{} {} · {}",
                    row.pane,
                    ui::agents::kind_name(row.kind),
                    row.title
                ),
                detail: format!("{detail}. Only you can answer, in its terminal."),
                remedy: Remedy::Open(row.pane, row.generation),
            });
        }
        if let Some(protection) = run.protected {
            needs.push(Need {
                title: "This project is read-only".into(),
                detail: match protection {
                    Protection::Newer => {
                        "It was saved by a newer Neptune. Its chat is shown as far as this \
                         version reads it; nothing of it is changed and its lead stays off. \
                         Update Neptune to go on with it."
                    }
                    Protection::Chat => {
                        "Its chat was saved by a newer Neptune, so it is not shown here. \
                         Nothing of it is changed and its lead stays off. Update Neptune to go \
                         on with it."
                    }
                    Protection::Unreadable => {
                        "Neptune could not read what this project saved, so nothing of it is \
                         changed and its lead stays off. Reveal the project folder to look at \
                         its files."
                    }
                }
                .into(),
                remedy: Remedy::None,
            });
        }
        if run.unsaved {
            needs.push(Need {
                title: "Not everything is saved".into(),
                detail: "Neptune could not write to this project's folder. What is said here \
                         from now on may be gone once Neptune closes. Check that the disk has \
                         room."
                    .into(),
                remedy: Remedy::None,
            });
        }
        if let Some(failure) = &run.trouble {
            needs.push(Need {
                title: "The lead needs you".into(),
                detail: trouble_text(failure, project.lead(), now, run.limited),
                remedy: Remedy::Retry,
            });
        }
        if let Some(guard) = run.guard {
            needs.push(Need {
                title: match guard {
                    Guard::Streak => "Paused after 20 turns without you".into(),
                    Guard::Hourly => "Paused after 30 turns in an hour".into(),
                },
                detail: "Neptune paused this project so that it does not run on by itself. \
                         Resume it once you have looked."
                    .into(),
                remedy: Remedy::Resume,
            });
        }
        Panel {
            body: PanelBody::Project(Box::new(Shown {
                id: project.id(),
                name: project.name().to_owned(),
                directory: ui::helpers::path_label(
                    if run.named.1.as_os_str().is_empty() {
                        workspace.cwd()
                    } else {
                        &run.named.1
                    },
                    40,
                ),
                lead: project.lead(),
                paused: project.paused(),
                needs,
                open: rows.iter().map(|row| (row.pane, row.generation)).collect(),
                // What a row of the chat offers for an agent's pull request:
                // only one at rest takes a message.
                follow: rows
                    .iter()
                    .filter(|row| row.doing == Doing::Idle)
                    .map(|row| {
                        let needs = |ask| needed(&row.pull_requests, ask).is_some();
                        (row.pane, needs(FollowUp::Checks), needs(FollowUp::Comments))
                    })
                    .filter(|(_, checks, comments)| *checks || *comments)
                    .collect(),
                // The same on the row of a watched pull request, where it
                // is the one its agent would be told about.
                pulls: run
                    .subscriptions
                    .iter()
                    .filter_map(|watch| {
                        let Trigger::PullRequest { url, .. } = &watch.trigger else {
                            return None;
                        };
                        rows.iter()
                            .filter(|row| row.doing == Doing::Idle)
                            .find_map(|row| {
                                let needs = |ask| {
                                    needed(&row.pull_requests, ask).is_some_and(|(link, _)| {
                                        link.url().eq_ignore_ascii_case(url)
                                    })
                                };
                                let (checks, comments) =
                                    (needs(FollowUp::Checks), needs(FollowUp::Comments));
                                (checks || comments)
                                    .then_some((watch.id, row.pane, checks, comments))
                            })
                    })
                    .collect(),
                // Nothing said, nothing being written and nothing asked of
                // Neptune: the wait is for the lead's first word.
                thinking: run.turn.as_ref().is_some_and(|turn| {
                    turn.block == 0
                        && turn.tools.is_empty()
                        && !(run.stream.block > turn.block && !run.stream.text.is_empty())
                }) && run.status.is_none(),
                elapsed: run
                    .turn
                    .as_ref()
                    .map(|turn| now.saturating_duration_since(turn.since)),
                usage: {
                    let cost: f64 = run
                        .chat
                        .records()
                        .iter()
                        .filter_map(|record| match &record.entry {
                            Entry::End { cost, .. } => *cost,
                            _ => None,
                        })
                        .sum();
                    (cost > 0.0).then(|| format!("${cost:.2}"))
                },
                members: rows
                    .into_iter()
                    .map(|row| Member {
                        pane: row.pane,
                        generation: row.generation,
                        kind: row.kind,
                        title: row.title,
                        state: state_word(row.doing, row.ready).to_owned(),
                        detail: doing_words(row.doing).to_owned(),
                        waits: matches!(row.doing, Doing::NeedsPerson(_) | Doing::Asking),
                        works: matches!(row.doing, Doing::Working | Doing::Starting),
                        ready: row.doing == Doing::Idle && row.ready,
                        elapsed: row.elapsed,
                        branch: row.branch,
                        pull_requests: row.pull_requests,
                        can_background: model.can_background(row.pane).is_ok(),
                    })
                    .collect(),
                pending: run
                    .turn
                    .iter()
                    .flat_map(|turn| &turn.tools)
                    .map(|pending| pending.line.clone())
                    .collect(),
                files: {
                    let at = unix_now();
                    let mut files: Vec<&context::FileInfo> = run.context.files.iter().collect();
                    // What nobody has touched for longest is what to look
                    // at first when the folder is tidied.
                    files.sort_by_key(|file| file.modified);
                    files
                        .into_iter()
                        .map(|file| File {
                            name: file.name.clone(),
                            author: file.author.clone(),
                            age: Duration::from_secs(at.saturating_sub(file.modified)),
                            size: context::size(file.size),
                            // Neptune writes the index again at once.
                            removable: file.name != context::INDEX,
                        })
                        .collect()
                },
                watches: {
                    let at = unix_now();
                    run.subscriptions
                        .iter()
                        .map(|watch| ui::project::Watch {
                            id: watch.id,
                            title: watch.title.clone(),
                            trigger: watch.trigger_words(),
                            detail: watch.detail(at),
                            paused: watch.paused,
                            proposed: !watch.allowed,
                            instruction: if watch.allowed {
                                String::new()
                            } else {
                                watch.instruction.clone()
                            },
                        })
                        .collect()
                },
                proposed: run
                    .subscriptions
                    .iter()
                    .filter(|watch| !watch.allowed)
                    .map(|watch| watch.id)
                    .collect(),
            })),
        }
    }

    /// A watch of the project in view has a time to say: when it runs next
    /// or how long ago it last ran.
    fn watches_age(&self) -> bool {
        self.active_project()
            .and_then(|project| self.projects.runs.get(&project.id()))
            .is_some_and(|run| {
                run.subscriptions.iter().any(|watch| {
                    watch.allowed
                        && (watch.last.is_some() || (watch.next.is_some() && !watch.paused))
                })
            })
    }
    /// Those times are said in minutes, as an agent's age is: the list is
    /// drawn again as they pass, and only while it is the one in view.
    pub(super) fn tick_watches(&self, ctx: &egui::Context) {
        if self.watches_age() {
            Self::tick_agents(ctx);
        }
    }

    /// Reads the context folder of the project in view again, once when
    /// its list comes into view and every two seconds while it stays: what
    /// is changed there by hand shows without a watcher. Nothing is read
    /// for a list nobody looks at.
    pub(super) fn sync_context(&mut self, ctx: &egui::Context) {
        let Some(id) = self.active_project().map(Project::id) else {
            return;
        };
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        let now = Instant::now();
        let since = run.read.map(|at| now.saturating_duration_since(at));
        match since {
            Some(since) if since < LOOK_EVERY => ctx.request_repaint_after(LOOK_EVERY - since),
            // Asked for already: its answer wakes the window.
            _ if run.looking.is_some() || run.stale => {}
            _ => {
                run.stale = true;
                ctx.request_repaint();
            }
        }
    }

    /// The tab is no longer the one in view: a field that is leaving must
    /// not keep the keyboard. What was typed in it stays.
    pub(super) fn leave_project(&mut self, ctx: &egui::Context) {
        self.ui.project.focus = false;
        self.ui.project.show_agents = false;
        ctx.memory_mut(|memory| {
            for id in [ui::chat::composer_id(), ui::chat::goal_id()]
                .into_iter()
                .chain(ui::project::field_ids())
            {
                if memory.has_focus(id) {
                    memory.surrender_focus(id);
                }
            }
        });
    }

    /// Escape in a field of the tab returns the keyboard to the terminal.
    /// Returns whether the key was the tab's.
    pub(super) fn project_escape(&mut self, ctx: &egui::Context) -> bool {
        // The toolkit has already taken focus from the field for this key.
        let held =
            |id| ctx.memory(|memory| memory.has_focus(id) || memory.had_focus_last_frame(id));
        match [ui::chat::composer_id(), ui::chat::goal_id()]
            .into_iter()
            .chain(ui::project::field_ids())
            .find(|id| held(*id))
        {
            Some(id) => {
                self.ui.project.focus = false;
                ctx.memory_mut(|memory| memory.surrender_focus(id));
                true
            }
            None => false,
        }
    }

    pub(super) fn project_event(&mut self, ctx: &egui::Context, event: Event) {
        enum Kind {
            Reveal,
            Other,
        }
        let event_kind = if matches!(event, Event::RevealFile(..)) {
            Kind::Reveal
        } else {
            Kind::Other
        };
        match event {
            Event::Compose => {
                self.panel_event(ctx, ui::panel::Event::Show(ui::panel::Tab::Project));
                self.ui.project.segment = ui::project::Segment::Chat;
                self.ui.project.focus = true;
            }
            Event::Show(segment) => {
                self.panel_event(ctx, ui::panel::Event::Show(ui::panel::Tab::Project));
                self.ui.project.show(segment);
            }
            Event::ShowAgents => {
                self.panel_event(ctx, ui::panel::Event::Show(ui::panel::Tab::Project));
                self.ui.project.show(ui::project::Segment::Chat);
                self.ui.project.show_agents = true;
            }
            Event::WriteWatch => {
                self.panel_event(ctx, ui::panel::Event::Show(ui::panel::Tab::Project));
                self.ui.project.show(ui::project::Segment::Watches);
                self.ui.project.watch.open = true;
                self.ui.project.watch.focus = true;
            }
            Event::Create {
                workspace,
                goal,
                lead,
                model,
                effort,
            } => match chosen_lead(lead, model, effort) {
                Ok(settings) => self.create_project(ctx, workspace, goal, lead, settings),
                Err(error) => self.ui.error = Some(error),
            },
            Event::SetLead {
                project,
                model,
                effort,
            } => {
                let Some(kind) = self.controller.model().project(project).map(Project::lead) else {
                    return;
                };
                let chosen = match chosen_lead(kind, model, effort) {
                    Ok(chosen) => chosen,
                    Err(error) => {
                        self.ui.error = Some(error);
                        return;
                    }
                };
                let Some(run) = self.writable(project) else {
                    self.ui.error = Some(SETTINGS_LOCKED.into());
                    return;
                };
                // A lead that runs is not touched here: it is let go at
                // rest, before its next turn, and resumed with these.
                if run.settings.lead != chosen {
                    run.settings.lead = chosen;
                    run.dirty = true;
                }
                ctx.request_repaint();
            }
            Event::SetAgentDefaults {
                project,
                kind,
                model,
                effort,
                ultracode,
            } => {
                let chosen = match chosen_agents(kind, model, effort, ultracode) {
                    Ok(chosen) => chosen,
                    Err(error) => {
                        self.ui.error = Some(error);
                        return;
                    }
                };
                let Some(run) = self.writable(project) else {
                    self.ui.error = Some(SETTINGS_LOCKED.into());
                    return;
                };
                let kept = match kind {
                    AgentKind::Claude => &mut run.settings.claude,
                    _ => &mut run.settings.codex,
                };
                if *kept != chosen {
                    *kept = chosen;
                    run.dirty = true;
                }
                ctx.request_repaint();
            }
            Event::RevealDirectory(project) => {
                let Some(directory) = self.project_home(project) else {
                    return;
                };
                if let Err(error) = self.file_opener.start(
                    crate::platform::files::Handoff::Reveal(directory),
                    ctx.clone(),
                ) {
                    self.ui.error = Some(error.into());
                }
            }
            Event::Send {
                project,
                text,
                attachments,
            } => {
                // A project the model holds is spoken to even before its
                // first poll.
                let Some(key) = self
                    .controller
                    .model()
                    .project(project)
                    .map(|item| item.key().clone())
                else {
                    return;
                };
                let run = self
                    .projects
                    .runs
                    .entry(project)
                    .or_insert_with(|| Run::new(key, Instant::now()));
                // Files alone are a message.
                if text.trim().is_empty() && attachments.is_empty() {
                    return;
                }
                // Nothing typed or attached is lost to a message that is
                // not taken: both are back in the field.
                let refused = if run.protected.is_some() {
                    Some("This project is read-only: its lead stays off.")
                } else if run.out.len() >= MAX_UNWRITTEN {
                    // Nothing is said that could not be saved in its turn.
                    Some("Neptune is still saving this chat. Send this in a moment.")
                } else if text.len() > MAX_MESSAGE {
                    Some("That message is too long for the lead. Shorten it.")
                } else if attachments.len() > MAX_ATTACHMENTS {
                    Some("That is more files than one message takes.")
                } else if !run.inbox.push_user(Message {
                    text: text.clone(),
                    attachments: attachments.clone(),
                }) {
                    Some("The lead has too many messages waiting. Send this one later.")
                } else {
                    None
                };
                if let Some(refused) = refused {
                    self.ui.project.drafts.insert(project, text);
                    self.ui.project.attached.insert(project, attachments);
                    self.ui.error = Some(refused.into());
                    return;
                }
                run.say(Entry::User { text, attachments });
                run.stale = true;
                // Writing again is asking again, except against a limit,
                // which only time or "Try again" lifts.
                if !matches!(run.trouble, Some(Failure::Limit { .. })) {
                    run.trouble = None;
                }
                self.ui.project.shown = ui::chat::PAGE;
                ctx.request_repaint();
            }
            Event::Attach(project) => self.pick_attachments(ctx, project),
            Event::Stop(project) => {
                if let Some(lead) = self
                    .projects
                    .runs
                    .get(&project)
                    .and_then(|run| run.lead.as_ref())
                {
                    lead.interrupt();
                }
            }
            Event::SetPaused(project, paused) => {
                self.dispatch(ctx, Command::SetProjectPaused { project, paused });
                if let Some(run) = self.projects.runs.get_mut(&project)
                    && !paused
                {
                    run.guard = None;
                    run.inbox.reset_guard();
                }
                ctx.request_repaint();
            }
            Event::Retry(project) => {
                if let Some(run) = self.projects.runs.get_mut(&project) {
                    run.trouble = None;
                    run.limited = None;
                }
                ctx.request_repaint();
            }
            Event::Rename(project) => {
                if let Some(item) = self.controller.model().project(project) {
                    self.ui.rename_name = item.name().into();
                    self.ui.overlay = OverlayState::RenameProject(project);
                    self.ui.overlay_focus = true;
                }
            }
            Event::SetName(project, name) => {
                self.dispatch(ctx, Command::RenameProject { project, name })
            }
            Event::Remove(project) => {
                if self.controller.model().project(project).is_some() {
                    self.ui.overlay = OverlayState::RemoveProject(project);
                }
            }
            Event::ConfirmRemove(project) => {
                let key = self
                    .controller
                    .model()
                    .project(project)
                    .map(|item| item.key().clone());
                // Its agents get tabs and go on; the next poll lets its
                // lead go and has its folder deleted, off the frame.
                self.dispatch(ctx, Command::RemoveProject(project));
                if let Some(key) = key
                    && self.controller.model().project(project).is_none()
                {
                    self.projects.removing.insert(key);
                }
                self.ui.overlay = OverlayState::None;
                ctx.request_repaint();
            }
            Event::NewChat(project) => {
                if let Some(run) = self.projects.runs.get_mut(&project)
                    && run.loading == Loading::Done
                    && run.protected.is_none()
                    && !run.unbegun()
                {
                    run.new_chat();
                    self.sessions.agents().close_lead(project);
                    self.ui.project.shown = ui::chat::PAGE;
                    ctx.request_repaint();
                }
            }
            Event::Reveal(project) => {
                let Some(item) = self.controller.model().project(project) else {
                    return;
                };
                let folder = project_store::folder(self.projects.store.root(), item.key());
                if let Err(error) = self
                    .file_opener
                    .start(crate::platform::files::Handoff::Reveal(folder), ctx.clone())
                {
                    self.ui.error = Some(error.into());
                }
            }
            Event::RevealContext(project) => {
                let Some(item) = self.controller.model().project(project) else {
                    return;
                };
                let folder = project_store::folder(self.projects.store.root(), item.key())
                    .join(context::FOLDER);
                if let Err(error) = self
                    .file_opener
                    .start(crate::platform::files::Handoff::Reveal(folder), ctx.clone())
                {
                    self.ui.error = Some(error.into());
                }
            }
            Event::Earlier(project) => {
                // What is held here is shown first; then its folder is read.
                if let Some(run) = self.projects.runs.get_mut(&project)
                    && run.more
                    && !run.fetching
                    && self.ui.project.shown.saturating_sub(ui::chat::PAGE)
                        >= ui::chat::readable(run.chat.records())
                    && let Some(first) = run.chat.first()
                {
                    run.fetching = self.projects.store.earlier(&run.key, first);
                }
            }
            Event::OpenFile(project, name) | Event::RevealFile(project, name) => {
                let reveal = matches!(event_kind, Kind::Reveal);
                let Some(item) = self.controller.model().project(project) else {
                    return;
                };
                // Only a file of the folder, by the name the list gave it.
                let Ok(place) = Place::parse(&name) else {
                    return;
                };
                let folder = project_store::folder(self.projects.store.root(), item.key())
                    .join(context::FOLDER);
                let path = place.path(&folder);
                let handoff = if reveal {
                    crate::platform::files::Handoff::Reveal(path)
                } else {
                    crate::platform::files::Handoff::Open(path)
                };
                if let Err(error) = self.file_opener.start(handoff, ctx.clone()) {
                    self.ui.error = Some(error.into());
                }
            }
            Event::DeleteFile(project, name) => {
                let Ok(place) = Place::parse(&name) else {
                    return;
                };
                self.ask_context(project, ContextOp::Delete { place });
            }
            Event::SaveInstructions { project, text } => {
                if text.trim().len() >= context::MAX_INSTRUCTIONS {
                    self.ui.error = Some(format!(
                        "The instructions hold at most {} KB. Shorten them.",
                        context::MAX_INSTRUCTIONS / 1024
                    ));
                    return;
                }
                // The instructions are the person's to write, and nobody
                // else's: the same rule a tool's call is held to.
                match context::may_write(&Place::Instructions, &Writer::User, None) {
                    Ok(_) => self.ask_context(project, ContextOp::Instructions { content: text }),
                    Err(reason) => self.ui.error = Some(reason),
                }
            }
            Event::AddWatch {
                project,
                title,
                trigger,
                instruction,
            } => {
                let Some(run) = self.writable(project) else {
                    return;
                };
                let new = subscription::New {
                    title,
                    trigger: match trigger {
                        WatchAsk::Every(minutes) => Trigger::Interval { minutes },
                        WatchAsk::PullRequest(url) => Trigger::PullRequest { url, auto: false },
                    },
                    instruction,
                    // The person wrote it: it needs no one's leave.
                    allowed: true,
                };
                let added = subscription::add(
                    &mut run.subscriptions,
                    &mut run.counters.watches,
                    new,
                    SystemTime::now(),
                );
                match added {
                    Ok(_) => run.dirty = true,
                    Err(reason) => self.ui.error = Some(reason),
                }
                ctx.request_repaint();
            }
            Event::AllowWatch(project, id, allow) => {
                let Some(run) = self.writable(project) else {
                    return;
                };
                let Some(at) = run
                    .subscriptions
                    .iter()
                    .position(|watch| watch.id == id && !watch.allowed)
                else {
                    return;
                };
                let title = run.subscriptions[at].title.clone();
                if allow {
                    let watch = &mut run.subscriptions[at];
                    watch.allowed = true;
                    // Its first run is a whole beat from the yes.
                    watch.next = watch
                        .minutes()
                        .map(|minutes| schedule::first(minutes, SystemTime::now()));
                    let beat = watch.trigger_words().to_lowercase();
                    run.notice(format!("You allowed the watch “{title}”: it runs {beat}."));
                } else {
                    run.subscriptions.remove(at);
                    run.notice(format!("You declined the watch “{title}”."));
                }
                run.dirty = true;
                ctx.request_repaint();
            }
            Event::PauseWatch(project, id, paused) => {
                let Some(run) = self.writable(project) else {
                    return;
                };
                let Some(watch) = run.subscriptions.iter_mut().find(|watch| watch.id == id) else {
                    return;
                };
                watch.paused = paused;
                if !paused && let (Some(minutes), Some(next)) = (watch.minutes(), watch.next) {
                    // What passed while it was paused is left out.
                    watch.next = Some(schedule::after(minutes, next, SystemTime::now()));
                }
                run.late.remove(&id);
                run.dirty = true;
                ctx.request_repaint();
            }
            Event::RunWatch(project, id) => {
                let Some(run) = self.writable(project) else {
                    return;
                };
                let Some(watch) = run
                    .subscriptions
                    .iter()
                    .find(|watch| watch.id == id && watch.allowed)
                else {
                    return;
                };
                let at = unix_now();
                let why = Why::Asked(at);
                let (title, saved, text) =
                    (watch.title.clone(), watch.saved, watch.instruction.clone());
                let (row, told) = match &watch.trigger {
                    Trigger::Interval { .. } => (
                        Entry::Event {
                            source: Source::Subscription,
                            agent: Some(id),
                            what: format!("Watch “{title}” fired · run by you"),
                            text: text.clone(),
                        },
                        prompt::fired(&title, &why.words(), saved, &text),
                    ),
                    Trigger::PullRequest { url, .. } => {
                        let Some(seen) = watch.seen else {
                            self.ui.error = Some(
                                "That pull request has not been read yet. Try again in a moment."
                                    .into(),
                            );
                            return;
                        };
                        let owner = run.owner(url);
                        (
                            Entry::Event {
                                source: Source::Pr,
                                agent: Some(id),
                                what: format!("Watch “{title}” · run by you"),
                                text: subscription::sentence(&seen.words()),
                            },
                            prompt::pull_request(
                                url,
                                owner
                                    .as_ref()
                                    .map(|(agent, title)| (*agent, title.as_str())),
                                "the user asked you to look at it",
                                &seen.words(),
                                saved,
                                &text,
                            ),
                        )
                    }
                };
                if run.firing(id) || !run.fire(row, Some(id), told, Instant::now()) {
                    self.ui.error = Some("The lead still has that watch's last run.".into());
                    return;
                }
                // A run the person asks for is theirs: the watch's day is
                // not charged for it.
                if let Some(watch) = run.subscriptions.iter_mut().find(|watch| watch.id == id) {
                    watch.fired(at, at, false);
                }
                run.dirty = true;
                self.ui.project.shown = ui::chat::PAGE;
                ctx.request_repaint();
            }
            Event::DeleteWatch(project, id) => {
                let Some(run) = self.writable(project) else {
                    return;
                };
                let before = run.subscriptions.len();
                run.subscriptions.retain(|watch| watch.id != id);
                run.late.remove(&id);
                run.dirty |= run.subscriptions.len() != before;
                ctx.request_repaint();
            }
            Event::Copy(text) => crate::platform::clipboard::copy(ctx, text),
            Event::Changes(pane, generation) => {
                // Its terminal comes forward, and the tab follows the
                // directory of the terminal in front.
                self.action(ctx, ui::Action::OpenAgent(pane, generation));
                if self.controller.model().active_pane() == Some(pane) {
                    self.panel_event(ctx, ui::panel::Event::Show(ui::panel::Tab::Changes));
                }
            }
            Event::FollowUp {
                project,
                pane,
                generation,
                ask,
            } => {
                let model = self.controller.model();
                let Some(item) = model.pane(pane).filter(|item| {
                    item.generation() == generation && item.project() == Some(project)
                }) else {
                    return;
                };
                let links: Vec<ui::helpers::LinkedPullRequest> = item
                    .pull_requests()
                    .iter()
                    .map(|link| ui::helpers::LinkedPullRequest {
                        link: link.clone(),
                        lookup: self.pull_requests.lookup(link),
                    })
                    .collect();
                // As the pull request stands now, not when the row was drawn.
                let Some((link, unresolved)) = needed(&links, ask) else {
                    self.ui.error = Some("That pull request no longer needs it.".into());
                    return;
                };
                let (text, label) = (follow_up(ask, link, unresolved), link.label());
                if model.project(project).is_some_and(Project::paused) {
                    self.ui.error = Some("This project is paused. Resume it first.".into());
                    return;
                }
                if self.writable(project).is_none() {
                    return;
                }
                // The way its lead's messages go: typed for it on the next
                // frame, led by what the project decided since it was told.
                if self
                    .sessions
                    .agents()
                    .tell(project, pane.get(), text)
                    .is_err()
                {
                    self.ui.error = Some(format!(
                        "Agent {pane} cannot take a message right now. Open its terminal."
                    ));
                    return;
                }
                if self.projects.own.len() < MAX_DONE {
                    self.projects.own.push(pane);
                }
                if let Some(run) = self.projects.runs.get_mut(&project) {
                    run.unread.remove(&pane.get());
                    run.say(Entry::Tool {
                        name: "follow_up".into(),
                        summary: match ask {
                            FollowUp::Checks => {
                                format!(
                                    "You asked agent {pane} to fix the failing checks of {label}"
                                )
                            }
                            FollowUp::Comments => format!(
                                "You asked agent {pane} to address the review comments of {label}"
                            ),
                        },
                        ok: true,
                    });
                }
                ctx.request_repaint();
            }
            Event::Reopen(workspace) => {
                // The one offered: it worked where the terminal in front is.
                let Some(directory) = self
                    .controller
                    .model()
                    .workspace(workspace)
                    .map(|item| focused_directory(item).to_path_buf())
                else {
                    return;
                };
                let Some(at) = self
                    .projects
                    .kept
                    .iter()
                    .position(|kept| kept.directory == directory)
                else {
                    return;
                };
                let kept = self.projects.kept[at].clone();
                // The model takes it under the key its folder has; it is
                // read back like any project of an earlier run.
                match self.controller.dispatch(Command::AddProject {
                    workspace,
                    name: kept.name,
                    key: kept.key,
                    lead: kept.lead,
                }) {
                    Ok(effects) => {
                        self.projects.kept.remove(at);
                        self.execute(ctx, effects);
                        self.ui.project.shown = ui::chat::PAGE;
                        ctx.request_repaint();
                    }
                    Err(error) => {
                        self.ui.error = Some(format!("Could not reopen the project: {error}."));
                    }
                }
            }
        }
    }

    /// The project as it may be changed: read back, and not read-only.
    fn writable(&mut self, project: ProjectId) -> Option<&mut Run> {
        self.projects
            .runs
            .get_mut(&project)
            .filter(|run| run.loading == Loading::Done && run.protected.is_none())
    }

    /// One word to the person, through the desktop, about a project they
    /// are not looking at. It names the project and never what was said in
    /// it. A project's agents raise no alerts of their own.
    fn announce_project(&mut self, ctx: &egui::Context, id: ProjectId, body: &'static str) {
        let model = self.controller.model();
        let Some(project) = model.project(id) else {
            return;
        };
        let in_view = ctx.input(|input| input.focused)
            && self.ui.panel.open
            && self.ui.panel.tab == ui::panel::Tab::Project
            && model.active_workspace() == Some(project.workspace());
        // An alert belongs to a terminal: the one in front in its workspace.
        let Some(pane) = model
            .workspace(project.workspace())
            .map(|workspace| workspace.active())
        else {
            return;
        };
        if in_view {
            return;
        }
        #[cfg(test)]
        self.projects.banners.push((id, body));
        if self.config.desktop_notifications && !cfg!(test) {
            let title = format!("Project {}", project.name());
            self.desktop_notifier
                .show(pane, title, body.to_owned(), ctx);
        }
    }

    /// The store's clock says that watch `id` of the project named `key` was
    /// due at `due`; it is `at` now. Whether it fires is decided here: never
    /// before its time, once for each run, not while the project or the
    /// watch is paused, not past its day's count, and not on top of an
    /// earlier fire the lead has yet to finish.
    fn watch_fired(
        &mut self,
        ctx: &egui::Context,
        key: &ProjectKey,
        id: u64,
        due: u64,
        at: SystemTime,
        now: Instant,
    ) {
        let Some((project, run)) = self
            .projects
            .runs
            .iter_mut()
            .find(|(_, run)| run.key == *key)
        else {
            return;
        };
        let paused = self
            .controller
            .model()
            .project(*project)
            .is_some_and(Project::paused);
        // The clock has let go of it: it is set again whatever follows.
        run.armed.clear();
        ctx.request_repaint();
        let late = run.late.remove(&id);
        let firing = run.firing(id);
        let held = paused || run.protected.is_some();
        let seconds = schedule::seconds(at);
        let Some(watch) = run.subscriptions.iter_mut().find(|watch| watch.id == id) else {
            return;
        };
        let Some(minutes) = watch.minutes() else {
            return;
        };
        let expected = late.map(|late| late.0).or(watch.next);
        if expected != Some(due) || due > seconds {
            // Not the run it waits for, or not its time yet: nothing changes.
            if let Some(late) = late {
                run.late.insert(id, late);
            }
            return;
        }
        // A run that fired already does not fire again, but its time has
        // passed all the same: left where it is, the clock would say it
        // again at once, without end.
        let fired = watch.fired_for(due);
        // Its next run keeps its beat. After runs that were missed while
        // Neptune was closed it begins again from now.
        watch.next = Some(match late {
            Some(_) => schedule::first(minutes, at),
            None => schedule::after(minutes, due, at),
        });
        run.dirty = true;
        if fired || held || !watch.runs() || firing || watch.spent(seconds) {
            // Passed over, not put off: it is not made up for later.
            return;
        }
        let why = match late {
            Some((_, runs)) => Why::Late { at: due, runs },
            None => Why::Due(due),
        };
        let (title, saved, text) = (watch.title.clone(), watch.saved, watch.instruction.clone());
        let row = Entry::Event {
            source: Source::Subscription,
            agent: Some(id),
            what: format!("Watch “{title}” fired · {}", why.words()),
            text: text.clone(),
        };
        let told = prompt::fired(&title, &why.words(), saved, &text);
        if run.fire(row, Some(id), told, now)
            && let Some(watch) = run.subscriptions.iter_mut().find(|watch| watch.id == id)
        {
            watch.fired(due, seconds, true);
        }
    }

    /// A call of a project's lead about its watches, answered at once.
    pub(super) fn watch_call(
        &mut self,
        project: ProjectId,
        from: Starter,
        call: ProjectCall,
    ) -> Result<String, String> {
        if from != Starter::Project(project) {
            return Err("Only a project's lead manages its watches.".into());
        }
        let model = self.controller.model();
        let paused = model.project(project).is_some_and(Project::paused);
        let linked: Vec<PullRequest> = model
            .project_agents(project)
            .flat_map(|pane| pane.pull_requests().iter().cloned())
            .collect();
        let run = self
            .projects
            .runs
            .get_mut(&project)
            .filter(|run| run.loading == Loading::Done)
            .ok_or("Neptune is busy; try again in a moment.")?;
        let read_only = || "This project is read-only: nothing of it is changed.".to_owned();
        let now = SystemTime::now();
        match call {
            ProjectCall::AddSubscription {
                title,
                trigger,
                instruction,
            } => {
                if run.protected.is_some() {
                    return Err(read_only());
                }
                if paused {
                    return Err(
                        "This project is paused, so no watch is added for it. Tell the user; \
                         they resume it in the Project tab."
                            .into(),
                    );
                }
                let add = |run: &mut Run, title: String, trigger: Trigger, allowed: bool| {
                    let added = subscription::add(
                        &mut run.subscriptions,
                        &mut run.counters.watches,
                        subscription::New {
                            title,
                            trigger,
                            instruction: instruction.clone(),
                            allowed,
                        },
                        now,
                    );
                    run.dirty |= added.is_ok();
                    added
                };
                match trigger {
                    WatchTrigger::Schedule { every_minutes } => {
                        // The lead proposes; only the person allows.
                        let trigger = Trigger::Interval {
                            minutes: every_minutes,
                        };
                        let id = add(run, title, trigger, false)?;
                        let watch = run.subscriptions.iter().find(|watch| watch.id == id);
                        let (title, beat) = watch.map_or_else(Default::default, |watch| {
                            (watch.title.clone(), watch.trigger_words().to_lowercase())
                        });
                        run.settled("add_subscription");
                        run.say(Entry::Proposal {
                            watch: id,
                            what: format!("The lead proposes a watch: “{title}” · {beat}"),
                            text: instruction.trim().to_owned(),
                        });
                        Ok(format!(
                            "Watch {id} \"{title}\" is proposed — the user must allow it. It does \
                             nothing until they press Allow beside it in the chat. Tell them it \
                             waits for that; do not say that it runs."
                        ))
                    }
                    WatchTrigger::PullRequest(link) if link.eq_ignore_ascii_case("linked") => {
                        if linked.is_empty() {
                            return Ok("The project's agents have linked no pull request yet. \
                                       Neptune follows each one from the moment it is linked."
                                .into());
                        }
                        let mut added = Vec::new();
                        for link in &linked {
                            let trigger = Trigger::PullRequest {
                                url: link.url().to_owned(),
                                auto: true,
                            };
                            match add(run, format!("PR #{}", link.number()), trigger, true) {
                                Ok(id) => added.push(format!("watch {id} follows {}", link.url())),
                                Err(reason) if reason.contains("already follows") => {}
                                Err(reason) => return Err(reason),
                            }
                        }
                        if added.is_empty() {
                            return Ok("Every pull request the project's agents linked is \
                                       followed already."
                                .into());
                        }
                        run.card(
                            "add_subscription",
                            match added.len() {
                                1 => "Added a pull request watch".to_owned(),
                                count => format!("Added {count} pull request watches"),
                            },
                        );
                        Ok(format!("Added: {}.", added.join("; ")))
                    }
                    WatchTrigger::PullRequest(url) => {
                        let trigger = Trigger::PullRequest { url, auto: false };
                        let id = add(run, title, trigger, true)?;
                        let label = run
                            .subscriptions
                            .iter()
                            .find(|watch| watch.id == id)
                            .map(Subscription::trigger_words)
                            .unwrap_or_default();
                        run.card("add_subscription", format!("Added a watch: {label}"));
                        Ok(format!(
                            "Watch {id} follows it. You are told when its checks fail or pass, \
                             when it is merged or closed, and when more review comments are open."
                        ))
                    }
                }
            }
            ProjectCall::ListSubscriptions => {
                Ok(subscription::listing(&run.subscriptions, unix_now()))
            }
            ProjectCall::RemoveSubscription { id } => {
                if run.protected.is_some() {
                    return Err(read_only());
                }
                let Some(at) = run.subscriptions.iter().position(|watch| watch.id == id) else {
                    return Err(format!(
                        "The project has no watch {id}. list_subscriptions shows them."
                    ));
                };
                let removed = run.subscriptions.remove(at);
                run.late.remove(&id);
                run.dirty = true;
                run.card(
                    "remove_subscription",
                    format!("Removed the watch “{}”", removed.title),
                );
                Ok(format!("Removed watch {id}."))
            }
            ProjectCall::PullRequestStatus { url } => {
                let links = match url {
                    Some(url) => {
                        let link = PullRequest::parse(&url)
                            .ok_or("url is the address of a pull request.")?;
                        // One nothing else follows is read for a while, so
                        // that asking again has an answer.
                        let followed = linked
                            .iter()
                            .cloned()
                            .chain(run.subscriptions.iter().filter_map(Subscription::link))
                            .any(|known| known.same(&link));
                        let asked = &mut self.projects.asked;
                        if !followed && !asked.iter().any(|(known, _)| known.same(&link)) {
                            if asked.len() >= MAX_ASKED {
                                asked.remove(0);
                            }
                            asked.push((link.clone(), Instant::now()));
                        }
                        vec![link]
                    }
                    None => {
                        let mut links: Vec<PullRequest> = Vec::new();
                        for link in linked
                            .into_iter()
                            .chain(run.subscriptions.iter().filter_map(Subscription::link))
                        {
                            if !links.iter().any(|known| known.same(&link)) {
                                links.push(link);
                            }
                        }
                        links
                    }
                };
                if links.is_empty() {
                    return Ok("The project follows no pull request yet.".into());
                }
                let watcher = &self.pull_requests;
                Ok(links
                    .iter()
                    .map(|link| {
                        let stands = match watcher.lookup(link) {
                            Lookup::Known(status) => seen_of(status).words(),
                            Lookup::Checking => "checking".into(),
                            Lookup::Unavailable => "unavailable".into(),
                        };
                        format!("{}: {stands}", link.url())
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            _ => Err("Neptune is busy; try again in a moment.".into()),
        }
    }

    /// Hands the store's worker a change the person made to a project's
    /// context in the tab. They are told if it could not be made.
    fn ask_context(&mut self, project: ProjectId, op: ContextOp) {
        let Some(key) = self.projects.runs.get(&project).map(|run| run.key.clone()) else {
            return;
        };
        if self.projects.ask(&key, op, Waiting::Person).is_err() {
            self.ui.error =
                Some("Neptune is still busy with this project. Try again in a moment.".into());
        }
    }

    /// A call of a project's lead or of one of its agents about what the
    /// project keeps. Who may read and write what is decided here; the
    /// store's worker carries it out and the caller is answered then.
    pub(super) fn context_call(
        &mut self,
        request: u64,
        project: ProjectId,
        from: Starter,
        call: ProjectCall,
    ) {
        if let Err(reason) = self.asked_of_context(request, project, from, call) {
            self.sessions.agents().call_result(request, Err(reason));
        }
    }
    fn asked_of_context(
        &mut self,
        request: u64,
        project: ProjectId,
        from: Starter,
        call: ProjectCall,
    ) -> Result<(), String> {
        const BUSY: &str = "Neptune is busy; try again in a moment.";
        let lead = from == Starter::Project(project);
        let member = match from {
            Starter::Pane(pane) => self
                .controller
                .model()
                .pane(pane)
                .is_some_and(|item| item.project() == Some(project))
                .then_some(pane.get()),
            Starter::Project(_) => None,
        };
        let run = self.projects.runs.get_mut(&project).ok_or(BUSY)?;
        let writer = match member {
            _ if lead => Writer::Lead,
            Some(agent) => Writer::Member {
                agent,
                title: run
                    .tasks
                    .get(agent)
                    .map(|task| task.title.clone())
                    .unwrap_or_default(),
            },
            None => return Err("This terminal's agent does not belong to this project.".into()),
        };
        let sized = |what: &str, text: &str, limit: usize| {
            if text.trim().is_empty() {
                Err(format!("{what} is empty."))
            } else if text.len() > limit {
                Err(format!("{what} is longer than {} KB.", limit / 1024))
            } else {
                Ok(())
            }
        };
        let card = |summary: String| lead.then_some(summary);
        let (op, card) = match call {
            ProjectCall::ReadContext { path, offset } => {
                let place = match path.as_deref().map(str::trim) {
                    None | Some("") => Place::Index,
                    Some(path) => Place::parse(path)?,
                };
                (ContextOp::Read { place, offset }, None)
            }
            ProjectCall::WriteContext {
                path,
                content,
                append,
            } => {
                let place = Place::parse(&path)?;
                let how = context::may_write(&place, &writer, append)?;
                sized("content", &content, context::MAX_WRITE)?;
                match place {
                    Place::Status => {
                        let append = how == context::Write::Append;
                        let done = if append { "Added to" } else { "Updated" };
                        (
                            ContextOp::Status { content, append },
                            card(format!("{done} {}", context::STATUS))
                                .map(|card| ("write_context", card)),
                        )
                    }
                    Place::Note(topic) => {
                        sized("content", &content, context::MAX_NOTE)?;
                        let entry = context::note(run.stamp(), &writer, &content);
                        let name = Place::Note(topic.clone()).name();
                        (
                            ContextOp::Note { topic, entry },
                            card(format!("Added a note to {name}"))
                                .map(|card| ("write_context", card)),
                        )
                    }
                    // Nothing else is anyone's to write through a tool.
                    other => return Err(format!("{} is not written this way.", other.name())),
                }
            }
            ProjectCall::RecordDecision { decision, why } => {
                if !lead {
                    return Err("Only the project's lead records decisions.".into());
                }
                sized("decision", &decision, context::MAX_DECISION)?;
                let why = why.filter(|why| !why.trim().is_empty());
                if let Some(why) = &why {
                    sized("why", why, context::MAX_DECISION)?;
                }
                let entry = context::decision(run.stamp(), &writer, &decision, why.as_deref());
                let gist = decision.trim().lines().next().unwrap_or_default();
                let more = if gist.len() > MAX_CARD { "…" } else { "" };
                (
                    ContextOp::Record { entry },
                    Some((
                        "record_decision",
                        format!("Recorded a decision: {}{more}", clip(gist, MAX_CARD)),
                    )),
                )
            }
            ProjectCall::AddNote { topic, content } => {
                let topic = context::slug(&topic)?;
                context::may_write(&Place::Note(topic.clone()), &writer, None)?;
                sized("content", &content, context::MAX_NOTE)?;
                let entry = context::note(run.stamp(), &writer, &content);
                (ContextOp::Note { topic, entry }, None)
            }
            // Answered elsewhere: nothing of these is in the folder.
            ProjectCall::Report { .. }
            | ProjectCall::AddSubscription { .. }
            | ProjectCall::ListSubscriptions
            | ProjectCall::RemoveSubscription { .. }
            | ProjectCall::PullRequestStatus { .. } => return Err(BUSY.into()),
        };
        let key = run.key.clone();
        self.projects
            .ask(&key, op, Waiting::Call { request, card })
            .map_err(|_| BUSY.to_owned())
    }

    /// Begins a project: its folder is made off the frame, and the model
    /// takes the project once the folder exists.
    fn create_project(
        &mut self,
        ctx: &egui::Context,
        workspace: WorkspaceId,
        goal: String,
        lead: AgentKind,
        settings: LeadSettings,
    ) {
        if self.projects.creating.is_some() {
            return;
        }
        if self.ephemeral {
            self.ui.error = Some("Projects are unavailable during a screenshot capture.".into());
            return;
        }
        if goal.len() > MAX_MESSAGE {
            // It is the first message to the lead, and stays in its field.
            self.ui.error = Some("That goal is too long for the lead. Shorten it.".into());
            return;
        }
        let model = self.controller.model();
        let Some(item) = model.workspace(workspace) else {
            return;
        };
        if item.remote().is_some() {
            self.ui.error = Some("Projects run on this computer.".into());
            return;
        }
        if model.project_of(workspace).is_some() {
            return;
        }
        if model.projects().len() >= Project::MAX {
            self.ui.error = Some(format!(
                "This window already has {} projects. Remove one to start another.",
                Project::MAX
            ));
            return;
        }
        let name: String = item.name().trim().chars().take(Project::MAX_NAME).collect();
        // Where it works is settled now and stays: the terminal in front
        // may be somewhere else by the time its folder is made.
        let directory = focused_directory(item).to_path_buf();
        self.wake_for_projects(ctx);
        if self.projects.store.create() {
            self.projects.creating = Some(Creating {
                workspace,
                name: if name.is_empty() {
                    "Project".into()
                } else {
                    name
                },
                goal,
                lead,
                directory,
                settings,
            });
        } else {
            self.ui.error = Some("Could not start a project. Try again in a moment.".into());
        }
    }

    /// The store's worker wakes the window when it has something ready.
    fn wake_for_projects(&self, ctx: &egui::Context) {
        self.projects.store.woken_by(|| {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint())
        });
    }

    /// The folder of the project being made exists, or could not be made.
    fn created(&mut self, ctx: &egui::Context, result: std::io::Result<ProjectKey>) {
        let Some(creating) = self.projects.creating.take() else {
            return;
        };
        let key = match result {
            Ok(key) => key,
            Err(error) => {
                self.diagnostics.failure(
                    "project_create",
                    None,
                    None,
                    &format!("{:?}", error.kind()),
                );
                self.ui.error = Some(format!(
                    "Could not make the project's folder under Neptune's data: {}.",
                    error.kind()
                ));
                return;
            }
        };
        let added = self.controller.dispatch(Command::AddProject {
            workspace: creating.workspace,
            name: creating.name.clone(),
            key: key.clone(),
            lead: creating.lead,
        });
        match added {
            Ok(effects) => self.execute(ctx, effects),
            Err(error) => {
                // The workspace closed or changed meanwhile. The folder
                // that was made for nothing goes, off the frame.
                self.projects.removals.push(key);
                self.ui.error = Some(format!("Could not start the project: {error}."));
                return;
            }
        }
        let Some(project) = self
            .controller
            .model()
            .project_of(creating.workspace)
            .map(Project::id)
        else {
            return;
        };
        let mut run = Run::new(key, Instant::now());
        // New: there is nothing to read back, and its state to write.
        run.loading = Loading::Done;
        run.dirty = true;
        run.named = (creating.name.clone(), creating.directory, creating.lead);
        run.settings.lead = creating.settings;
        run.inbox.push_user(creating.goal.clone().into());
        run.say(Entry::User {
            text: creating.goal,
            attachments: Vec::new(),
        });
        self.projects.runs.insert(project, run);
        self.ui.project.goal.clear();
        self.ui.project.lead_settings = LeadSettings::default();
        self.ui.project.lead_options = false;
        self.ui.project.custom = None;
        self.ui.project.shown = ui::chat::PAGE;
        self.ui.project.focus = self.ui.panel.open && self.ui.panel.tab == ui::panel::Tab::Project;
        ctx.request_repaint();
    }

    /// Runs once a frame, after agents' requests were served. It takes what
    /// leads and the bridge have ready and never waits for either.
    pub(super) fn poll_projects(&mut self, ctx: &egui::Context) {
        self.poll_projects_at(ctx, Instant::now());
    }

    fn poll_projects_at(&mut self, ctx: &egui::Context, now: Instant) {
        self.wake_for_projects(ctx);
        self.projects.installed.poll(now);
        // The model says which projects exist; a lead does not outlive its
        // project, and a key names one folder only.
        let live: Vec<(ProjectId, ProjectKey)> = self
            .controller
            .model()
            .projects()
            .iter()
            .map(|project| (project.id(), project.key().clone()))
            .collect();
        let bridge = self.sessions.agents();
        {
            let Projects {
                runs,
                store,
                removing,
                removals,
                leaving,
                ..
            } = &mut self.projects;
            runs.retain(|id, run| {
                let stays = live
                    .iter()
                    .any(|(project, key)| project == id && *key == run.key);
                if !stays {
                    run.lead = None;
                    bridge.close_lead(*id);
                    // Its watches are over, or rest until it is taken up.
                    store.schedule(&run.key, Vec::new());
                    if removing.remove(&run.key) {
                        // The person removed it: its folder goes.
                        removals.push(run.key.clone());
                    } else if run.loading == Loading::Done && run.protected.is_none() {
                        // Its workspace was closed: its folder stays, with
                        // everything said up to now.
                        if run.dirty {
                            store.save(&run.key, run.saved());
                        }
                        leaving.push((run.key.clone(), std::mem::take(&mut run.out)));
                    }
                }
                stays
            });
            // One that was removed before it was ever polled.
            removals.extend(std::mem::take(removing));
            removals.retain(|key| !store.remove(key));
            leaving.retain_mut(|(key, out)| {
                store.write(key, out);
                !out.is_empty()
            });
        }
        for (id, key) in &live {
            self.projects
                .runs
                .entry(*id)
                .or_insert_with(|| Run::new(key.clone(), now));
        }
        // What was done for a project that has gone since has no chat to
        // be written in.
        self.projects
            .done
            .retain(|(project, _)| live.iter().any(|(id, _)| id == project));
        let runs = &self.projects.runs;
        self.ui.project.drafts.retain(|id, _| runs.contains_key(id));
        self.ui
            .project
            .attached
            .retain(|id, _| runs.contains_key(id));
        self.ui
            .project
            .instructions
            .retain(|id, _| runs.contains_key(id));
        // Folders without a workspace are listed again whenever the
        // projects that have one change. A capture run reads none.
        let keys = || live.iter().map(|(_, key)| key);
        self.projects
            .kept
            .retain(|kept| keys().all(|key| *key != kept.key));
        let listed = self
            .projects
            .kept_for
            .as_ref()
            .is_some_and(|known| known.iter().eq(keys()));
        if !self.ephemeral && !listed {
            let open: Vec<ProjectKey> = keys().cloned().collect();
            if self.projects.store.kept(open.clone()) {
                self.projects.kept_for = Some(open);
            }
        }
        let asked = &mut self.projects.asked;
        asked.retain(|(_, since)| now.saturating_duration_since(*since) < ASKED_FOR);
        if let Some(until) = asked.iter().map(|(_, since)| *since + ASKED_FOR).min() {
            ctx.request_repaint_after(until.saturating_duration_since(now));
        }
        // Every project is read back as Neptune opens: one that is not
        // looked at may still have watches that are due.
        let model = self.controller.model();
        for (id, _) in &live {
            let Some(project) = model.project(*id) else {
                continue;
            };
            let Some(run) = self.projects.runs.get_mut(id) else {
                continue;
            };
            // Where it works is its own and never follows its workspace.
            run.named.2 = project.lead();
            if run.named.0 != project.name() {
                run.named.0 = project.name().to_owned();
                run.dirty = true;
            }
            if run.loading == Loading::No {
                if self.projects.store.load(&run.key) {
                    run.loading = Loading::Asked;
                } else {
                    ctx.request_repaint_after(RETRY);
                }
            }
        }
        self.store_replies(ctx, now);
        if live.is_empty() {
            self.projects.done.clear();
            return;
        }
        for (project, note) in self.sessions.agents().drain_project_notes() {
            let Some(run) = self.projects.runs.get_mut(&project) else {
                continue;
            };
            match note {
                ProjectNote::Undelivered { agent } => {
                    let title = run
                        .reports
                        .iter()
                        .find(|report| report.agent == agent)
                        .and_then(|report| report.title.clone());
                    run.event(AgentEvent::new(agent, title, What::Undelivered), now);
                }
                ProjectNote::Paused => run.refused = run.refused.saturating_add(1),
            }
        }
        for (id, _) in live {
            self.poll_project(ctx, id, now);
        }
    }

    /// Takes what the store's worker has ready. Never waits for it.
    fn store_replies(&mut self, ctx: &egui::Context, now: Instant) {
        let active = self.active_project().map(Project::id);
        for reply in self.projects.store.poll() {
            fn of<'a>(
                runs: &'a mut BTreeMap<ProjectId, Run>,
                key: &ProjectKey,
            ) -> Option<(ProjectId, &'a mut Run)> {
                runs.iter_mut()
                    .find(|(_, run)| run.key == *key)
                    .map(|(id, run)| (*id, run))
            }
            match reply {
                Reply::Created(result) => self.created(ctx, result),
                Reply::Loaded { key, loaded } => {
                    if let Some((id, run)) = of(&mut self.projects.runs, &key)
                        && run.loading == Loading::Asked
                    {
                        let model = self.controller.model();
                        let fallback = model
                            .project(id)
                            .and_then(|project| model.workspace(project.workspace()))
                            .map(|workspace| workspace.cwd().to_path_buf())
                            .unwrap_or_default();
                        run.loaded(*loaded, now, &fallback);
                        run.make_up(
                            self.projects.opened,
                            SystemTime::now(),
                            &mut self.projects.late,
                        );
                    }
                }
                Reply::Fired { key, id, due } => {
                    self.watch_fired(ctx, &key, id, due, SystemTime::now(), now);
                }
                Reply::Earlier { key, records, more } => {
                    if let Some((id, run)) = of(&mut self.projects.runs, &key) {
                        let before = run.chat.records().len();
                        let room = run.chat.prepend(records);
                        run.more = more && room;
                        run.fetching = false;
                        if Some(id) == active {
                            let added = run.chat.records().len() - before;
                            self.ui.project.shown += added.min(ui::chat::PAGE);
                        }
                    }
                }
                Reply::Unsaved(key) => {
                    if let Some((_, run)) = of(&mut self.projects.runs, &key)
                        && !std::mem::replace(&mut run.unsaved, true)
                    {
                        self.diagnostics
                            .failure("project_store", None, None, "Write");
                    }
                }
                Reply::NotRemoved(_) => {
                    self.diagnostics
                        .failure("project_store", None, None, "Remove");
                    self.ui.error =
                        Some("Could not delete the project's folder under Neptune's data.".into());
                }
                Reply::Context {
                    key,
                    ticket,
                    result,
                    digest,
                } => {
                    let waiting = self.projects.tickets.remove(&ticket);
                    let mut run = of(&mut self.projects.runs, &key).map(|(_, run)| run);
                    if let Some(run) = &mut run {
                        run.context(digest, now);
                    }
                    match waiting {
                        None => {}
                        Some(Waiting::Look) => {
                            if let Some(run) = run {
                                run.looking = None;
                            }
                        }
                        Some(Waiting::Call { request, card }) => {
                            // The chat says what was written, not what the
                            // lead says it wrote.
                            if let (Ok(_), Some((tool, summary)), Some(run)) = (&result, card, run)
                            {
                                run.card(tool, summary);
                            }
                            self.sessions.agents().call_result(request, result);
                        }
                        Some(Waiting::Brief(briefing)) => {
                            let Briefing {
                                request,
                                project,
                                generation,
                                task,
                                cwd,
                            } = *briefing;
                            self.serve_spawn(
                                ctx,
                                request,
                                Starter::Project(project),
                                generation,
                                task,
                                cwd,
                            );
                        }
                        Some(Waiting::Person) => {
                            if let Err(reason) = result {
                                self.ui.error = Some(reason);
                            }
                        }
                    }
                    ctx.request_repaint();
                }
                Reply::Kept(kept) => {
                    let open = &self.projects.runs;
                    self.projects.kept = kept
                        .into_iter()
                        .filter(|kept| !open.values().any(|run| run.key == kept.key))
                        .collect();
                }
            }
        }
    }

    fn poll_project(&mut self, ctx: &egui::Context, id: ProjectId, now: Instant) {
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        if run.loading != Loading::Done {
            return;
        }
        // The store's clock is told when its schedules are next due. A run
        // that saves nothing has no lead to wake.
        let times = if self.ephemeral {
            Vec::new()
        } else {
            run.times()
        };
        if times != run.armed {
            self.projects.store.schedule(&run.key, times.clone());
            run.armed = times;
        }
        // What waits to be written goes first. While the worker is behind,
        // nothing more is taken from the lead or the agents: they wait with
        // what they have, and nothing is dropped.
        self.projects.store.write(&run.key, &mut run.out);
        if run.out.len() >= MAX_UNWRITTEN {
            ctx.request_repaint_after(RETRY);
            return;
        }
        let protected = run.protected.is_some();
        // What its agents share is read once it is known, and again after
        // anything its lead's next turn should be told with.
        if (run.read.is_none() || run.stale) && run.looking.is_none() {
            let key = run.key.clone();
            let asked = self
                .projects
                .ask(&key, ContextOp::Look, Waiting::Look)
                .is_ok();
            let Some(run) = self.projects.runs.get_mut(&id) else {
                return;
            };
            run.looking = asked.then_some(now);
            // One the worker had no room for is asked for again.
            run.stale = !asked;
            if !asked {
                ctx.request_repaint_after(RETRY);
            }
            // The worker may have it ready by now, and then nothing waits.
            self.store_replies(ctx, now);
        }
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        // What the lead said comes before what Neptune did for it, so a
        // card finds the line it replaces.
        let events = run
            .lead
            .as_ref()
            .map(|lead| lead.try_events())
            .unwrap_or_default();
        for event in events {
            self.lead_event(ctx, id, event, now);
        }
        let done = std::mem::take(&mut self.projects.done);
        let (mine, others): (Vec<_>, Vec<_>) =
            done.into_iter().partition(|(project, _)| *project == id);
        self.projects.done = others;
        // A project that writes nothing takes nothing: its agents' replies
        // stay where they are.
        let bridge = self.sessions.agents();
        let (reports, look_again) = bridge.collect_project(id, !protected);
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        for (_, done) in mine {
            run.did(done);
        }
        if let Some(lead) = &run.lead
            && lead.revision() != run.stream.revision
        {
            run.stream = lead.stream();
        }
        for report in &reports {
            if protected || !run.hears(report) {
                continue;
            }
            let what = what(report);
            // The reply a turn ended with is what the project keeps of it.
            if matches!(what, What::Finished | What::Ended(_))
                && let Some(reply) = report.replies.last()
            {
                run.dirty |= run.tasks.reported(report.agent, reply);
                // Something to look at, until its terminal is opened.
                if what == What::Finished {
                    run.unread.insert(report.agent);
                }
            }
            let mut event = AgentEvent::new(report.agent, report.title.clone(), what);
            event.replies = report.replies.clone();
            if let Status::Asking { screen } = &report.status {
                event.screen = Some(screen.clone());
            }
            run.event(event, now);
        }
        run.asked
            .retain(|agent| reports.iter().any(|report| report.agent == *agent));
        run.heard.retain(|agent, _| {
            reports.iter().any(|report| report.agent == *agent) || run.tasks.get(*agent).is_some()
        });
        let model = self.controller.model();
        // The terminal in front of the person is one they have opened.
        let front = model.active_pane().map(PaneId::get);
        run.unread.retain(|agent| {
            Some(*agent) != front && reports.iter().any(|report| report.agent == *agent)
        });
        run.reports = reports;
        let paused = model.project(id).is_some_and(Project::paused);
        if !protected {
            // What the project remembers of its agents follows the model
            // and the bridge, so that it is true after a restart as well.
            let at = unix_now();
            let mut open = Vec::new();
            // Pull requests its agents linked since they were last looked
            // at, and worktrees whose branch was merged since.
            let mut linked: Vec<PullRequest> = Vec::new();
            let mut merged = Vec::new();
            for pane in model.project_agents(id) {
                let n = pane.id().get();
                open.push(n);
                let report = run.reports.iter().find(|report| report.agent == n);
                if run.tasks.get(n).is_none() {
                    // One the folder does not know, such as after its
                    // state was lost.
                    run.tasks.started(
                        Kept {
                            n,
                            title: report
                                .and_then(|report| report.title.clone())
                                .unwrap_or_default(),
                            kind: pane.agent().map_or(AgentKind::Claude, |agent| agent.kind),
                            session: None,
                            cwd: pane.cwd().into(),
                            worktree: None,
                            pull_requests: Vec::new(),
                            state: TaskState::Starting,
                            last_report: None,
                            started: at,
                            ended: None,
                            merged: false,
                            model: None,
                            effort: None,
                            ultracode: false,
                        },
                        None,
                    );
                    run.dirty = true;
                }
                let Some(task) = run.tasks.get_mut(n) else {
                    continue;
                };
                let mut changed = false;
                if let Some(agent) = pane.agent() {
                    changed |= task.kind != agent.kind
                        || task.cwd != agent.cwd
                        || (agent.session_id.is_some() && task.session != agent.session_id);
                    if changed {
                        task.kind = agent.kind;
                        task.cwd.clone_from(&agent.cwd);
                        if agent.session_id.is_some() {
                            task.session.clone_from(&agent.session_id);
                        }
                    }
                }
                if task.worktree.as_ref() != pane.worktree() {
                    task.worktree = pane.worktree().cloned();
                    changed = true;
                }
                // Said once for each agent: its record remembers that it was.
                if !task.merged
                    && let Some(worktree) = pane.worktree()
                    && let Some(into) = self.worktrees.merged_into(&worktree.path)
                {
                    task.merged = true;
                    changed = true;
                    merged.push((
                        n,
                        task.title.clone(),
                        worktree.branch.clone(),
                        into.to_owned(),
                    ));
                }
                let links = pane.pull_requests();
                if !task
                    .pull_requests
                    .iter()
                    .map(String::as_str)
                    .eq(links.iter().map(|link| link.url()))
                {
                    linked.extend(
                        links
                            .iter()
                            .filter(|link| {
                                !task.pull_requests.iter().any(|known| known == link.url())
                            })
                            .cloned(),
                    );
                    task.pull_requests = links.iter().map(|link| link.url().to_owned()).collect();
                    changed = true;
                }
                if let Some(report) = report {
                    let state = match report.status {
                        Status::Starting | Status::Asking { .. } => TaskState::Starting,
                        Status::Working => TaskState::Working,
                        Status::Waiting { .. } => TaskState::Waiting,
                        Status::Idle => TaskState::Review,
                        Status::Ended { .. } => TaskState::Ended,
                    };
                    changed |= std::mem::replace(&mut task.state, state) != state;
                    match &report.title {
                        // A terminal restored from an earlier run comes
                        // back without the name its lead gave it.
                        None if !task.title.is_empty() => bridge.remember(
                            id,
                            n,
                            Some(&task.title),
                            task.last_report.as_deref(),
                            None,
                            (None, None, false),
                        ),
                        Some(title) if task.title.is_empty() => {
                            task.title.clone_from(title);
                            changed = true;
                        }
                        _ => {}
                    }
                }
                run.dirty |= changed;
            }
            run.dirty |= run.tasks.end_others(&open, at);
            // A pull request an agent of the project links is followed from
            // then on. The person pauses or deletes its watch like any
            // other; one that was deleted does not come back.
            for link in linked {
                let added = subscription::add(
                    &mut run.subscriptions,
                    &mut run.counters.watches,
                    subscription::New {
                        title: format!("PR #{}", link.number()),
                        trigger: Trigger::PullRequest {
                            url: link.url().to_owned(),
                            auto: true,
                        },
                        instruction: String::new(),
                        allowed: true,
                    },
                    SystemTime::now(),
                );
                run.dirty |= added.is_ok();
            }
            for (agent, title, branch, into) in merged {
                let named = if title.is_empty() {
                    format!("Agent {agent}")
                } else {
                    format!("“{title}”")
                };
                run.fire(
                    Entry::Event {
                        source: Source::Worktree,
                        agent: Some(agent),
                        what: format!("{named}: its branch {branch} was merged into {into}"),
                        text: String::new(),
                    },
                    None,
                    prompt::merged(agent, &title, &branch, &into),
                    now,
                );
            }
            let watcher = &self.pull_requests;
            follow(run, |link| watcher.lookup(link), paused, at, now);
            if !std::mem::replace(&mut run.reminded, true) {
                // Agents whose terminals were closed in an earlier run can
                // be opened again by the conversation each one had.
                for task in run
                    .tasks
                    .tasks()
                    .iter()
                    .rev()
                    .filter(|task| task.ended.is_some() && task.session.is_some())
                    .take(AgentSession::MAX_SPAWNED)
                {
                    bridge.remember(
                        id,
                        task.n,
                        (!task.title.is_empty()).then_some(task.title.as_str()),
                        task.last_report.as_deref(),
                        Some(&AgentSession {
                            kind: task.kind,
                            session_id: task.session.clone(),
                            cwd: task.cwd.clone(),
                        }),
                        // Opened again, it runs with what it had.
                        (
                            task.model.as_deref(),
                            task.effort.as_deref(),
                            task.ultracode,
                        ),
                    );
                }
            }
        }
        // An agent's state can change by the clock alone, with no wake.
        if let Some(at) = look_again {
            ctx.request_repaint_after(at.saturating_duration_since(now));
        }
        if let Some(until) = run.limited {
            if now >= until {
                run.limited = None;
                run.trouble = None;
            } else {
                ctx.request_repaint_after(until - now);
            }
        }
        match run.inbox.next(now, paused, run.limited.is_some()) {
            Next::Nothing => self.rest_lead(ctx, id, now),
            Next::Wait(at) => ctx.request_repaint_after(at.saturating_duration_since(now)),
            Next::Guard(guard) => {
                run.guard = Some(guard);
                run.notice(match guard {
                    Guard::Streak => {
                        "Neptune paused the project: its lead took 20 turns in a row without \
                         a word from you."
                    }
                    Guard::Hourly => {
                        "Neptune paused the project: its lead took 30 turns by itself within \
                         an hour."
                    }
                });
                self.dispatch(
                    ctx,
                    Command::SetProjectPaused {
                        project: id,
                        paused: true,
                    },
                );
            }
            Next::Ready(_) => self.feed_lead(ctx, id, now),
        }
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        // The person is told once when something of it begins to need them:
        // an agent that waits, or a lead that cannot go on.
        let needs = run.waiting().count()
            + usize::from(run.trouble.is_some())
            + usize::from(run.guard.is_some());
        if !std::mem::replace(&mut run.needed, needs > 0) && needs > 0 {
            self.announce_project(ctx, id, "Needs you");
        }
        // What changed is handed to the store; its worker writes it.
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        if protected {
            return;
        }
        if std::mem::take(&mut run.dirty) {
            self.projects.store.save(&run.key, run.saved());
        }
        self.projects.store.write(&run.key, &mut run.out);
    }

    /// Lets go of a lead that has rested long enough with no agent at work.
    fn rest_lead(&mut self, ctx: &egui::Context, id: ProjectId, now: Instant) {
        let agents = self.controller.model().project_agents(id).count();
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        let resting = run.resting()
            && run
                .lead
                .as_ref()
                .is_some_and(|lead| lead.state() == LeadState::Ready);
        if !resting || agents > 0 {
            return;
        }
        let rested = now.saturating_duration_since(run.idle);
        if rested >= IDLE {
            run.lead = None;
            self.sessions.agents().close_lead(id);
        } else {
            ctx.request_repaint_after(IDLE - rested);
        }
    }

    /// Hands the lead what waits for it, starting the lead first where it
    /// is not running.
    fn feed_lead(&mut self, ctx: &egui::Context, id: ProjectId, now: Instant) {
        let Some(run) = self.projects.runs.get(&id) else {
            return;
        };
        if run.held() {
            return;
        }
        let Some(state) = run.lead.as_ref().map(|lead| lead.state()) else {
            return self.start_lead(ctx, id, now);
        };
        // A turn says what the project keeps as its folder holds it now:
        // one that is being read is waited for, a moment at most.
        if let Some(since) = run.looking {
            let waited = now.saturating_duration_since(since);
            if waited < LOOK_WAIT {
                ctx.request_repaint_after(LOOK_WAIT - waited);
                return;
            }
        }
        if state != LeadState::Ready || !run.resting() {
            // Starting or in a turn: its next event wakes the application.
            return;
        }
        let Some(project) = self.controller.model().project(id) else {
            return;
        };
        // The person chose another model or effort since this lead was
        // started. It is at rest, so it is let go and started again in the
        // conversation it has, and takes this turn once it is ready.
        let wanted = self
            .projects
            .lead_settings(project.lead(), &run.settings.lead);
        if run.launched.as_ref().is_some_and(|with| *with != wanted) {
            if let Some(run) = self.projects.runs.get_mut(&id) {
                run.lead = None;
                run.launched = None;
            }
            self.sessions.agents().close_lead(id);
            return self.start_lead(ctx, id, now);
        }
        let paused = project.paused();
        let members: Vec<prompt::Member> = self
            .project_rows(id, now)
            .into_iter()
            .map(|row| prompt::Member {
                agent: row.pane.get(),
                title: row.title,
                doing: row.doing,
                elapsed: row.elapsed,
            })
            .collect();
        let name = project.name().to_owned();
        let directory = self
            .project_home(id)
            .map(|directory| directory.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.projects.turns += 1;
        let turn_id = format!("turn-{}", self.projects.turns);
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        let through = run.chat.last();
        let turn = run.inbox.take(&turn_id, paused);
        run.taken = (!turn.events.is_empty()).then_some(through);
        let kept = run.context_lines();
        let set = run.settings.lines();
        let text = prompt::envelope(
            &prompt::State {
                name: &name,
                directory: &directory,
                paused,
                members: &members,
                context: &kept,
                settings: &set,
            },
            &turn,
        );
        // The lead is shown the pictures among what was attached; its own
        // thread reads them.
        let pictures: Vec<PathBuf> = turn
            .users
            .iter()
            .flat_map(|message| &message.attachments)
            .filter(|file| file.picture())
            .map(|file| PathBuf::from(&file.path))
            .collect();
        let sent = run.lead.as_ref().map_or(Err(Refused::Dead), |lead| {
            lead.send_turn(&turn_id, &text, &pictures)
        });
        match sent {
            Ok(()) => {}
            Err(Refused::TooLong) => {
                // Handed over again it would be as long. Messages and what
                // a turn carries of events are bounded well below a turn,
                // so this is not reached; were it, the turn is given up
                // rather than tried on every frame.
                run.inbox.started(&turn_id, now);
                run.taken = None;
                run.offered = None;
                run.notice("A turn was too long for the lead and was not sent.");
            }
            // Not idle after all: everything waits for its next event.
            Err(_) => {
                run.inbox.abandoned();
                run.offered = None;
            }
        }
        ctx.request_repaint();
    }

    fn start_lead(&mut self, ctx: &egui::Context, id: ProjectId, now: Instant) {
        if self.ephemeral {
            // A lead runs in its project's folder, and a run that saves
            // nothing makes none.
            return;
        }
        let Some(project) = self.controller.model().project(id) else {
            return;
        };
        let kind = project.lead();
        let folder = project_store::folder(self.projects.store.root(), project.key());
        let running = self
            .projects
            .runs
            .values()
            .filter(|run| run.lead.is_some())
            .count();
        if running >= MAX_LEADS {
            // The lead that has rested longest gives way; its session is
            // resumed when its project next needs it.
            let model = self.controller.model();
            let rested = self
                .projects
                .runs
                .iter()
                .filter(|(other, run)| {
                    run.resting()
                        && run
                            .lead
                            .as_ref()
                            .is_some_and(|lead| lead.state() == LeadState::Ready)
                        && model.project_agents(**other).count() == 0
                })
                .min_by_key(|(_, run)| run.idle)
                .map(|(other, _)| *other);
            let Some(other) = rested else {
                if let Some(run) = self.projects.runs.get_mut(&id) {
                    run.status =
                        Some("Waiting for another project's lead: three run at a time.".to_owned());
                }
                ctx.request_repaint_after(Duration::from_secs(5));
                return;
            };
            if let Some(run) = self.projects.runs.get_mut(&other) {
                run.lead = None;
            }
            self.sessions.agents().close_lead(other);
        }
        let Some(exe) = self.projects.exe.clone() else {
            if let Some(run) = self.projects.runs.get_mut(&id) {
                run.trouble = Some(Failure::Transport);
            }
            return;
        };
        let bridge = self.sessions.bridge();
        let wake: Wake = {
            let ctx = ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };
        let (opened, woken) = (folder.clone(), wake.clone());
        let Some(with) = self
            .projects
            .runs
            .get(&id)
            .map(|run| self.projects.lead_settings(kind, &run.settings.lead))
        else {
            return;
        };
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        let launch = Launch {
            kind,
            exe: None,
            project_dir: folder,
            session: SessionRef {
                id: run.session.id.clone(),
                opened: run.session.opened,
            },
            system_prompt: prompt::LEAD_PROMPT.to_owned(),
            mcp: Mcp {
                exe,
                // On the lead's worker: it may bind the bridge's socket,
                // and it makes sure the lead has its folder to run in.
                open: Box::new(move || {
                    project_store::ensure(&opened)?;
                    bridge.open_lead(id, kind, woken)
                }),
            },
            model: with.model.clone(),
            effort: with.effort.clone(),
        };
        run.launched = Some(with);
        run.status = Some(format!("Starting {}…", lead_name(kind)));
        run.idle = now;
        run.lead = Some((self.projects.start)(launch, wake));
    }

    fn lead_event(&mut self, ctx: &egui::Context, id: ProjectId, event: LeadEvent, now: Instant) {
        let kind = self
            .controller
            .model()
            .project(id)
            .map_or(AgentKind::Claude, Project::lead);
        let Some(run) = self.projects.runs.get_mut(&id) else {
            return;
        };
        // Whether the person is told that the lead has something for them.
        let mut update = false;
        match event {
            LeadEvent::Session { id: session } => {
                if run.session.id.as_deref() != Some(&session) {
                    run.session = Session {
                        id: Some(session),
                        opened: false,
                    };
                    run.dirty = true;
                }
            }
            LeadEvent::Ready { model, .. } => {
                run.model = model;
                run.dirty |= !std::mem::replace(&mut run.session.opened, true);
                run.status = None;
                run.idle = now;
            }
            LeadEvent::Failed(failure) => {
                self.diagnostics
                    .failure("project_lead", None, None, failure.kind());
                run.status = None;
                if failure == Failure::SessionLost {
                    // Nothing to resume: the next start opens a new one,
                    // and what waited is sent there.
                    run.session = Session::default();
                    run.dirty = true;
                    // Its next conversation knows nothing of the project.
                    run.full = true;
                    run.sent = None;
                    run.notice(trouble_text(&failure, kind, now, None));
                } else {
                    let said = run
                        .lead
                        .as_ref()
                        .and_then(|lead| lead.last_error())
                        .filter(|_| matches!(failure, Failure::Transport | Failure::Protocol));
                    let mut text = trouble_text(&failure, kind, now, None);
                    if let Some(said) = said {
                        let tail = said.len().saturating_sub(MAX_SAID);
                        let tail = (tail..=said.len())
                            .find(|at| said.is_char_boundary(*at))
                            .unwrap_or(said.len());
                        text = format!("{text} It said: {}", said[tail..].trim());
                    }
                    run.notice(text);
                    run.trouble = Some(failure);
                }
            }
            LeadEvent::TurnStarted { turn } => {
                let Some(begun) = run.inbox.started(&turn, now) else {
                    return;
                };
                run.say(Entry::Turn {
                    id: turn.clone(),
                    origin: begun.origin,
                });
                run.status = None;
                // What the turn said of the project's context is what the
                // lead now knows of it.
                if let Some(offered) = run.offered.take() {
                    run.sent = Some(offered);
                    run.full = false;
                }
                run.turn = Some(Running {
                    id: turn,
                    block: 0,
                    tools: Vec::new(),
                    through: run.taken.take(),
                    origin: begun.origin,
                    fires: begun.fires.iter().filter_map(|fire| fire.watch).collect(),
                    said: String::new(),
                    since: now,
                });
            }
            LeadEvent::TextDone { block, text } => {
                // It got through: a retry is no longer what it is doing.
                run.status = None;
                if let Some(turn) = &mut run.turn {
                    turn.block = block;
                    let room = MAX_HEARD.saturating_sub(turn.said.len());
                    turn.said.push_str(clip(&text, room));
                }
                run.say(Entry::Lead { text });
            }
            LeadEvent::ToolStarted { call, tool, input } => {
                run.status = None;
                if let Some(turn) = &mut run.turn {
                    let line = pending_line(&tool, &input);
                    turn.tools.push(Pending { call, tool, line });
                }
            }
            LeadEvent::ToolDone { call, ok } => {
                // A call Neptune carried out has its card already.
                let asked = run.turn.as_mut().and_then(|turn| {
                    let at = turn.tools.iter().position(|pending| pending.call == call)?;
                    Some(turn.tools.remove(at))
                });
                if let Some(asked) = asked
                    && !ok
                {
                    // The bridge said why, where it refused for a reason
                    // the person can change.
                    let acts = matches!(
                        asked.tool.as_str(),
                        "spawn_agent" | "send_agent_message" | "reopen_agent"
                    );
                    let why = if acts && run.refused > 0 {
                        run.refused -= 1;
                        ": the project is paused"
                    } else {
                        ""
                    };
                    run.say(Entry::Tool {
                        summary: format!("The lead's {} request was refused{why}", asked.tool),
                        name: asked.tool,
                        ok: false,
                    });
                }
            }
            LeadEvent::Retry { attempt, max, .. } => {
                // Not every CLI says how often it will try.
                let name = lead_name(kind);
                run.status = Some(if max == 0 {
                    format!("{name} is busy. Retrying…")
                } else {
                    format!("{name} is busy. Retrying ({attempt} of {max})…")
                });
            }
            LeadEvent::Limit { resets_at } => {
                run.limited = limit_end(resets_at, now);
                run.trouble = Some(Failure::Limit { resets_at });
            }
            LeadEvent::Compacted => {
                // What it was handed of the project may be gone with the
                // rest: its next turn carries all of it again.
                run.full = true;
                run.notice(
                    "The lead summarised earlier conversation to make room. It is handed what \
                     the project keeps again.",
                );
            }
            LeadEvent::TurnEnded {
                turn,
                outcome,
                cost_usd,
            } => {
                let ended = run.turn.take_if(|running| running.id == turn);
                // A refusal is said of the turn that asked. It is forgotten
                // here and not as a turn starts: a quick lead is refused in
                // the frame that learns its turn began.
                run.refused = 0;
                // What became of a watch's fire is what became of the turn
                // that carried it.
                if let Some(ended) = &ended {
                    let became = match &outcome {
                        Outcome::Completed => subscription::Outcome::Done,
                        Outcome::Interrupted => subscription::Outcome::Stopped,
                        Outcome::Failed(_) => subscription::Outcome::Failed,
                    };
                    for watch in &mut run.subscriptions {
                        if ended.fires.contains(&watch.id)
                            && let Some(last) = &mut watch.last
                        {
                            last.outcome = became;
                            run.dirty = true;
                        }
                    }
                    // A turn Neptune began that ended with something said is
                    // worth a word to the person. "Nothing to report." is
                    // not: that is how a watch usually ends.
                    update = ended.origin == Origin::Events
                        && outcome == Outcome::Completed
                        && !ended.said.trim().is_empty()
                        && !nothing_to_report(&ended.said);
                }
                let through = ended.and_then(|running| running.through);
                run.status = None;
                run.idle = now;
                run.stale = true;
                run.say(Entry::End {
                    turn,
                    outcome: match &outcome {
                        Outcome::Completed => Ended::Completed,
                        Outcome::Interrupted => Ended::Interrupted,
                        Outcome::Failed(Failure::Limit { .. }) => Ended::Limit,
                        Outcome::Failed(_) => Ended::Failed,
                    },
                    cost: cost_usd,
                });
                // What a failed turn carried is not known to have reached
                // the lead: without the mark it waits for it once more
                // when Neptune next opens, as after a turn cut short.
                if let Some(through) = through
                    && !matches!(outcome, Outcome::Failed(_))
                {
                    run.say(Entry::Delivered { through });
                }
                match outcome {
                    Outcome::Completed => {}
                    Outcome::Interrupted => run.notice("You stopped the lead."),
                    Outcome::Failed(failure) => {
                        self.diagnostics
                            .failure("project_turn", None, None, failure.kind());
                        if let Failure::Limit { resets_at } = failure {
                            run.limited = limit_end(resets_at, now).or(run.limited);
                        }
                        run.notice(trouble_text(&failure, kind, now, run.limited));
                        // What only the person can clear is pinned as well.
                        if matches!(failure, Failure::Auth | Failure::Limit { .. }) {
                            run.trouble = Some(failure);
                        }
                    }
                }
            }
            LeadEvent::Exited { .. } => {
                run.lead = None;
                run.stream = Stream::default();
                run.status = None;
                run.idle = now;
                // A turn it was handed and never began waits again.
                run.inbox.abandoned();
                run.taken = None;
                run.offered = None;
                if let Some(running) = run.turn.take() {
                    for watch in &mut run.subscriptions {
                        if running.fires.contains(&watch.id)
                            && let Some(last) = &mut watch.last
                        {
                            last.outcome = subscription::Outcome::Failed;
                            run.dirty = true;
                        }
                    }
                    run.say(Entry::End {
                        turn: running.id,
                        outcome: Ended::Failed,
                        cost: None,
                    });
                    run.notice("The lead stopped before it finished its turn.");
                }
                self.sessions.agents().close_lead(id);
            }
        }
        if update {
            self.announce_project(ctx, id, "The lead has an update");
        }
        ctx.request_repaint();
    }
}

/// Compares each pull request a project follows with what its lead was last
/// told of it, and says what changed. `lookup` is what the watcher knows.
/// Nothing is compared while the project or the watch is paused, so what
/// changed meanwhile is said once when it goes on; the same holds for what
/// changed while Neptune was closed. A pull request that was merged or
/// closed ends its watch.
fn follow(
    run: &mut Run,
    lookup: impl Fn(&PullRequest) -> Lookup,
    paused: bool,
    at: u64,
    now: Instant,
) {
    if paused {
        return;
    }
    let mut ended = Vec::new();
    for index in 0..run.subscriptions.len() {
        let watch = &run.subscriptions[index];
        let Trigger::PullRequest { url, .. } = &watch.trigger else {
            continue;
        };
        let Some(link) = watch.link().filter(|_| watch.runs()) else {
            continue;
        };
        let Some(stands) = lookup(&link).status().map(seen_of) else {
            continue;
        };
        let Some(before) = watch.seen.filter(|before| *before != stands) else {
            // Seen for the first time, it is remembered and nothing is said.
            if watch.seen.is_none() {
                run.subscriptions[index].seen = Some(stands);
                run.dirty = true;
            }
            continue;
        };
        let changes = subscription::changes(before, stands);
        if changes.is_empty() {
            run.subscriptions[index].seen = Some(stands);
            run.dirty = true;
            continue;
        }
        let ends = changes.iter().any(|change| change.ends());
        // An earlier word about it has not been taken, or it woke the lead
        // often enough today: what changed is said with what follows. That
        // it is over is always said.
        if run.firing(watch.id) || (watch.spent(at) && !ends) {
            continue;
        }
        let (id, saved) = (watch.id, watch.saved);
        let owner = run.owner(url);
        let changed = changes
            .iter()
            .map(|change| change.words())
            .collect::<Vec<_>>()
            .join("; ");
        let whose = match &owner {
            Some((agent, title)) if !title.is_empty() => format!(" of agent {agent} “{title}”"),
            Some((agent, _)) => format!(" of agent {agent}"),
            None => String::new(),
        };
        let row = Entry::Event {
            source: Source::Pr,
            agent: Some(id),
            what: format!("Pull request {}{whose}: {changed}", link.label()),
            text: subscription::sentence(&stands.words()),
        };
        let told = prompt::pull_request(
            url,
            owner
                .as_ref()
                .map(|(agent, title)| (*agent, title.as_str())),
            &changed,
            &stands.words(),
            saved,
            &watch.instruction,
        );
        if run.fire(row, Some(id), told, now) {
            let watch = &mut run.subscriptions[index];
            watch.seen = Some(stands);
            watch.fired(at, at, true);
            run.dirty = true;
            if ends {
                ended.push(id);
            }
        }
    }
    run.subscriptions.retain(|watch| !ended.contains(&watch.id));
}

/// When a usage limit that resets at `resets_at` (seconds since the Unix
/// epoch) ends, as the frame's clock counts. Unknown or past, it ends when
/// the person says so.
fn limit_end(resets_at: Option<u64>, now: Instant) -> Option<Instant> {
    let left = resets_at?
        .checked_sub(unix_now())
        .filter(|left| *left > 0)?;
    // A reset further off than a week is a mistake, not a wait.
    (left <= 7 * 24 * 3600).then(|| now + Duration::from_secs(left))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        projects::{inbox, transcript::Origin},
        runtime::agents::{AgentRequest, Starter, Task},
        ui::Action,
    };
    use eframe::egui::{Pos2, Rect, Vec2};
    use std::{cell::RefCell, rc::Rc};

    /// A lead played by the test: it takes turns and says what it is told to.
    #[derive(Default)]
    struct Script {
        state: Option<LeadState>,
        events: Vec<LeadEvent>,
        turns: Vec<(String, String)>,
        /// The pictures that went with the newest turn.
        pictures: Vec<PathBuf>,
        interrupts: usize,
        stream: Stream,
        said: Option<String>,
        dropped: bool,
    }
    impl Script {
        fn ready(&mut self, session: &str) {
            self.events.push(LeadEvent::Session { id: session.into() });
            self.events.push(LeadEvent::Ready {
                model: None,
                account_kind: None,
            });
            self.state = Some(LeadState::Ready);
        }
        fn fail(&mut self, failure: Failure) {
            self.events.push(LeadEvent::Failed(failure));
            self.events.push(LeadEvent::Exited { code: None });
            self.state = Some(LeadState::Dead);
        }
        fn turn(&self) -> String {
            self.turns
                .last()
                .expect("no turn was handed over")
                .0
                .clone()
        }
        fn begin(&mut self) {
            let turn = self.turn();
            self.events.push(LeadEvent::TurnStarted { turn });
        }
        fn end(&mut self, outcome: Outcome) {
            let turn = self.turn();
            self.events.push(LeadEvent::TurnEnded {
                turn,
                outcome,
                cost_usd: None,
            });
            self.state = Some(LeadState::Ready);
        }
        fn say(&mut self, block: u32, text: &str) {
            self.events.push(LeadEvent::TextDone {
                block,
                text: text.into(),
            });
        }
    }
    struct Fake(Rc<RefCell<Script>>);
    impl LeadHandle for Fake {
        fn state(&self) -> LeadState {
            self.0.borrow().state.unwrap_or(LeadState::Starting)
        }
        fn send_turn(&self, id: &str, text: &str, pictures: &[PathBuf]) -> Result<(), Refused> {
            let mut script = self.0.borrow_mut();
            match script.state.unwrap_or(LeadState::Starting) {
                LeadState::Ready => {
                    script.turns.push((id.into(), text.into()));
                    script.pictures = pictures.to_vec();
                    script.state = Some(LeadState::Busy);
                    Ok(())
                }
                LeadState::Starting => Err(Refused::Starting),
                LeadState::Busy => Err(Refused::Busy),
                LeadState::Dead => Err(Refused::Dead),
            }
        }
        fn interrupt(&self) {
            self.0.borrow_mut().interrupts += 1;
        }
        fn try_events(&self) -> Vec<LeadEvent> {
            std::mem::take(&mut self.0.borrow_mut().events)
        }
        fn revision(&self) -> u64 {
            self.0.borrow().stream.revision
        }
        fn stream(&self) -> Stream {
            self.0.borrow().stream.clone()
        }
        fn last_error(&self) -> Option<String> {
            self.0.borrow().said.clone()
        }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            self.0.borrow_mut().dropped = true;
        }
    }
    /// What a lead was started with, without the closure that opens it.
    struct Started {
        kind: AgentKind,
        folder: PathBuf,
        session: (Option<String>, bool),
        prompt: String,
        /// The model and the effort it was asked for.
        with: (Option<String>, Option<String>),
        script: Rc<RefCell<Script>>,
    }
    type Leads = Rc<RefCell<Vec<Started>>>;

    const WINDOW: Vec2 = Vec2::new(900.0, 640.0);

    fn application(root: &std::path::Path) -> (App, egui::Context, Leads) {
        let (mut app, _sender) = super::super::tests::fixture(root);
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        app.startup = None;
        // A capture run makes no project; this is an ordinary one that
        // saves nothing because its storage is protected.
        app.ephemeral = false;
        let leads = Leads::default();
        attach(&mut app, root, &leads);
        (app, ctx, leads)
    }
    /// Gives `app` the projects kept under `root`, with scripted leads.
    fn attach(app: &mut App, root: &std::path::Path, leads: &Leads) {
        app.projects = Projects::inline(root.join("projects"));
        let started = leads.clone();
        app.projects.start = Box::new(move |launch, _| {
            let script = Rc::new(RefCell::new(Script::default()));
            started.borrow_mut().push(Started {
                kind: launch.kind,
                folder: launch.project_dir,
                session: (launch.session.id, launch.session.opened),
                prompt: launch.system_prompt,
                with: (launch.model, launch.effort),
                script: script.clone(),
            });
            Box::new(Fake(script))
        });
    }
    fn workspace(app: &mut App, root: &std::path::Path, name: &str) -> WorkspaceId {
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: root.into(),
                name: name.into(),
                remote: None,
            })
            .unwrap();
        app.controller.model().active_workspace().unwrap()
    }
    /// A project as a restored window has one: in the model, with no lead.
    fn project(app: &mut App, workspace: WorkspaceId, key: u64) -> ProjectId {
        app.controller
            .dispatch(Command::AddProject {
                workspace,
                name: "shop".into(),
                key: ProjectKey::parse(&format!("{key:016x}")).unwrap(),
                lead: AgentKind::Claude,
            })
            .unwrap();
        app.controller.model().project_of(workspace).unwrap().id()
    }
    fn tick(app: &mut App, ctx: &egui::Context, now: Instant) {
        app.serve_agents(ctx);
        app.poll_projects_at(ctx, now);
    }
    fn frame(app: &mut App, ctx: &egui::Context) {
        frame_of(app, ctx, WINDOW);
    }
    fn frame_of(app: &mut App, ctx: &egui::Context, window: Vec2) {
        let mut host = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, window)),
                ..Default::default()
            },
            |ui| eframe::App::ui(app, ui, &mut host),
        );
        output.textures_delta.clear();
    }
    fn send(app: &mut App, ctx: &egui::Context, project: ProjectId, text: &str) {
        app.action(
            ctx,
            Action::Project(Event::Send {
                project,
                text: text.into(),
                attachments: Vec::new(),
            }),
        );
    }
    fn entries(app: &App, project: ProjectId) -> Vec<Entry> {
        app.projects.runs[&project]
            .chat
            .records()
            .iter()
            .map(|record| record.entry.clone())
            .collect()
    }
    /// The tab's project as it would be drawn.
    fn shown(app: &App) -> (Vec<String>, Vec<String>, Vec<String>, bool, usize) {
        let panel = app.project_panel(true);
        let Body::Project(project) = app.projects.view(&panel, ui::chat::PAGE) else {
            panic!("the workspace in view has no project");
        };
        (
            project
                .needs
                .iter()
                .map(|need| format!("{}: {}", need.title, need.detail))
                .collect(),
            project
                .members
                .iter()
                .map(|member| format!("{} {} · {}", member.pane, member.title, member.state))
                .collect(),
            project.chat.pending.to_vec(),
            project.busy,
            project.queued,
        )
    }

    #[cfg_attr(windows, ignore = "its directories are written as on Unix")]
    #[test]
    fn the_form_offers_the_clis_found_off_the_frame_and_codex_leads_where_it_is_picked() {
        use std::{cell::Cell, sync::mpsc::sync_channel};
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads) = application(dir.path());
        let shell_workspace = workspace(&mut app, dir.path(), "shop");
        let start = Instant::now();
        // Whoever looks answers when the test says so.
        let asked = Rc::new(Cell::new(0));
        let answers = Rc::new(RefCell::new(Vec::new()));
        let (count, waiting) = (asked.clone(), answers.clone());
        app.projects.installed = Installed {
            found: None,
            looking: None,
            looked: None,
            find: Box::new(move |_| {
                count.set(count.get() + 1);
                let (answer, found) = sync_channel(1);
                waiting.borrow_mut().push(answer);
                found
            }),
        };
        let offered = |app: &App| {
            let panel = app.project_panel(true);
            match app.projects.view(&panel, ui::chat::PAGE) {
                Body::Empty { leads, .. } => leads.map(<[AgentKind]>::to_vec),
                _ => panic!("no form is in view"),
            }
        };
        // Nothing is looked for until the form that offers them shows, and
        // then once; until the answer waits the form offers nothing.
        tick(&mut app, &ctx, start);
        assert_eq!(asked.get(), 0);
        for _ in 0..3 {
            let panel = app.project_panel(true);
            app.projects.look_for_leads(&ctx, &panel);
            tick(&mut app, &ctx, start);
        }
        assert_eq!((asked.get(), offered(&app)), (1, None));
        answers
            .borrow_mut()
            .remove(0)
            .send(vec![AgentKind::Codex])
            .unwrap();
        tick(&mut app, &ctx, start);
        assert_eq!(offered(&app), Some(vec![AgentKind::Codex]));
        // With one still missing it is looked for again a minute later, not
        // before; a look that never answers leaves what is known.
        let panel = app.project_panel(true);
        app.projects.look_for_leads(&ctx, &panel);
        assert_eq!(asked.get(), 1);
        app.projects.installed.looked = Some(Instant::now() - LOOK_AGAIN);
        app.projects.look_for_leads(&ctx, &panel);
        assert_eq!(asked.get(), 2);
        answers.borrow_mut().clear();
        tick(&mut app, &ctx, start);
        assert_eq!(offered(&app), Some(vec![AgentKind::Codex]));
        assert!(app.projects.installed.looking.is_none());
        // Both found, there is nothing left to look for.
        app.projects.installed.found = Some(project_lead::LEADS.to_vec());
        app.projects.installed.looked = None;
        app.projects.look_for_leads(&ctx, &panel);
        assert_eq!(asked.get(), 2);
        // Where a project is, its settings say whether its lead's CLI is
        // here: they are looked for the same way, once.
        app.projects.installed.found = None;

        // The project is led by the CLI it was made with, from the same
        // prompt as any lead.
        app.action(
            &ctx,
            Action::Project(Event::Create {
                workspace: shell_workspace,
                goal: "Split checkout".into(),
                lead: AgentKind::Codex,
                model: None,
                effort: None,
            }),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while leads.borrow().is_empty() {
            assert!(Instant::now() < deadline, "the lead was never started");
            std::thread::sleep(Duration::from_millis(5));
            tick(&mut app, &ctx, start);
        }
        for _ in 0..2 {
            let panel = app.project_panel(true);
            app.projects.look_for_leads(&ctx, &panel);
        }
        assert_eq!(asked.get(), 3);
        // No workspace in view, nothing is looked for.
        app.projects.look_for_leads(&ctx, &app.project_panel(false));
        assert_eq!(asked.get(), 3);
        let item = app.controller.model().project_of(shell_workspace).unwrap();
        let id = item.id();
        assert_eq!(item.lead(), AgentKind::Codex);
        assert_eq!(leads.borrow()[0].kind, AgentKind::Codex);
        assert_eq!(leads.borrow()[0].prompt, prompt::LEAD_PROMPT);
        // Codex names its conversation itself: none is handed to it.
        assert_eq!(leads.borrow()[0].session, (None, false));
        assert_eq!(
            app.projects.runs[&id].status.as_deref(),
            Some("Starting Codex…")
        );
        let script = leads.borrow()[0].script.clone();
        script.borrow_mut().ready("codex-thread");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let session = &app.projects.runs[&id].session;
        assert_eq!(
            (session.id.as_deref(), session.opened),
            (Some("codex-thread"), true)
        );
        // It does not say how often it will try again.
        script.borrow_mut().begin();
        script.borrow_mut().events.push(LeadEvent::Retry {
            attempt: 1,
            max: 0,
            delay_ms: 0,
        });
        tick(&mut app, &ctx, start);
        assert_eq!(
            app.projects.runs[&id].status.as_deref(),
            Some("Codex is busy. Retrying…")
        );
        // Signed out, the person is told how to sign in to this CLI.
        script.borrow_mut().end(Outcome::Failed(Failure::Auth));
        tick(&mut app, &ctx, start);
        let (needs, ..) = shown(&app);
        assert_eq!(
            needs,
            [
                "The lead needs you: Codex isn't signed in. Run `codex login` in a terminal, \
              then try again."
            ]
        );
    }

    #[cfg_attr(windows, ignore = "its directories are written as on Unix")]
    #[test]
    fn a_project_is_made_talks_to_its_lead_starts_an_agent_and_hears_what_became_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads) = application(dir.path());
        let shell_workspace = workspace(&mut app, dir.path(), "shop");
        let shell = app.controller.model().active_pane().unwrap();
        let start = Instant::now();
        // Nothing is polled, collected or started without a project.
        tick(&mut app, &ctx, start);
        assert!(app.projects.runs.is_empty() && leads.borrow().is_empty());
        assert!(matches!(
            app.project_panel(true).body,
            PanelBody::Empty {
                starting: false,
                ..
            }
        ));

        // Its folder is made off the frame; the model takes the project
        // once the folder exists.
        app.ui.project.goal = "Split checkout".into();
        app.action(
            &ctx,
            Action::Project(Event::Create {
                workspace: shell_workspace,
                goal: "Split checkout".into(),
                lead: AgentKind::Claude,
                model: None,
                effort: None,
            }),
        );
        assert!(app.controller.model().projects().is_empty());
        assert!(matches!(
            app.project_panel(true).body,
            PanelBody::Empty { starting: true, .. }
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.controller.model().projects().is_empty() {
            assert!(Instant::now() < deadline, "the project was never made");
            std::thread::sleep(Duration::from_millis(5));
            tick(&mut app, &ctx, start);
        }
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        let item = app.controller.model().project_of(shell_workspace).unwrap();
        let (project, key) = (item.id(), item.key().clone());
        assert_eq!(
            (item.name(), item.lead(), item.paused()),
            ("shop", AgentKind::Claude, false)
        );
        let folder = dir.path().join("projects").join(key.as_str());
        assert!(folder.is_dir());
        assert_eq!(app.ui.project.goal, "", "the goal became the first message");
        assert_eq!(
            entries(&app, project),
            [Entry::User {
                text: "Split checkout".into(),
                attachments: Vec::new(),
            }]
        );

        // The first message starts the lead, in the project's own folder
        // and in a session of its own.
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(
            leads.borrow().len(),
            1,
            "one lead, however often it is polled"
        );
        let script = {
            let leads = leads.borrow();
            assert_eq!(leads[0].folder, folder);
            assert_eq!(leads[0].session, (None, false));
            assert_eq!(leads[0].prompt, prompt::LEAD_PROMPT);
            leads[0].script.clone()
        };
        assert_eq!(shown(&app).4, 1, "the message waits while the lead starts");
        assert!(script.borrow().turns.is_empty());

        // Ready, it is handed the turn: the state of the project, then the
        // user's words.
        script.borrow_mut().ready("session-1");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let turns = script.borrow().turns.clone();
        assert_eq!(turns.len(), 1, "a turn is handed over once");
        assert!(
            turns[0].1.starts_with(
                "<project-state generated-by=\"neptune\">\nproject: shop · directory: "
            )
        );
        assert!(turns[0].1.contains("\nagents: none\n</project-state>\n"));
        assert!(turns[0].1.ends_with("\nSplit checkout\n"));
        assert!(!turns[0].1.contains("<neptune-events>"));
        // Handed over is not begun: the message still counts as waiting.
        let (_, _, _, busy, queued) = shown(&app);
        assert!(busy && queued == 1);

        // The lead plans and asks for an agent.
        {
            let mut script = script.borrow_mut();
            script.begin();
            script.say(1, "One agent for the auth part.");
            script.events.push(LeadEvent::ToolStarted {
                call: "call-1".into(),
                tool: "spawn_agent".into(),
                input: serde_json::json!({"agent": "codex", "title": "auth"}),
            });
            script.stream = Stream {
                revision: 4,
                block: 2,
                text: "Starti".into(),
            };
        }
        tick(&mut app, &ctx, start);
        let (_, members, pending, busy, queued) = shown(&app);
        assert!(busy && queued == 0 && members.is_empty());
        assert_eq!(pending, ["Starting agent “auth”…"]);
        {
            let panel = app.project_panel(true);
            let Body::Project(view) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("no project in view");
            };
            assert_eq!(view.chat.streaming, Some("Starti"));
        }

        // Neptune opens the agent's terminal out of view, and the chat says
        // what Neptune did, in place of what the lead said it would.
        app.sessions.agents().request(AgentRequest::Spawn {
            request: 1,
            parent: Starter::Project(project),
            generation: 1,
            task: Task {
                title: Some("auth".into()),
                ..Task::new(AgentKind::Codex, "PRIVATE BRIEF")
            },
            cwd: Default::default(),
        });
        tick(&mut app, &ctx, start);
        let Some(Ok(agent)) = app.sessions.agents().spawned(1) else {
            panic!("no terminal was opened");
        };
        let model = app.controller.model();
        assert_eq!(model.pane(agent).unwrap().project(), Some(project));
        assert!(model.workspaces()[0].is_background(agent));
        assert_eq!(model.active_pane(), Some(shell));
        let (_, members, pending, ..) = shown(&app);
        assert!(pending.is_empty());
        assert_eq!(members, [format!("{agent} auth · Starting")]);
        let card = Entry::Tool {
            name: "spawn_agent".into(),
            summary: format!("Started agent {agent} · Codex · auth"),
            ok: true,
        };
        assert_eq!(entries(&app, project).last(), Some(&card));

        // The turn ends; the lead waits to be woken.
        {
            let mut script = script.borrow_mut();
            script.events.push(LeadEvent::ToolDone {
                call: "call-1".into(),
                ok: true,
            });
            script.say(2, "Started it. I will report when it is done.");
            script.end(Outcome::Completed);
        }
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start + Duration::from_secs(60));
        assert_eq!(script.borrow().turns.len(), 1, "nothing happened: no turn");
        let (_, _, _, busy, _) = shown(&app);
        assert!(!busy);
        assert_eq!(
            entries(&app, project)
                .iter()
                .filter(|entry| **entry == card)
                .count(),
            1,
            "a call that was carried out has one card"
        );
        // The tab draws with all of it in view.
        app.action(
            &ctx,
            Action::Panel(ui::panel::Event::Show(ui::panel::Tab::Project)),
        );
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert!(app.ui.error.is_none());

        // The agent says what it did and its terminal closes. Nobody
        // polled: the application collects it and tells the lead, once it
        // has been quiet for a moment.
        app.sessions.agents().replied(agent, "PR #214 is up.");
        app.controller.dispatch(Command::ClosePane(agent)).unwrap();
        let later = start + Duration::from_secs(120);
        tick(&mut app, &ctx, later);
        let Some(Entry::Event {
            source: Source::Agent,
            agent: about,
            what,
            text,
        }) = entries(&app, project).pop()
        else {
            panic!("no event row: {:?}", entries(&app, project));
        };
        assert_eq!(
            (about, text.as_str()),
            (Some(agent.get()), "PR #214 is up.")
        );
        assert!(what.starts_with("“auth” ended"), "{what}");
        assert_eq!(script.borrow().turns.len(), 1, "not before a quiet moment");
        tick(
            &mut app,
            &ctx,
            later + inbox::QUIET - Duration::from_millis(1),
        );
        assert_eq!(script.borrow().turns.len(), 1);
        tick(&mut app, &ctx, later + inbox::QUIET);
        tick(&mut app, &ctx, later + inbox::QUIET);
        let turns = script.borrow().turns.clone();
        assert_eq!(turns.len(), 2, "one turn for what happened");
        assert!(turns[1].1.contains("\nagents: none\n"));
        assert!(turns[1].1.contains(&format!(
            "<neptune-events>\n[agent {agent} \"auth\" ended: "
        )));
        assert!(turns[1].1.contains(&format!(
            "<agent-report agent=\"{agent}\" trust=\"agent\">\nPR #214 is up.\n</agent-report>\n</neptune-events>\n"
        )));
        {
            let mut script = script.borrow_mut();
            script.begin();
            script.say(1, "Agent done: PR #214.");
            script.end(Outcome::Completed);
        }
        tick(&mut app, &ctx, later + inbox::QUIET);
        let all = entries(&app, project);
        let kinds: Vec<&str> = all
            .iter()
            .map(|entry| match entry {
                Entry::User { .. } => "user",
                Entry::Turn {
                    origin: Origin::User,
                    ..
                } => "turn:user",
                Entry::Turn { .. } => "turn:events",
                Entry::Lead { .. } => "lead",
                Entry::Tool { .. } => "tool",
                Entry::Event { .. } => "event",
                Entry::Proposal { .. } => "proposal",
                Entry::End { .. } => "end",
                Entry::Delivered { .. } => "delivered",
                Entry::Notice { .. } => "notice",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user",
                "turn:user",
                "lead",
                "tool",
                "lead",
                "end",
                "event",
                "turn:events",
                "lead",
                "end",
                "delivered"
            ]
        );
        // The mark names the event row the lead was told of.
        assert_eq!(all.last(), Some(&Entry::Delivered { through: 7 }));
        // None of it is in what is saved of the window, or said aloud.
        let saved = serde_json::to_string(
            &crate::persistence::workspace_state::StateSnapshot::from_model(app.controller.model()),
        )
        .unwrap();
        for private in ["Split checkout", "PRIVATE", "PR #214", "auth", "session-1"] {
            assert!(!saved.contains(private), "{private} in {saved}");
        }
        assert!(saved.contains(key.as_str()));
        frame(&mut app, &ctx);
        assert!(app.ui.error.is_none());
    }

    #[cfg_attr(windows, ignore = "its directories are written as on Unix")]
    #[test]
    fn a_paused_project_sends_only_the_users_words_and_a_removed_one_lets_its_lead_go() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads) = application(dir.path());
        let space = workspace(&mut app, dir.path(), "shop");
        let id = project(&mut app, space, 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        // A project from an earlier run has no lead until it is spoken to.
        assert!(leads.borrow().is_empty());
        send(&mut app, &ctx, id, "hello");
        tick(&mut app, &ctx, start);
        let script = leads.borrow()[0].script.clone();
        script.borrow_mut().ready("s");
        tick(&mut app, &ctx, start);
        script.borrow_mut().begin();
        script.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start);
        assert_eq!(script.borrow().turns.len(), 1);

        // An agent of the project, so that there is something to hear of.
        app.sessions.agents().request(AgentRequest::Spawn {
            request: 1,
            parent: Starter::Project(id),
            generation: 1,
            task: Task {
                title: Some("api".into()),
                ..Task::new(AgentKind::Claude, "task")
            },
            cwd: Default::default(),
        });
        tick(&mut app, &ctx, start);
        let Some(Ok(agent)) = app.sessions.agents().spawned(1) else {
            panic!("no terminal was opened");
        };
        // Paused: what Neptune would send by itself is held, however long.
        app.action(&ctx, Action::Project(Event::SetPaused(id, true)));
        assert!(app.controller.model().project(id).unwrap().paused());
        app.sessions.agents().replied(agent, "halfway");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start + Duration::from_secs(600));
        assert_eq!(script.borrow().turns.len(), 1);
        assert!(matches!(
            entries(&app, id).last(),
            Some(Entry::Event { .. })
        ));
        // The user's own message still goes, and says the project is paused.
        send(&mut app, &ctx, id, "status?");
        tick(&mut app, &ctx, start + Duration::from_secs(600));
        let turns = script.borrow().turns.clone();
        assert_eq!(turns.len(), 2);
        assert!(turns[1].1.contains("\npaused: yes"));
        assert!(!turns[1].1.contains("<neptune-events>") && turns[1].1.ends_with("\nstatus?\n"));
        script.borrow_mut().begin();
        // Stop interrupts the turn that runs.
        tick(&mut app, &ctx, start + Duration::from_secs(600));
        app.action(&ctx, Action::Project(Event::Stop(id)));
        assert_eq!(script.borrow().interrupts, 1);
        script.borrow_mut().end(Outcome::Interrupted);
        tick(&mut app, &ctx, start + Duration::from_secs(600));
        assert!(matches!(
            entries(&app, id).last(),
            Some(Entry::Notice { text }) if text == "You stopped the lead."
        ));
        assert_eq!(script.borrow().turns.len(), 2, "still paused");
        // Resumed, what waited is sent as one turn.
        app.action(&ctx, Action::Project(Event::SetPaused(id, false)));
        tick(&mut app, &ctx, start + Duration::from_secs(700));
        let turns = script.borrow().turns.clone();
        assert_eq!(turns.len(), 3);
        assert!(turns[2].1.contains("halfway") && !turns[2].1.contains("paused: yes"));
        script.borrow_mut().begin();
        script.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start + Duration::from_secs(700));

        // Renaming and removal go through their sheets.
        app.action(&ctx, Action::Project(Event::Rename(id)));
        assert_eq!(app.ui.overlay, OverlayState::RenameProject(id));
        assert_eq!(app.ui.rename_name, "shop");
        app.action(&ctx, Action::Project(Event::SetName(id, "checkout".into())));
        assert_eq!(
            app.controller.model().project(id).unwrap().name(),
            "checkout"
        );
        app.ui.project.drafts.insert(id, "half a thought".into());
        app.action(&ctx, Action::Project(Event::Remove(id)));
        assert_eq!(app.ui.overlay, OverlayState::RemoveProject(id));
        frame(&mut app, &ctx);
        assert!(app.controller.model().project(id).is_some(), "asked first");
        app.action(&ctx, Action::Project(Event::ConfirmRemove(id)));
        assert_eq!(app.ui.overlay, OverlayState::None);
        // Its agent goes on in a tab of its own; nothing was stopped.
        let model = app.controller.model();
        assert!(model.projects().is_empty());
        let pane = model.pane(agent).unwrap();
        assert_eq!(pane.project(), None);
        assert!(!model.workspaces()[0].is_background(agent));
        // The lead is let go on the next poll, with everything of its chat.
        assert!(!script.borrow().dropped);
        tick(&mut app, &ctx, start + Duration::from_secs(700));
        assert!(script.borrow().dropped);
        assert!(app.projects.runs.is_empty() && app.ui.project.drafts.is_empty());
        assert!(matches!(
            app.project_panel(true).body,
            PanelBody::Empty { .. }
        ));
        assert_eq!(app.project_needs(), 0);
        // A message for a project that is gone goes nowhere.
        send(&mut app, &ctx, id, "anyone?");
        tick(&mut app, &ctx, start + Duration::from_secs(700));
        assert_eq!(leads.borrow().len(), 1);
    }

    #[test]
    fn a_lead_that_cannot_go_on_says_what_the_person_can_do_and_waits_for_them() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads) = application(dir.path());
        let space = workspace(&mut app, dir.path(), "shop");
        let id = project(&mut app, space, 1);
        let start = Instant::now();
        let script = |leads: &Leads, at: usize| leads.borrow()[at].script.clone();
        send(&mut app, &ctx, id, "hello");
        tick(&mut app, &ctx, start);
        // Not signed in: said where it is seen first, and not tried again
        // until the person asks.
        script(&leads, 0).borrow_mut().fail(Failure::Auth);
        tick(&mut app, &ctx, start);
        assert!(script(&leads, 0).borrow().dropped);
        for n in 0..3 {
            tick(&mut app, &ctx, start + Duration::from_secs(n));
        }
        assert_eq!(leads.borrow().len(), 1, "no start is tried in a loop");
        let (needs, _, _, busy, queued) = shown(&app);
        assert_eq!(
            needs,
            [
                "The lead needs you: Claude Code isn't signed in. Run `claude` in a terminal and sign in, then try again."
            ]
        );
        assert!(!busy && queued == 1, "the message is kept");
        assert_eq!(app.project_needs(), 1);
        app.action(&ctx, Action::Project(Event::Retry(id)));
        tick(&mut app, &ctx, start);
        assert_eq!(leads.borrow().len(), 2);
        assert_eq!(app.project_needs(), 0);
        // The session it never opened is opened, not resumed.
        assert_eq!(leads.borrow()[1].session, (None, false));

        // Not installed, tools unreachable and an unexpected end each say so.
        for (failure, said) in [
            (Failure::NotInstalled, "isn't installed or isn't on PATH"),
            (Failure::ToolsUnavailable, "could not reach Neptune's tools"),
            (Failure::Transport, "stopped unexpectedly. It said: boom"),
        ] {
            let at = leads.borrow().len() - 1;
            script(&leads, at).borrow_mut().said = Some("boom".into());
            script(&leads, at).borrow_mut().fail(failure);
            tick(&mut app, &ctx, start);
            let Some(Entry::Notice { text }) = entries(&app, id).pop() else {
                panic!("no notice");
            };
            assert!(text.contains(said), "{said} in {text}");
            assert_eq!(shown(&app).0.len(), 1);
            // Writing again is asking again.
            send(&mut app, &ctx, id, "again");
            tick(&mut app, &ctx, start);
            assert_eq!(leads.borrow().len(), at + 2);
        }

        // A session that is gone is replaced without the person's help.
        let at = leads.borrow().len() - 1;
        script(&leads, at).borrow_mut().ready("session-1");
        tick(&mut app, &ctx, start);
        script(&leads, at).borrow_mut().begin();
        script(&leads, at).borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start);
        // Idle half an hour with no agent at work, the lead is let go.
        tick(&mut app, &ctx, start + IDLE - Duration::from_secs(1));
        assert!(!script(&leads, at).borrow().dropped);
        tick(&mut app, &ctx, start + IDLE);
        assert!(script(&leads, at).borrow().dropped);
        send(&mut app, &ctx, id, "back");
        tick(&mut app, &ctx, start + IDLE);
        // It comes back in the session it had.
        assert_eq!(
            leads.borrow()[at + 1].session,
            (Some("session-1".into()), true)
        );
        script(&leads, at + 1)
            .borrow_mut()
            .fail(Failure::SessionLost);
        tick(&mut app, &ctx, start + IDLE);
        tick(&mut app, &ctx, start + IDLE);
        assert_eq!(leads.borrow().len(), at + 3);
        assert_eq!(leads.borrow()[at + 2].session, (None, false));
        assert_eq!(app.project_needs(), 0);

        // A usage limit holds everything until it resets, or until the
        // person says to try.
        let lead = script(&leads, at + 2);
        lead.borrow_mut().ready("session-2");
        tick(&mut app, &ctx, start + IDLE);
        lead.borrow_mut().begin();
        let resets_at = unix_now() + 3600;
        lead.borrow_mut().events.push(LeadEvent::Limit {
            resets_at: Some(resets_at),
        });
        lead.borrow_mut().end(Outcome::Failed(Failure::Limit {
            resets_at: Some(resets_at),
        }));
        let now = start + IDLE;
        tick(&mut app, &ctx, now);
        let (needs, ..) = shown(&app);
        assert!(
            needs[0].contains("usage limit was reached. It resets in 1 h"),
            "{needs:?}"
        );
        send(&mut app, &ctx, id, "more");
        tick(&mut app, &ctx, now + Duration::from_secs(60));
        let turns = lead.borrow().turns.len();
        assert_eq!(app.project_needs(), 1, "a message does not lift a limit");
        tick(&mut app, &ctx, now + Duration::from_secs(3700));
        assert_eq!(lead.borrow().turns.len(), turns + 1, "it goes on by itself");
        assert_eq!(app.project_needs(), 0);
        lead.borrow_mut().begin();
        lead.borrow_mut()
            .end(Outcome::Failed(Failure::Limit { resets_at: None }));
        tick(&mut app, &ctx, now + Duration::from_secs(3700));
        send(&mut app, &ctx, id, "and now?");
        tick(&mut app, &ctx, now + Duration::from_secs(99_999));
        assert_eq!(lead.borrow().turns.len(), turns + 1);
        app.action(&ctx, Action::Project(Event::Retry(id)));
        tick(&mut app, &ctx, now + Duration::from_secs(99_999));
        assert_eq!(lead.borrow().turns.len(), turns + 2);
        // A turn that fails otherwise is said in the chat and pins nothing.
        // A retry is said while it lasts, not once the lead got through.
        lead.borrow_mut().begin();
        lead.borrow_mut().events.push(LeadEvent::Retry {
            attempt: 1,
            max: 10,
            delay_ms: 500,
        });
        tick(&mut app, &ctx, now + Duration::from_secs(99_999));
        assert_eq!(
            app.projects.runs[&id].status.as_deref(),
            Some("Claude Code is busy. Retrying (1 of 10)…")
        );
        lead.borrow_mut().say(1, "Here.");
        tick(&mut app, &ctx, now + Duration::from_secs(99_999));
        assert_eq!(app.projects.runs[&id].status, None);
        lead.borrow_mut()
            .end(Outcome::Failed(Failure::Provider("bad request".into())));
        tick(&mut app, &ctx, now + Duration::from_secs(99_999));
        assert!(matches!(
            entries(&app, id).last(),
            Some(Entry::Notice { text }) if text == "The lead's turn failed: bad request"
        ));
        assert_eq!(app.project_needs(), 0);
        // A lead that dies in a turn ends the turn, and what it was handed
        // but never began waits for the next one.
        send(&mut app, &ctx, id, "first");
        tick(&mut app, &ctx, now + Duration::from_secs(99_999));
        lead.borrow_mut().begin();
        lead.borrow_mut()
            .events
            .push(LeadEvent::Exited { code: Some(1) });
        lead.borrow_mut().state = Some(LeadState::Dead);
        tick(&mut app, &ctx, now + Duration::from_secs(99_999));
        assert!(matches!(
            entries(&app, id).last(),
            Some(Entry::Notice { text }) if text == "The lead stopped before it finished its turn."
        ));
        assert!(!shown(&app).3);
    }

    #[test]
    fn three_leads_run_at_once_and_neptune_pauses_a_project_that_runs_on_by_itself() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads) = application(dir.path());
        let start = Instant::now();
        let mut projects = Vec::new();
        for n in 1..=4 {
            let space = workspace(&mut app, dir.path(), &format!("w{n}"));
            projects.push(project(&mut app, space, n));
        }
        for id in &projects {
            send(&mut app, &ctx, *id, "go");
        }
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(leads.borrow().len(), 3, "the fourth waits");
        assert_eq!(
            app.projects.runs[&projects[3]].status.as_deref(),
            Some("Waiting for another project's lead: three run at a time.")
        );
        // A lead that rests, with no agent at work, gives way.
        let first = leads.borrow()[0].script.clone();
        first.borrow_mut().ready("s1");
        tick(&mut app, &ctx, start);
        assert_eq!(leads.borrow().len(), 3, "a lead in a turn is not let go");
        first.borrow_mut().begin();
        first.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(leads.borrow().len(), 4);
        assert!(first.borrow().dropped);
        assert!(app.projects.runs[&projects[0]].lead.is_none());

        // The fourth project runs on by itself, turn after turn, until
        // Neptune pauses it.
        let id = projects[3];
        app.action(
            &ctx,
            Action::SelectWorkspace(app.controller.model().project(id).unwrap().workspace()),
        );
        let lead = leads.borrow()[3].script.clone();
        lead.borrow_mut().ready("s4");
        tick(&mut app, &ctx, start);
        lead.borrow_mut().begin();
        lead.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start);
        let mut now = start;
        for round in 0..inbox::MAX_AUTO_STREAK {
            let run = app.projects.runs.get_mut(&id).unwrap();
            run.event(AgentEvent::new(7, None, What::Finished), now);
            now += Duration::from_secs(600);
            tick(&mut app, &ctx, now);
            assert_eq!(
                lead.borrow().turns.len(),
                round as usize + 2,
                "round {round}"
            );
            lead.borrow_mut().begin();
            lead.borrow_mut().end(Outcome::Completed);
            tick(&mut app, &ctx, now);
        }
        let turns = lead.borrow().turns.len();
        let run = app.projects.runs.get_mut(&id).unwrap();
        run.event(AgentEvent::new(7, None, What::Finished), now);
        now += Duration::from_secs(600);
        tick(&mut app, &ctx, now);
        tick(&mut app, &ctx, now + Duration::from_secs(600));
        assert_eq!(lead.borrow().turns.len(), turns, "one too many is not sent");
        assert!(app.controller.model().project(id).unwrap().paused());
        let (needs, ..) = shown(&app);
        assert_eq!(needs.len(), 1);
        assert!(needs[0].starts_with("Paused after 20 turns without you"));
        assert_eq!(app.project_needs(), 1);
        // Resuming is the person's word that it may go on.
        app.action(&ctx, Action::Project(Event::SetPaused(id, false)));
        tick(&mut app, &ctx, now + Duration::from_secs(600));
        assert_eq!(lead.borrow().turns.len(), turns + 1);
        assert_eq!(app.project_needs(), 0);
    }

    /// Frames with the projects polled inside them, as the application
    /// does. Returns how soon the last of them asked to be drawn again.
    #[test]
    fn an_agent_that_asks_before_its_task_is_news_for_the_lead_once() {
        let mut run = Run::new(
            ProjectKey::parse("0000000000000007").unwrap(),
            Instant::now(),
        );
        let report = |agent: u64, status: Status, news: bool| AgentReport {
            agent,
            kind: AgentKind::Claude,
            status,
            news,
            replies: Vec::new(),
            reopens: false,
            title: None,
        };
        let asking = |screen: &str| Status::Asking {
            screen: screen.into(),
        };
        // One that has not started in good time is news, and so is its
        // question: the lead tells the person.
        assert!(run.hears(&report(2, Status::Starting, true)));
        assert!(!run.hears(&report(2, Status::Starting, false)));
        assert!(run.hears(&report(2, asking("Trust this folder?"), true)));
        // The person opens its terminal and moves in it: what it shows
        // changes and stands still again. It is the same wait.
        assert!(!run.hears(&report(2, Status::Starting, true)));
        assert!(!run.hears(&report(2, asking(""), true)));
        assert!(!run.hears(&report(2, asking("Allow all edits?"), true)));
        // Another agent's question is its own.
        assert!(run.hears(&report(3, asking("Trust this folder?"), true)));
        // What it says is heard whatever it shows.
        let mut said = report(2, asking(""), true);
        said.replies.push("Halfway".into());
        assert!(run.hears(&said));
        // Its task taken, a later question is news again.
        assert!(!run.hears(&report(2, Status::Working, false)));
        assert!(run.hears(&report(2, Status::Idle, true)));
        assert!(run.hears(&report(2, asking("Trust this folder?"), true)));
    }

    fn settle(app: &mut App, ctx: &egui::Context, frames: usize) -> Duration {
        let mut delay = Duration::ZERO;
        for _ in 0..frames {
            let mut host = eframe::Frame::_new_kittest();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, WINDOW)),
                    ..Default::default()
                },
                |ui| {
                    app.serve_agents(ctx);
                    app.poll_projects(ctx);
                    eframe::App::ui(app, ui, &mut host)
                },
            );
            output.textures_delta.clear();
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        }
        delay
    }

    #[test]
    fn a_project_with_nothing_to_do_does_not_keep_the_window_drawing() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads) = application(dir.path());
        let space = workspace(&mut app, dir.path(), "shop");
        // What the window asks for with no project is not the project's.
        let quiet = settle(&mut app, &ctx, 120);
        let id = project(&mut app, space, 1);
        app.action(
            &ctx,
            Action::Panel(ui::panel::Event::Show(ui::panel::Tab::Project)),
        );
        let still = |app: &mut App, what: &str| {
            let delay = settle(app, &ctx, 120);
            assert!(
                delay >= quiet.min(Duration::from_secs(1)),
                "{what}: drawn again after {delay:?}, without a project {quiet:?}"
            );
        };
        still(&mut app, "a project nobody spoke to");
        // A message waits while its lead starts: the lead's wake is what
        // moves it on, not a frame.
        send(&mut app, &ctx, id, "hello");
        still(&mut app, "a lead that is starting");
        let script = leads.borrow()[0].script.clone();
        script.borrow_mut().ready("s");
        still(&mut app, "a turn handed over");
        assert_eq!(script.borrow().turns.len(), 1);
        script.borrow_mut().begin();
        script.borrow_mut().say(1, "A plan.");
        still(&mut app, "a turn that runs");
        script.borrow_mut().end(Outcome::Completed);
        still(&mut app, "a lead that rests");
        // A lead that cannot go on waits for the person, and so does the
        // window.
        send(&mut app, &ctx, id, "again");
        settle(&mut app, &ctx, 2);
        script.borrow_mut().begin();
        script.borrow_mut().end(Outcome::Failed(Failure::Auth));
        send(&mut app, &ctx, id, "and again");
        app.projects.runs.get_mut(&id).unwrap().trouble = Some(Failure::Auth);
        still(&mut app, "a lead that needs the person");
        // Paused with news waiting, nothing is due.
        app.action(&ctx, Action::Project(Event::Retry(id)));
        settle(&mut app, &ctx, 2);
        script.borrow_mut().begin();
        script.borrow_mut().end(Outcome::Completed);
        app.action(&ctx, Action::Project(Event::SetPaused(id, true)));
        let run = app.projects.runs.get_mut(&id).unwrap();
        run.event(AgentEvent::new(7, None, What::Finished), Instant::now());
        still(&mut app, "a paused project with news");
    }

    #[test]
    fn a_turn_the_lead_never_began_is_handed_to_the_next_lead_once() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads) = application(dir.path());
        let space = workspace(&mut app, dir.path(), "shop");
        let id = project(&mut app, space, 1);
        let start = Instant::now();
        send(&mut app, &ctx, id, "first");
        tick(&mut app, &ctx, start);
        let lead = leads.borrow()[0].script.clone();
        lead.borrow_mut().ready("s");
        tick(&mut app, &ctx, start);
        assert_eq!(lead.borrow().turns.len(), 1);
        // News comes and a second message is written while the turn is
        // only handed over: neither is written into it.
        let run = app.projects.runs.get_mut(&id).unwrap();
        run.event(AgentEvent::new(7, None, What::Finished), start);
        send(&mut app, &ctx, id, "second");
        tick(&mut app, &ctx, start + Duration::from_secs(60));
        assert_eq!(lead.borrow().turns.len(), 1);
        // The lead ends without having begun it.
        lead.borrow_mut()
            .events
            .push(LeadEvent::Exited { code: Some(1) });
        lead.borrow_mut().state = Some(LeadState::Dead);
        tick(&mut app, &ctx, start + Duration::from_secs(60));
        assert!(lead.borrow().dropped);
        assert_eq!(leads.borrow().len(), 2, "a lead is started for what waits");
        assert_eq!(
            leads.borrow()[1].session,
            (Some("s".into()), true),
            "in the session it had"
        );
        assert_eq!(shown(&app).4, 2);
        let next = leads.borrow()[1].script.clone();
        next.borrow_mut().ready("s");
        tick(&mut app, &ctx, start + Duration::from_secs(60));
        let turns = next.borrow().turns.clone();
        assert_eq!(turns.len(), 1);
        // Everything that waited goes, once, and nothing is said twice.
        assert_eq!(turns[0].1.matches("\nfirst\n").count(), 1);
        assert_eq!(turns[0].1.matches("\nsecond\n").count(), 1);
        assert_eq!(turns[0].1.matches("[agent 7 finished its turn]").count(), 1);
        next.borrow_mut().begin();
        // A message written while the turn runs waits for its end.
        tick(&mut app, &ctx, start + Duration::from_secs(60));
        send(&mut app, &ctx, id, "third");
        for _ in 0..3 {
            tick(&mut app, &ctx, start + Duration::from_secs(120));
        }
        assert_eq!(next.borrow().turns.len(), 1);
        assert_eq!(shown(&app).4, 1);
        next.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start + Duration::from_secs(120));
        let turns = next.borrow().turns.clone();
        assert_eq!(turns.len(), 2);
        assert!(turns[1].1.ends_with("\nthird\n") && !turns[1].1.contains("first"));
        let said: Vec<Entry> = entries(&app, id)
            .into_iter()
            .filter(|entry| matches!(entry, Entry::User { .. } | Entry::Event { .. }))
            .collect();
        assert_eq!(said.len(), 4, "{said:?}");
    }

    #[test]
    fn the_keyboard_leaves_the_tabs_field_with_escape_and_with_the_tab() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, _leads) = application(dir.path());
        let space = workspace(&mut app, dir.path(), "shop");
        let id = project(&mut app, space, 1);
        tick(&mut app, &ctx, Instant::now());
        // Nothing of the tab holds the keyboard: Escape is the shell's.
        assert!(!app.project_escape(&ctx));
        // "Message the lead" shows the tab and puts the keyboard in it.
        app.action(&ctx, Action::Project(Event::Compose));
        assert!(app.ui.panel.open && app.ui.panel.tab == ui::panel::Tab::Project);
        assert!(app.ui.project.focus);
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert!(!app.ui.project.focus, "asked for once");
        let composer = ui::chat::composer_id();
        assert!(ctx.memory(|memory| memory.has_focus(composer)));
        app.ui.project.drafts.insert(id, "half a thought".into());
        assert!(app.project_escape(&ctx));
        assert!(!ctx.memory(|memory| memory.has_focus(composer)));
        assert_eq!(app.ui.project.drafts[&id], "half a thought");
        // Another tab, or the panel closing, takes the keyboard back too.
        app.action(&ctx, Action::Project(Event::Compose));
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert!(ctx.memory(|memory| memory.has_focus(composer)));
        app.action(
            &ctx,
            Action::Panel(ui::panel::Event::Show(ui::panel::Tab::Agents)),
        );
        assert!(!ctx.memory(|memory| memory.has_focus(composer)));
        assert_eq!(app.ui.project.drafts[&id], "half a thought");
        // A message too long for the lead stays in the field.
        let long = "x".repeat(MAX_MESSAGE + 1);
        send(&mut app, &ctx, id, &long);
        assert_eq!(app.ui.project.drafts[&id], long);
        assert!(app.ui.error.take().is_some());
        assert!(app.projects.runs[&id].chat.records().is_empty());
        // A capture run and an SSH workspace make no project.
        app.ephemeral = true;
        let other = workspace(&mut app, dir.path(), "other");
        app.action(
            &ctx,
            Action::Project(Event::Create {
                workspace: other,
                goal: "x".into(),
                lead: AgentKind::Claude,
                model: None,
                effort: None,
            }),
        );
        assert!(app.projects.creating.is_none() && app.ui.error.take().is_some());
        // Nor does a goal that could never be sent as a message.
        app.ephemeral = false;
        app.action(
            &ctx,
            Action::Project(Event::Create {
                workspace: other,
                goal: long,
                lead: AgentKind::Claude,
                model: None,
                effort: None,
            }),
        );
        assert!(app.projects.creating.is_none() && app.ui.error.take().is_some());
        assert_eq!(span(Duration::from_secs(7800)), "2 h 10 min");
        assert_eq!(span(Duration::from_secs(20)), "1 min");
        assert_eq!(span(Duration::ZERO), "less than a minute");
        assert_eq!(limit_end(Some(1), Instant::now()), None);
        assert_eq!(limit_end(None, Instant::now()), None);
    }

    const AGENT_SESSION: &str = "019a1234-5678-7000-8000-123456789abc";

    fn folder_of(root: &std::path::Path, key: u64) -> PathBuf {
        root.join("projects").join(format!("{key:016x}"))
    }
    /// Neptune closes: what waits is written, and everything it held goes.
    fn close(mut app: App) {
        app.projects.shutdown();
    }
    /// Neptune opens on what `root` holds, with the window as it was saved:
    /// one workspace with the project named by `key`.
    fn reopen(root: &std::path::Path, key: u64) -> (App, egui::Context, Leads, ProjectId) {
        let (mut app, ctx, leads) = application(root);
        let space = workspace(&mut app, root, "shop");
        let id = project(&mut app, space, key);
        (app, ctx, leads, id)
    }
    fn notices(app: &App, project: ProjectId, wanted: &str) -> usize {
        entries(app, project)
            .iter()
            .filter(|entry| matches!(entry, Entry::Notice { text } if text == wanted))
            .count()
    }
    fn spawn(app: &App, request: u64, project: ProjectId, task: Task, cwd: PathBuf) {
        app.sessions.agents().request(AgentRequest::Spawn {
            request,
            parent: Starter::Project(project),
            generation: 1,
            task,
            cwd,
        });
    }

    #[test]
    fn a_project_comes_back_after_a_restart_and_what_was_cut_short_is_said_once() {
        let dir = tempfile::tempdir().unwrap();
        let folder = folder_of(dir.path(), 1);
        let agent;
        {
            let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
            let start = Instant::now();
            send(&mut app, &ctx, id, "Split checkout");
            tick(&mut app, &ctx, start);
            let lead = leads.borrow()[0].script.clone();
            lead.borrow_mut().ready("session-1");
            tick(&mut app, &ctx, start);
            lead.borrow_mut().begin();
            lead.borrow_mut().say(1, "One agent for the auth part.");
            spawn(
                &app,
                1,
                id,
                Task {
                    title: Some("auth".into()),
                    ..Task::new(AgentKind::Codex, "PRIVATE BRIEF")
                },
                PathBuf::new(),
            );
            tick(&mut app, &ctx, start);
            let Some(Ok(pane)) = app.sessions.agents().spawned(1) else {
                panic!("no terminal was opened");
            };
            agent = pane;
            lead.borrow_mut().end(Outcome::Completed);
            tick(&mut app, &ctx, start);
            // Its CLI names its conversation, as its hooks do.
            app.controller
                .dispatch(Command::PaneAgentChanged {
                    pane,
                    generation: 1,
                    agent: Some(AgentSession {
                        kind: AgentKind::Codex,
                        session_id: Some(AGENT_SESSION.into()),
                        cwd: dir.path().into(),
                    }),
                })
                .unwrap();
            tick(&mut app, &ctx, start);
            // It says what it did and its terminal is closed: news the lead
            // has not heard when the user writes again.
            app.sessions.agents().replied(pane, "PR #214 is up.");
            app.controller.dispatch(Command::ClosePane(pane)).unwrap();
            let later = start + Duration::from_secs(60);
            tick(&mut app, &ctx, later);
            send(&mut app, &ctx, id, "second");
            tick(&mut app, &ctx, later);
            tick(&mut app, &ctx, later);
            assert_eq!(lead.borrow().turns.len(), 2);
            // The lead is in that turn, with words on their way, and the
            // user has written once more, when Neptune closes.
            lead.borrow_mut().begin();
            lead.borrow_mut().stream = Stream {
                revision: 9,
                block: 1,
                text: "STREAMED-ONLY".into(),
            };
            tick(&mut app, &ctx, later);
            send(&mut app, &ctx, id, "third");
            tick(&mut app, &ctx, later);
            close(app);
        }
        // Its folder holds the chat and what it knows of its agent; text
        // that only streamed, and what the agent was asked, are nowhere.
        let chat = std::fs::read_to_string(folder.join("chat.jsonl")).unwrap();
        assert!(chat.starts_with(&format!("{}\n", transcript::HEADER)));
        for said in [
            "Split checkout",
            "One agent for the auth part.",
            "PR #214 is up.",
            "third",
        ] {
            assert_eq!(chat.matches(said).count(), 1, "{said}");
        }
        let state = std::fs::read_to_string(folder.join("project.json")).unwrap();
        for private in ["STREAMED-ONLY", "PRIVATE BRIEF"] {
            assert!(
                !chat.contains(private) && !state.contains(private),
                "{private}"
            );
        }
        let saved: Saved = serde_json::from_str(&state).unwrap();
        assert_eq!(
            (saved.name.as_str(), saved.directory.as_path()),
            ("shop", dir.path())
        );
        assert_eq!(
            saved.lead,
            Some(LeadRef {
                kind: AgentKind::Claude,
                session: Some("session-1".into()),
                opened: true,
            })
        );
        let [task] = &saved.tasks[..] else {
            panic!("{:?}", saved.tasks);
        };
        assert_eq!(
            (task.n, task.title.as_str(), task.kind, task.state),
            (agent.get(), "auth", AgentKind::Codex, TaskState::Ended)
        );
        assert_eq!(task.session.as_deref(), Some(AGENT_SESSION));
        assert_eq!(task.last_report.as_deref(), Some("PR #214 is up."));
        assert!(task.ended.is_some() && task.cwd == dir.path());
        assert_eq!(saved.counters.spawned_today, 1);

        // Neptune opens again. No lead is started for it.
        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert!(leads.borrow().is_empty(), "no lead at launch");
        let all = entries(&app, id);
        assert!(all.contains(&Entry::User {
            text: "Split checkout".into(),
            attachments: Vec::new(),
        }));
        // The turn that was cut short is ended, and the person is told
        // of it and of the message nobody took, each once.
        let ended: Vec<&Entry> = all
            .iter()
            .filter(|entry| matches!(entry, Entry::End { .. }))
            .collect();
        assert!(matches!(
            ended[..],
            [
                Entry::End {
                    outcome: Ended::Completed,
                    ..
                },
                Entry::End {
                    outcome: Ended::Failed,
                    ..
                }
            ]
        ));
        assert_eq!(notices(&app, id, transcript::CLOSED_MID_TURN), 1);
        assert_eq!(notices(&app, id, transcript::CLOSED_UNSENT), 1);
        let (_, _, _, busy, queued) = shown(&app);
        assert!(!busy && queued == 0, "nothing is sent again by itself");
        // The agent whose terminal was closed is known again: by the name
        // its lead gave it, with its last reply, and as one to open again.
        let (known, _) = app.sessions.agents().collect_project(id, false);
        assert_eq!(known.len(), 1);
        assert_eq!(
            (
                known[0].agent,
                known[0].title.as_deref(),
                known[0].reopens,
                known[0].news
            ),
            (agent.get(), Some("auth"), true, false)
        );
        app.sessions.agents().request(AgentRequest::Project {
            request: 5,
            project: id,
            from: Starter::Project(id),
            call: crate::runtime::agents::ProjectCall::Report { agent: agent.get() },
        });
        tick(&mut app, &ctx, start);
        assert_eq!(
            app.sessions.agents().called(5),
            Some(Ok("PR #214 is up.".into()))
        );
        // What the lead never heard waits for it once more. Its quiet
        // moment over, the lead comes back in the conversation it had.
        tick(&mut app, &ctx, start + inbox::QUIET);
        tick(&mut app, &ctx, start + inbox::QUIET);
        assert_eq!(leads.borrow().len(), 1);
        assert_eq!(
            leads.borrow()[0].session,
            (Some("session-1".into()), true),
            "resumed, not opened anew"
        );
        let lead = leads.borrow()[0].script.clone();
        lead.borrow_mut().ready("session-1");
        tick(&mut app, &ctx, start + inbox::QUIET);
        let turns = lead.borrow().turns.clone();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].1.matches(transcript::RESTARTED).count(), 1);
        assert!(
            turns[0].1.contains(&format!("[agent {agent} “auth” ended"))
                && turns[0]
                    .1
                    .contains(&format!("{}]\n", transcript::RESTARTED)),
            "{}",
            turns[0].1
        );
        assert_eq!(turns[0].1.matches("PR #214 is up.").count(), 1);
        assert!(!turns[0].1.contains("second") && !turns[0].1.contains("third"));
        lead.borrow_mut().begin();
        lead.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start + inbox::QUIET);
        assert!(matches!(
            entries(&app, id).last(),
            Some(Entry::Delivered { .. })
        ));
        // Its lead opens the closed agent again: the project keeps one
        // record of it, under the number of its new terminal.
        spawn(
            &app,
            9,
            id,
            Task {
                title: Some("auth".into()),
                resume: Some((agent, AGENT_SESSION.into())),
                ..Task::new(AgentKind::Codex, "One more thing")
            },
            dir.path().into(),
        );
        tick(&mut app, &ctx, start + inbox::QUIET);
        let Some(Ok(again)) = app.sessions.agents().spawned(9) else {
            panic!("the agent was not opened again");
        };
        assert_eq!(
            entries(&app, id).last(),
            Some(&Entry::Tool {
                name: "reopen_agent".into(),
                summary: format!("Reopened agent {again} · Codex · auth"),
                ok: true,
            })
        );
        let tasks = app.projects.runs[&id].tasks.tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            (tasks[0].n, tasks[0].title.as_str(), tasks[0].ended),
            (again.get(), "auth", None)
        );
        assert_eq!(tasks[0].last_report.as_deref(), Some("PR #214 is up."));
        assert_eq!(
            app.projects.runs[&id].counters.spawned_today, 1,
            "opening again is not starting another"
        );
        assert_eq!(shown(&app).1, [format!("{again} auth · Starting")]);
        // A turn fails in the provider's own words: the chat quotes them.
        send(&mut app, &ctx, id, "fifth");
        tick(&mut app, &ctx, start + inbox::QUIET);
        lead.borrow_mut().begin();
        lead.borrow_mut()
            .end(Outcome::Failed(Failure::Provider("PROVIDER WORDS".into())));
        tick(&mut app, &ctx, start + inbox::QUIET);
        assert_eq!(
            notices(&app, id, "The lead's turn failed: PROVIDER WORDS"),
            1
        );
        // Nothing of the chat is in what is saved of the window, or in
        // what Neptune says aloud about failures.
        let window = serde_json::to_string(
            &crate::persistence::workspace_state::StateSnapshot::from_model(app.controller.model()),
        )
        .unwrap();
        let aloud = app.diagnostics.said.borrow().join("\n");
        assert_eq!(
            aloud,
            r#"{"error_kind":"Provider","generation":null,"operation":"failure","pane":null,"source_operation":"project_turn"}"#
        );
        for private in [
            "Split checkout",
            "PR #214",
            "session-1",
            "second",
            "third",
            "fifth",
            "PROVIDER",
            "One agent",
            AGENT_SESSION,
        ] {
            assert!(
                !window.replace(AGENT_SESSION, "").contains(private),
                "{private} in {window}"
            );
            assert!(!aloud.contains(private), "{private} in {aloud}");
        }
        close(app);

        // Opened a third time, nothing waits: what was said once is not
        // said again, and the lead is told nothing twice.
        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        for seconds in [0, 0, 2, 30, 600] {
            tick(&mut app, &ctx, start + Duration::from_secs(seconds));
        }
        assert!(leads.borrow().is_empty(), "nothing waits for the lead");
        assert_eq!(notices(&app, id, transcript::CLOSED_MID_TURN), 1);
        assert_eq!(notices(&app, id, transcript::CLOSED_UNSENT), 1);
        assert_eq!(
            entries(&app, id)
                .iter()
                .filter(|entry| matches!(entry, Entry::Event { .. }))
                .count(),
            1
        );
        // The next message goes to the lead in the conversation it had.
        send(&mut app, &ctx, id, "fourth");
        tick(&mut app, &ctx, start + Duration::from_secs(600));
        assert_eq!(leads.borrow()[0].session, (Some("session-1".into()), true));
    }

    #[test]
    fn what_a_failed_turn_carried_waits_for_the_lead_again_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let marks = |app: &App, id| {
            entries(app, id)
                .iter()
                .filter(|entry| matches!(entry, Entry::Delivered { .. }))
                .count()
        };
        {
            let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
            let start = Instant::now();
            send(&mut app, &ctx, id, "go");
            tick(&mut app, &ctx, start);
            let lead = leads.borrow()[0].script.clone();
            lead.borrow_mut().ready("session-1");
            tick(&mut app, &ctx, start);
            lead.borrow_mut().begin();
            lead.borrow_mut().end(Outcome::Completed);
            tick(&mut app, &ctx, start);
            // News of an agent begins a turn that the provider fails.
            let run = app.projects.runs.get_mut(&id).unwrap();
            let mut event = AgentEvent::new(7, Some("docs".into()), What::Finished);
            event.replies.push("Docs are done.".into());
            run.event(event, start);
            let due = start + inbox::QUIET;
            tick(&mut app, &ctx, due);
            assert_eq!(lead.borrow().turns.len(), 2);
            lead.borrow_mut().begin();
            lead.borrow_mut().end(Outcome::Failed(Failure::Overloaded));
            tick(&mut app, &ctx, due);
            assert!(!shown(&app).3);
            assert_eq!(marks(&app, id), 0, "not known to have reached the lead");
            close(app);
        }
        // Opened again, the lead hears it once, and then it is done with.
        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let due = start + inbox::QUIET;
        tick(&mut app, &ctx, due);
        tick(&mut app, &ctx, due);
        let lead = leads.borrow()[0].script.clone();
        lead.borrow_mut().ready("session-1");
        tick(&mut app, &ctx, due);
        let turns = lead.borrow().turns.clone();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].1.matches(transcript::RESTARTED).count(), 1);
        assert_eq!(turns[0].1.matches("Docs are done.").count(), 1);
        lead.borrow_mut().begin();
        lead.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, due);
        assert_eq!(marks(&app, id), 1);
        close(app);
        let (mut app, ctx, leads, _) = reopen(dir.path(), 1);
        let start = Instant::now();
        for seconds in [0, 0, 2, 30] {
            tick(&mut app, &ctx, start + Duration::from_secs(seconds));
        }
        assert!(leads.borrow().is_empty(), "nothing waits for the lead");
    }

    #[test]
    fn a_project_saved_by_a_newer_neptune_is_shown_and_nothing_of_it_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let folder = folder_of(dir.path(), 1);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("project.json"),
            r#"{"version":7,"plans":["from a later build"]}"#,
        )
        .unwrap();
        std::fs::write(
            folder.join("chat.jsonl"),
            format!(
                "{}\n{}\n",
                transcript::HEADER,
                r#"{"seq":1,"at":5,"kind":"lead","text":"Written by a later build."}"#
            ),
        )
        .unwrap();
        let before = |name: &str| std::fs::read(folder.join(name)).unwrap();
        let (state, chat) = (before("project.json"), before("chat.jsonl"));

        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        // It is shown as far as it was read, and says why it is left alone.
        assert_eq!(
            entries(&app, id),
            [Entry::Lead {
                text: "Written by a later build.".into()
            }]
        );
        let (needs, ..) = shown(&app);
        assert_eq!(needs.len(), 1);
        assert!(
            needs[0].starts_with("This project is read-only: It was saved by a newer Neptune."),
            "{needs:?}"
        );
        assert!(!needs[0].contains("  "), "{needs:?}");
        assert_eq!(app.project_needs(), 1);
        {
            let panel = app.project_panel(true);
            let Body::Project(view) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("no project in view");
            };
            assert!(view.locked);
        }
        // Its lead stays off whatever is asked of it.
        send(&mut app, &ctx, id, "hello?");
        assert_eq!(
            app.ui.project.drafts[&id], "hello?",
            "what was typed is kept"
        );
        assert!(app.ui.error.take().is_some());
        app.action(&ctx, Action::Project(Event::NewChat(id)));
        app.action(&ctx, Action::Project(Event::Retry(id)));
        app.action(&ctx, Action::Project(Event::SetName(id, "renamed".into())));
        for seconds in [0, 1, 30, 600] {
            tick(&mut app, &ctx, start + Duration::from_secs(seconds));
        }
        assert!(leads.borrow().is_empty());
        assert_eq!(entries(&app, id).len(), 1);
        frame(&mut app, &ctx);
        // Another project is read, led and written as ever.
        let other_space = workspace(&mut app, dir.path(), "other");
        let other = project(&mut app, other_space, 2);
        send(&mut app, &ctx, other, "hello");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(leads.borrow().len(), 1);
        assert!(
            std::fs::read_to_string(folder_of(dir.path(), 2).join("chat.jsonl"))
                .unwrap()
                .contains("hello")
        );
        assert!(folder_of(dir.path(), 2).join("project.json").exists());
        close(app);
        // Not a byte of the protected one changed, and nothing was put
        // beside it.
        assert_eq!(
            (before("project.json"), before("chat.jsonl")),
            (state, chat)
        );
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);
    }

    #[test]
    fn a_capture_run_writes_nothing_of_a_project_and_starts_no_lead() {
        let dir = tempfile::tempdir().unwrap();
        // The fixture is a run that saves nothing, with the worker a real
        // run has.
        let (mut app, _sender) = super::super::tests::fixture(dir.path());
        let ctx = egui::Context::default();
        app.startup = None;
        let leads = Leads::default();
        let started = leads.clone();
        app.projects.start = Box::new(move |launch, _| {
            let script = Rc::new(RefCell::new(Script::default()));
            started.borrow_mut().push(Started {
                kind: launch.kind,
                folder: launch.project_dir,
                session: (launch.session.id, launch.session.opened),
                prompt: launch.system_prompt,
                with: (launch.model, launch.effort),
                script: script.clone(),
            });
            Box::new(Fake(script))
        });
        let space = workspace(&mut app, dir.path(), "shop");
        let id = project(&mut app, space, 1);
        send(&mut app, &ctx, id, "hello");
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.projects.runs[&id].loading != Loading::Done {
            assert!(Instant::now() < deadline, "the project was never read");
            std::thread::sleep(Duration::from_millis(5));
            tick(&mut app, &ctx, Instant::now());
        }
        tick(&mut app, &ctx, Instant::now());
        app.action(&ctx, Action::Project(Event::NewChat(id)));
        app.action(&ctx, Action::Project(Event::Remove(id)));
        app.action(&ctx, Action::Project(Event::ConfirmRemove(id)));
        tick(&mut app, &ctx, Instant::now());
        assert!(leads.borrow().is_empty());
        close(app);
        assert!(!dir.path().join("projects").exists());
    }

    #[cfg_attr(windows, ignore = "its directories are written as on Unix")]
    #[test]
    fn a_new_chat_sets_the_old_one_aside_and_a_closed_workspaces_project_is_offered_again() {
        let dir = tempfile::tempdir().unwrap();
        let folder = folder_of(dir.path(), 1);
        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        send(&mut app, &ctx, id, "hello");
        tick(&mut app, &ctx, start);
        let lead = leads.borrow()[0].script.clone();
        lead.borrow_mut().ready("session-1");
        tick(&mut app, &ctx, start);
        lead.borrow_mut().begin();
        lead.borrow_mut().say(1, "Hi.");
        lead.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert!(!folder.join("chat.1.jsonl").exists());

        // A new chat: the lead is let go and the next one starts afresh;
        // the chat so far is the earlier one in the folder.
        app.action(&ctx, Action::Project(Event::NewChat(id)));
        assert!(lead.borrow().dropped);
        let [Entry::Notice { text }] = &entries(&app, id)[..] else {
            panic!("{:?}", entries(&app, id));
        };
        assert!(text.starts_with("New chat."));
        assert!(!text.contains("  "), "{text}");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let earlier = std::fs::read_to_string(folder.join("chat.1.jsonl")).unwrap();
        assert!(earlier.contains("hello") && earlier.contains("Hi."));
        let chat = std::fs::read_to_string(folder.join("chat.jsonl")).unwrap();
        assert_eq!(chat.lines().count(), 2);
        assert!(chat.contains("New chat.") && !chat.contains("hello"));
        // Asked for again before anything was said, nothing is set aside:
        // the earlier chat is not replaced by one that holds nothing.
        app.action(&ctx, Action::Project(Event::NewChat(id)));
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(entries(&app, id).len(), 1);
        assert_eq!(
            std::fs::read_to_string(folder.join("chat.1.jsonl")).unwrap(),
            earlier
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("chat.jsonl")).unwrap(),
            chat
        );
        send(&mut app, &ctx, id, "again");
        tick(&mut app, &ctx, start);
        assert_eq!(leads.borrow().len(), 2);
        assert_eq!(
            leads.borrow()[1].session,
            (None, false),
            "a new conversation"
        );
        let lead = leads.borrow()[1].script.clone();
        lead.borrow_mut().ready("session-2");
        tick(&mut app, &ctx, start);
        lead.borrow_mut().begin();
        lead.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start);

        // While the store's worker is behind, the lead is not listened to:
        // what it says waits with it, and nothing is dropped.
        let lines = project_store::MAX_JOBS + MAX_UNWRITTEN + 100;
        let run = app.projects.runs.get_mut(&id).unwrap();
        for n in 0..lines {
            run.notice(format!("line {n}"));
        }
        send(&mut app, &ctx, id, "while it saves");
        assert!(app.ui.error.take().is_some(), "said when it can be saved");
        assert_eq!(app.ui.project.drafts[&id], "while it saves");
        lead.borrow_mut().say(1, "Not yet heard.");
        tick(&mut app, &ctx, start);
        assert_eq!(lead.borrow().events.len(), 1, "left with the lead");
        tick(&mut app, &ctx, start);
        assert!(lead.borrow().events.is_empty());
        tick(&mut app, &ctx, start);
        let chat = std::fs::read_to_string(folder.join("chat.jsonl")).unwrap();
        assert_eq!(chat.matches("\"text\":\"line ").count(), lines);
        assert!(chat.find("line 0\"").unwrap() < chat.find("Not yet heard.").unwrap());
        close(app);

        // Opened again, the chat holds its newest entries and reads older
        // ones from the folder when they are asked for.
        let (mut app, ctx, _leads, id) = reopen(dir.path(), 1);
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let held = app.projects.runs[&id].chat.records().len();
        assert_eq!(held, transcript::MAX_ENTRIES);
        let more = |app: &App| {
            let panel = app.project_panel(true);
            let Body::Project(view) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("no project in view");
            };
            view.chat.more
        };
        assert!(more(&app));
        // What is held is shown first.
        app.ui.project.shown = ui::chat::PAGE * 2;
        app.action(&ctx, Action::Project(Event::Earlier(id)));
        tick(&mut app, &ctx, start);
        assert_eq!(app.projects.runs[&id].chat.records().len(), held);
        app.ui.project.shown = held + ui::chat::PAGE;
        app.action(&ctx, Action::Project(Event::Earlier(id)));
        assert!(!more(&app), "asked for once");
        tick(&mut app, &ctx, start);
        let run = &app.projects.runs[&id];
        assert_eq!(run.chat.records().len(), held + project_store::PAGE);
        assert_eq!(app.ui.project.shown, held + ui::chat::PAGE * 2);
        let numbers: Vec<u64> = run.chat.records().iter().map(|record| record.seq).collect();
        assert!(numbers.windows(2).all(|pair| pair[0] + 1 == pair[1]));
        assert!(more(&app));

        // Its workspace is closed: the project leaves the window and its
        // folder stays. Where it worked, it is offered again.
        let first = app.controller.model().active_workspace().unwrap();
        let second = workspace(&mut app, dir.path(), "elsewhere");
        let offered = |app: &App| match app.project_panel(true).body {
            PanelBody::Empty { reopen, .. } => reopen,
            _ => panic!("the workspace in view has a project"),
        };
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(offered(&app), None, "an open project is not offered");
        app.controller
            .dispatch(Command::CloseWorkspace(first))
            .unwrap();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert!(app.controller.model().projects().is_empty());
        assert!(folder.join("chat.jsonl").exists() && folder.join("project.json").exists());
        assert_eq!(offered(&app).as_deref(), Some("shop"));
        frame(&mut app, &ctx);
        app.action(&ctx, Action::Project(Event::Reopen(second)));
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        let item = app.controller.model().project_of(second).unwrap();
        assert_eq!(
            (item.name(), item.key().as_str(), item.lead()),
            ("shop", "0000000000000001", AgentKind::Claude)
        );
        let id = item.id();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(app.projects.runs[&id].chat.records().len(), held);
        assert_eq!(
            app.projects.runs[&id].session.id.as_deref(),
            Some("session-2")
        );
        assert_eq!(kept_first(&app), None);

        // Removed, its folder goes with everything in it.
        std::fs::create_dir_all(folder.join("context")).unwrap();
        std::fs::write(folder.join("context").join("STATUS.md"), "notes").unwrap();
        app.action(&ctx, Action::Project(Event::Remove(id)));
        app.action(&ctx, Action::Project(Event::ConfirmRemove(id)));
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert!(!folder.exists());
        assert_eq!(offered(&app), None);
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        close(app);
        assert!(!folder.exists(), "nothing wrote it back");
    }

    /// The first project that is offered again, whatever is in view.
    fn kept_first(app: &App) -> Option<String> {
        app.projects.kept.first().map(|kept| kept.name.clone())
    }

    // A lead's requests reach the bridge over a socket it opens on Unix.
    #[cfg(unix)]
    #[test]
    fn a_paused_refusal_says_why_and_a_project_starts_only_so_many_agents_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        send(&mut app, &ctx, id, "go");
        tick(&mut app, &ctx, start);
        let lead = leads.borrow()[0].script.clone();
        lead.borrow_mut().ready("s");
        tick(&mut app, &ctx, start);
        app.action(&ctx, Action::Project(Event::SetPaused(id, true)));
        tick(&mut app, &ctx, start);
        // The lead asks for an agent through its tools, and the bridge
        // refuses because the project is paused. A lead that is quick has
        // begun its turn and been refused within one frame.
        lead.borrow_mut().begin();
        let env = app
            .sessions
            .bridge()
            .open_lead(id, AgentKind::Claude, Arc::new(|| {}))
            .unwrap();
        let refused = app.sessions.agents().lead_spawns(&env, "auth");
        assert!(refused.is_some_and(|reason| reason.contains("paused")));
        for (call, tool) in [("c1", "spawn_agent"), ("c2", "list_agents")] {
            lead.borrow_mut().events.push(LeadEvent::ToolStarted {
                call: call.into(),
                tool: tool.into(),
                input: serde_json::json!({"title": "auth"}),
            });
            lead.borrow_mut().events.push(LeadEvent::ToolDone {
                call: call.into(),
                ok: false,
            });
        }
        tick(&mut app, &ctx, start);
        let cards: Vec<String> = entries(&app, id)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Tool {
                    summary, ok: false, ..
                } => Some(summary),
                _ => None,
            })
            .collect();
        assert_eq!(
            cards,
            [
                "The lead's spawn_agent request was refused: the project is paused",
                "The lead's list_agents request was refused"
            ]
        );
        lead.borrow_mut().end(Outcome::Completed);
        app.action(&ctx, Action::Project(Event::SetPaused(id, false)));
        tick(&mut app, &ctx, start);

        // A day's agents used up, another is refused in words for the
        // lead, and the chat says so.
        let run = app.projects.runs.get_mut(&id).unwrap();
        for _ in 0..crate::projects::registry::MAX_SPAWNS_A_DAY {
            run.counters.spawned(unix_now());
        }
        spawn(
            &app,
            3,
            id,
            Task {
                title: Some("one more".into()),
                ..Task::new(AgentKind::Claude, "task")
            },
            PathBuf::new(),
        );
        tick(&mut app, &ctx, start);
        assert!(matches!(
            app.sessions.agents().spawned(3),
            Some(Err(reason)) if reason.contains("40 agents today")
        ));
        assert!(matches!(
            entries(&app, id).last(),
            Some(Entry::Tool { summary, ok: false, .. })
                if summary.starts_with("Could not start an agent: This project has started 40")
        ));
        assert_eq!(app.controller.model().project_agents(id).count(), 0);
    }
    /// Asks something of a project's context as a tool does, and returns
    /// what the caller was answered once the store carried it out.
    fn call(
        app: &mut App,
        ctx: &egui::Context,
        request: u64,
        project: ProjectId,
        from: Starter,
        call: ProjectCall,
    ) -> Result<String, String> {
        app.sessions.agents().request(AgentRequest::Project {
            request,
            project,
            from,
            call,
        });
        // Asked on one frame, answered on the next: the file work is the
        // store's, not the frame's.
        tick(app, ctx, Instant::now());
        tick(app, ctx, Instant::now());
        app.sessions
            .agents()
            .called(request)
            .expect("the call was never answered")
    }
    fn write(path: &str, content: &str, append: Option<bool>) -> ProjectCall {
        ProjectCall::WriteContext {
            path: path.into(),
            content: content.into(),
            append,
        }
    }
    fn read(path: Option<&str>) -> ProjectCall {
        ProjectCall::ReadContext {
            path: path.map(str::to_owned),
            offset: 0,
        }
    }
    fn decide(decision: &str, why: Option<&str>) -> ProjectCall {
        ProjectCall::RecordDecision {
            decision: decision.into(),
            why: why.map(str::to_owned),
        }
    }

    #[test]
    fn what_a_project_keeps_is_written_by_its_rules_and_read_by_its_lead_and_its_agents() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, _leads) = application(dir.path());
        let shell_workspace = workspace(&mut app, dir.path(), "shop");
        let shell = app.controller.model().active_pane().unwrap();
        let id = project(&mut app, shell_workspace, 7);
        let lead = Starter::Project(id);
        let kept = folder_of(dir.path(), 7).join("context");
        let text = |name: &str| std::fs::read_to_string(kept.join(name)).unwrap_or_default();
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        // Looking at a project that keeps nothing makes no folder for it.
        assert!(app.projects.runs[&id].read.is_some());
        assert!(!kept.exists());
        assert_eq!(
            call(&mut app, &ctx, 1, id, lead, read(None)),
            Ok(context::index(&[]))
        );
        assert!(!kept.exists());

        // The lead records what was approved. Each entry says who and when.
        assert_eq!(
            call(
                &mut app,
                &ctx,
                2,
                id,
                lead,
                decide("Use JWT.", Some("Sessions do not scale."))
            ),
            Ok("Recorded in DECISIONS.md.".into())
        );
        assert_eq!(
            call(
                &mut app,
                &ctx,
                3,
                id,
                lead,
                decide("Ship behind a flag.", None)
            ),
            Ok("Recorded in DECISIONS.md.".into())
        );
        let decided = text("DECISIONS.md");
        assert!(decided.starts_with(context::DECISIONS_HEAD));
        let entries_of = context::decisions(&decided);
        assert_eq!(entries_of.len(), 2);
        assert!(
            entries_of[0].at < entries_of[1].at,
            "each has a time of its own"
        );
        assert!(
            entries_of[0]
                .text
                .ends_with("UTC · lead\n\nUse JWT.\n\nWhy: Sessions do not scale.")
        );
        // The chat says what Neptune wrote, in the lead's own turn or not.
        assert!(entries(&app, id).contains(&Entry::Tool {
            name: "record_decision".into(),
            summary: "Recorded a decision: Use JWT.".into(),
            ok: true,
        }));

        // The status is the lead's to replace, as often as the picture
        // changes; the one before it is kept, unless it stood for less than
        // two minutes. It can be added to.
        let status = "# Status\n\nGoal: split checkout.";
        assert_eq!(
            call(
                &mut app,
                &ctx,
                4,
                id,
                lead,
                write("STATUS.md", status, None)
            ),
            Ok("Wrote STATUS.md.".into())
        );
        assert_eq!(
            call(
                &mut app,
                &ctx,
                5,
                id,
                lead,
                write("status.md", "Draft", None)
            ),
            Ok("Replaced STATUS.md. The one before it is kept as STATUS.prev.md.".into())
        );
        // A lead that hears from two agents in a minute is not refused.
        assert_eq!(
            call(
                &mut app,
                &ctx,
                50,
                id,
                lead,
                write("STATUS.md", status, None)
            ),
            Ok("Replaced STATUS.md.".into())
        );
        assert_eq!(text("STATUS.prev.md"), format!("{status}\n"));
        assert_eq!(text("STATUS.md"), format!("{status}\n"));
        assert_eq!(
            call(
                &mut app,
                &ctx,
                6,
                id,
                lead,
                write("STATUS.md", "Next: API tests.", Some(true))
            ),
            Ok("Added to STATUS.md.".into())
        );
        assert_eq!(text("STATUS.md"), format!("{status}\n\nNext: API tests.\n"));
        let old = std::time::SystemTime::now() - Duration::from_secs(600);
        std::fs::File::options()
            .write(true)
            .open(kept.join("STATUS.md"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(
            call(
                &mut app,
                &ctx,
                7,
                id,
                lead,
                write("STATUS.md", "Goal: ship.", None)
            ),
            Ok("Replaced STATUS.md. The one before it is kept as STATUS.prev.md.".into())
        );
        assert_eq!(text("STATUS.md"), "Goal: ship.\n");
        assert_eq!(
            text("STATUS.prev.md"),
            format!("{status}\n\nNext: API tests.\n")
        );

        // Nothing else is written through a tool, and nothing outside the
        // folder is named by a path.
        let mut request = 10;
        for (path, why) in [
            ("INSTRUCTIONS.md", "Only the user writes INSTRUCTIONS.md"),
            ("DECISIONS.md", "record_decision"),
            ("INDEX.md", "Neptune writes INDEX.md"),
            ("decisions-archive.md", "not written through Neptune"),
            ("STATUS.prev.md", "not written through Neptune"),
            ("../project.json", "not a file of the project's context"),
            ("/etc/passwd", "not a file of the project's context"),
            (
                "notes/../../chat.jsonl",
                "not a file of the project's context",
            ),
            ("chat.jsonl", "not a file of the project's context"),
        ] {
            for append in [None, Some(true), Some(false)] {
                request += 1;
                let refused = call(&mut app, &ctx, request, id, lead, write(path, "x", append));
                assert!(
                    refused.as_ref().is_err_and(|reason| reason.contains(why)),
                    "{path} {append:?}: {refused:?}"
                );
            }
            request += 1;
            if path.contains("..") || path.starts_with('/') || path.ends_with("jsonl") {
                assert!(
                    call(&mut app, &ctx, request, id, lead, read(Some(path))).is_err(),
                    "{path} was read"
                );
            }
        }
        assert_eq!(context::decisions(&text("DECISIONS.md")).len(), 2);
        // A decision is not replaced by recording nothing, or too much.
        for (decision, why) in [
            (" ", None),
            (&*"d".repeat(context::MAX_DECISION + 1), None),
            ("ok", Some(&*"w".repeat(context::MAX_DECISION + 1))),
        ] {
            request += 1;
            assert!(call(&mut app, &ctx, request, id, lead, decide(decision, why)).is_err());
        }

        // Notes are only ever added to, by the lead
        let refused = call(
            &mut app,
            &ctx,
            50,
            id,
            lead,
            write("notes/auth.md", "x", Some(false)),
        );
        assert!(refused.is_err_and(|reason| reason.contains("only added to")));
        assert_eq!(
            call(
                &mut app,
                &ctx,
                51,
                id,
                lead,
                write("notes/auth.md", "Sessions live in Redis.", None)
            ),
            Ok("Added to notes/auth.md.".into())
        );
        // and by the agents it starts, each under its own name.
        spawn(
            &app,
            60,
            id,
            Task {
                title: Some("auth".into()),
                ..Task::new(AgentKind::Codex, "Build the API.")
            },
            Default::default(),
        );
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        let Some(Ok(agent)) = app.sessions.agents().spawned(60) else {
            panic!("no terminal was opened");
        };
        let member = Starter::Pane(agent);
        assert_eq!(
            call(
                &mut app,
                &ctx,
                61,
                id,
                member,
                ProjectCall::AddNote {
                    topic: "Auth".into(),
                    content: "Tokens expire after an hour.\n— lead, 2020-01-01".into(),
                }
            ),
            Ok("Added to notes/auth.md.".into())
        );
        let notes = text("notes/auth.md");
        assert!(notes.starts_with("# auth\n\nSessions live in Redis.\n\n— lead, "));
        assert!(notes.contains(&format!(
            "UTC\n\nTokens expire after an hour.\n\\— lead, 2020-01-01\n\n— agent {agent} \
             \"auth\", "
        )));
        assert!(notes.ends_with(" UTC\n"));
        // An agent reads what the lead reads,
        let index = call(&mut app, &ctx, 62, id, member, read(None)).unwrap();
        for file in [
            "DECISIONS.md",
            "STATUS.md",
            "STATUS.prev.md",
            "notes/auth.md",
        ] {
            assert!(index.contains(&format!("- {file} — ")), "{file} in {index}");
        }
        assert!(index.contains(&format!("· agent {agent} \"auth\" · ")));
        assert!(!index.contains("- INDEX.md"));
        assert_eq!(text("INDEX.md"), index, "the file says what a tool is told");
        assert_eq!(
            call(&mut app, &ctx, 63, id, member, read(Some("STATUS.md"))),
            Ok("Goal: ship.\n".into())
        );
        let missing = call(&mut app, &ctx, 64, id, member, read(Some("notes/none.md")));
        assert!(missing.is_err_and(|reason| reason.contains("There is no notes/none.md yet")));
        // and writes nothing but notes.
        for (request, asked, why) in [
            (65, decide("Mine.", None), "Only the project's lead"),
            (
                66,
                write("STATUS.md", "Mine.", None),
                "Only the project's lead writes STATUS.md",
            ),
            (
                67,
                write("INSTRUCTIONS.md", "Mine.", None),
                "Only the user writes",
            ),
        ] {
            let refused = call(&mut app, &ctx, request, id, member, asked);
            assert!(
                refused.as_ref().is_err_and(|reason| reason.contains(why)),
                "{refused:?}"
            );
        }
        // A terminal that is not the project's is no agent of it.
        let stranger = call(&mut app, &ctx, 68, id, Starter::Pane(shell), read(None));
        assert!(stranger.is_err_and(|reason| reason.contains("does not belong")));
        assert_eq!(context::decisions(&text("DECISIONS.md")).len(), 2);
        assert_eq!(text("STATUS.md"), "Goal: ship.\n");

        // Everything is the user's own, in files only they can read.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &std::path::Path| {
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777
            };
            assert_eq!(mode(&kept), 0o700);
            assert_eq!(mode(&kept.join("notes")), 0o700);
            for file in ["DECISIONS.md", "STATUS.md", "INDEX.md", "notes/auth.md"] {
                assert_eq!(mode(&kept.join(file)), 0o600, "{file}");
            }
        }
        // None of it is in the saved window or said in diagnostics.
        let saved = serde_json::to_string(
            &crate::persistence::workspace_state::StateSnapshot::from_model(app.controller.model()),
        )
        .unwrap();
        let said = app.diagnostics.said.borrow().join("\n");
        for private in ["JWT", "Redis", "Tokens expire", "Goal: ship", "notes/auth"] {
            assert!(
                !saved.contains(private) && !said.contains(private),
                "{private}"
            );
        }
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
    }

    #[test]
    fn an_agent_is_briefed_from_what_the_project_keeps_and_its_helpers_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, _leads) = application(dir.path());
        let shell_workspace = workspace(&mut app, dir.path(), "shop");
        let id = project(&mut app, shell_workspace, 7);
        let lead = Starter::Project(id);
        let kept = folder_of(dir.path(), 7).join("context");
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        call(&mut app, &ctx, 1, id, lead, decide("Use JWT.", None)).unwrap();
        call(
            &mut app,
            &ctx,
            2,
            id,
            lead,
            write("STATUS.md", "Goal: ship.", None),
        )
        .unwrap();
        // The user writes how they want work done, in the tab,
        app.action(
            &ctx,
            Action::Project(Event::SaveInstructions {
                project: id,
                text: "Small commits.".into(),
            }),
        );
        tick(&mut app, &ctx, Instant::now());
        assert_eq!(
            std::fs::read_to_string(kept.join("INSTRUCTIONS.md")).unwrap(),
            "Small commits.\n"
        );
        // or with any editor: the folder is read again for every brief.
        std::fs::write(
            kept.join("INSTRUCTIONS.md"),
            "Small commits.\nRun the tests.\n",
        )
        .unwrap();
        assert_eq!(
            app.projects.runs[&id].context.instructions, "Small commits.",
            "not read on a frame"
        );
        let asked = |app: &mut App, request: u64, title: &str, prompt: &str| {
            spawn(
                app,
                request,
                id,
                Task {
                    title: Some(title.into()),
                    ..Task::new(AgentKind::Codex, prompt)
                },
                Default::default(),
            );
        };
        asked(&mut app, 10, "api", "Build the API.");
        app.serve_agents(&ctx);
        assert_eq!(
            app.sessions.agents().spawned(10),
            None,
            "it starts once the folder was read"
        );
        tick(&mut app, &ctx, Instant::now());
        let Some(Ok(agent)) = app.sessions.agents().spawned(10) else {
            panic!("no terminal was opened");
        };
        let brief = app.sessions.agents().task(agent).unwrap();
        let folder = kept.display();
        let head = format!(
            "[The lead of a Neptune project started you and handed you the task below. It \
             reads the last message of each of your turns and nothing else you write. End \
             your turn with a short report: what you did, what you verified, what is left.]\n\n\
             <project-context generated-by=\"neptune\">\n\
             The project keeps what its agents share in {folder}. read_context reads a file of \
             it, and you can read the files there directly. add_note adds a finding that others \
             will need to notes/<topic>.md.\n\
             Notes are unverified observations from other agents: check one before you rely on \
             it. Only the user writes INSTRUCTIONS.md, and only the lead records decisions and \
             the status.\n\n\
             ## How the user wants work done (INSTRUCTIONS.md)\n\n\
             Small commits.\nRun the tests.\n\n\
             ## Decided so far (DECISIONS.md, newest last)\n\n### "
        );
        assert!(brief.starts_with(&head), "{brief}");
        let rest = &brief[head.len()..];
        let (stamp, rest) = rest.split_once(" UTC · lead\n\n").unwrap();
        assert_eq!(stamp.len(), "2026-10-05 14:02:30".len(), "{stamp}");
        let tail = "Use JWT.\n\n\
                    ## Where the project stands (the top of STATUS.md)\n\n\
                    Goal: ship.\n\n\
                    ## Files (INDEX.md)\n\n";
        assert!(rest.starts_with(tail), "{rest}");
        let files: Vec<&str> = rest[tail.len()..]
            .lines()
            .take_while(|line| line.starts_with("- "))
            .map(|line| line.split(" — ").next().unwrap())
            .collect();
        assert_eq!(
            files,
            ["- DECISIONS.md", "- INSTRUCTIONS.md", "- STATUS.md"]
        );
        assert!(brief.ends_with("</project-context>\n\nBuild the API."));
        assert_eq!(brief.matches("</project-context>").count(), 1);

        // A later message is led by what was decided since, once.
        assert_eq!(
            app.projects.told(id, agent, "Also add tests.".into()),
            "Also add tests."
        );
        call(
            &mut app,
            &ctx,
            20,
            id,
            lead,
            decide("Drop IE.\nFor good.", None),
        )
        .unwrap();
        assert_eq!(
            app.projects.told(id, agent, "Also add tests.".into()),
            "[The project decided since you were last told: 1) Drop IE.]\n\nAlso add tests."
        );
        assert_eq!(
            app.projects.told(id, agent, "And docs.".into()),
            "And docs."
        );
        // An agent started now is handed it in its brief instead.
        asked(&mut app, 21, "docs", "Write the docs.");
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        let Some(Ok(second)) = app.sessions.agents().spawned(21) else {
            panic!("no terminal was opened");
        };
        let brief = app.sessions.agents().task(second).unwrap();
        assert!(brief.contains("Use JWT.") && brief.contains("Drop IE.\nFor good."));
        assert_eq!(
            app.projects.told(id, second, "Go on.".into()),
            "Go on.",
            "nothing was decided since it was briefed"
        );

        // What one of the project's agents starts by itself is handed its
        // task and nothing of the project.
        let generation = app.controller.model().pane(agent).unwrap().generation();
        app.controller
            .dispatch(Command::PaneAgentChanged {
                pane: agent,
                generation,
                agent: Some(AgentSession {
                    kind: AgentKind::Codex,
                    session_id: None,
                    cwd: dir.path().into(),
                }),
            })
            .unwrap();
        app.sessions.agents().request(AgentRequest::Spawn {
            request: 30,
            parent: Starter::Pane(agent),
            generation,
            task: Task::new(AgentKind::Claude, "HELPER TASK"),
            cwd: dir.path().into(),
        });
        app.serve_agents(&ctx);
        let Some(Ok(helper)) = app.sessions.agents().spawned(30) else {
            panic!("no terminal was opened for the helper");
        };
        assert_eq!(
            app.sessions.agents().task(helper).as_deref(),
            Some("HELPER TASK")
        );
        assert_eq!(app.controller.model().pane(helper).unwrap().project(), None);
        // It is no agent of the project, and neither reads nor notes.
        let refused = call(&mut app, &ctx, 31, id, Starter::Pane(helper), read(None));
        assert!(refused.is_err_and(|reason| reason.contains("does not belong")));
        let refused = call(
            &mut app,
            &ctx,
            32,
            id,
            Starter::Pane(helper),
            ProjectCall::AddNote {
                topic: "x".into(),
                content: "y".into(),
            },
        );
        assert!(refused.is_err());
        assert!(!kept.join("notes").exists());
    }

    #[test]
    fn a_lead_is_handed_all_a_project_keeps_once_and_then_what_changed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads) = application(dir.path());
        let shell_workspace = workspace(&mut app, dir.path(), "shop");
        let id = project(&mut app, shell_workspace, 7);
        let lead = Starter::Project(id);
        let kept = folder_of(dir.path(), 7).join("context");
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        call(&mut app, &ctx, 1, id, lead, decide("Use JWT.", None)).unwrap();
        std::fs::write(kept.join("INSTRUCTIONS.md"), "Small commits.\n").unwrap();
        // One turn: said, begun, ended. Returns what the lead was handed.
        let turn = |app: &mut App, words: &str| -> String {
            send(app, &ctx, id, words);
            for _ in 0..3 {
                tick(app, &ctx, Instant::now());
            }
            let script = leads.borrow().last().unwrap().script.clone();
            if script.borrow().state.is_none() {
                script.borrow_mut().ready("session-1");
                tick(app, &ctx, Instant::now());
                tick(app, &ctx, Instant::now());
            }
            let text = script.borrow().turns.last().unwrap().1.clone();
            assert!(text.ends_with(&format!("\n{words}\n")), "{text}");
            script.borrow_mut().begin();
            script.borrow_mut().end(Outcome::Completed);
            tick(app, &ctx, Instant::now());
            tick(app, &ctx, Instant::now());
            text
        };
        let state_of = |text: &str| -> String {
            let (_, rest) = text.split_once("\nagents: none\n").unwrap();
            rest.split_once("</project-state>").unwrap().0.to_owned()
        };

        // A new conversation: the user's instructions, the decisions, the
        // index. The folder was read again as the message was sent.
        let first = state_of(&turn(&mut app, "Plan it."));
        assert!(
            first.starts_with(
                "instructions (INSTRUCTIONS.md, written by the user: how they want work \
                 done):\nSmall commits.\ndecisions (newest last):\n## "
            ),
            "{first}"
        );
        assert!(first.contains(" UTC · lead\n\nUse JWT.\ncontext index:\n- DECISIONS.md — "));
        assert!(first.contains("\n- INSTRUCTIONS.md — Small commits. · "));
        assert!(!first.contains("status ("), "it has written none yet");
        // Nothing changed: nothing is said again.
        assert_eq!(state_of(&turn(&mut app, "And then?")), "");

        // What was decided since, and the index once its files changed.
        call(&mut app, &ctx, 2, id, lead, decide("Drop IE.", None)).unwrap();
        let third = state_of(&turn(&mut app, "Go on."));
        assert!(third.starts_with("new decisions:\n## "), "{third}");
        assert!(third.contains(" UTC · lead\n\nDrop IE.\ncontext index:\n- DECISIONS.md"));
        assert!(!third.contains("Use JWT.") && !third.contains("instructions"));
        assert_eq!(state_of(&turn(&mut app, "Still there?")), "");
        // The user changes their instructions by hand: the lead hears it.
        std::fs::write(kept.join("INSTRUCTIONS.md"), "Tabs, not spaces.\n").unwrap();
        let fifth = state_of(&turn(&mut app, "Look."));
        assert!(
            fifth.starts_with("instructions changed (INSTRUCTIONS.md, written by the user"),
            "{fifth}"
        );
        assert!(fifth.contains("\nTabs, not spaces.\n") && !fifth.contains("decisions"));

        // Its CLI summarised the conversation: all of it once more.
        let script = leads.borrow().last().unwrap().script.clone();
        send(&mut app, &ctx, id, "Long talk.");
        for _ in 0..3 {
            tick(&mut app, &ctx, Instant::now());
        }
        {
            let mut script = script.borrow_mut();
            script.begin();
            script.events.push(LeadEvent::Compacted);
            script.end(Outcome::Completed);
        }
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        let after = state_of(&turn(&mut app, "Where were we?"));
        assert!(
            after.starts_with("instructions (INSTRUCTIONS.md"),
            "{after}"
        );
        assert!(after.contains("Use JWT.") && after.contains("Drop IE."));
        assert!(after.contains("context index:\n"));
        assert_eq!(state_of(&turn(&mut app, "Fine.")), "");

        // A folder that is being read is waited for, so that the turn says
        // what it holds now; a moment at most, and then it goes without.
        send(&mut app, &ctx, id, "One more.");
        let at = Instant::now();
        let handed = script.borrow().turns.len();
        {
            let run = app.projects.runs.get_mut(&id).unwrap();
            run.stale = false;
            run.looking = Some(at);
        }
        tick(&mut app, &ctx, at);
        tick(&mut app, &ctx, at + LOOK_WAIT - Duration::from_millis(1));
        assert_eq!(script.borrow().turns.len(), handed);
        tick(&mut app, &ctx, at + LOOK_WAIT);
        assert_eq!(script.borrow().turns.len(), handed + 1);
        {
            let mut script = script.borrow_mut();
            script.begin();
            script.end(Outcome::Completed);
        }
        app.projects.runs.get_mut(&id).unwrap().looking = None;
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());

        // A new chat is a lead that knows nothing: all of it again.
        app.action(&ctx, Action::Project(Event::NewChat(id)));
        tick(&mut app, &ctx, Instant::now());
        let fresh = state_of(&turn(&mut app, "Hello again."));
        assert!(
            fresh.starts_with("instructions (INSTRUCTIONS.md"),
            "{fresh}"
        );
        assert!(fresh.contains("decisions (newest last):") && fresh.contains("Drop IE."));
        assert_eq!(leads.borrow().len(), 2);
    }

    #[test]
    fn the_tab_lists_what_a_project_keeps_and_reads_it_again_only_while_in_view() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, _leads) = application(dir.path());
        let shell_workspace = workspace(&mut app, dir.path(), "shop");
        let id = project(&mut app, shell_workspace, 7);
        let lead = Starter::Project(id);
        let kept = folder_of(dir.path(), 7).join("context");
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        call(&mut app, &ctx, 1, id, lead, decide("Use JWT.", None)).unwrap();
        call(
            &mut app,
            &ctx,
            2,
            id,
            lead,
            write("notes/auth.md", "Seen.", None),
        )
        .unwrap();
        let listed = |app: &App| -> Vec<(String, String, bool)> {
            let panel = app.project_panel(true);
            let Body::Project(view) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("the workspace in view has no project");
            };
            assert!(view.context.read);
            view.context
                .files
                .iter()
                .map(|file| (file.name.clone(), file.author.clone(), file.removable))
                .collect()
        };
        let mut files = listed(&app);
        files.sort();
        assert_eq!(
            files,
            [
                ("DECISIONS.md".to_owned(), "lead".to_owned(), true),
                ("INDEX.md".to_owned(), "Neptune".to_owned(), false),
                ("notes/auth.md".to_owned(), "lead".to_owned(), true),
            ]
        );
        // The longest untouched comes first.
        let old = std::time::SystemTime::now() - Duration::from_secs(3 * 3600);
        std::fs::File::options()
            .write(true)
            .open(kept.join("notes/auth.md"))
            .unwrap()
            .set_modified(old)
            .unwrap();

        // Out of view, a change made by hand is not looked for.
        let read_at = |app: &App| app.projects.runs[&id].read.unwrap();
        // A frame as the window has one: what is ready is taken, then drawn.
        let draw = |app: &mut App, frames: usize| {
            for _ in 0..frames {
                tick(app, &ctx, Instant::now());
                frame(app, &ctx);
            }
        };
        let before = read_at(&app);
        std::thread::sleep(Duration::from_millis(5));
        draw(&mut app, 3);
        assert_eq!(read_at(&app), before, "nothing looks at a list nobody sees");
        // In view, it is read when it comes into view and every two
        // seconds after that.
        app.action(
            &ctx,
            Action::Panel(ui::panel::Event::Show(ui::panel::Tab::Project)),
        );
        draw(&mut app, 3);
        assert_eq!(read_at(&app), before, "the chat is in view, not the list");
        app.ui.project.segment = ui::project::Segment::Context;
        app.projects.runs.get_mut(&id).unwrap().read = Some(before - LOOK_EVERY);
        draw(&mut app, 3);
        let shown_at = read_at(&app);
        assert!(shown_at > before);
        assert_eq!(listed(&app)[0].0, "notes/auth.md");
        draw(&mut app, 3);
        assert_eq!(read_at(&app), shown_at, "not again within two seconds");
        std::fs::write(kept.join("mine.md"), "# Mine\n").unwrap();
        app.projects.runs.get_mut(&id).unwrap().read = Some(shown_at - LOOK_EVERY);
        draw(&mut app, 3);
        assert!(
            listed(&app)
                .iter()
                .any(|(name, author, _)| name == "mine.md" && author.is_empty())
        );
        assert!(
            std::fs::read_to_string(kept.join("INDEX.md"))
                .unwrap()
                .contains("- mine.md — Mine · ")
        );

        // The person deletes a file; Neptune's own index comes back.
        for name in ["notes/auth.md", "INDEX.md", "../project.json"] {
            app.action(&ctx, Action::Project(Event::DeleteFile(id, name.into())));
        }
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        assert!(!kept.join("notes/auth.md").exists());
        assert!(kept.join("INDEX.md").exists());
        assert!(folder_of(dir.path(), 7).join("project.json").exists());
        assert!(
            !listed(&app)
                .iter()
                .any(|(name, ..)| name == "notes/auth.md")
        );
        // Instructions too long to keep are refused where they are written.
        app.action(
            &ctx,
            Action::Project(Event::SaveInstructions {
                project: id,
                text: "x".repeat(context::MAX_INSTRUCTIONS),
            }),
        );
        assert!(
            app.ui
                .error
                .take()
                .is_some_and(|said| said.contains("at most 16 KB"))
        );
        assert!(!kept.join("INSTRUCTIONS.md").exists());
        // Emptied, the file goes.
        for text in ["Small commits.", " "] {
            app.action(
                &ctx,
                Action::Project(Event::SaveInstructions {
                    project: id,
                    text: text.into(),
                }),
            );
            tick(&mut app, &ctx, Instant::now());
            tick(&mut app, &ctx, Instant::now());
            assert_eq!(
                kept.join("INSTRUCTIONS.md").exists(),
                !text.trim().is_empty()
            );
        }
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
    }
    const PULL: &str = "https://github.com/zevem/neptune/pull/83";

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }
    fn watches(app: &App, project: ProjectId) -> &[Subscription] {
        &app.projects.runs[&project].subscriptions
    }
    fn add_watch(
        app: &mut App,
        ctx: &egui::Context,
        project: ProjectId,
        title: &str,
        trigger: WatchAsk,
        instruction: &str,
    ) {
        app.action(
            ctx,
            Action::Project(Event::AddWatch {
                project,
                title: title.into(),
                trigger,
                instruction: instruction.into(),
            }),
        );
    }
    /// The rows of the chat that watches wrote, as `(source, watch, what)`.
    fn fires(app: &App, project: ProjectId) -> Vec<(Source, Option<u64>, String)> {
        entries(app, project)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Event {
                    source,
                    agent,
                    what,
                    ..
                } if source != Source::Agent => Some((source, agent, what)),
                _ => None,
            })
            .collect()
    }
    /// The clock says watch `id` was due at `due`, and it is `now`.
    fn clock(app: &mut App, ctx: &egui::Context, project: ProjectId, id: u64, due: u64, now: u64) {
        let key = app.projects.runs[&project].key.clone();
        app.watch_fired(ctx, &key, id, due, at(now), Instant::now());
    }
    /// Lets the lead take the turn that waits and end it saying `said`.
    fn lead_turn(app: &mut App, ctx: &egui::Context, leads: &Leads, said: &str) -> String {
        // Each turn is later than what the one before left waiting.
        static TURNS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(2);
        let turns = TURNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let now = Instant::now() + inbox::MAX_WAIT * turns;
        tick(app, ctx, now);
        let lead = leads.borrow().last().unwrap().script.clone();
        if lead.borrow().state.is_none() {
            lead.borrow_mut().ready("session-1");
            tick(app, ctx, now);
        }
        tick(app, ctx, now);
        lead.borrow_mut().begin();
        tick(app, ctx, now);
        let text = lead.borrow().turns.last().unwrap().1.clone();
        lead.borrow_mut().say(1, said);
        lead.borrow_mut().end(Outcome::Completed);
        tick(app, ctx, now);
        text
    }

    #[test]
    fn a_schedule_fires_once_for_each_run_never_early_and_never_on_top_of_itself() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let began = schedule::seconds(SystemTime::now());
        add_watch(
            &mut app,
            &ctx,
            id,
            " Nightly ",
            WatchAsk::Every(60),
            " Check the build. ",
        );
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        // What the person adds runs without being allowed again, a whole
        // beat from now.
        let watch = watches(&app, id)[0].clone();
        let due = watch.next.unwrap();
        assert_eq!(
            (watch.id, watch.title.as_str(), watch.allowed, watch.paused),
            (1, "Nightly", true, false)
        );
        assert!((began + 3600..=began + 3605).contains(&due));
        // Its row says how long until then, so the list in view is drawn
        // again as the minutes pass; a paused one has no time to say.
        assert!(app.watches_age());
        app.action(&ctx, Action::Project(Event::PauseWatch(id, 1, true)));
        assert!(!app.watches_age());
        app.action(&ctx, Action::Project(Event::PauseWatch(id, 1, false)));
        assert!(app.watches_age());
        tick(&mut app, &ctx, start);
        // The store's clock is told when it is due, and nothing else.
        assert_eq!(app.projects.runs[&id].armed, [(1, due)]);
        assert!(fires(&app, id).is_empty());

        // Never early, and only for the run it waits for.
        clock(&mut app, &ctx, id, 1, due, due - 1);
        clock(&mut app, &ctx, id, 1, due - 60, due);
        clock(&mut app, &ctx, id, 9, due, due);
        assert!(fires(&app, id).is_empty());
        assert_eq!(watches(&app, id)[0].next, Some(due));

        // On time it fires: a row of the chat, and words for the lead.
        clock(&mut app, &ctx, id, 1, due, due + 2);
        let row = format!(
            "Watch “Nightly” fired · schedule · due {}",
            schedule::clock(due)
        );
        assert_eq!(
            fires(&app, id),
            [(Source::Subscription, Some(1), row.clone())]
        );
        let watch = watches(&app, id)[0].clone();
        assert_eq!(watch.next, Some(due + 3600));
        assert_eq!(
            watch.last.map(|last| (last.due, last.outcome)),
            Some((due, subscription::Outcome::Waiting))
        );
        assert_eq!(watch.fires_today, 1);
        // The same run does not fire twice, whoever says it is due.
        clock(&mut app, &ctx, id, 1, due, due + 3);
        assert_eq!(fires(&app, id).len(), 1);
        // Nor does a run that fired by another way in the second it was
        // due, as "Run now" pressed just then: it does not fire again, and
        // it does not stay due either, which the clock would say without
        // end.
        {
            let watch = &mut app.projects.runs.get_mut(&id).unwrap().subscriptions[0];
            watch.next = Some(due);
        }
        clock(&mut app, &ctx, id, 1, due, due + 3);
        assert_eq!(fires(&app, id).len(), 1);
        assert_eq!(watches(&app, id)[0].next, Some(due + 3600));
        assert_eq!(watches(&app, id)[0].fires_today, 1);
        // The clock is set for the run that follows.
        tick(&mut app, &ctx, start);
        assert_eq!(app.projects.runs[&id].armed, [(1, due + 3600)]);

        // The lead is woken with the instruction as it was saved, marked
        // as written earlier. It has nothing to say: nobody is disturbed.
        let lead_started = leads.borrow().len();
        assert_eq!(lead_started, 0, "a watch that has not fired starts no lead");
        let now = Instant::now() + inbox::MAX_WAIT;
        tick(&mut app, &ctx, now);
        let lead = leads.borrow()[0].script.clone();
        lead.borrow_mut().ready("session-1");
        tick(&mut app, &ctx, now);
        tick(&mut app, &ctx, now);
        lead.borrow_mut().begin();
        tick(&mut app, &ctx, now);
        let turn = lead.borrow().turns[0].1.clone();
        let saved = context::date(watch.saved);
        assert!(
            turn.contains(&format!(
                "<neptune-events>\n[watch \"Nightly\" fired · schedule · due {}]\n\
                 <instruction saved=\"{saved}\">\nCheck the build.\n</instruction>\n\
                 </neptune-events>\n",
                schedule::clock(due)
            )),
            "{turn}"
        );
        // Due again while the lead is still at the last one: passed over,
        // not stacked, and not made up for.
        clock(&mut app, &ctx, id, 1, due + 3600, due + 3600);
        assert_eq!(fires(&app, id).len(), 1);
        assert_eq!(watches(&app, id)[0].next, Some(due + 7200));
        lead.borrow_mut().say(1, "Nothing to report.");
        lead.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, now);
        assert_eq!(
            watches(&app, id)[0].last.map(|last| last.outcome),
            Some(subscription::Outcome::Done)
        );
        assert!(
            app.projects.banners.is_empty(),
            "nothing to report is no news"
        );

        // The next run finds something: the person is told once, in words
        // that say nothing of the chat.
        clock(&mut app, &ctx, id, 1, due + 7200, due + 7200);
        assert_eq!(fires(&app, id).len(), 2);
        lead_turn(
            &mut app,
            &ctx,
            &leads,
            "The build is red since this morning.",
        );
        assert_eq!(app.projects.banners, [(id, "The lead has an update")]);
        // A turn the person began is theirs to read: no banner.
        send(&mut app, &ctx, id, "ok");
        lead_turn(&mut app, &ctx, &leads, "Noted.");
        assert_eq!(app.projects.banners.len(), 1);

        // A watch wakes the lead twelve times a day and no more.
        let mut next = due + 3 * 3600;
        {
            let watch = &mut app.projects.runs.get_mut(&id).unwrap().subscriptions[0];
            watch.fires_today = subscription::MAX_FIRES_A_DAY;
            watch.day = next / (24 * 3600);
        }
        clock(&mut app, &ctx, id, 1, next, next);
        assert_eq!(fires(&app, id).len(), 2);
        next += 3600;
        assert_eq!(watches(&app, id)[0].next, Some(next));
        app.projects.runs.get_mut(&id).unwrap().subscriptions[0].fires_today = 0;

        // A paused watch is not on the clock; resumed, it goes on with the
        // next run that is still to come.
        app.action(&ctx, Action::Project(Event::PauseWatch(id, 1, true)));
        tick(&mut app, &ctx, start);
        assert!(app.projects.runs[&id].armed.is_empty());
        clock(&mut app, &ctx, id, 1, next, next);
        assert_eq!(fires(&app, id).len(), 2);
        app.action(&ctx, Action::Project(Event::PauseWatch(id, 1, false)));
        let next = watches(&app, id)[0].next.unwrap();
        tick(&mut app, &ctx, start);
        assert_eq!(app.projects.runs[&id].armed, [(1, next)]);
        // A paused project holds its watches: the run passes.
        app.action(&ctx, Action::Project(Event::SetPaused(id, true)));
        clock(&mut app, &ctx, id, 1, next, next);
        assert_eq!(fires(&app, id).len(), 2);
        assert_eq!(watches(&app, id)[0].next, Some(next + 3600));
        app.action(&ctx, Action::Project(Event::SetPaused(id, false)));

        // "Run now" is the person's: it fires at once, is not counted
        // against the watch's day, and is not stacked either.
        app.action(&ctx, Action::Project(Event::RunWatch(id, 1)));
        assert_eq!(
            fires(&app, id).last().unwrap().2,
            "Watch “Nightly” fired · run by you"
        );
        assert_eq!(watches(&app, id)[0].fires_today, 0);
        app.action(&ctx, Action::Project(Event::RunWatch(id, 1)));
        assert_eq!(fires(&app, id).len(), 3);
        assert_eq!(
            app.ui.error.take().as_deref(),
            Some("The lead still has that watch's last run.")
        );
        let told = lead_turn(&mut app, &ctx, &leads, "Nothing to report");
        assert!(
            told.contains("[watch \"Nightly\" fired · run by the user · "),
            "{told}"
        );

        // Deleted, it is gone from the project and from the clock.
        app.action(&ctx, Action::Project(Event::DeleteWatch(id, 1)));
        tick(&mut app, &ctx, start);
        assert!(watches(&app, id).is_empty() && app.projects.runs[&id].armed.is_empty());
        // What a watch was told to do is in the project's folder only.
        let window = serde_json::to_string(
            &crate::persistence::workspace_state::StateSnapshot::from_model(app.controller.model()),
        )
        .unwrap();
        assert!(!window.contains("Nightly") && !window.contains("Check the build"));
    }

    #[test]
    fn a_schedule_missed_while_neptune_was_closed_runs_once_after_it_opens() {
        let dir = tempfile::tempdir().unwrap();
        let folder = folder_of(dir.path(), 1);
        {
            let (mut app, ctx, _leads, id) = reopen(dir.path(), 1);
            tick(&mut app, &ctx, Instant::now());
            tick(&mut app, &ctx, Instant::now());
            add_watch(
                &mut app,
                &ctx,
                id,
                "Hourly",
                WatchAsk::Every(15),
                "Look at CI.",
            );
            add_watch(
                &mut app,
                &ctx,
                id,
                "Review",
                WatchAsk::PullRequest(PULL.into()),
                "",
            );
            add_watch(
                &mut app,
                &ctx,
                id,
                "Paused",
                WatchAsk::Every(30),
                "Never mind.",
            );
            assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
            // As after most of an hour closed: the first was due three
            // quarters of an hour ago, the paused one as well.
            let now = schedule::seconds(SystemTime::now());
            let run = app.projects.runs.get_mut(&id).unwrap();
            run.subscriptions[0].next = Some(now - 3 * 900 - 10);
            run.subscriptions[1].seen = Some(subscription::Seen {
                state: subscription::Review::Open,
                checks: subscription::Checks::Pending,
                unresolved: 0,
            });
            run.subscriptions[2].next = Some(now - 7200);
            run.subscriptions[2].paused = true;
            run.dirty = true;
            tick(&mut app, &ctx, Instant::now());
            close(app);
        }
        // Its watches are in its folder, as its file names them.
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("project.json")).unwrap()).unwrap();
        let kept = state["subscriptions"].as_array().unwrap();
        assert_eq!(kept.len(), 3);
        assert_eq!(
            kept[0]["trigger"],
            serde_json::json!({"kind": "interval", "minutes": 15})
        );
        assert_eq!(kept[0]["instruction"], "Look at CI.");
        assert_eq!(
            kept[1]["trigger"],
            serde_json::json!({"kind": "pull_request", "url": PULL})
        );
        assert_eq!(kept[1]["seen"]["checks"], "pending");
        assert_eq!(state["counters"]["watches"], 3);

        let opened = schedule::seconds(SystemTime::now());
        let due;
        {
            let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
            tick(&mut app, &ctx, Instant::now());
            tick(&mut app, &ctx, Instant::now());
            assert_eq!(watches(&app, id).len(), 3);
            // The one that may run and was due runs once, half a minute
            // after Neptune opened. The paused one does not.
            let run = &app.projects.runs[&id];
            assert_eq!(run.late.keys().copied().collect::<Vec<_>>(), [1]);
            let (at, runs) = run.late[&1];
            assert_eq!(runs, 4);
            assert!((opened + 30..=opened + 40).contains(&at), "{at} {opened}");
            tick(&mut app, &ctx, Instant::now());
            assert_eq!(app.projects.runs[&id].armed, [(1, at)]);
            assert!(fires(&app, id).is_empty() && leads.borrow().is_empty());
            // Not before that moment.
            clock(&mut app, &ctx, id, 1, at, at - 1);
            assert!(fires(&app, id).is_empty());
            assert!(app.projects.runs[&id].late.contains_key(&1));
            clock(&mut app, &ctx, id, 1, at, at);
            assert_eq!(
                fires(&app, id),
                [(
                    Source::Subscription,
                    Some(1),
                    format!(
                        "Watch “Hourly” fired · schedule · due {} · missed 4 runs while Neptune \
                         was closed",
                        schedule::clock(at)
                    )
                )]
            );
            // Once: its next run is a whole beat from now.
            assert_eq!(watches(&app, id)[0].next, Some(at + 900));
            assert!(app.projects.runs[&id].late.is_empty());
            clock(&mut app, &ctx, id, 1, at, at + 1);
            assert_eq!(fires(&app, id).len(), 1);

            // What its pull request became while Neptune was closed is said
            // once, from what the lead was last told.
            app.pull_requests = pull_requests::Watcher::with(Box::new(|_, links| {
                vec![
                    Some(pull_requests::Status {
                        state: pull_requests::State::Open,
                        checks: pull_requests::Checks::Failing,
                        unresolved: 0,
                    });
                    links.len()
                ]
            }));
            read_pulls(&mut app, &ctx);
            tick(&mut app, &ctx, Instant::now());
            tick(&mut app, &ctx, Instant::now());
            assert_eq!(
                fires(&app, id)[1..],
                [(
                    Source::Pr,
                    Some(2),
                    "Pull request zevem/neptune#83: its checks are failing".to_owned()
                )]
            );
            // As if Neptune then stayed closed past the schedule's next run
            // with its fire still waiting for the lead.
            let run = app.projects.runs.get_mut(&id).unwrap();
            run.subscriptions[0].next = Some(opened - 1800);
            run.dirty = true;
            tick(&mut app, &ctx, Instant::now());
            due = opened - 1800;
            close(app);
        }
        {
            // Opened again before the lead was told: what the watches said
            // waits for it once, and nothing is late any more. A watch that
            // comes due meanwhile is not told on top of it.
            let (mut app, ctx, _leads, id) = reopen(dir.path(), 1);
            tick(&mut app, &ctx, Instant::now());
            tick(&mut app, &ctx, Instant::now());
            let run = &app.projects.runs[&id];
            assert!(run.late.is_empty());
            assert!(run.inbox.awaits(1) && run.inbox.awaits(2));
            assert_eq!(fires(&app, id).len(), 2);
            // The fire that waits is the schedule's one late run: the runs
            // missed since are not made up for beside it.
            let now = schedule::seconds(SystemTime::now());
            clock(&mut app, &ctx, id, 1, due, now);
            tick(&mut app, &ctx, Instant::now());
            assert_eq!(fires(&app, id).len(), 2);
            assert!(app.projects.runs[&id].late.is_empty());
            assert!(watches(&app, id)[0].next.unwrap() > now);
            // What waits is still said to be running; a fire whose turn
            // Neptune was closed in is not.
            assert_eq!(
                watches(&app, id)[0].last.map(|last| last.outcome),
                Some(subscription::Outcome::Waiting)
            );
        }
    }

    /// Names the pull requests projects follow to the watcher, as a frame
    /// does, and waits until each was read.
    fn read_pulls(app: &mut App, ctx: &egui::Context) {
        let links = app.projects.links(app.controller.model());
        assert!(!links.is_empty(), "no pull request is followed");
        app.pull_requests.watch(links.iter().cloned(), ctx);
        let deadline = Instant::now() + Duration::from_secs(10);
        while links
            .iter()
            .any(|link| app.pull_requests.lookup(link) == Lookup::Checking)
        {
            assert!(
                Instant::now() < deadline,
                "the pull requests were never read"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// The watcher reads every pull request as `status` from now on.
    fn pulls_stand(
        app: &mut App,
        ctx: &egui::Context,
        state: pull_requests::State,
        checks: pull_requests::Checks,
        unresolved: u8,
    ) {
        app.pull_requests = pull_requests::Watcher::with(Box::new(move |_, links| {
            vec![
                Some(pull_requests::Status {
                    state,
                    checks,
                    unresolved,
                });
                links.len()
            ]
        }));
        read_pulls(app, ctx);
        tick(app, ctx, Instant::now());
    }

    #[test]
    fn a_pull_request_an_agent_links_is_followed_and_its_lead_hears_what_changed() {
        use pull_requests::{Checks, State};
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        spawn(
            &app,
            1,
            id,
            Task {
                title: Some("auth".into()),
                ..Task::new(AgentKind::Codex, "brief")
            },
            PathBuf::new(),
        );
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let Some(Ok(pane)) = app.sessions.agents().spawned(1) else {
            panic!("no terminal was opened");
        };
        app.controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation: 1,
                agent: Some(AgentSession {
                    kind: AgentKind::Codex,
                    session_id: Some(AGENT_SESSION.into()),
                    cwd: dir.path().into(),
                }),
            })
            .unwrap();
        tick(&mut app, &ctx, start);
        assert!(watches(&app, id).is_empty());
        // The agent links its pull request: Neptune follows it from then
        // on, without anyone asking.
        app.controller
            .dispatch(Command::PanePullRequestLinked {
                pane,
                generation: 1,
                pull_request: PullRequest::parse(PULL).unwrap(),
            })
            .unwrap();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let watch = watches(&app, id)[0].clone();
        assert_eq!(
            (watch.title.as_str(), watch.allowed, watch.seen),
            ("PR #83", true, None)
        );
        assert_eq!(
            watch.trigger,
            Trigger::PullRequest {
                url: PULL.into(),
                auto: true
            }
        );
        assert_eq!(watches(&app, id).len(), 1);
        let pulls = |app: &App| -> Vec<String> {
            fires(app, id)
                .into_iter()
                .filter(|(source, _, _)| *source == Source::Pr)
                .map(|(_, _, what)| what)
                .collect()
        };

        // As it is first read it is remembered, and nothing is said.
        pulls_stand(&mut app, &ctx, State::Open, Checks::Pending, 0);
        assert_eq!(
            watches(&app, id)[0].seen.map(|seen| seen.words()),
            Some("open · checks running".into())
        );
        assert!(pulls(&app).is_empty());
        // Its checks fail and two comments are open: one event that names
        // the agent whose pull request it is, in counts and states only.
        pulls_stand(&mut app, &ctx, State::Open, Checks::Failing, 2);
        let agent = pane.get();
        assert_eq!(
            pulls(&app),
            [format!(
                "Pull request zevem/neptune#83 of agent {agent} “auth”: its checks are failing; \
                 unresolved review comments went from 0 to 2"
            )]
        );
        // What changes before the lead has taken that is said with what
        // follows, not stacked on it.
        pulls_stand(&mut app, &ctx, State::Open, Checks::Passing, 2);
        assert_eq!(pulls(&app).len(), 1);
        let told = lead_turn(&mut app, &ctx, &leads, "Agent told.");
        assert!(
            told.contains(&format!(
                "[pull request {PULL} of agent {agent} \"auth\": its checks are failing; \
                 unresolved review comments went from 0 to 2 · now open · checks failing · \
                 2 unresolved review comments]\n"
            )),
            "{told}"
        );
        tick(&mut app, &ctx, Instant::now());
        assert_eq!(pulls(&app).len(), 2);
        assert!(pulls(&app)[1].ends_with("“auth”: its checks pass"));
        lead_turn(&mut app, &ctx, &leads, "Nothing to report.");

        // A paused watch says nothing; resumed, it says once what changed.
        let watch = watches(&app, id)[0].id;
        app.action(&ctx, Action::Project(Event::PauseWatch(id, watch, true)));
        pulls_stand(&mut app, &ctx, State::Open, Checks::Failing, 2);
        assert_eq!(pulls(&app).len(), 2);
        app.action(&ctx, Action::Project(Event::PauseWatch(id, watch, false)));
        tick(&mut app, &ctx, Instant::now());
        assert_eq!(pulls(&app).len(), 3);
        lead_turn(&mut app, &ctx, &leads, "Nothing to report.");

        // One pull request wakes the lead ten times a day and no more,
        // except to say that it is over.
        {
            let kept = &mut app.projects.runs.get_mut(&id).unwrap().subscriptions[0];
            kept.fires_today = subscription::MAX_PR_WAKES_A_DAY;
            kept.day = unix_now() / (24 * 3600);
        }
        pulls_stand(&mut app, &ctx, State::Open, Checks::Passing, 2);
        assert_eq!(pulls(&app).len(), 3);
        // Merged: said once, and its watch ends itself.
        pulls_stand(&mut app, &ctx, State::Merged, Checks::Passing, 0);
        assert_eq!(pulls(&app).len(), 4);
        assert!(pulls(&app)[3].ends_with("“auth”: it was merged"));
        assert!(watches(&app, id).is_empty());
        // It does not come back while the link stays on the terminal.
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        assert!(watches(&app, id).is_empty());
    }

    #[test]
    fn a_lead_proposes_a_schedule_the_person_allows_and_follows_pull_requests_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, _leads, id) = reopen(dir.path(), 1);
        let shell = app.controller.model().active_pane().unwrap();
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        let lead = Starter::Project(id);
        let add =
            |title: &str, trigger: WatchTrigger, instruction: &str| ProjectCall::AddSubscription {
                title: title.into(),
                trigger,
                instruction: instruction.into(),
            };
        let every = |every_minutes| WatchTrigger::Schedule { every_minutes };

        // A schedule the lead asks for is only proposed: it has no run, is
        // not on the clock, and the chat asks the person.
        let said = call(
            &mut app,
            &ctx,
            1,
            id,
            lead,
            add("Nightly", every(60), "Check the build."),
        );
        assert!(
            said.clone()
                .unwrap()
                .starts_with("Watch 1 \"Nightly\" is proposed — the user must allow it."),
            "{said:?}"
        );
        let watch = watches(&app, id)[0].clone();
        assert_eq!((watch.allowed, watch.next), (false, None));
        assert!(app.projects.runs[&id].armed.is_empty());
        assert!(entries(&app, id).contains(&Entry::Proposal {
            watch: 1,
            what: "The lead proposes a watch: “Nightly” · every 1 h".into(),
            text: "Check the build.".into(),
        }));
        {
            let panel = app.project_panel(true);
            let Body::Project(project) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("the workspace in view has no project");
            };
            assert_eq!(project.chat.proposed, [1]);
            assert!(project.watches[0].proposed);
            assert_eq!(
                project.watches[0].detail,
                "Proposed by the lead · waits for you"
            );
        }
        // Until the person says yes it cannot be run, by anyone.
        app.action(&ctx, Action::Project(Event::RunWatch(id, 1)));
        assert!(fires(&app, id).is_empty());
        assert_eq!(
            call(&mut app, &ctx, 2, id, lead, ProjectCall::ListSubscriptions),
            Ok("Watch 1 \"Nightly\": every 1 h; proposed — the user must allow it".into())
        );
        // Allowed, it runs a whole beat from the yes, and the chat says so.
        let before = schedule::seconds(SystemTime::now());
        app.action(&ctx, Action::Project(Event::AllowWatch(id, 1, true)));
        let watch = watches(&app, id)[0].clone();
        assert!(watch.allowed && watch.next.is_some_and(|next| next >= before + 3600));
        assert_eq!(
            notices(
                &app,
                id,
                "You allowed the watch “Nightly”: it runs every 1 h."
            ),
            1
        );
        tick(&mut app, &ctx, Instant::now());
        assert_eq!(app.projects.runs[&id].armed.len(), 1);
        // A yes or a no is given once.
        app.action(&ctx, Action::Project(Event::AllowWatch(id, 1, false)));
        assert_eq!(watches(&app, id).len(), 1);
        // Declined, a proposal is gone.
        assert!(call(&mut app, &ctx, 3, id, lead, add("Hourly", every(15), "x")).is_ok());
        app.action(&ctx, Action::Project(Event::AllowWatch(id, 2, false)));
        assert_eq!(watches(&app, id).len(), 1);
        assert_eq!(notices(&app, id, "You declined the watch “Hourly”."), 1);
        // What is not a watch is refused with the reason.
        for (request, asked, why) in [
            (4, add("Often", every(5), "x"), "every 15 minutes"),
            (5, add("Empty", every(60), " "), "what the lead should do"),
            (6, add("", every(60), "x"), "needs a title"),
            (
                7,
                add("PR", WatchTrigger::PullRequest("not a link".into()), ""),
                "not the address",
            ),
        ] {
            let refused = call(&mut app, &ctx, request, id, lead, asked).unwrap_err();
            assert!(refused.contains(why), "{refused}");
        }
        // Only the lead manages watches.
        assert_eq!(
            call(
                &mut app,
                &ctx,
                8,
                id,
                Starter::Pane(shell),
                ProjectCall::ListSubscriptions
            ),
            Err("Only a project's lead manages its watches.".into())
        );

        // A pull request is followed at once, under its plain address.
        let said = call(
            &mut app,
            &ctx,
            9,
            id,
            lead,
            add(
                "Review",
                WatchTrigger::PullRequest(format!("{PULL}/files")),
                "",
            ),
        );
        assert!(said.unwrap().starts_with("Watch 3 follows it."));
        assert!(watches(&app, id)[1].allowed);
        assert_eq!(
            call(
                &mut app,
                &ctx,
                10,
                id,
                lead,
                add("x", WatchTrigger::PullRequest("linked".into()), "")
            )
            .map(|said| said.contains("linked no pull request yet")),
            Ok(true)
        );
        // Where it stands is what the watcher read: states and counts.
        let status = |url: Option<&str>| ProjectCall::PullRequestStatus {
            url: url.map(str::to_owned),
        };
        assert_eq!(
            call(&mut app, &ctx, 11, id, lead, status(None)),
            Ok(format!("{PULL}: checking"))
        );
        pulls_stand(
            &mut app,
            &ctx,
            pull_requests::State::Open,
            pull_requests::Checks::Failing,
            2,
        );
        assert_eq!(
            call(&mut app, &ctx, 12, id, lead, status(Some(PULL))),
            Ok(format!(
                "{PULL}: open · checks failing · 2 unresolved review comments"
            ))
        );
        // One nobody follows is read from then on, for a while.
        let other = "https://github.com/zevem/neptune/pull/84";
        assert_eq!(
            call(&mut app, &ctx, 13, id, lead, status(Some(other))),
            Ok(format!("{other}: checking"))
        );
        assert!(
            app.projects
                .links(app.controller.model())
                .iter()
                .any(|link| link.url() == other)
        );
        assert!(
            call(&mut app, &ctx, 14, id, lead, status(Some("nowhere")))
                .unwrap_err()
                .contains("address of a pull request")
        );

        // The lead ends a watch; the chat says what Neptune did.
        assert_eq!(
            call(
                &mut app,
                &ctx,
                15,
                id,
                lead,
                ProjectCall::RemoveSubscription { id: 3 }
            ),
            Ok("Removed watch 3.".into())
        );
        assert!(
            call(
                &mut app,
                &ctx,
                16,
                id,
                lead,
                ProjectCall::RemoveSubscription { id: 3 }
            )
            .unwrap_err()
            .contains("no watch 3")
        );
        let cards: Vec<String> = entries(&app, id)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Tool { summary, .. } => Some(summary),
                _ => None,
            })
            .collect();
        assert_eq!(
            cards,
            [
                "Added a watch: Pull request zevem/neptune#83",
                "Removed the watch “Review”"
            ]
        );
        // A paused project takes no new watch.
        app.action(&ctx, Action::Project(Event::SetPaused(id, true)));
        assert!(
            call(&mut app, &ctx, 17, id, lead, add("Later", every(60), "x"))
                .unwrap_err()
                .contains("paused")
        );
        assert_eq!(watches(&app, id).len(), 1);
    }

    #[test]
    fn the_person_is_told_once_when_a_project_begins_to_need_them() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        send(&mut app, &ctx, id, "go");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert!(app.projects.banners.is_empty());
        // Its lead cannot go on without them: one word, however long it
        // stays that way.
        leads.borrow()[0].script.borrow_mut().fail(Failure::Auth);
        for _ in 0..3 {
            tick(&mut app, &ctx, start);
        }
        assert_eq!(app.projects.banners, [(id, "Needs you")]);
        // Cleared and needed again, they are told again; not while they
        // are looking at the project.
        app.action(&ctx, Action::Project(Event::Retry(id)));
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        leads
            .borrow()
            .last()
            .unwrap()
            .script
            .borrow_mut()
            .fail(Failure::Auth);
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(app.projects.banners.len(), 2);
        app.action(&ctx, Action::Project(Event::Retry(id)));
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        app.ui.panel.open = true;
        app.ui.panel.tab = ui::panel::Tab::Project;
        leads
            .borrow()
            .last()
            .unwrap()
            .script
            .borrow_mut()
            .fail(Failure::Auth);
        // The window has the keyboard, as it does while it is looked at.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        assert!(ctx.input(|input| input.focused));
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(app.projects.runs[&id].trouble, Some(Failure::Auth));
        assert_eq!(app.projects.banners.len(), 2);
    }

    #[test]
    fn a_window_too_narrow_for_the_panel_shows_the_project_in_a_sheet() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, _leads) = application(dir.path());
        let space = workspace(&mut app, dir.path(), "shop");
        let id = project(&mut app, space, 1);
        tick(&mut app, &ctx, Instant::now());
        let composer = ui::chat::composer_id();
        let show = Action::Panel(ui::panel::Event::Show(ui::panel::Tab::Project));
        // The smallest window Neptune takes still has room for the panel,
        // however wide the sidebar is set: the sidebar gives way first.
        let smallest = Vec2::new(640.0, 400.0);
        app.config.sidebar_width = 360.0;
        frame_of(&mut app, &ctx, smallest);
        assert!(app.ui.panel.available);
        app.action(&ctx, show.clone());
        assert_eq!(app.ui.overlay, OverlayState::None);
        assert!(app.ui.panel.open && app.ui.panel.tab == ui::panel::Tab::Project);
        app.action(&ctx, Action::Panel(ui::panel::Event::Toggle));
        // The same window with the application drawn half as large again
        // has none: 427 by 267 points. Asked for by name, the project
        // comes in a sheet, with the keyboard in its field when a message
        // was asked for.
        let narrow = Vec2::new(427.0, 267.0);
        frame_of(&mut app, &ctx, narrow);
        assert!(!app.ui.panel.available);
        app.action(&ctx, show);
        assert_eq!(app.ui.overlay, OverlayState::Project);
        assert!(!app.ui.panel.open && !app.ui.project.focus);
        frame_of(&mut app, &ctx, narrow);
        assert!(!ctx.memory(|memory| memory.has_focus(composer)));
        app.action(&ctx, Action::CloseOverlay);
        app.action(&ctx, Action::Project(Event::Compose));
        assert_eq!(app.ui.overlay, OverlayState::Project);
        assert!(app.ui.project.focus);
        // What is drawn stays inside the window, the composer at its foot.
        let mut drawn = Vec::new();
        for _ in 0..3 {
            let mut host = eframe::Frame::_new_kittest();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, narrow)),
                    ..Default::default()
                },
                |ui| eframe::App::ui(&mut app, ui, &mut host),
            );
            output.textures_delta.clear();
            drawn = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text)
                        if clipped.clip_rect.intersects(text.visual_bounding_rect()) =>
                    {
                        Some((
                            text.galley.text().to_owned(),
                            text.visual_bounding_rect().intersect(clipped.clip_rect),
                        ))
                    }
                    _ => None,
                })
                .collect();
        }
        assert!(!app.ui.project.focus, "asked for once");
        assert!(ctx.memory(|memory| memory.has_focus(composer)));
        let found = |wanted: &str| {
            drawn
                .iter()
                .find(|(text, _)| text.contains(wanted))
                .unwrap_or_else(|| panic!("{wanted} in {drawn:?}"))
                .1
        };
        // The sheet's title row is the project's header, and who leads is
        // under the composer.
        for wanted in ["shop", "Message the lead…", "Claude Code"] {
            let rect = found(wanted);
            assert!(
                rect.left() >= 0.0 && rect.right() <= narrow.x && rect.bottom() <= narrow.y,
                "{wanted} at {rect:?}"
            );
        }
        assert!(found("Message the lead…").top() > found("shop").bottom());
        // A message goes to the same lead as from the tab.
        send(&mut app, &ctx, id, "from the sheet");
        assert!(matches!(
            entries(&app, id).as_slice(),
            [Entry::User { text, .. }] if text == "from the sheet"
        ));
        // Closed, the keyboard is the terminal's again and the draft stays.
        app.ui.project.drafts.insert(id, "half a thought".into());
        app.action(&ctx, Action::CloseOverlay);
        frame_of(&mut app, &ctx, narrow);
        assert!(!ctx.memory(|memory| memory.has_focus(composer)));
        assert_eq!(app.ui.project.drafts[&id], "half a thought");
        // In a window made wide while the sheet is open, the project is in
        // its tab again.
        app.action(&ctx, Action::Project(Event::Compose));
        frame_of(&mut app, &ctx, narrow);
        frame_of(&mut app, &ctx, narrow);
        assert!(ctx.memory(|memory| memory.has_focus(composer)));
        for _ in 0..4 {
            frame(&mut app, &ctx);
        }
        assert_eq!(app.ui.overlay, OverlayState::None);
        assert!(app.ui.panel.open && app.ui.panel.tab == ui::panel::Tab::Project);
        // A message being written there goes on in the tab: what is typed
        // next must not reach the shell.
        assert!(ctx.memory(|memory| memory.has_focus(composer)));
    }

    fn git(directory: &std::path::Path, arguments: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {arguments:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
    /// A repository with one commit on `main`, by the path git names it.
    fn repository(under: &std::path::Path) -> PathBuf {
        let root = under.canonicalize().unwrap().join("shop");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        for (key, value) in [
            ("user.name", "Neptune Test"),
            ("user.email", "test@neptune.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            git(&root, &["config", key, value]);
        }
        std::fs::write(root.join("README.md"), "shop").unwrap();
        git(&root, &["add", "--all"]);
        git(&root, &["commit", "--quiet", "-m", "first"]);
        std::path::Path::new(&git(&root, &["rev-parse", "--show-toplevel"]))
            .components()
            .collect()
    }
    /// Frames until the application has answered the spawn `request`.
    fn started(app: &mut App, ctx: &egui::Context, request: u64) -> Result<PaneId, String> {
        let limit = Instant::now() + Duration::from_secs(30);
        loop {
            tick(app, ctx, Instant::now());
            app.poll_worktrees(ctx);
            if let Some(result) = app.sessions.agents().spawned(request) {
                return result;
            }
            assert!(Instant::now() < limit, "the spawn was never answered");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn cards(app: &App, project: ProjectId) -> Vec<String> {
        entries(app, project)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Tool { summary, .. } => Some(summary),
                _ => None,
            })
            .collect()
    }

    #[cfg_attr(windows, ignore = "its directories are written as on Unix")]
    #[test]
    fn a_lead_gives_an_agent_a_worktree_of_its_own_and_hears_why_where_git_makes_none() {
        let dir = tempfile::tempdir().unwrap();
        let root = repository(dir.path());
        let (mut app, ctx, _leads) = application(dir.path());
        let space = workspace(&mut app, &root, "shop");
        let id = project(&mut app, space, 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let ask = |app: &App, request: u64, project: ProjectId, title: &str, branch: &str| {
            spawn(
                app,
                request,
                project,
                Task {
                    title: Some(title.into()),
                    worktree: Some(branch.into()),
                    ..Task::new(AgentKind::Codex, "Fix the login.")
                },
                PathBuf::new(),
            );
        };
        // Git makes the worktree off the frame: until it has answered there
        // is no terminal, and the lead's call is not answered either.
        ask(&app, 1, id, "login", "fix-login");
        for _ in 0..3 {
            tick(&mut app, &ctx, start);
        }
        assert_eq!(app.controller.model().pane_count(), 1);
        assert!(app.sessions.agents().spawned(1).is_none());
        let pane = started(&mut app, &ctx, 1).expect("no terminal was opened");
        let tree = root.parent().unwrap().join("shop.worktrees/fix-login");
        assert!(tree.join("README.md").exists());
        assert_eq!(git(&tree, &["branch", "--show-current"]), "fix-login");
        let model = app.controller.model();
        let item = model.pane(pane).unwrap();
        let worktree = item
            .worktree()
            .cloned()
            .expect("the terminal has no worktree");
        assert_eq!(
            (
                worktree.branch.as_str(),
                &worktree.path,
                &worktree.repository
            ),
            ("fix-login", &tree, &root)
        );
        assert_eq!((item.cwd(), item.project()), (tree.as_path(), Some(id)));
        assert!(model.workspace(space).unwrap().is_background(pane));
        // Its brief names the branch and where it is, before the task.
        let brief = app.sessions.agents().task(pane).unwrap();
        assert!(
            brief.contains(&format!(
                "You work in a git worktree of your own: branch fix-login, in {}.",
                tree.display()
            )) && brief.ends_with("Fix the login."),
            "{brief}"
        );
        // The chat, the roster and what the project remembers say the same.
        tick(&mut app, &ctx, Instant::now());
        assert!(cards(&app, id).contains(&format!(
            "Started agent {pane} · Codex · login · in its own worktree, on fix-login"
        )));
        let panel = app.project_panel(true);
        let Body::Project(shown) = app.projects.view(&panel, ui::chat::PAGE) else {
            panic!("the workspace in view has no project");
        };
        assert_eq!(shown.members[0].branch.as_deref(), Some("fix-login"));
        assert_eq!(
            app.projects.runs[&id]
                .tasks
                .get(pane.get())
                .unwrap()
                .worktree,
            Some(worktree.clone())
        );
        // A second agent on the same branch is given the same worktree.
        ask(&app, 2, id, "tests", "fix-login");
        let second = started(&mut app, &ctx, 2).expect("no terminal was opened");
        assert_eq!(
            app.controller.model().pane(second).unwrap().worktree(),
            Some(&worktree)
        );
        // One opened again goes on in the worktree it had.
        app.dispatch(&ctx, Command::ClosePane(pane));
        tick(&mut app, &ctx, Instant::now());
        spawn(
            &app,
            3,
            id,
            Task {
                title: Some("login".into()),
                resume: Some((pane, AGENT_SESSION.into())),
                ..Task::new(AgentKind::Codex, "One more thing")
            },
            tree.clone(),
        );
        let again = started(&mut app, &ctx, 3).expect("the agent was not opened again");
        assert_eq!(
            app.controller.model().pane(again).unwrap().worktree(),
            Some(&worktree)
        );

        // What git refuses is said to the lead and in the chat, and opens
        // nothing: the branch the repository itself has checked out,
        let panes = app.controller.model().pane_count();
        ask(&app, 4, id, "main", "main");
        let refused = started(&mut app, &ctx, 4).unwrap_err();
        assert!(
            refused.starts_with("Neptune could not make the worktree of main: main is checked out"),
            "{refused}"
        );
        tick(&mut app, &ctx, Instant::now());
        assert!(cards(&app, id).contains(&format!("Could not start an agent: {refused}")));
        // a name git does not take,
        ask(&app, 5, id, "bad", "a..b");
        let refused = started(&mut app, &ctx, 5).unwrap_err();
        assert_eq!(
            refused,
            "Git does not take a..b as a branch name. Choose another."
        );
        // and a project whose directory is no repository.
        let plain = dir.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        let other = workspace(&mut app, &plain, "plain");
        let unversioned = project(&mut app, other, 2);
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        ask(&app, 6, unversioned, "x", "fix-login");
        let refused = started(&mut app, &ctx, 6).unwrap_err();
        assert!(refused.contains("not in a git repository"), "{refused}");
        assert_eq!(app.controller.model().pane_count(), panes + 1);
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        // Nor does one that has all the agents it may have open: no branch
        // and no checkout are left behind for an agent that cannot start.
        let open = app.controller.model().project_agents(id).count();
        for request in 0..(Project::MAX_AGENTS - open) as u64 {
            spawn(
                &app,
                20 + request,
                id,
                Task::new(AgentKind::Codex, "brief"),
                PathBuf::new(),
            );
            started(&mut app, &ctx, 20 + request).expect("no terminal was opened");
        }
        ask(&app, 30, id, "seventh", "one-too-many");
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        assert!(matches!(
            app.sessions.agents().spawned(30),
            Some(Err(reason)) if reason.contains("agents open at once")
        ));
        let worktrees = root.parent().unwrap().join("shop.worktrees");
        assert!(!worktrees.join("one-too-many").exists());
        assert!(git(&root, &["branch", "--list", "one-too-many"]).is_empty());
        // A project that has started all a day allows asks git for nothing.
        let run = app.projects.runs.get_mut(&id).unwrap();
        for _ in 0..crate::projects::registry::MAX_SPAWNS_A_DAY {
            run.counters.spawned(unix_now());
        }
        ask(&app, 7, id, "late", "late-branch");
        tick(&mut app, &ctx, Instant::now());
        tick(&mut app, &ctx, Instant::now());
        assert!(matches!(
            app.sessions.agents().spawned(7),
            Some(Err(reason)) if reason.contains("agents today")
        ));
        assert!(
            !root
                .parent()
                .unwrap()
                .join("shop.worktrees/late-branch")
                .exists()
        );
    }

    #[test]
    fn a_lead_closes_an_agent_but_not_a_terminal_the_person_opened() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, _leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let shell = app.controller.model().active_pane().unwrap();
        let mut panes = Vec::new();
        for (request, title) in [(1, "auth"), (2, "docs"), (3, "tests")] {
            spawn(
                &app,
                request,
                id,
                Task {
                    title: Some(title.into()),
                    ..Task::new(AgentKind::Codex, "brief")
                },
                PathBuf::new(),
            );
            tick(&mut app, &ctx, start);
            tick(&mut app, &ctx, start);
            let Some(Ok(pane)) = app.sessions.agents().spawned(request) else {
                panic!("no terminal was opened");
            };
            app.controller
                .dispatch(Command::PaneAgentChanged {
                    pane,
                    generation: 1,
                    agent: Some(AgentSession {
                        kind: AgentKind::Codex,
                        session_id: Some(AGENT_SESSION.into()),
                        cwd: dir.path().into(),
                    }),
                })
                .unwrap();
            panes.push(pane);
        }
        let (opened, unseen, returned) = (panes[0], panes[1], panes[2]);
        tick(&mut app, &ctx, start);
        assert!(
            app.projects.runs[&id]
                .tasks
                .get(opened.get())
                .unwrap()
                .session
                .is_some()
        );
        // The person opens one agent's terminal and goes back to their own.
        app.action(&ctx, Action::OpenAgent(opened, 1));
        app.action(&ctx, Action::Focus(shell));
        let space = app.controller.model().active_workspace().unwrap();
        let background = |app: &App, pane| {
            app.controller
                .model()
                .workspace(space)
                .unwrap()
                .is_background(pane)
        };
        assert!(!background(&app, opened) && background(&app, unseen));
        // Another they open and send back out of view. Its row offers that
        // only while it has a tab, and its agent stays the project's.
        let offered = |app: &App, pane| {
            let Panel {
                body: PanelBody::Project(view),
            } = app.project_panel(true)
            else {
                panic!("no project in view");
            };
            let member = view.members.iter().find(|member| member.pane == pane);
            member.expect("the agent is not listed").can_background
        };
        app.action(&ctx, Action::OpenAgent(returned, 1));
        assert!(offered(&app, returned) && offered(&app, opened) && !offered(&app, unseen));
        app.action(&ctx, Action::Background(returned));
        assert!(background(&app, returned) && !offered(&app, returned));
        assert!(app.ui.overlay == OverlayState::None && app.pending_close.is_none());
        // The tab that followed it is in front; they go back to their own.
        assert_eq!(app.controller.model().active_pane(), Some(opened));
        app.action(&ctx, Action::Focus(shell));
        assert_eq!(app.controller.model().project_agents(id).count(), 3);
        // The lead closes all three. Those out of view go, the one sent
        // back like the one never opened; the tab stays, its agent
        // running, and is no longer the project's.
        for pane in [opened, unseen, returned] {
            app.sessions.agents().request(AgentRequest::Close {
                pane,
                generation: 1,
                parent: Starter::Project(id),
            });
        }
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let model = app.controller.model();
        assert!(model.pane(unseen).is_none());
        let kept = model.pane(opened).expect("the opened terminal was closed");
        assert_eq!((kept.project(), kept.agent().is_some()), (None, true));
        assert!(!background(&app, opened));
        assert_eq!(model.project_agents(id).count(), 0);
        // The chat says which was which.
        let cards = cards(&app, id);
        assert!(cards.contains(&format!(
            "Agent {opened} left the project. Its terminal stays open: you had opened it as a tab"
        )));
        assert!(cards.contains(&format!("Closed agent {unseen}")));
        assert!(app.controller.model().pane(returned).is_none());
        assert!(cards.contains(&format!("Closed agent {returned}")));
        // Nothing of it is the lead's any more: not its reply, and not its
        // conversation to open again beside the one that still runs.
        assert!(app.sessions.agents().last_report(id, opened.get()).is_err());
        let task = app.projects.runs[&id].tasks.get(opened.get()).unwrap();
        assert!(task.session.is_none() && task.ended.is_some());
        assert!(shown(&app).1.is_empty());
        // A request that names a terminal of another generation closes
        // and releases nothing.
        app.sessions.agents().request(AgentRequest::Close {
            pane: shell,
            generation: 1,
            parent: Starter::Project(id),
        });
        tick(&mut app, &ctx, start);
        assert!(app.controller.model().pane(shell).is_some());
    }

    #[cfg_attr(windows, ignore = "its directories are written as on Unix")]
    #[test]
    fn an_agent_is_ready_for_review_until_its_terminal_is_opened_and_is_told_what_its_pull_request_needs()
     {
        use pull_requests::{Checks, State};
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, _leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let shell = app.controller.model().active_pane().unwrap();
        let mut panes = Vec::new();
        for (request, title) in [(1, "docs"), (2, "auth")] {
            spawn(
                &app,
                request,
                id,
                Task {
                    title: Some(title.into()),
                    ..Task::new(AgentKind::Codex, "brief")
                },
                PathBuf::new(),
            );
            tick(&mut app, &ctx, start);
            tick(&mut app, &ctx, start);
            let Some(Ok(pane)) = app.sessions.agents().spawned(request) else {
                panic!("no terminal was opened");
            };
            panes.push(pane);
        }
        let (docs, auth) = (panes[0], panes[1]);
        let roster = |app: &App| shown(app).1;
        assert_eq!(
            roster(&app),
            [
                format!("{docs} docs · Starting"),
                format!("{auth} auth · Starting")
            ]
        );
        // The second ends a turn with a report: there is something to look
        // at, and it leads the roster, before one that is only starting.
        app.controller
            .dispatch(Command::PaneAgentChanged {
                pane: auth,
                generation: 1,
                agent: Some(AgentSession {
                    kind: AgentKind::Codex,
                    session_id: Some(AGENT_SESSION.into()),
                    cwd: dir.path().into(),
                }),
            })
            .unwrap();
        // Its terminal's start is prepared on a worker; what stands in for
        // its CLI below comes after that.
        let limit = Instant::now() + Duration::from_secs(10);
        while !app.sessions.agents().starting().contains(&(auth, 1)) {
            assert!(Instant::now() < limit, "the terminal was never prepared");
            std::thread::sleep(Duration::from_millis(5));
        }
        app.sessions
            .agents()
            .rested(auth, 1, "Opened the pull request.");
        tick(&mut app, &ctx, start);
        assert_eq!(
            roster(&app),
            [
                format!("{auth} auth · Ready for review"),
                format!("{docs} docs · Starting")
            ]
        );
        assert!(entries(&app, id).iter().any(|entry| matches!(
            entry,
            Entry::Event { what, text, .. }
                if what == "“auth” finished its turn" && text == "Opened the pull request."
        )));
        // It stays so however often the tab is drawn, until the person
        // opens its terminal.
        tick(&mut app, &ctx, start);
        assert_eq!(roster(&app)[0], format!("{auth} auth · Ready for review"));
        app.action(&ctx, Action::OpenAgent(auth, 1));
        tick(&mut app, &ctx, start);
        assert_eq!(
            roster(&app),
            [
                format!("{docs} docs · Starting"),
                format!("{auth} auth · Idle")
            ]
        );
        app.action(&ctx, Action::Focus(shell));

        // Its pull request's checks fail and two comments are open: the
        // roster shows how it stands, and the chat offers to have it told.
        app.controller
            .dispatch(Command::PanePullRequestLinked {
                pane: auth,
                generation: 1,
                pull_request: PullRequest::parse(PULL).unwrap(),
            })
            .unwrap();
        tick(&mut app, &ctx, start);
        pulls_stand(&mut app, &ctx, State::Open, Checks::Failing, 2);
        app.sessions.agents().rested(auth, 1, "Pushed.");
        tick(&mut app, &ctx, start);
        let offered = |app: &App| {
            let panel = app.project_panel(true);
            let Body::Project(shown) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("the workspace in view has no project");
            };
            let member = shown
                .members
                .iter()
                .find(|member| member.pane == auth)
                .unwrap();
            (
                shown.chat.follow.to_vec(),
                member.state.clone(),
                member
                    .pull_requests
                    .iter()
                    .map(|linked| {
                        (
                            linked.link.number(),
                            linked.lookup.status().map(|s| s.checks),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            offered(&app),
            (
                vec![(auth, true, true)],
                "Ready for review".to_owned(),
                vec![(83, Some(Checks::Failing))]
            )
        );
        // The row of the watch that follows the pull request offers the
        // same, for the agent that owns it.
        let pulls = |app: &App| {
            let panel = app.project_panel(true);
            let Body::Project(shown) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("the workspace in view has no project");
            };
            shown.chat.pulls.to_vec()
        };
        let watch = watches(&app, id)
            .iter()
            .find(|watch| matches!(&watch.trigger, Trigger::PullRequest { url, .. } if url == PULL))
            .expect("the linked pull request is followed")
            .id;
        assert_eq!(pulls(&app), [(watch, auth, true, true)]);
        let ask = |app: &mut App, ask| {
            app.action(
                &ctx,
                Action::Project(Event::FollowUp {
                    project: id,
                    pane: auth,
                    generation: 1,
                    ask,
                }),
            );
        };
        // One press sends fixed words the way the lead's messages go, and
        // the chat says who asked.
        ask(&mut app, FollowUp::Checks);
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        let typed = app.sessions.agents().drain_requests();
        assert!(
            matches!(
                typed.as_slice(),
                [AgentRequest::Tell { pane, generation: 1, text }]
                    if *pane == auth
                        && text.starts_with(&format!("The checks of your pull request {PULL} are failing."))
            ),
            "{typed:?}"
        );
        for request in typed {
            app.sessions.agents().request(request);
        }
        tick(&mut app, &ctx, start);
        ask(&mut app, FollowUp::Comments);
        let typed = app.sessions.agents().drain_requests();
        assert!(matches!(
            typed.as_slice(),
            [AgentRequest::Tell { text, .. }]
                if text.contains("has 2 unresolved review comments")
        ));
        for request in typed {
            app.sessions.agents().request(request);
        }
        tick(&mut app, &ctx, start);
        let said = cards(&app, id);
        assert!(said.contains(&format!(
            "You asked agent {auth} to fix the failing checks of zevem/neptune#83"
        )));
        assert!(said.contains(&format!(
            "You asked agent {auth} to address the review comments of zevem/neptune#83"
        )));
        assert!(!said.iter().any(|card| card.starts_with("Took a message")));
        // It is at work on it, so there is nothing to review or to offer.
        let (follow, state, _) = offered(&app);
        assert!(
            follow.is_empty() && state == "Working",
            "{follow:?} {state}"
        );
        assert!(pulls(&app).is_empty());
        // The lead's own message still gets its line.
        app.sessions.agents().request(AgentRequest::Tell {
            pane: auth,
            generation: 1,
            text: "Also update the docs".into(),
        });
        tick(&mut app, &ctx, start);
        assert!(cards(&app, id).contains(&format!("Took a message for agent {auth}")));
        // A paused project sends nothing, and a pull request that no
        // longer needs it is not asked about: both say so.
        let before = cards(&app, id).len();
        app.action(&ctx, Action::Project(Event::SetPaused(id, true)));
        ask(&mut app, FollowUp::Checks);
        assert!(app.ui.error.take().unwrap().contains("paused"));
        app.action(&ctx, Action::Project(Event::SetPaused(id, false)));
        pulls_stand(&mut app, &ctx, State::Open, Checks::Passing, 0);
        ask(&mut app, FollowUp::Checks);
        assert!(app.ui.error.take().unwrap().contains("no longer needs it"));
        ask(&mut app, FollowUp::Comments);
        assert!(app.ui.error.take().is_some());
        // An agent that cannot take a message says so, too.
        pulls_stand(&mut app, &ctx, State::Open, Checks::Failing, 0);
        app.action(
            &ctx,
            Action::Project(Event::FollowUp {
                project: id,
                pane: docs,
                generation: 1,
                ask: FollowUp::Checks,
            }),
        );
        assert!(app.sessions.agents().drain_requests().is_empty());
        assert_eq!(cards(&app, id).len(), before);

        // "Changes" brings its terminal forward and shows what changed there.
        app.action(&ctx, Action::Project(Event::Changes(auth, 2)));
        assert_eq!(app.controller.model().active_pane(), Some(shell));
        app.action(&ctx, Action::Project(Event::Changes(auth, 1)));
        assert_eq!(app.controller.model().active_pane(), Some(auth));
        assert!(app.ui.panel.open && app.ui.panel.tab == ui::panel::Tab::Changes);
        assert_eq!(state_word(Doing::Ended, true), "Ended");
        assert_eq!(state_word(Doing::Asking, true), "Needs you");
    }

    #[test]
    fn a_lead_at_work_is_timed_and_is_thinking_until_something_of_its_turn_comes() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        // Whether it thinks, whether its turn is timed, and what it cost.
        let seen = |app: &App| {
            let panel = app.project_panel(true);
            let Body::Project(shown) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("the workspace in view has no project");
            };
            assert_eq!(shown.elapsed.is_some(), panel.running());
            (
                shown.chat.thinking,
                shown.elapsed.is_some(),
                shown.usage.map(str::to_owned),
            )
        };
        send(&mut app, &ctx, id, "go");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let lead = leads.borrow()[0].script.clone();
        lead.borrow_mut().ready("session-1");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        // Handed over and not begun: nothing is counted yet.
        assert_eq!(seen(&app), (false, false, None));
        lead.borrow_mut().begin();
        tick(&mut app, &ctx, start);
        assert_eq!(seen(&app), (true, true, None));
        // What it is writing, what it asks of Neptune and what it has said
        // each end the wait for its first word.
        lead.borrow_mut().stream = Stream {
            revision: 1,
            block: 1,
            text: "I am".into(),
        };
        tick(&mut app, &ctx, start);
        assert_eq!(seen(&app), (false, true, None));
        lead.borrow_mut().stream = Stream {
            revision: 2,
            block: 1,
            text: String::new(),
        };
        tick(&mut app, &ctx, start);
        assert!(seen(&app).0);
        lead.borrow_mut().events.push(LeadEvent::ToolStarted {
            call: "c1".into(),
            tool: "list_agents".into(),
            input: serde_json::Value::Null,
        });
        tick(&mut app, &ctx, start);
        assert_eq!(seen(&app), (false, true, None));
        lead.borrow_mut().events.push(LeadEvent::ToolDone {
            call: "c1".into(),
            ok: true,
        });
        lead.borrow_mut().say(1, "No agents yet.");
        // Its CLI made room in the conversation: the chat says so.
        lead.borrow_mut().events.push(LeadEvent::Compacted);
        tick(&mut app, &ctx, start);
        assert_eq!(seen(&app), (false, true, None));
        assert_eq!(
            notices(
                &app,
                id,
                "The lead summarised earlier conversation to make room. It is handed what the \
                 project keeps again."
            ),
            1
        );
        // The turn over, nothing is counted, and what its CLI put on it is
        // under the menu as an estimate.
        let turn = lead.borrow().turn();
        lead.borrow_mut().events.push(LeadEvent::TurnEnded {
            turn,
            outcome: Outcome::Completed,
            cost_usd: Some(0.0312),
        });
        lead.borrow_mut().state = Some(LeadState::Ready);
        tick(&mut app, &ctx, start);
        assert_eq!(seen(&app), (false, false, Some("$0.03".into())));
    }

    fn cd(app: &mut App, pane: PaneId, to: &std::path::Path) {
        let generation = app.controller.model().pane(pane).unwrap().generation();
        app.controller
            .dispatch(Command::PaneCwdChanged {
                pane,
                generation,
                cwd: to.into(),
            })
            .unwrap();
    }
    /// Frames until the project being made in `workspace` is in the model.
    fn made(app: &mut App, ctx: &egui::Context, workspace: WorkspaceId) -> ProjectId {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            tick(app, ctx, Instant::now());
            if let Some(project) = app.controller.model().project_of(workspace) {
                return project.id();
            }
            assert!(Instant::now() < deadline, "the project was never made");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[cfg_attr(windows, ignore = "its directories are written as on Unix")]
    #[test]
    fn a_project_works_where_the_terminal_in_front_was_when_it_was_made_and_stays_there() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, other) = (root.join("code/x"), root.join("elsewhere"));
        std::fs::create_dir_all(home.join("inner")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let (mut app, ctx, leads) = application(&root);
        let space = workspace(&mut app, &root, "shop");
        let shell = app.controller.model().active_pane().unwrap();
        let form = |app: &App| {
            let panel = app.project_panel(true);
            match app.projects.view(&panel, ui::chat::PAGE) {
                Body::Empty { directory, .. } => directory.to_owned(),
                _ => panic!("no form is in view"),
            }
        };
        // The form follows the terminal in front: its workspace's
        // directory first, then wherever its shell goes.
        let label = |path: &std::path::Path| ui::helpers::path_label(path, 40);
        assert_eq!(form(&app), label(&root));
        cd(&mut app, shell, &home);
        assert_eq!(form(&app), label(&home));
        assert_ne!(label(&home), label(&root));

        app.action(
            &ctx,
            Action::Project(Event::Create {
                workspace: space,
                goal: "Split checkout".into(),
                lead: AgentKind::Claude,
                model: None,
                effort: None,
            }),
        );
        // The terminal moves on while the folder is made: the project
        // works where it was asked for.
        cd(&mut app, shell, &other);
        let id = made(&mut app, &ctx, space);
        assert_eq!(app.projects.directory(id), Some(home.as_path()));
        assert_eq!(app.project_home(id), Some(home.clone()));
        let shown = |app: &App| {
            let panel = app.project_panel(true);
            let Body::Project(project) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("no project in view");
            };
            (
                project.directory.to_owned(),
                project.directory_path.to_path_buf(),
                project.lead,
            )
        };
        assert_eq!(shown(&app), (label(&home), home.clone(), AgentKind::Claude));
        // Its lead is told that directory, not its workspace's.
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        let lead = leads.borrow()[0].script.clone();
        lead.borrow_mut().ready("session-1");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let turn = lead.borrow().turns[0].1.clone();
        assert!(
            turn.contains(&format!("project: shop · directory: {}\n", home.display())),
            "{turn}"
        );
        lead.borrow_mut().begin();
        lead.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start);

        // Its agents start there when no directory is named, may work
        // inside it, and not in the workspace's directory around it.
        let task = || Task {
            title: Some("api".into()),
            ..Task::new(AgentKind::Codex, "x")
        };
        spawn(&app, 1, id, task(), PathBuf::new());
        let first = started(&mut app, &ctx, 1).unwrap();
        assert_eq!(app.controller.model().pane(first).unwrap().cwd(), home);
        spawn(&app, 2, id, task(), home.join("inner"));
        let second = started(&mut app, &ctx, 2).unwrap();
        assert_eq!(
            app.controller.model().pane(second).unwrap().cwd(),
            home.join("inner")
        );
        for (request, outside) in [(3, root.clone()), (4, other.clone())] {
            spawn(&app, request, id, task(), outside);
            let refused = started(&mut app, &ctx, request).unwrap_err();
            assert!(
                refused.contains("outside the project")
                    && refused.contains(&home.display().to_string()),
                "{refused}"
            );
        }
        // A worktree is asked of git from there as well: it is no
        // repository, and git says so of that directory.
        spawn(
            &app,
            5,
            id,
            Task {
                worktree: Some("fix-login".into()),
                ..task()
            },
            PathBuf::new(),
        );
        let refused = started(&mut app, &ctx, 5).unwrap_err();
        assert!(refused.contains("not in a git repository"), "{refused}");

        // What its folder keeps says where it works, and nothing else does.
        tick(&mut app, &ctx, Instant::now());
        let key = app.controller.model().project(id).unwrap().key().clone();
        let folder = root.join("projects").join(key.as_str());
        let window = serde_json::to_value(
            crate::persistence::workspace_state::StateSnapshot::from_model(app.controller.model()),
        )
        .unwrap();
        assert!(window["projects"][0].get("directory").is_none(), "{window}");
        close(app);
        let read = |folder: &std::path::Path| -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(folder.join("project.json")).unwrap()).unwrap()
        };
        assert_eq!(read(&folder)["directory"], home.to_string_lossy().as_ref());

        // After a restart it is read back from there, in a workspace whose
        // own directory is another.
        let again = |root: &std::path::Path| {
            let (mut app, ctx, leads) = application(root);
            let space = workspace(&mut app, root, "shop");
            app.controller
                .dispatch(Command::AddProject {
                    workspace: space,
                    name: "shop".into(),
                    key: key.clone(),
                    lead: AgentKind::Claude,
                })
                .unwrap();
            let id = app.controller.model().project_of(space).unwrap().id();
            (app, ctx, leads, space, id)
        };
        let (mut app, ctx, _leads, space, id) = again(&root);
        // Until it is read, its workspace's directory stands in.
        assert_eq!(app.projects.directory(id), None);
        assert_eq!(app.project_home(id), Some(root.clone()));
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(app.projects.directory(id), Some(home.as_path()));
        assert_eq!(shown(&app).1, home);

        // Its workspace is closed. It is offered again where it worked:
        // while the terminal in front is there, and not in the workspace's
        // own directory.
        let second = workspace(&mut app, &root, "next");
        let shell = app.controller.model().active_pane().unwrap();
        app.controller
            .dispatch(Command::CloseWorkspace(space))
            .unwrap();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let offered = |app: &App| match app.project_panel(true).body {
            PanelBody::Empty { reopen, .. } => reopen,
            _ => panic!("the workspace in view has a project"),
        };
        assert_eq!(offered(&app), None);
        app.action(&ctx, Action::Project(Event::Reopen(second)));
        assert!(app.controller.model().projects().is_empty());
        cd(&mut app, shell, &home);
        assert_eq!(offered(&app).as_deref(), Some("shop"));
        app.action(&ctx, Action::Project(Event::Reopen(second)));
        let id = app.controller.model().project_of(second).unwrap().id();
        cd(&mut app, shell, &other);
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(app.projects.directory(id), Some(home.as_path()));
        tick(&mut app, &ctx, start);
        close(app);

        // A folder that does not say where it worked, as one whose state
        // was lost, leaves it its workspace's directory, and keeps that.
        let mut saved = read(&folder);
        saved.as_object_mut().unwrap().remove("directory");
        std::fs::write(folder.join("project.json"), saved.to_string()).unwrap();
        let (mut app, ctx, _leads, _, id) = again(&root);
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        assert_eq!(app.projects.directory(id), Some(root.as_path()));
        assert!(app.projects.runs[&id].protected.is_none());
        tick(&mut app, &ctx, start);
        close(app);
        assert_eq!(read(&folder)["directory"], root.to_string_lossy().as_ref());
    }

    #[test]
    fn the_person_chooses_what_the_lead_and_its_agents_run_with_and_it_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, ctx, leads) = application(dir.path());
        let space = workspace(&mut app, dir.path(), "shop");
        let start = Instant::now();
        let make = |app: &mut App, model: Option<&str>, effort: Option<&str>| {
            app.action(
                &ctx,
                Action::Project(Event::Create {
                    workspace: space,
                    goal: "Split checkout".into(),
                    lead: AgentKind::Claude,
                    model: model.map(str::to_owned),
                    effort: effort.map(str::to_owned),
                }),
            );
        };
        // What no CLI takes makes no project: a lead has no ultra mode, and
        // a model's name is never an option.
        for (model, effort) in [
            (None, Some("ultra")),
            (None, Some("minimal")),
            (Some("--dangerous"), None),
            (Some("two words"), None),
        ] {
            make(&mut app, model, effort);
            assert!(app.ui.error.take().is_some(), "{model:?} {effort:?}");
            assert!(app.projects.creating.is_none());
        }
        make(&mut app, Some(" opus "), Some("High"));
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        let id = made(&mut app, &ctx, space);
        tick(&mut app, &ctx, start);
        assert_eq!(
            leads.borrow()[0].with,
            (Some("opus".into()), Some("high".into()))
        );
        let view = |app: &App| {
            let panel = app.project_panel(true);
            let Body::Project(project) = app.projects.view(&panel, ui::chat::PAGE) else {
                panic!("no project in view");
            };
            (
                project.settings.clone(),
                project.lead_model.map(str::to_owned),
            )
        };
        assert_eq!(view(&app).1, None);
        let lead = leads.borrow()[0].script.clone();
        lead.borrow_mut().events.extend([
            LeadEvent::Session {
                id: "session-1".into(),
            },
            LeadEvent::Ready {
                model: Some("claude-opus-5-5".into()),
                account_kind: None,
            },
        ]);
        lead.borrow_mut().state = Some(LeadState::Ready);
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        // The tab says what its lead runs on, as its CLI named it.
        assert_eq!(view(&app).1.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(lead.borrow().turns.len(), 1);
        assert!(!lead.borrow().turns[0].1.contains("agent settings"));

        // Changed while its lead is in a turn, nothing of the turn is
        // touched: the lead goes on as it was started.
        lead.borrow_mut().begin();
        let set = |app: &mut App, model: Option<&str>, effort: Option<&str>| {
            app.action(
                &ctx,
                Action::Project(Event::SetLead {
                    project: id,
                    model: model.map(str::to_owned),
                    effort: effort.map(str::to_owned),
                }),
            );
        };
        set(&mut app, Some("sonnet"), None);
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        for _ in 0..3 {
            tick(&mut app, &ctx, start);
        }
        assert!(!lead.borrow().dropped && leads.borrow().len() == 1);
        assert_eq!(lead.borrow().interrupts, 0);
        assert_eq!(view(&app).0.lead.model.as_deref(), Some("sonnet"));
        // The tab says that the lead has not taken the choice up yet.
        let pending = |app: &App| {
            let panel = app.project_panel(true);
            match app.projects.view(&panel, ui::chat::PAGE) {
                Body::Project(project) => project.pending,
                _ => panic!("no project in view"),
            }
        };
        assert!(pending(&app));
        lead.borrow_mut().say(1, "A plan.");
        lead.borrow_mut().end(Outcome::Completed);
        for _ in 0..3 {
            tick(&mut app, &ctx, start);
        }
        // At rest it still runs, until there is a turn for it: then it is
        // let go and started again in the conversation it had.
        assert!(!lead.borrow().dropped && leads.borrow().len() == 1);
        send(&mut app, &ctx, id, "Go ahead");
        tick(&mut app, &ctx, start);
        assert!(lead.borrow().dropped);
        assert_eq!(lead.borrow().turns.len(), 1, "the old lead took no more");
        assert_eq!(leads.borrow().len(), 2);
        assert_eq!(leads.borrow()[1].with, (Some("sonnet".into()), None));
        assert!(!pending(&app));
        assert_eq!(
            leads.borrow()[1].session,
            (Some("session-1".into()), true),
            "the same conversation, resumed"
        );
        let before = entries(&app, id);
        let lead = leads.borrow()[1].script.clone();
        lead.borrow_mut().ready("session-1");
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let turns = lead.borrow().turns.clone();
        assert_eq!(turns.len(), 1);
        assert!(turns[0].1.contains("Go ahead"), "{}", turns[0].1);
        // The chat is as it was, with the turn that was waiting after it.
        let said = entries(&app, id);
        assert_eq!(said[..before.len()], before[..]);
        assert!(
            said.iter()
                .any(|entry| matches!(entry, Entry::Lead { text, .. } if text == "A plan.")),
            "{said:?}"
        );
        // Set to what it already runs with, it is not started again.
        lead.borrow_mut().begin();
        lead.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start);
        set(&mut app, Some("sonnet"), None);
        send(&mut app, &ctx, id, "And then?");
        tick(&mut app, &ctx, start);
        assert_eq!(leads.borrow().len(), 2);
        assert_eq!(lead.borrow().turns.len(), 2);
        lead.borrow_mut().begin();
        lead.borrow_mut().end(Outcome::Completed);
        tick(&mut app, &ctx, start);
        // What its CLI does not take is said and changes nothing.
        set(&mut app, None, Some("ultra"));
        assert!(app.ui.error.take().is_some());
        set(&mut app, Some("-x"), None);
        assert!(app.ui.error.take().is_some());
        assert_eq!(view(&app).0.lead.model.as_deref(), Some("sonnet"));

        // For the agents its lead starts, by CLI. What is set wins over
        // what the lead asks for; what is not stays the lead's.
        let defaults = |app: &mut App, kind, model: Option<&str>, effort: Option<&str>, ultra| {
            app.action(
                &ctx,
                Action::Project(Event::SetAgentDefaults {
                    project: id,
                    kind,
                    model: model.map(str::to_owned),
                    effort: effort.map(str::to_owned),
                    ultracode: ultra,
                }),
            );
        };
        defaults(&mut app, AgentKind::Claude, None, Some("ultra"), None);
        assert!(
            app.ui.error.take().is_some(),
            "ultra is no effort of Claude Code"
        );
        defaults(&mut app, AgentKind::Gemini, None, None, None);
        assert!(app.ui.error.take().is_some());
        defaults(&mut app, AgentKind::Claude, Some("haiku"), None, Some(true));
        // Codex has no ultracode: its Ultra is an effort.
        defaults(&mut app, AgentKind::Codex, None, Some("ultra"), Some(true));
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        let kept = view(&app).0;
        assert_eq!(
            (kept.claude.clone(), kept.codex.clone()),
            (
                AgentSettings {
                    model: Some("haiku".into()),
                    effort: None,
                    ultracode: Some(true),
                },
                AgentSettings {
                    model: None,
                    effort: Some("ultra".into()),
                    ultracode: None,
                }
            )
        );
        let asked = |kind, model: Option<&str>, effort: Option<&str>, ultracode| Task {
            title: Some("api".into()),
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
            ultracode,
            ..Task::new(kind, "x")
        };
        let with = |app: &mut App, request, task: Task| {
            spawn(app, request, id, task, PathBuf::new());
            let pane = started(app, &ctx, request).unwrap();
            app.sessions.agents().started_as(pane).unwrap()
        };
        assert_eq!(
            with(
                &mut app,
                1,
                asked(AgentKind::Claude, Some("opus"), Some("high"), false)
            ),
            " on haiku at high effort with ultracode on"
        );
        assert_eq!(
            with(
                &mut app,
                2,
                asked(AgentKind::Codex, Some("gpt-6-sol"), Some("low"), false)
            ),
            " on gpt-6-sol at ultra effort"
        );
        // Ultracode turned off stays off whatever the lead asks.
        defaults(&mut app, AgentKind::Claude, None, None, Some(false));
        assert_eq!(
            with(&mut app, 3, asked(AgentKind::Claude, None, None, true)),
            ""
        );
        // Nothing set: the lead's own choice, its ultra mode among it.
        defaults(&mut app, AgentKind::Claude, None, None, None);
        assert_eq!(
            with(
                &mut app,
                4,
                asked(AgentKind::Claude, Some("opus"), Some("max"), true)
            ),
            " on opus at max effort with ultracode on"
        );
        defaults(&mut app, AgentKind::Claude, Some("haiku"), None, Some(true));
        // The lead's next turn says what is in force.
        send(&mut app, &ctx, id, "What do agents run with?");
        let limit = Instant::now() + Duration::from_secs(30);
        while lead.borrow().turns.len() < 3 {
            tick(&mut app, &ctx, Instant::now());
            assert!(Instant::now() < limit, "the turn was never handed over");
            std::thread::sleep(Duration::from_millis(10));
        }
        let turn = lead.borrow().turns.last().unwrap().1.clone();
        assert!(
            turn.contains(
                "agent settings (the user's; they win over what you pass to spawn_agent): \
                 claude: model haiku, ultracode on | codex: effort ultra\n"
            ),
            "{turn}"
        );

        // All of it is in the project's folder, and comes back from it.
        tick(&mut app, &ctx, Instant::now());
        let key = app.controller.model().project(id).unwrap().key().clone();
        let folder = dir.path().join("projects").join(key.as_str());
        close(app);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("project.json")).unwrap()).unwrap();
        // Each agent's record keeps what it was started with, so that one
        // opened again after a restart runs with the same.
        let ran: Vec<_> = saved["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|task| {
                (
                    task["model"].as_str(),
                    task["effort"].as_str(),
                    task["ultracode"].as_bool(),
                )
            })
            .collect();
        assert_eq!(
            ran,
            [
                (Some("haiku"), Some("high"), Some(true)),
                (Some("gpt-6-sol"), Some("ultra"), None),
                (None, None, None),
                (Some("opus"), Some("max"), Some(true)),
            ]
        );
        assert_eq!(
            saved["settings"],
            serde_json::json!({
                "lead": {"model": "sonnet"},
                "claude": {"model": "haiku", "ultracode": true},
                "codex": {"effort": "ultra"},
            })
        );
        let (mut app, ctx, leads) = application(dir.path());
        let space = workspace(&mut app, dir.path(), "shop");
        app.controller
            .dispatch(Command::AddProject {
                workspace: space,
                name: "shop".into(),
                key,
                lead: AgentKind::Claude,
            })
            .unwrap();
        let id = app.controller.model().project_of(space).unwrap().id();
        // Before it is read back there is nothing to change.
        app.action(
            &ctx,
            Action::Project(Event::SetLead {
                project: id,
                model: None,
                effort: None,
            }),
        );
        assert_eq!(app.ui.error.take().as_deref(), Some(SETTINGS_LOCKED));
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let kept: Settings = serde_json::from_value(saved["settings"].clone()).unwrap();
        assert_eq!(view(&app).0, kept);
        send(&mut app, &ctx, id, "hello");
        tick(&mut app, &ctx, start);
        assert_eq!(leads.borrow()[0].with, (Some("sonnet".into()), None));

        // A check's model is asked of every lead, whatever was chosen, and
        // a Codex lead is then asked to think little as well.
        app.projects.model = Some("cheap".into());
        let chosen = LeadSettings {
            model: Some("opus".into()),
            effort: Some("max".into()),
        };
        assert_eq!(
            app.projects.lead_settings(AgentKind::Claude, &chosen),
            LeadSettings {
                model: Some("cheap".into()),
                effort: None,
            }
        );
        assert_eq!(
            app.projects.lead_settings(AgentKind::Codex, &chosen),
            LeadSettings {
                model: Some("cheap".into()),
                effort: Some("low".into()),
            }
        );
        app.projects.model = None;
        assert_eq!(
            app.projects.lead_settings(AgentKind::Codex, &chosen),
            chosen
        );
    }

    #[test]
    fn the_settings_offer_each_clis_models_and_efforts_and_a_lead_no_ultra() {
        let catalog = catalog();
        let values = |choices: &Choices| -> Vec<&str> {
            choices.efforts.iter().map(|choice| choice.value).collect()
        };
        let of = |list: &[Choices], kind| {
            list.iter()
                .find(|choices| choices.kind == kind)
                .cloned()
                .unwrap()
        };
        assert_eq!(
            values(&of(&catalog.lead, AgentKind::Claude)),
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            values(&of(&catalog.lead, AgentKind::Codex)),
            ["minimal", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            values(&of(&catalog.agents, AgentKind::Codex)),
            ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"]
        );
        // Every level offered is one the rule that starts an agent takes,
        // and every model one that rule takes as a name.
        for choices in &catalog.agents {
            for effort in &choices.efforts {
                assert!(agents::effort_level(choices.kind, Some(effort.value), false).is_ok());
            }
            for model in &choices.models {
                assert_eq!(settings::model_name(model.value), Some(model.value));
            }
        }
        let claude = of(&catalog.lead, AgentKind::Claude);
        assert_eq!(
            claude
                .models
                .iter()
                .map(|choice| (choice.value, choice.label.as_str()))
                .collect::<Vec<_>>(),
            [
                ("fable", "Fable"),
                ("opus", "Opus"),
                ("sonnet", "Sonnet"),
                ("haiku", "Haiku")
            ]
        );
        assert_eq!(claude.efforts[3].label, "Extra high");
    }

    #[test]
    fn files_attached_to_a_message_reach_the_lead_with_it_and_stay_in_the_chat() {
        use crate::projects::transcript::Attachment;
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot 1.png");
        let log = dir.path().join("build.log");
        std::fs::write(&shot, b"\x89PNG\r\n\x1a\npixels").unwrap();
        std::fs::write(&log, b"error").unwrap();
        let attached = |path: &std::path::Path, bytes| Attachment {
            path: path.to_str().unwrap().into(),
            bytes,
        };
        let files = [attached(&shot, 14), attached(&log, 5)];
        let named = format!(
            "Attached files:\n- {} (image, 14 B)\n- {} (file, 5 B)\n",
            files[0].path, files[1].path
        );
        let message = |app: &mut App, ctx: &egui::Context, id, text: &str, files: &[Attachment]| {
            app.action(
                ctx,
                Action::Project(Event::Send {
                    project: id,
                    text: text.into(),
                    attachments: files.to_vec(),
                }),
            );
        };
        {
            let (mut app, ctx, leads, id) = reopen(dir.path(), 1);
            let start = Instant::now();
            tick(&mut app, &ctx, start);

            // Picked, pasted or dropped, files are looked at off the frame
            // and then wait over the field; what cannot go is said.
            let look = |app: &mut App, paths: Vec<PathBuf>| {
                app.ui.error = None;
                app.attach_files(&ctx, id, paths);
                let deadline = Instant::now() + Duration::from_secs(10);
                while app.ui.error.is_none() && !app.ui.project.focus {
                    app.poll_attachments(&ctx);
                    assert!(Instant::now() < deadline, "the files were not looked at");
                    std::thread::sleep(Duration::from_millis(5));
                }
            };
            look(
                &mut app,
                vec![
                    shot.clone(),
                    dir.path().into(),
                    dir.path().join("gone.txt"),
                    log.clone(),
                ],
            );
            assert_eq!(app.ui.project.attached[&id], files);
            let said = app.ui.error.take().unwrap();
            assert!(said.contains("is a folder") && said.contains("“gone.txt” could not be read."));
            assert!(std::mem::take(&mut app.ui.project.focus));
            // Attached again is attached once, and a message takes so many.
            look(&mut app, vec![shot.clone()]);
            assert_eq!(app.ui.project.attached[&id], files);
            app.ui.project.focus = false;
            let many: Vec<PathBuf> = (0..MAX_ATTACHMENTS)
                .map(|n| {
                    let path = dir.path().join(format!("{n}.txt"));
                    std::fs::write(&path, b"x").unwrap();
                    path
                })
                .collect();
            look(&mut app, many);
            assert_eq!(app.ui.project.attached[&id].len(), MAX_ATTACHMENTS);
            assert_eq!(
                app.ui.error.take().unwrap(),
                format!("A message takes at most {MAX_ATTACHMENTS} files.")
            );
            app.action(&ctx, Action::Project(Event::Attach(id)));
            assert!(
                app.ui.error.take().is_some(),
                "no picker for a full message"
            );

            // A message that is not taken keeps its files with its words.
            let long = "x".repeat(MAX_MESSAGE + 1);
            message(&mut app, &ctx, id, &long, &files);
            assert_eq!(app.ui.project.drafts[&id], long);
            assert_eq!(app.ui.project.attached[&id], files);
            assert!(app.ui.error.take().is_some());
            assert!(entries(&app, id).is_empty());

            // Sent, they are an entry of the chat, and the lead is told
            // where each is and shown the pictures among them.
            message(&mut app, &ctx, id, "Why is it cut off?", &files);
            let first = Entry::User {
                text: "Why is it cut off?".into(),
                attachments: files.to_vec(),
            };
            assert_eq!(entries(&app, id), std::slice::from_ref(&first));
            tick(&mut app, &ctx, start);
            let lead = leads.borrow()[0].script.clone();
            lead.borrow_mut().ready("session-1");
            tick(&mut app, &ctx, start);
            tick(&mut app, &ctx, start);
            {
                let lead = lead.borrow();
                assert_eq!(lead.turns.len(), 1);
                assert!(
                    lead.turns[0]
                        .1
                        .ends_with(&format!("\nWhy is it cut off?\n{named}")),
                    "{}",
                    lead.turns[0].1
                );
                assert_eq!(lead.pictures, std::slice::from_ref(&shot));
            }
            // Files alone are a message. Sent while the lead works, it
            // waits with them, and a new chat does not take what waits
            // over the field.
            lead.borrow_mut().begin();
            tick(&mut app, &ctx, start);
            message(&mut app, &ctx, id, "", &files[..1]);
            message(&mut app, &ctx, id, "and the log", &files[1..]);
            assert_eq!(shown(&app).4, 2);
            lead.borrow_mut().end(Outcome::Completed);
            tick(&mut app, &ctx, start);
            tick(&mut app, &ctx, start);
            {
                let lead = lead.borrow();
                assert_eq!(lead.turns.len(), 2);
                assert!(
                    lead.turns[1].1.ends_with(&format!(
                        "\nAttached files:\n- {} (image, 14 B)\n\nand the log\nAttached \
                         files:\n- {} (file, 5 B)\n",
                        files[0].path, files[1].path
                    )),
                    "{}",
                    lead.turns[1].1
                );
                assert_eq!(lead.pictures, std::slice::from_ref(&shot));
            }
            // The lead never begins that turn: Neptune closes.
            close(app);
        }
        // The chat is read back with every message's files, and holds no
        // more of them than where they are.
        let saved = std::fs::read_to_string(folder_of(dir.path(), 1).join("chat.jsonl")).unwrap();
        assert!(!saved.contains("pixels") && saved.contains("shot 1.png"));
        let (mut app, ctx, _leads, id) = reopen(dir.path(), 1);
        let start = Instant::now();
        tick(&mut app, &ctx, start);
        tick(&mut app, &ctx, start);
        let users: Vec<Entry> = entries(&app, id)
            .into_iter()
            .filter(|entry| matches!(entry, Entry::User { .. }))
            .collect();
        assert_eq!(
            users,
            [
                Entry::User {
                    text: "Why is it cut off?".into(),
                    attachments: files.to_vec(),
                },
                Entry::User {
                    text: String::new(),
                    attachments: files[..1].to_vec(),
                },
                Entry::User {
                    text: "and the log".into(),
                    attachments: files[1..].to_vec(),
                },
            ]
        );
        assert_eq!(notices(&app, id, transcript::CLOSED_UNSENT), 1);
        // A picture that has gone since is still an entry to draw.
        std::fs::remove_file(&shot).unwrap();
        app.action(&ctx, Action::Project(Event::Compose));
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        // What waits over the field outlasts a new chat.
        app.ui.project.attached.insert(id, files.to_vec());
        app.action(&ctx, Action::Project(Event::NewChat(id)));
        tick(&mut app, &ctx, start);
        assert_eq!(app.ui.project.attached[&id], files);
    }
}
