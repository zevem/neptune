//! One headless CLI per live project lead, owned off the frame.
//! A pure driver turns the CLI's lines into events; three named threads own
//! the process, and every queue between them and the frame is bounded.
//! Nothing here is logged: a frame is the conversation.
mod claude;
mod codex;

use super::agents::{self, LeadEnv};
use crate::projects::{prompt::unseen, transcript::MAX_PICTURE};
use neptune_model::AgentKind;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    io::{BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

pub type Wake = Arc<dyn Fn() + Send + Sync>;
/// Opens the lead to the application. It may bind the bridge's socket, so
/// the worker calls it.
pub type Opener = Box<dyn FnOnce() -> std::io::Result<LeadEnv> + Send>;

/// Events the frame has not taken; past this the CLI itself is held back.
const EVENTS: usize = 64;
/// Lines and requests waiting for the CLI's input.
const INPUTS: usize = 8;
/// The text of the block being written, and of one finished block.
const MAX_STREAM: usize = 256 * 1024;
/// What a tool was called with, as carried to the chat.
const MAX_INPUT: usize = 4 * 1024;
/// One turn: a message, or every event that waited for the lead.
pub const MAX_TURN: usize = 256 * 1024;
/// The pictures of one turn, by the size of their files: as text they stay
/// well inside what a provider takes in one request.
const MAX_PICTURES: u64 = 15 * 1024 * 1024;
/// One line of the CLI. A longer one is passed over, not kept.
const MAX_FRAME: u64 = 8 * 1024 * 1024;
/// The end of what the CLI said on its error stream, for the chat only.
const MAX_STDERR: usize = 2 * 1024;
/// Streaming text repaints at most this often.
const DELTA_WAKE: Duration = Duration::from_millis(50);
const FIND: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);
/// A first open of a session another process still holds; it is resumed.
const IN_USE: &str = "already in use";
const NO_SESSION: &str = "No conversation found";

const STARTING: u8 = 0;
const READY: u8 = 1;
const BUSY: u8 = 2;
const DEAD: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Being found, started and asked what it can do.
    Starting,
    /// Takes a turn.
    Ready,
    /// In a turn; nothing is written to it but an interrupt.
    Busy,
    /// Gone. A new lead resumes the session.
    Dead,
}
#[derive(Debug, Clone, PartialEq)]
pub enum LeadEvent {
    /// The session the lead runs in, known before the CLI starts.
    Session {
        id: String,
    },
    /// Signed in, with Neptune's tools connected. `account_kind` is the
    /// plan's name, such as "Claude Max"; never who is signed in.
    Ready {
        model: Option<String>,
        account_kind: Option<String>,
    },
    /// The lead never became ready. `Exited` follows.
    Failed(Failure),
    TurnStarted {
        turn: String,
    },
    /// A finished block of text. What streams until then is `Lead::stream`.
    TextDone {
        block: u32,
        text: String,
    },
    ToolStarted {
        call: String,
        tool: String,
        input: Value,
    },
    ToolDone {
        call: String,
        ok: bool,
    },
    Retry {
        attempt: u32,
        max: u32,
        delay_ms: u64,
    },
    Limit {
        resets_at: Option<u64>,
    },
    Compacted,
    /// `cost_usd` is the CLI's own estimate for this turn.
    TurnEnded {
        turn: String,
        outcome: Outcome,
        cost_usd: Option<f64>,
    },
    Exited {
        code: Option<i32>,
    },
}
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Completed,
    Interrupted,
    Failed(Failure),
}
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    NotInstalled,
    Auth,
    /// Neptune's tool server did not connect: the lead could only talk.
    ToolsUnavailable,
    Limit {
        resets_at: Option<u64>,
    },
    Overloaded,
    PromptTooLong,
    /// The session to resume is no longer on this computer.
    SessionLost,
    Transport,
    Protocol,
    /// The provider's own words, for the chat and nowhere else.
    Provider(String),
}
impl Failure {
    /// The closed name diagnostics may carry.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotInstalled => "NotInstalled",
            Self::Auth => "Auth",
            Self::ToolsUnavailable => "ToolsUnavailable",
            Self::Limit { .. } => "Limit",
            Self::Overloaded => "Overloaded",
            Self::PromptTooLong => "PromptTooLong",
            Self::SessionLost => "SessionLost",
            Self::Transport => "Transport",
            Self::Protocol => "Protocol",
            Self::Provider(_) => "Provider",
        }
    }
}
/// Why a turn was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Starting,
    /// A turn is running. A message written now would be folded into it.
    Busy,
    Dead,
    TooLong,
}
pub struct SessionRef {
    /// Made on the worker when the project has none yet.
    pub id: Option<String>,
    /// A handshake succeeded in it once: it is resumed, not opened.
    pub opened: bool,
}
pub struct Mcp {
    /// This application, which serves the lead's tools.
    pub exe: PathBuf,
    pub open: Opener,
}
pub struct Launch {
    pub kind: AgentKind,
    /// The CLI to run; looked for on the worker when not given.
    pub exe: Option<PathBuf>,
    /// The project's folder under the data root, never the repository.
    pub project_dir: PathBuf,
    pub session: SessionRef,
    pub system_prompt: String,
    pub mcp: Mcp,
    /// The model the lead is asked for; its CLI's own choice without one.
    pub model: Option<String>,
    /// How hard its model is asked to think, one of `effort_levels`.
    pub effort: Option<String>,
}
/// The effort levels a lead of `kind` can be given: those of the agents
/// Neptune starts with that CLI, without Codex's Ultra. Ultra is "maximum
/// reasoning with automatic task delegation": a lead delegates through
/// Neptune's tools, where the person sees each agent, and its own
/// multi-agent features are turned off. Claude Code's ultracode is left out
/// for the same reason: it orchestrates with tools a lead is not given.
pub fn effort_levels(kind: AgentKind) -> Vec<&'static str> {
    super::agents::effort_levels(kind)
        .iter()
        .copied()
        .filter(|level| *level != "ultra")
        .collect()
}
/// The text of the block the lead is writing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stream {
    /// Changes whenever the rest does.
    pub revision: u64,
    /// The block of this turn the text belongs to; its `TextDone` replaces it.
    pub block: u32,
    pub text: String,
}

