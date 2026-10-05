//! Carries out what agents ask of the application for the agents they start:
//! opening a terminal for one, typing a message for it, and closing it.
use super::App;
use crate::runtime::agents::{AgentRequest, Press, Task};
use eframe::egui;
use neptune_model::{Command, Lifecycle, PaneId};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// A pasted prompt is submitted once the CLI has taken the paste.
const SUBMIT_AFTER: Duration = Duration::from_millis(150);

/// The task of the agent whose terminal is being opened, and the terminal
/// once the model has made it.
pub(super) struct Spawning {
    /// Until the terminal's session takes it.
    pub task: Option<Task>,
    pub pane: Option<PaneId>,
}
/// A CLI that has drawn nothing new for this long without taking its task
/// is asking something first.
const STILL: Duration = Duration::from_secs(4);
/// How often a starting agent's terminal is looked at.
const LOOK: Duration = Duration::from_millis(500);
/// The lines of a question that are passed on.
const SCREEN_LINES: usize = 24;

/// A started agent's terminal while its CLI has not taken its task.
struct Watch {
    generation: u64,
    revision: u64,
    /// Since when it has shown the same.
    since: Instant,
    /// The agent that started it was told what it shows.
    told: bool,
}
/// How long a message waits for its agent's prompt to take a paste.
const DELIVER_WITHIN: Duration = Duration::from_secs(5);

/// A message for a started agent: pasted into its prompt once the CLI takes
/// a paste whole, then submitted.
struct Delivery {
    pane: PaneId,
    generation: u64,
    /// Until it has been pasted.
    text: Option<String>,
    /// When to submit what was pasted, or to give the paste up.
    due: Instant,
}
#[derive(Default)]
pub(super) struct Delegation {
    pub spawning: Option<Spawning>,
    deliveries: Vec<Delivery>,
    starting: BTreeMap<PaneId, Watch>,
}
impl Delegation {
    /// `Some` while the started agent in `pane` has not taken its task:
    /// whether it stands at a question of its own.
    pub fn starting(&self, pane: PaneId) -> Option<bool> {
        self.starting.get(&pane).map(|watch| watch.told)
    }
}

impl App {
    /// Runs once a frame, after agents' own changes have reached the model.
    pub(super) fn serve_agents(&mut self, ctx: &egui::Context) {
        for request in self.sessions.agents().drain_requests() {
            match request {
                AgentRequest::Spawn {
                    request,
                    parent,
                    generation,
                    task,
                    cwd,
                } => {
                    let result = self.spawn_agent(ctx, parent, generation, task, cwd);
                    self.sessions.agents().spawn_result(request, result);
                }
                AgentRequest::Press {
                    pane,
                    generation,
                    keys,
                } => {
                    if let Some(session) = self
                        .sessions
                        .get(pane)
                        .filter(|_| self.sessions.generation(pane) == Some(generation))
                    {
                        use terminal_core::input::{Key, encode_key, encode_text};
                        let modes = session.modes();
                        for key in keys {
                            let named = |key| encode_key(key, Default::default(), modes);
                            let bytes = match key {
                                Press::Enter => named(Key::Enter),
                                Press::Escape => named(Key::Escape),
                                Press::Tab => named(Key::Tab),
                                Press::Up => named(Key::ArrowUp),
                                Press::Down => named(Key::ArrowDown),
                                Press::Left => named(Key::ArrowLeft),
                                Press::Right => named(Key::ArrowRight),
                                Press::Char(letter) => Some(encode_text(
                                    letter.encode_utf8(&mut [0; 4]),
                                    Default::default(),
                                    modes,
                                )),
                            };
                            if let Some(bytes) = bytes {
                                let _ = session.write(&bytes);
                            }
                        }
                    }
                    // What it shows next is looked at afresh.
                    self.delegation.starting.remove(&pane);
                }
                AgentRequest::Tell {
                    pane,
                    generation,
                    text,
                } => self.delegation.deliveries.push(Delivery {
                    pane,
                    generation,
                    text: Some(text),
                    due: Instant::now() + DELIVER_WITHIN,
                }),
                AgentRequest::Close {
                    pane,
                    generation,
                    parent,
                } => {
                    // The agent that started it asked; no person is closing
                    // work of their own, so nothing is confirmed.
                    if self.controller.model().pane(pane).is_some_and(|item| {
                        item.generation() == generation && item.spawned_by() == Some(parent)
                    }) {
                        self.dispatch(ctx, Command::ClosePane(pane));
                    }
                }
            }
        }
        self.deliver(ctx);
        self.watch_starting(ctx);
        let model = self.controller.model();
        // What is shown for a started agent follows its title where its hooks
        // are silent; the agent that waits for it is told the same.
        let links: Vec<(PaneId, PaneId)> = model
            .workspaces()
            .iter()
            .flat_map(|workspace| workspace.panes())
            .filter_map(|pane| {
                let parent = pane.spawned_by()?;
                if let Some((activity, _)) = self.agents.get(pane.id()) {
                    self.sessions
                        .agents()
                        .observe(pane.id(), pane.generation(), activity);
                }
                Some((pane.id(), parent))
            })
            .collect();
        self.sessions.agents().sync_spawned(&links);
    }

