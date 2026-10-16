//! One writer owns all destinations. Frames publish immutable replacements;
//! there is at most one pending snapshot per destination.
//! Serialization, fsync and replacement happen only on this worker. Submitted
//! generations are monotonic per destination, so superseded work cannot regress
//! a saved file. Flushing belongs at the intentional shutdown boundary.

use crate::{
    config::{self, Config},
    persistence::{window_state::WindowState, workspace_state::StateSnapshot},
};
use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveKind {
    State,
    Config,
    Window,
}
impl SaveKind {
    fn index(self) -> usize {
        match self {
            Self::State => 0,
            Self::Config => 1,
            Self::Window => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveEvent {
    pub kind: SaveKind,
    pub generation: u64,
    pub result: Result<(), String>,
}

enum Payload {
    State(StateSnapshot),
    Config(Config),
    Window(WindowState),
}
struct Job {
    kind: SaveKind,
    generation: u64,
    payload: Payload,
}
type WriteOperation = dyn Fn(&Job) -> Result<(), String> + Send + Sync;

#[derive(Default)]
struct Queue {
    pending: [Option<Job>; 3],
    latest: [Option<u64>; 3],
    completed: [Option<SaveEvent>; 3],
    events: [Option<SaveEvent>; 3],
    stopping: bool,
    exited: bool,
    next_kind: usize,
    last_submit: Option<Instant>,
    flushing: bool,
}
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

pub struct PersistenceWriter {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    enabled: bool,
}

impl PersistenceWriter {
    pub fn new(
        state_path: PathBuf,
        config_path: PathBuf,
        window_path: PathBuf,
        enabled: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<Self> {
        Self::with_operation(
            enabled,
            wake,
            Arc::new(move |job| match &job.payload {
                Payload::State(snapshot) => {
                    let bytes = serde_json::to_vec_pretty(snapshot)
                        .map_err(|error| format!("Cannot serialize workspace state: {error}"))?;
                    config::atomic_write(&state_path, &bytes).map_err(|error| {
                        format!(
                            "Cannot save workspaces to {}: {error:#}",
                            state_path.display()
                        )
                    })
                }
                Payload::Config(config) => config.save(&config_path).map_err(|error| {
                    format!(
                        "Cannot save preferences to {}: {error:#}",
                        config_path.display()
                    )
                }),
                Payload::Window(window) => {
                    let bytes = serde_json::to_vec_pretty(window)
                        .map_err(|error| format!("Cannot serialize window state: {error}"))?;
                    config::atomic_write(&window_path, &bytes).map_err(|error| {
                        format!("Cannot save window to {}: {error:#}", window_path.display())
                    })
                }
            }),
        )
    }

    fn with_operation(
        enabled: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
        operation: Arc<WriteOperation>,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("neptune-persistence".into())
            .spawn(move || worker_loop(&worker_shared, &wake, &operation))?;
        Ok(Self {
            shared,
            worker: Some(worker),
            enabled,
        })
    }

    pub fn submit_state(&self, generation: u64, snapshot: StateSnapshot) -> Result<(), String> {
        self.submit(Job {
            kind: SaveKind::State,
            generation,
            payload: Payload::State(snapshot),
        })
    }
    pub fn submit_config(&self, generation: u64, config: Config) -> Result<(), String> {
        self.submit(Job {
            kind: SaveKind::Config,
            generation,
            payload: Payload::Config(config),
        })
    }
    pub fn submit_window(&self, generation: u64, window: WindowState) -> Result<(), String> {
        self.submit(Job {
            kind: SaveKind::Window,
            generation,
            payload: Payload::Window(window),
        })
    }

    fn submit(&self, job: Job) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let mut queue = self
            .shared
            .queue
            .lock()
            .map_err(|_| "Persistence queue poisoned".to_string())?;
        if queue.stopping {
            return Err("Persistence writer has shut down".into());
        }
        let index = job.kind.index();
        if queue.latest[index].is_some_and(|latest| job.generation <= latest) {
            return Ok(());
        }
        queue.latest[index] = Some(job.generation);
        queue.pending[index] = Some(job);
        queue.last_submit = Some(Instant::now());
        self.shared.ready.notify_all();
        Ok(())
    }

    /// Events are bounded, retaining the newest completion for each destination.
    pub fn drain_events(&self) -> Vec<SaveEvent> {
        let Ok(mut queue) = self.shared.queue.lock() else {
            return vec![SaveEvent {
                kind: SaveKind::State,
                generation: 0,
                result: Err("Persistence queue poisoned".into()),
            }];
        };
        queue.events.iter_mut().filter_map(Option::take).collect()
    }

    pub fn flush(&self) -> Result<(), String> {
        self.flush_timeout(Duration::from_secs(5))
    }

    /// Wait for the generations accepted before this call. Newer accepted work
    /// can satisfy the same boundary because it contains a complete replacement.
    pub fn flush_timeout(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut queue = self
            .shared
            .queue
            .lock()
            .map_err(|_| "Persistence queue poisoned".to_string())?;
        let targets = queue.latest;
        queue.flushing = true;
        self.shared.ready.notify_all();
        loop {
            let complete = targets.iter().enumerate().all(|(index, target)| {
                target.is_none_or(|target| {
                    queue.completed[index]
                        .as_ref()
                        .is_some_and(|event| event.generation >= target)
                })
            });
            if complete {
                queue.flushing = false;
                for (index, target) in targets.iter().enumerate() {
                    if target.is_some()
                        && let Some(event) = &queue.completed[index]
                    {
                        event.result.clone()?;
                    }
                }
                return Ok(());
            }
            if queue.exited {
                queue.flushing = false;
                return Err("Persistence worker exited before saving the latest snapshots".into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                queue.flushing = false;
                return Err("Timed out while flushing persisted snapshots".into());
            }
            let (next, _) = self
                .shared
                .ready
                .wait_timeout(queue, remaining)
                .map_err(|_| "Persistence queue poisoned".to_string())?;
            queue = next;
        }
    }

    pub fn shutdown(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let result = self.flush_timeout(timeout);
        {
            let mut queue = self
                .shared
                .queue
                .lock()
                .map_err(|_| "Persistence queue poisoned".to_string())?;
            queue.stopping = true;
            self.shared.ready.notify_all();
            while !queue.exited {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err("Timed out waiting for persistence shutdown".into());
                }
                let (next, _) = self
                    .shared
                    .ready
                    .wait_timeout(queue, remaining)
                    .map_err(|_| "Persistence queue poisoned".to_string())?;
                queue = next;
            }
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "Persistence worker panicked".to_string())?;
        }
        result
    }
}

impl Drop for PersistenceWriter {
    fn drop(&mut self) {
        if let Ok(mut queue) = self.shared.queue.lock() {
            queue.stopping = true;
            self.shared.ready.notify_all();
        }
        // Explicit shutdown owns the blocking boundary; a frame-time owner drop
        // never waits for slow storage. The worker still drains pending snapshots.
    }
}

fn worker_loop(
    shared: &Shared,
    wake: &Arc<dyn Fn() + Send + Sync>,
    operation: &Arc<WriteOperation>,
) {
    loop {
        let job = {
            let Ok(mut queue) = shared.queue.lock() else {
                return;
            };
            while queue.pending.iter().all(Option::is_none) && !queue.stopping {
                let Ok(next) = shared.ready.wait(queue) else {
                    return;
                };
                queue = next;
            }
            // A short quiet period prevents sliders/dividers from fsyncing each
            // frame. Intentional flush and shutdown bypass this debounce.
            while !queue.stopping && !queue.flushing {
                let remaining = queue
                    .last_submit
                    .map(|last| Duration::from_millis(75).saturating_sub(last.elapsed()))
                    .unwrap_or_default();
                if remaining.is_zero() {
                    break;
                }
                let Ok((next, _)) = shared.ready.wait_timeout(queue, remaining) else {
                    return;
                };
                queue = next;
            }
            let job = (0..queue.pending.len())
                .map(|offset| (queue.next_kind + offset) % queue.pending.len())
                .find(|&index| queue.pending[index].is_some())
                .and_then(|index| queue.pending[index].take());
            if let Some(job) = job {
                queue.next_kind = (job.kind.index() + 1) % queue.pending.len();
                job
            } else {
                queue.exited = true;
                shared.ready.notify_all();
                return;
            }
        };
        let event = SaveEvent {
            kind: job.kind,
            generation: job.generation,
            result: operation(&job),
        };
        if let Ok(mut queue) = shared.queue.lock() {
            let index = event.kind.index();
            queue.completed[index] = Some(event.clone());
            queue.events[index] = Some(event);
            shared.ready.notify_all();
        } else {
            return;
        }
        wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neptune_model::{Command, Controller, Model};
    use std::sync::{
        Barrier,
        atomic::{AtomicBool, Ordering},
    };

    fn snapshot(name: &str) -> StateSnapshot {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: PathBuf::from("/fake"),
                name: name.into(),
                remote: None,
            })
            .unwrap();
        StateSnapshot::from_model(controller.model())
    }

    fn drain_file_writes(mut writer: PersistenceWriter) {
        // File-content tests must not assert a wall-clock bound on fsync. Drop
        // requests draining; join keeps the temporary destinations alive until
        // completion. Test shutdown deadlines separately with injected storage.
        let shared = Arc::clone(&writer.shared);
        let worker = writer.worker.take().unwrap();
        drop(writer);
        worker.join().unwrap();
        let queue = shared.queue.lock().unwrap();
        assert!(queue.exited);
        assert!(queue.pending.iter().all(Option::is_none));
        for (target, completed) in queue.latest.iter().zip(&queue.completed) {
            if let Some(target) = target {
                let event = completed.as_ref().expect("accepted snapshot completed");
                assert_eq!(event.generation, *target);
                // A directory-sync failure can follow a successful replacement,
                // so correct file contents alone do not prove a successful save.
                assert_eq!(event.result, Ok(()));
            }
        }
    }

    #[test]
    fn single_writer_coalesces_blocked_storage_and_rejects_old_snapshots() {
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let saved = Arc::new(Mutex::new(Vec::new()));
        let op_started = Arc::clone(&started);
        let op_release = Arc::clone(&release);
        let op_saved = Arc::clone(&saved);
        let mut writer = PersistenceWriter::with_operation(
            true,
            Arc::new(|| {}),
            Arc::new(move |job| {
                if job.generation == 1 {
                    op_started.wait();
                    op_release.wait();
                }
                op_saved.lock().unwrap().push(job.generation);
                Ok(())
            }),
        )
        .unwrap();
        writer.submit_state(1, snapshot("first")).unwrap();
        started.wait();
        for generation in 2..=100 {
            writer.submit_state(generation, snapshot("newer")).unwrap();
        }
        writer.submit_state(3, snapshot("older")).unwrap();
        assert_eq!(
            writer
                .shared
                .queue
                .lock()
                .unwrap()
                .pending
                .iter()
                .filter(|item| item.is_some())
                .count(),
            1
        );
        release.wait();
        writer.flush().unwrap();
        assert_eq!(*saved.lock().unwrap(), vec![1, 100]);
        assert_eq!(writer.drain_events()[0].generation, 100);
        writer.shutdown(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn failed_save_is_reported_against_correct_destination_and_generation() {
        let mut writer = PersistenceWriter::with_operation(
            true,
            Arc::new(|| {}),
            Arc::new(|job| Err(format!("injected {:?} failure", job.kind))),
        )
        .unwrap();
        writer.submit_config(8, Config::default()).unwrap();
        assert!(writer.flush().unwrap_err().contains("Config"));
        let events = writer.drain_events();
        assert_eq!(events[0].kind, SaveKind::Config);
        assert_eq!(events[0].generation, 8);
        assert!(writer.shutdown(Duration::from_secs(2)).is_err());
    }

    #[test]
    fn destination_generations_are_independent_and_drain_latest_files() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("workspaces.json");
        let config_path = directory.path().join("config.toml");
        let window_path = directory.path().join("window.json");
        let writer = PersistenceWriter::new(
            state_path.clone(),
            config_path.clone(),
            window_path.clone(),
            true,
            Arc::new(|| {}),
        )
        .unwrap();
        writer.submit_state(90, snapshot("latest state")).unwrap();
        writer
            .submit_config(
                1,
                Config {
                    font_size: 20.0,
                    ..Config::default()
                },
            )
            .unwrap();
        let window = WindowState {
            inner_size: [900.0, 640.0],
            maximized: true,
            ..WindowState::default()
        };
        writer.submit_window(2, window).unwrap();
        writer.submit_window(1, WindowState::default()).unwrap();
        drain_file_writes(writer);
        let saved: StateSnapshot =
            serde_json::from_slice(&std::fs::read(state_path).unwrap()).unwrap();
        assert_eq!(saved.workspaces[0].name, "latest state");
        assert_eq!(Config::load(&config_path).unwrap().font_size, 20.0);
        assert_eq!(
            crate::persistence::window_state::load(&window_path).state,
            window
        );
    }

    #[test]
    fn flush_timeout_does_not_hold_queue_or_block_subsequent_submissions() {
        let release = Arc::new(AtomicBool::new(false));
        let worker_release = Arc::clone(&release);
        let mut writer = PersistenceWriter::with_operation(
            true,
            Arc::new(|| {}),
            Arc::new(move |_| {
                while !worker_release.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(())
            }),
        )
        .unwrap();
        writer.submit_state(1, snapshot("first")).unwrap();
        assert!(writer.flush_timeout(Duration::from_millis(10)).is_err());
        writer.submit_state(2, snapshot("second")).unwrap();
        release.store(true, Ordering::Release);
        writer.shutdown(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn shutdown_timeout_keeps_draining_pending_snapshots() {
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let op_started = Arc::clone(&started);
        let op_release = Arc::clone(&release);
        let mut writer = PersistenceWriter::with_operation(
            true,
            Arc::new(|| {}),
            Arc::new(move |job| {
                if job.kind == SaveKind::State {
                    op_started.wait();
                    op_release.wait();
                }
                Ok(())
            }),
        )
        .unwrap();
        writer.submit_state(90, snapshot("blocked state")).unwrap();
        started.wait();
        writer.submit_config(1, Config::default()).unwrap();
        writer.submit_window(2, WindowState::default()).unwrap();
        writer.submit_window(1, WindowState::default()).unwrap();

        // Hold storage beyond the entire shutdown budget without timing a sleep.
        // The shared deadline reports this as a shutdown timeout even though the
        // worker is still writing, not stuck in its exit handshake.
        assert_eq!(
            writer.shutdown(Duration::ZERO).unwrap_err(),
            "Timed out waiting for persistence shutdown"
        );
        {
            let queue = writer.shared.queue.lock().unwrap();
            assert!(queue.stopping);
            assert!(!queue.exited);
            assert!(queue.completed.iter().all(Option::is_none));
        }
        assert_eq!(
            writer.submit_state(91, snapshot("too late")).unwrap_err(),
            "Persistence writer has shut down"
        );
        release.wait();
        writer.shutdown(Duration::from_secs(2)).unwrap();
        assert!(writer.worker.is_none());
        assert_eq!(
            writer.drain_events(),
            [
                (SaveKind::State, 90),
                (SaveKind::Config, 1),
                (SaveKind::Window, 2),
            ]
            .map(|(kind, generation)| SaveEvent {
                kind,
                generation,
                result: Ok(()),
            })
        );
    }

    #[test]
    fn ephemeral_writer_never_touches_destinations() {
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join("state.json");
        let config = directory.path().join("config.toml");
        let window = directory.path().join("window.json");
        let mut writer = PersistenceWriter::new(
            state.clone(),
            config.clone(),
            window.clone(),
            false,
            Arc::new(|| {}),
        )
        .unwrap();
        writer.submit_state(1, snapshot("ephemeral")).unwrap();
        writer.submit_config(1, Config::default()).unwrap();
        writer.submit_window(1, WindowState::default()).unwrap();
        writer.shutdown(Duration::from_secs(2)).unwrap();
        assert!(!state.exists());
        assert!(!config.exists());
        assert!(!window.exists());
    }

    #[test]
    fn unsupported_workspace_format_does_not_disable_preferences_saving() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("workspaces.json");
        let config_path = directory.path().join("config.toml");
        let original = br#"{"version":999,"opaque":"future state"}"#;
        std::fs::write(&state_path, original).unwrap();
        let report = crate::persistence::workspace_state::load_state(
            &state_path,
            neptune_model::Limits::default(),
        );
        let writer = PersistenceWriter::new(
            state_path.clone(),
            config_path.clone(),
            directory.path().join("window.json"),
            true,
            Arc::new(|| {}),
        )
        .unwrap();
        if report.can_write {
            writer.submit_state(1, snapshot("fallback")).unwrap();
        }
        writer
            .submit_config(
                1,
                Config {
                    font_size: 22.0,
                    ..Config::default()
                },
            )
            .unwrap();
        drain_file_writes(writer);
        assert_eq!(std::fs::read(state_path).unwrap(), original);
        assert_eq!(Config::load(&config_path).unwrap().font_size, 22.0);
    }
}
