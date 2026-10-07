//! Carries out what agents and projects ask of the application for the agents
//! they start: opening a terminal for one, typing a message for it, and
//! closing it.
use super::{
    App,
    projects::{Briefing, Done},
};
use crate::runtime::agents::{AgentRequest, Press, ProjectCall, Starter, Task};
use eframe::egui;
use neptune_model::{Command, Lifecycle, PaneId, Project, ProjectId, Worktree};
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
                    // A new agent of a project is briefed from what the
                    // project keeps, read from its folder first. It starts
                    // once that is read, on a later frame; where it cannot
                    // wait, it is briefed from what was read last.
                    let held = match parent {
                        Starter::Project(project) if task.resume.is_none() => {
                            self.projects.brief_after_reading(Box::new(Briefing {
                                request,
                                project,
                                generation,
                                task,
                                cwd,
                            }))
                        }
                        Starter::Project(project) => Err(Box::new(Briefing {
                            request,
                            project,
                            generation,
                            task,
                            cwd,
                        })),
                        Starter::Pane(_) => {
                            self.serve_spawn(ctx, request, parent, generation, task, cwd);
                            continue;
                        }
                    };
                    if let Err(now) = held {
                        let now = *now;
                        self.serve_spawn(
                            ctx,
                            now.request,
                            parent,
                            now.generation,
                            now.task,
                            now.cwd,
                        );
                    }
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
                } => {
                    let project = self
                        .controller
                        .model()
                        .pane(pane)
                        .filter(|item| item.generation() == generation)
                        .and_then(|item| item.project());
                    // What the person had it told has its own line in the chat.
                    let own = self.projects.asked_by_person(pane);
                    let text = match project {
                        // A project's agent hears with it what the project
                        // decided since it was last told.
                        Some(project) => {
                            if !own {
                                self.projects.record(project, Done::Told(pane));
                            }
                            self.projects.told(project, pane, text)
                        }
                        None => text,
                    };
                    self.delegation.deliveries.push(Delivery {
                        pane,
                        generation,
                        text: Some(text),
                        due: Instant::now() + DELIVER_WITHIN,
                    })
                }
                AgentRequest::Close {
                    pane,
                    generation,
                    parent,
                } => {
                    // The agent or the project that started it asked; no
                    // person is closing work of their own, so nothing is
                    // confirmed.
                    let model = self.controller.model();
                    if model.pane(pane).is_some_and(|item| {
                        item.generation() == generation
                            && match parent {
                                Starter::Pane(parent) => item.spawned_by() == Some(parent),
                                Starter::Project(project) => item.project() == Some(project),
                            }
                    }) {
                        // A terminal the person gave a tab is theirs: a lead
                        // that is done with its agent does not close what
                        // they may be reading or typing in. It leaves the
                        // project and goes on as an ordinary terminal.
                        let revealed = model
                            .workspace_for_pane(pane)
                            .and_then(|id| model.workspace(id))
                            .is_some_and(|workspace| !workspace.is_background(pane));
                        match parent {
                            Starter::Project(project) if revealed => {
                                self.dispatch(
                                    ctx,
                                    Command::ReleaseProjectAgent { pane, generation },
                                );
                                // Nothing of it is the lead's any more, also
                                // not to open again beside the one that runs.
                                self.sessions.agents().forget_spawn(pane);
                                self.projects.record(project, Done::Released(pane));
                            }
                            Starter::Project(project) => {
                                self.dispatch(ctx, Command::ClosePane(pane));
                                self.projects.record(project, Done::Closed(pane));
                            }
                            Starter::Pane(_) => self.dispatch(ctx, Command::ClosePane(pane)),
                        }
                    }
                }
                AgentRequest::Project {
                    request,
                    project,
                    from,
                    call,
                } => {
                    let result = match call {
                        // The bridge has the whole reply while it knows the
                        // agent; the project's folder keeps the reply of
                        // one it no longer knows, such as after a restart.
                        ProjectCall::Report { agent } if from == Starter::Project(project) => {
                            let agents = self.sessions.agents();
                            agents.last_report(project, agent).or_else(|unknown| {
                                self.projects.report(project, agent).ok_or(unknown)
                            })
                        }
                        ProjectCall::Report { .. } => {
                            Err("Only a project's lead reads its agents' replies.".into())
                        }
                        // Its watches are the application's own to answer.
                        call @ (ProjectCall::AddSubscription { .. }
                        | ProjectCall::ListSubscriptions
                        | ProjectCall::RemoveSubscription { .. }
                        | ProjectCall::PullRequestStatus { .. }) => {
                            self.watch_call(project, from, call)
                        }
                        // What a project keeps is in its folder: the store's
                        // worker answers, on a later frame.
                        call => {
                            self.context_call(request, project, from, call);
                            continue;
                        }
                    };
                    self.sessions.agents().call_result(request, result);
                }
            }
        }
        self.deliver(ctx);
        self.watch_starting(ctx);
        let model = self.controller.model();
        // What is shown for a started agent follows its title where its hooks
        // are silent; the agent that waits for it is told the same.
        let links: Vec<(PaneId, Starter)> = model
            .workspaces()
            .iter()
            .flat_map(|workspace| workspace.panes())
            .filter_map(|pane| {
                let parent = pane
                    .spawned_by()
                    .map(Starter::Pane)
                    .or(pane.project().map(Starter::Project))?;
                if let Some((activity, _)) = self.agents.get(pane.id()) {
                    self.sessions
                        .agents()
                        .observe(pane.id(), pane.generation(), activity);
                }
                Some((pane.id(), parent))
            })
            .collect();
        let projects: Vec<(ProjectId, bool)> = model
            .projects()
            .iter()
            .map(|project| (project.id(), project.paused()))
            .collect();
        self.sessions.agents().sync_projects(&projects);
        self.sessions.agents().sync_spawned(&links);
    }

    /// Starts the agent a request asked for and tells who asked what became
    /// of it. A new agent of a project that was given a branch works in that
    /// branch's worktree, which git makes first, off the frame: the agent
    /// then starts when git has answered.
    pub(super) fn serve_spawn(
        &mut self,
        ctx: &egui::Context,
        request: u64,
        parent: Starter,
        generation: u64,
        task: Task,
        cwd: std::path::PathBuf,
    ) {
        let project = match parent {
            Starter::Project(project) if task.resume.is_none() && task.worktree.is_some() => {
                project
            }
            _ => return self.start_agent(ctx, request, parent, generation, task, cwd, None),
        };
        // Nothing is made for an agent that would not start.
        if !self.projects.may_spawn(project) {
            return self.refuse_spawn(request, project, spawned_enough());
        }
        // Those git is still making a worktree for count: a lead that asks
        // on at its limit must not leave a branch and a checkout each time.
        let open = self.controller.model().project_agents(project).count();
        if open + self.worktrees.asked_for(project) >= Project::MAX_AGENTS {
            return self.refuse_spawn(
                request,
                project,
                format!("{}.", neptune_model::Error::ProjectAgentLimit),
            );
        }
        if let Err((_, reason)) = self.project_worktree(
            ctx,
            Box::new(Briefing {
                request,
                project,
                generation,
                task,
                cwd,
            }),
        ) {
            self.refuse_spawn(request, project, reason);
        }
    }

    /// Git has made the worktree a project's new agent was asked for in, or
    /// says why it could not.
    pub(super) fn worktree_made(
        &mut self,
        ctx: &egui::Context,
        briefing: Briefing,
        result: Result<Worktree, String>,
    ) {
        let Briefing {
            request,
            project,
            generation,
            task,
            ..
        } = briefing;
        match result {
            Ok(worktree) => self.start_agent(
                ctx,
                request,
                Starter::Project(project),
                generation,
                task,
                worktree.path.clone(),
                Some(worktree),
            ),
            Err(error) => {
                let branch = task.worktree.unwrap_or_default();
                let reason = if error.contains("not in a git repository") {
                    "The project's directory is not in a git repository, so Neptune cannot \
                     make a worktree there. Start the agent without worktree."
                        .to_owned()
                } else if error.contains("not take this as a branch name") {
                    format!("Git does not take {branch} as a branch name. Choose another.")
                } else {
                    format!("Neptune could not make the worktree of {branch}: {error}.")
                };
                self.refuse_spawn(request, project, reason);
            }
        }
    }

    /// Tells a project's lead why the agent it asked for was not started,
    /// and its chat the same.
    fn refuse_spawn(&mut self, request: u64, project: ProjectId, reason: String) {
        self.projects.record(
            project,
            Done::NotStarted {
                again: false,
                reason: reason.clone(),
            },
        );
        self.sessions.agents().spawn_result(request, Err(reason));
    }

    /// Opens the terminal of an agent and hands it its task. A project's
    /// agent is briefed here, from what its project keeps; `worktree` is the
    /// one git made for it.
    #[allow(clippy::too_many_arguments)]
    fn start_agent(
        &mut self,
        ctx: &egui::Context,
        request: u64,
        parent: Starter,
        generation: u64,
        mut task: Task,
        cwd: std::path::PathBuf,
        worktree: Option<Worktree>,
    ) {
        let resumed = task.resume.as_ref().map(|resume| resume.0);
        // One opened again goes on in the worktree it had, where its folder
        // is still that worktree's.
        // What the person set for the project's agents wins over what its
        // lead asked for. One opened again keeps what it had.
        if let (Starter::Project(project), None) = (parent, resumed) {
            self.projects.agent_settings(project, &mut task);
        }
        let worktree = worktree.or_else(|| match (parent, resumed) {
            (Starter::Project(project), Some(before)) => {
                self.projects.worktree_of(project, before, &cwd)
            }
            _ => None,
        });
        if let Starter::Project(project) = parent
            && let Some(item) = self.controller.model().project(project)
        {
            let folder = self.projects.context_folder(item.key());
            task.prompt = self
                .projects
                .brief(project, &folder, &task, worktree.as_ref());
        }
        let (kind, title) = (task.kind, task.title.clone());
        let with = (task.model.clone(), task.effort.clone(), task.ultracode);
        let again = resumed.is_some();
        // A project starts only so many agents in a day.
        let within = match parent {
            Starter::Project(project) => again || self.projects.may_spawn(project),
            Starter::Pane(_) => true,
        };
        let result = if within {
            self.spawn_agent(ctx, parent, generation, task, cwd, worktree)
        } else {
            Err(spawned_enough())
        };
        // A project's chat says what was done, not what its
        // lead says it asked for.
        if let Starter::Project(project) = parent {
            if let Ok(agent) = &result {
                self.projects.briefed(project, *agent);
            }
            self.projects.record(
                project,
                match &result {
                    Ok(agent) => {
                        let pane = self.controller.model().pane(*agent);
                        Done::Started {
                            agent: *agent,
                            kind,
                            title,
                            again,
                            cwd: pane
                                .map(|pane| pane.cwd().to_path_buf())
                                .unwrap_or_default(),
                            branch: pane
                                .and_then(|pane| pane.worktree())
                                .map(|worktree| worktree.branch.clone()),
                            resumed,
                            with,
                        }
                    }
                    Err(reason) => Done::NotStarted {
                        again,
                        reason: reason.clone(),
                    },
                },
            );
        }
        self.sessions.agents().spawn_result(request, result);
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
                    if now >= delivery.due {
                        // Given up. A project hears that its message never
                        // arrived; nothing else would tell it.
                        self.sessions
                            .agents()
                            .undelivered(delivery.pane, delivery.generation);
                    }
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
                    } else {
                        // Pasted and never submitted: its agent turned to a
                        // person meanwhile. A project hears of that as well.
                        self.sessions
                            .agents()
                            .undelivered(delivery.pane, delivery.generation);
                    }
                    false
                }
            }
        });
        self.delegation.deliveries = deliveries;
    }

    /// Where an agent of `project` works: the project's own directory when
    /// `cwd` is empty, else `cwd` if it is that directory, lies inside it or
    /// inside a worktree one of the workspace's terminals was opened for.
    /// Compared as written; nothing is read from disk on a frame.
    fn project_directory(
        &self,
        project: ProjectId,
        cwd: std::path::PathBuf,
    ) -> Result<std::path::PathBuf, String> {
        let model = self.controller.model();
        let workspace = model
            .project(project)
            .and_then(|project| model.workspace(project.workspace()))
            .ok_or("This project no longer exists.")?;
        let home = self
            .project_home(project)
            .ok_or("This project no longer exists.")?;
        if cwd.as_os_str().is_empty() {
            return Ok(home);
        }
        let plain = !cwd
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir));
        let inside = cwd.starts_with(&home)
            || workspace.panes().iter().any(|pane| {
                pane.worktree()
                    .is_some_and(|worktree| cwd.starts_with(&worktree.path))
            });
        if plain && inside {
            Ok(cwd)
        } else {
            Err(format!(
                "cwd is outside the project: its agents work in {} or a directory inside it.",
                home.display()
            ))
        }
    }

    fn spawn_agent(
        &mut self,
        ctx: &egui::Context,
        parent: Starter,
        generation: u64,
        task: Task,
        cwd: std::path::PathBuf,
        worktree: Option<Worktree>,
    ) -> Result<PaneId, String> {
        let command = match parent {
            Starter::Pane(parent) => Command::SpawnAgent {
                parent,
                generation,
                cwd,
            },
            // One opened again goes on where it worked, which was the
            // project's when it started.
            Starter::Project(project) if task.resume.is_some() => Command::SpawnProjectAgent {
                project,
                cwd,
                worktree,
            },
            // The worktree git just made for it is where it works.
            Starter::Project(project) => Command::SpawnProjectAgent {
                project,
                cwd: match &worktree {
                    Some(worktree) => worktree.path.clone(),
                    None => self.project_directory(project, cwd)?,
                },
                worktree,
            },
        };
        self.delegation.spawning = Some(Spawning {
            task: Some(task),
            pane: None,
        });
        let effects = self.controller.dispatch(command);
        let effects = match effects {
            Ok(effects) => effects,
            Err(error) => {
                self.delegation.spawning = None;
                return Err(match error {
                    neptune_model::Error::UnknownPane(_) => {
                        "Neptune is no longer tracking an agent in this terminal.".into()
                    }
                    neptune_model::Error::UnknownProject(_) => {
                        "This project no longer exists.".into()
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

/// Said to a lead whose project has started all the agents a day allows.
fn spawned_enough() -> String {
    format!(
        "This project has started {} agents today, the most Neptune starts for one project in \
         a day. Tell the user.",
        crate::projects::registry::MAX_SPAWNS_A_DAY
    )
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
            parent: parent.into(),
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
        // Its tab goes back out of view and the agent runs on, listed as
        // before. Nothing is closed, so nothing asks. The starter's own
        // terminal, which no agent lists, has no such way.
        let listed = |app: &App| app.spawned_agents(parent)[0].can_background;
        assert!(listed(&app) && app.controller.model().can_background(parent).is_err());
        app.action(&ctx, crate::ui::Action::Background(child));
        assert!(app.ui.overlay == crate::ui::OverlayState::None && app.ui.error.is_none());
        assert!(app.pending_close.is_none());
        let model = app.controller.model();
        assert!(model.workspaces()[0].is_background(child));
        assert_eq!(model.pane(child).unwrap().generation(), 1);
        assert_eq!(
            (app.shown(), model.active_pane()),
            (vec![parent], Some(parent))
        );
        assert_eq!(app.views()[0].panes, 1);
        assert!(!listed(&app));
        // The last terminal in view stays: the refusal is said, not hidden.
        app.action(&ctx, crate::ui::Action::Background(parent));
        assert!(app.ui.error.take().is_some());
        assert_eq!(app.shown(), [parent]);
        // Zoomed, the terminal that takes its place is the one shown.
        app.action(&ctx, crate::ui::Action::Focus(child));
        app.ui.zoomed = true;
        app.action(&ctx, crate::ui::Action::Background(child));
        assert_eq!(app.shown(), [parent]);
        app.ui.zoomed = false;

        // Only the agent that started it closes it, and only as it was:
        // out of view again, it is closed like one that was never opened.
        for (generation, parent) in [(1, PaneId::new(99)), (2, parent)] {
            app.sessions.agents().request(AgentRequest::Close {
                pane: child,
                generation,
                parent: parent.into(),
            });
            app.serve_agents(&ctx);
            assert!(app.controller.model().pane(child).is_some());
        }
        app.sessions.agents().request(AgentRequest::Close {
            pane: child,
            generation: 1,
            parent: parent.into(),
        });
        app.serve_agents(&ctx);
        assert!(app.controller.model().pane(child).is_none());
        assert!(app.spawned_agents(parent).is_empty());
    }
    #[test]
    fn a_project_starts_agents_out_of_view_in_its_own_directory_and_closes_only_its_own() {
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
        let workspace = app.controller.model().workspaces()[0].id();
        let shell = app.controller.model().active_pane().unwrap();
        app.controller
            .dispatch(Command::AddProject {
                workspace,
                name: "One".into(),
                key: neptune_model::ProjectKey::parse("0123456789abcdef").unwrap(),
                lead: AgentKind::Claude,
            })
            .unwrap();
        let project = app.controller.model().project_of(workspace).unwrap().id();
        let starter = Starter::Project(project);
        let spawn = |request, cwd: std::path::PathBuf| AgentRequest::Spawn {
            request,
            parent: starter,
            generation: 1,
            task: Task {
                title: Some("api".into()),
                ..Task::new(AgentKind::Codex, "PRIVATE TASK")
            },
            cwd,
        };
        let agents = |app: &App| app.controller.model().project_agents(project).count();

        // No directory given is the project's own.
        app.sessions.agents().request(spawn(1, Default::default()));
        app.serve_agents(&ctx);
        let Some(Ok(child)) = app.sessions.agents().spawned(1) else {
            panic!("no terminal was opened");
        };
        let model = app.controller.model();
        let pane = model.pane(child).unwrap();
        assert_eq!((pane.project(), pane.spawned_by()), (Some(project), None));
        assert_eq!(pane.cwd(), dir.path());
        assert!(model.workspaces()[0].is_background(child));
        assert_eq!(model.active_pane(), Some(shell));
        assert!(app.delegation.spawning.is_none() && app.ui.error.is_none());
        // The bridge holds its task under the project, which now collects it.
        let (listed, _) = app.sessions.agents().collect_project(project, false);
        assert_eq!(listed.len(), 1);
        assert_eq!(
            (listed[0].agent, listed[0].title.as_deref()),
            (child.get(), Some("api"))
        );
        let saved = serde_json::to_string(
            &crate::persistence::workspace_state::StateSnapshot::from_model(app.controller.model()),
        )
        .unwrap();
        assert!(!saved.contains("PRIVATE") && !saved.contains("api"));

        // A directory outside the project, or one reached by stepping out
        // of it, starts nothing.
        for (request, cwd) in [
            (2, std::env::temp_dir().join("elsewhere")),
            (3, dir.path().join("sub/../..")),
        ] {
            app.sessions.agents().request(spawn(request, cwd));
            app.serve_agents(&ctx);
            assert!(matches!(
                app.sessions.agents().spawned(request),
                Some(Err(reason)) if reason.contains("outside the project")
            ));
        }
        assert_eq!(agents(&app), 1);
        // Paused, the model refuses even what the bridge let through.
        app.controller
            .dispatch(Command::SetProjectPaused {
                project,
                paused: true,
            })
            .unwrap();
        app.sessions
            .agents()
            .request(spawn(4, dir.path().join("sub")));
        app.serve_agents(&ctx);
        assert!(matches!(
            app.sessions.agents().spawned(4),
            Some(Err(reason)) if reason.contains("paused")
        ));
        assert!(agents(&app) == 1 && app.delegation.spawning.is_none());
        app.controller
            .dispatch(Command::SetProjectPaused {
                project,
                paused: false,
            })
            .unwrap();
        app.sessions
            .agents()
            .request(spawn(5, dir.path().join("sub")));
        app.serve_agents(&ctx);
        let Some(Ok(second)) = app.sessions.agents().spawned(5) else {
            panic!("no terminal was opened inside the project");
        };
        assert_eq!(
            app.controller.model().pane(second).unwrap().cwd(),
            dir.path().join("sub")
        );

        // The lead reads an agent's whole reply through the application.
        let report = |request, from| AgentRequest::Project {
            request,
            project,
            from,
            call: ProjectCall::Report { agent: child.get() },
        };
        app.sessions.agents().request(report(6, starter));
        app.sessions
            .agents()
            .request(report(7, Starter::Pane(second)));
        app.serve_agents(&ctx);
        assert!(matches!(
            app.sessions.agents().called(6),
            Some(Err(reason)) if reason.contains("has not ended a turn")
        ));
        assert!(matches!(
            app.sessions.agents().called(7),
            Some(Err(reason)) if reason.contains("Only a project's lead")
        ));

        // Only its project closes it, and only as it was.
        for (generation, parent) in [
            (1, Starter::Project(neptune_model::ProjectId::new(99))),
            (1, Starter::Pane(shell)),
            (2, starter),
        ] {
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
            parent: starter,
        });
        app.serve_agents(&ctx);
        assert!(app.controller.model().pane(child).is_none());
        assert_eq!(agents(&app), 1);
    }
}