    /// Notices a started agent whose CLI stands still before taking its
    /// task: it is asking something, such as whether to trust a folder. What
    /// its terminal shows goes to the agent that started it, which tells the
    /// user; nothing else of a terminal's contents leaves it.
    fn watch_starting(&mut self, ctx: &egui::Context) {
        let starting = self.sessions.agents().starting();
        self.delegation
            .starting
            .retain(|pane, watch| starting.contains(&(*pane, watch.generation)));
        if starting.is_empty() {
            return;
        }
        let shown = self.shown();
        let now = Instant::now();
        for (pane, generation) in starting {
            let Some(session) = self
                .sessions
                .get(pane)
                .filter(|_| self.sessions.generation(pane) == Some(generation))
            else {
                continue;
            };
            let revision = session.revision();
            let watch = self.delegation.starting.entry(pane).or_insert(Watch {
                generation,
                revision,
                since: now,
                told: false,
            });
            if watch.revision != revision {
                watch.revision = revision;
                watch.since = now;
                if std::mem::take(&mut watch.told) {
                    self.sessions.agents().asking(pane, generation, None);
                }
            } else if !watch.told && now.saturating_duration_since(watch.since) >= STILL {
                // A terminal in view is the renderer's to read; the user
                // is looking at what it asks.
                let screen = if shown.contains(&pane) {
                    Some(String::new())
                } else {
                    let text = session.screen_text();
                    let lines: Vec<&str> = text
                        .lines()
                        .map(str::trim_end)
                        .filter(|line| !line.trim().is_empty())
                        .collect();
                    let from = lines.len().saturating_sub(SCREEN_LINES);
                    // A shell that has drawn nothing is still starting.
                    (!lines.is_empty()).then(|| lines[from..].join("\n"))
                };
                if let Some(screen) = screen {
                    watch.told = true;
                    self.sessions
                        .agents()
                        .asking(pane, generation, Some(screen));
                }
            }
        }
        ctx.request_repaint_after(LOOK);
    }

    /// Types what is waiting for started agents. Only an agent still open
    /// and not asking a person anything takes it, and only as a bracketed
    /// paste: the same words would be run by a shell, or answer a request.
    fn deliver(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let mut deliveries = std::mem::take(&mut self.delegation.deliveries);
        // One message at a time reaches a prompt, in the order they came.
        let mut busy = Vec::new();
        deliveries.retain_mut(|delivery| {
            let session = self
                .sessions
                .get(delivery.pane)
                .filter(|_| self.sessions.generation(delivery.pane) == Some(delivery.generation));
            let Some(session) = session else {
                return false;
            };
            if busy.contains(&delivery.pane) {
                return true;
            }
            busy.push(delivery.pane);
            let ready = self
                .sessions
                .agents()
                .takes_prompt(delivery.pane, delivery.generation);
            match &delivery.text {
                Some(text) => {
                    if ready
                        && session
                            .modes()
                            .contains(terminal_core::Mode::BRACKETED_PASTE)
                        && session.paste(text).is_ok()
                    {
                        delivery.text = None;
                        delivery.due = now + SUBMIT_AFTER;
                        ctx.request_repaint_after(SUBMIT_AFTER);
                        return true;
                    }
                    // Its CLI may still be drawing its prompt.
                    ctx.request_repaint_after(Duration::from_millis(100));
                    now < delivery.due
                }
                None if now < delivery.due => {
                    ctx.request_repaint_after(delivery.due - now);
                    true
                }
                None => {
                    if ready
                        && let Some(enter) = terminal_core::input::encode_key(
                            terminal_core::input::Key::Enter,
                            Default::default(),
                            session.modes(),
                        )
                    {
                        let _ = session.write(&enter);
                    }
                    false
                }
            }
        });
        self.delegation.deliveries = deliveries;
    }

