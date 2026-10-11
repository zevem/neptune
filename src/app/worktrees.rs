//! An agent's git worktree: the sheets' requests go to the git worker, and
//! its answers open the tab, mark a merged branch and remove what is done.
use super::projects::Briefing;
use super::*;
use crate::runtime::worktrees::{Event as GitEvent, Request};
use crate::ui::worktrees::{Event, Probe, Removal, Tab};
use neptune_model::{AgentKind, ProjectId, WindowId, Worktree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Target {
    window: WindowId,
    pane: PaneId,
    generation: u64,
}
struct Probing {
    target: Target,
    cwd: PathBuf,
}

/// The worktree git is making: the terminal its tab follows and what runs
/// in it. The sheet's terminal stays the target through focus and restart.
struct Creating {
    target: Target,
    /// Where git was asked from, with the branch: what its answer names.
    cwd: PathBuf,
    branch: String,
    agent: AgentKind,
}
/// Worktrees git may be making for projects' agents at once.
const MAX_ASKED: usize = 8;
/// An agent a project's lead asked for in a worktree of its own: it starts
/// once git has made the worktree, and is refused with what git said where
/// it could not.
struct Asked {
    cwd: PathBuf,
    branch: String,
    briefing: Box<Briefing>,
}
#[derive(Default)]
pub(super) struct Worktrees {
    git: crate::runtime::worktrees::Worktrees,
    /// The worktrees open in terminals, as last given to the worker.
    watched: Vec<Worktree>,
    /// Those whose branch is merged with nothing uncommitted, and into what.
    finished: BTreeMap<PathBuf, String>,
    /// Each window's sheet retains its own terminal and directory.
    probing: BTreeMap<WindowId, Probing>,
    creating: Option<Creating>,
    /// Removal errors return to the sheet that requested them.
    removing: BTreeMap<PathBuf, WindowId>,
    /// What projects wait for, in the order it was asked.
    asked: Vec<Asked>,
}
impl Worktrees {
    /// The agents of `project` that wait for git to make their worktree.
    pub(super) fn asked_for(&self, project: ProjectId) -> usize {
        self.asked
            .iter()
            .filter(|asked| asked.briefing.project == project)
            .count()
    }
    /// The branch the worktree at `path` was merged into, where it was and
    /// nothing in it is left uncommitted.
    pub(super) fn merged_into(&self, path: &std::path::Path) -> Option<&str> {
        self.finished.get(path).map(String::as_str)
    }
}

impl App {
    pub(super) fn worktree_tab(&self, pane: &neptune_model::Pane) -> Option<Tab> {
        pane.worktree().map(|worktree| Tab {
            branch: worktree.branch.clone(),
            merged: self.worktrees.finished.get(&worktree.path).cloned(),
        })
    }

    fn git(&mut self, ctx: &egui::Context, request: Request) -> bool {
        let wake = ctx.clone();
        match self.worktrees.git.request(
            request,
            Arc::new(move || wake.request_repaint_of(egui::ViewportId::ROOT)),
        ) {
            Ok(()) => true,
            Err(error) => {
                self.ui.error = Some(error);
                false
            }
        }
    }

    /// Leaves either sheet. A worktree already being made still opens.
    pub(super) fn close_worktree_sheet(&mut self) {
        self.worktrees.probing.remove(&self.presentation_window());
        self.ui.worktree.removal = None;
    }

    fn worktree_target_alive(&self, target: Target) -> bool {
        self.controller
            .model()
            .pane(target.pane)
            .is_some_and(|pane| pane.generation() == target.generation)
            && self.remote_of(target.pane).is_none()
    }

    fn worktree_target_window(&self, target: Target) -> WindowId {
        let model = self.controller.model();
        model
            .pane(target.pane)
            .filter(|pane| pane.generation() == target.generation)
            .and_then(|_| model.workspace_for_pane(target.pane))
            .and_then(|workspace| model.workspace(workspace))
            .map_or(target.window, |workspace| workspace.window())
    }

    fn worktree_busy(&mut self, busy: bool) {
        let windows: Vec<_> = self
            .controller
            .model()
            .windows()
            .iter()
            .map(|window| window.id())
            .collect();
        for window in windows {
            if let Some(ui) = self.window_ui_state_mut(window) {
                ui.worktree.creating = busy;
            }
        }
    }

    pub(super) fn worktree_event(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            Event::New(pane) => {
                // git runs here, so only a terminal on this machine has a
                // repository it can add a worktree to.
                let Some(item) = self.controller.model().pane(pane) else {
                    return;
                };
                let target = Target {
                    window: self.presentation_window(),
                    pane,
                    generation: item.generation(),
                };
                if self.remote_of(pane).is_some() {
                    self.ui.error = Some("Worktrees are made for terminals on this machine".into());
                    return;
                }
                let cwd = self
                    .sessions
                    .get(pane)
                    .map(|session| session.metadata().cwd)
                    .unwrap_or_else(|| item.cwd().to_path_buf());
                // The CLI already in use here is the likely choice.
                if let Some(agent) = item.agent() {
                    self.ui.worktree.agent = agent.kind;
                }
                self.ui.worktree.branch.clear();
                self.ui.worktree.error = None;
                self.ui.worktree.creating = self.worktrees.creating.is_some();
                self.ui.worktree.repository = Probe::Looking;
                self.ui.worktree.removal = None;
                self.ui.overlay = OverlayState::NewWorktree(pane);
                self.ui.overlay_focus = true;
                self.worktrees.probing.insert(
                    target.window,
                    Probing {
                        target,
                        cwd: cwd.clone(),
                    },
                );
                if !self.git(ctx, Request::Probe { cwd }) {
                    self.ui.overlay = OverlayState::None;
                    self.close_worktree_sheet();
                }
            }
            Event::Create { branch, agent } => {
                let OverlayState::NewWorktree(pane) = self.ui.overlay else {
                    return;
                };
                let Some(probing) = self.worktrees.probing.get(&self.presentation_window()) else {
                    return;
                };
                let target = probing.target;
                if target.pane != pane || !self.worktree_target_alive(target) {
                    self.ui.worktree.error = Some(
                        "The terminal this sheet was opened from has closed or restarted".into(),
                    );
                    return;
                }
                let cwd = probing.cwd.clone();
                if self.worktrees.creating.is_some() {
                    return;
                }
                if self.git(
                    ctx,
                    Request::Create {
                        cwd: cwd.clone(),
                        branch: branch.clone(),
                    },
                ) {
                    self.worktree_busy(true);
                    self.ui.worktree.error = None;
                    self.worktrees.creating = Some(Creating {
                        target,
                        cwd,
                        branch,
                        agent,
                    });
                }
            }
            Event::Open(worktree, agent) => {
                let OverlayState::NewWorktree(pane) = self.ui.overlay else {
                    return;
                };
                let Some(target) = self
                    .worktrees
                    .probing
                    .get(&self.presentation_window())
                    .map(|probing| probing.target)
                    .filter(|target| target.pane == pane)
                else {
                    return;
                };
                self.close_worktree_sheet();
                self.ui.overlay = OverlayState::None;
                self.open_worktree(ctx, target, worktree, agent);
            }
            Event::RemoveOf(pane) => {
                if let Some(worktree) = self
                    .controller
                    .model()
                    .pane(pane)
                    .and_then(|pane| pane.worktree())
                    .cloned()
                {
                    self.worktree_event(ctx, Event::Remove(worktree));
                }
            }
            Event::Remove(worktree) => {
                self.worktrees.probing.remove(&self.presentation_window());
                if self.git(ctx, Request::Inspect(worktree.clone())) {
                    self.ui.worktree.removal = Some(Removal {
                        worktree,
                        condition: None,
                    });
                    self.ui.overlay = OverlayState::RemoveWorktree;
                }
            }
            Event::ConfirmRemove { discard } => {
                if self.ui.overlay != OverlayState::RemoveWorktree {
                    return;
                }
                let Some(removal) = self.ui.worktree.removal.take() else {
                    return;
                };
                if self.worktrees.removing.contains_key(&removal.worktree.path)
                    || self.worktrees.removing.len() >= MAX_ASKED
                {
                    self.ui.worktree.removal = Some(removal);
                    self.ui.error = Some(
                        "Neptune is still removing other worktrees; try again in a moment".into(),
                    );
                    return;
                }
                let owner = self.presentation_window();
                self.ui.overlay = OverlayState::None;
                // A shell holds its directory open on Windows, so there the
                // terminals go first. Elsewhere they stay until git is done,
                // and a removal git refuses leaves everything as it was.
                if cfg!(windows) {
                    self.close_worktree_terminals(ctx, &removal.worktree);
                }
                let path = removal.worktree.path.clone();
                if self.git(
                    ctx,
                    Request::Remove {
                        worktree: removal.worktree,
                        discard,
                    },
                ) {
                    self.worktrees.removing.insert(path, owner);
                }
            }
        }
    }

    /// Has git fetch a pull request into a branch of its own and make that
    /// branch's worktree, beside the terminal in `pane`: its tab opens with
    /// `agent` when git answers, as one made from the sheet does.
    pub(super) fn checkout_worktree(
        &mut self,
        ctx: &egui::Context,
        pane: PaneId,
        (repository, number): (String, u64),
        agent: AgentKind,
    ) -> Result<(), &'static str> {
        if self.remote_of(pane).is_some() {
            return Err("Worktrees are made for terminals on this machine");
        }
        if self.worktrees.creating.is_some() {
            return Err("Another worktree is being made. Try again in a moment.");
        }
        let Some(item) = self.controller.model().pane(pane) else {
            return Err("Open a terminal first.");
        };
        let target = Target {
            window: self.presentation_window(),
            pane,
            generation: item.generation(),
        };
        let cwd = self
            .sessions
            .get(pane)
            .map(|session| session.metadata().cwd)
            .unwrap_or_else(|| item.cwd().to_path_buf());
        let branch = format!("pr-{number}");
        let asked = self.git(
            ctx,
            Request::Checkout {
                cwd: cwd.clone(),
                branch: branch.clone(),
                repository,
                number,
            },
        );
        if !asked {
            return Err("Could not start git.");
        }
        self.worktrees.creating = Some(Creating {
            target,
            cwd,
            branch,
            agent,
        });
        self.worktree_busy(true);
        Ok(())
    }

    /// Has git make the worktree of the branch a project's lead named for
    /// a new agent, in the repository the project works in. The agent
    /// starts when git answers, on a later frame. `Err` hands the request
    /// back with why git was not asked.
    pub(super) fn project_worktree(
        &mut self,
        ctx: &egui::Context,
        briefing: Box<Briefing>,
    ) -> Result<(), (Box<Briefing>, String)> {
        // In the repository the project works in, wherever its workspace
        // was opened.
        let Some(cwd) = self.project_home(briefing.project) else {
            return Err((briefing, "This project no longer exists.".into()));
        };
        let Some(branch) = briefing.task.worktree.clone() else {
            return Err((briefing, "No branch was named for the worktree.".into()));
        };
        if self.worktrees.asked.len() >= MAX_ASKED {
            return Err((
                briefing,
                "Neptune is still making other worktrees; try again in a moment.".into(),
            ));
        }
        let wake = ctx.clone();
        let asked = self.worktrees.git.request(
            Request::Create {
                cwd: cwd.clone(),
                branch: branch.clone(),
            },
            Arc::new(move || wake.request_repaint_of(egui::ViewportId::ROOT)),
        );
        match asked {
            Ok(()) => {
                self.worktrees.asked.push(Asked {
                    cwd,
                    branch,
                    briefing,
                });
                Ok(())
            }
            Err(error) => Err((briefing, format!("{error}."))),
        }
    }

    /// Opens `worktree` in a new tab that follows the captured terminal and starts `agent`
    /// there. Tabs already in the worktree are left as they are: several
    /// agents, of either CLI, can work in one worktree.
    fn open_worktree(
        &mut self,
        ctx: &egui::Context,
        target: Target,
        worktree: Worktree,
        agent: AgentKind,
    ) {
        if !self.worktree_target_alive(target) {
            if let Some(ui) = self.window_ui_state_mut(target.window) {
                ui.error = Some(format!(
                    "The worktree of {} is ready, but its source terminal has closed or restarted",
                    worktree.branch
                ));
            }
            return;
        }
        let model = self.controller.model();
        let Some(workspace) = model.workspace_for_pane(target.pane) else {
            return;
        };
        let owner = model
            .workspace(workspace)
            .map(|workspace| workspace.window());
        let previous = self.presentation_window();
        if let Some(owner) = owner {
            self.enter_window(owner);
        }
        self.dispatch(
            ctx,
            Command::OpenWorktree {
                workspace,
                pane: target.pane,
                worktree,
                // The adapters that start a CLI exist on Unix only.
                agent: cfg!(unix).then_some(agent),
            },
        );
        self.enter_window(previous);
    }

    fn close_worktree_terminals(&mut self, ctx: &egui::Context, worktree: &Worktree) {
        let panes: Vec<PaneId> = self
            .controller
            .model()
            .workspaces()
            .iter()
            .flat_map(|workspace| workspace.panes())
            .filter(|pane| {
                pane.worktree()
                    .is_some_and(|open| open.path == worktree.path)
            })
            .map(|pane| pane.id())
            .collect();
        // The sheet that asked said they close; nothing asks again.
        let previous = self.presentation_window();
        for pane in panes {
            let owner = self
                .controller
                .model()
                .workspace_for_pane(pane)
                .and_then(|workspace| self.controller.model().workspace(workspace))
                .map(|workspace| workspace.window());
            if let Some(owner) = owner {
                self.enter_window(owner);
            }
            self.dispatch(ctx, Command::ClosePane(pane));
        }
        self.enter_window(previous);
    }

    fn worktree_answer(&mut self, ctx: &egui::Context, event: GitEvent) {
        match event {
            GitEvent::Probed { cwd, result } => {
                let targets: Vec<_> = self
                    .worktrees
                    .probing
                    .values()
                    .filter(|probing| probing.cwd == cwd)
                    .map(|probing| probing.target)
                    .collect();
                for target in targets {
                    if !self.worktree_target_alive(target) {
                        continue;
                    }
                    if let Some(ui) = self.window_ui_state_mut(target.window)
                        && ui.overlay == OverlayState::NewWorktree(target.pane)
                    {
                        ui.worktree.repository = match result.clone() {
                            Ok(repository) => Probe::Found(repository),
                            Err(error) => Probe::Failed(error),
                        };
                    }
                }
            }
            GitEvent::Created {
                cwd,
                branch,
                result,
            } => {
                // The sheet's own request first, then a project's: git
                // answers in the order it was asked, and two that ask
                // for one branch are given the same worktree.
                let Some(creating) = self
                    .worktrees
                    .creating
                    .take_if(|creating| creating.branch == branch && creating.cwd == cwd)
                else {
                    if let Some(at) = self
                        .worktrees
                        .asked
                        .iter()
                        .position(|asked| asked.branch == branch && asked.cwd == cwd)
                    {
                        let asked = self.worktrees.asked.remove(at);
                        self.worktree_made(ctx, *asked.briefing, result);
                    }
                    return;
                };
                self.worktree_busy(false);
                let target = creating.target;
                let owner = self.worktree_target_window(target);
                let sheet = self
                    .worktrees
                    .probing
                    .get(&target.window)
                    .is_some_and(|probing| probing.target == target)
                    && self
                        .window_ui_state(target.window)
                        .is_some_and(|ui| ui.overlay == OverlayState::NewWorktree(target.pane));
                match result {
                    Ok(worktree) => {
                        if sheet {
                            self.worktrees.probing.remove(&target.window);
                            if let Some(ui) = self.window_ui_state_mut(target.window) {
                                ui.worktree.removal = None;
                                ui.overlay = OverlayState::None;
                            }
                        }
                        self.open_worktree(ctx, target, worktree, creating.agent);
                    }
                    Err(error) if sheet && owner == target.window => {
                        if let Some(ui) = self.window_ui_state_mut(owner) {
                            ui.worktree.error = Some(error);
                        }
                    }
                    Err(error) => {
                        if let Some(ui) = self.window_ui_state_mut(owner) {
                            ui.error =
                                Some(format!("Could not make the worktree of {branch}: {error}"));
                        }
                    }
                }
            }
            GitEvent::Inspected { path, result } => {
                let windows: Vec<_> = self
                    .controller
                    .model()
                    .windows()
                    .iter()
                    .map(|window| window.id())
                    .collect();
                for window in windows {
                    if let Some(ui) = self.window_ui_state_mut(window)
                        && ui.overlay == OverlayState::RemoveWorktree
                        && let Some(removal) = &mut ui.worktree.removal
                        && removal.worktree.path == path
                    {
                        removal.condition = Some(result.clone());
                    }
                }
            }
            GitEvent::Removed { worktree, result } => {
                let owner = self.worktrees.removing.remove(&worktree.path);
                if let Err(error) = &result
                    && let Some(owner) = owner
                    && let Some(ui) = self.window_ui_state_mut(owner)
                {
                    ui.error = Some(format!(
                        "Could not remove the worktree of {}: {error}",
                        worktree.branch
                    ));
                }
                // Its terminals have nowhere left to be.
                if result.is_ok() || !worktree.path.exists() {
                    self.close_worktree_terminals(ctx, &worktree);
                }
            }
            GitEvent::Finished { path, merged } => {
                match merged {
                    Some(into) => self.worktrees.finished.insert(path, into),
                    None => self.worktrees.finished.remove(&path),
                };
            }
        }
    }

    /// Runs once a frame: takes the worker's answers and tells it which
    /// worktrees are open, so that it looks at those and no others.
    pub(super) fn poll_worktrees(&mut self, ctx: &egui::Context) {
        for event in self.worktrees.git.drain() {
            self.worktree_answer(ctx, event);
        }
        let model = self.controller.model();
        self.worktrees
            .probing
            .retain(|window, _| model.window(*window).is_some());
        let mut open: Vec<Worktree> = Vec::new();
        for pane in self
            .controller
            .model()
            .workspaces()
            .iter()
            .flat_map(|workspace| workspace.panes())
        {
            if let Some(worktree) = pane.worktree()
                && !open.iter().any(|known| known.path == worktree.path)
            {
                open.push(worktree.clone());
            }
        }
        if open != self.worktrees.watched {
            self.worktrees
                .finished
                .retain(|path, _| open.iter().any(|worktree| &worktree.path == path));
            if self.git(ctx, Request::Watch(open.clone())) {
                self.worktrees.watched = open;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_fixture(directory: &std::path::Path) -> (App, Target, Target) {
        let (mut app, _) = super::super::tests::fixture(directory);
        app.startup = None;
        app.command = None;
        let mut panes = Vec::new();
        for name in ["Root", "Child"] {
            app.controller
                .dispatch(Command::AddWorkspace {
                    group: None,
                    cwd: directory.join(name),
                    name: name.into(),
                    remote: None,
                })
                .unwrap();
            panes.push(app.controller.model().active_pane().unwrap());
        }
        let workspace = app.controller.model().workspace_for_pane(panes[1]).unwrap();
        app.controller
            .dispatch(Command::DetachWorkspace(workspace))
            .unwrap();
        let child = app
            .controller
            .model()
            .workspace(workspace)
            .unwrap()
            .window();
        let root_target = Target {
            window: WindowId::ROOT,
            pane: panes[0],
            generation: 1,
        };
        let child_target = Target {
            window: child,
            pane: panes[1],
            generation: 1,
        };
        app.enter_window(child);
        app.enter_window(WindowId::ROOT);
        (app, root_target, child_target)
    }

    fn sheet(app: &mut App, target: Target, cwd: PathBuf) {
        app.worktrees
            .probing
            .insert(target.window, Probing { target, cwd });
        let ui = app.window_ui_state_mut(target.window).unwrap();
        ui.overlay = OverlayState::NewWorktree(target.pane);
        ui.worktree.repository = Probe::Looking;
    }

    fn tree(directory: &std::path::Path) -> Worktree {
        Worktree {
            repository: directory.join("repository"),
            path: directory.join("repository.worktrees/fix-login"),
            branch: "fix-login".into(),
            start: "a".repeat(40),
        }
    }

    fn creating(app: &mut App, target: Target, cwd: PathBuf, branch: &str) {
        app.worktrees.creating = Some(Creating {
            target,
            cwd,
            branch: branch.into(),
            agent: AgentKind::Codex,
        });
        app.worktree_busy(true);
    }

    #[test]
    fn closing_the_root_sheet_does_not_cancel_a_child_repository_probe() {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, root, child) = windows_fixture(directory.path());
        let root_cwd = directory.path().join("root");
        let child_cwd = directory.path().join("child");
        sheet(&mut app, root, root_cwd);
        sheet(&mut app, child, child_cwd.clone());
        app.close_worktree_sheet();
        app.ui.overlay = OverlayState::None;
        app.worktree_answer(
            &egui::Context::default(),
            GitEvent::Probed {
                cwd: child_cwd,
                result: Err("Child repository missing".into()),
            },
        );
        assert!(!app.worktrees.probing.contains_key(&root.window));
        assert!(app.worktrees.probing.contains_key(&child.window));
        assert!(
            matches!(&app.window_ui_state(child.window).unwrap().worktree.repository,
            Probe::Failed(error) if error == "Child repository missing")
        );
        assert!(matches!(app.ui.worktree.repository, Probe::Looking));
    }

    #[test]
    fn simultaneous_window_probes_update_only_the_matching_sheets() {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, root, child) = windows_fixture(directory.path());
        let root_cwd = directory.path().join("root");
        let child_cwd = directory.path().join("child");
        sheet(&mut app, root, root_cwd.clone());
        sheet(&mut app, child, child_cwd.clone());
        let ctx = egui::Context::default();
        app.worktree_answer(
            &ctx,
            GitEvent::Probed {
                cwd: child_cwd,
                result: Err("Child".into()),
            },
        );
        assert!(matches!(app.ui.worktree.repository, Probe::Looking));
        app.worktree_answer(
            &ctx,
            GitEvent::Probed {
                cwd: root_cwd,
                result: Err("Root".into()),
            },
        );
        assert!(matches!(&app.ui.worktree.repository, Probe::Failed(error) if error == "Root"));
        assert!(
            matches!(&app.window_ui_state(child.window).unwrap().worktree.repository,
            Probe::Failed(error) if error == "Child")
        );
    }

    #[test]
    fn a_child_creation_completion_opens_its_tab_and_preserves_the_root_sheet() {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, root, child) = windows_fixture(directory.path());
        let ctx = egui::Context::default();
        let cwd = directory.path().join("child");
        let worktree = tree(directory.path());
        sheet(&mut app, child, cwd.clone());
        app.ui.overlay = OverlayState::Settings;
        creating(&mut app, child, cwd.clone(), &worktree.branch);
        app.worktree_answer(
            &ctx,
            GitEvent::Created {
                cwd,
                branch: worktree.branch.clone(),
                result: Ok(worktree.clone()),
            },
        );
        assert_eq!(app.presentation_window(), root.window);
        assert_eq!(app.ui.overlay, OverlayState::Settings);
        assert_eq!(app.controller.model().active_pane(), Some(root.pane));
        let child_window = app.controller.model().window(child.window).unwrap();
        let workspace = app
            .controller
            .model()
            .workspace(child_window.active_workspace().unwrap())
            .unwrap();
        assert_eq!(workspace.panes().len(), 2);
        assert_eq!(
            app.controller
                .model()
                .pane(workspace.active())
                .unwrap()
                .worktree(),
            Some(&worktree)
        );
        assert_eq!(
            app.window_ui_state(child.window).unwrap().overlay,
            OverlayState::None
        );
        assert!(app.worktrees.creating.is_none());
    }

    #[test]
    fn a_child_creation_failure_updates_its_sheet_and_releases_other_windows() {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, root, child) = windows_fixture(directory.path());
        let cwd = directory.path().join("child");
        sheet(&mut app, root, directory.path().join("root"));
        sheet(&mut app, child, cwd.clone());
        creating(&mut app, child, cwd.clone(), "fix-login");
        app.worktree_answer(
            &egui::Context::default(),
            GitEvent::Created {
                cwd,
                branch: "fix-login".into(),
                result: Err("Branch is checked out".into()),
            },
        );
        let ui = app.window_ui_state(child.window).unwrap();
        assert_eq!(ui.overlay, OverlayState::NewWorktree(child.pane));
        assert_eq!(ui.worktree.error.as_deref(), Some("Branch is checked out"));
        assert!(!ui.worktree.creating);
        assert_eq!(app.ui.overlay, OverlayState::NewWorktree(root.pane));
        assert!(app.ui.worktree.error.is_none());
        assert!(!app.ui.worktree.creating);
    }

    #[test]
    fn a_creation_failure_follows_its_surviving_source_after_a_child_rejoins() {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, root, child) = windows_fixture(directory.path());
        let cwd = directory.path().join("child");
        sheet(&mut app, child, cwd.clone());
        creating(&mut app, child, cwd.clone(), "fix-login");
        app.ui.overlay = OverlayState::Settings;
        let workspace = app
            .controller
            .model()
            .workspace_for_pane(child.pane)
            .unwrap();
        app.controller
            .dispatch(Command::MoveWorkspaceToWindow {
                workspace,
                window: root.window,
            })
            .unwrap();
        assert!(app.controller.model().window(child.window).is_none());
        app.worktree_answer(
            &egui::Context::default(),
            GitEvent::Created {
                cwd,
                branch: "fix-login".into(),
                result: Err("Branch is checked out".into()),
            },
        );
        assert_eq!(app.ui.overlay, OverlayState::Settings);
        assert_eq!(
            app.ui.error.as_deref(),
            Some("Could not make the worktree of fix-login: Branch is checked out")
        );
        assert!(app.worktrees.creating.is_none());
    }

    #[test]
    fn a_closed_or_restarted_create_target_does_not_retarget_another_terminal() {
        for restart in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let (mut app, root, child) = windows_fixture(directory.path());
            let cwd = directory.path().join("child");
            let worktree = tree(directory.path());
            sheet(&mut app, child, cwd.clone());
            app.ui.overlay = OverlayState::Settings;
            creating(&mut app, child, cwd.clone(), &worktree.branch);
            app.controller
                .dispatch(if restart {
                    Command::RestartPane(child.pane)
                } else {
                    Command::ClosePane(child.pane)
                })
                .unwrap();
            let count = app.controller.model().pane_count();
            app.worktree_answer(
                &egui::Context::default(),
                GitEvent::Created {
                    cwd,
                    branch: worktree.branch.clone(),
                    result: Ok(worktree),
                },
            );
            assert_eq!(app.controller.model().pane_count(), count);
            assert_eq!(app.controller.model().active_pane(), Some(root.pane));
            assert_eq!(app.ui.overlay, OverlayState::Settings);
            assert!(app.ui.error.is_none());
            if restart {
                assert!(
                    app.window_ui_state(child.window)
                        .unwrap()
                        .error
                        .as_deref()
                        .unwrap()
                        .contains("source terminal")
                );
            }
        }
    }

    #[test]
    fn child_removal_checks_and_errors_do_not_update_the_root_sheet() {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, root, child) = windows_fixture(directory.path());
        let worktree = tree(directory.path());
        std::fs::create_dir_all(&worktree.path).unwrap();
        for (target, path) in [
            (root, directory.path().join("other")),
            (child, worktree.path.clone()),
        ] {
            let ui = app.window_ui_state_mut(target.window).unwrap();
            ui.overlay = OverlayState::RemoveWorktree;
            ui.worktree.removal = Some(Removal {
                worktree: Worktree {
                    path,
                    ..worktree.clone()
                },
                condition: None,
            });
        }
        let ctx = egui::Context::default();
        app.worktree_answer(
            &ctx,
            GitEvent::Inspected {
                path: worktree.path.clone(),
                result: Ok(Default::default()),
            },
        );
        assert!(
            app.window_ui_state(child.window)
                .unwrap()
                .worktree
                .removal
                .as_ref()
                .unwrap()
                .condition
                .is_some()
        );
        assert!(
            app.ui
                .worktree
                .removal
                .as_ref()
                .unwrap()
                .condition
                .is_none()
        );
        app.worktrees
            .removing
            .insert(worktree.path.clone(), child.window);
        app.worktree_answer(
            &ctx,
            GitEvent::Removed {
                worktree,
                result: Err("Changes remain".into()),
            },
        );
        assert!(
            app.window_ui_state(child.window)
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("Changes remain")
        );
        assert!(app.ui.error.is_none());
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
    fn commit(directory: &std::path::Path, file: &str) {
        std::fs::write(directory.join(file), file).unwrap();
        git(directory, &["add", "--all"]);
        git(directory, &["commit", "--quiet", "-m", file]);
    }
    /// Frames until the git worker has answered.
    fn settle(app: &mut App, ctx: &egui::Context, what: &str, done: impl Fn(&App) -> bool) {
        let limit = Instant::now() + Duration::from_secs(30);
        loop {
            app.poll_worktrees(ctx);
            if done(app) {
                return;
            }
            assert!(Instant::now() < limit, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_branch_named_in_the_sheet_becomes_a_tab_in_its_worktree_and_goes_once_merged() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join("project");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        for (key, value) in [
            ("user.name", "Neptune Test"),
            ("user.email", "test@neptune.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            git(&root, &["config", key, value]);
        }
        commit(&root, "README.md");
        // As git names it, which is how every worktree's path is reported.
        let root: PathBuf = std::path::Path::new(&git(&root, &["rev-parse", "--show-toplevel"]))
            .components()
            .collect();
        let (mut app, _sender) = super::super::tests::fixture(directory.path());
        let ctx = egui::Context::default();
        app.startup = None;
        // A launch command would claim the focused terminal for a shell.
        app.command = None;
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: root.clone(),
                name: "One".into(),
                remote: None,
            })
            .unwrap();
        let first = app.controller.model().active_pane().unwrap();

        // The sheet opens at once and learns of the repository off the frame.
        app.action(&ctx, Action::Worktree(Event::New(first)));
        assert_eq!(app.ui.overlay, OverlayState::NewWorktree(first));
        settle(&mut app, &ctx, "the repository", |app| {
            !matches!(app.ui.worktree.repository, Probe::Looking)
        });
        let Probe::Found(repository) = &app.ui.worktree.repository else {
            panic!("no repository was found");
        };
        assert_eq!(
            (repository.root.as_path(), repository.base.as_str()),
            (root.as_path(), "main")
        );

        // A name git refuses stays in the sheet with what git said.
        app.action(
            &ctx,
            Action::Worktree(Event::Create {
                branch: "main".into(),
                agent: AgentKind::Codex,
            }),
        );
        assert!(app.ui.worktree.creating);
        settle(&mut app, &ctx, "the refusal", |app| {
            !app.ui.worktree.creating
        });
        assert!(
            app.ui
                .worktree
                .error
                .as_deref()
                .unwrap()
                .contains("checked out")
        );
        assert_eq!(app.controller.model().pane_count(), 1);

        app.action(
            &ctx,
            Action::Worktree(Event::Create {
                branch: "fix-login".into(),
                agent: AgentKind::Codex,
            }),
        );
        settle(&mut app, &ctx, "the tab", |app| {
            app.controller.model().pane_count() == 2
        });
        assert_eq!(app.ui.overlay, OverlayState::None);
        let model = app.controller.model();
        let opened = model.active_pane().unwrap();
        let workspace = &model.workspaces()[0];
        assert_eq!(workspace.layout().panes(), [first, opened]);
        let pane = model.pane(opened).unwrap();
        let tree = pane.worktree().unwrap().clone();
        assert_eq!(tree.branch, "fix-login");
        assert_eq!(
            tree.path,
            root.parent().unwrap().join("project.worktrees/fix-login")
        );
        assert_eq!(pane.cwd(), tree.path);
        assert_eq!(
            pane.agent()
                .map(|agent| (agent.kind, agent.session_id.clone())),
            cfg!(unix).then_some((AgentKind::Codex, None))
        );
        // The tab is named by the branch, and says nothing of a merge yet.
        assert_eq!(
            app.presentations()[&opened].worktree,
            Some(Tab {
                branch: "fix-login".into(),
                merged: None
            })
        );

        // Asked for again, the worktree gets another agent in a tab of its
        // own, whether the same CLI or the other one, and nothing is made twice.
        app.action(&ctx, Action::Worktree(Event::New(opened)));
        // The sheet offers the CLI the terminal runs, where one can be run.
        if cfg!(unix) {
            assert_eq!(app.ui.worktree.agent, AgentKind::Codex);
        }
        settle(
            &mut app,
            &ctx,
            "the list",
            |app| matches!(&app.ui.worktree.repository, Probe::Found(found) if found.worktrees.len() == 1),
        );
        for (count, agent) in [(3, AgentKind::Codex), (4, AgentKind::Claude)] {
            app.action(&ctx, Action::Worktree(Event::New(opened)));
            app.action(&ctx, Action::Worktree(Event::Open(tree.clone(), agent)));
            assert_eq!(app.ui.overlay, OverlayState::None);
            let model = app.controller.model();
            assert_eq!(model.pane_count(), count);
            let added = model.active_pane().unwrap();
            assert_ne!(added, opened);
            let pane = model.pane(added).unwrap();
            assert_eq!(pane.worktree(), Some(&tree));
            assert_eq!(
                pane.agent().map(|agent| agent.kind),
                cfg!(unix).then_some(agent)
            );
        }
        // Typing the branch's name does the same.
        app.action(&ctx, Action::Worktree(Event::New(opened)));
        app.action(
            &ctx,
            Action::Worktree(Event::Create {
                branch: "fix-login".into(),
                agent: AgentKind::Claude,
            }),
        );
        settle(&mut app, &ctx, "the tab", |app| {
            app.controller.model().pane_count() == 5
        });
        for pane in app.controller.model().workspaces()[0].layout().panes() {
            if pane != first && pane != opened {
                app.dispatch(&ctx, Command::ClosePane(pane));
            }
        }
        assert_eq!(app.controller.model().pane_count(), 2);
        app.action(&ctx, Action::Focus(opened));

        // Work that is not committed is named before anything is removed,
        // and a removal git refuses leaves the tab and the folder alone.
        commit(&tree.path, "login.rs");
        std::fs::write(tree.path.join("draft.txt"), "draft").unwrap();
        app.action(&ctx, Action::Worktree(Event::RemoveOf(opened)));
        assert_eq!(app.ui.overlay, OverlayState::RemoveWorktree);
        settle(&mut app, &ctx, "what it holds", |app| {
            app.ui
                .worktree
                .removal
                .as_ref()
                .is_some_and(|removal| removal.condition.is_some())
        });
        let removal = app.ui.worktree.removal.as_ref().unwrap();
        let Some(Ok(condition)) = &removal.condition else {
            panic!("the worktree could not be read");
        };
        assert_eq!((condition.uncommitted, condition.unmerged), (1, 1));
        app.action(&ctx, Action::CloseOverlay);
        assert!(app.ui.worktree.removal.is_none());
        assert!(app.controller.model().pane(opened).is_some() && tree.path.exists());

        // Merged, with nothing left uncommitted: the tab offers the cleanup.
        std::fs::remove_file(tree.path.join("draft.txt")).unwrap();
        git(&root, &["merge", "--quiet", "--no-edit", "fix-login"]);
        app.action(&ctx, Action::Worktree(Event::RemoveOf(opened)));
        settle(&mut app, &ctx, "the merge", |app| {
            app.presentations()[&opened]
                .worktree
                .as_ref()
                .is_some_and(|tab| tab.merged.as_deref() == Some("main"))
                && app
                    .ui
                    .worktree
                    .removal
                    .as_ref()
                    .is_some_and(|removal| removal.condition.is_some())
        });
        app.action(
            &ctx,
            Action::Worktree(Event::ConfirmRemove { discard: false }),
        );
        assert_eq!(app.ui.overlay, OverlayState::None);
        // Where the terminals close first, git is still at work after them.
        settle(&mut app, &ctx, "the removal", |app| {
            app.controller.model().pane(opened).is_none()
                && (app.ui.error.is_some()
                    || !tree.path.exists()
                        && git(&root, &["branch", "--format=%(refname:short)"]) == "main")
        });
        assert!(app.ui.error.is_none(), "{:?}", app.ui.error);
        assert!(!tree.path.exists());
        assert_eq!(git(&root, &["branch", "--format=%(refname:short)"]), "main");
        assert_eq!(app.controller.model().active_pane(), Some(first));
    }

    #[test]
    fn a_terminal_outside_a_repository_or_on_another_machine_makes_no_worktree() {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, _sender) = super::super::tests::fixture(directory.path());
        let ctx = egui::Context::default();
        app.startup = None;
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: directory.path().into(),
                name: "One".into(),
                remote: None,
            })
            .unwrap();
        let pane = app.controller.model().active_pane().unwrap();
        app.action(&ctx, Action::Worktree(Event::New(pane)));
        settle(&mut app, &ctx, "the answer", |app| {
            !matches!(app.ui.worktree.repository, Probe::Looking)
        });
        assert!(matches!(
            &app.ui.worktree.repository,
            Probe::Failed(error) if error == "This terminal is not in a git repository"
        ));
        // Create has nothing to work with; the sheet stays for a correction.
        app.action(&ctx, Action::CloseOverlay);
        let workspace = app.controller.model().active_workspace().unwrap();
        app.controller
            .dispatch(Command::SetWorkspaceRemote {
                workspace,
                remote: Some("devbox".into()),
            })
            .unwrap();
        app.action(&ctx, Action::Worktree(Event::New(pane)));
        assert_eq!(app.ui.overlay, OverlayState::None);
        assert!(app.ui.error.is_some());
    }
}
