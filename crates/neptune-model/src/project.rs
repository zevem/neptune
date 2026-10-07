//! A project: a lead that plans work in one local workspace and answers for
//! the agents it starts there. Only identity and membership live here; the
//! conversation and everything else the project keeps belong to the desktop.
use crate::{AgentKind, Error, ProjectId, WorkspaceId};

/// Names what a project keeps outside the model. A closed project's identity
/// can be issued again, so its saved work follows this key and never the
/// identity. Sixteen lowercase hex digits: never a path, an option or a name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectKey(String);
impl ProjectKey {
    pub const LENGTH: usize = 16;

    pub fn parse(text: &str) -> Option<Self> {
        (text.len() == Self::LENGTH
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| Self(text.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Display for ProjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub(crate) id: ProjectId,
    pub(crate) key: ProjectKey,
    pub(crate) name: String,
    pub(crate) workspace: WorkspaceId,
    pub(crate) lead: AgentKind,
    pub(crate) paused: bool,
}
impl Project {
    /// The most projects one window holds.
    pub const MAX: usize = 8;
    /// The most agents a project can have open at once.
    pub const MAX_AGENTS: usize = 6;
    /// The longest name, in characters.
    pub const MAX_NAME: usize = 80;

    pub fn id(&self) -> ProjectId {
        self.id
    }
    pub fn key(&self) -> &ProjectKey {
        &self.key
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The local workspace whose directory the project works in and whose
    /// terminals its agents are.
    pub fn workspace(&self) -> WorkspaceId {
        self.workspace
    }
    /// The CLI that leads: Claude Code or Codex.
    pub fn lead(&self) -> AgentKind {
        self.lead
    }
    /// A paused project starts no agent.
    pub fn paused(&self) -> bool {
        self.paused
    }

    pub(crate) fn validate(name: &str, lead: AgentKind) -> Result<(), Error> {
        if name.trim().is_empty() {
            return Err(Error::InvalidName);
        }
        if name.chars().count() > Self::MAX_NAME
            || !matches!(lead, AgentKind::Claude | AgentKind::Codex)
        {
            return Err(Error::InvalidProject);
        }
        Ok(())
    }
}

/// Construction data; model adoption validates identity, workspace and lead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSpec {
    pub id: ProjectId,
    pub key: ProjectKey,
    pub name: String,
    pub workspace: WorkspaceId,
    pub lead: AgentKind,
    pub paused: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentSession, Command, Controller, Effect, Limits, Model, PaneId, PaneSpec, WorkspaceSpec,
    };
    use std::path::PathBuf;

    fn key(n: u64) -> ProjectKey {
        ProjectKey::parse(&format!("{n:016x}")).unwrap()
    }
    fn agent() -> AgentSession {
        AgentSession {
            kind: AgentKind::Claude,
            session_id: Some("019a1234-5678-7000-8000-123456789abc".into()),
            cwd: std::env::temp_dir(),
        }
    }
    fn workspace(controller: &mut Controller, remote: Option<&str>) -> WorkspaceId {
        controller
            .dispatch(Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: "a".into(),
                group: None,
                remote: remote.map(str::to_owned),
            })
            .unwrap();
        controller.model().active_workspace().unwrap()
    }
    fn add(
        controller: &mut Controller,
        workspace: WorkspaceId,
        n: u64,
    ) -> Result<ProjectId, Error> {
        controller
            .dispatch(Command::AddProject {
                workspace,
                name: "shop".into(),
                key: key(n),
                lead: AgentKind::Claude,
            })
            .map(|_| controller.model().project_of(workspace).unwrap().id())
    }
    fn setup() -> (Controller, WorkspaceId, PaneId, ProjectId) {
        let mut controller = Controller::new(Model::default());
        let workspace = workspace(&mut controller, None);
        let pane = controller.model().active_pane().unwrap();
        let project = add(&mut controller, workspace, 1).unwrap();
        (controller, workspace, pane, project)
    }
    fn spawn(controller: &mut Controller, project: ProjectId) -> Result<PaneId, Error> {
        controller
            .dispatch(Command::SpawnProjectAgent {
                project,
                cwd: std::env::temp_dir(),
                worktree: None,
            })
            .map(|effects| {
                effects
                    .iter()
                    .find_map(|effect| match effect {
                        Effect::StartSession { pane, .. } => Some(*pane),
                        _ => None,
                    })
                    .unwrap()
            })
    }
    fn open(controller: &mut Controller, pane: PaneId, agent: Option<AgentSession>) -> Vec<Effect> {
        let generation = controller.model().pane(pane).unwrap().generation();
        controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent,
            })
            .unwrap()
    }
    /// A project agent whose CLI reported itself, as one that lasts has.
    fn member(controller: &mut Controller, project: ProjectId) -> PaneId {
        let pane = spawn(controller, project).unwrap();
        open(controller, pane, Some(agent()));
        pane
    }
    fn members(controller: &Controller, project: ProjectId) -> Vec<PaneId> {
        controller
            .model()
            .project_agents(project)
            .map(|pane| pane.id())
            .collect()
    }
    fn persists(effects: &[Effect]) -> bool {
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Persist { .. }))
    }
    fn restore(controller: &Controller) -> Result<Model, Error> {
        Model::restore_with_projects(
            controller.model().specs(),
            controller.model().group_specs(),
            Some(controller.model().sidebar_order().to_vec()),
            controller.model().project_specs(),
            controller.model().active_workspace(),
            controller.model().sidebar(),
            controller.model().limits(),
        )
    }

    #[test]
    fn a_key_is_sixteen_lowercase_hex_digits_and_nothing_else() {
        assert_eq!(
            ProjectKey::parse("0123456789abcdef").unwrap().as_str(),
            "0123456789abcdef"
        );
        for text in [
            "",
            "0123456789abcde",
            "0123456789abcdef0",
            "0123456789ABCDEF",
            "../../etc/passwd",
            "--0123456789abcd",
            "0123456789abcde/",
            "0123456789abcdé",
        ] {
            assert_eq!(ProjectKey::parse(text), None, "{text}");
        }
    }

    #[test]
    fn a_project_is_added_to_one_local_workspace_or_nothing_changes() {
        let mut controller = Controller::new(Model::default());
        let local = workspace(&mut controller, None);
        let remote = workspace(&mut controller, Some("me@devbox"));
        let add = |controller: &mut Controller, workspace, name: &str, n, lead| {
            controller.dispatch(Command::AddProject {
                workspace,
                name: name.into(),
                key: key(n),
                lead,
            })
        };
        let before = controller.model().clone();
        let generation = controller.generation();
        for (workspace, name, lead, error) in [
            (local, " ", AgentKind::Claude, Error::InvalidName),
            (
                local,
                &"n".repeat(Project::MAX_NAME + 1),
                AgentKind::Claude,
                Error::InvalidProject,
            ),
            (local, "shop", AgentKind::Gemini, Error::InvalidProject),
            (remote, "shop", AgentKind::Claude, Error::InvalidProject),
            (
                WorkspaceId::new(99),
                "shop",
                AgentKind::Claude,
                Error::UnknownWorkspace(WorkspaceId::new(99)),
            ),
        ] {
            assert_eq!(add(&mut controller, workspace, name, 1, lead), Err(error));
        }
        assert_eq!(controller.model(), &before);
        assert_eq!(controller.generation(), generation);

        let effects = add(
            &mut controller,
            local,
            &"é".repeat(Project::MAX_NAME),
            1,
            AgentKind::Codex,
        );
        assert!(persists(&effects.unwrap()));
        let project = controller.model().project_of(local).unwrap().clone();
        assert_eq!(
            controller.model().projects(),
            std::slice::from_ref(&project)
        );
        assert_eq!(controller.model().project(project.id()), Some(&project));
        assert_eq!(
            (project.workspace(), project.lead(), project.paused()),
            (local, AgentKind::Codex, false)
        );
        assert_eq!(project.key(), &key(1));
        assert!(controller.model().project_of(remote).is_none());
        // Its terminals and focus are untouched: a project is not a pane.
        assert_eq!(controller.model().workspaces(), before.workspaces());

        // One project to a workspace, and one project to a key.
        let other = workspace(&mut controller, None);
        let before = controller.model().clone();
        assert_eq!(
            add(&mut controller, local, "again", 2, AgentKind::Claude),
            Err(Error::ProjectExists)
        );
        assert_eq!(
            add(&mut controller, other, "again", 1, AgentKind::Claude),
            Err(Error::InvalidIdentity)
        );
        assert_eq!(controller.model(), &before);
    }

    #[test]
    fn a_window_holds_a_bounded_number_of_projects() {
        let mut controller = Controller::new(Model::default());
        for n in 0..Project::MAX as u64 {
            let workspace = workspace(&mut controller, None);
            add(&mut controller, workspace, n).unwrap();
        }
        let extra = workspace(&mut controller, None);
        let before = controller.model().clone();
        assert_eq!(add(&mut controller, extra, 99), Err(Error::ProjectLimit));
        assert_eq!(controller.model(), &before);
        // Removing one makes room, under an identity never issued before.
        let first = controller.model().projects()[0].id();
        controller.dispatch(Command::RemoveProject(first)).unwrap();
        let added = add(&mut controller, extra, 99).unwrap();
        assert!(
            controller
                .model()
                .projects()
                .iter()
                .all(|p| p.id() != first)
        );
        assert_eq!(added.get(), Project::MAX as u64 + 1);
    }

    #[test]
    fn renaming_and_pausing_persist_only_real_changes() {
        let (mut controller, _, _, project) = setup();
        let rename = |controller: &mut Controller, project, name: &str| {
            controller.dispatch(Command::RenameProject {
                project,
                name: name.into(),
            })
        };
        let pause = |controller: &mut Controller, project, paused| {
            controller.dispatch(Command::SetProjectPaused { project, paused })
        };
        assert!(rename(&mut controller, project, "shop").unwrap().is_empty());
        assert!(pause(&mut controller, project, false).unwrap().is_empty());
        assert_eq!(
            rename(&mut controller, project, ""),
            Err(Error::InvalidName)
        );
        assert_eq!(
            rename(&mut controller, project, &"n".repeat(Project::MAX_NAME + 1)),
            Err(Error::InvalidProject)
        );
        assert_eq!(controller.model().project(project).unwrap().name(), "shop");

        assert!(persists(
            &rename(&mut controller, project, "store").unwrap()
        ));
        assert!(persists(&pause(&mut controller, project, true).unwrap()));
        assert!(pause(&mut controller, project, true).unwrap().is_empty());
        let item = controller.model().project(project).unwrap();
        assert_eq!((item.name(), item.paused()), ("store", true));

        // A project that is gone, or never was, is nobody's to change.
        let unknown = ProjectId::new(99);
        let before = controller.model().clone();
        assert_eq!(
            rename(&mut controller, unknown, "x"),
            Err(Error::UnknownProject(unknown))
        );
        assert_eq!(
            pause(&mut controller, unknown, true),
            Err(Error::UnknownProject(unknown))
        );
        assert_eq!(
            controller.dispatch(Command::RemoveProject(unknown)),
            Err(Error::UnknownProject(unknown))
        );
        assert_eq!(
            spawn(&mut controller, unknown),
            Err(Error::UnknownProject(unknown))
        );
        assert_eq!(controller.model(), &before);
        controller
            .dispatch(Command::RemoveProject(project))
            .unwrap();
        assert_eq!(
            controller.dispatch(Command::RemoveProject(project)),
            Err(Error::UnknownProject(project))
        );
        assert_eq!(
            spawn(&mut controller, project),
            Err(Error::UnknownProject(project))
        );
    }

    #[test]
    fn a_project_agent_starts_out_of_view_within_the_projects_limits() {
        let (mut controller, workspace, pane, project) = setup();
        let effects = controller
            .dispatch(Command::SpawnProjectAgent {
                project,
                cwd: PathBuf::from("/fake"),
                worktree: None,
            })
            .unwrap();
        let first = members(&controller, project)[0];
        assert_eq!(
            effects[0],
            Effect::StartSession {
                pane: first,
                generation: 1,
                cwd: PathBuf::from("/fake"),
                remote: None,
                remote_cwd: None,
                replacement: false,
            }
        );
        assert!(persists(&effects));
        // Focus and what is in view stay; the pane says whose agent it is.
        assert!(!effects.iter().any(|e| matches!(e, Effect::Focus { .. })));
        let ws = controller.model().workspace(workspace).unwrap();
        assert!(ws.is_background(first));
        assert_eq!((ws.active(), ws.layout().panes()), (pane, vec![pane]));
        let item = controller.model().pane(first).unwrap();
        assert_eq!((item.project(), item.spawned_by()), (Some(project), None));
        assert_eq!(controller.model().pane(pane).unwrap().project(), None);

        // A paused project starts nothing; the agent it has stays.
        controller
            .dispatch(Command::SetProjectPaused {
                project,
                paused: true,
            })
            .unwrap();
        let before = controller.model().clone();
        assert_eq!(spawn(&mut controller, project), Err(Error::ProjectPaused));
        assert_eq!(controller.model(), &before);
        assert_eq!(members(&controller, project), [first]);
        controller
            .dispatch(Command::SetProjectPaused {
                project,
                paused: false,
            })
            .unwrap();

        let unusable = crate::Worktree {
            repository: std::env::temp_dir(),
            path: std::env::temp_dir(),
            branch: "feat/x".into(),
            start: "a".repeat(40),
        };
        assert_eq!(
            controller.dispatch(Command::SpawnProjectAgent {
                project,
                cwd: std::env::temp_dir(),
                worktree: Some(unusable.clone()),
            }),
            Err(Error::InvalidWorktree)
        );
        let worktree = crate::Worktree {
            path: std::env::temp_dir().join("repo.worktrees").join("feat-x"),
            ..unusable
        };
        controller
            .dispatch(Command::SpawnProjectAgent {
                project,
                cwd: worktree.path.clone(),
                worktree: Some(worktree.clone()),
            })
            .unwrap();
        let second = members(&controller, project)[1];
        assert_eq!(
            controller.model().pane(second).unwrap().worktree(),
            Some(&worktree)
        );

        for _ in 2..Project::MAX_AGENTS {
            spawn(&mut controller, project).unwrap();
        }
        let before = controller.model().clone();
        assert_eq!(
            spawn(&mut controller, project),
            Err(Error::ProjectAgentLimit)
        );
        assert_eq!(controller.model(), &before);
        // Opening one keeps it a member, so it still counts.
        controller
            .dispatch(Command::FocusPane {
                workspace,
                pane: first,
            })
            .unwrap();
        let ws = controller.model().workspace(workspace).unwrap();
        assert_eq!(
            (ws.layout().panes(), ws.active()),
            (vec![pane, first], first)
        );
        assert_eq!(
            controller.model().pane(first).unwrap().project(),
            Some(project)
        );
        assert_eq!(
            spawn(&mut controller, project),
            Err(Error::ProjectAgentLimit)
        );
        // One leaving makes room.
        controller.dispatch(Command::ClosePane(second)).unwrap();
        spawn(&mut controller, project).unwrap();
        assert_eq!(members(&controller, project).len(), Project::MAX_AGENTS);
    }

    #[test]
    fn project_agents_share_the_workspaces_pane_limits() {
        let mut controller = Controller::new(Model::new(Limits {
            workspaces: 4,
            panes_per_workspace: 3,
            total_panes: 4,
        }));
        let first = workspace(&mut controller, None);
        let project = add(&mut controller, first, 1).unwrap();
        spawn(&mut controller, project).unwrap();
        spawn(&mut controller, project).unwrap();
        assert_eq!(spawn(&mut controller, project), Err(Error::PaneLimit));
        let second = workspace(&mut controller, None);
        let other = add(&mut controller, second, 2).unwrap();
        assert_eq!(spawn(&mut controller, other), Err(Error::TotalPaneLimit));
    }

    #[test]
    fn a_project_agent_can_start_agents_and_those_cannot() {
        let (mut controller, _, _, project) = setup();
        let worker = member(&mut controller, project);
        let help = |controller: &mut Controller, parent| {
            let before = controller.model().pane_count();
            controller
                .dispatch(Command::SpawnAgent {
                    parent,
                    generation: 1,
                    cwd: std::env::temp_dir(),
                })
                .map(|_| controller.model().workspaces()[0].panes()[before].id())
        };
        let helper = help(&mut controller, worker).unwrap();
        open(&mut controller, helper, Some(agent()));
        assert_eq!(help(&mut controller, helper), Err(Error::SpawnDepth));
        // The helper answers to the agent that started it, not to the project.
        let item = controller.model().pane(helper).unwrap();
        assert_eq!((item.spawned_by(), item.project()), (Some(worker), None));
        assert_eq!(members(&controller, project), [worker]);
        let restored = restore(&controller).unwrap();
        assert_eq!(restored.pane(helper).unwrap().spawned_by(), Some(worker));
        // A chain one longer is not restored either.
        let mut specs = controller.model().specs();
        let mut extra = specs[0].panes.last().unwrap().clone();
        extra.id = PaneId::new(90);
        extra.spawned_by = Some(helper);
        specs[0].panes.push(extra);
        assert_eq!(
            Model::restore_with_projects(
                specs,
                Vec::new(),
                None,
                controller.model().project_specs(),
                None,
                true,
                Limits::default()
            ),
            Err(Error::InvalidLayout("invalid spawned agent"))
        );
    }

    #[test]
    fn removing_a_project_gives_its_agents_tabs_and_stops_nothing() {
        let (mut controller, workspace, pane, project) = setup();
        let first = member(&mut controller, project);
        let second = member(&mut controller, project);
        let effects = controller
            .dispatch(Command::RemoveProject(project))
            .unwrap();
        assert_eq!(
            effects,
            [Effect::Persist {
                generation: controller.generation()
            }]
        );
        assert!(controller.model().projects().is_empty());
        let ws = controller.model().workspace(workspace).unwrap();
        assert_eq!(ws.layout().panes(), [pane, second, first]);
        assert_eq!((ws.layout().shown(), ws.active()), (vec![pane], pane));
        for id in [first, second] {
            let item = controller.model().pane(id).unwrap();
            assert_eq!((item.project(), item.agent().is_some()), (None, true));
        }
        assert!(Model::restore(controller.model().specs(), None, true, Limits::default()).is_ok());
    }

    #[test]
    fn an_agent_released_from_its_project_goes_on_as_an_ordinary_terminal() {
        let (mut controller, workspace, pane, project) = setup();
        let shown = member(&mut controller, project);
        let unseen = member(&mut controller, project);
        controller
            .dispatch(Command::FocusPane {
                workspace,
                pane: shown,
            })
            .unwrap();
        // A stale request, or one for a terminal of no project, changes nothing.
        let before = controller.model().clone();
        for (target, generation) in [(shown, 2), (pane, 1), (PaneId::new(99), 1)] {
            let effects = controller
                .dispatch(Command::ReleaseProjectAgent {
                    pane: target,
                    generation,
                })
                .unwrap();
            assert!(!persists(&effects));
            assert_eq!(controller.model(), &before);
        }
        // The tab the person opened stays where it is, with its agent.
        let effects = controller
            .dispatch(Command::ReleaseProjectAgent {
                pane: shown,
                generation: 1,
            })
            .unwrap();
        assert!(persists(&effects));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::StopSession { .. }))
        );
        let item = controller.model().pane(shown).unwrap();
        assert_eq!((item.project(), item.agent().is_some()), (None, true));
        assert_eq!(members(&controller, project), [unseen]);
        let ws = controller.model().workspace(workspace).unwrap();
        assert_eq!((ws.active(), ws.is_background(unseen)), (shown, true));
        // One out of view gets a tab: nothing runs unseen that no project
        // answers for.
        controller
            .dispatch(Command::ReleaseProjectAgent {
                pane: unseen,
                generation: 1,
            })
            .unwrap();
        let ws = controller.model().workspace(workspace).unwrap();
        assert!(!ws.is_background(unseen) && ws.active() == shown);
        assert!(members(&controller, project).is_empty());
        assert!(Model::restore(controller.model().specs(), None, true, Limits::default()).is_ok());
    }

    #[test]
    fn membership_ends_with_the_agent_and_an_unseen_terminal_closes() {
        let (mut controller, workspace, _, project) = setup();
        let stopped = |effects: &[Effect], pane| {
            effects.contains(&Effect::StopSession {
                pane,
                generation: 1,
            })
        };
        // The agent leaving, the shell exiting and a failed start each end it.
        let left = member(&mut controller, project);
        let effects = open(&mut controller, left, None);
        assert!(stopped(&effects, left) && persists(&effects));
        let exited = member(&mut controller, project);
        controller
            .dispatch(Command::SessionStarted {
                pane: exited,
                generation: 1,
            })
            .unwrap();
        let effects = controller
            .dispatch(Command::SessionExited {
                pane: exited,
                generation: 1,
            })
            .unwrap();
        assert!(stopped(&effects, exited) && persists(&effects));
        let failed = spawn(&mut controller, project).unwrap();
        let effects = controller
            .dispatch(Command::SessionFailed {
                pane: failed,
                generation: 1,
                error: "no shell".into(),
            })
            .unwrap();
        assert!(stopped(&effects, failed) && persists(&effects));
        for pane in [left, exited, failed] {
            assert!(controller.model().pane(pane).is_none());
        }
        assert!(members(&controller, project).is_empty());

        // A stale completion ends nothing.
        let kept = member(&mut controller, project);
        for command in [
            Command::SessionExited {
                pane: kept,
                generation: 7,
            },
            Command::SessionFailed {
                pane: kept,
                generation: 7,
                error: "late".into(),
            },
            Command::PaneAgentChanged {
                pane: kept,
                generation: 7,
                agent: None,
            },
        ] {
            assert!(controller.dispatch(command).unwrap().is_empty());
        }
        assert_eq!(members(&controller, project), [kept]);

        // One that was opened keeps its tab and stops being a member.
        let seen = member(&mut controller, project);
        controller
            .dispatch(Command::FocusPane {
                workspace,
                pane: seen,
            })
            .unwrap();
        open(&mut controller, seen, None);
        let item = controller.model().pane(seen).unwrap();
        assert_eq!((item.project(), item.agent()), (None, None));
        // Restarting replaces the agent's shell with a plain one, in a tab.
        let effects = controller.dispatch(Command::RestartPane(kept)).unwrap();
        assert!(persists(&effects));
        let item = controller.model().pane(kept).unwrap();
        assert_eq!((item.project(), item.generation()), (None, 2));
        assert!(
            !controller
                .model()
                .workspace(workspace)
                .unwrap()
                .is_background(kept)
        );
        assert!(members(&controller, project).is_empty());
        // A shell that ends or fails in a tab keeps the tab and leaves too.
        for fails in [false, true] {
            let shown = member(&mut controller, project);
            controller
                .dispatch(Command::FocusPane {
                    workspace,
                    pane: shown,
                })
                .unwrap();
            let effects = if fails {
                controller.dispatch(Command::SessionFailed {
                    pane: shown,
                    generation: 1,
                    error: "no shell".into(),
                })
            } else {
                controller
                    .dispatch(Command::SessionStarted {
                        pane: shown,
                        generation: 1,
                    })
                    .unwrap();
                controller.dispatch(Command::SessionExited {
                    pane: shown,
                    generation: 1,
                })
            };
            assert!(persists(&effects.unwrap()));
            assert_eq!(controller.model().pane(shown).unwrap().project(), None);
        }
        assert!(members(&controller, project).is_empty());
        assert!(restore(&controller).is_ok());
    }

    #[test]
    fn a_project_leaves_with_its_workspace_and_stays_on_this_machine() {
        let (mut controller, first, pane, project) = setup();
        let worker = member(&mut controller, project);
        // Connecting the workspace elsewhere would strand the project.
        let before = controller.model().clone();
        assert_eq!(
            controller.dispatch(Command::SetWorkspaceRemote {
                workspace: first,
                remote: Some("me@devbox".into()),
            }),
            Err(Error::InvalidProject)
        );
        assert_eq!(controller.model(), &before);

        // A member moved to another workspace is that workspace's terminal.
        let second = workspace(&mut controller, None);
        controller
            .dispatch(Command::MovePane {
                pane: worker,
                destination: crate::Destination::Workspace(second),
            })
            .unwrap();
        assert_eq!(controller.model().pane(worker).unwrap().project(), None);
        assert!(members(&controller, project).is_empty());
        assert!(restore(&controller).is_ok());

        // Closing the workspace, by whichever way it goes, removes the project.
        let unseen = member(&mut controller, project);
        let effects = controller.dispatch(Command::CloseWorkspace(first)).unwrap();
        for id in [pane, unseen] {
            assert!(effects.contains(&Effect::StopSession {
                pane: id,
                generation: 1
            }));
        }
        assert!(controller.model().projects().is_empty());
        let other = add(&mut controller, second, 2).unwrap();
        let third = workspace(&mut controller, None);
        let last = controller.model().workspace(second).unwrap().panes()[0].id();
        controller.dispatch(Command::ClosePane(worker)).unwrap();
        controller
            .dispatch(Command::MovePane {
                pane: last,
                destination: crate::Destination::Workspace(third),
            })
            .unwrap();
        assert!(controller.model().workspace(second).is_none());
        assert!(controller.model().project(other).is_none());
        assert!(restore(&controller).is_ok());
    }

    #[test]
    fn a_refused_move_leaves_a_project_agent_out_of_view() {
        let mut controller = Controller::new(Model::new(Limits {
            workspaces: 4,
            panes_per_workspace: 2,
            total_panes: 8,
        }));
        let first = workspace(&mut controller, None);
        let project = add(&mut controller, first, 1).unwrap();
        let worker = member(&mut controller, project);
        let remote = workspace(&mut controller, Some("me@devbox"));
        let full = workspace(&mut controller, None);
        let pane = controller.model().workspace(full).unwrap().active();
        controller
            .dispatch(Command::AddTab {
                workspace: full,
                pane,
                cwd: std::env::temp_dir(),
            })
            .unwrap();
        let before = controller.model().clone();
        for (destination, error) in [
            (remote, Error::RemoteMismatch),
            (full, Error::PaneLimit),
            (
                WorkspaceId::new(99),
                Error::UnknownWorkspace(WorkspaceId::new(99)),
            ),
        ] {
            assert_eq!(
                controller.dispatch(Command::MovePane {
                    pane: worker,
                    destination: crate::Destination::Workspace(destination),
                }),
                Err(error)
            );
            assert_eq!(controller.model(), &before);
        }
        assert!(
            controller
                .model()
                .workspace(first)
                .unwrap()
                .is_background(worker)
        );
    }

    #[test]
    fn a_tab_goes_back_out_of_view_while_its_agent_runs_on() {
        let (mut controller, workspace, pane, project) = setup();
        let worker = member(&mut controller, project);
        let unseen = member(&mut controller, project);
        let background = |controller: &mut Controller, pane| {
            let before = controller.model().clone();
            let result = controller.dispatch(Command::BackgroundPane(pane));
            if result.is_err() {
                assert_eq!(controller.model(), &before);
            }
            assert_eq!(result.is_ok(), before.can_background(pane).is_ok());
            result
        };
        let focus = |controller: &mut Controller, pane| {
            controller
                .dispatch(Command::FocusPane { workspace, pane })
                .unwrap()
        };
        let tabs = |controller: &Controller| {
            let ws = controller.model().workspace(workspace).unwrap();
            (ws.layout().panes(), ws.active())
        };
        // One that has no tab, or does not exist, has none to put away.
        for missing in [worker, PaneId::new(99)] {
            assert_eq!(
                background(&mut controller, missing),
                Err(Error::UnknownPane(missing))
            );
        }
        focus(&mut controller, worker);
        focus(&mut controller, pane);
        assert_eq!(tabs(&controller), (vec![pane, worker], pane));

        // Nobody lists a terminal the person started: it keeps its tab.
        assert_eq!(background(&mut controller, pane), Err(Error::Unanswered));
        // Out of view the tab goes without the focus moving. Nothing stops
        // or starts, and the agent is the project's as before.
        let effects = background(&mut controller, worker).unwrap();
        assert_eq!(
            effects,
            [Effect::Persist {
                generation: controller.generation()
            }]
        );
        assert_eq!(tabs(&controller), (vec![pane], pane));
        let ws = controller.model().workspace(workspace).unwrap();
        assert!(ws.is_background(worker) && ws.is_background(unseen));
        let item = controller.model().pane(worker).unwrap();
        assert_eq!(
            (item.project(), item.agent(), item.generation()),
            (Some(project), Some(&agent()), 1)
        );
        assert_eq!(members(&controller, project), [worker, unseen]);
        // It is restored as it was left, and opens again.
        let restored = restore(&controller).unwrap();
        assert!(restored.workspaces()[0].is_background(worker));
        assert_eq!(restored.pane(worker).unwrap().project(), Some(project));
        focus(&mut controller, worker);
        assert_eq!(tabs(&controller), (vec![pane, worker], worker));

        // The focused tab leaving passes the focus to the tab in its place.
        let effects = background(&mut controller, worker).unwrap();
        assert!(effects.contains(&Effect::Focus {
            old: Some(worker),
            new: Some(pane)
        }));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::StopSession { .. } | Effect::StartSession { .. }))
        );
        // A workspace keeps a terminal in view, whatever else runs in it.
        focus(&mut controller, worker);
        controller.dispatch(Command::ClosePane(pane)).unwrap();
        assert_eq!(tabs(&controller), (vec![worker], worker));
        assert_eq!(background(&mut controller, worker), Err(Error::LastInView));
        let ws = controller.model().workspace(workspace).unwrap();
        assert!(ws.is_background(unseen));

        // An agent's own agent goes out of view while that agent is here,
        // and gets its tab back when the agent leaves.
        controller
            .dispatch(Command::SpawnAgent {
                parent: worker,
                generation: 1,
                cwd: std::env::temp_dir(),
            })
            .unwrap();
        let helper = controller.model().spawned(worker).next().unwrap().id();
        open(&mut controller, helper, Some(agent()));
        focus(&mut controller, helper);
        focus(&mut controller, unseen);
        background(&mut controller, helper).unwrap();
        background(&mut controller, worker).unwrap();
        assert_eq!(tabs(&controller), (vec![unseen], unseen));
        open(&mut controller, worker, None);
        assert!(controller.model().pane(worker).is_none());
        assert_eq!(tabs(&controller), (vec![unseen, helper], unseen));
        assert_eq!(background(&mut controller, helper), Err(Error::Unanswered));
        // So does a project's agent when the project is removed.
        focus(&mut controller, helper);
        background(&mut controller, unseen).unwrap();
        controller
            .dispatch(Command::RemoveProject(project))
            .unwrap();
        assert_eq!(tabs(&controller), (vec![helper, unseen], helper));
        assert!(restore(&controller).is_ok());
    }

    #[test]
    fn a_tab_sent_out_of_view_leaves_its_place_to_its_neighbours() {
        let (mut controller, workspace, pane, project) = setup();
        let first = member(&mut controller, project);
        let second = member(&mut controller, project);
        let focus = |controller: &mut Controller, pane| {
            controller
                .dispatch(Command::FocusPane { workspace, pane })
                .unwrap();
        };
        let layout = |controller: &Controller| {
            let ws = controller.model().workspace(workspace).unwrap();
            (ws.layout().clone(), ws.layout().shown(), ws.active())
        };
        // Among other tabs, the one that followed it comes into view.
        focus(&mut controller, second);
        focus(&mut controller, pane);
        focus(&mut controller, first);
        let (tabs, ..) = layout(&controller);
        assert_eq!(tabs.panes(), [pane, first, second]);
        controller.dispatch(Command::BackgroundPane(first)).unwrap();
        let (tabs, shown, active) = layout(&controller);
        assert_eq!(
            (tabs.panes(), shown, active),
            (vec![pane, second], vec![second], second)
        );

        // A place it had to itself closes, and the split with it.
        focus(&mut controller, first);
        controller
            .dispatch(Command::MovePane {
                pane: first,
                destination: crate::Destination::Beside {
                    pane,
                    edge: crate::Edge::Right,
                },
            })
            .unwrap();
        let (split, shown, active) = layout(&controller);
        assert!(matches!(split, crate::Layout::Split { .. }));
        assert_eq!((shown, active), (vec![second, first], first));
        controller.dispatch(Command::BackgroundPane(first)).unwrap();
        let (tabs, shown, active) = layout(&controller);
        assert!(matches!(tabs, crate::Layout::Tabs { .. }));
        assert_eq!(
            (tabs.panes(), shown, active),
            (vec![pane, second], vec![second], second)
        );
        // One that is not focused leaves the focus where it is, also in a
        // workspace that is not in front.
        focus(&mut controller, first);
        focus(&mut controller, second);
        let other = super::tests::workspace(&mut controller, None);
        let effects = controller.dispatch(Command::BackgroundPane(first)).unwrap();
        assert!(persists(&effects) && effects.len() == 1);
        assert_eq!(controller.model().active_workspace(), Some(other));
        assert_eq!(layout(&controller).2, second);
    }

    #[test]
    fn restoration_keeps_projects_and_rejects_what_cannot_be_one() {
        let (mut controller, workspace, _, project) = setup();
        let worker = member(&mut controller, project);
        let unopened = spawn(&mut controller, project).unwrap();
        // A member is restored only once its agent can be resumed.
        assert_eq!(
            restore(&controller),
            Err(Error::InvalidLayout("invalid project agent"))
        );
        open(&mut controller, unopened, Some(agent()));
        controller
            .dispatch(Command::SetProjectPaused {
                project,
                paused: true,
            })
            .unwrap();
        let restored = restore(&controller).unwrap();
        assert_eq!(restored.projects(), controller.model().projects());
        assert_eq!(restored.pane(worker).unwrap().project(), Some(project));
        assert!(restored.workspaces()[0].is_background(worker));
        assert!(restored.project(project).unwrap().paused());
        // The next project gets an identity no restored one has.
        let mut next = Controller::new(restored);
        let second = super::tests::workspace(&mut next, None);
        assert_eq!(add(&mut next, second, 2).unwrap().get(), project.get() + 1);

        let specs = || controller.model().specs();
        let projects = || controller.model().project_specs();
        let attempt = |specs: Vec<WorkspaceSpec>, projects: Vec<ProjectSpec>| {
            Model::restore_with_projects(
                specs,
                Vec::new(),
                None,
                projects,
                None,
                true,
                Limits::default(),
            )
        };
        assert!(attempt(specs(), projects()).is_ok());
        // Entries that restore no project reject a terminal that names one.
        let missing = Err(Error::UnknownProject(project));
        assert_eq!(attempt(specs(), Vec::new()), missing);
        assert_eq!(
            Model::restore(specs(), None, true, Limits::default()),
            missing
        );
        assert_eq!(
            Model::restore_grouped(specs(), Vec::new(), None, true, Limits::default()),
            missing
        );
        assert_eq!(
            Model::restore_ordered(specs(), Vec::new(), None, None, true, Limits::default()),
            missing
        );

        let with = |change: &dyn Fn(&mut ProjectSpec)| {
            let mut projects = projects();
            change(&mut projects[0]);
            attempt(specs(), projects)
        };
        assert_eq!(
            with(&|p| p.id = ProjectId::new(0)),
            Err(Error::InvalidIdentity)
        );
        assert_eq!(with(&|p| p.name = " ".into()), Err(Error::InvalidName));
        assert_eq!(
            with(&|p| p.name = "n".repeat(Project::MAX_NAME + 1)),
            Err(Error::InvalidProject)
        );
        assert_eq!(
            with(&|p| p.lead = AgentKind::Opencode),
            Err(Error::InvalidProject)
        );
        assert_eq!(
            with(&|p| p.workspace = WorkspaceId::new(99)),
            Err(Error::UnknownWorkspace(WorkspaceId::new(99)))
        );
        assert_eq!(
            with(&|p| p.id = ProjectId::new(u64::MAX)),
            Err(Error::IdentityExhausted)
        );

        // Two projects never share an identity, a key or a workspace.
        let other = |change: &dyn Fn(&mut ProjectSpec)| {
            let mut specs = specs();
            let mut plain = specs[0].clone();
            plain.id = WorkspaceId::new(50);
            plain.panes = vec![PaneSpec {
                id: PaneId::new(50),
                cwd: std::env::temp_dir(),
                remote_cwd: None,
                agent: None,
                pull_requests: Vec::new(),
                attachments: Vec::new(),
                spawned_by: None,
                project: None,
                worktree: None,
            }];
            plain.layout = crate::Layout::pane(PaneId::new(50));
            plain.active = PaneId::new(50);
            let mut remote = plain.clone();
            remote.id = WorkspaceId::new(51);
            remote.panes[0].id = PaneId::new(51);
            remote.layout = crate::Layout::pane(PaneId::new(51));
            remote.active = PaneId::new(51);
            remote.remote = Some("me@devbox".into());
            specs.extend([plain, remote]);
            let mut projects = projects();
            let mut second = ProjectSpec {
                id: ProjectId::new(7),
                key: key(7),
                workspace: WorkspaceId::new(50),
                ..projects[0].clone()
            };
            change(&mut second);
            projects.push(second);
            attempt(specs, projects)
        };
        assert!(other(&|_| ()).is_ok());
        assert_eq!(other(&|p| p.id = project), Err(Error::InvalidIdentity));
        assert_eq!(other(&|p| p.key = key(1)), Err(Error::InvalidIdentity));
        assert_eq!(
            other(&|p| p.workspace = workspace),
            Err(Error::ProjectExists)
        );
        assert_eq!(
            other(&|p| p.workspace = WorkspaceId::new(51)),
            Err(Error::InvalidProject)
        );
        let many = (0..=Project::MAX as u64)
            .map(|n| ProjectSpec {
                id: ProjectId::new(n + 1),
                key: key(n + 1),
                ..projects()[0].clone()
            })
            .collect();
        assert_eq!(attempt(specs(), many), Err(Error::ProjectLimit));

        // A member belongs to its project's workspace, was started by no
        // agent, and counts against the project's limit.
        let pane = |change: &dyn Fn(&mut Vec<PaneSpec>)| {
            let mut specs = specs();
            change(&mut specs[0].panes);
            attempt(specs, projects())
        };
        let invalid = Err(Error::InvalidLayout("invalid project agent"));
        assert_eq!(pane(&|panes| panes[1].agent = None), invalid);
        assert_eq!(
            pane(&|panes| panes[1].spawned_by = Some(panes[2].id)),
            invalid
        );
        assert_eq!(
            pane(&|panes| panes[1].project = Some(ProjectId::new(9))),
            Err(Error::UnknownProject(ProjectId::new(9)))
        );
        assert_eq!(
            pane(&|panes| {
                for n in 0..Project::MAX_AGENTS as u64 {
                    let mut extra = panes[1].clone();
                    extra.id = PaneId::new(60 + n);
                    panes.push(extra);
                }
            }),
            Err(Error::ProjectAgentLimit)
        );
        // Without its link a terminal that never had a tab has no place.
        assert!(pane(&|panes| panes[1].project = None).is_err());
        let mut moved = specs();
        let mut elsewhere = moved[0].clone();
        elsewhere.id = WorkspaceId::new(50);
        elsewhere.panes = vec![PaneSpec {
            id: PaneId::new(50),
            ..moved[0].panes[1].clone()
        }];
        elsewhere.layout = crate::Layout::pane(PaneId::new(50));
        elsewhere.active = PaneId::new(50);
        moved.push(elsewhere);
        assert_eq!(attempt(moved, projects()), invalid);
    }

    #[test]
    fn every_new_error_says_what_is_wrong() {
        for (error, text) in [
            (
                Error::UnknownProject(ProjectId::new(4)),
                "Project 4 does not exist",
            ),
            (Error::ProjectLimit, "A window can have 8 projects"),
            (Error::ProjectExists, "This workspace already has a project"),
            (
                Error::ProjectAgentLimit,
                "A project can have 6 agents open at once",
            ),
            (Error::ProjectPaused, "The project is paused"),
            (Error::LastInView, "A workspace keeps one terminal in view"),
            (
                Error::Unanswered,
                "Only a terminal an agent or a project started for its own agent runs without a tab",
            ),
            (
                Error::InvalidProject,
                "A project runs in a local workspace, with a name of at most 80 characters and Claude Code or Codex as its lead",
            ),
        ] {
            assert_eq!(error.to_string(), text);
        }
    }
}
