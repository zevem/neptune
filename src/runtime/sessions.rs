//! Owns live, starting and closing sessions. No PTY is opened on the UI thread.
//! Closing sessions retain their reservation until every worker has stopped.
use neptune_model::PaneId;
use std::{
    collections::{BTreeMap, VecDeque},
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
}
pub enum AgentLaunch {
    Disabled,
    /// An SSH connection: adapters are installed on the host, and its agents
    /// report through the terminal.
    Remote,
    Local {
        resume: Option<neptune_model::AgentSession>,
        /// The terminal whose agent started the one this terminal opens.
        spawned_by: Option<PaneId>,
    },
}
impl From<SessionOptions> for SessionLaunch {
    fn from(terminal: SessionOptions) -> Self {
        Self {
            terminal,
            agent: AgentLaunch::Disabled,
        }
    }
}
struct Request {
    pane: PaneId,
    generation: u64,
    launch: SessionLaunch,
    wake: Wake,
}
struct Completion {
    pane: PaneId,
    generation: u64,
    result: Result<TerminalSession, String>,
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
}
pub enum SessionCompletion {
    Started {
        pane: PaneId,
        generation: u64,
        elapsed: Duration,
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
                        let result = (|| {
                            if let AgentLaunch::Local { resume, spawned_by } = &request.launch.agent
                            {
                                agents
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
                                agents
                                    .prepare_remote(
                                        request.pane,
                                        request.generation,
                                        &mut request.launch.terminal,
                                        request.wake.clone(),
                                    )
                                    .map_err(|_| "Agent integration could not start".to_owned())?;
                            }
                            TerminalSession::spawn(request.launch.terminal, request.wake.clone())
                                .map_err(|error| format!("{error:#}"))
                        })();
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
        }
    }
    pub fn policy(&self) -> &ResourcePolicy {
        &self.policy
    }
    pub fn get(&self, pane: PaneId) -> Option<&TerminalSession> {
        self.live.get(&pane).map(|entry| &entry.session)
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
            });
        }
        let request = Request {
            pane,
            generation,
            launch,
            wake,
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
            });
        }
    }
    fn reap(&mut self) {
        self.closing.retain(|closing| {
            let pending = closing.session.metrics().active_workers != 0;
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
            if self.desired.get(&completion.pane) != Some(&completion.generation) {
                self.agents
                    .close_generation(completion.pane, completion.generation);
                if let Ok(session) = completion.result {
                    session.shutdown();
                    self.closing.push(Closing {
                        pane: completion.pane,
                        generation: completion.generation,
                        session,
                        since: Instant::now(),
                    });
                }
                continue;
            }
            match completion.result {
                Ok(session) => {
                    self.live.insert(
                        completion.pane,
                        Entry {
                            generation: completion.generation,
                            session,
                        },
                    );
                    events.push(SessionCompletion::Started {
                        pane: completion.pane,
                        generation: completion.generation,
                        elapsed: completion.elapsed,
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
