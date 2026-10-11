use crate::{WindowId, WorkspaceId};

/// Durable organization of one native window. Geometry and GUI state belong
/// to the desktop; this construction data contains only workspace membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSpec {
    pub id: WindowId,
    pub workspaces: Vec<WorkspaceId>,
    pub active: Option<WorkspaceId>,
    pub sidebar: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub(crate) id: WindowId,
    pub(crate) active: Option<WorkspaceId>,
    pub(crate) sidebar: bool,
}

impl Window {
    pub fn id(&self) -> WindowId {
        self.id
    }

    pub fn active_workspace(&self) -> Option<WorkspaceId> {
        self.active
    }

    pub fn sidebar(&self) -> bool {
        self.sidebar
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Controller, Effect, Lifecycle, Limits, Model, PaneId};

    fn workspace(controller: &mut Controller, name: &str) -> WorkspaceId {
        controller
            .dispatch(Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: name.into(),
                group: None,
                remote: None,
            })
            .unwrap();
        controller.model().active_workspace().unwrap()
    }

    fn tab(controller: &mut Controller, workspace: WorkspaceId) -> PaneId {
        let pane = controller.model().workspace(workspace).unwrap().active();
        controller
            .dispatch(Command::AddTab {
                workspace,
                pane,
                cwd: std::env::temp_dir(),
            })
            .unwrap();
        controller.model().workspace(workspace).unwrap().active()
    }

    #[test]
    fn tear_off_keeps_every_live_pane_and_layout_without_starting_or_stopping() {
        let mut controller = Controller::new(Model::default());
        let moved = workspace(&mut controller, "out");
        let running = controller.model().active_pane().unwrap();
        controller.dispatch(Command::RestartPane(running)).unwrap();
        controller
            .dispatch(Command::SessionStarted {
                pane: running,
                generation: 2,
            })
            .unwrap();
        let exited = tab(&mut controller, moved);
        controller
            .dispatch(Command::SessionStarted {
                pane: exited,
                generation: 1,
            })
            .unwrap();
        controller
            .dispatch(Command::SessionExited {
                pane: exited,
                generation: 1,
            })
            .unwrap();
        let failed = tab(&mut controller, moved);
        controller
            .dispatch(Command::SessionFailed {
                pane: failed,
                generation: 1,
                error: "spawn failed".into(),
            })
            .unwrap();
        let starting = tab(&mut controller, moved);
        controller
            .dispatch(Command::OpenBrowser {
                workspace: moved,
                pane: starting,
                axis: Some(crate::Axis::Vertical),
            })
            .unwrap();
        let before = controller.model().workspace(moved).unwrap().clone();
        let effects = controller
            .dispatch(Command::DetachWorkspace(moved))
            .unwrap();
        let after = controller.model().workspace(moved).unwrap();
        assert_ne!(after.window(), WindowId::ROOT);
        assert_eq!(after.panes(), before.panes());
        assert_eq!(after.layout(), before.layout());
        assert_eq!(after.active(), before.active());
        assert_eq!(after.name(), "out");
        assert_eq!(controller.model().active_workspace(), None);
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::StartSession { .. } | Effect::StopSession { .. } | Effect::StartBrowser { .. }
        )));
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Persist { .. }))
        );
    }

    #[test]
    fn a_project_and_its_background_agents_keep_their_ownership_when_the_workspace_moves() {
        let mut controller = Controller::new(Model::default());
        let moved = workspace(&mut controller, "project");
        controller
            .dispatch(Command::AddProject {
                workspace: moved,
                name: "project".into(),
                key: crate::ProjectKey::parse("0123456789abcdef").unwrap(),
                lead: crate::AgentKind::Codex,
            })
            .unwrap();
        let project = controller.model().project_of(moved).unwrap().id();
        controller
            .dispatch(Command::SpawnProjectAgent {
                project,
                cwd: std::env::temp_dir(),
                worktree: None,
            })
            .unwrap();
        let agent = controller
            .model()
            .project_agents(project)
            .next()
            .unwrap()
            .id();
        assert!(
            controller
                .model()
                .workspace(moved)
                .unwrap()
                .is_background(agent)
        );
        let panes = controller
            .model()
            .workspace(moved)
            .unwrap()
            .panes()
            .to_vec();
        let projects = controller.model().projects().to_vec();
        let effects = controller
            .dispatch(Command::DetachWorkspace(moved))
            .unwrap();
        assert_eq!(controller.model().projects(), projects);
        assert_eq!(controller.model().workspace(moved).unwrap().panes(), panes);
        assert!(
            controller
                .model()
                .workspace(moved)
                .unwrap()
                .is_background(agent)
        );
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::StartSession { .. } | Effect::StopSession { .. }
        )));
    }

    #[test]
    fn closing_a_parent_window_reveals_its_background_agent_in_a_surviving_window() {
        let mut controller = Controller::new(Model::default());
        let original = workspace(&mut controller, "child stays here");
        let parent = controller.model().active_pane().unwrap();
        controller
            .dispatch(Command::PaneAgentChanged {
                pane: parent,
                generation: 1,
                agent: Some(crate::AgentSession {
                    kind: crate::AgentKind::Codex,
                    session_id: Some("019a1234-5678-7000-8000-123456789abc".into()),
                    cwd: std::env::temp_dir(),
                }),
            })
            .unwrap();
        controller
            .dispatch(Command::SpawnAgent {
                parent,
                generation: 1,
                cwd: std::env::temp_dir(),
            })
            .unwrap();
        let child = controller.model().spawned(parent).next().unwrap().id();
        controller
            .dispatch(Command::SessionStarted {
                pane: child,
                generation: 1,
            })
            .unwrap();
        let neighbour = tab(&mut controller, original);
        let elsewhere = workspace(&mut controller, "parent moves here");
        controller
            .dispatch(Command::MovePane {
                pane: parent,
                destination: crate::Destination::Workspace(elsewhere),
            })
            .unwrap();
        controller
            .dispatch(Command::DetachWorkspace(elsewhere))
            .unwrap();
        let window = controller.model().workspace(elsewhere).unwrap().window();
        assert!(
            controller
                .model()
                .workspace(original)
                .unwrap()
                .is_background(child)
        );
        let effects = controller.dispatch(Command::CloseWindow(window)).unwrap();
        let orphan = controller.model().pane(child).unwrap();
        assert_eq!(orphan.spawned_by(), None);
        assert_eq!(orphan.generation(), 1);
        assert_eq!(orphan.lifecycle(), &Lifecycle::Running);
        assert!(
            !controller
                .model()
                .workspace(original)
                .unwrap()
                .is_background(child)
        );
        assert_eq!(
            controller.model().workspace(original).unwrap().active(),
            neighbour
        );
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::StopSession { pane, .. } if *pane == child
        )));
        assert!(effects.contains(&Effect::StopSession {
            pane: parent,
            generation: 1,
        }));
    }

    #[test]
    fn closing_a_selected_workspace_chooses_its_next_local_neighbour() {
        let mut controller = Controller::new(Model::default());
        let first = workspace(&mut controller, "first");
        let elsewhere = workspace(&mut controller, "elsewhere");
        let next = workspace(&mut controller, "next");
        controller
            .dispatch(Command::DetachWorkspace(elsewhere))
            .unwrap();
        controller
            .dispatch(Command::SelectWorkspace(first))
            .unwrap();
        controller.dispatch(Command::CloseWorkspace(first)).unwrap();
        assert_eq!(controller.model().active_workspace(), Some(next));
        assert_eq!(
            controller
                .model()
                .window(controller.model().workspace(elsewhere).unwrap().window())
                .unwrap()
                .active_workspace(),
            Some(elsewhere)
        );
    }

    #[test]
    fn transferring_the_selected_workspace_chooses_the_next_source_window_neighbour() {
        let mut controller = Controller::new(Model::default());
        let first = workspace(&mut controller, "first");
        let moved = workspace(&mut controller, "out");
        let next = workspace(&mut controller, "next");
        controller
            .dispatch(Command::SelectWorkspace(moved))
            .unwrap();
        controller
            .dispatch(Command::DetachWorkspace(moved))
            .unwrap();
        assert_eq!(controller.model().active_workspace(), Some(next));
        let child = controller.model().workspace(moved).unwrap().window();
        controller
            .dispatch(Command::MoveWorkspaceToWindow {
                workspace: next,
                window: child,
            })
            .unwrap();
        assert_eq!(controller.model().active_workspace(), Some(first));
        assert_eq!(
            controller.model().window(child).unwrap().active_workspace(),
            Some(next)
        );
    }

    #[test]
    fn viewport_context_is_ephemeral_and_selections_and_sidebar_belong_to_each_window() {
        let mut controller = Controller::new(Model::default());
        let root = workspace(&mut controller, "root");
        let moved = workspace(&mut controller, "out");
        controller
            .dispatch(Command::DetachWorkspace(moved))
            .unwrap();
        let child = controller.model().workspace(moved).unwrap().window();
        let generation = controller.generation();
        assert!(
            controller
                .dispatch(Command::SetWindowContext(child))
                .unwrap()
                .is_empty()
        );
        assert_eq!(controller.generation(), generation);
        assert_eq!(controller.model().active_workspace(), Some(moved));
        controller.dispatch(Command::SetSidebar(false)).unwrap();
        let extra = workspace(&mut controller, "second child workspace");
        assert_eq!(controller.model().workspace(extra).unwrap().window(), child);
        controller
            .dispatch(Command::SetWindowContext(WindowId::ROOT))
            .unwrap();
        assert_eq!(controller.model().active_workspace(), Some(root));
        assert!(controller.model().sidebar());
        controller
            .dispatch(Command::SetWindowContext(child))
            .unwrap();
        assert_eq!(controller.model().active_workspace(), Some(extra));
        assert!(!controller.model().sidebar());
        controller.dispatch(Command::CloseWorkspace(extra)).unwrap();
        assert_eq!(controller.model().active_workspace(), Some(moved));
        controller
            .dispatch(Command::SetWindowContext(WindowId::ROOT))
            .unwrap();
        assert_eq!(controller.model().active_workspace(), Some(root));
    }

    #[test]
    fn moving_back_and_closing_a_window_only_affects_its_workspaces() {
        let mut controller = Controller::new(Model::default());
        let root = workspace(&mut controller, "root");
        let moved = workspace(&mut controller, "out");
        let moved_pane = controller.model().active_pane().unwrap();
        controller
            .dispatch(Command::DetachWorkspace(moved))
            .unwrap();
        let child = controller.model().workspace(moved).unwrap().window();
        let effects = controller
            .dispatch(Command::MoveWorkspaceToWindow {
                workspace: moved,
                window: WindowId::ROOT,
            })
            .unwrap();
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::StartSession { .. } | Effect::StopSession { .. }
        )));
        assert_eq!(controller.model().active_workspace(), Some(moved));
        assert!(controller.model().window(child).is_none());
        controller
            .dispatch(Command::DetachWorkspace(moved))
            .unwrap();
        let child = controller.model().workspace(moved).unwrap().window();
        assert_eq!(controller.model().active_workspace(), Some(root));
        let effects = controller.dispatch(Command::CloseWindow(child)).unwrap();
        assert!(effects.contains(&Effect::StopSession {
            pane: moved_pane,
            generation: 1
        }));
        assert_eq!(
            effects
                .iter()
                .filter(|effect| matches!(effect, Effect::StopSession { .. }))
                .count(),
            1
        );
        assert!(controller.model().workspace(root).is_some());
        assert!(controller.model().window(child).is_none());
        assert_eq!(controller.model().active_workspace(), Some(root));
    }

    #[test]
    fn starting_and_stale_completions_still_target_the_same_generation_after_tear_off() {
        let mut controller = Controller::new(Model::default());
        let moved = workspace(&mut controller, "out");
        let pane = controller.model().active_pane().unwrap();
        controller.dispatch(Command::RestartPane(pane)).unwrap();
        controller
            .dispatch(Command::DetachWorkspace(moved))
            .unwrap();
        controller
            .dispatch(Command::SessionStarted {
                pane,
                generation: 1,
            })
            .unwrap();
        assert_eq!(
            controller.model().pane(pane).unwrap().lifecycle(),
            &Lifecycle::Starting
        );
        controller
            .dispatch(Command::SessionStarted {
                pane,
                generation: 2,
            })
            .unwrap();
        assert_eq!(
            controller.model().pane(pane).unwrap().lifecycle(),
            &Lifecycle::Running
        );
    }

    #[test]
    fn restoration_validates_membership_and_advances_the_window_identity_counter() {
        let mut controller = Controller::new(Model::default());
        let workspace = workspace(&mut controller, "out");
        let root = WindowSpec {
            id: WindowId::ROOT,
            workspaces: Vec::new(),
            active: None,
            sidebar: true,
        };
        let child = WindowSpec {
            id: WindowId::new(12),
            workspaces: vec![workspace],
            active: Some(workspace),
            sidebar: false,
        };
        let model = controller.model().clone();
        assert_eq!(
            model.clone().with_windows(vec![child.clone()]),
            Err(crate::Error::InvalidWindows)
        );
        assert_eq!(
            model.clone().with_windows(vec![root.clone()]),
            Err(crate::Error::InvalidWindows)
        );
        let mut duplicate = root.clone();
        duplicate.workspaces.push(workspace);
        assert_eq!(
            model.clone().with_windows(vec![duplicate, child.clone()]),
            Err(crate::Error::InvalidWindows)
        );
        let mut invalid_active = child.clone();
        invalid_active.active = Some(WorkspaceId::new(99));
        assert_eq!(
            model
                .clone()
                .with_windows(vec![root.clone(), invalid_active]),
            Err(crate::Error::InvalidWindows)
        );
        let restored = model.with_windows(vec![root, child]).unwrap();
        let mut next = Controller::new(restored);
        next.dispatch(Command::DetachWorkspace(workspace)).unwrap();
        assert_eq!(
            next.model().workspace(workspace).unwrap().window(),
            WindowId::new(13)
        );
    }

    #[test]
    fn rejected_window_commands_are_atomic_and_aggregate_limits_do_not_reset() {
        let mut controller = Controller::new(Model::new(Limits {
            workspaces: 1,
            panes_per_workspace: 2,
            total_panes: 2,
        }));
        let moved = workspace(&mut controller, "out");
        controller
            .dispatch(Command::DetachWorkspace(moved))
            .unwrap();
        let before = controller.model().clone();
        assert_eq!(
            controller.dispatch(Command::MoveWorkspaceToWindow {
                workspace: moved,
                window: WindowId::new(99)
            }),
            Err(crate::Error::UnknownWindow(WindowId::new(99)))
        );
        assert_eq!(controller.model(), &before);
        let old_child = controller.model().workspace(moved).unwrap().window();
        controller
            .dispatch(Command::DetachWorkspace(moved))
            .unwrap();
        assert!(controller.model().window(old_child).is_none());
        assert_eq!(controller.model().windows().len(), 2);
        let child = controller.model().workspace(moved).unwrap().window();
        controller
            .dispatch(Command::SetWindowContext(child))
            .unwrap();
        assert_eq!(
            controller.dispatch(Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: "extra".into(),
                group: None,
                remote: None
            }),
            Err(crate::Error::WorkspaceLimit)
        );
    }

    #[test]
    fn moving_the_last_pane_between_windows_repairs_the_source_selection_without_stopping_it() {
        let mut controller = Controller::new(Model::default());
        let source = workspace(&mut controller, "source");
        let pane = controller.model().active_pane().unwrap();
        let destination = workspace(&mut controller, "destination");
        controller
            .dispatch(Command::DetachWorkspace(destination))
            .unwrap();
        let child = controller.model().workspace(destination).unwrap().window();
        let effects = controller
            .dispatch(Command::MovePane {
                pane,
                destination: crate::Destination::Workspace(destination),
            })
            .unwrap();
        assert!(controller.model().workspace(source).is_none());
        assert_eq!(controller.model().active_workspace(), None);
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::StopSession { .. } | Effect::StartSession { .. }
        )));
        controller
            .dispatch(Command::SetWindowContext(child))
            .unwrap();
        assert_eq!(controller.model().active_pane(), Some(pane));
    }

    #[test]
    fn mixed_window_and_pane_transitions_keep_membership_selection_and_restore_invariants() {
        let mut controller = Controller::new(Model::new(Limits {
            workspaces: 8,
            panes_per_workspace: 4,
            total_panes: 16,
        }));
        let mut seed = 0x6e657074756e65_u64;
        for step in 0..600 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let model = controller.model();
            let workspaces: Vec<_> = model
                .workspaces()
                .iter()
                .map(|workspace| (workspace.id(), workspace.active()))
                .collect();
            let windows: Vec<_> = model.windows().iter().map(Window::id).collect();
            let panes: Vec<_> = model
                .workspaces()
                .iter()
                .flat_map(|workspace| workspace.panes().iter().map(|pane| pane.id()))
                .collect();
            let target = workspaces
                .get(seed as usize % workspaces.len().max(1))
                .copied();
            let window = windows[(seed >> 20) as usize % windows.len()];
            let add = || Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: format!("workspace {step}"),
                group: None,
                remote: None,
            };
            let command = match ((seed >> 32) % 12, target) {
                (0, _) | (_, None) => add(),
                (1, Some((workspace, _))) => Command::DetachWorkspace(workspace),
                (2, Some((workspace, _))) => Command::MoveWorkspaceToWindow { workspace, window },
                (3, _) => Command::SetWindowContext(window),
                (4, Some((workspace, _))) => Command::SelectWorkspace(workspace),
                (5, Some((workspace, _))) => Command::CloseWorkspace(workspace),
                (6, _) => Command::CloseWindow(window),
                (7, Some((workspace, pane))) => Command::AddTab {
                    workspace,
                    pane,
                    cwd: std::env::temp_dir(),
                },
                (8, Some((workspace, _))) => Command::MovePane {
                    pane: panes[(seed >> 8) as usize % panes.len()],
                    destination: crate::Destination::Workspace(workspace),
                },
                (9, Some((_, pane))) => Command::RestartPane(pane),
                (10, _) => Command::SetSidebar(seed & 1 == 0),
                (_, Some((workspace, pane))) => Command::FocusPane { workspace, pane },
            };
            let context_only = matches!(command, Command::SetWindowContext(_));
            let before = controller.model().clone();
            let generation = controller.generation();
            match controller.dispatch(command) {
                Ok(effects) if context_only => {
                    assert!(effects.is_empty());
                    assert_eq!(controller.generation(), generation);
                }
                Ok(_) => {}
                Err(_) => assert_eq!(controller.model(), &before),
            }
            let model = controller.model();
            assert!(model.window(WindowId::ROOT).is_some());
            assert!(model.window(model.current_window()).is_some());
            assert!(model.windows().len() <= model.workspaces().len() + 1);
            assert!(model.pane_count() <= model.limits().total_panes);
            for workspace in model.workspaces() {
                assert!(model.window(workspace.window()).is_some());
            }
            for window in model.windows() {
                let members: Vec<_> = model
                    .workspaces()
                    .iter()
                    .filter(|workspace| workspace.window() == window.id())
                    .map(|workspace| workspace.id())
                    .collect();
                assert_eq!(window.active_workspace().is_none(), members.is_empty());
                assert!(
                    window
                        .active_workspace()
                        .is_none_or(|id| members.contains(&id))
                );
                assert!(window.id() == WindowId::ROOT || !members.is_empty());
            }
            let restored = Model::restore(
                model.specs(),
                model.active_workspace(),
                model.sidebar(),
                model.limits(),
            )
            .unwrap()
            .with_windows(model.window_specs())
            .unwrap();
            assert_eq!(restored.window_specs(), model.window_specs());
        }
    }
}
