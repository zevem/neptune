//! Scoped CLI adapters and a bounded, metadata-only local hook bridge.
//! All setup and socket/file I/O runs on startup workers or the bridge worker.
use crate::agent_activity::{Activity, Attention};
use neptune_model::{AgentKind, AgentSession, PaneId, PullRequest};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{IsTerminal, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use terminal_core::SessionOptions;

type Wake = Arc<dyn Fn() + Send + Sync>;
const MAX_MESSAGE: u64 = 40 * 1024;
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
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
    Activity {
        signal: Signal,
    },
    /// The CLI is gone but its reference is kept, as after a failed resume.
    Stopped,
}
/// What a hook says about the agent's turn. It names the kind of moment and
/// at most a tool; prompts, commands and results never leave the hook process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "snake_case", deny_unknown_fields)]
enum Signal {
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
    /// Set while an invocation is open.
    activity: Option<Tracked>,
    wake: Wake,
    _startup: Option<tempfile::TempDir>,
}
#[derive(Default)]
struct Shared {
    slots: BTreeMap<PaneId, Slot>,
    changes: BTreeMap<PaneId, (u64, Option<AgentSession>)>,
    /// The latest activity per pane since the last frame; `None` once closed.
    activity: BTreeMap<PaneId, (u64, Option<Activity>)>,
    links: Vec<(PaneId, u64, PullRequest)>,
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
        wake: Wake,
    ) -> std::io::Result<()> {
        #[cfg(not(unix))]
        {
            let _ = (pane, generation, options, resume, wake);
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
                for kind in [AgentKind::Claude, AgentKind::Codex] {
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
                            let taken = serde_json::from_slice::<Message>(&bytes)
                                .is_ok_and(|message| apply_message(&shared, message));
                            let _ = stream.write_all(if taken { b"ok" } else { b"no" });
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
            configure_shell(options, startup.path(), &std::env::current_exe()?, resume)?;
            let (old, retired) = {
                let mut shared = self
                    .shared
                    .lock()
                    .map_err(|_| std::io::Error::other("Agent bridge unavailable"))?;
                shared.changes.remove(&pane);
                shared.activity.remove(&pane);
                shared.links.retain(|link| link.0 != pane);
                let old = shared.slots.insert(
                    pane,
                    Slot {
                        generation,
                        token,
                        run: None,
                        activity: None,
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
    pub fn close(&self, pane: PaneId) {
        if let Ok(mut shared) = self.shared.lock() {
            if let Some(mut slot) = shared.slots.remove(&pane)
                && let Some(startup) = slot._startup.take()
            {
                shared.retired.push(startup);
            }
            shared.changes.remove(&pane);
            shared.activity.remove(&pane);
            shared.links.retain(|link| link.0 != pane);
        }
    }
    pub fn close_generation(&self, pane: PaneId, generation: u64) {
        if let Ok(mut shared) = self.shared.lock()
            && shared
                .slots
                .get(&pane)
                .is_some_and(|slot| slot.generation == generation)
        {
            if let Some(mut slot) = shared.slots.remove(&pane)
                && let Some(startup) = slot._startup.take()
            {
                shared.retired.push(startup);
            }
            shared.changes.remove(&pane);
            shared.activity.remove(&pane);
            shared.links.retain(|link| link.0 != pane);
        }
    }
    pub fn drain(&self) -> Vec<(PaneId, u64, Option<AgentSession>)> {
        self.shared
            .lock()
            .map(|mut shared| {
                std::mem::take(&mut shared.changes)
                    .into_iter()
                    .map(|(pane, (generation, agent))| (pane, generation, agent))
                    .collect()
            })
            .unwrap_or_default()
    }
    /// What each agent turned to since the last call; `None` when it left.
    pub fn drain_activity(&self) -> Vec<(PaneId, u64, Option<Activity>)> {
        self.shared
            .lock()
            .map(|mut shared| {
                std::mem::take(&mut shared.activity)
                    .into_iter()
                    .map(|(pane, (generation, activity))| (pane, generation, activity))
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
}
#[cfg(unix)]
fn configure_shell(
    options: &mut SessionOptions,
    directory: &std::path::Path,
    helper: &std::path::Path,
    resume: Option<&AgentSession>,
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
    let resume_json = resume
        .filter(|agent| agent.is_valid())
        .map(serde_json::to_string)
        .transpose()?;
    let mut setup = "export PATH=\"$NEPTUNE_AGENT_SHIMS:$PATH\"\n".to_owned();
    if let Some(resume) = &resume_json {
        if name == "zsh" {
            // Powerlevel10k's instant prompt keeps stdio off the terminal until the
            // first prompt, later than any startup file; the agent needs it now.
            setup.push_str("(( ${+functions[p10k]} )) && p10k clear-instant-prompt\n");
        }
        // Quoted literal data, never interpolated terminal input. This runs after user startup files.
        setup.push_str(&format!(
            "{} --agent-restore {}\n",
            quote(&helper.to_string_lossy()),
            quote(resume)
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
            if let Some(resume) = resume_json {
                options.shell = Some("/bin/sh".into());
                let args = std::mem::take(&mut options.args);
                options.args = vec![
                    "-c".into(),
                    "\"$1\" --agent-restore \"$2\"; shift 2; exec \"$@\"".into(),
                    "neptune".into(),
                    helper.to_string_lossy().into_owned(),
                    resume,
                    shell,
                ];
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
    let Ok(mut shared) = shared.lock() else {
        return false;
    };
    let Some((&pane, slot)) = shared
        .slots
        .iter_mut()
        .find(|(_, slot)| slot.token == message.token)
    else {
        return false;
    };
    if let Event::PullRequest { url } = &message.event {
        // Only the invocation that opened in this pane may link to it.
        let link = PullRequest::parse(url).filter(|_| slot.run.as_ref() == Some(&message.run));
        let (generation, wake) = (slot.generation, slot.wake.clone());
        let Some(link) = link.filter(|_| shared.links.len() < MAX_LINKS) else {
            return false;
        };
        shared.links.push((pane, generation, link));
        drop(shared);
        wake();
        return true;
    }
    if matches!(message.event, Event::Stopped) {
        if slot.run.as_ref() != Some(&message.run) {
            return false;
        }
        slot.run = None;
        slot.activity = None;
        let (generation, wake) = (slot.generation, slot.wake.clone());
        shared.activity.insert(pane, (generation, None));
        drop(shared);
        wake();
        return true;
    }
    if let Event::Activity { signal } = message.event {
        // Only the open invocation describes this pane.
        if slot.run.as_ref() != Some(&message.run) {
            return false;
        }
        let Some(tracked) = slot.activity.as_mut() else {
            return false;
        };
        if tracked.apply(signal) {
            let (state, generation, wake) = (tracked.state, slot.generation, slot.wake.clone());
            shared.activity.insert(pane, (generation, Some(state)));
            drop(shared);
            wake();
        }
        return true;
    }
    let mut reset = None;
    let agent = match message.event {
        Event::Open { agent } if agent.is_valid() => {
            slot.run = Some(message.run);
            slot.activity = Some(Tracked::idle());
            reset = Some(Some(Activity::Idle));
            Some(agent)
        }
        Event::Session { agent } if slot.run.as_ref() == Some(&message.run) && agent.is_valid() => {
            Some(agent)
        }
        Event::Close if slot.run.as_ref() == Some(&message.run) => {
            slot.run = None;
            slot.activity = None;
            reset = Some(None);
            None
        }
        _ => return false,
    };
    let generation = slot.generation;
    let wake = slot.wake.clone();
    // A new conversation in the same run keeps what the agent is doing.
    if let Some(activity) = reset {
        shared.activity.insert(pane, (generation, activity));
    }
    shared.changes.insert(pane, (generation, agent));
    drop(shared);
    wake();
    true
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn kind(value: &str) -> anyhow::Result<AgentKind> {
    match value {
        "claude" => Ok(AgentKind::Claude),
        "codex" => Ok(AgentKind::Codex),
        _ => anyhow::bail!("Unknown agent"),
    }
}
/// Delivers one event and reports whether the bridge took it for this pane.
fn send(event: Event, run: &str) -> anyhow::Result<bool> {
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
    let mut ack = [0; 2];
    stream.read_exact(&mut ack)?;
    Ok(&ack == b"ok")
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
            let run = std::env::var(RUN).unwrap_or_default();
            super::agent_mcp::serve(
                std::io::stdin().lock(),
                std::io::stdout().lock(),
                &mut |url| send(Event::PullRequest { url: url.into() }, &run).unwrap_or(false),
            )?;
            Ok(Some(0))
        }
        Some("--agent-run") => {
            let provider = kind(args.get(1).map(String::as_str).unwrap_or_default())?;
            Ok(Some(run_agent(provider, &args[2..], None)?))
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
            let arguments = match &agent.session_id {
                Some(id) => vec![
                    match agent.kind {
                        AgentKind::Claude => "--resume",
                        AgentKind::Codex => "resume",
                    }
                    .into(),
                    id.clone(),
                ],
                None => Vec::new(),
            };
            Ok(Some(run_agent(agent.kind, &arguments, Some(&agent))?))
        }
        _ => Ok(None),
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
fn activity_hooks(provider: AgentKind) -> &'static [&'static str] {
    match provider {
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
fn interactive(provider: AgentKind, args: &[String]) -> bool {
    if args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--help" | "-h" | "--version" | "-V" | "--remote" | "--remote-control"
        )
    }) {
        return false;
    }
    if provider == AgentKind::Claude && args.iter().any(|arg| arg == "-p" || arg == "--print") {
        return false;
    }
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
    };
    // Conservative around option-led subcommands too: never relaunch a batch job.
    !args.iter().any(|arg| commands.contains(&arg.as_str()))
}
fn run_agent(
    provider: AgentKind,
    args: &[String],
    resumed: Option<&AgentSession>,
) -> anyhow::Result<i32> {
    let executable = resolve(provider)?;
    let mut command = std::process::Command::new(executable);
    let nested = std::env::var(RUN).is_ok_and(|value| !value.is_empty());
    if nested
        || !interactive(provider, args)
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || std::env::var(ENDPOINT).is_err()
    {
        command.args(args);
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
    match provider {
        AgentKind::Claude => {
            let mut hooks = serde_json::Map::new();
            hooks.insert(
                "SessionStart".into(),
                serde_json::json!([{"hooks":[{"type":"command","command":hook}]}]),
            );
            // In order, so states follow each other as the agent reached them;
            // the helper answers in milliseconds and never blocks a tool.
            for event in activity_hooks(provider) {
                hooks.insert(
                    (*event).into(),
                    serde_json::json!([{"hooks":[{
                        "type":"command","command":format!("{hook} {event}"),"timeout":5
                    }]}]),
                );
            }
            let settings = serde_json::json!({
                "hooks":hooks,
                "permissions":{"allow":["mcp__neptune__link_pull_request"]},
            });
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
            let activity: Vec<String> = activity_hooks(provider)
                .iter()
                .map(|event| {
                    // Codex allows an interrupt hook three seconds and says so
                    // at every start when asked for more.
                    let timeout = if *event == "Interrupt" { 3 } else { 5 };
                    format!(
                        "hooks.{event}=[{{hooks=[{{type=\"command\",command={},timeout={timeout}}}]}}]",
                        toml::Value::String(format!("{hook} {event}"))
                    )
                })
                .collect();
            let hook = toml::Value::String(hook).to_string();
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
                command.args([
                    "--no-daemon",
                    "-c",
                    &format!(
                        "hooks.SessionStart=[{{hooks=[{{type=\"command\",command={hook}}}]}}]"
                    ),
                ]);
                for hook in &activity {
                    command.args(["-c", hook]);
                }
                command.args([
                    "-c",
                    &format!(
                        "mcp_servers.neptune.command={}",
                        toml::Value::String(helper.clone())
                    ),
                    "-c",
                    "mcp_servers.neptune.args=[\"--agent-mcp\"]",
                ]);
                // Codex excludes *TOKEN* from the default tool/hook environment.
                // Supply only this pane's bridge metadata as invocation-scoped overrides;
                // leave the user's other environment filters and hook trust intact.
                for (name, value) in [
                    (TOKEN, std::env::var(TOKEN).unwrap_or_default()),
                    (ENDPOINT, std::env::var(ENDPOINT).unwrap_or_default()),
                    (RUN, run.clone()),
                    (HELPER, helper.clone()),
                ] {
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
            .arg(command.get_program()).args(command.get_args()).env(RUN, &run).env(HELPER, &helper);
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
        let result = command.env(RUN, &run).env(HELPER, &helper).status();
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
                activity: None,
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
            [(PaneId::new(1), 7, Some(Activity::Idle))]
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
                Some(Activity::NeedsInput(Attention::Question))
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
        assert_eq!(bridge.drain_activity(), [(PaneId::new(1), 7, None)]);
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
            [(PaneId::new(1), 7, Some(Activity::Idle))]
        );
        // A resume that failed keeps the reference but is no running agent.
        bridge.drain();
        assert!(!emit("one", "c", Event::Stopped), "an earlier invocation");
        assert!(emit("one", "d", Event::Stopped));
        assert_eq!(bridge.drain_activity(), [(PaneId::new(1), 7, None)]);
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
                activity: None,
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
        // A link that has not reached a frame leaves with its pane.
        assert!(emit("one", "a", link(url)));
        bridge.close(PaneId::new(1));
        assert!(bridge.drain_links().is_empty());
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
                    activity: None,
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
                .prepare(PaneId::new(1), 1, &mut options, None, Arc::new(|| {}))
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
            .prepare(PaneId::new(1), 1, &mut options, None, Arc::new(|| {}))
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
            configure_shell(&mut options, root.path(), &helper, Some(&agent(FIRST))).unwrap();
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
            .prepare(PaneId::new(1), 1, &mut options, None, Arc::new(|| {}))
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
                Arc::new(|| {}),
            )
            .unwrap();
        assert_eq!(options.shell.as_deref(), Some("/bin/sh"));
        // `shift 2` leaves the shell and its own arguments for `exec "$@"`.
        assert_eq!(options.args[5..], ["/bin/zsh", "-l"]);
    }
}