/// What one process is started with.
struct Spawn<'a> {
    exe: &'a Path,
    /// This application, which serves the lead's tools.
    neptune: &'a Path,
    /// The file that names the lead's tool server and its credential, for a
    /// CLI that reads one.
    config: Option<&'a Path>,
    dir: &'a Path,
    /// None where the CLI names its own and has not yet.
    session: Option<&'a str>,
    resume: bool,
    env: &'a LeadEnv,
    model: Option<&'a str>,
    effort: Option<&'a str>,
}
/// What reading one line produced.
#[derive(Default)]
struct Output {
    events: Vec<LeadEvent>,
    /// Lines the CLI is owed in answer.
    replies: Vec<String>,
    /// Streamed text, by block. A block opens with an empty one.
    deltas: Vec<(u32, String)>,
    /// Call `tick` this much later.
    again: Option<Duration>,
}
/// A provider's protocol: lines in, lines out, no I/O.
trait Driver: Send {
    /// The tool servers the CLI is given, as the file it reads them from.
    /// None for a CLI that is told them over its input.
    fn config(&self, neptune: &Path, env: &LeadEnv) -> Option<String>;
    fn command(&mut self, spawn: &Spawn<'_>) -> Command;
    fn hello(&mut self) -> Vec<String>;
    /// Pictures are part of what the CLI is written, not files it opens.
    fn embeds(&self) -> bool {
        false
    }
    /// Nothing is returned unless the provider is idle.
    fn turn(&mut self, id: &str, text: &str, pictures: &[Shown]) -> Vec<String>;
    fn interrupt(&mut self) -> Vec<String>;
    /// An interrupt was asked for and is written once the CLI can take it:
    /// the turn is held to the same limit as one that was written.
    fn deferred(&self) -> bool {
        false
    }
    fn tick(&mut self) -> Vec<String>;
    fn read(&mut self, line: &str, out: &mut Output);
    /// The process is gone: what it left unfinished ends here.
    fn eof(&mut self, out: &mut Output);
}
fn driver(
    kind: AgentKind,
    prompt: &str,
    seed: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Option<Box<dyn Driver>> {
    match kind {
        AgentKind::Claude => Some(Box::new(claude::Claude::new(prompt, seed, model))),
        AgentKind::Codex => Some(Box::new(codex::Codex::new(prompt, model, effort))),
        _ => None,
    }
}
/// The inherited environment without what another caller of the bridge
/// would hand the lead, and without Neptune's adapters on `PATH`. `more`
/// names what else a provider's CLI must not inherit.
fn scrub(
    command: &mut Command,
    inherited: impl Iterator<Item = (OsString, OsString)>,
    more: impl Fn(&str) -> bool,
) {
    let (mut shims, mut path) = (None, None);
    for (name, value) in inherited {
        let Some(text) = name.to_str() else {
            continue;
        };
        if text == agents::SHIMS {
            shims = Some(PathBuf::from(&value));
        }
        if text == "PATH" {
            path = Some(value);
        } else if text.starts_with("NEPTUNE_AGENT_") || more(text) {
            command.env_remove(name);
        }
    }
    if let (Some(shims), Some(path)) = (shims, path)
        && let Ok(path) =
            std::env::join_paths(std::env::split_paths(&path).filter(|folder| *folder != shims))
    {
        command.env("PATH", path);
    }
}

/// A picture as a lead's CLI is handed it with a turn.
struct Shown {
    path: String,
    media: &'static str,
    /// What the file holds in base64, for a CLI that takes it so.
    data: String,
}
/// The kind of picture a file is, by how it begins.
fn media(head: &[u8]) -> Option<&'static str> {
    match head {
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => Some("image/png"),
        [0xff, 0xd8, 0xff, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    }
}
fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |group, (at, byte)| {
            group | u32::from(*byte) << (16 - 8 * at)
        });
        for digit in 0..4 {
            out.push(if digit <= chunk.len() {
                DIGITS[(group >> (18 - 6 * digit)) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}
/// Looks at one picture of a turn. `room` is what the turn can still take.
fn picture(path: &Path, embed: bool, room: &mut u64) -> Result<Shown, &'static str> {
    const GONE: &str = "it is no longer there or cannot be read";
    let name = path.to_str().ok_or(GONE)?;
    let file = std::fs::File::open(path).map_err(|_| GONE)?;
    let size = file
        .metadata()
        .ok()
        .filter(|about| about.is_file())
        .ok_or(GONE)?
        .len();
    if size > MAX_PICTURE {
        return Err("it is too large");
    }
    if size > *room {
        return Err("this turn has as many pictures as fit already");
    }
    // A CLI that opens the file itself is still spared one that is none.
    let mut bytes = Vec::new();
    let most = if embed { MAX_PICTURE + 1 } else { 12 };
    file.take(most).read_to_end(&mut bytes).map_err(|_| GONE)?;
    if bytes.len() as u64 > MAX_PICTURE {
        // It grew since its size was read.
        return Err("it is too large");
    }
    let media = media(&bytes).ok_or("it is not a PNG, JPEG, GIF or WebP picture")?;
    *room -= size;
    Ok(Shown {
        path: name.to_owned(),
        media,
        data: if embed { base64(&bytes) } else { String::new() },
    })
}
/// The pictures of a turn as its CLI takes them. One that cannot be shown
/// is left out, and `text` tells the lead so: its path is still in the
/// message, for an agent to read.
fn pictures(paths: &[PathBuf], embed: bool, text: &mut String) -> Vec<Shown> {
    let mut room = MAX_PICTURES;
    let mut shown = Vec::new();
    for path in paths {
        match picture(path, embed, &mut room) {
            Ok(picture) => shown.push(picture),
            Err(why) => {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&unseen(&path.to_string_lossy(), why));
            }
        }
    }
    shown
}

enum Input {
    Line(String),
    Turn {
        id: String,
        text: String,
        /// The pictures the person attached, where they are.
        pictures: Vec<PathBuf>,
    },
    Interrupt,
    Tick(Instant),
    Wake(Instant),
    /// The handshake is done: the CLI has read what it was started with.
    Ready,
    /// Start-up failed and was said so.
    Quit,
    /// The CLI closed its output. Always the reader's last word.
    Eof,
    Stop,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reason {
    Eof,
    Quit,
    /// No answer to a handshake or an interrupt in time.
    Hung,
    Stop,
}
#[derive(Clone, Copy)]
struct Patience {
    handshake: Duration,
    interrupt: Duration,
    /// After its input closes, before it is asked to end.
    close: Duration,
    /// After it is asked to end, before it is killed.
    term: Duration,
}
impl Default for Patience {
    fn default() -> Self {
        Self {
            handshake: Duration::from_secs(25),
            interrupt: Duration::from_secs(3),
            close: Duration::from_secs(2),
            term: Duration::from_secs(5),
        }
    }
}
struct Shared {
    state: AtomicU8,
    stop: AtomicBool,
    /// Turns that ended, so a late interrupt deadline knows its turn is over.
    turns: AtomicU64,
    started: Instant,
    /// When the frame was last woken, in milliseconds since `started`.
    woke: AtomicU64,
    /// A wake for streamed text is waiting on the writer.
    armed: AtomicBool,
    /// The writer is in a write, which a CLI that stopped reading never ends.
    writing: AtomicBool,
    /// Held while a turn's start or end is published, so they keep their order.
    order: Mutex<()>,
    stream: Mutex<Stream>,
    stderr: Mutex<String>,
    /// The lead's credentials for Neptune's tools: a CLI that repeats what
    /// it was handed must not put them in the chat.
    secrets: Mutex<Vec<String>>,
    child: Mutex<Option<Child>>,
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Lead {
    events: Receiver<LeadEvent>,
    input: SyncSender<Input>,
    shared: Arc<Shared>,
}
impl Lead {
    /// Starts a lead off the frame. `wake` is called for every event and at
    /// most every 50 ms while text streams.
    pub fn start(launch: Launch, wake: Wake) -> Self {
        Self::start_with(launch, wake, Patience::default())
    }
    fn start_with(launch: Launch, wake: Wake, patience: Patience) -> Self {
        let (publish, events) = sync_channel(EVENTS);
        let (input, inputs) = sync_channel(INPUTS);
        let shared = Arc::new(Shared {
            state: AtomicU8::new(STARTING),
            stop: AtomicBool::new(false),
            turns: AtomicU64::new(0),
            started: Instant::now(),
            woke: AtomicU64::new(0),
            armed: AtomicBool::new(false),
            writing: AtomicBool::new(false),
            order: Mutex::new(()),
            stream: Mutex::default(),
            stderr: Mutex::default(),
            secrets: Mutex::default(),
            child: Mutex::new(None),
        });
        let worker = Worker {
            wake,
            events: publish.clone(),
            input: input.clone(),
            inputs,
            shared: shared.clone(),
            patience,
            eof: Cell::new(false),
            private: RefCell::new(None),
        };
        let started = thread::Builder::new()
            .name("neptune-lead-write".into())
            .spawn(move || worker.run(launch));
        if started.is_err() {
            shared.state.store(DEAD, Ordering::Release);
            let _ = publish.try_send(LeadEvent::Failed(Failure::Transport));
            let _ = publish.try_send(LeadEvent::Exited { code: None });
        }
        Self {
            events,
            input,
            shared,
        }
    }
    pub fn state(&self) -> State {
        match self.shared.state.load(Ordering::Acquire) {
            STARTING => State::Starting,
            READY => State::Ready,
            BUSY => State::Busy,
            _ => State::Dead,
        }
    }
    /// Hands the lead one turn. Only an idle lead takes one: the caller
    /// keeps what waits until `TurnEnded`. Taken is not yet written:
    /// `TurnStarted` says it was, and a lead that exits without one never
    /// had the turn. `pictures` are read by the lead's own thread, never
    /// by the caller's.
    pub fn send_turn(&self, id: &str, text: &str, pictures: &[PathBuf]) -> Result<(), Refused> {
        if text.len() > MAX_TURN {
            return Err(Refused::TooLong);
        }
        let state = &self.shared.state;
        match state.compare_exchange(READY, BUSY, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {}
            Err(STARTING) => return Err(Refused::Starting),
            Err(BUSY) => return Err(Refused::Busy),
            Err(_) => return Err(Refused::Dead),
        }
        let turn = Input::Turn {
            id: id.to_owned(),
            text: text.to_owned(),
            pictures: pictures.to_vec(),
        };
        self.input.try_send(turn).map_err(|error| {
            let _ = state.compare_exchange(BUSY, READY, Ordering::AcqRel, Ordering::Acquire);
            match error {
                TrySendError::Full(_) => Refused::Busy,
                TrySendError::Disconnected(_) => Refused::Dead,
            }
        })
    }
    /// Asks the running turn to stop. Its `TurnEnded` says it did; a CLI
    /// that does not answer is stopped instead.
    pub fn interrupt(&self) {
        if self.state() == State::Busy {
            let _ = self.input.try_send(Input::Interrupt);
        }
    }
    /// What happened since the last call, in order. Never waits.
    pub fn try_events(&self) -> Vec<LeadEvent> {
        self.events.try_iter().take(EVENTS).collect()
    }
    pub fn revision(&self) -> u64 {
        lock(&self.shared.stream).revision
    }
    pub fn stream(&self) -> Stream {
        lock(&self.shared.stream).clone()
    }
    /// The end of what the CLI said on its error stream. It may name paths:
    /// for the chat's error row, never for diagnostics.
    pub fn last_error(&self) -> Option<String> {
        let mut said = lock(&self.shared.stderr).trim().to_owned();
        for secret in lock(&self.shared.secrets).iter() {
            said = said.replace(secret, "…");
            // Only the end is kept, and it may begin inside one: what is
            // left of it is no less private.
            if let Some(rest) = (1..secret.len().saturating_sub(3))
                .filter(|at| secret.is_char_boundary(*at))
                .find(|at| said.starts_with(&secret[*at..]))
                .map(|at| secret.len() - at)
            {
                said.replace_range(..rest, "…");
            }
        }
        (!said.is_empty()).then_some(said)
    }
}
impl Drop for Lead {
    /// Never waits: the writer closes the CLI's input, then asks it to end,
    /// then kills it.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        let full = matches!(self.input.try_send(Input::Stop), Err(TrySendError::Full(_)));
        if (full || self.shared.writing.load(Ordering::Acquire))
            && let Ok(mut child) = self.shared.child.try_lock()
            && let Some(child) = child.as_mut()
        {
            // The writer is held by a CLI that stopped reading, and would
            // never come to the ladder.
            let _ = child.kill();
        }
    }
}

struct Worker {
    wake: Wake,
    events: SyncSender<LeadEvent>,
    input: SyncSender<Input>,
    inputs: Receiver<Input>,
    shared: Arc<Shared>,
    patience: Patience,
    /// The reader of the current process said its last word.
    eof: Cell<bool>,
    /// The folder a starting CLI reads the lead's credential from.
    private: RefCell<Option<tempfile::TempDir>>,
}
impl Worker {
    fn emit(&self, event: LeadEvent) {
        {
            let _order = lock(&self.shared.order);
            let _ = self.events.send(event);
        }
        woke(&self.shared);
        (self.wake)();
    }
    fn fail(&self, failure: Failure) {
        self.shared.state.store(DEAD, Ordering::Release);
        self.emit(LeadEvent::Failed(failure));
        self.emit(LeadEvent::Exited { code: None });
    }
    fn write(&self, stdin: &mut ChildStdin, lines: &[String]) {
        // A CLI that is gone is noticed where its output ends.
        self.shared.writing.store(true, Ordering::Release);
        for line in lines {
            let _ = stdin.write_all(line.as_bytes());
            let _ = stdin.write_all(b"\n");
        }
        let _ = stdin.flush();
        self.shared.writing.store(false, Ordering::Release);
    }
    fn stopping(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }
    fn run(self, launch: Launch) {
        let Launch {
            kind,
            exe,
            project_dir,
            session,
            system_prompt,
            mcp: Mcp { exe: neptune, open },
            model,
            effort,
        } = launch;
        let Some(exe) = exe.or_else(|| find(kind, &Search::from_env())) else {
            return self.fail(Failure::NotInstalled);
        };
        // Claude Code is told its session's name; Codex names its own, and
        // its driver says it once the CLI has.
        let id = match session.id {
            None if kind != AgentKind::Codex => match uuid() {
                Ok(id) => Some(id),
                Err(_) => return self.fail(Failure::Transport),
            },
            id => id,
        };
        if let Some(id) = &id {
            self.emit(LeadEvent::Session { id: id.clone() });
        }
        if self.stopping() {
            return self.shared.state.store(DEAD, Ordering::Release);
        }
        let Ok(env) = open() else {
            return self.fail(Failure::Transport);
        };
        *lock(&self.shared.secrets) = vec![env.token.clone(), env.run.clone()];
        let mut resume = session.opened;
        loop {
            // A provider without a driver here cannot be a lead yet.
            let Some(mut driver) = driver(
                kind,
                &system_prompt,
                &env.run,
                model.as_deref(),
                effort.as_deref(),
            ) else {
                return self.fail(Failure::Protocol);
            };
            let config = match driver.config(&neptune, &env) {
                Some(text) => match self.hand_over(&text) {
                    Some(config) => Some(config),
                    None => return self.fail(Failure::Transport),
                },
                None => None,
            };
            let command = driver.command(&Spawn {
                exe: &exe,
                neptune: &neptune,
                config: config.as_deref(),
                dir: &project_dir,
                session: id.as_deref(),
                resume,
                env: &env,
                model: model.as_deref(),
                effort: effort.as_deref(),
            });
            if !self.attempt(command, driver, resume, kind == AgentKind::Claude) {
                return;
            }
            resume = true;
        }
    }
    /// Writes what a starting CLI is handed where only this user reads it.
    /// It names the lead's credential, so it is kept no longer than the
    /// start-up that reads it: neither for as long as the lead runs nor
    /// behind an application that ends before this worker does.
    fn hand_over(&self, text: &str) -> Option<PathBuf> {
        let mut private = self.private.borrow_mut();
        if private.is_none() {
            *private = private_dir().ok();
        }
        write_private(private.as_ref()?.path(), text).ok()
    }
    /// Runs one process to its end. True when it must be started again,
    /// resuming the session it could not open: `named` is a CLI that was
    /// told its session's name, which another process may hold.
    fn attempt(
        &self,
        mut command: Command,
        driver: Box<dyn Driver>,
        resumed: bool,
        named: bool,
    ) -> bool {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.fail(if error.kind() == std::io::ErrorKind::NotFound {
                    Failure::NotInstalled
                } else {
                    Failure::Transport
                });
                return false;
            }
        };
        let pid = child.id();
        let (mut stdin, stdout, stderr) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take());
        self.eof.set(false);
        lock(&self.shared.stderr).clear();
        *lock(&self.shared.child) = Some(child);
        let driver = Arc::new(Mutex::new(driver));
        let reader = stdout.and_then(|stdout| {
            let reader = Reader {
                driver: driver.clone(),
                shared: self.shared.clone(),
                events: self.events.clone(),
                input: self.input.clone(),
                wake: self.wake.clone(),
            };
            thread::Builder::new()
                .name("neptune-lead-read".into())
                .spawn(move || reader.run(stdout))
                .ok()
        });
        let errors = stderr.and_then(|stderr| {
            let shared = self.shared.clone();
            thread::Builder::new()
                .name("neptune-lead-err".into())
                .spawn(move || keep_tail(stderr, &shared))
                .ok()
        });
        let reason = match (&reader, stdin.as_mut()) {
            (Some(_), Some(stdin)) => {
                let hello = lock(&driver).hello();
                self.write(stdin, &hello);
                self.pump(stdin, &driver)
            }
            _ => Reason::Hung,
        };
        drop(stdin);
        let code = self.reap(pid);
        // Unsettled, something it started still holds its output open: the
        // reader is left to end with it.
        if let Some(reader) = reader
            && self.settled(pid)
        {
            let _ = reader.join();
        }
        if let Some(errors) = errors {
            let until = Instant::now() + Duration::from_millis(500);
            while !errors.is_finished() && Instant::now() < until {
                thread::sleep(POLL);
            }
        }
        *lock(&self.shared.child) = None;
        if self.stopping() {
            self.shared.state.store(DEAD, Ordering::Release);
            return false;
        }
        let starting = self.shared.state.load(Ordering::Acquire) == STARTING;
        let said = lock(&self.shared.stderr).clone();
        if named && starting && !resumed && reason == Reason::Eof && said.contains(IN_USE) {
            return true;
        }
        let mut out = Output::default();
        lock(&driver).eof(&mut out);
        self.shared.state.store(DEAD, Ordering::Release);
        for event in out.events {
            self.emit(match event {
                LeadEvent::Failed(Failure::Transport) if resumed && said.contains(NO_SESSION) => {
                    LeadEvent::Failed(Failure::SessionLost)
                }
                event => event,
            });
        }
        self.emit(LeadEvent::Exited { code });
        false
    }
    /// Owns the CLI's input until the process or the lead ends.
    fn pump(&self, stdin: &mut ChildStdin, driver: &Mutex<Box<dyn Driver>>) -> Reason {
        let shared = &self.shared;
        // The turn count an interrupt was sent at; none for the handshake.
        let mut limit = Some((Instant::now() + self.patience.handshake, None::<u64>));
        let (mut tick, mut wake) = (None::<Instant>, None::<Instant>);
        loop {
            if self.stopping() {
                return Reason::Stop;
            }
            let next = [limit.map(|(at, _)| at), tick, wake]
                .into_iter()
                .flatten()
                .min();
            let item = match next {
                Some(at) => self
                    .inputs
                    .recv_timeout(at.saturating_duration_since(Instant::now())),
                None => self
                    .inputs
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            match item {
                Ok(Input::Line(line)) => self.write(stdin, &[line]),
                Ok(Input::Turn {
                    id,
                    mut text,
                    pictures: paths,
                }) => {
                    {
                        let mut stream = lock(&shared.stream);
                        stream.block = 0;
                        stream.text.clear();
                        stream.revision += 1;
                    }
                    let embed = lock(driver).embeds();
                    let shown = pictures(&paths, embed, &mut text);
                    let lines = lock(driver).turn(&id, &text, &shown);
                    if lines.is_empty() {
                        // Not idle after all: nothing was written, nothing runs.
                        let _ = shared.state.compare_exchange(
                            BUSY,
                            READY,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                    } else {
                        self.emit(LeadEvent::TurnStarted { turn: id });
                        self.write(stdin, &lines);
                    }
                }
                Ok(Input::Interrupt) => {
                    if shared.state.load(Ordering::Acquire) == BUSY && limit.is_none() {
                        // Asked for a turn that ended since, nothing is written
                        // and the turn waiting behind it is not held to a limit.
                        let (lines, deferred) = {
                            let mut driver = lock(driver);
                            (driver.interrupt(), driver.deferred())
                        };
                        if !lines.is_empty() || deferred {
                            self.write(stdin, &lines);
                            limit = Some((
                                Instant::now() + self.patience.interrupt,
                                Some(shared.turns.load(Ordering::Acquire)),
                            ));
                        }
                    }
                }
                Ok(Input::Tick(at)) => tick = Some(at),
                Ok(Input::Wake(at)) => wake = Some(at),
                Ok(Input::Ready) => drop(self.private.take()),
                Ok(Input::Quit) => return Reason::Quit,
                Ok(Input::Eof) => {
                    self.eof.set(true);
                    return Reason::Eof;
                }
                Ok(Input::Stop) | Err(RecvTimeoutError::Disconnected) => return Reason::Stop,
                Err(RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if wake.is_some_and(|at| at <= now) {
                wake = None;
                shared.armed.store(false, Ordering::Release);
                woke(shared);
                (self.wake)();
            }
            if tick.is_some_and(|at| at <= now) {
                tick = None;
                let lines = lock(driver).tick();
                self.write(stdin, &lines);
            }
            if let Some((at, turn)) = limit {
                let state = shared.state.load(Ordering::Acquire);
                let answered = match turn {
                    None => state != STARTING,
                    Some(turn) => state != BUSY || shared.turns.load(Ordering::Acquire) != turn,
                };
                if answered {
                    limit = None;
                } else if at <= now {
                    return Reason::Hung;
                }
            }
        }
    }
    /// What the reader sends while nobody writes is taken, so it never
    /// stalls on a full queue with output still to read.
    fn drain(&self) {
        while let Ok(item) = self.inputs.try_recv() {
            if matches!(item, Input::Eof) {
                self.eof.set(true);
            }
        }
    }
    /// The shutdown ladder: its input is closed by now; then it is asked to
    /// end; then it is killed.
    fn reap(&self, pid: u32) -> Option<i32> {
        let wait = |limit: Duration| {
            let until = Instant::now() + limit;
            loop {
                if let Some(child) = lock(&self.shared.child).as_mut()
                    && let Ok(Some(status)) = child.try_wait()
                {
                    return Some(status);
                }
                if Instant::now() >= until {
                    return None;
                }
                self.drain();
                thread::sleep(POLL);
            }
        };
        wait(self.patience.close)
            .or_else(|| {
                signal(pid, "-TERM");
                wait(self.patience.term)
            })
            .or_else(|| {
                signal(pid, "-KILL");
                let mut child = lock(&self.shared.child).take()?;
                let _ = child.kill();
                child.wait().ok()
            })
            .and_then(|status| status.code())
    }
    /// Waits for the reader's last word, so nothing it published is
    /// overtaken by what follows the process's end.
    fn settled(&self, pid: u32) -> bool {
        for (wait, kill) in [
            (Duration::from_secs(1), true),
            (Duration::from_secs(2), false),
        ] {
            let until = Instant::now() + wait;
            while !self.eof.get() {
                match self
                    .inputs
                    .recv_timeout(until.saturating_duration_since(Instant::now()))
                {
                    Ok(Input::Eof) => self.eof.set(true),
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            if self.eof.get() {
                return true;
            }
            if kill {
                // What the CLI started outlived it and holds its output open.
                signal(pid, "-KILL");
            }
        }
        false
    }
}
fn woke(shared: &Shared) {
    shared.woke.store(
        shared.started.elapsed().as_millis() as u64,
        Ordering::Release,
    );
}
/// Signals the process and everything it started.
#[cfg(unix)]
pub(super) fn signal(pid: u32, name: &str) {
    let _ = Command::new("kill")
        .args([name, "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
#[cfg(not(unix))]
pub(super) fn signal(_: u32, _: &str) {}
fn keep_tail(mut stderr: impl Read, shared: &Shared) {
    let mut chunk = [0; 1024];
    while let Ok(read @ 1..) = stderr.read(&mut chunk) {
        let mut tail = lock(&shared.stderr);
        tail.push_str(&String::from_utf8_lossy(&chunk[..read]));
        if tail.len() > MAX_STDERR {
            let mut cut = tail.len() - MAX_STDERR;
            while !tail.is_char_boundary(cut) {
                cut += 1;
            }
            tail.drain(..cut);
        }
    }
}

struct Reader {
    driver: Arc<Mutex<Box<dyn Driver>>>,
    shared: Arc<Shared>,
    events: SyncSender<LeadEvent>,
    input: SyncSender<Input>,
    wake: Wake,
}
impl Reader {
    fn run(self, stdout: impl Read) {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            match frame(&mut reader, &mut line, MAX_FRAME) {
                Ok(Some(true)) => {}
                Ok(Some(false)) => continue,
                Ok(None) | Err(_) => break,
            }
            let Ok(text) = std::str::from_utf8(&line) else {
                continue;
            };
            let mut out = Output::default();
            lock(&self.driver).read(text.trim_end(), &mut out);
            self.publish(out);
        }
        let _ = self.input.send(Input::Eof);
    }
    fn publish(&self, out: Output) {
        let shared = &self.shared;
        if !out.deltas.is_empty() {
            {
                let mut stream = lock(&shared.stream);
                for (block, text) in &out.deltas {
                    if stream.block != *block {
                        stream.block = *block;
                        stream.text.clear();
                    }
                    let room = MAX_STREAM.saturating_sub(stream.text.len());
                    stream.text.push_str(clip(text, room));
                }
                stream.revision += 1;
            }
            let now = shared.started.elapsed();
            let last = Duration::from_millis(shared.woke.load(Ordering::Acquire));
            if now.saturating_sub(last) >= DELTA_WAKE {
                woke(shared);
                (self.wake)();
            } else if !shared.armed.swap(true, Ordering::AcqRel)
                && self
                    .input
                    .try_send(Input::Wake(shared.started + last + DELTA_WAKE))
                    .is_err()
            {
                shared.armed.store(false, Ordering::Release);
            }
        }
        for line in out.replies {
            let _ = self.input.send(Input::Line(line));
        }
        if let Some(after) = out.again {
            let _ = self.input.send(Input::Tick(Instant::now() + after));
        }
        if out.events.is_empty() {
            return;
        }
        let (mut ready, mut quit) = (false, false);
        {
            // A full queue holds the reader, and with it the CLI: nothing
            // is dropped. A lead that is gone takes nothing.
            let _order = lock(&shared.order);
            let state = &shared.state;
            for event in out.events {
                match &event {
                    LeadEvent::Ready { .. } => {
                        ready = true;
                        let _ = state.compare_exchange(
                            STARTING,
                            READY,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                    }
                    LeadEvent::TurnEnded { .. } => {
                        shared.turns.fetch_add(1, Ordering::AcqRel);
                        let _ = state.compare_exchange(
                            BUSY,
                            READY,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                    }
                    LeadEvent::Failed(_) => quit = true,
                    _ => {}
                }
                let _ = self.events.send(event);
            }
        }
        woke(shared);
        (self.wake)();
        if ready {
            let _ = self.input.send(Input::Ready);
        }
        if quit {
            let _ = self.input.send(Input::Quit);
        }
    }
}
/// Reads one line of at most `max` bytes. A longer one is passed over and
/// reported as `false`; `None` is the end.
fn frame(reader: &mut impl BufRead, line: &mut Vec<u8>, max: u64) -> std::io::Result<Option<bool>> {
    line.clear();
    let read = reader.by_ref().take(max).read_until(b'\n', line)? as u64;
    if read == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') || read < max {
        return Ok(Some(true));
    }
    loop {
        let rest = reader.fill_buf()?;
        let (end, used) = match rest.iter().position(|byte| *byte == b'\n') {
            Some(at) => (true, at + 1),
            None => (rest.is_empty(), rest.len()),
        };
        reader.consume(used);
        if end {
            return Ok(Some(false));
        }
    }
}
/// The start of `text` that fits `max` bytes.
fn clip(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
/// What a tool was called with, short enough to carry: long words are cut,
/// and an input that is still too long is not carried at all.
fn clip_input(input: &Value) -> Value {
    let fits = |value: &Value| value.to_string().len() <= MAX_INPUT;
    if fits(input) {
        return input.clone();
    }
    let mut short = input.clone();
    for value in short
        .as_object_mut()
        .into_iter()
        .flat_map(|fields| fields.values_mut())
    {
        if let Value::String(text) = value
            && text.len() > 256
        {
            *text = format!("{}…", clip(text, 256));
        }
    }
    if fits(&short) { short } else { Value::Null }
}
/// A folder only this user enters, for what a lead's process is handed.
fn private_dir() -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("neptune-lead-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir()
}
/// Writes the lead's tool servers where only this user reads them. The
/// text names the lead's credential, and an argument is readable by every
/// user of the computer.
fn write_private(directory: &Path, text: &str) -> std::io::Result<PathBuf> {
    let path = directory.join("mcp.json");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(text.as_bytes())?;
    file.flush()?;
    Ok(path)
}
/// A version 4 UUID from the system's randomness. It reads a device, so
/// only workers call it.
pub fn uuid() -> std::io::Result<String> {
    let mut bytes = [0; 16];
    let system =
        std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
    if system.is_err() {
        // No such device here: a name no other file has stands in.
        let nonce = tempfile::Builder::new().rand_bytes(32).tempfile()?;
        let name = nonce
            .path()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        bytes.copy_from_slice(&Sha256::digest(name.as_bytes())[..16]);
    }
    Ok(shape(bytes))
}
fn shape(mut bytes: [u8; 16]) -> String {
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Where a CLI is looked for. A desktop launch may not have the person's
/// shell `PATH`.
struct Search {
    path: OsString,
    /// Neptune's adapters, which start a CLI in a terminal and are never it.
    shims: Option<PathBuf>,
    /// Folders CLIs are commonly installed to.
    known: Vec<PathBuf>,
    shell: Option<PathBuf>,
    patience: Duration,
}
impl Search {
    fn from_env() -> Self {
        let home = directories::BaseDirs::new().map(|folders| folders.home_dir().to_owned());
        let at_home = |folders: &[&str]| -> Vec<PathBuf> {
            home.iter()
                .flat_map(|home| folders.iter().map(move |folder| home.join(folder)))
                .collect()
        };
        let mut known = at_home(&[".local/bin", ".bun/bin"]);
        known.extend(["/usr/local/bin", "/opt/homebrew/bin"].map(PathBuf::from));
        known.extend(at_home(&[".npm-global/bin", ".claude/local"]));
        Self {
            path: std::env::var_os("PATH").unwrap_or_default(),
            shims: std::env::var_os(agents::SHIMS).map(PathBuf::from),
            known,
            shell: std::env::var_os("SHELL")
                .filter(|shell| !shell.is_empty())
                .map(PathBuf::from),
            patience: FIND,
        }
    }
}
/// The CLIs that can lead a project, in the order they are offered.
pub const LEADS: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];
/// Which of them are installed here, looked for off the frame: the person's
/// login shell may be asked. `wake` is called once the answer waits. An
/// answer that never comes ends the wait.
pub fn installed(wake: Wake) -> Receiver<Vec<AgentKind>> {
    let (answer, found) = sync_channel(1);
    let _ = thread::Builder::new()
        .name("neptune-lead-find".into())
        .spawn(move || {
            let _ = answer.send(leads(&Search::from_env()));
            wake();
        });
    found
}
fn leads(search: &Search) -> Vec<AgentKind> {
    LEADS
        .into_iter()
        .filter(|kind| find(*kind, search).is_some())
        .collect()
}
/// Find a local CLI for a read-only account probe, on a worker.
pub(super) fn executable(kind: AgentKind, stop: &AtomicBool) -> Option<PathBuf> {
    find_cancelled(kind, &Search::from_env(), Some(stop))
}
/// The installed CLI: on `PATH`, then where CLIs are commonly installed,
/// then where the person's login shell finds it.
fn find(kind: AgentKind, search: &Search) -> Option<PathBuf> {
    find_cancelled(kind, search, None)
}
fn find_cancelled(kind: AgentKind, search: &Search, stop: Option<&AtomicBool>) -> Option<PathBuf> {
    if stop.is_some_and(|stop| stop.load(Ordering::Acquire)) {
        return None;
    }
    let shims = search.shims.as_deref();
    if let Ok(found) = agents::resolve_in(kind, &search.path, shims) {
        return Some(found);
    }
    if !search.known.is_empty()
        && let Ok(known) = std::env::join_paths(&search.known)
        && let Ok(found) = agents::resolve_in(kind, &known, shims)
    {
        return Some(found);
    }
    let shell = search.shell.as_ref()?;
    let mut output = tempfile::tempfile().ok()?;
    let mut command = Command::new(shell);
    command
        .args(["-lc", &format!("command -v {}", kind.executable())])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(output.try_clone().ok()?);
    if let Some(shims) = shims
        && let Ok(path) = std::env::join_paths(
            std::env::split_paths(&search.path).filter(|folder| folder != shims),
        )
    {
        command.env("PATH", path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().ok()?;
    let until = Instant::now() + search.patience;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None)
                if Instant::now() < until
                    && stop.is_none_or(|stop| !stop.load(Ordering::Acquire)) =>
            {
                thread::sleep(POLL)
            }
            _ => {
                signal(child.id(), "-KILL");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut said = String::new();
    output.rewind().ok()?;
    output.take(4096).read_to_string(&mut said).ok()?;
    // A profile may print before the answer; an alias or a function is no file.
    let found = said
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| Path::new(line).is_absolute())?;
    agents::resolve_in(kind, Path::new(found).parent()?.as_os_str(), shims).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_longer_than_a_frame_is_passed_over_and_the_next_is_read() {
        let text = format!("one\n{}\ntwo\n\nlast", "x".repeat(100));
        let mut reader = BufReader::with_capacity(16, text.as_bytes());
        let mut line = Vec::new();
        let mut read = Vec::new();
        while let Some(whole) = frame(&mut reader, &mut line, 32).unwrap() {
            read.push(whole.then(|| String::from_utf8(line.clone()).unwrap()));
        }
        assert_eq!(
            read,
            [
                Some("one\n".into()),
                None,
                Some("two\n".into()),
                Some("\n".into()),
                Some("last".into())
            ]
        );
        // One that ends exactly at the limit, and one that never ends.
        let mut reader = BufReader::new(&b"abc\nabcd\nabcde"[..]);
        let mut seen = Vec::new();
        while let Some(whole) = frame(&mut reader, &mut line, 4).unwrap() {
            seen.push(whole);
        }
        assert_eq!(seen, [true, false, false]);
    }

    #[test]
    fn text_and_tool_inputs_are_carried_short() {
        assert_eq!(clip("héllo", 2), "h");
        assert_eq!(clip("héllo", 3), "hé");
        assert_eq!(clip("hi", 9), "hi");
        assert_eq!(clip("hi", 0), "");
        let small = serde_json::json!({"title":"docs","n":3});
        assert_eq!(clip_input(&small), small);
        let long = serde_json::json!({"title":"docs","prompt":"é".repeat(5000)});
        let short = clip_input(&long);
        assert_eq!(short["title"], "docs");
        assert_eq!(short["prompt"], format!("{}…", "é".repeat(128)));
        // Nothing is carried of what cannot be made short.
        let many: serde_json::Map<String, Value> = (0..400)
            .map(|n| (format!("field{n}"), "value".into()))
            .collect();
        assert_eq!(clip_input(&Value::Object(many)), Value::Null);
        assert_eq!(clip_input(&Value::String("x".repeat(5000))), Value::Null);
    }

    #[test]
    fn a_turns_pictures_are_read_for_the_cli_and_one_that_cannot_be_shown_is_said_so() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0xfb, 0xff, 0xfe, 0x00]), "+//+AA==");
        assert_eq!(media(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(media(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(media(b"GIF89a.."), Some("image/gif"));
        assert_eq!(media(b"RIFF\x10\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(media(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(media(b"%PDF-1.7"), None);

        let folder = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let path = folder.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let png = file("shot.png", b"\x89PNG\r\n\x1a\npixels");
        let renamed = file("really a jpeg.png", &[0xff, 0xd8, 0xff, 0xe0, 1, 2]);
        let text_file = file("notes.png", b"not a picture");
        let gone = folder.path().join("pruned.png");
        let large = folder.path().join("large.png");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(MAX_PICTURE + 1)
            .unwrap();
        let paths = [
            png.clone(),
            renamed.clone(),
            text_file.clone(),
            gone.clone(),
            large.clone(),
            folder.path().to_owned(),
        ];
        let said = |path: &Path, why: &str| unseen(path.to_str().unwrap(), why);
        let refused = [
            said(&text_file, "it is not a PNG, JPEG, GIF or WebP picture"),
            said(&gone, "it is no longer there or cannot be read"),
            said(&large, "it is too large"),
            said(folder.path(), "it is no longer there or cannot be read"),
        ]
        .concat();
        // Written into the turn for a CLI that takes pictures so,
        let mut text = String::from("look\n");
        let shown = pictures(&paths, true, &mut text);
        assert_eq!(text, format!("look\n{refused}"));
        assert_eq!(shown.len(), 2);
        assert_eq!(
            (
                shown[0].path.as_str(),
                shown[0].media,
                shown[0].data.as_str()
            ),
            (png.to_str().unwrap(), "image/png", "iVBORw0KGgpwaXhlbHM=")
        );
        // by what the file is, whatever it is called;
        assert_eq!(
            (shown[1].media, shown[1].data.as_str()),
            ("image/jpeg", "/9j/4AEC")
        );
        // named for one that opens them itself, under the same rules.
        let mut text = String::new();
        let shown = pictures(&paths, false, &mut text);
        assert_eq!(text, refused);
        assert_eq!(shown.len(), 2);
        assert!(shown.iter().all(|picture| picture.data.is_empty()));

        // A turn takes pictures while they fit together.
        let each = MAX_PICTURE - 1024;
        let many: Vec<PathBuf> = (0..=MAX_PICTURES / each)
            .map(|n| {
                let path = folder.path().join(format!("{n}.png"));
                let mut file = std::fs::File::create(&path).unwrap();
                file.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
                file.set_len(each).unwrap();
                path
            })
            .collect();
        let mut text = String::new();
        let shown = pictures(&many, false, &mut text);
        assert_eq!(shown.len(), many.len() - 1);
        assert_eq!(
            text,
            said(
                many.last().unwrap(),
                "this turn has as many pictures as fit already"
            )
        );
    }

    #[test]
    fn a_session_id_is_a_random_uuid() {
        let (first, second) = (uuid().unwrap(), uuid().unwrap());
        assert_ne!(first, second);
        for id in [&first, &second, &shape([0xff; 16]), &shape([0; 16])] {
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(
                parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
                [8, 4, 4, 4, 12]
            );
            assert!(
                id.bytes()
                    .all(|byte| byte == b'-' || byte.is_ascii_hexdigit())
            );
            assert!(
                parts[2].starts_with('4')
                    && matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b')
            );
        }
        assert_eq!(shape([0; 16]), "00000000-0000-4000-8000-000000000000");
        assert_eq!(Failure::Provider("words".into()).kind(), "Provider");
        assert_eq!(Failure::Limit { resets_at: Some(1) }.kind(), "Limit");
    }

    #[cfg(unix)]
    mod process {
        use super::super::*;
        use std::sync::atomic::AtomicUsize;

        /// Writes a script that is run right away. A shell writes it, not
        /// this process: tests run on many threads, and a file one of them
        /// has open for writing is inherited by every process another
        /// thread forks at that moment. Until that process has run its own
        /// program the kernel refuses to run the file ("text file busy"),
        /// which failed a start here about once in twenty runs.
        fn executable(path: &Path, text: &str) {
            let mut writer = Command::new("/bin/sh")
                .args(["-c", "cat > \"$1\" && chmod 700 \"$1\"", "sh"])
                .arg(path)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = writer.stdin.take().unwrap();
            stdin.write_all(text.as_bytes()).unwrap();
            drop(stdin);
            assert!(
                writer.wait().unwrap().success(),
                "the script was not written"
            );
        }

        /// Enough of the CLI's stream protocol for one lead, in `sh`.
        const SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' "$@" > "$state/argv"
env > "$state/env"
pwd > "$state/cwd"
say() { printf '%s\n' "$1"; }
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      say '{"type":"system","subtype":"hook_started","hook_name":"SessionStart:startup"}'
      say 'not a frame'
      say '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"models":[{"value":"default","resolvedModel":"claude-stand-in"}],"account":{"email":"someone@example.com","subscriptionType":"Claude Max"}}}}' ;;
    *'"subtype":"mcp_status"'*)
      say '{"type":"control_response","response":{"subtype":"success","request_id":"mcp-1","response":{"mcpServers":[{"name":"neptune","status":"TOOLS"}]}}}' ;;
    *'"subtype":"interrupt"'*)
      INTERRUPT ;;
    *'"content":"slow"'*)
      say '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"mcp__neptune__list_agents","input":{}}]},"parent_tool_use_id":null}' ;;
    *'"content":"pause"'*)
      say '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"parent_tool_use_id":null}'
      say '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"a"}},"parent_tool_use_id":null}'
      say '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"b"}},"parent_tool_use_id":null}'
      sleep 1
      say '{"type":"assistant","message":{"content":[{"type":"text","text":"ab"}]},"parent_tool_use_id":null}'
      say '{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed"}' ;;
    *'"content":"many"'*)
      n=0
      while [ $n -lt 200 ]; do
        say '{"type":"assistant","message":{"content":[{"type":"text","text":"'$n'"}]},"parent_tool_use_id":null}'
        n=$((n + 1))
      done
      say '{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed"}' ;;
    *'"type":"user"'*)
      say '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"parent_tool_use_id":null}'
      say '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}},"parent_tool_use_id":null}'
      say '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo."}},"parent_tool_use_id":null}'
      say '{"type":"assistant","message":{"content":[{"type":"text","text":"Hello."}]},"parent_tool_use_id":null}'
      say '{"total_cost_usd":0.25,"terminal_reason":"completed","api_error_status":null,"is_error":false,"subtype":"success","type":"result"}' ;;
  esac
done
"#;
        const ANSWERS: &str = r#"say '{"type":"control_response","response":{"subtype":"success","request_id":"int-1","response":{"still_queued":[]}}}'
      say '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","is_error":true,"tool_use_id":"toolu_1","content":"stopped"}]},"parent_tool_use_id":null}'
      say '{"terminal_reason":"aborted_tools","is_error":true,"subtype":"error_during_execution","type":"result"}'"#;

        struct Fixture {
            folder: tempfile::TempDir,
            wakes: Arc<AtomicUsize>,
        }
        impl Fixture {
            fn new() -> Self {
                Self {
                    folder: tempfile::tempdir().unwrap(),
                    wakes: Arc::default(),
                }
            }
            /// A stand-in for the CLI. `before` runs ahead of its read loop.
            fn script(&self, before: &str, tools: &str, interrupt: &str) -> PathBuf {
                let path = self.folder.path().join("claude");
                let text = SCRIPT
                    .replacen(
                        "#!/bin/sh\n",
                        &format!(
                            "#!/bin/sh\nstate='{}'\necho $$ > \"$state/pid\"\n{before}\n",
                            self.folder.path().display()
                        ),
                        1,
                    )
                    .replace("TOOLS", tools)
                    .replace("INTERRUPT", interrupt);
                executable(&path, &text);
                path
            }
            fn launch(&self, exe: PathBuf, session: SessionRef) -> Launch {
                Launch {
                    kind: AgentKind::Claude,
                    exe: Some(exe),
                    project_dir: self.folder.path().to_owned(),
                    session,
                    system_prompt: "You lead.".into(),
                    mcp: Mcp {
                        exe: "/opt/neptune/neptune".into(),
                        open: Box::new(|| {
                            Ok(LeadEnv {
                                endpoint: "127.0.0.1:9".into(),
                                token: "lead-token".into(),
                                run: "run-token".into(),
                            })
                        }),
                    },
                    model: None,
                    effort: None,
                }
            }
            fn start(&self, exe: PathBuf, session: SessionRef, patience: Patience) -> Lead {
                let wakes = self.wakes.clone();
                Lead::start_with(
                    self.launch(exe, session),
                    Arc::new(move || {
                        wakes.fetch_add(1, Ordering::AcqRel);
                    }),
                    patience,
                )
            }
            fn said(&self, file: &str) -> String {
                std::fs::read_to_string(self.folder.path().join(file)).unwrap_or_default()
            }
            /// The stand-in's process, once it runs.
            fn pid(&self) -> String {
                let until = Instant::now() + Duration::from_secs(20);
                loop {
                    let pid = self.said("pid");
                    if pid.ends_with('\n') {
                        return pid.trim().to_owned();
                    }
                    assert!(Instant::now() < until, "the stand-in never ran");
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
        fn fresh() -> SessionRef {
            SessionRef {
                id: None,
                opened: false,
            }
        }
        fn quick() -> Patience {
            Patience {
                handshake: Duration::from_secs(20),
                interrupt: Duration::from_millis(200),
                close: Duration::from_millis(200),
                term: Duration::from_millis(200),
            }
        }
        /// Takes events until one is `last`.
        fn until(lead: &Lead, seen: &mut Vec<LeadEvent>, last: impl Fn(&LeadEvent) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let events = lead.try_events();
                let done = events.iter().any(&last);
                seen.extend(events);
                if done {
                    return;
                }
                assert!(Instant::now() < deadline, "{seen:?}");
                thread::sleep(Duration::from_millis(2));
            }
        }
        fn ended(event: &LeadEvent) -> bool {
            matches!(event, LeadEvent::TurnEnded { .. })
        }
        fn exited(event: &LeadEvent) -> bool {
            matches!(event, LeadEvent::Exited { .. })
        }
        fn gone(pid: &str) {
            let until = Instant::now() + Duration::from_secs(20);
            while Command::new("kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
            {
                assert!(Instant::now() < until, "the stand-in outlived its lead");
                thread::sleep(Duration::from_millis(10));
            }
        }

        #[test]
        fn a_lead_starts_takes_turns_is_interrupted_and_leaves_with_its_owner() {
            let fixture = Fixture::new();
            // What it is handed is looked at as it starts: it is not kept.
            let handed = r#"for a in "$@"; do case "$a" in */mcp.json) cat "$a" > "$state/config"; ls -ld "$a" > "$state/modes"; ls -ld "${a%/*}" >> "$state/modes";; esac; done"#;
            let exe = fixture.script(handed, "connected", ANSWERS);
            let lead = fixture.start(exe, fresh(), Patience::default());
            assert_eq!(
                lead.send_turn("early", "hello", &[]),
                Err(Refused::Starting)
            );
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            let [
                LeadEvent::Session { id },
                LeadEvent::Ready {
                    model,
                    account_kind,
                },
            ] = &seen[..]
            else {
                panic!("{seen:?}");
            };
            assert!(id.len() == 36 && model.as_deref() == Some("claude-stand-in"));
            assert_eq!(account_kind.as_deref(), Some("Claude Max"));
            assert_eq!(lead.state(), State::Ready);
            assert!(fixture.wakes.load(Ordering::Acquire) >= 2);

            // What it was started with: no tools but Neptune's, the lead's
            // own credential, and its project's folder.
            let arguments = fixture.said("argv");
            let arguments: Vec<&str> = arguments.lines().collect();
            assert_eq!(arguments[6], format!("--session-id={id}"));
            let at = |name: &str| {
                arguments
                    .iter()
                    .position(|argument| *argument == name)
                    .unwrap()
            };
            assert_eq!(arguments[at("--tools") + 1], "");
            assert_eq!(arguments[at("--setting-sources") + 1], "");
            // Its credential is in a file only this user reads, never in
            // an argument, which every user of the computer can read.
            let config = Path::new(arguments[at("--mcp-config") + 1]).to_owned();
            assert!(config.is_absolute());
            assert!(
                !arguments
                    .iter()
                    .any(|argument| argument.contains("lead-token"))
            );
            assert!(
                fixture
                    .said("config")
                    .contains("\"NEPTUNE_AGENT_TOKEN\":\"lead-token\"")
            );
            let modes = fixture.said("modes");
            let modes: Vec<&str> = modes.lines().map(|line| &line[..10]).collect();
            assert_eq!(modes, ["-rw-------", "drwx------"]);
            // Read, it is removed: a running lead leaves no credential in a
            // file, and neither does an application that ends before it.
            let limit = Instant::now() + Duration::from_secs(20);
            while config.parent().unwrap().exists() {
                assert!(Instant::now() < limit, "the credential outlived the start");
                thread::sleep(Duration::from_millis(10));
            }
            assert!(!arguments.contains(&"-p") && !arguments.contains(&"--print"));
            let environment = fixture.said("env");
            for line in [
                "NEPTUNE_AGENT_ROLE=lead",
                "NEPTUNE_AGENT_TOKEN=lead-token",
                "NEPTUNE_AGENT_RUN=run-token",
                "NEPTUNE_AGENT_ENDPOINT=127.0.0.1:9",
                "CLAUDE_CODE_ENTRYPOINT=sdk-ts",
            ] {
                assert!(environment.lines().any(|set| set == line), "{line}");
            }
            assert!(
                !environment.contains("NEPTUNE_AGENT_SHIMS=")
                    && !environment.contains("CLAUDECODE=")
            );
            assert_eq!(
                std::fs::canonicalize(fixture.said("cwd").trim()).unwrap(),
                std::fs::canonicalize(fixture.folder.path()).unwrap()
            );

            // A turn: its text streams, then is said whole, then it ends.
            assert_eq!(
                lead.send_turn("turn-1", &"x".repeat(MAX_TURN + 1), &[]),
                Err(Refused::TooLong)
            );
            assert_eq!(lead.send_turn("turn-1", "hello", &[]), Ok(()));
            assert_eq!(lead.state(), State::Busy);
            // A message written into a running turn would be folded into it.
            assert_eq!(lead.send_turn("turn-2", "and", &[]), Err(Refused::Busy));
            seen.clear();
            until(&lead, &mut seen, ended);
            assert_eq!(
                seen,
                [
                    LeadEvent::TurnStarted {
                        turn: "turn-1".into()
                    },
                    LeadEvent::TextDone {
                        block: 1,
                        text: "Hello.".into()
                    },
                    LeadEvent::TurnEnded {
                        turn: "turn-1".into(),
                        outcome: Outcome::Completed,
                        cost_usd: Some(0.25),
                    },
                ]
            );
            let stream = lead.stream();
            assert_eq!((stream.block, stream.text.as_str()), (1, "Hello."));
            assert_eq!(stream.revision, lead.revision());
            assert_eq!(lead.state(), State::Ready);
            lead.interrupt();

            // Interrupted during a tool call, the same process goes on.
            assert_eq!(lead.send_turn("turn-2", "slow", &[]), Ok(()));
            seen.clear();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::ToolStarted { .. })
            });
            assert_eq!(lead.stream().text, "");
            lead.interrupt();
            lead.interrupt();
            until(&lead, &mut seen, ended);
            assert_eq!(
                seen,
                [
                    LeadEvent::TurnStarted {
                        turn: "turn-2".into()
                    },
                    LeadEvent::ToolStarted {
                        call: "toolu_1".into(),
                        tool: "list_agents".into(),
                        input: serde_json::json!({}),
                    },
                    LeadEvent::ToolDone {
                        call: "toolu_1".into(),
                        ok: false
                    },
                    LeadEvent::TurnEnded {
                        turn: "turn-2".into(),
                        outcome: Outcome::Interrupted,
                        cost_usd: None,
                    },
                ]
            );
            let pid = fixture.pid();
            assert_eq!(lead.send_turn("turn-3", "hello", &[]), Ok(()));
            seen.clear();
            until(&lead, &mut seen, ended);
            assert_eq!(pid, fixture.pid());
            assert!(matches!(
                &seen[2],
                LeadEvent::TurnEnded {
                    outcome: Outcome::Completed,
                    ..
                }
            ));

            // Text that streams and then pauses is shown: its wakes are few,
            // and the last of them comes after the last of the text.
            let before = fixture.wakes.load(Ordering::Acquire);
            assert_eq!(lead.send_turn("turn-p", "pause", &[]), Ok(()));
            thread::sleep(Duration::from_millis(500));
            assert_eq!(lead.stream().text, "ab");
            let woken = fixture.wakes.load(Ordering::Acquire) - before;
            assert!((2..=3).contains(&woken), "{woken}");
            seen.clear();
            until(&lead, &mut seen, ended);
            assert_eq!(
                seen[1],
                LeadEvent::TextDone {
                    block: 1,
                    text: "ab".into()
                }
            );

            // More than the frame takes at once holds the CLI: nothing is lost.
            assert_eq!(lead.send_turn("turn-4", "many", &[]), Ok(()));
            thread::sleep(Duration::from_millis(300));
            seen.clear();
            assert_eq!(lead.try_events().len(), EVENTS);
            until(&lead, &mut seen, ended);
            let said: Vec<String> = seen
                .iter()
                .filter_map(|event| match event {
                    LeadEvent::TextDone { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                said,
                (EVENTS - 1..200).map(|n| n.to_string()).collect::<Vec<_>>()
            );
            assert!(lead.last_error().is_none());

            // Dropped, it is gone without the frame waiting for it.
            let started = Instant::now();
            drop(lead);
            assert!(started.elapsed() < Duration::from_millis(100));
            gone(&pid);
        }

        /// Enough of Codex's app-server for one lead, in `sh`. Each line it
        /// is written is kept, and so is what it was started with.
        const CODEX: &str = r#"#!/bin/sh
printf '%s\n' "$@" > "$state/argv"
env > "$state/env"
pwd > "$state/cwd"
say() { printf '%s\n' "$1"; }
thread=22222222-3333-7444-8555-666666666666
opened() {
  say '{"id":"thread","result":{"thread":{"id":"'$thread'","turns":[]},"model":"codex-stand-in","approvalPolicy":"never","sandbox":{"type":"readOnly","networkAccess":false}}}'
  say '{"method":"mcpServer/startupStatus/updated","params":{"threadId":"'$thread'","name":"neptune","status":"starting","error":null}}'
  say '{"method":"mcpServer/startupStatus/updated","params":{"threadId":"'$thread'","name":"neptune","status":"TOOLS","error":null}}'
}
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$state/input"
  case "$line" in
    *'"method":"initialize"'*)
      say '{"method":"remoteControl/status/changed","params":{"status":"disabled"}}'
      say 'not a frame'
      say '{"id":"init","result":{"userAgent":"stand-in","platformOs":"linux"}}' ;;
    *'"method":"account/read"'*)
      say '{"id":"account","result":{"account":ACCOUNT,"requiresOpenaiAuth":true}}' ;;
    *'"method":"config/read"'*)
      say '{"id":"config","result":{"config":{"mcp_servers":{"playwright":{"command":"npx","env":{"SECRET":"theirs"}}}},"origins":{}}}' ;;
    *'"method":"thread/start"'*|*'"method":"thread/resume"'*)
      opened ;;
    *'"method":"turn/interrupt"'*)
      say '{"id":"interrupt-1","result":{}}'
      say '{"method":"turn/completed","params":{"threadId":"'$thread'","turn":{"id":"wire-2","items":[],"status":"interrupted","error":null}}}' ;;
    *'"text":"mute"'*) ;;
    *'"text":"slow"'*)
      n=$((${n:-0} + 1))
      say '{"id":"turn-2","result":{"turn":{"id":"wire-2","status":"inProgress"}}}'
      say '{"method":"item/started","params":{"threadId":"'$thread'","turnId":"wire-2","item":{"type":"mcpToolCall","id":"call-1","server":"neptune","tool":"list_agents","status":"inProgress","arguments":{}}}}' ;;
    *'"method":"turn/start"'*)
      n=$((${n:-0} + 1))
      say '{"id":"turn-'$n'","result":{"turn":{"id":"wire-'$n'","status":"inProgress"}}}'
      say '{"method":"turn/started","params":{"threadId":"'$thread'","turn":{"id":"wire-'$n'","status":"inProgress"}}}'
      say '{"method":"item/started","params":{"threadId":"'$thread'","turnId":"wire-'$n'","item":{"type":"agentMessage","id":"msg-'$n'","text":"","phase":"final_answer"}}}'
      say '{"method":"item/agentMessage/delta","params":{"threadId":"'$thread'","turnId":"wire-'$n'","itemId":"msg-'$n'","delta":"Hel"}}'
      say '{"method":"item/agentMessage/delta","params":{"threadId":"'$thread'","turnId":"wire-'$n'","itemId":"msg-'$n'","delta":"lo."}}'
      say '{"method":"item/completed","params":{"threadId":"'$thread'","turnId":"wire-'$n'","item":{"type":"agentMessage","id":"msg-'$n'","text":"Hello.","phase":"final_answer"}}}'
      say '{"method":"turn/completed","params":{"threadId":"'$thread'","turn":{"id":"wire-'$n'","items":[],"status":"completed","error":null}}}' ;;
  esac
done
"#;
        impl Fixture {
            /// A stand-in for Codex, signed in or not, whose tool server
            /// ends up as `tools` says.
            fn codex(&self, account: &str, tools: &str) -> PathBuf {
                let path = self.folder.path().join("codex");
                let text = CODEX
                    .replacen(
                        "#!/bin/sh\n",
                        &format!(
                            "#!/bin/sh\nstate='{}'\necho $$ > \"$state/pid\"\n",
                            self.folder.path().display()
                        ),
                        1,
                    )
                    .replace("ACCOUNT", account)
                    .replace("TOOLS", tools);
                executable(&path, &text);
                path
            }
            fn lead(&self, exe: PathBuf, session: SessionRef) -> Lead {
                let mut launch = self.launch(exe, session);
                launch.kind = AgentKind::Codex;
                let wakes = self.wakes.clone();
                Lead::start_with(
                    launch,
                    Arc::new(move || {
                        wakes.fetch_add(1, Ordering::AcqRel);
                    }),
                    quick(),
                )
            }
        }

        #[test]
        fn codex_leads_over_its_app_server_and_resumes_the_conversation_it_named() {
            const SIGNED_IN: &str =
                r#"{"type":"chatgpt","email":"someone@example.com","planType":"pro"}"#;
            const THREAD: &str = "22222222-3333-7444-8555-666666666666";
            let fixture = Fixture::new();
            let lead = fixture.lead(fixture.codex(SIGNED_IN, "ready"), fresh());
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            // The conversation is the CLI's to name: nothing names it before.
            assert_eq!(
                seen,
                [
                    LeadEvent::Session { id: THREAD.into() },
                    LeadEvent::Ready {
                        model: Some("codex-stand-in".into()),
                        account_kind: Some("pro".into()),
                    },
                ]
            );
            assert_eq!(lead.state(), State::Ready);

            // Started with one word, in its project's folder. Its credential
            // for Neptune's tools is in no argument and not in its
            // environment: it is written to its input, and no file holds it.
            assert_eq!(fixture.said("argv"), "app-server\n");
            let environment = fixture.said("env");
            assert!(
                !environment.contains("NEPTUNE_AGENT_") && !environment.contains("lead-token"),
                "{environment}"
            );
            assert_eq!(
                std::fs::canonicalize(fixture.said("cwd").trim()).unwrap(),
                std::fs::canonicalize(fixture.folder.path()).unwrap()
            );
            let written = fixture.said("input");
            let written: Vec<Value> = written
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let methods: Vec<&str> = written
                .iter()
                .map(|frame| frame["method"].as_str().unwrap())
                .collect();
            assert_eq!(
                methods,
                [
                    "initialize",
                    "initialized",
                    "account/read",
                    "config/read",
                    "thread/start"
                ]
            );
            assert!(written.iter().all(|frame| frame.get("jsonrpc").is_none()));
            let opening = &written[4]["params"];
            assert_eq!(opening["approvalPolicy"], "never");
            assert_eq!(opening["sandbox"], "read-only");
            assert_eq!(opening["developerInstructions"], "You lead.");
            let servers = &opening["config"]["mcp_servers"];
            assert_eq!(servers["neptune"]["command"], "/opt/neptune/neptune");
            assert_eq!(
                servers["neptune"]["env"]["NEPTUNE_AGENT_TOKEN"],
                "lead-token"
            );
            assert_eq!(servers["neptune"]["env"]["NEPTUNE_AGENT_ROLE"], "lead");
            // The person's own tool servers are kept off, and nothing of
            // what they are started with is said back.
            assert_eq!(servers["playwright"], serde_json::json!({"enabled":false}));
            assert_eq!(opening["config"]["web_search"], "disabled");
            assert_eq!(opening["config"]["features"]["shell_tool"], false);

            // A turn: its text streams, then is said whole, then it ends.
            assert_eq!(lead.send_turn("turn-a", "hello", &[]), Ok(()));
            assert_eq!(lead.send_turn("turn-b", "and", &[]), Err(Refused::Busy));
            seen.clear();
            until(&lead, &mut seen, ended);
            assert_eq!(
                seen,
                [
                    LeadEvent::TurnStarted {
                        turn: "turn-a".into()
                    },
                    LeadEvent::TextDone {
                        block: 1,
                        text: "Hello.".into()
                    },
                    LeadEvent::TurnEnded {
                        turn: "turn-a".into(),
                        outcome: Outcome::Completed,
                        cost_usd: None,
                    },
                ]
            );
            let stream = lead.stream();
            assert_eq!((stream.block, stream.text.as_str()), (1, "Hello."));

            // Stopped inside a tool call: the call is closed, the turn ends
            // interrupted and the same process takes the next.
            assert_eq!(lead.send_turn("turn-b", "slow", &[]), Ok(()));
            seen.clear();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::ToolStarted { .. })
            });
            lead.interrupt();
            until(&lead, &mut seen, ended);
            assert_eq!(
                seen[1..],
                [
                    LeadEvent::ToolStarted {
                        call: "call-1".into(),
                        tool: "list_agents".into(),
                        input: serde_json::json!({}),
                    },
                    LeadEvent::ToolDone {
                        call: "call-1".into(),
                        ok: false
                    },
                    LeadEvent::TurnEnded {
                        turn: "turn-b".into(),
                        outcome: Outcome::Interrupted,
                        cost_usd: None,
                    },
                ]
            );
            let pid = fixture.pid();
            assert_eq!(lead.send_turn("turn-c", "hello", &[]), Ok(()));
            seen.clear();
            until(&lead, &mut seen, ended);
            assert_eq!(pid, fixture.pid());
            let turns: Vec<Value> = fixture
                .said("input")
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .filter(|frame| frame["method"] == "turn/start")
                .collect();
            assert_eq!(turns.len(), 3);
            for turn in &turns {
                assert_eq!(turn["params"]["approvalPolicy"], "never");
                assert_eq!(turn["params"]["sandboxPolicy"]["type"], "readOnly");
                assert_eq!(turn["params"]["threadId"], THREAD);
            }
            assert!(lead.last_error().is_none());
            let started = Instant::now();
            drop(lead);
            assert!(started.elapsed() < Duration::from_millis(100));
            gone(&pid);

