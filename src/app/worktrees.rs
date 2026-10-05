//! An agent's git worktree: the sheets' requests go to the git worker, and
//! its answers open the tab, mark a merged branch and remove what is done.
use super::*;
use crate::runtime::worktrees::{Event as GitEvent, Request};
use crate::ui::worktrees::{Event, Probe, Removal, Tab};
use neptune_model::{AgentKind, Worktree};

/// The worktree git is making: the terminal its tab follows and what runs
/// in it. Captured when Create is pressed, whatever is focused by then.
struct Creating {
    pane: PaneId,
    branch: String,
    agent: AgentKind,
}
#[derive(Default)]
pub(super) struct Worktrees {
    git: crate::runtime::worktrees::Worktrees,
    /// The worktrees open in terminals, as last given to the worker.
    watched: Vec<Worktree>,
    /// Those whose branch is merged with nothing uncommitted, and into what.
    finished: BTreeMap<PathBuf, String>,
    /// The directory the open sheet asked about.
    probing: Option<PathBuf>,
    creating: Option<Creating>,
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
        match self
            .worktrees
            .git
            .request(request, Arc::new(move || wake.request_repaint()))
        {
            Ok(()) => true,
            Err(error) => {
                self.ui.error = Some(error);
                false
            }
        }
    }

    /// Leaves either sheet. A worktree already being made still opens.
    pub(super) fn close_worktree_sheet(&mut self) {
        self.worktrees.probing = None;
        self.ui.worktree.removal = None;
    }

    pub(super) fn worktree_event(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            Event::New(pane) => {
                // git runs here, so only a terminal on this machine has a
                // repository it can add a worktree to.
                let Some(item) = self.controller.model().pane(pane) else {
                    return;
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
                self.worktrees.probing = Some(cwd.clone());
                if !self.git(ctx, Request::Probe { cwd }) {
                    self.ui.overlay = OverlayState::None;
                }
            }
            Event::Create { branch, agent } => {
                let OverlayState::NewWorktree(pane) = self.ui.overlay else {
                    return;
                };
                let Some(cwd) = self.worktrees.probing.clone() else {
                    return;
                };
                if self.worktrees.creating.is_some() {
                    return;
                }
                if self.git(
                    ctx,
                    Request::Create {
                        cwd,
                        branch: branch.clone(),
                    },
                ) {
                    self.ui.worktree.creating = true;
                    self.ui.worktree.error = None;
                    self.worktrees.creating = Some(Creating {
                        pane,
                        branch,
                        agent,
                    });
                }
            }
            Event::Open(worktree, agent) => {
                let OverlayState::NewWorktree(pane) = self.ui.overlay else {
                    return;
                };
                self.close_worktree_sheet();
                self.ui.overlay = OverlayState::None;
                self.open_worktree(ctx, pane, worktree, agent);
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
                self.worktrees.probing = None;
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
                self.ui.overlay = OverlayState::None;
                // A shell holds its directory open on Windows, so there the
                // terminals go first. Elsewhere they stay until git is done,
                // and a removal git refuses leaves everything as it was.
                if cfg!(windows) {
                    self.close_worktree_terminals(ctx, &removal.worktree);
                }
                self.git(
                    ctx,
                    Request::Remove {
                        worktree: removal.worktree,
                        discard,
                    },
                );
            }
        }
    }

    /// Opens `worktree` in a new tab that follows `pane` and starts `agent`
    /// there. Tabs already in the worktree are left as they are: several
    /// agents, of either CLI, can work in one worktree.
    fn open_worktree(
        &mut self,
        ctx: &egui::Context,
        pane: PaneId,
        worktree: Worktree,
        agent: AgentKind,
    ) {
        let model = self.controller.model();
        // The terminal it was asked from, or the focused one if that closed.
        let local = |pane: &PaneId| model.pane(*pane).is_some() && self.remote_of(*pane).is_none();
        let Some(pane) = Some(pane)
            .filter(local)
            .or_else(|| model.active_pane().filter(local))
        else {
            self.ui.error = Some(format!(
                "The worktree of {} is ready, but there is no local terminal to open it beside",
                worktree.branch
            ));
            return;
        };
        let Some(workspace) = model.workspace_for_pane(pane) else {
            return;
        };
        self.dispatch(
            ctx,
            Command::OpenWorktree {
                workspace,
                pane,
                worktree,
                // The adapters that start a CLI exist on Unix only.
                agent: cfg!(unix).then_some(agent),
            },
        );
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
        for pane in panes {
            self.dispatch(ctx, Command::ClosePane(pane));
        }
    }

    /// Runs once a frame: takes the worker's answers and tells it which
    /// worktrees are open, so that it looks at those and no others.
    pub(super) fn poll_worktrees(&mut self, ctx: &egui::Context) {
        for event in self.worktrees.git.drain() {
            match event {
                GitEvent::Probed { cwd, result } => {
                    if self.worktrees.probing.as_ref() == Some(&cwd)
                        && matches!(self.ui.overlay, OverlayState::NewWorktree(_))
                    {
                        self.ui.worktree.repository = match result {
                            Ok(repository) => Probe::Found(repository),
                            Err(error) => Probe::Failed(error),
                        };
                    }
                }
                GitEvent::Created { branch, result } => {
                    let Some(creating) = self
                        .worktrees
                        .creating
                        .take_if(|creating| creating.branch == branch)
                    else {
                        continue;
                    };
                    self.ui.worktree.creating = false;
                    let sheet = self.ui.overlay == OverlayState::NewWorktree(creating.pane);
                    match result {
                        Ok(worktree) => {
                            if sheet {
                                self.close_worktree_sheet();
                                self.ui.overlay = OverlayState::None;
                            }
                            self.open_worktree(ctx, creating.pane, worktree, creating.agent);
                        }
                        Err(error) if sheet => self.ui.worktree.error = Some(error),
                        Err(error) => {
                            self.ui.error =
                                Some(format!("Could not make the worktree of {branch}: {error}"))
                        }
                    }
                }
                GitEvent::Inspected { path, result } => {
                    if let Some(removal) = &mut self.ui.worktree.removal
                        && removal.worktree.path == path
                    {
                        removal.condition = Some(result);
                    }
                }
                GitEvent::Removed { worktree, result } => {
                    if let Err(error) = &result {
                        self.ui.error = Some(format!(
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
        assert_eq!(app.ui.worktree.agent, AgentKind::Codex);
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
        settle(&mut app, &ctx, "the removal", |app| {
            app.controller.model().pane(opened).is_none()
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