    fn spawn_agent(
        &mut self,
        ctx: &egui::Context,
        parent: PaneId,
        generation: u64,
        task: Task,
        cwd: std::path::PathBuf,
    ) -> Result<PaneId, String> {
        self.delegation.spawning = Some(Spawning {
            task: Some(task),
            pane: None,
        });
        let effects = self.controller.dispatch(Command::SpawnAgent {
            parent,
            generation,
            cwd,
        });
        let effects = match effects {
            Ok(effects) => effects,
            Err(error) => {
                self.delegation.spawning = None;
                return Err(match error {
                    neptune_model::Error::UnknownPane(_) => {
                        "Neptune is no longer tracking an agent in this terminal.".into()
                    }
                    error => format!("{error}."),
                });
            }
        };
        // Starting the session takes the task for the pane the model made.
        self.execute(ctx, effects);
        let pane = self
            .delegation
            .spawning
            .take()
            .and_then(|spawning| spawning.pane)
            .ok_or("Neptune could not open a terminal for the agent.")?;
        match self
            .controller
            .model()
            .pane(pane)
            .map(|pane| pane.lifecycle())
        {
            Some(Lifecycle::Failed(error)) => {
                let error = format!("Neptune could not open a terminal for the agent: {error}.");
                self.dispatch(ctx, Command::ClosePane(pane));
                self.sessions.agents().forget_spawn(pane);
                Err(error)
            }
            Some(_) => Ok(pane),
            None => {
                self.sessions.agents().forget_spawn(pane);
                Err("Neptune could not open a terminal for the agent.".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neptune_model::{AgentKind, AgentSession};

    #[test]
    fn requests_open_a_terminal_out_of_view_for_a_started_agent_and_close_only_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _sender) = super::super::tests::fixture(dir.path());
        let ctx = egui::Context::default();
        app.startup = None;
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: dir.path().into(),
                name: "One".into(),
                remote: None,
            })
            .unwrap();
        let parent = app.controller.model().active_pane().unwrap();
        let generation = app.controller.model().pane(parent).unwrap().generation();
        app.controller
            .dispatch(Command::PaneAgentChanged {
                pane: parent,
                generation,
                agent: Some(AgentSession {
                    kind: AgentKind::Claude,
                    session_id: None,
                    cwd: dir.path().into(),
                }),
            })
            .unwrap();
        let spawn = |request, generation| AgentRequest::Spawn {
            request,
            parent,
            generation,
            task: Task::new(AgentKind::Codex, "PRIVATE TASK"),
            cwd: dir.path().into(),
        };
        // A request from a terminal replaced since it asked opens nothing.
        app.sessions.agents().request(spawn(1, generation + 1));
        app.serve_agents(&ctx);
        assert_eq!(app.controller.model().pane_count(), 1);
        assert!(matches!(app.sessions.agents().spawned(1), Some(Err(_))));

        app.sessions.agents().request(spawn(2, generation));
        app.serve_agents(&ctx);
        let Some(Ok(child)) = app.sessions.agents().spawned(2) else {
            panic!("no terminal was opened");
        };
        let model = app.controller.model();
        assert_eq!(model.pane(child).unwrap().spawned_by(), Some(parent));
        // The keyboard and the view stay with the agent that asked.
        assert_eq!(model.active_pane(), Some(parent));
        let listed = app.spawned_agents(parent);
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].pane, listed[0].state()), (child, "Starting"));
        assert!(app.delegation.spawning.is_none());
        // Its terminal has no tab and is not counted until it is opened.
        let workspace = &app.controller.model().workspaces()[0];
        assert!(workspace.is_background(child));
        assert_eq!(app.shown(), [parent]);
        assert_eq!(app.views()[0].panes, 1);
        // The task is held for the shell, not shown or saved.
        assert!(app.ui.error.is_none());
        let saved = serde_json::to_string(
            &crate::persistence::workspace_state::StateSnapshot::from_model(app.controller.model()),
        )
        .unwrap();
        assert!(!saved.contains("PRIVATE") && !saved.contains("spawned_by"));

        // Choosing it in its starter's list gives it a tab and the focus.
        app.action(&ctx, crate::ui::Action::Focus(child));
        let workspace = &app.controller.model().workspaces()[0];
        assert_eq!(workspace.layout().panes(), [parent, child]);
        assert_eq!(app.shown(), [child]);
        assert_eq!(app.spawned_agents(parent).len(), 1);

        // Only the agent that started it closes it, and only as it was.
        for (generation, parent) in [(1, PaneId::new(99)), (2, parent)] {
            app.sessions.agents().request(AgentRequest::Close {
                pane: child,
                generation,
                parent,
            });
            app.serve_agents(&ctx);
            assert!(app.controller.model().pane(child).is_some());
        }
        app.sessions.agents().request(AgentRequest::Close {
            pane: child,
            generation: 1,
            parent,
        });
        app.serve_agents(&ctx);
        assert!(app.controller.model().pane(child).is_none());
        assert!(app.spawned_agents(parent).is_empty());
    }
}