            // After a restart the conversation it named is resumed, with
            // what a turn may do and its tool server said again.
            let fixture = Fixture::new();
            let lead = fixture.lead(
                fixture.codex(SIGNED_IN, "ready"),
                SessionRef {
                    id: Some(THREAD.into()),
                    opened: true,
                },
            );
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            assert_eq!(seen.len(), 2, "{seen:?}");
            assert_eq!(seen[0], LeadEvent::Session { id: THREAD.into() });
            let written = fixture.said("input");
            let resumed: Value = serde_json::from_str(written.lines().last().unwrap()).unwrap();
            assert_eq!(resumed["method"], "thread/resume");
            assert_eq!(resumed["params"]["threadId"], THREAD);
            assert_eq!(resumed["params"]["excludeTurns"], true);
            assert_eq!(resumed["params"]["sandbox"], "read-only");
            assert_eq!(
                resumed["params"]["config"]["mcp_servers"]["neptune"]["tool_timeout_sec"],
                60
            );
            assert!(resumed["params"].get("developerInstructions").is_none());
            drop(lead);

            // A CLI that repeats what it was written before it ends: its
            // last words reach the chat without the lead's credential.
            let fixture = Fixture::new();
            let echo = fixture.folder.path().join("codex");
            let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":"init","result":{}}' ;;
    *'"method":"account/read"'*) echo '{"id":"account","result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}' ;;
    *'"method":"config/read"'*) echo '{"id":"config","result":{"config":{}}}' ;;
    *'"method":"thread/start"'*) printf 'bad request: %s\n' "$line" >&2; exit 3 ;;
  esac
