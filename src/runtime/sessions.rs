//! Owns live, starting and closing sessions. No PTY is opened on the UI thread.
//! Closing sessions retain their reservation until every worker has stopped.
use super::{
    kept_sessions::{KeptSessions, SessionData, SshData},
    ssh_control::SshControl,
};
use neptune_model::{PaneId, SessionRef};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use terminal_core::{SessionOptions, TerminalSession};

type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    pub max_sessions: usize,
    pub concurrent_spawns: usize,
    pub total_cells: usize,
    pub queue_bytes_per_session: usize,
    pub cache_bytes_per_session: usize,
}
impl Default for ResourcePolicy {
    fn default() -> Self {
        Self {
            max_sessions: 64,
            concurrent_spawns: 2,
            total_cells: 64_000_000,
            queue_bytes_per_session: terminal_core::TRANSPORT_BUFFER_BUDGET,
            cache_bytes_per_session: 4 * 1024 * 1024,
        }
    }
}
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ResourceUsage {
    pub running: usize,
    pub starting: usize,
    pub closing: usize,
    pub reserved_cells: usize,
    pub queue_capacity_bytes: usize,
    pub cache_budget_bytes: usize,
}
/// Desktop launch intent; terminal-core receives only PTY options.
pub struct SessionLaunch {
    pub terminal: SessionOptions,
    pub agent: AgentLaunch,
    /// Give this SSH pane its own private connection-sharing socket.
    pub ssh_control: bool,
    /// Reattach to this kept session rather than start a program.
    pub attach: Option<SessionRef>,
}
pub enum AgentLaunch {
    Disabled,
    /// An SSH connection: adapters are installed on the host, and its agents
    /// report through the terminal.
    Remote,
    Local {
        resume: Option<neptune_model::AgentSession>,
        /// The terminal whose agent started the one this terminal opens,
        /// or the project that did.
        spawned_by: Option<super::agents::Starter>,
    },
}
impl From<SessionOptions> for SessionLaunch {
    fn from(terminal: SessionOptions) -> Self {
        Self {
            terminal,
            agent: AgentLaunch::Disabled,
            ssh_control: false,
            attach: None,
        }
    }
}
struct Request {
    pane: PaneId,
    generation: u64,
    launch: SessionLaunch,
    wake: Wake,
    /// Where sessions are kept, while Neptune keeps them.
    kept: Option<Arc<KeptSessions>>,
}
struct Completion {
    pane: PaneId,
    generation: u64,
    result: Result<Entry, String>,
    elapsed: Duration,
}
struct Jobs {
    pending: VecDeque<Request>,
    stopping: bool,
}
struct Closing {
    pane: PaneId,
    generation: u64,
    session: TerminalSession,
    since: Instant,
    control: Option<Arc<SshControl>>,
}
pub enum SessionCompletion {
    Started {
        pane: PaneId,
        generation: u64,
        elapsed: Duration,
        /// The kept session it started in or reattached to.
        session: Option<SessionRef>,
    },
    Failed {
        pane: PaneId,
        generation: u64,
        message: String,
    },
}
struct Entry {
    generation: u64,
    session: TerminalSession,
    control: Option<Arc<SshControl>>,
    /// Names the kept session in the session daemon; `None` runs in Neptune.
    reference: Option<SessionRef>,
}