done
"#;
            executable(&echo, script);
            let lead = fixture.lead(echo, fresh());
            let mut seen = Vec::new();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen,
                [
                    LeadEvent::Failed(Failure::Transport),
                    LeadEvent::Exited { code: Some(3) }
                ]
            );
            let said = lead.last_error().unwrap();
            assert!(said.contains("NEPTUNE_AGENT_TOKEN"), "{said}");
            assert!(!said.contains("lead-token") && !said.contains("run-token"));
            // Only the end of what it said is kept: one cut inside the
            // credential does not say the rest of it.
            *lock(&lead.shared.stderr) = "d-token\" was refused".into();
            assert_eq!(lead.last_error().as_deref(), Some("…\" was refused"));
            drop(lead);

            // Asked to stop a turn the CLI never named: no interrupt can be
            // written, and the turn is still not left running for ever.
            let fixture = Fixture::new();
            let lead = fixture.lead(fixture.codex(SIGNED_IN, "ready"), fresh());
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            assert_eq!(lead.send_turn("turn-m", "mute", &[]), Ok(()));
            seen.clear();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::TurnStarted { .. })
            });
            lead.interrupt();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen[1],
                LeadEvent::TurnEnded {
                    turn: "turn-m".into(),
                    outcome: Outcome::Interrupted,
                    cost_usd: None,
                }
            );
            assert!(!fixture.said("input").contains("turn/interrupt"));
            gone(&fixture.pid());

            // Nobody is signed in; its tool server does not start.
            for (account, tools, failure, events) in [
                ("null", "ready", Failure::Auth, 2),
                (SIGNED_IN, "failed", Failure::ToolsUnavailable, 3),
            ] {
                let fixture = Fixture::new();
                let lead = fixture.lead(fixture.codex(account, tools), fresh());
                let mut seen = Vec::new();
                until(&lead, &mut seen, exited);
                assert_eq!(seen.len(), events, "{seen:?}");
                assert_eq!(
                    seen[events - 2..],
                    [
                        LeadEvent::Failed(failure),
                        LeadEvent::Exited { code: Some(0) }
                    ]
                );
                assert_eq!(lead.state(), State::Dead);
                gone(&fixture.pid());
            }
        }

        #[test]
        fn a_turns_pictures_are_written_to_the_cli_with_its_words() {
            let fixture = Fixture::new();
            // The stand-in keeps the turn it was written.
            let exe = fixture.script("", "connected", ANSWERS);
            let keeping = std::fs::read_to_string(&exe).unwrap().replacen(
                "    *'\"type\":\"user\"'*)\n",
                "    *'\"type\":\"user\"'*)\n      printf '%s\\n' \"$line\" > \"$state/turn\"\n",
                1,
            );
            executable(&exe, &keeping);
            let shot = fixture.folder.path().join("shot.png");
            std::fs::write(&shot, b"\x89PNG\r\n\x1a\npixels").unwrap();
            let gone = fixture.folder.path().join("gone.png");
            let lead = fixture.start(exe, fresh(), Patience::default());
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            assert_eq!(
                lead.send_turn("turn-1", "look", &[shot.clone(), gone.clone()]),
                Ok(())
            );
            until(&lead, &mut seen, ended);
            let turn: Value = serde_json::from_str(&fixture.said("turn")).unwrap();
            assert_eq!(
                turn["message"]["content"],
                serde_json::json!([
                    {"type":"text","text":format!(
                        "look\n[Neptune could not show you {}: it is no longer there or \
                         cannot be read]\n",
                        gone.display()
                    )},
                    {"type":"image","source":{
                        "type":"base64",
                        "media_type":"image/png",
                        "data":"iVBORw0KGgpwaXhlbHM=",
                    }},
                ])
            );
        }

        #[test]
        fn a_session_another_process_holds_is_resumed_once() {
            let fixture = Fixture::new();
            let refuse = "case \"$*\" in *--session-id=*) echo 'Error: Session ID 1 is already in use.' >&2; exit 1;; esac";
            let session = || SessionRef {
                id: Some("11111111-2222-4333-8444-555555555555".into()),
                opened: false,
            };
            let lead = fixture.start(
                fixture.script(refuse, "connected", ANSWERS),
                session(),
                quick(),
            );
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            assert_eq!(seen.len(), 2, "{seen:?}");
            assert_eq!(
                seen[0],
                LeadEvent::Session {
                    id: "11111111-2222-4333-8444-555555555555".into()
                }
            );
            assert!(
                fixture
                    .said("argv")
                    .contains("--resume=11111111-2222-4333-8444-555555555555\n")
            );
            assert_eq!(lead.send_turn("turn-1", "hello", &[]), Ok(()));
            until(&lead, &mut seen, ended);
            drop(lead);

            // Once: a CLI that keeps refusing is a failure, in its own words.
            let fixture = Fixture::new();
            let refuse = "printf '%s\\n' \"$*\" >> \"$state/tries\"; echo 'Error: Session ID 1 is already in use.' >&2; exit 1";
            let lead = fixture.start(
                fixture.script(refuse, "connected", ANSWERS),
                session(),
                quick(),
            );
            let mut seen = Vec::new();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen[1..],
                [
                    LeadEvent::Failed(Failure::Transport),
                    LeadEvent::Exited { code: Some(1) }
                ]
            );
            let tries = fixture.said("tries");
            let tries: Vec<&str> = tries.lines().collect();
            assert!(
                tries.len() == 2
                    && tries[0].contains(" --session-id=")
                    && tries[1].contains(" --resume=")
            );
            assert_eq!(
                lead.last_error().as_deref(),
                Some("Error: Session ID 1 is already in use.")
            );
            assert_eq!(lead.state(), State::Dead);
            assert_eq!(lead.send_turn("turn-1", "hello", &[]), Err(Refused::Dead));

            // A session that is resumed and is not there is said to be lost.
            let fixture = Fixture::new();
            let lost = "echo 'No conversation found with session ID: 1' >&2; exit 1";
            let opened = SessionRef {
                id: Some("11111111-2222-4333-8444-555555555555".into()),
                opened: true,
            };
            let lead = fixture.start(fixture.script(lost, "connected", ANSWERS), opened, quick());
            let mut seen = Vec::new();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen[1..],
                [
                    LeadEvent::Failed(Failure::SessionLost),
                    LeadEvent::Exited { code: Some(1) }
                ]
            );
        }

        #[test]
        fn a_lead_that_cannot_start_says_why_and_ends() {
            // No such program.
            let fixture = Fixture::new();
            let lead = fixture.start(fixture.folder.path().join("absent"), fresh(), quick());
            let mut seen = Vec::new();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen[1..],
                [
                    LeadEvent::Failed(Failure::NotInstalled),
                    LeadEvent::Exited { code: None }
                ]
            );
            assert_eq!(lead.state(), State::Dead);

            // The bridge is not there to open.
            let mut launch = fixture.launch(fixture.script("", "connected", ANSWERS), fresh());
            launch.mcp.open = Box::new(|| Err(std::io::Error::other("no bridge")));
            let lead = Lead::start(launch, Arc::new(|| {}));
            let mut seen = Vec::new();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen[1..],
                [
                    LeadEvent::Failed(Failure::Transport),
                    LeadEvent::Exited { code: None }
                ]
            );
            // A provider with no driver.
            let mut launch = fixture.launch(fixture.script("", "connected", ANSWERS), fresh());
            launch.kind = AgentKind::Gemini;
            let lead = Lead::start(launch, Arc::new(|| {}));
            let mut seen = Vec::new();
            until(&lead, &mut seen, exited);
            assert_eq!(seen[1], LeadEvent::Failed(Failure::Protocol));

            // Its tools did not connect: it is not left to talk without them.
            let fixture = Fixture::new();
            let lead = fixture.start(fixture.script("", "failed", ANSWERS), fresh(), quick());
            let mut seen = Vec::new();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen[1..],
                [
                    LeadEvent::Failed(Failure::ToolsUnavailable),
                    LeadEvent::Exited { code: Some(0) }
                ]
            );
            gone(&fixture.pid());

            // It ends before it answers, and what it said last is kept short.
            let fixture = Fixture::new();
            let dies = "i=0; while [ $i -lt 300 ]; do echo 'a long line of noise' >&2; i=$((i + 1)); done; echo 'boom é' >&2; exit 3";
            let lead = fixture.start(fixture.script(dies, "connected", ANSWERS), fresh(), quick());
            let mut seen = Vec::new();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen[1..],
                [
                    LeadEvent::Failed(Failure::Transport),
                    LeadEvent::Exited { code: Some(3) }
                ]
            );
            let said = lead.last_error().unwrap();
            assert!(
                said.ends_with("boom é") && said.len() <= MAX_STDERR,
                "{said}"
            );
        }

        #[test]
        fn a_cli_that_stops_answering_is_stopped_and_dropping_its_lead_never_waits() {
            // Deaf to its input closing and to being asked to end.
            let deaf = "trap '' TERM\nwhile :; do sleep 1; done";
            // No answer to the handshake.
            let fixture = Fixture::new();
            let patience = Patience {
                handshake: Duration::from_millis(300),
                ..quick()
            };
            let lead = fixture.start(
                fixture.script(deaf, "connected", ANSWERS),
                fresh(),
                patience,
            );
            let pid = fixture.pid();
            let mut seen = Vec::new();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen[1..],
                [
                    LeadEvent::Failed(Failure::Transport),
                    LeadEvent::Exited { code: None }
                ]
            );
            assert_eq!(lead.state(), State::Dead);
            gone(&pid);

            // No answer to an interrupt: the turn still ends interrupted.
            let fixture = Fixture::new();
            let lead = fixture.start(
                fixture.script("trap '' TERM", "connected", "sleep 30"),
                fresh(),
                quick(),
            );
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            assert_eq!(lead.send_turn("turn-1", "slow", &[]), Ok(()));
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::ToolStarted { .. })
            });
            seen.clear();
            lead.interrupt();
            until(&lead, &mut seen, exited);
            assert_eq!(
                seen,
                [
                    LeadEvent::TurnEnded {
                        turn: "turn-1".into(),
                        outcome: Outcome::Interrupted,
                        cost_usd: None,
                    },
                    LeadEvent::Exited { code: None },
                ]
            );
            gone(&fixture.pid());

            // Dropped while it starts, while it hangs and with a turn running.
            let fixture = Fixture::new();
            let lead = fixture.start(fixture.script(deaf, "connected", ANSWERS), fresh(), quick());
            let pid = fixture.pid();
            let started = Instant::now();
            drop(lead);
            assert!(started.elapsed() < Duration::from_millis(100));
            gone(&pid);
            let fixture = Fixture::new();
            let lead = fixture.start(
                fixture.script("", "connected", "sleep 30"),
                fresh(),
                quick(),
            );
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            assert_eq!(lead.send_turn("turn-1", "many", &[]), Ok(()));
            let started = Instant::now();
            drop(lead);
            assert!(started.elapsed() < Duration::from_millis(100));
            gone(&fixture.pid());
        }

        #[test]
        fn a_late_interrupt_does_not_stop_the_next_turn_and_a_stuck_write_does_not_outlive_its_lead()
         {
            // An interrupt that reaches the writer after its turn ended, with
            // the next turn already taken, is for a turn that is over.
            let fixture = Fixture::new();
            let lead = fixture.start(fixture.script("", "connected", ANSWERS), fresh(), quick());
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            lead.shared.state.store(BUSY, Ordering::Release);
            // The first only ends the wait for the handshake.
            lead.input.send(Input::Interrupt).unwrap();
            lead.input.send(Input::Interrupt).unwrap();
            let turn = Input::Turn {
                id: "turn-1".into(),
                text: "slow".into(),
                pictures: Vec::new(),
            };
            lead.input.send(turn).unwrap();
            seen.clear();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::ToolStarted { .. })
            });
            // Well past the time an interrupt is given to be answered.
            thread::sleep(Duration::from_millis(600));
            assert_eq!(lead.state(), State::Busy);
            assert!(lead.try_events().is_empty());
            // The turn that runs is still interrupted when asked.
            lead.interrupt();
            seen.clear();
            until(&lead, &mut seen, ended);
            assert!(matches!(
                seen.last(),
                Some(LeadEvent::TurnEnded {
                    outcome: Outcome::Interrupted,
                    ..
                })
            ));
            assert_eq!(lead.state(), State::Ready);
            drop(lead);
            gone(&fixture.pid());

            // A CLI that stops reading holds the writer in a write, where no
            // request to stop reaches it: dropped, the lead ends it at once.
            let fixture = Fixture::new();
            let stuck = r#"IFS= read -r line
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{}}}'
IFS= read -r line
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"mcp-1","response":{"mcpServers":[{"name":"neptune","status":"connected"}]}}}'
exec sleep 30"#;
            let lead = fixture.start(
                fixture.script(stuck, "connected", ANSWERS),
                fresh(),
                quick(),
            );
            let pid = fixture.pid();
            let mut seen = Vec::new();
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::Ready { .. })
            });
            assert_eq!(lead.send_turn("turn-1", &"x".repeat(MAX_TURN), &[]), Ok(()));
            until(&lead, &mut seen, |event| {
                matches!(event, LeadEvent::TurnStarted { .. })
            });
            let until = Instant::now() + Duration::from_secs(20);
            while !lead.shared.writing.load(Ordering::Acquire) {
                assert!(Instant::now() < until, "the write never began");
                thread::sleep(Duration::from_millis(2));
            }
            thread::sleep(Duration::from_millis(100));
            assert!(lead.shared.writing.load(Ordering::Acquire));
            let started = Instant::now();
            drop(lead);
            assert!(started.elapsed() < Duration::from_millis(100));
            gone(&pid);
        }

        #[test]
        fn the_cli_is_found_where_a_desktop_launch_would_miss_it() {
            let folder = tempfile::tempdir().unwrap();
            let install = |at: &str| -> PathBuf {
                let directory = folder.path().join(at);
                std::fs::create_dir_all(&directory).unwrap();
                let path = directory.join("claude");
                executable(&path, "#!/bin/sh\n");
                path
            };
            let shell = |name: &str, text: &str| -> PathBuf {
                let path = folder.path().join(name);
                executable(&path, &format!("#!/bin/sh\n{text}\n"));
                path
            };
            let search = |path: &[&str], known: &[&str], shell: Option<PathBuf>| Search {
                path: std::env::join_paths(path.iter().map(|at| folder.path().join(at))).unwrap(),
                shims: Some(folder.path().join("shims")),
                known: known.iter().map(|at| folder.path().join(at)).collect(),
                shell,
                patience: Duration::from_millis(300),
            };
            let adapter = install("shims");
            let on_path = install("bin");
            let known = install("home/.local/bin");
            let elsewhere = install("opt/node/bin");
            // `PATH` first, and never Neptune's own adapter.
            assert_eq!(
                find(
                    AgentKind::Claude,
                    &search(&["shims", "bin"], &["home/.local/bin"], None)
                ),
                Some(on_path)
            );
            assert_eq!(
                find(
                    AgentKind::Claude,
                    &search(
                        &["shims", "empty"],
                        &["none", "shims", "home/.local/bin"],
                        None
                    )
                ),
                Some(known)
            );
            // Then the login shell, whose profile may print first.
            let says = shell(
                "login",
                &format!(
                    "[ \"$1\" = -lc ] && [ \"$2\" = 'command -v claude' ] || exit 1\necho Welcome\necho '{}'",
                    elsewhere.display()
                ),
            );
            assert_eq!(
                find(AgentKind::Claude, &search(&["shims"], &[], Some(says))),
                Some(elsewhere)
            );
            // An alias, the adapter, a file that is not there, a shell that
            // never answers: not found.
            for text in [
                "echo \"alias claude='claude --verbose'\"".to_owned(),
                format!("echo '{}'", adapter.display()),
                format!("echo '{}'", folder.path().join("nowhere/claude").display()),
                "exit 1".to_owned(),
                "/bin/sleep 30".to_owned(),
            ] {
                let started = Instant::now();
                assert_eq!(
                    find(
                        AgentKind::Claude,
                        &search(&["shims"], &[], Some(shell("other", &text)))
                    ),
                    None
                );
                assert!(started.elapsed() < Duration::from_secs(5));
            }
            assert_eq!(
                find(AgentKind::Claude, &search(&["empty"], &[], None)),
                None
            );
            // Each CLI that can lead is looked for by its own name.
            assert_eq!(leads(&search(&["bin"], &[], None)), [AgentKind::Claude]);
            let codex = folder.path().join("home/.local/bin/codex");
            executable(&codex, "#!/bin/sh\n");
            assert_eq!(
                find(
                    AgentKind::Codex,
                    &search(&["shims", "bin"], &["home/.local/bin"], None)
                ),
                Some(codex)
            );
            assert_eq!(leads(&search(&["bin"], &["home/.local/bin"], None)), LEADS);
            assert!(leads(&search(&["empty"], &[], None)).is_empty());
            // Off the frame, the same answer waits for whoever asked.
            let found = installed(Arc::new(|| {}));
            let found = found.recv_timeout(Duration::from_secs(30)).unwrap();
            assert!(found.iter().all(|kind| LEADS.contains(kind)));
        }

        #[test]
        fn usage_discovery_cancels_and_reaps_the_owned_login_shell() {
            let folder = tempfile::tempdir().unwrap();
            let shell = folder.path().join("login");
            let pid = folder.path().join("pid");
            executable(
                &shell,
                &format!("#!/bin/sh\necho $$ > '{}'\n/bin/sleep 30\n", pid.display()),
            );
            let stop = Arc::new(AtomicBool::new(false));
            let cancel = Arc::clone(&stop);
            let marker = pid.clone();
            let cancelled = thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(2);
                while !marker.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                cancel.store(true, Ordering::Release);
            });
            let search = Search {
                path: OsString::new(),
                shims: None,
                known: Vec::new(),
                shell: Some(shell),
                patience: Duration::from_secs(5),
            };
            assert_eq!(
                find_cancelled(AgentKind::Claude, &search, Some(&stop)),
                None
            );
            cancelled.join().unwrap();
            gone(std::fs::read_to_string(pid).unwrap().trim());
        }
    }
}