pub struct SessionManager {
    policy: ResourcePolicy,
    live: BTreeMap<PaneId, Entry>,
    desired: BTreeMap<PaneId, u64>,
    closing: Vec<Closing>,
    deferred: BTreeMap<PaneId, Request>,
    jobs: Arc<(Mutex<Jobs>, std::sync::Condvar)>,
    completions: mpsc::Receiver<Completion>,
    workers: Vec<thread::JoinHandle<()>>,
    in_flight: BTreeMap<(PaneId, u64), Instant>,
    diagnostics: bool,
    agents: Arc<super::agents::AgentBridge>,
    kept: Option<Arc<KeptSessions>>,
    /// Starts whose results are let go of, not closed, when they arrive.
    detached: BTreeSet<(PaneId, u64)>,
}
impl SessionManager {
    pub fn new(policy: ResourcePolicy, diagnostics: bool) -> Self {
        let jobs = Arc::new((
            Mutex::new(Jobs {
                pending: VecDeque::new(),
                stopping: false,
            }),
            std::sync::Condvar::new(),
        ));
        let (sender, completions) = mpsc::sync_channel(policy.max_sessions.max(1));
        let mut workers = Vec::new();
        let agents = Arc::new(super::agents::AgentBridge::default());
        for index in 0..policy.concurrent_spawns.max(1) {
            let jobs = jobs.clone();
            let sender = sender.clone();
            let agents = agents.clone();
            if let Ok(worker) = thread::Builder::new()
                .name(format!("neptune-spawn-{index}"))
                .spawn(move || {
                    loop {
                        let request = {
                            let (lock, changed) = &*jobs;
                            let Ok(mut state) = lock.lock() else {
                                return;
                            };
                            while state.pending.is_empty() && !state.stopping {
                                let Ok(next) = changed.wait(state) else {
                                    return;
                                };
                                state = next;
                            }
                            if state.stopping {
                                return;
                            }
                            state.pending.pop_front()
                        };
                        let Some(mut request) = request else {
                            continue;
                        };
                        let tick = Instant::now();
                        // A kept SSH session's control socket, reserved for it.
                        let mut reserved = None;
                        let result = (|| {
                            if let Some(reference) = request.launch.attach.take() {
                                let kept = request
                                    .kept
                                    .as_ref()
                                    .ok_or("Terminals are not kept running after quit")?;
                                let terminal = &request.launch.terminal;
                                let (session, data) = kept.attach(
                                    &reference,
                                    terminal.cols,
                                    terminal.rows,
                                    request.wake.clone(),
                                )?;
                                // Its agents reach this launch with the
                                // credential they were started with.
                                if let Some(credential) = &data.agent {
                                    let (agent, spawned_by) = match &request.launch.agent {
                                        AgentLaunch::Local { resume, spawned_by } => {
                                            (resume.as_ref(), *spawned_by)
                                        }
                                        _ => (None, None),
                                    };
                                    agents.adopt(
                                        request.pane,
                                        request.generation,
                                        credential,
                                        agent,
                                        spawned_by,
                                        request.wake.clone(),
                                    );
                                }
                                return Ok(Entry {
                                    generation: request.generation,
                                    session,
                                    control: kept_control(kept, &reference, data),
                                    reference: Some(reference),
                                });
                            }
                            // A kept session's control socket lives with it;
                            // otherwise it goes with Neptune.
                            let mut control = None;
                            if request.launch.ssh_control && cfg!(unix) {
                                const FAILED: &str = "Could not prepare SSH connection sharing";
                                reserved = match &request.kept {
                                    Some(kept) => SshControl::reserve(&kept.ssh_directory())
                                        .map_err(|_| FAILED.to_owned())?,
                                    None => None,
                                };
                                let path = match &reserved {
                                    Some(path) => path.clone(),
                                    None => {
                                        let private = Arc::new(
                                            SshControl::private().map_err(|_| FAILED.to_owned())?,
                                        );
                                        let path = private.path().to_owned();
                                        control = Some(private);
                                        path
                                    }
                                };
                                request.launch.terminal.args.splice(
                                    0..0,
                                    [
                                        "-o".into(),
                                        "ControlMaster=auto".into(),
                                        "-o".into(),
                                        "ControlPersist=no".into(),
                                        "-o".into(),
                                        format!("ControlPath={}", path.display()),
                                    ],
                                );
                            }
                            let mut credential = None;
                            if let AgentLaunch::Local { resume, spawned_by } = &request.launch.agent
                            {
                                credential = agents
                                    .prepare(
                                        request.pane,
                                        request.generation,
                                        &mut request.launch.terminal,
                                        resume.as_ref(),
                                        *spawned_by,
                                        request.wake.clone(),
                                    )
                                    .map_err(|_| "Agent integration could not start".to_owned())?;
                            }
                            if matches!(request.launch.agent, AgentLaunch::Remote) {
                                credential = agents
                                    .prepare_remote(
                                        request.pane,
                                        request.generation,
                                        &mut request.launch.terminal,
                                        request.wake.clone(),
                                    )
                                    .map_err(|_| "Agent integration could not start".to_owned())?;
                            }
                            let (session, reference) = match &request.kept {
                                Some(kept) => {
                                    let (session, reference) =
                                        kept.start(request.launch.terminal, request.wake.clone())?;
                                    let data = SessionData {
                                        agent: credential,
                                        ssh: reserved.clone().map(|control| SshData {
                                            control,
                                            forwards: Vec::new(),
                                        }),
                                    };
                                    if data != SessionData::default() {
                                        // Without it, the session still runs; only its
                                        // agents go unrecognized and its forwards
                                        // unlisted after a reattach.
                                        let _ = kept.set_data(&reference, &data);
                                    }
                                    control =
                                        control.or_else(|| kept_control(kept, &reference, data));
                                    (session, Some(reference))
                                }
                                None => TerminalSession::spawn(
                                    request.launch.terminal,
                                    request.wake.clone(),
                                )
                                .map(|session| (session, None))
                                .map_err(|error| format!("{error:#}"))?,
                            };
                            Ok(Entry {
                                generation: request.generation,
                                session,
                                control,
                                reference,
                            })
                        })();
                        if result.is_err()
                            && let Some(directory) = reserved.as_deref().and_then(|s| s.parent())
                        {
                            let _ = std::fs::remove_dir_all(directory);
                        }
                        if sender
                            .send(Completion {
                                pane: request.pane,
                                generation: request.generation,
                                result,
                                elapsed: tick.elapsed(),
                            })
                            .is_err()
                        {
                            return;
                        }
                        (request.wake)();
                    }
                })
            {
                workers.push(worker);
            }
        }
        Self {
            policy,
            live: BTreeMap::new(),
            desired: BTreeMap::new(),
            closing: Vec::new(),
            deferred: BTreeMap::new(),
            jobs,
            completions,
            workers,
            in_flight: BTreeMap::new(),
            diagnostics,
            agents,
            kept: None,
            detached: BTreeSet::new(),
        }
    }
    /// From now on, start and attach terminals in the session daemon `kept`
    /// reaches, so that they outlive Neptune.
    pub fn keep_sessions(&mut self, kept: KeptSessions) {
        self.kept = Some(Arc::new(kept));
    }
    pub fn policy(&self) -> &ResourcePolicy {
        &self.policy
    }
    pub fn get(&self, pane: PaneId) -> Option<&TerminalSession> {
        self.live.get(&pane).map(|entry| &entry.session)
    }
    pub fn ssh_control(&self, pane: PaneId) -> Option<Arc<SshControl>> {
        self.live.get(&pane).and_then(|entry| entry.control.clone())
    }
    pub fn generation(&self, pane: PaneId) -> Option<u64> {
        self.desired
            .get(&pane)
            .copied()
            .or_else(|| self.live.get(&pane).map(|entry| entry.generation))
    }
    pub fn iter(&self) -> impl Iterator<Item = (PaneId, &TerminalSession)> {
        self.live.iter().map(|(id, entry)| (*id, &entry.session))
    }
    pub fn usage(&self) -> ResourceUsage {
        // Each pending/replacement reservation is counted, including orphaned starts.
        let count = self.live.len() + self.in_flight.len() + self.closing.len();
        ResourceUsage {
            running: self.live.len(),
            starting: self.in_flight.len() + self.deferred.len(),
            closing: self.closing.len(),
            reserved_cells: count * self.cell_budget(),
            queue_capacity_bytes: count * self.policy.queue_bytes_per_session,
            cache_budget_bytes: count * self.policy.cache_bytes_per_session,
        }
    }
    fn cell_budget(&self) -> usize {
        self.policy.total_cells / self.policy.max_sessions.max(1)
    }
    pub fn history_limit(&self, cols: u16, rows: u16, requested: usize) -> usize {
        requested.min(
            self.cell_budget()
                .checked_div(usize::from(cols).max(1))
                .unwrap_or(0)
                .saturating_sub(2 * usize::from(rows)),
        )
    }
    pub fn start(
        &mut self,
        pane: PaneId,
        generation: u64,
        replacement: bool,
        launch: impl Into<SessionLaunch>,
        wake: Wake,
    ) -> Result<(), String> {
        self.reap();
        let mut launch = launch.into();
        let options = &mut launch.terminal;
        if self.workers.is_empty() {
            return Err("Shell startup worker could not be created".into());
        }
        if replacement && !self.desired.contains_key(&pane) {
            return Err("A replacement must target an existing pane session".into());
        }
        if 2 * usize::from(options.rows) * usize::from(options.cols) > self.cell_budget() {
            return Err(
                "Terminal geometry exceeds this session's aggregate cell reservation".into(),
            );
        }
        let replacing = self.desired.contains_key(&pane);
        let count = self.live.len() + self.in_flight.len() + self.closing.len();
        if !replacing && count + self.deferred.len() >= self.policy.max_sessions {
            return Err(format!(
                "Up to {} sessions may be running, starting or closing",
                self.policy.max_sessions
            ));
        }
        options.scrollback = self.history_limit(options.cols, options.rows, options.scrollback);
        self.desired.insert(pane, generation);
        self.cancel_pending(pane);
        if let Some(old) = self.live.remove(&pane) {
            old.session.shutdown();
            self.closing.push(Closing {
                pane,
                generation: old.generation,
                session: old.session,
                since: Instant::now(),
                control: old.control,
            });
        }
        let request = Request {
            pane,
            generation,
            launch,
            wake,
            kept: self.kept.clone(),
        };
        if count >= self.policy.max_sessions || self.in_flight.keys().any(|(id, _)| *id == pane) {
            self.deferred.insert(pane, request);
        } else {
            self.enqueue(request)?;
        }
        Ok(())
    }
    fn enqueue(&mut self, request: Request) -> Result<(), String> {
        let (lock, changed) = &*self.jobs;
        let mut state = lock
            .lock()
            .map_err(|_| "Startup queue failed".to_string())?;
        if state.stopping || state.pending.len() >= self.policy.max_sessions {
            return Err("Startup queue is closed or full".into());
        }
        self.in_flight
            .insert((request.pane, request.generation), Instant::now());
        state.pending.push_back(request);
        changed.notify_one();
        Ok(())
    }
    fn cancel_pending(&mut self, pane: PaneId) {
        self.deferred.remove(&pane);
        if let Ok(mut jobs) = self.jobs.0.lock() {
            jobs.pending.retain(|request| {
                if request.pane == pane {
                    self.in_flight.remove(&(request.pane, request.generation));
                    false
                } else {
                    true
                }
            });
        }
    }
    pub fn agent_changes(&self) -> Vec<(PaneId, u64, Option<neptune_model::AgentSession>)> {
        self.agents.drain()
    }
    /// What agents turned to since the last call; `None` for one that left.
    /// An agent on an SSH host comes with its CLI.
    pub fn agent_activity(
        &self,
    ) -> Vec<(
        PaneId,
        u64,
        Option<crate::agent_activity::Activity>,
        Option<neptune_model::AgentKind>,
    )> {
        self.agents.drain_activity()
    }
    pub fn pull_request_links(&self) -> Vec<(PaneId, u64, neptune_model::PullRequest)> {
        self.agents.drain_links()
    }
    pub fn attached_files(&self) -> Vec<(PaneId, u64, neptune_model::Attachment)> {
        self.agents.drain_attachments()
    }
    /// The bridge agents reach the application through: what they asked for,
    /// and what the application tells it about the agents they started.
    /// The bridge itself, for a worker that opens a project's lead to it.
    pub fn bridge(&self) -> Arc<super::agents::AgentBridge> {
        self.agents.clone()
    }
    pub fn agents(&self) -> &super::agents::AgentBridge {
        &self.agents
    }
    pub fn close(&mut self, pane: PaneId) {
        self.agents.close(pane);
        self.desired.remove(&pane);
        self.cancel_pending(pane);
        if let Some(entry) = self.live.remove(&pane) {
            entry.session.shutdown();
            self.closing.push(Closing {
                pane,
                generation: entry.generation,
                session: entry.session,
                since: Instant::now(),
                control: entry.control,
            });
        }
    }
    /// Let go of a pane's kept session without ending it, as Neptune quits.
    /// A start or reattach still under way is let go of when it completes.
    /// A session that runs in Neptune closes.
    pub fn detach(&mut self, pane: PaneId) {
        if self
            .live
            .get(&pane)
            .is_some_and(|entry| entry.reference.is_none())
        {
            return self.close(pane);
        }
        self.desired.remove(&pane);
        self.cancel_pending(pane);
        // Dropping a daemon-backed handle detaches it.
        self.live.remove(&pane);
        self.detached
            .extend(self.in_flight.keys().filter(|(id, _)| *id == pane).copied());
    }
    fn reap(&mut self) {
        self.closing.retain(|closing| {
            let pending = closing.session.metrics().active_workers != 0;
            if !pending && let Some(control) = &closing.control {
                // A kept session's socket outlives Neptune, but not its close.
                control.remove();
            }
            if !pending && self.diagnostics { eprintln!("{}", serde_json::json!({"operation":"cleanup","pane":closing.pane.get(),"session_generation":closing.generation,"elapsed_ms":closing.since.elapsed().as_secs_f64()*1000.0})); }
            pending
        });
    }
    pub fn poll(&mut self) -> Vec<SessionCompletion> {
        self.reap();
        let mut events = Vec::new();
        while let Ok(completion) = self.completions.try_recv() {
            self.in_flight
                .remove(&(completion.pane, completion.generation));
            if self
                .detached
                .remove(&(completion.pane, completion.generation))
            {
                continue;
            }
            if self.desired.get(&completion.pane) != Some(&completion.generation) {
                self.agents
                    .close_generation(completion.pane, completion.generation);
                if let Ok(entry) = completion.result {
                    entry.session.shutdown();
                    self.closing.push(Closing {
                        pane: completion.pane,
                        generation: completion.generation,
                        session: entry.session,
                        since: Instant::now(),
                        control: entry.control,
                    });
                }
                continue;
            }
            match completion.result {
                Ok(entry) => {
                    let session = entry.reference.clone();
                    self.live.insert(completion.pane, entry);
                    events.push(SessionCompletion::Started {
                        pane: completion.pane,
                        generation: completion.generation,
                        elapsed: completion.elapsed,
                        session,
                    });
                }
                Err(message) => {
                    self.agents
                        .close_generation(completion.pane, completion.generation);
                    events.push(SessionCompletion::Failed {
                        pane: completion.pane,
                        generation: completion.generation,
                        message,
                    });
                }
            }
        }
        let available = self
            .policy
            .max_sessions
            .saturating_sub(self.live.len() + self.in_flight.len() + self.closing.len());
        let ready: Vec<_> = self
            .deferred
            .keys()
            .filter(|id| !self.in_flight.keys().any(|(pending, _)| pending == *id))
            .take(available)
            .copied()
            .collect();
        for id in ready {
            if let Some(request) = self.deferred.remove(&id) {
                let generation = request.generation;
                if let Err(message) = self.enqueue(request) {
                    events.push(SessionCompletion::Failed {
                        pane: id,
                        generation,
                        message,
                    });
                }
            }
        }
        events
    }
    pub fn resize(
        &self,
        pane: PaneId,
        cols: u16,
        rows: u16,
        width: u16,
        height: u16,
        history: usize,
    ) -> Result<(), String> {
        let Some(session) = self.get(pane) else {
            return Ok(());
        };
        if 2 * usize::from(rows) * usize::from(cols) > self.cell_budget() {
            return Err(
                "Terminal geometry exceeds this session's aggregate cell reservation".into(),
            );
        }
        let target_history = self.history_limit(cols, rows, history);
        let current = session.viewport();
        let current_limit =
            self.history_limit(current.columns as u16, current.screen_lines as u16, history);
        // Either old or new geometry can be wider. Release history before resize,
        // then grow it only once the grid uses the destination dimensions.
        session
            .set_scrollback(target_history.min(current_limit))
            .map_err(|error| error.to_string())?;
        session
            .resize(cols, rows, width, height)
            .map_err(|error| error.to_string())?;
        session
            .set_scrollback(target_history)
            .map_err(|error| error.to_string())
    }
    /// End every kept session, those no terminal shows included. Only a
    /// running session daemon is asked; none is launched for it.
    pub fn end_kept(&self) -> Result<(), String> {
        self.kept.as_ref().map_or(Ok(()), |kept| kept.end_all())
    }
    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        let ids: Vec<_> = self.desired.keys().copied().collect();
        for id in ids {
            self.close(id);
        }
        if let Ok(mut jobs) = self.jobs.0.lock() {
            jobs.stopping = true;
            jobs.pending.clear();
            self.jobs.1.notify_all();
        }
        let deadline = Instant::now() + timeout;
        loop {
            self.poll();
            if self.closing.is_empty() && self.in_flight.is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}
/// A kept SSH terminal's control socket, as recorded with its session. Each
/// change to the forwards made through it is kept with the session too.
fn kept_control(
    kept: &Arc<KeptSessions>,
    reference: &SessionRef,
    data: SessionData,
) -> Option<Arc<SshControl>> {
    let ssh = data.ssh.clone()?;
    // Only a socket in a directory of Neptune's own is ever removed.
    let directory = ssh.control.parent()?;
    if directory.parent() != Some(kept.ssh_directory().as_path()) {
        return None;
    }
    let (kept, reference, data) = (kept.clone(), reference.clone(), Mutex::new(data));
    SshControl::kept(
        ssh.control,
        ssh.forwards,
        Box::new(move |forwards| {
            let Ok(mut data) = data.lock() else {
                return;
            };
            if let Some(ssh) = &mut data.ssh {
                ssh.forwards = forwards.to_vec();
            }
            let _ = kept.set_data(&reference, &data);
        }),
    )
    .map(Arc::new)
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(2));
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn policy() -> ResourcePolicy {
        ResourcePolicy {
            max_sessions: 1,
            concurrent_spawns: 1,
            total_cells: 20_000,
            ..ResourcePolicy::default()
        }
    }
    fn options() -> SessionOptions {
        SessionOptions {
            shell: Some("/bin/sh".into()),
            args: vec!["-c".into(), "printf 'READY\r\n'; sleep 30".into()],
            cols: 80,
            rows: 12,
            scrollback: 128,
            ..SessionOptions::default()
        }
    }
    #[track_caller]
    fn until(
        manager: &mut SessionManager,
        mut condition: impl FnMut(&SessionManager, &[SessionCompletion]) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events = manager.poll();
            assert!(
                manager.usage().running
                    + manager
                        .usage()
                        .starting
                        .saturating_sub(manager.deferred.len())
                    + manager.usage().closing
                    <= manager.policy.max_sessions,
                "Manager exceeded aggregate reservation capacity"
            );
            if condition(manager, &events) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Session manager operation timed out: usage={:?}, closing workers={:?}",
                manager.usage(),
                manager
                    .closing
                    .iter()
                    .map(|entry| entry.session.shutdown_completion().active_workers())
                    .collect::<Vec<_>>()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    /// A session daemon serving a temporary directory, on a thread of this
    /// test. It exits shortly after its last session and client are gone.
    fn daemon() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        thread::JoinHandle<()>,
    ) {
        let root = tempfile::tempdir().unwrap();
        let dir = super::super::kept_sessions::directory(root.path());
        let serving = dir.clone();
        let daemon = thread::spawn(move || {
            terminal_core::daemon::run(
                &serving,
                terminal_core::daemon::DaemonOptions {
                    idle_exit: Duration::from_millis(200),
                },
            )
            .unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while terminal_core::daemon::DaemonClient::connect(&dir).is_err() {
            assert!(
                Instant::now() < deadline,
                "the session daemon never listened"
            );
            thread::sleep(Duration::from_millis(5));
        }
        (root, dir, daemon)
    }
    fn kept(dir: &std::path::Path) -> SessionManager {
        let mut manager = SessionManager::new(policy(), false);
        manager.keep_sessions(super::super::kept_sessions::KeptSessions::new(
            dir.into(),
            None,
        ));
        manager
    }
    fn started(events: &[SessionCompletion]) -> Option<Option<SessionRef>> {
        events.first().map(|event| match event {
            SessionCompletion::Started { session, .. } => session.clone(),
            SessionCompletion::Failed { message, .. } => panic!("{message}"),
        })
    }
    fn listed(dir: &std::path::Path) -> Vec<(String, terminal_core::SessionStatus)> {
        terminal_core::daemon::DaemonClient::connect(dir)
            .unwrap()
            .list()
            .unwrap()
            .into_iter()
            .map(|entry| (entry.id.to_string(), entry.status))
            .collect()
    }
    fn activity(manager: &SessionManager, pane: PaneId) -> terminal_core::ProcessActivity {
        manager
            .get(pane)
            .unwrap()
            .check_process_activity()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
    }

    #[test]
    fn kept_sessions_outlive_their_manager_reattach_and_end_on_close() {
        let (_root, dir, daemon) = daemon();
        let pane = PaneId::new(1);
        let wake: Wake = Arc::new(|| {});
        let mut first = kept(&dir);
        first
            .start(pane, 1, false, options(), wake.clone())
            .unwrap();
        let mut reference = None;
        until(&mut first, |manager, events| {
            reference = reference.take().or_else(|| started(events));
            reference.is_some()
                && manager
                    .get(pane)
                    .is_some_and(|session| session.screen_text().contains("READY"))
        });
        let reference = reference.unwrap().expect("a kept session has a reference");
        // Its shell has a program running, which a close would stop.
        assert_eq!(
            activity(&first, pane),
            terminal_core::ProcessActivity::Running
        );
        first.detach(pane);
        assert!(first.shutdown(Duration::from_secs(2)));
        drop(first);
        assert_eq!(
            listed(&dir),
            vec![(reference.id.clone(), terminal_core::SessionStatus::Running)]
        );

        let mut second = kept(&dir);
        second
            .start(
                pane,
                1,
                false,
                SessionLaunch {
                    attach: Some(reference.clone()),
                    ..options().into()
                },
                wake.clone(),
            )
            .unwrap();
        let mut attached = None;
        until(&mut second, |manager, events| {
            attached = attached.take().or_else(|| started(events));
            attached.is_some()
                && manager
                    .get(pane)
                    .is_some_and(|session| session.screen_text().contains("READY"))
        });
        assert_eq!(attached, Some(Some(reference)));
        second.close(pane);
        until(&mut second, |manager, _| manager.usage().closing == 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !listed(&dir).is_empty() {
            assert!(
                Instant::now() < deadline,
                "closing left the session running"
            );
            thread::sleep(Duration::from_millis(10));
        }
        drop(second);
        daemon.join().unwrap();
    }

    /// A kept SSH terminal's control socket lives with its session: it
    /// outlives the manager that started it, comes back with the forwards
    /// made through it on reattach, and goes when the session closes.
    #[test]
    fn a_kept_ssh_terminals_control_socket_outlives_neptune_and_goes_with_close() {
        use super::super::ssh_control::KeptForward;
        use std::os::unix::fs::PermissionsExt;
        let (root, dir, daemon) = daemon();
        // An ssh stand-in that listens where it is told to, as a master.
        let client = root.path().join("ssh");
        std::fs::write(
            &client,
            "#!/bin/sh\nwhile [ \"$1\" = -o ]; do case \"$2\" in ControlPath=*) : > \"${2#ControlPath=}\";; esac; shift 2; done\nprintf 'READY\\r\\n'\nexec sleep 30\n",
        )
        .unwrap();
        std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();
        let pane = PaneId::new(1);
        let wake: Wake = Arc::new(|| {});
        let launch = |attach: Option<SessionRef>| SessionLaunch {
            terminal: SessionOptions {
                shell: Some(client.to_str().unwrap().into()),
                args: Vec::new(),
                ..options()
            },
            agent: AgentLaunch::Disabled,
            ssh_control: attach.is_none(),
            attach,
        };
        let ready = |manager: &SessionManager| {
            manager
                .get(pane)
                .is_some_and(|session| session.screen_text().contains("READY"))
        };
        let mut first = kept(&dir);
        first
            .start(pane, 1, false, launch(None), wake.clone())
            .unwrap();
        let mut reference = None;
        until(&mut first, |manager, events| {
            reference = reference.take().or_else(|| started(events));
            reference.is_some() && ready(manager)
        });
        let reference = reference.unwrap().unwrap();
        let control = first.ssh_control(pane).unwrap();
        let socket = control.path().to_owned();
        assert!(socket.starts_with(dir.join("ssh")), "{socket:?}");
        assert!(socket.exists());
        let forward = KeptForward {
            listener: crate::platform::ports::Listener {
                address: std::net::Ipv4Addr::LOCALHOST.into(),
                port: 3000,
            },
            local: 4000,
        };
        control.record(&[forward]);
        drop(control);
        first.detach(pane);
        assert!(first.shutdown(Duration::from_secs(2)));
        drop(first);
        assert!(
            socket.exists(),
            "Neptune quitting removed the control socket"
        );

        let mut second = kept(&dir);
        second
            .start(pane, 2, false, launch(Some(reference)), wake.clone())
            .unwrap();
        until(&mut second, |manager, _| ready(manager));
        let control = second.ssh_control(pane).unwrap();
        assert_eq!(control.path(), socket);
        assert_eq!(control.forwards(), [forward]);
        drop(control);
        second.close(pane);
        until(&mut second, |manager, _| manager.usage().closing == 0);
        assert!(!socket.exists(), "closing left the control socket");
        assert!(!socket.parent().unwrap().exists());
        drop(second);
        daemon.join().unwrap();
    }

    #[test]
    fn a_kept_terminals_agents_reach_the_launch_that_reattaches_it() {
        use super::super::agents::{Event, Signal, exchange_with};
        let (root, dir, daemon) = daemon();
        let bridge = root.path().join("agents");
        let pane = PaneId::new(1);
        let wake: Wake = Arc::new(|| {});
        let terminal = SessionOptions {
            args: vec![
                "-c".into(),
                "printf 'AT=%s\\r\\nAS=%s\\r\\n' \"$NEPTUNE_AGENT_ENDPOINT\" \"$NEPTUNE_AGENT_TOKEN\"; sleep 30"
                    .into(),
            ],
            ..options()
        };
        let launch = |resume, attach| SessionLaunch {
            terminal: terminal.clone(),
            agent: AgentLaunch::Local {
                resume,
                spawned_by: None,
            },
            ssh_control: false,
            attach,
        };
        let mut first = kept(&dir);
        first.agents().listen_in(bridge.clone());
        first
            .start(pane, 1, false, launch(None, None), wake.clone())
            .unwrap();
        let mut reference = None;
        until(&mut first, |manager, events| {
            reference = reference.take().or_else(|| started(events));
            reference.is_some()
                && manager
                    .get(pane)
                    .is_some_and(|session| session.screen_text().contains("AS="))
        });
        let reference = reference.unwrap().unwrap();
        let screen = first.get(pane).unwrap().screen_text();
        let shown = |name: &str| {
            screen
                .lines()
                .find_map(|line| line.trim().strip_prefix(name))
                .unwrap()
                .to_owned()
        };
        let (endpoint, token) = (shown("AT="), shown("AS="));
        assert_eq!(std::path::Path::new(&endpoint), bridge.join("bridge.sock"));
        first.detach(pane);
        assert!(first.shutdown(Duration::from_secs(2)));
        drop(first);
        // Its startup workers let go of the bridge as they stop; Neptune's
        // own process would have exited.
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::path::Path::new(&endpoint).exists() {
            assert!(Instant::now() < deadline, "the bridge outlived its launch");
            thread::sleep(Duration::from_millis(10));
        }
        let tool = || Event::Activity {
            signal: Signal::Tool,
        };
        // While Neptune is closed, its agents are not heard, and not held up.
        let asked = Instant::now();
        assert!(exchange_with(&endpoint, &token, tool(), "a").is_err());
        assert!(asked.elapsed() < Duration::from_secs(1));

        // The next launch knows the agent it saved from its next hook.
        let saved = neptune_model::AgentSession {
            kind: neptune_model::AgentKind::Claude,
            session_id: Some("019a1234-5678-7000-8000-123456789abc".into()),
            cwd: root.path().into(),
        };
        let mut second = kept(&dir);
        second.agents().listen_in(bridge);
        second
            .start(
                pane,
                2,
                false,
                launch(Some(saved), Some(reference)),
                wake.clone(),
            )
            .unwrap();
        until(&mut second, |manager, _| manager.get(pane).is_some());
        assert_eq!(
            exchange_with(&endpoint, "guess", tool(), "a").unwrap(),
            b"no"
        );
        assert_eq!(
            exchange_with(&endpoint, &token, tool(), "a").unwrap(),
            b"ok"
        );
        assert_eq!(
            second.agent_activity(),
            [(
                pane,
                2,
                Some(crate::agent_activity::Activity::Working),
                None
            )]
        );
        second.close(pane);
        until(&mut second, |manager, _| manager.usage().closing == 0);
        drop(second);
        daemon.join().unwrap();
    }

    #[test]
    fn a_kept_shell_at_its_prompt_is_idle() {
        let (_root, dir, daemon) = daemon();
        let pane = PaneId::new(1);
        let mut manager = kept(&dir);
        let shell = SessionOptions {
            shell: Some("/bin/sh".into()),
            args: Vec::new(),
            ..options()
        };
        manager
            .start(pane, 1, false, shell, Arc::new(|| {}))
            .unwrap();
        until(&mut manager, |manager, _| manager.get(pane).is_some());
        assert_eq!(
            activity(&manager, pane),
            terminal_core::ProcessActivity::Idle
        );
        assert!(manager.shutdown(Duration::from_secs(2)));
        drop(manager);
        daemon.join().unwrap();
    }

    #[test]
    fn aggregate_history_policy_accounts_for_both_visible_grids() {
        let manager = SessionManager::new(policy(), false);
        assert_eq!(manager.history_limit(80, 12, 10_000), 20_000 / 80 - 24);
        assert_eq!(manager.history_limit(4096, 4096, 10_000), 0);
    }
    #[test]
    fn restart_at_capacity_defers_replacement_until_cleanup_and_tracks_generation() {
        let mut manager = SessionManager::new(policy(), false);
        let pane = PaneId::new(1);
        manager
            .start(pane, 1, false, options(), Arc::new(|| {}))
            .unwrap();
        until(&mut manager, |manager, _| {
            manager
                .get(pane)
                .is_some_and(|session| session.screen_text().contains("READY"))
        });
        let old_pid = manager.get(pane).unwrap().metadata().process_id;
        manager
            .start(pane, 2, true, options(), Arc::new(|| {}))
            .unwrap();
        assert_eq!(manager.usage().closing, 1);
        assert_eq!(manager.usage().starting, 1);
        assert!(manager.get(pane).is_none());
        assert_eq!(manager.generation(pane), Some(2));
        until(&mut manager, |manager, events| {
            assert!(
                events.iter().all(|event| !matches!(
                    event,
                    SessionCompletion::Started { generation: 1, .. }
                ))
            );
            manager
                .get(pane)
                .is_some_and(|session| session.screen_text().contains("READY"))
        });
        assert_ne!(manager.get(pane).unwrap().metadata().process_id, old_pid);
        assert!(manager.shutdown(Duration::from_secs(5)));
        assert_eq!(manager.usage().reserved_cells, 0);
    }
    #[test]
    fn closing_retains_resources_until_all_workers_stop() {
        let mut manager = SessionManager::new(policy(), false);
        let pane = PaneId::new(1);
        manager
            .start(pane, 1, false, options(), Arc::new(|| {}))
            .unwrap();
        until(&mut manager, |manager, _| {
            manager
                .get(pane)
                .is_some_and(|session| session.screen_text().contains("READY"))
        });
        let completion = manager.get(pane).unwrap().shutdown_completion();
        manager.close(pane);
        assert!(manager.get(pane).is_none());
        assert_eq!(manager.usage().closing, 1);
        assert_eq!(manager.usage().reserved_cells, 20_000);
        // A new identity cannot spend the retiring session's reservation.
        if !completion.is_complete() {
            assert!(
                manager
                    .start(PaneId::new(2), 1, false, options(), Arc::new(|| {}))
                    .is_err()
            );
        }
        until(&mut manager, |manager, _| manager.usage().closing == 0);
        assert!(completion.is_complete());
        assert_eq!(manager.usage().reserved_cells, 0);
    }
    #[test]
    fn close_during_start_discards_stale_results_and_releases_every_reservation() {
        let mut manager = SessionManager::new(policy(), false);
        let pane = PaneId::new(1);
        manager
            .start(pane, 1, false, options(), Arc::new(|| {}))
            .unwrap();
        manager.close(pane);
        until(&mut manager, |manager, events| {
            assert!(
                events.is_empty(),
                "Closed pane accepted a stale startup completion"
            );
            manager.usage().reserved_cells == 0
        });
        assert!(manager.get(pane).is_none());
        assert_eq!(manager.generation(pane), None);
        assert!(manager.shutdown(Duration::from_secs(5)));
    }
    #[test]
    fn failed_spawn_is_targeted_and_releases_capacity_for_another_pane() {
        let mut manager = SessionManager::new(policy(), false);
        let pane = PaneId::new(1);
        let mut bad = options();
        bad.shell = Some("/missing/neptune-test-shell".into());
        manager.start(pane, 7, false, bad, Arc::new(|| {})).unwrap();
        until(&mut manager, |_, events| {
            events.iter().any(|event|matches!(event,SessionCompletion::Failed{pane:id,generation:7,..} if *id==pane))
        });
        assert_eq!(manager.usage().reserved_cells, 0);
        manager
            .start(PaneId::new(2), 1, false, options(), Arc::new(|| {}))
            .unwrap();
        until(&mut manager, |manager, _| {
            manager.get(PaneId::new(2)).is_some()
        });
        assert!(manager.shutdown(Duration::from_secs(5)));
    }
    #[test]
    fn geometry_policy_rejects_unbudgeted_grids_without_changing_a_live_session() {
        let mut manager = SessionManager::new(policy(), false);
        let pane = PaneId::new(1);
        manager
            .start(pane, 1, false, options(), Arc::new(|| {}))
            .unwrap();
        until(&mut manager, |manager, _| manager.get(pane).is_some());
        assert!(manager.resize(pane, 4096, 4096, 0, 0, 0).is_err());
        let snapshot = manager.get(pane).unwrap().viewport();
        assert_eq!((snapshot.columns, snapshot.screen_lines), (80, 12));
        manager.resize(pane, 100, 12, 0, 0, 10_000).unwrap();
        manager.resize(pane, 40, 12, 0, 0, 10_000).unwrap();
        let snapshot = manager.get(pane).unwrap().viewport();
        assert_eq!((snapshot.columns, snapshot.screen_lines), (40, 12));
        assert!(manager.shutdown(Duration::from_secs(5)));
    }
    #[test]
    fn replacements_cannot_bypass_capacity_with_an_unrelated_pane() {
        let mut manager = SessionManager::new(policy(), false);
        assert!(
            manager
                .start(PaneId::new(99), 1, true, options(), Arc::new(|| {}))
                .is_err()
        );
        assert_eq!(manager.usage().starting, 0);
    }
}
