use crate::{
    Axis, Edge, Error, Layout, Lifecycle, Model, Pane, PaneId, Project, ProjectId, ProjectKey,
    Remote, SidebarItem, SplitId, Workspace, WorkspaceGroup, WorkspaceGroupId, WorkspaceId,
};
use std::path::PathBuf;

/// Where a moved pane lands. The pane keeps its session and identity, so it
/// can only enter a workspace on the same machine as its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// In a place of its own against one edge of a pane's tab group. Naming
    /// the moved pane itself splits it away from the tabs it shares a place with.
    Beside { pane: PaneId, edge: Edge },
    /// Among the tabs of a pane's group. The index counts the tabs without the
    /// moved pane; a position past the end means last.
    Tab { pane: PaneId, index: usize },
    /// In another workspace, as the last tab beside its focused pane.
    Workspace(WorkspaceId),
}

/// All durable UI mutations use this path. Targets are captured when commands
/// are created rather than resolved against whichever pane is active later.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    AddWorkspace {
        cwd: PathBuf,
        name: String,
        group: Option<WorkspaceGroupId>,
        /// An SSH destination: every terminal of the workspace opens there.
        remote: Option<String>,
    },
    AddWorkspaceGroup {
        name: String,
    },
    RenameWorkspaceGroup {
        group: WorkspaceGroupId,
        name: String,
    },
    SetWorkspaceGroupCollapsed {
        group: WorkspaceGroupId,
        collapsed: bool,
    },
    /// Changes only the starting directory of future local workspaces.
    SetWorkspaceGroupDefaultDirectory {
        group: WorkspaceGroupId,
        directory: Option<PathBuf>,
    },
    SetWorkspaceGroup {
        workspace: WorkspaceId,
        group: Option<WorkspaceGroupId>,
    },
    /// Removes the folder, retaining its workspaces and every running session.
    RemoveWorkspaceGroup(WorkspaceGroupId),
    /// Moves a folder to `index` in the group order; a position past the end
    /// means last. Membership, order within each folder and sessions stay intact.
    MoveWorkspaceGroup {
        group: WorkspaceGroupId,
        index: usize,
    },
    /// Moves a group (with all its children) or ungrouped workspace to a
    /// top-level sidebar index. A position past the end means last.
    MoveSidebarItem {
        item: SidebarItem,
        index: usize,
    },
    SelectWorkspace(WorkspaceId),
    SplitPane {
        workspace: WorkspaceId,
        pane: PaneId,
        axis: Axis,
        cwd: PathBuf,
    },
    /// Opens a terminal as a tab following `pane`, in the same place.
    AddTab {
        workspace: WorkspaceId,
        pane: PaneId,
        cwd: PathBuf,
    },
    /// Opens a tab following `pane` in a git worktree made for an agent,
    /// and has its shell open `agent` there. The tab is named by the branch.
    OpenWorktree {
        workspace: WorkspaceId,
        pane: PaneId,
        worktree: crate::Worktree,
        agent: Option<crate::AgentKind>,
    },
    /// Focusing a tab that is out of view brings it into view.
    FocusPane {
        workspace: WorkspaceId,
        pane: PaneId,
    },
    /// Moving the last pane out of a workspace removes that workspace.
    MovePane {
        pane: PaneId,
        destination: Destination,
    },
    ClosePane(PaneId),
    /// The reverse of opening a background terminal: its tab goes and its
    /// session runs on. Refused where `Model::can_background` refuses.
    BackgroundPane(PaneId),
    CloseWorkspace(WorkspaceId),
    /// Moves a workspace to `index` in the ordered list. A position past the
    /// end means last.
    MoveWorkspace {
        workspace: WorkspaceId,
        index: usize,
    },
    RenameWorkspace {
        workspace: WorkspaceId,
        name: String,
    },
    /// Connect a workspace to an SSH destination, or return it to local
    /// shells with `None`. Every terminal it holds is replaced, because a
    /// running session cannot move to another machine.
    SetWorkspaceRemote {
        workspace: WorkspaceId,
        remote: Option<String>,
    },
    SetSplitRatio {
        split: SplitId,
        ratio: f32,
    },
    /// Give the places in the line this split divides the same share of it.
    EvenSplit(SplitId),
    /// Give the places of every line in a workspace the same share of it.
    EvenSplits(WorkspaceId),
    RestartPane(PaneId),
    SetSidebar(bool),
    /// The composition layer owns the typed configuration and supplies its
    /// immutable replacement to the preference writer when this effect runs.
    UpdatePreferences,
    AcknowledgeSave {
        generation: u64,
    },
    SessionStarted {
        pane: PaneId,
        generation: u64,
    },
    SessionFailed {
        pane: PaneId,
        generation: u64,
        error: String,
    },
    SessionExited {
        pane: PaneId,
        generation: u64,
    },
    PaneAgentChanged {
        pane: PaneId,
        generation: u64,
        agent: Option<crate::AgentSession>,
    },
    /// The agent in `parent` starts another in a tab that follows its own.
    /// Focus and the tab in view stay where they are.
    SpawnAgent {
        parent: PaneId,
        generation: u64,
        cwd: PathBuf,
    },
    /// Gives a local workspace its project. The key names what the desktop
    /// keeps for it; the lead is Claude Code or Codex.
    AddProject {
        workspace: WorkspaceId,
        name: String,
        key: ProjectKey,
        lead: crate::AgentKind,
    },
    RenameProject {
        project: ProjectId,
        name: String,
    },
    /// A paused project starts no agent. Those it has keep running.
    SetProjectPaused {
        project: ProjectId,
        paused: bool,
    },
    /// The project's agents keep running and get tabs; nothing is stopped.
    RemoveProject(ProjectId),
    /// The lead of `project` starts an agent in a terminal of the project's
    /// workspace that runs without a tab, as `SpawnAgent` does for an agent.
    /// The new terminal is the pane of the returned `Effect::StartSession`,
    /// and `Pane::project` names the project it was started for.
    SpawnProjectAgent {
        project: ProjectId,
        cwd: PathBuf,
        worktree: Option<crate::Worktree>,
    },
    /// The agent in `pane` leaves its project. Its terminal goes on as an
    /// ordinary one, with a tab if it had none; nothing is stopped.
    ReleaseProjectAgent {
        pane: PaneId,
        generation: u64,
    },
    /// An agent named a pull request it made or works on in this terminal.
    PanePullRequestLinked {
        pane: PaneId,
        generation: u64,
        pull_request: crate::PullRequest,
    },
    /// An agent attached a file to this terminal for the user to look at.
    PaneFileAttached {
        pane: PaneId,
        generation: u64,
        attachment: crate::Attachment,
    },
    /// The user takes one attached file off a terminal's list.
    RemoveAttachment {
        pane: PaneId,
        path: PathBuf,
    },
    /// The user empties a terminal's list of attached files.
    ClearAttachments {
        pane: PaneId,
    },
    PaneCwdChanged {
        pane: PaneId,
        generation: u64,
        cwd: PathBuf,
    },
    /// A directory explicitly reported by the remote shell, never a local path.
    PaneRemoteCwdChanged {
        pane: PaneId,
        generation: u64,
        cwd: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    StartSession {
        pane: PaneId,
        generation: u64,
        cwd: PathBuf,
        /// Where the session runs; `None` is a local shell.
        remote: Option<Remote>,
        remote_cwd: Option<PathBuf>,
        replacement: bool,
    },
    StopSession {
        pane: PaneId,
        generation: u64,
    },
    Focus {
        old: Option<PaneId>,
        new: Option<PaneId>,
    },
    ResetSearch,
    Persist {
        generation: u64,
    },
    SavePreferences,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    Started {
        pane: PaneId,
        generation: u64,
    },
    Failed {
        pane: PaneId,
        generation: u64,
        error: String,
    },
    Exited {
        pane: PaneId,
        generation: u64,
    },
    Saved {
        generation: u64,
    },
}

#[derive(Debug, Clone)]
pub struct Controller {
    model: Model,
    generation: u64,
    saved_generation: u64,
}

impl Controller {
    pub fn new(model: Model) -> Self {
        Self {
            model,
            generation: 0,
            saved_generation: 0,
        }
    }
    pub fn model(&self) -> &Model {
        &self.model
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn saved_generation(&self) -> u64 {
        self.saved_generation
    }
    pub fn is_dirty(&self) -> bool {
        self.saved_generation < self.generation
    }

    /// Restoration can populate panes immediately before bounded workers start
    /// their shells. It does not serialize or touch a filesystem.
    pub fn start_effects(&self) -> Vec<Effect> {
        self.model
            .workspaces
            .iter()
            .flat_map(|workspace| {
                workspace.panes.iter().map(|pane| Effect::StartSession {
                    pane: pane.id,
                    generation: pane.generation,
                    cwd: pane.cwd.clone(),
                    remote: workspace.remote.clone(),
                    remote_cwd: pane.remote_cwd.clone(),
                    replacement: false,
                })
            })
            .collect()
    }

    pub fn complete(&mut self, completion: Completion) -> Result<Vec<Effect>, Error> {
        self.dispatch(match completion {
            Completion::Started { pane, generation } => {
                Command::SessionStarted { pane, generation }
            }
            Completion::Failed {
                pane,
                generation,
                error,
            } => Command::SessionFailed {
                pane,
                generation,
                error,
            },
            Completion::Exited { pane, generation } => Command::SessionExited { pane, generation },
            Completion::Saved { generation } => Command::AcknowledgeSave { generation },
        })
    }

    pub fn dispatch(&mut self, command: Command) -> Result<Vec<Effect>, Error> {
        let old_focus = self.model.active_pane();
        let mut effects = Vec::new();
        let mut dirty = false;
        match command {
            Command::AddWorkspace {
                cwd,
                name,
                remote,
                group,
            } => {
                if name.trim().is_empty() {
                    return Err(Error::InvalidName);
                }
                let remote = remote.as_deref().map(Remote::parse).transpose()?;
                if let Some(group) = group
                    && self.model.group(group).is_none()
                {
                    return Err(Error::UnknownWorkspaceGroup(group));
                }
                if self.model.workspaces.len() >= self.model.limits.workspaces {
                    return Err(Error::WorkspaceLimit);
                }
                self.check_pane_capacity(0)?;
                let cwd = if remote.is_none() {
                    group
                        .and_then(|id| self.model.group(id))
                        .and_then(WorkspaceGroup::default_directory)
                        .map(PathBuf::from)
                        .unwrap_or(cwd)
                } else {
                    cwd
                };
                let id = WorkspaceId::new(self.model.next_workspace);
                let pane_id = PaneId::new(self.model.next_pane);
                let next_workspace = self
                    .model
                    .next_workspace
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?;
                let next_pane = self
                    .model
                    .next_pane
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?;
                self.model.next_workspace = next_workspace;
                self.model.next_pane = next_pane;
                self.model.workspaces.push(Workspace {
                    id,
                    group,
                    name,
                    cwd: cwd.clone(),
                    remote: remote.clone(),
                    panes: vec![Pane {
                        id: pane_id,
                        cwd: cwd.clone(),
                        remote_cwd: None,
                        agent: None,
                        pull_requests: Vec::new(),
                        attachments: Vec::new(),
                        spawned_by: None,
                        project: None,
                        worktree: None,
                        generation: 1,
                        lifecycle: Lifecycle::Starting,
                    }],
                    layout: Layout::pane(pane_id),
                    active: pane_id,
                });
                self.model.active = Some(id);
                effects.push(Effect::StartSession {
                    pane: pane_id,
                    generation: 1,
                    cwd,
                    remote,
                    remote_cwd: None,
                    replacement: false,
                });
                dirty = true;
            }
            Command::AddWorkspaceGroup { name } => {
                if name.trim().is_empty() {
                    return Err(Error::InvalidName);
                }
                if self.model.groups.len() >= self.model.limits.workspaces {
                    return Err(Error::WorkspaceGroupLimit);
                }
                let id = WorkspaceGroupId::new(self.model.next_group);
                self.model.next_group = self
                    .model
                    .next_group
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?;
                self.model.groups.push(WorkspaceGroup {
                    id,
                    name,
                    collapsed: false,
                    default_directory: None,
                });
                dirty = true;
            }
            Command::RenameWorkspaceGroup { group, name } => {
                if name.trim().is_empty() {
                    return Err(Error::InvalidName);
                }
                let folder = self.model.group_mut(group)?;
                if folder.name != name {
                    folder.name = name;
                    dirty = true;
                }
            }
            Command::SetWorkspaceGroupCollapsed { group, collapsed } => {
                let folder = self.model.group_mut(group)?;
                if folder.collapsed != collapsed {
                    folder.collapsed = collapsed;
                    dirty = true;
                }
            }
            Command::SetWorkspaceGroupDefaultDirectory { group, directory } => {
                crate::workspace::validate_default_directory(directory.as_deref())?;
                let folder = self.model.group_mut(group)?;
                if folder.default_directory != directory {
                    folder.default_directory = directory;
                    dirty = true;
                }
            }
            Command::SetWorkspaceGroup { workspace, group } => {
                if let Some(group) = group
                    && self.model.group(group).is_none()
                {
                    return Err(Error::UnknownWorkspaceGroup(group));
                }
                let ws = self.model.workspace_mut(workspace)?;
                if ws.group != group {
                    ws.group = group;
                    dirty = true;
                }
                if let Some(group) = group {
                    let folder = self.model.group_mut(group)?;
                    dirty |= folder.collapsed;
                    folder.collapsed = false;
                }
            }
            Command::RemoveWorkspaceGroup(group) => {
                let position = self
                    .model
                    .groups
                    .iter()
                    .position(|folder| folder.id == group)
                    .ok_or(Error::UnknownWorkspaceGroup(group))?;
                if let Some(index) = self
                    .model
                    .sidebar_order
                    .iter()
                    .position(|item| *item == SidebarItem::Group(group))
                {
                    let children = self
                        .model
                        .workspaces
                        .iter()
                        .filter(|w| w.group == Some(group))
                        .map(|w| SidebarItem::Workspace(w.id));
                    self.model.sidebar_order.splice(index..=index, children);
                }
                self.model.groups.remove(position);
                for workspace in &mut self.model.workspaces {
                    if workspace.group == Some(group) {
                        workspace.group = None;
                    }
                }
                dirty = true;
            }
            Command::MoveWorkspaceGroup { group, index } => {
                let position = self
                    .model
                    .groups
                    .iter()
                    .position(|folder| folder.id == group)
                    .ok_or(Error::UnknownWorkspaceGroup(group))?;
                let index = index.min(self.model.groups.len() - 1);
                if index != position {
                    let target = SidebarItem::Group(self.model.groups[index].id);
                    let target_index = self
                        .model
                        .sidebar_order
                        .iter()
                        .position(|item| *item == target)
                        .ok_or(Error::InvalidSidebarOrder)?;
                    self.move_sidebar_item(SidebarItem::Group(group), target_index)?;
                    dirty = true;
                }
            }
            Command::MoveSidebarItem { item, index } => {
                dirty = self.move_sidebar_item(item, index)?;
            }
            Command::SelectWorkspace(id) => {
                if self.model.workspace(id).is_none() {
                    return Err(Error::UnknownWorkspace(id));
                }
                if self.model.active != Some(id) {
                    self.model.active = Some(id);
                    dirty = true;
                }
                if let Some(group) = self.model.workspace(id).and_then(Workspace::group) {
                    let folder = self.model.group_mut(group)?;
                    dirty |= folder.collapsed;
                    folder.collapsed = false;
                }
            }
            Command::SplitPane {
                workspace,
                pane,
                axis,
                cwd,
            } => {
                effects.push(self.add_pane(workspace, pane, Some(axis), cwd)?);
                dirty = true;
            }
            Command::AddTab {
                workspace,
                pane,
                cwd,
            } => {
                effects.push(self.add_pane(workspace, pane, None, cwd)?);
                dirty = true;
            }
            Command::OpenWorktree {
                workspace,
                pane,
                worktree,
                agent,
            } => {
                // git ran on this machine, so the terminal has to as well.
                if !worktree.is_valid()
                    || self
                        .model
                        .workspace(workspace)
                        .ok_or(Error::UnknownWorkspace(workspace))?
                        .remote
                        .is_some()
                {
                    return Err(Error::InvalidWorktree);
                }
                effects.push(self.add_pane(workspace, pane, None, worktree.path.clone())?);
                let ws = self.model.workspace_mut(workspace)?;
                let id = ws.active;
                if let Some(item) = ws.panes.iter_mut().find(|item| item.id == id) {
                    // Without a conversation yet: the shell opens the CLI anew.
                    item.agent = agent.map(|kind| crate::AgentSession {
                        kind,
                        session_id: None,
                        cwd: worktree.path.clone(),
                    });
                    item.worktree = Some(worktree);
                }
                dirty = true;
            }
            Command::FocusPane { workspace, pane } => {
                let was_active = self.model.active == Some(workspace);
                let ws = self.model.workspace_mut(workspace)?;
                if ws.pane(pane).is_none() {
                    return Err(Error::UnknownPane(pane));
                }
                // Opening a background terminal gives it a tab.
                dirty = ws.reveal(pane) || ws.active != pane || !was_active;
                ws.active = pane;
                ws.layout.show(pane);
                self.model.active = Some(workspace);
            }
            Command::MovePane { pane, destination } => dirty = self.move_pane(pane, destination)?,
            Command::ClosePane(pane) => {
                let workspace = self
                    .model
                    .workspace_for_pane(pane)
                    .ok_or(Error::UnknownPane(pane))?;
                let ws = self.model.workspace_mut(workspace)?;
                if ws.panes.len() == 1 {
                    self.close_workspace(workspace, &mut effects)?;
                } else {
                    let position = ws
                        .panes
                        .iter()
                        .position(|item| item.id == pane)
                        .ok_or(Error::UnknownPane(pane))?;
                    let removed = ws.panes.remove(position);
                    ws.remove_from_layout(pane)?;
                    effects.push(Effect::StopSession {
                        pane,
                        generation: removed.generation,
                    });
                }
                dirty = true;
            }
            Command::BackgroundPane(pane) => {
                self.model.can_background(pane)?;
                let workspace = self
                    .model
                    .workspace_for_pane(pane)
                    .ok_or(Error::UnknownPane(pane))?;
                self.model
                    .workspace_mut(workspace)?
                    .remove_from_layout(pane)?;
                dirty = true;
            }
            Command::CloseWorkspace(workspace) => {
                self.close_workspace(workspace, &mut effects)?;
                dirty = true;
            }
            Command::MoveWorkspace { workspace, index } => {
                let position = self
                    .model
                    .workspaces
                    .iter()
                    .position(|item| item.id == workspace)
                    .ok_or(Error::UnknownWorkspace(workspace))?;
                let index = index.min(self.model.workspaces.len() - 1);
                if index != position {
                    if self.model.workspaces[position].group.is_none() {
                        let target = &self.model.workspaces[index];
                        let target = target
                            .group
                            .map_or(SidebarItem::Workspace(target.id), SidebarItem::Group);
                        let target_index = self
                            .model
                            .sidebar_order
                            .iter()
                            .position(|item| *item == target)
                            .ok_or(Error::InvalidSidebarOrder)?;
                        self.move_sidebar_item(SidebarItem::Workspace(workspace), target_index)?;
                    }
                    let moved = self.model.workspaces.remove(position);
                    self.model.workspaces.insert(index, moved);
                    dirty = true;
                }
            }
            Command::RenameWorkspace { workspace, name } => {
                if name.trim().is_empty() {
                    return Err(Error::InvalidName);
                }
                let ws = self.model.workspace_mut(workspace)?;
                if ws.name != name {
                    ws.name = name;
                    dirty = true;
                }
            }
            Command::SetWorkspaceRemote { workspace, remote } => {
                let remote = remote.as_deref().map(Remote::parse).transpose()?;
                // A project and its agents run on this machine.
                let project = remote.is_some() && self.model.project_of(workspace).is_some();
                let ws = self.model.workspace_mut(workspace)?;
                if project {
                    return Err(Error::InvalidProject);
                }
                if ws.remote != remote {
                    if ws
                        .panes
                        .iter()
                        .any(|pane| pane.generation.checked_add(1).is_none())
                    {
                        return Err(Error::IdentityExhausted);
                    }
                    ws.remote.clone_from(&remote);
                    for pane in &mut ws.panes {
                        let previous = pane.generation;
                        pane.generation += 1;
                        pane.lifecycle = Lifecycle::Starting;
                        pane.remote_cwd = None;
                        pane.agent = None;
                        pane.pull_requests.clear();
                        pane.attachments.clear();
                        pane.spawned_by = None;
                        pane.worktree = None;
                        effects.push(Effect::StopSession {
                            pane: pane.id,
                            generation: previous,
                        });
                        effects.push(Effect::StartSession {
                            pane: pane.id,
                            generation: pane.generation,
                            cwd: pane.cwd.clone(),
                            remote: remote.clone(),
                            remote_cwd: None,
                            replacement: true,
                        });
                    }
                    if old_focus.is_some_and(|pane| ws.pane(pane).is_some()) {
                        effects.push(Effect::ResetSearch);
                    }
                    dirty = true;
                }
            }
            Command::SetSplitRatio { split, ratio } => {
                if !ratio.is_finite() || !(0.1..=0.9).contains(&ratio) {
                    return Err(Error::InvalidRatio);
                }
                dirty = self
                    .model
                    .workspaces
                    .iter_mut()
                    .find_map(|workspace| workspace.layout.set_ratio(split, ratio))
                    .ok_or(Error::UnknownSplit(split))?;
            }
            Command::EvenSplit(split) => {
                dirty = self
                    .model
                    .workspaces
                    .iter_mut()
                    .find_map(|workspace| workspace.layout.even_line(split))
                    .ok_or(Error::UnknownSplit(split))?;
            }
            Command::EvenSplits(workspace) => {
                dirty = self.model.workspace_mut(workspace)?.layout.even(true);
            }
            Command::RestartPane(pane) => {
                let remote = self
                    .model
                    .workspace_for_pane(pane)
                    .and_then(|id| self.model.workspace(id))
                    .and_then(|workspace| workspace.remote.clone());
                let item = self.model.pane_mut(pane)?;
                let previous = item.generation;
                item.generation = item
                    .generation
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?;
                item.lifecycle = Lifecycle::Starting;
                dirty |= item.agent.take().is_some();
                dirty |= item.spawned_by.take().is_some();
                dirty |= item.project.take().is_some();
                item.pull_requests.clear();
                item.attachments.clear();
                effects.push(Effect::StopSession {
                    pane,
                    generation: previous,
                });
                effects.push(Effect::StartSession {
                    pane,
                    generation: item.generation,
                    cwd: item.cwd.clone(),
                    remote,
                    remote_cwd: item.remote_cwd.clone(),
                    replacement: true,
                });
                if old_focus == Some(pane) {
                    effects.push(Effect::ResetSearch);
                }
            }
            Command::SetSidebar(sidebar) => {
                if self.model.sidebar != sidebar {
                    self.model.sidebar = sidebar;
                    dirty = true;
                }
            }
            Command::UpdatePreferences => effects.push(Effect::SavePreferences),
            Command::AcknowledgeSave { generation } => {
                if generation <= self.generation {
                    self.saved_generation = self.saved_generation.max(generation);
                }
            }
            Command::SessionStarted { pane, generation } => {
                self.lifecycle(pane, generation, Lifecycle::Running)
            }
            Command::SessionFailed {
                pane,
                generation,
                error,
            } => {
                self.lifecycle(pane, generation, Lifecycle::Failed(error));
                if let Ok(item) = self.model.pane_mut(pane)
                    && item.generation == generation
                {
                    dirty |= item.spawned_by.take().is_some();
                    dirty |= item.project.take().is_some();
                    self.close_background(pane, &mut effects);
                }
            }
            Command::SessionExited { pane, generation } => {
                self.lifecycle(pane, generation, Lifecycle::Exited);
                if let Ok(item) = self.model.pane_mut(pane)
                    && item.generation == generation
                {
                    dirty |= item.agent.take().is_some();
                    dirty |= item.spawned_by.take().is_some();
                    dirty |= item.project.take().is_some();
                    item.pull_requests.clear();
                    item.attachments.clear();
                    self.close_background(pane, &mut effects);
                }
            }
            Command::PaneAgentChanged {
                pane,
                generation,
                agent,
            } => {
                let local = self
                    .model
                    .workspace_for_pane(pane)
                    .and_then(|id| self.model.workspace(id))
                    .is_some_and(|ws| ws.remote.is_none());
                if local
                    && agent.as_ref().is_none_or(crate::AgentSession::is_valid)
                    && let Ok(item) = self.model.pane_mut(pane)
                    && item.generation == generation
                    && matches!(item.lifecycle, Lifecycle::Starting | Lifecycle::Running)
                {
                    // Links belong to the agent's run; they leave with it. An
                    // agent that never opened leaves the one that started it too.
                    if agent.is_none() {
                        item.pull_requests.clear();
                        item.attachments.clear();
                        dirty |= item.spawned_by.take().is_some();
                        dirty |= item.project.take().is_some();
                    }
                    let left = agent.is_none();
                    if item.agent != agent {
                        item.agent = agent;
                        dirty = true;
                    }
                    if left {
                        dirty |= self.close_background(pane, &mut effects);
                    }
                }
            }
            Command::SpawnAgent {
                parent,
                generation,
                cwd,
            } => {
                let workspace = self
                    .model
                    .workspace_for_pane(parent)
                    .ok_or(Error::UnknownPane(parent))?;
                let ws = self
                    .model
                    .workspace(workspace)
                    .ok_or(Error::UnknownPane(parent))?;
                let item = ws.pane(parent).ok_or(Error::UnknownPane(parent))?;
                // Only the agent still open in a local terminal starts another.
                if ws.remote.is_some()
                    || item.generation != generation
                    || item.agent.is_none()
                    || !matches!(item.lifecycle, Lifecycle::Starting | Lifecycle::Running)
                {
                    return Err(Error::UnknownPane(parent));
                }
                if self.model.spawned(parent).count() >= crate::AgentSession::MAX_SPAWNED {
                    return Err(Error::SpawnLimit);
                }
                if self.model.spawn_depth(parent) >= crate::AgentSession::MAX_SPAWN_DEPTH {
                    return Err(Error::SpawnDepth);
                }
                // It runs out of view: no tab until someone opens it.
                self.check_pane_capacity(ws.panes.len())?;
                let id = PaneId::new(self.model.next_pane);
                self.model.next_pane = self
                    .model
                    .next_pane
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?;
                let ws = self.model.workspace_mut(workspace)?;
                ws.panes.push(Pane {
                    id,
                    cwd: cwd.clone(),
                    remote_cwd: None,
                    agent: None,
                    pull_requests: Vec::new(),
                    attachments: Vec::new(),
                    spawned_by: Some(parent),
                    project: None,
                    worktree: None,
                    generation: 1,
                    lifecycle: Lifecycle::Starting,
                });
                effects.push(Effect::StartSession {
                    pane: id,
                    generation: 1,
                    cwd,
                    remote: None,
                    remote_cwd: None,
                    replacement: false,
                });
                dirty = true;
            }
            Command::AddProject {
                workspace,
                name,
                key,
                lead,
            } => {
                Project::validate(&name, lead)?;
                let ws = self
                    .model
                    .workspace(workspace)
                    .ok_or(Error::UnknownWorkspace(workspace))?;
                if ws.remote.is_some() {
                    return Err(Error::InvalidProject);
                }
                if self.model.project_of(workspace).is_some() {
                    return Err(Error::ProjectExists);
                }
                // What a key names belongs to one project.
                if self.model.projects.iter().any(|project| project.key == key) {
                    return Err(Error::InvalidIdentity);
                }
                if self.model.projects.len() >= Project::MAX {
                    return Err(Error::ProjectLimit);
                }
                let id = ProjectId::new(self.model.next_project);
                self.model.next_project = self
                    .model
                    .next_project
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?;
                self.model.projects.push(Project {
                    id,
                    key,
                    name,
                    workspace,
                    lead,
                    paused: false,
                });
                dirty = true;
            }
            Command::RenameProject { project, name } => {
                let item = self.project_mut(project)?;
                Project::validate(&name, item.lead)?;
                if item.name != name {
                    item.name = name;
                    dirty = true;
                }
            }
            Command::SetProjectPaused { project, paused } => {
                let item = self.project_mut(project)?;
                if item.paused != paused {
                    item.paused = paused;
                    dirty = true;
                }
            }
            Command::RemoveProject(project) => {
                let position = self
                    .model
                    .projects
                    .iter()
                    .position(|item| item.id == project)
                    .ok_or(Error::UnknownProject(project))?;
                // Its agents lose their link and get tabs below.
                self.model.projects.remove(position);
                dirty = true;
            }
            Command::SpawnProjectAgent {
                project,
                cwd,
                worktree,
            } => {
                let item = self
                    .model
                    .project(project)
                    .ok_or(Error::UnknownProject(project))?;
                if item.paused {
                    return Err(Error::ProjectPaused);
                }
                let workspace = item.workspace;
                let ws = self
                    .model
                    .workspace(workspace)
                    .ok_or(Error::UnknownWorkspace(workspace))?;
                if ws.remote.is_some() {
                    return Err(Error::InvalidProject);
                }
                if worktree
                    .as_ref()
                    .is_some_and(|worktree| !worktree.is_valid())
                {
                    return Err(Error::InvalidWorktree);
                }
                if self.model.project_agents(project).count() >= Project::MAX_AGENTS {
                    return Err(Error::ProjectAgentLimit);
                }
                // It runs out of view: no tab until someone opens it.
                self.check_pane_capacity(ws.panes.len())?;
                let id = PaneId::new(self.model.next_pane);
                self.model.next_pane = self
                    .model
                    .next_pane
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?;
                let ws = self.model.workspace_mut(workspace)?;
                ws.panes.push(Pane {
                    id,
                    cwd: cwd.clone(),
                    remote_cwd: None,
                    agent: None,
                    pull_requests: Vec::new(),
                    attachments: Vec::new(),
                    spawned_by: None,
                    project: Some(project),
                    worktree,
                    generation: 1,
                    lifecycle: Lifecycle::Starting,
                });
                effects.push(Effect::StartSession {
                    pane: id,
                    generation: 1,
                    cwd,
                    remote: None,
                    remote_cwd: None,
                    replacement: false,
                });
                dirty = true;
            }
            Command::ReleaseProjectAgent { pane, generation } => {
                // The tab it may lack is given below, with every other
                // terminal nothing answers for.
                if let Ok(item) = self.model.pane_mut(pane)
                    && item.generation == generation
                {
                    dirty |= item.project.take().is_some();
                }
            }
            Command::PanePullRequestLinked {
                pane,
                generation,
                pull_request,
            } => {
                if let Ok(item) = self.model.pane_mut(pane)
                    && item.generation == generation
                    && matches!(item.lifecycle, Lifecycle::Starting | Lifecycle::Running)
                    && item.agent.is_some()
                    && !item
                        .pull_requests
                        .iter()
                        .any(|link| link.same(&pull_request))
                {
                    if item.pull_requests.len() >= crate::PullRequest::MAX_PER_PANE {
                        item.pull_requests.remove(0);
                    }
                    item.pull_requests.push(pull_request);
                    dirty = true;
                }
            }
            Command::PaneFileAttached {
                pane,
                generation,
                attachment,
            } => {
                if let Ok(item) = self.model.pane_mut(pane)
                    && item.generation == generation
                    && matches!(item.lifecycle, Lifecycle::Starting | Lifecycle::Running)
                    && item.agent.is_some()
                    && item.attachments.last() != Some(&attachment)
                {
                    // The same file again is the newest, under its new title.
                    item.attachments
                        .retain(|kept| kept.path() != attachment.path());
                    if item.attachments.len() >= crate::Attachment::MAX_PER_PANE {
                        item.attachments.remove(0);
                    }
                    item.attachments.push(attachment);
                    dirty = true;
                }
            }
            Command::RemoveAttachment { pane, path } => {
                if let Ok(item) = self.model.pane_mut(pane) {
                    let kept = item.attachments.len();
                    item.attachments.retain(|kept| kept.path() != path);
                    dirty |= item.attachments.len() != kept;
                }
            }
            Command::ClearAttachments { pane } => {
                if let Ok(item) = self.model.pane_mut(pane)
                    && !item.attachments.is_empty()
                {
                    item.attachments.clear();
                    dirty = true;
                }
            }
            Command::PaneCwdChanged {
                pane,
                generation,
                cwd,
            } => {
                if let Ok(item) = self.model.pane_mut(pane)
                    && item.generation == generation
                    && item.cwd != cwd
                {
                    item.cwd = cwd;
                    dirty = true;
                }
            }
            Command::PaneRemoteCwdChanged {
                pane,
                generation,
                cwd,
            } => {
                let remote = self
                    .model
                    .workspace_for_pane(pane)
                    .and_then(|id| self.model.workspace(id))
                    .is_some_and(|workspace| workspace.remote.is_some());
                if remote
                    && let Ok(item) = self.model.pane_mut(pane)
                    && item.generation == generation
                    && item.remote_cwd.as_ref() != Some(&cwd)
                {
                    item.remote_cwd = Some(cwd);
                    dirty = true;
                }
            }
        }
        dirty |= self.model.release_spawned();
        if dirty {
            self.model.order_workspaces();
        }
        let new_focus = self.model.active_pane();
        if old_focus != new_focus {
            if let Some(group) = self
                .model
                .active
                .and_then(|id| self.model.workspace(id))
                .and_then(Workspace::group)
            {
                let folder = self.model.group_mut(group)?;
                dirty |= folder.collapsed;
                folder.collapsed = false;
            }
            effects.push(Effect::Focus {
                old: old_focus,
                new: new_focus,
            });
            effects.push(Effect::ResetSearch);
        }
        if dirty {
            self.generation = self.generation.saturating_add(1);
            effects.push(Effect::Persist {
                generation: self.generation,
            });
        }
        Ok(effects)
    }

    fn project_mut(&mut self, id: ProjectId) -> Result<&mut Project, Error> {
        self.model
            .projects
            .iter_mut()
            .find(|project| project.id == id)
            .ok_or(Error::UnknownProject(id))
    }

    fn check_pane_capacity(&self, count: usize) -> Result<(), Error> {
        if count >= self.model.limits.panes_per_workspace {
            return Err(Error::PaneLimit);
        }
        if self.model.pane_count() >= self.model.limits.total_panes {
            return Err(Error::TotalPaneLimit);
        }
        Ok(())
    }

    /// Opens a pane beside `pane`: across a split of its tab group, or as the
    /// tab that follows it.
    fn add_pane(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        axis: Option<Axis>,
        cwd: PathBuf,
    ) -> Result<Effect, Error> {
        let ws = self
            .model
            .workspace(workspace)
            .ok_or(Error::UnknownWorkspace(workspace))?;
        let remote_cwd = ws
            .pane(pane)
            .ok_or(Error::UnknownPane(pane))?
            .remote_cwd
            .clone();
        self.check_pane_capacity(ws.panes.len())?;
        let id = PaneId::new(self.model.next_pane);
        let split = SplitId::new(self.model.next_split);
        let next_pane = self
            .model
            .next_pane
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        let next_split = self
            .model
            .next_split
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        let ws = self.model.workspace_mut(workspace)?;
        let placed = match axis {
            Some(Axis::Vertical) => ws.layout.split(pane, id, split, Edge::Right),
            Some(Axis::Horizontal) => ws.layout.split(pane, id, split, Edge::Bottom),
            None => ws.layout.add_tab(pane, id, None),
        };
        if !placed {
            return Err(Error::UnknownPane(pane));
        }
        ws.panes.push(Pane {
            id,
            cwd: cwd.clone(),
            remote_cwd: remote_cwd.clone(),
            agent: None,
            pull_requests: Vec::new(),
            attachments: Vec::new(),
            spawned_by: None,
            project: None,
            worktree: None,
            generation: 1,
            lifecycle: Lifecycle::Starting,
        });
        ws.active = id;
        let remote = ws.remote.clone();
        self.model.active = Some(workspace);
        self.model.next_pane = next_pane;
        if axis.is_some() {
            self.model.next_split = next_split;
        }
        Ok(Effect::StartSession {
            pane: id,
            generation: 1,
            cwd,
            remote,
            remote_cwd,
            replacement: false,
        })
    }

    /// Reports whether anything changed. Sessions are untouched: a pane's
    /// identity and generation do not depend on where it is shown.
    fn move_pane(&mut self, pane: PaneId, destination: Destination) -> Result<bool, Error> {
        let source = self
            .model
            .workspace_for_pane(pane)
            .ok_or(Error::UnknownPane(pane))?;
        let (target, edge, index) = match destination {
            Destination::Beside { pane: target, edge } => (target, Some(edge), None),
            Destination::Tab {
                pane: target,
                index,
            } => (target, None, Some(index)),
            Destination::Workspace(workspace) => {
                let ws = self
                    .model
                    .workspace(workspace)
                    .ok_or(Error::UnknownWorkspace(workspace))?;
                if workspace == source {
                    return Ok(false);
                }
                (ws.active, None, Some(usize::MAX))
            }
        };
        let destination = self
            .model
            .workspace_for_pane(target)
            .ok_or(Error::UnknownPane(target))?;
        // Nothing is placed against a terminal that has no place itself, and
        // one that is moved has a tab from then on.
        if self
            .model
            .workspace(destination)
            .is_some_and(|ws| ws.is_background(target))
        {
            return Err(Error::UnknownPane(target));
        }
        if destination != source {
            // A local shell would be shown as running on the host, and a
            // connection as a local shell, until the next restart.
            if self.model.workspace(source).map(|ws| &ws.remote)
                != self.model.workspace(destination).map(|ws| &ws.remote)
            {
                return Err(Error::RemoteMismatch);
            }
            let limit = self.model.limits.panes_per_workspace;
            if self
                .model
                .workspace(destination)
                .is_some_and(|ws| ws.panes.len() >= limit)
            {
                return Err(Error::PaneLimit);
            }
        }
        // Refused moves leave a background terminal where it was.
        let split = SplitId::new(self.model.next_split);
        let next_split = self
            .model
            .next_split
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        self.model.workspace_mut(source)?.reveal(pane);
        // A pane placed relative to itself is placed relative to the tabs it
        // shares a place with. Alone there, it is already where it would land.
        let target = if target == pane {
            let ws = self.model.workspace_mut(source)?;
            let sibling = ws
                .layout
                .tabs(pane)
                .and_then(|(tabs, _)| tabs.iter().copied().find(|tab| *tab != pane));
            let Some(sibling) = sibling else {
                let changed = ws.active != pane;
                ws.active = pane;
                return Ok(changed);
            };
            sibling
        } else {
            target
        };
        let place = |layout: &mut Layout| match edge {
            Some(edge) => layout.split(target, pane, split, edge),
            None => layout.add_tab(target, pane, index),
        };
        if destination == source {
            let ws = self.model.workspace_mut(source)?;
            // The target is another pane, so a leaf remains.
            let mut layout = ws
                .layout
                .clone()
                .remove(pane)
                .ok_or(Error::InvalidLayout("move removed every leaf"))?;
            place(&mut layout);
            if layout.same_arrangement(&ws.layout) {
                // Dropped where it already was: keep the split and its ratio.
                let changed = ws.active != pane;
                ws.active = pane;
                ws.layout.show(pane);
                return Ok(changed);
            }
            ws.layout = layout;
            ws.active = pane;
        } else {
            let ws = self.model.workspace_mut(source)?;
            let position = ws
                .panes
                .iter()
                .position(|item| item.id == pane)
                .ok_or(Error::UnknownPane(pane))?;
            let moved = ws.panes.remove(position);
            if ws.panes.is_empty() {
                self.model.workspaces.retain(|item| item.id != source);
                if self.model.active == Some(source) {
                    self.model.active = Some(destination);
                }
            } else {
                ws.remove_from_layout(pane)?;
            }
            let ws = self.model.workspace_mut(destination)?;
            place(&mut ws.layout);
            ws.panes.push(moved);
            ws.active = pane;
        }
        if edge.is_some() {
            self.model.next_split = next_split;
        }
        Ok(true)
    }

    fn move_sidebar_item(&mut self, item: SidebarItem, index: usize) -> Result<bool, Error> {
        match item {
            SidebarItem::Workspace(id) => {
                let workspace = self
                    .model
                    .workspace(id)
                    .ok_or(Error::UnknownWorkspace(id))?;
                if workspace.group().is_some() {
                    return Err(Error::InvalidSidebarOrder);
                }
            }
            SidebarItem::Group(id) => {
                self.model
                    .group(id)
                    .ok_or(Error::UnknownWorkspaceGroup(id))?;
            }
        }
        let position = self
            .model
            .sidebar_order
            .iter()
            .position(|entry| *entry == item)
            .ok_or(Error::InvalidSidebarOrder)?;
        let index = index.min(self.model.sidebar_order.len() - 1);
        if position == index {
            return Ok(false);
        }
        self.model.sidebar_order.remove(position);
        self.model.sidebar_order.insert(index, item);
        Ok(true)
    }

    fn close_workspace(
        &mut self,
        workspace: WorkspaceId,
        effects: &mut Vec<Effect>,
    ) -> Result<(), Error> {
        let position = self
            .model
            .workspaces
            .iter()
            .position(|item| item.id == workspace)
            .ok_or(Error::UnknownWorkspace(workspace))?;
        let removed = self.model.workspaces.remove(position);
        for pane in removed.panes {
            effects.push(Effect::StopSession {
                pane: pane.id,
                generation: pane.generation,
            });
        }
        if self.model.active == Some(workspace) {
            self.model.active = self
                .model
                .workspaces
                .get(position.min(self.model.workspaces.len().saturating_sub(1)))
                .map(Workspace::id);
        }
        Ok(())
    }

    /// Closes a terminal that ran without a tab once the agent it was opened
    /// for is gone: a shell nobody has in view is nobody's to keep.
    fn close_background(&mut self, pane: PaneId, effects: &mut Vec<Effect>) -> bool {
        let Some(ws) = self
            .model
            .workspace_for_pane(pane)
            .and_then(|id| self.model.workspace_mut(id).ok())
            .filter(|ws| ws.is_background(pane))
        else {
            return false;
        };
        let Some(position) = ws.panes.iter().position(|item| item.id == pane) else {
            return false;
        };
        let removed = ws.panes.remove(position);
        effects.push(Effect::StopSession {
            pane,
            generation: removed.generation,
        });
        true
    }

    fn lifecycle(&mut self, pane: PaneId, generation: u64, lifecycle: Lifecycle) {
        // A closed pane or a previous replacement cannot publish into a new one.
        if let Ok(item) = self.model.pane_mut(pane)
            && item.generation == generation
            && matches!(
                (&item.lifecycle, &lifecycle),
                (
                    Lifecycle::Starting,
                    Lifecycle::Running | Lifecycle::Failed(_)
                ) | (Lifecycle::Running, Lifecycle::Exited)
            )
        {
            item.lifecycle = lifecycle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Limits, PaneSpec, WorkspaceSpec};

    #[derive(Default)]
    struct FakeRuntime {
        effects: Vec<Effect>,
    }
    impl FakeRuntime {
        fn run(&mut self, controller: &mut Controller, command: Command) -> Vec<Effect> {
            let effects = controller.dispatch(command).unwrap();
            self.effects.extend(effects.clone());
            for effect in &effects {
                if let Effect::StartSession {
                    pane, generation, ..
                } = effect
                {
                    controller
                        .complete(Completion::Started {
                            pane: *pane,
                            generation: *generation,
                        })
                        .unwrap();
                }
            }
            effects
        }
    }

    fn create(controller: &mut Controller, name: &str) -> WorkspaceId {
        controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: PathBuf::from("/fake"),
                name: name.into(),
                remote: None,
            })
            .unwrap();
        controller.model().active_workspace().unwrap()
    }
    fn setup() -> (Controller, WorkspaceId, PaneId) {
        let mut controller = Controller::new(Model::default());
        let workspace = create(&mut controller, "main");
        let pane = controller.model().active_pane().unwrap();
        (controller, workspace, pane)
    }

    #[test]
    fn selection_and_focus_share_reports_search_reset_and_dirty_policy() {
        let (mut controller, first, first_pane) = setup();
        let second = create(&mut controller, "second");
        let second_pane = controller.model().active_pane().unwrap();
        let sidebar = controller
            .dispatch(Command::SelectWorkspace(first))
            .unwrap();
        assert!(sidebar.contains(&Effect::Focus {
            old: Some(second_pane),
            new: Some(first_pane)
        }));
        assert!(sidebar.contains(&Effect::ResetSearch));
        controller
            .dispatch(Command::SelectWorkspace(second))
            .unwrap();
        let direct = controller
            .dispatch(Command::FocusPane {
                workspace: first,
                pane: first_pane,
            })
            .unwrap();
        let without_save = |effects: Vec<Effect>| {
            effects
                .into_iter()
                .filter(|effect| !matches!(effect, Effect::Persist { .. }))
                .collect::<Vec<_>>()
        };
        assert_eq!(without_save(sidebar), without_save(direct));
    }

    #[test]
    fn queued_split_targets_original_workspace_after_selection_changes() {
        let (mut controller, workspace, pane) = setup();
        create(&mut controller, "another");
        controller
            .dispatch(Command::SplitPane {
                workspace,
                pane,
                axis: Axis::Horizontal,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        assert_eq!(
            controller
                .model()
                .workspace(workspace)
                .unwrap()
                .panes()
                .len(),
            2
        );
        assert_eq!(controller.model().workspaces()[1].panes().len(), 1);
    }

    #[test]
    fn closing_before_active_workspace_preserves_identity() {
        let (mut controller, first, _) = setup();
        let second = create(&mut controller, "second");
        create(&mut controller, "third");
        controller
            .dispatch(Command::SelectWorkspace(second))
            .unwrap();
        controller.dispatch(Command::CloseWorkspace(first)).unwrap();
        assert_eq!(controller.model().active_workspace(), Some(second));
    }

    #[test]
    fn closing_active_workspace_focuses_successor_then_predecessor() {
        let (mut controller, first, _) = setup();
        let second = create(&mut controller, "second");
        let third = create(&mut controller, "third");
        controller
            .dispatch(Command::SelectWorkspace(second))
            .unwrap();
        controller
            .dispatch(Command::CloseWorkspace(second))
            .unwrap();
        assert_eq!(controller.model().active_workspace(), Some(third));
        controller.dispatch(Command::CloseWorkspace(third)).unwrap();
        assert_eq!(controller.model().active_workspace(), Some(first));
    }

    #[test]
    fn closing_after_active_workspace_preserves_focus() {
        let (mut controller, first, pane) = setup();
        let second = create(&mut controller, "second");
        controller
            .dispatch(Command::SelectWorkspace(first))
            .unwrap();
        let effects = controller
            .dispatch(Command::CloseWorkspace(second))
            .unwrap();
        assert_eq!(controller.model().active_pane(), Some(pane));
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Focus { .. }))
        );
    }

    #[test]
    fn moving_a_workspace_reorders_without_changing_identity_or_focus() {
        let (mut controller, first, _) = setup();
        let second = create(&mut controller, "second");
        let third = create(&mut controller, "third");
        controller
            .dispatch(Command::SelectWorkspace(second))
            .unwrap();
        let pane = controller.model().active_pane();
        let order = |controller: &Controller| {
            controller
                .model()
                .workspaces()
                .iter()
                .map(Workspace::id)
                .collect::<Vec<_>>()
        };
        let generation = controller.generation();
        let effects = controller
            .dispatch(Command::MoveWorkspace {
                workspace: first,
                index: 2,
            })
            .unwrap();
        assert_eq!(order(&controller), [second, third, first]);
        assert_eq!(
            effects,
            [Effect::Persist {
                generation: generation + 1
            }],
            "sessions, focus and search are untouched"
        );
        assert_eq!(controller.model().active_workspace(), Some(second));
        assert_eq!(controller.model().active_pane(), pane);
        // A position past the end means last.
        controller
            .dispatch(Command::MoveWorkspace {
                workspace: second,
                index: usize::MAX,
            })
            .unwrap();
        assert_eq!(order(&controller), [third, first, second]);
        controller
            .dispatch(Command::MoveWorkspace {
                workspace: second,
                index: 0,
            })
            .unwrap();
        assert_eq!(order(&controller), [second, third, first]);
    }

    #[test]
    fn moving_to_the_same_position_or_an_unknown_workspace_changes_nothing() {
        let (mut controller, first, _) = setup();
        create(&mut controller, "second");
        let before = controller.model().clone();
        let generation = controller.generation();
        assert!(
            controller
                .dispatch(Command::MoveWorkspace {
                    workspace: first,
                    index: 0
                })
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            controller.dispatch(Command::MoveWorkspace {
                workspace: WorkspaceId::new(99),
                index: 0
            }),
            Err(Error::UnknownWorkspace(WorkspaceId::new(99)))
        );
        assert_eq!(controller.model(), &before);
        assert_eq!(controller.generation(), generation);
    }

    #[test]
    fn close_pane_collapses_layout_and_keeps_unrelated_split_identity() {
        let (mut controller, workspace, first) = setup();
        controller
            .dispatch(Command::SplitPane {
                workspace,
                pane: first,
                axis: Axis::Vertical,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        let second = controller.model().active_pane().unwrap();
        let outer = match controller.model().workspace(workspace).unwrap().layout() {
            Layout::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        controller
            .dispatch(Command::SplitPane {
                workspace,
                pane: second,
                axis: Axis::Horizontal,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        let third = controller.model().active_pane().unwrap();
        controller.dispatch(Command::ClosePane(third)).unwrap();
        assert_eq!(
            controller
                .model()
                .workspace(workspace)
                .unwrap()
                .layout()
                .panes(),
            vec![first, second]
        );
        assert!(
            matches!(controller.model().workspace(workspace).unwrap().layout(), Layout::Split { id, .. } if *id == outer)
        );
    }

    fn split(controller: &mut Controller, pane: PaneId, axis: Axis) -> PaneId {
        let workspace = controller.model().workspace_for_pane(pane).unwrap();
        controller
            .dispatch(Command::SplitPane {
                workspace,
                pane,
                axis,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        controller.model().workspace(workspace).unwrap().active()
    }
    fn split_ids(layout: &Layout) -> Vec<SplitId> {
        match layout {
            Layout::Tabs { .. } => Vec::new(),
            Layout::Split {
                id, first, second, ..
            } => [vec![*id], split_ids(first), split_ids(second)].concat(),
        }
    }
    fn without_save(effects: Vec<Effect>) -> Vec<Effect> {
        effects
            .into_iter()
            .filter(|effect| !matches!(effect, Effect::Persist { .. }))
            .collect()
    }

    #[test]
    fn moving_a_pane_rearranges_the_layout_without_touching_sessions() {
        let (mut controller, workspace, first) = setup();
        let second = split(&mut controller, first, Axis::Vertical);
        let third = split(&mut controller, second, Axis::Horizontal);
        let inner = split_ids(controller.model().workspace(workspace).unwrap().layout())[1];
        let effects = controller
            .dispatch(Command::MovePane {
                pane: first,
                destination: Destination::Beside {
                    pane: third,
                    edge: Edge::Bottom,
                },
            })
            .unwrap();
        // The moved pane takes focus; no session starts or stops.
        assert_eq!(
            without_save(effects),
            [
                Effect::Focus {
                    old: Some(third),
                    new: Some(first)
                },
                Effect::ResetSearch
            ]
        );
        let ws = controller.model().workspace(workspace).unwrap();
        assert_eq!(ws.layout().panes(), [second, third, first]);
        assert_eq!(ws.panes().len(), 3);
        assert_eq!(ws.pane(first).unwrap().generation(), 1);
        assert!(
            split_ids(ws.layout()).contains(&inner),
            "unrelated split kept"
        );
        assert!(matches!(
            ws.layout(),
            Layout::Split { axis: Axis::Horizontal, second: below, .. }
                if matches!(&**below, Layout::Split { axis: Axis::Horizontal, .. })
        ));
    }

    #[test]
    fn moving_a_pane_to_where_it_already_is_keeps_the_split_and_its_ratio() {
        let (mut controller, workspace, first) = setup();
        let second = split(&mut controller, first, Axis::Vertical);
        let id = split_ids(controller.model().workspace(workspace).unwrap().layout())[0];
        controller
            .dispatch(Command::SetSplitRatio {
                split: id,
                ratio: 0.3,
            })
            .unwrap();
        let before = controller.model().clone();
        let generation = controller.generation();
        for destination in [
            Destination::Beside {
                pane: first,
                edge: Edge::Right,
            },
            Destination::Beside {
                pane: second,
                edge: Edge::Left,
            },
            Destination::Tab {
                pane: second,
                index: 0,
            },
            Destination::Workspace(workspace),
        ] {
            let effects = controller
                .dispatch(Command::MovePane {
                    pane: second,
                    destination,
                })
                .unwrap();
            assert!(effects.is_empty(), "{destination:?}");
        }
        assert_eq!(controller.model(), &before);
        assert_eq!(controller.generation(), generation);
    }

    fn tab(controller: &mut Controller, pane: PaneId) -> PaneId {
        let workspace = controller.model().workspace_for_pane(pane).unwrap();
        controller
            .dispatch(Command::AddTab {
                workspace,
                pane,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        controller.model().workspace(workspace).unwrap().active()
    }
    fn group(controller: &Controller, pane: PaneId) -> (Vec<PaneId>, PaneId) {
        let workspace = controller.model().workspace_for_pane(pane).unwrap();
        let layout = controller.model().workspace(workspace).unwrap().layout();
        let (tabs, shown) = layout.tabs(pane).unwrap();
        (tabs.to_vec(), shown)
    }

    #[test]
    fn a_tab_opens_after_its_pane_in_view_and_focused_without_a_split() {
        let (mut controller, workspace, first) = setup();
        let last = tab(&mut controller, first);
        let effects = controller
            .dispatch(Command::AddTab {
                workspace,
                pane: first,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        let middle = controller.model().active_pane().unwrap();
        assert_eq!(
            without_save(effects),
            [
                Effect::StartSession {
                    pane: middle,
                    generation: 1,
                    cwd: PathBuf::from("/fake"),
                    remote: None,
                    remote_cwd: None,
                    replacement: false,
                },
                Effect::Focus {
                    old: Some(last),
                    new: Some(middle)
                },
                Effect::ResetSearch
            ]
        );
        let ws = controller.model().workspace(workspace).unwrap();
        assert_eq!(
            ws.layout(),
            &Layout::Tabs {
                panes: vec![first, middle, last],
                shown: middle
            }
        );
        // Only splits take split identities.
        let split = split(&mut controller, middle, Axis::Vertical);
        let ws = controller.model().workspace(workspace).unwrap();
        assert_eq!(split_ids(ws.layout()), [SplitId::new(1)]);
        assert_eq!(ws.layout().shown(), [middle, split]);
    }

    fn worktree(branch: &str) -> crate::Worktree {
        crate::Worktree {
            repository: std::env::temp_dir().join("repo"),
            path: std::env::temp_dir().join("repo.worktrees").join(branch),
            branch: branch.into(),
            start: "c".repeat(40),
        }
    }

    #[test]
    fn a_worktree_opens_as_a_focused_tab_whose_shell_starts_the_agent_there() {
        let (mut controller, workspace, first) = setup();
        let tree = worktree("fix-login");
        let effects = controller
            .dispatch(Command::OpenWorktree {
                workspace,
                pane: first,
                worktree: tree.clone(),
                agent: Some(crate::AgentKind::Codex),
            })
            .unwrap();
        let opened = controller.model().active_pane().unwrap();
        assert_eq!(
            without_save(effects)[0],
            Effect::StartSession {
                pane: opened,
                generation: 1,
                cwd: tree.path.clone(),
                remote: None,
                remote_cwd: None,
                replacement: false,
            }
        );
        assert_eq!(group(&controller, first), (vec![first, opened], opened));
        let item = controller.model().pane(opened).unwrap();
        assert_eq!(item.worktree(), Some(&tree));
        assert_eq!(
            item.agent(),
            Some(&crate::AgentSession {
                kind: crate::AgentKind::Codex,
                session_id: None,
                cwd: tree.path.clone(),
            })
        );
        // Tabs and splits beside it are ordinary terminals.
        let beside = tab(&mut controller, opened);
        assert!(
            controller
                .model()
                .pane(beside)
                .unwrap()
                .worktree()
                .is_none()
        );

        // The worktree outlives its agent and a restart, and is restored.
        controller
            .dispatch(Command::PaneAgentChanged {
                pane: opened,
                generation: 1,
                agent: None,
            })
            .unwrap();
        controller.dispatch(Command::RestartPane(opened)).unwrap();
        assert_eq!(
            controller.model().pane(opened).unwrap().worktree(),
            Some(&tree)
        );
        let restored =
            Model::restore(controller.model().specs(), None, true, Default::default()).unwrap();
        assert_eq!(restored.pane(opened).unwrap().worktree(), Some(&tree));

        // A terminal on another machine has no local worktree.
        controller
            .dispatch(Command::SetWorkspaceRemote {
                workspace,
                remote: Some("devbox".into()),
            })
            .unwrap();
        assert!(
            controller
                .model()
                .pane(opened)
                .unwrap()
                .worktree()
                .is_none()
        );
    }

    #[test]
    fn an_unusable_worktree_or_a_remote_workspace_opens_nothing() {
        let (mut controller, workspace, first) = setup();
        let before = controller.model().clone();
        let open = |worktree| Command::OpenWorktree {
            workspace,
            pane: first,
            worktree,
            agent: None,
        };
        let unusable = crate::Worktree {
            branch: "-D".into(),
            ..worktree("x")
        };
        assert_eq!(
            controller.dispatch(open(unusable)),
            Err(Error::InvalidWorktree)
        );
        assert_eq!(controller.model(), &before);
        controller
            .dispatch(Command::SetWorkspaceRemote {
                workspace,
                remote: Some("devbox".into()),
            })
            .unwrap();
        assert_eq!(
            controller.dispatch(open(worktree("x"))),
            Err(Error::InvalidWorktree)
        );
        let mut specs = before.specs();
        specs[0].panes[0].worktree = Some(crate::Worktree {
            start: "main".into(),
            ..worktree("x")
        });
        assert_eq!(
            Model::restore(specs, None, true, Default::default()),
            Err(Error::InvalidWorktree)
        );
    }

    #[test]
    fn focusing_a_tab_brings_it_into_view_and_closing_one_reveals_its_neighbour() {
        let (mut controller, workspace, first) = setup();
        let second = tab(&mut controller, first);
        let third = tab(&mut controller, second);
        let beside = split(&mut controller, third, Axis::Vertical);
        assert_eq!(
            group(&controller, first),
            (vec![first, second, third], third)
        );
        let effects = controller
            .dispatch(Command::FocusPane {
                workspace,
                pane: second,
            })
            .unwrap();
        assert_eq!(
            without_save(effects),
            [
                Effect::Focus {
                    old: Some(beside),
                    new: Some(second)
                },
                Effect::ResetSearch
            ]
        );
        assert_eq!(group(&controller, first).1, second);
        // A tab out of view closes without disturbing the view or the focus.
        let effects = controller.dispatch(Command::ClosePane(first)).unwrap();
        assert_eq!(
            without_save(effects),
            [Effect::StopSession {
                pane: first,
                generation: 1
            }]
        );
        assert_eq!(group(&controller, second), (vec![second, third], second));
        // The tab in view hands its place and the focus to the next tab, and
        // the last tab of a place to the neighbouring place.
        controller.dispatch(Command::ClosePane(second)).unwrap();
        assert_eq!(controller.model().active_pane(), Some(third));
        controller.dispatch(Command::ClosePane(third)).unwrap();
        let ws = controller.model().workspace(workspace).unwrap();
        assert_eq!(ws.active(), beside);
        assert_eq!(ws.layout(), &Layout::pane(beside));
    }

    #[test]
    fn tabs_reorder_join_other_places_and_split_away_from_their_own() {
        let (mut controller, workspace, first) = setup();
        let second = tab(&mut controller, first);
        let third = tab(&mut controller, second);
        let beside = split(&mut controller, third, Axis::Vertical);
        let mut run = |pane, destination| {
            controller
                .dispatch(Command::MovePane { pane, destination })
                .unwrap();
            let ws = controller.model().workspace(workspace).unwrap();
            assert_eq!(ws.active(), pane);
            assert!(ws.layout().shown().contains(&pane));
            (ws.layout().clone(), split_ids(ws.layout()))
        };
        // The index counts the other tabs, whichever tab names the place.
        let (layout, _) = run(
            third,
            Destination::Tab {
                pane: third,
                index: 0,
            },
        );
        assert_eq!(layout.tabs(first).unwrap().0, [third, first, second]);
        let (layout, _) = run(
            third,
            Destination::Tab {
                pane: first,
                index: 9,
            },
        );
        assert_eq!(layout.tabs(first).unwrap().0, [first, second, third]);
        let (layout, splits) = run(
            first,
            Destination::Tab {
                pane: beside,
                index: 0,
            },
        );
        assert_eq!(layout.tabs(beside).unwrap().0, [first, beside]);
        assert_eq!(layout.shown(), [third, first]);
        assert_eq!(splits, [SplitId::new(1)]);
        // Against its own place, a tab leaves the others where they are.
        let (layout, splits) = run(
            third,
            Destination::Beside {
                pane: third,
                edge: Edge::Bottom,
            },
        );
        assert_eq!(layout.shown(), [second, third, first]);
        assert_eq!(splits, [SplitId::new(1), SplitId::new(2)]);
        // The last tab of a place takes the place with it.
        let (layout, splits) = run(
            second,
            Destination::Tab {
                pane: third,
                index: 1,
            },
        );
        assert_eq!(layout.tabs(third).unwrap().0, [third, second]);
        assert_eq!(splits, [SplitId::new(1)]);
        assert_eq!(layout.panes(), [third, second, first, beside]);
    }

    #[test]
    fn moving_a_pane_to_another_workspace_keeps_the_view_and_the_session() {
        let (mut controller, home, first) = setup();
        let second = split(&mut controller, first, Axis::Vertical);
        let other = create(&mut controller, "other");
        let resident = controller.model().active_pane().unwrap();
        controller.dispatch(Command::SelectWorkspace(home)).unwrap();
        let effects = controller
            .dispatch(Command::MovePane {
                pane: second,
                destination: Destination::Workspace(other),
            })
            .unwrap();
        // The view stays on the source workspace, whose neighbour takes focus.
        assert_eq!(
            without_save(effects),
            [
                Effect::Focus {
                    old: Some(second),
                    new: Some(first)
                },
                Effect::ResetSearch
            ]
        );
        let model = controller.model();
        assert_eq!(model.active_workspace(), Some(home));
        assert_eq!(
            model.workspace(home).unwrap().layout(),
            &Layout::pane(first)
        );
        let destination = model.workspace(other).unwrap();
        assert_eq!(
            destination.layout(),
            &Layout::Tabs {
                panes: vec![resident, second],
                shown: second
            }
        );
        assert_eq!(destination.active(), second);
        assert_eq!(model.workspace_for_pane(second), Some(other));
        assert_eq!(model.pane_count(), 3);
        // The session it carried still reports into the same pane.
        controller
            .complete(Completion::Started {
                pane: second,
                generation: 1,
            })
            .unwrap();
        assert_eq!(
            controller.model().pane(second).unwrap().lifecycle(),
            &Lifecycle::Running
        );
    }

    #[test]
    fn panes_sent_to_a_workspace_become_the_last_tabs_beside_its_focused_pane() {
        let (mut controller, home, resident) = setup();
        let beside = split(&mut controller, resident, Axis::Vertical);
        controller
            .dispatch(Command::FocusPane {
                workspace: home,
                pane: resident,
            })
            .unwrap();
        let mut arrivals = Vec::new();
        for _ in 0..2 {
            create(&mut controller, "source");
            arrivals.push(controller.model().active_pane().unwrap());
            controller
                .dispatch(Command::MovePane {
                    pane: *arrivals.last().unwrap(),
                    destination: Destination::Workspace(home),
                })
                .unwrap();
        }
        let ws = controller.model().workspace(home).unwrap();
        assert_eq!(
            ws.layout().tabs(resident),
            Some((&[resident, arrivals[0], arrivals[1]][..], arrivals[1]))
        );
        assert_eq!(ws.layout().shown(), [arrivals[1], beside]);
        assert_eq!(ws.active(), arrivals[1]);
        assert_eq!(controller.model().workspaces().len(), 1);
    }

    #[test]
    fn moving_the_last_pane_out_removes_its_workspace_and_follows_the_pane() {
        let (mut controller, home, first) = setup();
        let other = create(&mut controller, "other");
        let alone = controller.model().active_pane().unwrap();
        let effects = controller
            .dispatch(Command::MovePane {
                pane: alone,
                destination: Destination::Beside {
                    pane: first,
                    edge: Edge::Left,
                },
            })
            .unwrap();
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::StopSession { .. } | Effect::Focus { .. })),
            "the same pane stays focused in its new workspace"
        );
        let model = controller.model();
        assert!(model.workspace(other).is_none());
        assert_eq!(model.active_workspace(), Some(home));
        assert_eq!(model.active_pane(), Some(alone));
        assert_eq!(
            model.workspace(home).unwrap().layout().panes(),
            [alone, first]
        );
    }

    #[test]
    fn rejected_moves_are_atomic() {
        let mut controller = Controller::new(Model::new(Limits {
            panes_per_workspace: 2,
            ..Limits::default()
        }));
        let home = create(&mut controller, "home");
        let first = controller.model().active_pane().unwrap();
        let second = split(&mut controller, first, Axis::Vertical);
        let other = create(&mut controller, "other");
        let third = controller.model().active_pane().unwrap();
        let before = controller.model().clone();
        let generation = controller.generation();
        for (pane, destination, error) in [
            (third, Destination::Workspace(home), Error::PaneLimit),
            (
                third,
                Destination::Beside {
                    pane: second,
                    edge: Edge::Top,
                },
                Error::PaneLimit,
            ),
            (
                third,
                Destination::Tab {
                    pane: first,
                    index: 0,
                },
                Error::PaneLimit,
            ),
            (
                third,
                Destination::Workspace(WorkspaceId::new(99)),
                Error::UnknownWorkspace(WorkspaceId::new(99)),
            ),
            (
                PaneId::new(99),
                Destination::Workspace(other),
                Error::UnknownPane(PaneId::new(99)),
            ),
        ] {
            assert_eq!(
                controller.dispatch(Command::MovePane { pane, destination }),
                Err(error)
            );
        }
        assert_eq!(controller.model(), &before);
        assert_eq!(controller.generation(), generation);
        // Rearranging a full workspace needs no spare capacity.
        controller
            .dispatch(Command::MovePane {
                pane: first,
                destination: Destination::Beside {
                    pane: second,
                    edge: Edge::Bottom,
                },
            })
            .unwrap();
        assert_eq!(
            controller.model().workspace(home).unwrap().layout().panes(),
            [second, first]
        );
    }

    #[test]
    fn invalid_focus_and_split_are_atomic() {
        let (mut controller, workspace, _) = setup();
        let before = controller.model().clone();
        assert!(
            controller
                .dispatch(Command::FocusPane {
                    workspace,
                    pane: PaneId::new(99)
                })
                .is_err()
        );
        assert!(
            controller
                .dispatch(Command::SplitPane {
                    workspace,
                    pane: PaneId::new(99),
                    axis: Axis::Vertical,
                    cwd: PathBuf::new()
                })
                .is_err()
        );
        assert_eq!(controller.model(), &before);
    }

    #[test]
    fn unchanged_split_ratio_does_not_schedule_storage_work() {
        let (mut controller, workspace, pane) = setup();
        controller
            .dispatch(Command::SplitPane {
                workspace,
                pane,
                axis: Axis::Vertical,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        let split = match controller.model().workspace(workspace).unwrap().layout() {
            Layout::Split { id, .. } => *id,
            _ => panic!("expected split"),
        };
        let generation = controller.generation();
        assert!(
            controller
                .dispatch(Command::SetSplitRatio { split, ratio: 0.5 })
                .unwrap()
                .is_empty()
        );
        assert_eq!(controller.generation(), generation);
    }

    #[test]
    fn splits_share_their_line_and_evening_reports_only_real_changes() {
        fn ratios(layout: &Layout) -> Vec<f32> {
            match layout {
                Layout::Tabs { .. } => Vec::new(),
                Layout::Split {
                    ratio,
                    first,
                    second,
                    ..
                } => [vec![*ratio], ratios(first), ratios(second)].concat(),
            }
        }
        let (mut controller, workspace, first) = setup();
        let second = split(&mut controller, first, Axis::Vertical);
        let third = split(&mut controller, second, Axis::Vertical);
        let layout = |controller: &Controller| {
            controller
                .model()
                .workspace(workspace)
                .unwrap()
                .layout()
                .clone()
        };
        // Three columns of one width: a third, then half of the rest.
        assert_eq!(ratios(&layout(&controller)), [1.0 / 3.0, 0.5]);
        controller.dispatch(Command::ClosePane(third)).unwrap();
        assert_eq!(ratios(&layout(&controller)), [0.5]);
        let third = split(&mut controller, first, Axis::Vertical);
        assert_eq!(ratios(&layout(&controller)), [2.0 / 3.0, 0.5]);
        // Moving a terminal along the line keeps the columns even.
        controller
            .dispatch(Command::MovePane {
                pane: first,
                destination: Destination::Beside {
                    pane: second,
                    edge: Edge::Right,
                },
            })
            .unwrap();
        assert_eq!(
            controller
                .model()
                .workspace(workspace)
                .unwrap()
                .layout()
                .shown(),
            [third, second, first]
        );
        assert_eq!(ratios(&layout(&controller)), [1.0 / 3.0, 0.5]);

        let ids = split_ids(&layout(&controller));
        let generation = controller.generation();
        // Dropped where it stands, a terminal leaves the line as it is.
        let unmoved = Command::MovePane {
            pane: first,
            destination: Destination::Beside {
                pane: second,
                edge: Edge::Right,
            },
        };
        for command in [
            unmoved,
            Command::EvenSplit(ids[1]),
            Command::EvenSplits(workspace),
        ] {
            assert!(controller.dispatch(command).unwrap().is_empty());
        }
        assert_eq!(controller.generation(), generation);
        controller
            .dispatch(Command::SetSplitRatio {
                split: ids[0],
                ratio: 0.7,
            })
            .unwrap();
        // Either divider evens the whole line.
        assert!(
            !controller
                .dispatch(Command::EvenSplit(ids[1]))
                .unwrap()
                .is_empty()
        );
        assert_eq!(ratios(&layout(&controller)), [1.0 / 3.0, 0.5]);
        controller
            .dispatch(Command::SetSplitRatio {
                split: ids[1],
                ratio: 0.2,
            })
            .unwrap();
        assert!(
            !controller
                .dispatch(Command::EvenSplits(workspace))
                .unwrap()
                .is_empty()
        );
        assert_eq!(ratios(&layout(&controller)), [1.0 / 3.0, 0.5]);
        assert_eq!(
            controller.dispatch(Command::EvenSplit(SplitId::new(99))),
            Err(Error::UnknownSplit(SplitId::new(99)))
        );
        assert_eq!(
            controller.dispatch(Command::EvenSplits(WorkspaceId::new(99))),
            Err(Error::UnknownWorkspace(WorkspaceId::new(99)))
        );
    }

    #[test]
    fn mixed_command_sequences_always_preserve_restorable_layout_and_focus() {
        let (mut controller, _, _) = setup();
        let mut sequence = 0x1234_5678_u64;
        let (mut spawned, mut put_away) = (0, 0);
        for step in 0..4000 {
            sequence = sequence
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            if controller.model().workspaces().is_empty() {
                create(&mut controller, "replacement");
            }
            let index = (sequence as usize) % controller.model().workspaces().len();
            let ws = &controller.model().workspaces()[index];
            let workspace = ws.id();
            let pane = ws.panes()[(sequence.rotate_right(17) as usize) % ws.panes().len()].id();
            let other = controller.model().workspaces()
                [(sequence.rotate_right(29) as usize) % controller.model().workspaces().len()]
            .active();
            let project = controller.model().project_of(workspace).map(Project::id);
            let stale = ProjectId::new(1 + sequence.rotate_right(41) % 8);
            let command = match sequence % 20 {
                0 => Command::AddWorkspace {
                    group: None,
                    cwd: PathBuf::from("/fake"),
                    name: format!("workspace {step}"),
                    remote: (step % 3 == 0).then(|| "me@devbox".to_owned()),
                },
                1 => Command::SplitPane {
                    workspace,
                    pane,
                    axis: Axis::Horizontal,
                    cwd: PathBuf::from("/fake"),
                },
                2 => Command::ClosePane(pane),
                3 => Command::CloseWorkspace(workspace),
                4 => Command::FocusPane { workspace, pane },
                5 => Command::RenameWorkspace {
                    workspace,
                    name: format!("renamed {step}"),
                },
                6 => Command::SetWorkspaceRemote {
                    workspace,
                    remote: (step % 2 == 0).then(|| format!("host{step}")),
                },
                7 => Command::MovePane {
                    pane,
                    destination: Destination::Beside {
                        pane: other,
                        edge: [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom]
                            [(sequence.rotate_right(41) as usize) % 4],
                    },
                },
                8 => Command::MovePane {
                    pane,
                    destination: Destination::Tab {
                        pane: other,
                        index: (sequence.rotate_right(41) as usize) % 4,
                    },
                },
                9 => Command::MovePane {
                    pane: other,
                    destination: Destination::Workspace(workspace),
                },
                10 => Command::MoveWorkspace {
                    workspace,
                    index: (sequence.rotate_right(29) as usize) % 30,
                },
                11 => Command::AddTab {
                    workspace,
                    pane,
                    cwd: PathBuf::from("/fake"),
                },
                12 => Command::AddProject {
                    workspace,
                    name: format!("project {step}"),
                    key: ProjectKey::parse(&format!("{step:016x}")).unwrap(),
                    lead: crate::AgentKind::Claude,
                },
                13 | 16 | 17 => Command::SpawnProjectAgent {
                    project: project.unwrap_or(stale),
                    cwd: PathBuf::from("/fake"),
                    worktree: None,
                },
                14 => Command::RemoveProject(project.unwrap_or(stale)),
                15 => Command::SetProjectPaused {
                    project: project.unwrap_or(stale),
                    paused: step % 5 == 0,
                },
                18 => Command::BackgroundPane(pane),
                _ => Command::SelectWorkspace(workspace),
            };
            let before = controller.model().clone();
            let hides = command == Command::BackgroundPane(pane);
            match controller.dispatch(command) {
                Err(_) => assert_eq!(controller.model(), &before),
                Ok(effects) => {
                    put_away += usize::from(hides);
                    // A project's agent reports itself, as one that lasts does.
                    for effect in effects {
                        if let Effect::StartSession { pane, .. } = effect
                            && controller.model().pane(pane).unwrap().project().is_some()
                        {
                            spawned += 1;
                            controller
                                .dispatch(Command::PaneAgentChanged {
                                    pane,
                                    generation: 1,
                                    agent: Some(crate::AgentSession {
                                        kind: crate::AgentKind::Claude,
                                        session_id: None,
                                        cwd: std::env::temp_dir(),
                                    }),
                                })
                                .unwrap();
                        }
                    }
                }
            }
            let model = controller.model();
            assert!(model.projects().len() <= Project::MAX);
            for project in model.projects() {
                let ws = model.workspace(project.workspace()).unwrap();
                assert!(ws.remote().is_none(), "step {step}");
                assert_eq!(model.project_of(ws.id()), Some(project), "step {step}");
                assert!(model.project_agents(project.id()).count() <= Project::MAX_AGENTS);
            }
            for ws in model.workspaces() {
                assert!(ws.layout().shown().contains(&ws.active()), "step {step}");
                for pane in ws.panes() {
                    // Nothing runs out of view that no project answers for.
                    let owner = pane.project().and_then(|id| model.project(id));
                    assert_eq!(pane.project().is_some(), owner.is_some(), "step {step}");
                    assert!(owner.is_none_or(|owner| owner.workspace() == ws.id()));
                    assert!(
                        owner.is_some() || !ws.is_background(pane.id()),
                        "step {step}"
                    );
                }
            }
            let restored = Model::restore_with_projects(
                model.specs(),
                model.group_specs(),
                Some(model.sidebar_order().to_vec()),
                model.project_specs(),
                model.active_workspace(),
                model.sidebar(),
                model.limits(),
            )
            .unwrap();
            assert_eq!(restored.projects(), model.projects(), "step {step}");
        }
        // The sequence did put project agents among the moves and closes.
        assert!(spawned > 20, "{spawned}");
        // And sent a tab of theirs back out of view.
        assert!(put_away > 0, "{put_away}");
    }

    #[test]
    fn restart_at_capacity_replaces_session_and_rejects_stale_completion() {
        let mut controller = Controller::new(Model::new(Limits {
            total_panes: 1,
            ..Limits::default()
        }));
        create(&mut controller, "main");
        let pane = controller.model().active_pane().unwrap();
        let effects = controller.dispatch(Command::RestartPane(pane)).unwrap();
        assert!(effects.contains(&Effect::StartSession {
            pane,
            generation: 2,
            cwd: PathBuf::from("/fake"),
            remote: None,
            remote_cwd: None,
            replacement: true
        }));
        controller
            .complete(Completion::Started {
                pane,
                generation: 1,
            })
            .unwrap();
        assert_eq!(
            controller.model().pane(pane).unwrap().lifecycle(),
            &Lifecycle::Starting
        );
        controller
            .complete(Completion::Failed {
                pane,
                generation: 2,
                error: "fixture failure".into(),
            })
            .unwrap();
        assert_eq!(
            controller.model().pane(pane).unwrap().lifecycle(),
            &Lifecycle::Failed("fixture failure".into())
        );
    }

    #[test]
    fn completion_after_close_is_ignored() {
        let (mut controller, _, pane) = setup();
        controller.dispatch(Command::ClosePane(pane)).unwrap();
        assert!(
            controller
                .complete(Completion::Started {
                    pane,
                    generation: 1
                })
                .unwrap()
                .is_empty()
        );
        assert!(controller.model().pane(pane).is_none());
    }

    #[test]
    fn saved_generation_cannot_acknowledge_newer_changes() {
        let (mut controller, _, _) = setup();
        let first = controller.generation();
        controller.dispatch(Command::SetSidebar(false)).unwrap();
        controller
            .complete(Completion::Saved { generation: first })
            .unwrap();
        assert!(controller.is_dirty());
        controller
            .complete(Completion::Saved {
                generation: u64::MAX,
            })
            .unwrap();
        assert!(controller.is_dirty());
        controller
            .complete(Completion::Saved {
                generation: controller.generation(),
            })
            .unwrap();
        assert!(!controller.is_dirty());
    }

    #[test]
    fn fake_runtime_drives_real_model_without_shells_fonts_or_gpu() {
        let mut controller = Controller::new(Model::default());
        let mut runtime = FakeRuntime::default();
        runtime.run(
            &mut controller,
            Command::AddWorkspace {
                group: None,
                cwd: PathBuf::from("/fake"),
                name: "fixture".into(),
                remote: None,
            },
        );
        let pane = controller.model().active_pane().unwrap();
        assert_eq!(
            controller.model().pane(pane).unwrap().lifecycle(),
            &Lifecycle::Running
        );
        runtime.run(&mut controller, Command::RestartPane(pane));
        assert_eq!(controller.model().pane(pane).unwrap().generation(), 2);
        runtime.run(&mut controller, Command::ClosePane(pane));
        assert_eq!(controller.model().pane_count(), 0);
    }

    fn remote(destination: &str) -> Option<Remote> {
        Some(Remote::parse(destination).unwrap())
    }
    fn starts(effects: &[Effect]) -> Vec<(PaneId, u64, Option<Remote>, bool)> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::StartSession {
                    pane,
                    generation,
                    remote,
                    replacement,
                    ..
                } => Some((*pane, *generation, remote.clone(), *replacement)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn remote_directories_follow_each_pane_through_split_restart_and_restore() {
        let (mut controller, workspace, first) = setup();
        controller
            .dispatch(Command::SetWorkspaceRemote {
                workspace,
                remote: Some("devbox".into()),
            })
            .unwrap();
        let directory = PathBuf::from("/srv/project ' % λ");
        let generation = controller.model().pane(first).unwrap().generation();
        controller
            .dispatch(Command::PaneRemoteCwdChanged {
                pane: first,
                generation,
                cwd: directory.clone(),
            })
            .unwrap();
        let split = controller
            .dispatch(Command::SplitPane {
                workspace,
                pane: first,
                axis: Axis::Vertical,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        let second = controller.model().active_pane().unwrap();
        assert!(split.iter().any(|effect| matches!(effect, Effect::StartSession { pane, remote_cwd: Some(cwd), .. } if *pane == second && *cwd == directory)));
        controller
            .dispatch(Command::PaneRemoteCwdChanged {
                pane: second,
                generation: 1,
                cwd: PathBuf::from("/srv/other"),
            })
            .unwrap();
        let restarted = controller.dispatch(Command::RestartPane(first)).unwrap();
        assert!(restarted.iter().any(|effect| matches!(effect, Effect::StartSession { pane, remote_cwd: Some(cwd), .. } if *pane == first && *cwd == directory)));
        let restored = Controller::new(
            Model::restore(
                controller.model().specs(),
                Some(workspace),
                true,
                Limits::default(),
            )
            .unwrap(),
        );
        let directories: Vec<_> = restored
            .start_effects()
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::StartSession { remote_cwd, .. } => remote_cwd,
                _ => None,
            })
            .collect();
        assert_eq!(directories, [directory, PathBuf::from("/srv/other")]);
        for pane in [first, second] {
            assert_eq!(
                restored.model().pane(pane).unwrap().cwd(),
                std::path::Path::new("/fake")
            );
        }
    }

    #[test]
    fn remote_directory_reports_ignore_stale_closed_and_local_sessions() {
        let (mut controller, workspace, pane) = setup();
        let report = |generation| Command::PaneRemoteCwdChanged {
            pane,
            generation,
            cwd: PathBuf::from("/srv/project"),
        };
        let generation = controller.generation();
        assert!(controller.dispatch(report(1)).unwrap().is_empty());
        assert_eq!(controller.generation(), generation);
        controller
            .dispatch(Command::SetWorkspaceRemote {
                workspace,
                remote: Some("devbox".into()),
            })
            .unwrap();
        assert!(controller.dispatch(report(1)).unwrap().is_empty());
        assert!(
            controller
                .dispatch(report(2))
                .unwrap()
                .iter()
                .any(|effect| matches!(effect, Effect::Persist { .. }))
        );
        assert!(controller.dispatch(report(2)).unwrap().is_empty());
        controller.dispatch(Command::RestartPane(pane)).unwrap();
        assert!(controller.dispatch(report(2)).unwrap().is_empty());
        assert_eq!(
            controller.model().pane(pane).unwrap().remote_cwd(),
            Some(std::path::Path::new("/srv/project"))
        );
        controller.dispatch(Command::ClosePane(pane)).unwrap();
        assert!(controller.dispatch(report(3)).unwrap().is_empty());
    }

    #[test]
    fn changing_ssh_hosts_and_disconnecting_clear_remote_directories() {
        let (mut controller, workspace, pane) = setup();
        for destination in [Some("devbox"), Some("otherbox"), None] {
            let effects = controller
                .dispatch(Command::SetWorkspaceRemote {
                    workspace,
                    remote: destination.map(str::to_owned),
                })
                .unwrap();
            assert_eq!(controller.model().pane(pane).unwrap().remote_cwd(), None);
            assert!(effects.iter().any(|effect| matches!(
                effect,
                Effect::StartSession {
                    remote_cwd: None,
                    ..
                }
            )));
            if destination.is_some() {
                let generation = controller.model().pane(pane).unwrap().generation();
                controller
                    .dispatch(Command::PaneRemoteCwdChanged {
                        pane,
                        generation,
                        cwd: PathBuf::from("/srv/project"),
                    })
                    .unwrap();
            }
        }
    }

    #[test]
    fn every_terminal_of_a_remote_workspace_starts_on_its_host() {
        let mut controller = Controller::new(Model::default());
        let created = controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: PathBuf::from("/fake"),
                name: "devbox".into(),
                remote: Some(" me@devbox ".into()),
            })
            .unwrap();
        let workspace = controller.model().active_workspace().unwrap();
        let first = controller.model().active_pane().unwrap();
        assert_eq!(
            controller.model().workspace(workspace).unwrap().remote(),
            remote("me@devbox").as_ref()
        );
        assert_eq!(starts(&created), [(first, 1, remote("me@devbox"), false)]);

        let split = controller
            .dispatch(Command::SplitPane {
                workspace,
                pane: first,
                axis: Axis::Vertical,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        let second = controller.model().active_pane().unwrap();
        assert_eq!(starts(&split), [(second, 1, remote("me@devbox"), false)]);

        let restarted = controller.dispatch(Command::RestartPane(first)).unwrap();
        assert_eq!(starts(&restarted), [(first, 2, remote("me@devbox"), true)]);

        // A local neighbour is unaffected, and restoration reconnects.
        create(&mut controller, "local");
        let restored = Controller::new(
            Model::restore(
                controller.model().specs(),
                None,
                true,
                controller.model().limits(),
            )
            .unwrap(),
        );
        let remotes: Vec<_> = starts(&restored.start_effects())
            .into_iter()
            .map(|(_, _, remote, _)| remote)
            .collect();
        assert_eq!(remotes, [remote("me@devbox"), remote("me@devbox"), None]);
    }

    #[test]
    fn connecting_a_workspace_replaces_all_of_its_terminals_and_only_those() {
        let (mut controller, workspace, first) = setup();
        controller
            .dispatch(Command::SplitPane {
                workspace,
                pane: first,
                axis: Axis::Horizontal,
                cwd: PathBuf::from("/fake"),
            })
            .unwrap();
        let second = controller.model().active_pane().unwrap();
        let other = create(&mut controller, "other");
        let other_pane = controller.model().active_pane().unwrap();
        for pane in [first, second, other_pane] {
            controller
                .complete(Completion::Started {
                    pane,
                    generation: 1,
                })
                .unwrap();
        }
        let saved = controller.generation();

        let effects = controller
            .dispatch(Command::SetWorkspaceRemote {
                workspace,
                remote: Some("me@devbox".into()),
            })
            .unwrap();
        assert_eq!(
            starts(&effects),
            [
                (first, 2, remote("me@devbox"), true),
                (second, 2, remote("me@devbox"), true)
            ]
        );
        for pane in [first, second] {
            assert!(effects.contains(&Effect::StopSession {
                pane,
                generation: 1
            }));
            assert_eq!(
                controller.model().pane(pane).unwrap().lifecycle(),
                &Lifecycle::Starting
            );
        }
        assert!(effects.contains(&Effect::Persist {
            generation: saved + 1
        }));
        // The focused pane is elsewhere: its search and session are untouched.
        assert!(!effects.contains(&Effect::ResetSearch));
        assert_eq!(controller.model().pane(other_pane).unwrap().generation(), 1);
        assert_eq!(controller.model().workspace(other).unwrap().remote(), None);
        // The local shell's exit cannot reach its replacement.
        controller
            .complete(Completion::Exited {
                pane: first,
                generation: 1,
            })
            .unwrap();
        assert_eq!(
            controller.model().pane(first).unwrap().lifecycle(),
            &Lifecycle::Starting
        );

        // The same host again is not a reason to drop the connections.
        assert!(
            controller
                .dispatch(Command::SetWorkspaceRemote {
                    workspace,
                    remote: Some("me@devbox".into()),
                })
                .unwrap()
                .is_empty()
        );

        // Disconnecting is the reverse action.
        controller
            .dispatch(Command::SelectWorkspace(workspace))
            .unwrap();
        let effects = controller
            .dispatch(Command::SetWorkspaceRemote {
                workspace,
                remote: None,
            })
            .unwrap();
        assert_eq!(
            starts(&effects),
            [(first, 3, None, true), (second, 3, None, true)]
        );
        assert!(effects.contains(&Effect::ResetSearch));
        assert_eq!(
            controller.model().workspace(workspace).unwrap().remote(),
            None
        );
    }

    #[test]
    fn a_terminal_moves_only_between_workspaces_on_the_same_machine() {
        let mut controller = Controller::new(Model::default());
        let add = |controller: &mut Controller, remote: Option<&str>| {
            controller
                .dispatch(Command::AddWorkspace {
                    group: None,
                    cwd: PathBuf::from("/fake"),
                    name: "workspace".into(),
                    remote: remote.map(str::to_owned),
                })
                .unwrap();
            (
                controller.model().active_workspace().unwrap(),
                controller.model().active_pane().unwrap(),
            )
        };
        let (local, local_pane) = add(&mut controller, None);
        let (devbox, devbox_pane) = add(&mut controller, Some("me@devbox"));
        let (same_host, same_host_pane) = add(&mut controller, Some("me@devbox"));
        let (_, other_host_pane) = add(&mut controller, Some("me@buildbox"));
        let before = controller.model().clone();
        for (pane, destination) in [
            (local_pane, Destination::Workspace(devbox)),
            (devbox_pane, Destination::Workspace(local)),
            (
                other_host_pane,
                Destination::Beside {
                    pane: devbox_pane,
                    edge: Edge::Right,
                },
            ),
        ] {
            assert_eq!(
                controller.dispatch(Command::MovePane { pane, destination }),
                Err(Error::RemoteMismatch)
            );
        }
        assert_eq!(controller.model(), &before);

        // The same host is the same machine: the session moves untouched.
        let effects = controller
            .dispatch(Command::MovePane {
                pane: same_host_pane,
                destination: Destination::Workspace(devbox),
            })
            .unwrap();
        assert!(starts(&effects).is_empty());
        assert!(controller.model().workspace(same_host).is_none());
        assert_eq!(
            controller.model().workspace(devbox).unwrap().panes().len(),
            2
        );
    }

    #[test]
    fn an_unusable_ssh_destination_is_refused_without_changing_anything() {
        let (mut controller, workspace, _) = setup();
        let before = controller.model().clone();
        let generation = controller.generation();
        for command in [
            Command::AddWorkspace {
                group: None,
                cwd: PathBuf::from("/fake"),
                name: "bad".into(),
                remote: Some("-oProxyCommand=id".into()),
            },
            Command::SetWorkspaceRemote {
                workspace,
                remote: Some("two words".into()),
            },
        ] {
            assert_eq!(controller.dispatch(command), Err(Error::InvalidRemote));
        }
        assert_eq!(
            controller.dispatch(Command::SetWorkspaceRemote {
                workspace: WorkspaceId::new(99),
                remote: None,
            }),
            Err(Error::UnknownWorkspace(WorkspaceId::new(99)))
        );
        assert_eq!(controller.model(), &before);
        assert_eq!(controller.generation(), generation);

        let mut specs = before.specs();
        specs[0].remote = Some("-oProxyCommand=id".into());
        assert_eq!(
            Model::restore(specs, None, true, Limits::default()),
            Err(Error::InvalidRemote)
        );
    }

    #[test]
    fn restoration_rejects_duplicate_missing_leaves_and_invalid_ratios() {
        let mut spec = WorkspaceSpec {
            group: None,
            id: WorkspaceId::new(1),
            name: "fixture".into(),
            cwd: PathBuf::new(),
            remote: None,
            panes: vec![
                PaneSpec {
                    id: PaneId::new(1),
                    cwd: PathBuf::new(),
                    remote_cwd: None,
                    agent: None,
                    pull_requests: Vec::new(),
                    attachments: Vec::new(),
                    spawned_by: None,
                    project: None,
                    worktree: None,
                },
                PaneSpec {
                    id: PaneId::new(2),
                    cwd: PathBuf::new(),
                    remote_cwd: None,
                    agent: None,
                    pull_requests: Vec::new(),
                    attachments: Vec::new(),
                    spawned_by: None,
                    project: None,
                    worktree: None,
                },
            ],
            layout: Layout::pane(PaneId::new(1)),
            active: PaneId::new(1),
        };
        assert!(Model::restore(vec![spec.clone()], None, true, Limits::default()).is_err());
        for (panes, shown) in [(vec![1, 2, 2], 1), (vec![1, 2], 3), (vec![], 1)] {
            spec.layout = Layout::Tabs {
                panes: panes.into_iter().map(PaneId::new).collect(),
                shown: PaneId::new(shown),
            };
            assert!(Model::restore(vec![spec.clone()], None, true, Limits::default()).is_err());
        }
        // The focused pane comes into view wherever the saved tabs left it.
        spec.layout = Layout::Tabs {
            panes: vec![PaneId::new(2), PaneId::new(1)],
            shown: PaneId::new(2),
        };
        let model = Model::restore(vec![spec.clone()], None, true, Limits::default()).unwrap();
        assert_eq!(model.workspaces()[0].layout().shown(), [PaneId::new(1)]);
        spec.layout = Layout::Split {
            id: SplitId::new(1),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::pane(PaneId::new(1))),
            second: Box::new(Layout::pane(PaneId::new(1))),
        };
        assert!(Model::restore(vec![spec.clone()], None, true, Limits::default()).is_err());
        if let Layout::Split { ratio, second, .. } = &mut spec.layout {
            *ratio = f32::NAN;
            **second = Layout::pane(PaneId::new(2));
        }
        assert_eq!(
            Model::restore(vec![spec], None, true, Limits::default()),
            Err(Error::InvalidRatio)
        );
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use crate::Limits;

    #[test]
    fn mixed_sidebar_moves_preserve_sessions_and_drive_navigation_and_restoration() {
        let mut controller = Controller::new(Model::default());
        for name in ["first", "second"] {
            controller
                .dispatch(Command::AddWorkspace {
                    cwd: "/fake".into(),
                    name: name.into(),
                    remote: None,
                    group: None,
                })
                .unwrap();
        }
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        for name in ["client", "server"] {
            controller
                .dispatch(Command::AddWorkspace {
                    cwd: "/fake".into(),
                    name: name.into(),
                    remote: None,
                    group: Some(group),
                })
                .unwrap();
        }
        controller
            .dispatch(Command::SetWorkspaceGroupCollapsed {
                group,
                collapsed: true,
            })
            .unwrap();
        let before = controller.model().workspaces().to_owned();
        let active = controller.model().active_workspace();
        for (index, names) in [
            (0, ["client", "server", "first", "second"]),
            (1, ["first", "client", "server", "second"]),
            (usize::MAX, ["first", "second", "client", "server"]),
        ] {
            assert!(matches!(
                controller
                    .dispatch(Command::MoveSidebarItem {
                        item: SidebarItem::Group(group),
                        index
                    })
                    .unwrap()
                    .as_slice(),
                [Effect::Persist { .. }]
            ));
            assert_eq!(
                controller
                    .model()
                    .workspaces()
                    .iter()
                    .map(Workspace::name)
                    .collect::<Vec<_>>(),
                names
            );
            for workspace in &before {
                assert_eq!(
                    controller.model().workspace(workspace.id()),
                    Some(workspace)
                );
            }
            assert_eq!(controller.model().active_workspace(), active);
            let restored = Model::restore_ordered(
                controller.model().specs(),
                controller.model().group_specs(),
                Some(controller.model().sidebar_order().to_owned()),
                active,
                true,
                Limits::default(),
            )
            .unwrap();
            assert_eq!(restored.sidebar_order(), controller.model().sidebar_order());
            assert_eq!(restored.workspaces(), controller.model().workspaces());
        }
        let before = controller.model().clone();
        let generation = controller.generation();
        assert!(
            controller
                .dispatch(Command::MoveSidebarItem {
                    item: SidebarItem::Group(group),
                    index: usize::MAX
                })
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            controller.dispatch(Command::MoveSidebarItem {
                item: SidebarItem::Workspace(before.workspaces()[2].id()),
                index: 0
            }),
            Err(Error::InvalidSidebarOrder)
        );
        assert_eq!(
            controller.dispatch(Command::MoveSidebarItem {
                item: SidebarItem::Group(WorkspaceGroupId::new(99)),
                index: 0
            }),
            Err(Error::UnknownWorkspaceGroup(WorkspaceGroupId::new(99)))
        );
        assert_eq!(controller.model(), &before);
        assert_eq!(controller.generation(), generation);
        let order = before.sidebar_order().to_owned();
        for invalid in [
            vec![],
            vec![order[0]; order.len()],
            vec![
                SidebarItem::Workspace(before.workspaces()[2].id()),
                order[1],
                order[2],
            ],
        ] {
            assert_eq!(
                Model::restore_ordered(
                    before.specs(),
                    before.group_specs(),
                    Some(invalid),
                    active,
                    true,
                    Limits::default()
                ),
                Err(Error::InvalidSidebarOrder)
            );
        }
    }

    #[test]
    fn removing_a_mixed_group_replaces_it_with_its_children_in_place() {
        let mut controller = Controller::new(Model::default());
        for name in ["first", "second"] {
            controller
                .dispatch(Command::AddWorkspace {
                    cwd: "/fake".into(),
                    name: name.into(),
                    remote: None,
                    group: None,
                })
                .unwrap();
        }
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        controller
            .dispatch(Command::AddWorkspace {
                cwd: "/fake".into(),
                name: "child".into(),
                remote: None,
                group: Some(group),
            })
            .unwrap();
        controller
            .dispatch(Command::MoveSidebarItem {
                item: SidebarItem::Group(group),
                index: 1,
            })
            .unwrap();
        let pane = controller.model().active_pane();
        controller
            .dispatch(Command::RemoveWorkspaceGroup(group))
            .unwrap();
        assert_eq!(
            controller
                .model()
                .workspaces()
                .iter()
                .map(Workspace::name)
                .collect::<Vec<_>>(),
            ["first", "child", "second"]
        );
        assert_eq!(controller.model().active_pane(), pane);
        let first = controller.model().workspaces()[0].id();
        controller
            .dispatch(Command::MoveWorkspace {
                workspace: first,
                index: 2,
            })
            .unwrap();
        assert_eq!(
            controller
                .model()
                .workspaces()
                .iter()
                .map(Workspace::name)
                .collect::<Vec<_>>(),
            ["child", "second", "first"]
        );
        controller.dispatch(Command::CloseWorkspace(first)).unwrap();
        assert_eq!(controller.model().sidebar_order().len(), 2);
    }

    #[test]
    fn moving_folders_only_persists_order_and_preserves_their_workspaces() {
        let mut controller = Controller::new(Model::default());
        for name in ["Projects", "Servers", "Empty"] {
            controller
                .dispatch(Command::AddWorkspaceGroup { name: name.into() })
                .unwrap();
        }
        let ids: Vec<_> = controller
            .model()
            .groups()
            .iter()
            .map(WorkspaceGroup::id)
            .collect();
        for (name, group) in [
            ("root", None),
            ("client", Some(ids[0])),
            ("server", Some(ids[0])),
            ("remote", Some(ids[1])),
        ] {
            controller
                .dispatch(Command::AddWorkspace {
                    cwd: "/fake".into(),
                    name: name.into(),
                    remote: None,
                    group,
                })
                .unwrap();
        }
        controller
            .dispatch(Command::SetWorkspaceGroupCollapsed {
                group: ids[0],
                collapsed: true,
            })
            .unwrap();
        let workspaces = controller.model().workspaces().to_owned();
        let active = controller.model().active_workspace();
        let generation = controller.generation();
        assert_eq!(
            controller
                .dispatch(Command::MoveWorkspaceGroup {
                    group: ids[0],
                    index: 2
                })
                .unwrap(),
            [Effect::Persist {
                generation: generation + 1
            }]
        );
        assert_eq!(
            controller
                .model()
                .groups()
                .iter()
                .map(WorkspaceGroup::id)
                .collect::<Vec<_>>(),
            [ids[1], ids[2], ids[0]]
        );
        assert!(controller.model().group(ids[0]).unwrap().collapsed());
        assert_eq!(
            controller
                .model()
                .workspaces()
                .iter()
                .map(Workspace::name)
                .collect::<Vec<_>>(),
            ["root", "remote", "client", "server"]
        );
        for workspace in &workspaces {
            assert_eq!(
                controller.model().workspace(workspace.id()),
                Some(workspace)
            );
        }
        assert_eq!(controller.model().active_workspace(), active);
        controller
            .dispatch(Command::MoveWorkspaceGroup {
                group: ids[0],
                index: 0,
            })
            .unwrap();
        assert_eq!(
            controller
                .model()
                .groups()
                .iter()
                .map(WorkspaceGroup::id)
                .collect::<Vec<_>>(),
            ids
        );
        controller
            .dispatch(Command::MoveWorkspaceGroup {
                group: ids[0],
                index: usize::MAX,
            })
            .unwrap();
        let before = controller.model().clone();
        let generation = controller.generation();
        assert!(
            controller
                .dispatch(Command::MoveWorkspaceGroup {
                    group: ids[0],
                    index: 2
                })
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            controller.dispatch(Command::MoveWorkspaceGroup {
                group: WorkspaceGroupId::new(99),
                index: 0
            }),
            Err(Error::UnknownWorkspaceGroup(WorkspaceGroupId::new(99)))
        );
        assert_eq!(controller.model(), &before);
        assert_eq!(controller.generation(), generation);
        assert_eq!(
            Controller::new(Model::default()).dispatch(Command::MoveWorkspaceGroup {
                group: ids[0],
                index: 0
            }),
            Err(Error::UnknownWorkspaceGroup(ids[0]))
        );
    }

    #[test]
    fn folder_changes_preserve_terminal_identity_and_only_persist() {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        controller
            .dispatch(Command::AddWorkspace {
                cwd: "/fake".into(),
                name: "app".into(),
                remote: None,
                group: Some(group),
            })
            .unwrap();
        let workspace = controller.model().active_workspace().unwrap();
        let pane = controller.model().active_pane().unwrap();
        let before = controller.model().pane(pane).unwrap().clone();
        for command in [
            Command::SetWorkspaceGroupCollapsed {
                group,
                collapsed: true,
            },
            Command::RenameWorkspaceGroup {
                group,
                name: "Work".into(),
            },
            Command::SetWorkspaceGroup {
                workspace,
                group: None,
            },
            Command::SetWorkspaceGroup {
                workspace,
                group: Some(group),
            },
            Command::RemoveWorkspaceGroup(group),
        ] {
            assert!(matches!(
                controller.dispatch(command).unwrap().as_slice(),
                [Effect::Persist { .. }]
            ));
            assert_eq!(controller.model().active_pane(), Some(pane));
            assert_eq!(controller.model().pane(pane), Some(&before));
        }
        assert!(controller.model().groups().is_empty());
        assert_eq!(
            controller.model().workspace(workspace).unwrap().group(),
            None
        );
    }

    #[test]
    fn selecting_or_creating_a_workspace_reveals_its_folder() {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        controller
            .dispatch(Command::AddWorkspace {
                cwd: "/fake".into(),
                name: "app".into(),
                remote: None,
                group: Some(group),
            })
            .unwrap();
        let workspace = controller.model().active_workspace().unwrap();
        controller
            .dispatch(Command::SetWorkspaceGroupCollapsed {
                group,
                collapsed: true,
            })
            .unwrap();
        controller
            .dispatch(Command::SelectWorkspace(workspace))
            .unwrap();
        assert!(!controller.model().group(group).unwrap().collapsed());
        controller
            .dispatch(Command::SetWorkspaceGroupCollapsed {
                group,
                collapsed: true,
            })
            .unwrap();
        controller
            .dispatch(Command::AddWorkspace {
                cwd: "/fake".into(),
                name: "remote".into(),
                remote: Some("me@host".into()),
                group: Some(group),
            })
            .unwrap();
        assert!(!controller.model().group(group).unwrap().collapsed());
        assert_eq!(controller.model().workspaces()[1].group(), Some(group));
        controller
            .dispatch(Command::CloseWorkspace(workspace))
            .unwrap();
        let last = controller.model().active_workspace().unwrap();
        controller.dispatch(Command::CloseWorkspace(last)).unwrap();
        assert!(controller.model().workspaces().is_empty());
        assert_eq!(
            controller.model().groups().len(),
            1,
            "empty folders survive the last workspace closing"
        );
    }

    #[test]
    fn invalid_group_commands_are_atomic_and_empty_groups_are_bounded() {
        let mut controller = Controller::new(Model::new(crate::Limits {
            workspaces: 1,
            ..Default::default()
        }));
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let before = controller.model().clone();
        let generation = controller.generation();
        for command in [
            Command::AddWorkspaceGroup {
                name: "Second".into(),
            },
            Command::AddWorkspaceGroup { name: "  ".into() },
            Command::AddWorkspace {
                cwd: "/fake".into(),
                name: "app".into(),
                remote: None,
                group: Some(WorkspaceGroupId::new(99)),
            },
            Command::RemoveWorkspaceGroup(WorkspaceGroupId::new(99)),
        ] {
            assert!(controller.dispatch(command).is_err());
            assert_eq!(controller.model(), &before);
            assert_eq!(controller.generation(), generation);
        }
        controller
            .dispatch(Command::RemoveWorkspaceGroup(before.groups()[0].id()))
            .unwrap();
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Replacement".into(),
            })
            .unwrap();
        assert_ne!(controller.model().groups()[0].id(), before.groups()[0].id());
    }

    #[test]
    fn group_default_only_changes_new_local_workspaces_and_is_restored() {
        let directory = if cfg!(windows) {
            r"C:\project"
        } else {
            "/project"
        };
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: "/original".into(),
                name: "original".into(),
                group: None,
                remote: None,
            })
            .unwrap();
        let original = controller.model().active_workspace().unwrap();
        let pane = controller.model().active_pane().unwrap();
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        controller
            .dispatch(Command::SetWorkspaceGroup {
                workspace: original,
                group: Some(group),
            })
            .unwrap();
        let original_workspace = controller.model().workspace(original).unwrap().clone();
        let effects = controller
            .dispatch(Command::SetWorkspaceGroupDefaultDirectory {
                group,
                directory: Some(directory.into()),
            })
            .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::Persist { .. }]));
        assert_eq!(
            controller.model().workspace(original),
            Some(&original_workspace)
        );
        for command in [
            Command::AddTab {
                workspace: original,
                pane,
                cwd: "/current-tab".into(),
            },
            Command::SplitPane {
                workspace: original,
                pane,
                axis: Axis::Horizontal,
                cwd: "/current-split".into(),
            },
        ] {
            let expected = match &command {
                Command::AddTab { cwd, .. } | Command::SplitPane { cwd, .. } => cwd.clone(),
                _ => unreachable!(),
            };
            let effects = controller.dispatch(command).unwrap();
            assert!(effects.iter().any(
                |effect| matches!(effect, Effect::StartSession { cwd, .. } if cwd == &expected)
            ));
        }
        let restored = Model::restore_ordered(
            controller.model().specs(),
            controller.model().group_specs(),
            Some(controller.model().sidebar_order().to_vec()),
            controller.model().active_workspace(),
            true,
            crate::Limits::default(),
        )
        .unwrap();
        assert_eq!(
            restored.group(group).unwrap().default_directory(),
            Some(std::path::Path::new(directory))
        );
        controller = Controller::new(restored);
        for (membership, remote, expected) in [
            (Some(group), None, directory),
            (None, None, "/provided"),
            (Some(group), Some("host".to_owned()), "/provided"),
        ] {
            let effects = controller
                .dispatch(Command::AddWorkspace {
                    cwd: "/provided".into(),
                    name: "new".into(),
                    group: membership,
                    remote,
                })
                .unwrap();
            let workspace = controller
                .model()
                .workspace(controller.model().active_workspace().unwrap())
                .unwrap();
            assert_eq!(workspace.cwd(), std::path::Path::new(expected));
            assert!(effects.iter().any(|effect| matches!(effect, Effect::StartSession { cwd, remote_cwd: None, .. } if cwd == std::path::Path::new(expected))));
        }
        let effects = controller.dispatch(Command::RestartPane(pane)).unwrap();
        assert!(effects.iter().any(|effect| matches!(effect, Effect::StartSession { cwd, .. } if cwd == original_workspace.pane(pane).unwrap().cwd())));
        controller
            .dispatch(Command::RemoveWorkspaceGroup(group))
            .unwrap();
        assert_eq!(
            controller.model().workspace(original).unwrap().group(),
            None
        );
        assert_eq!(
            controller
                .model()
                .workspace(original)
                .unwrap()
                .pane(pane)
                .unwrap()
                .cwd(),
            original_workspace.pane(pane).unwrap().cwd()
        );
    }

    #[test]
    fn group_directory_changes_are_atomic_idempotent_and_resettable() {
        let directory = if cfg!(windows) {
            r"C:\project"
        } else {
            "/project"
        };
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        for (target, directory, error) in [
            (group, Some("relative".into()), Error::InvalidDirectory),
            (group, Some("/bad\0path".into()), Error::InvalidDirectory),
            (
                WorkspaceGroupId::new(99),
                Some(directory.into()),
                Error::UnknownWorkspaceGroup(WorkspaceGroupId::new(99)),
            ),
        ] {
            let before = controller.model().clone();
            let generation = controller.generation();
            assert_eq!(
                controller.dispatch(Command::SetWorkspaceGroupDefaultDirectory {
                    group: target,
                    directory
                }),
                Err(error)
            );
            assert_eq!(controller.model(), &before);
            assert_eq!(controller.generation(), generation);
        }
        let set = Command::SetWorkspaceGroupDefaultDirectory {
            group,
            directory: Some(directory.into()),
        };
        controller.dispatch(set.clone()).unwrap();
        assert!(controller.dispatch(set).unwrap().is_empty());
        let mut groups = controller.model().group_specs();
        groups[0].default_directory = Some("relative".into());
        assert_eq!(
            Model::restore_grouped(Vec::new(), groups, None, true, crate::Limits::default()),
            Err(Error::InvalidDirectory)
        );
        controller
            .dispatch(Command::SetWorkspaceGroupDefaultDirectory {
                group,
                directory: None,
            })
            .unwrap();
        let effects = controller
            .dispatch(Command::AddWorkspace {
                cwd: "/provided".into(),
                name: "new".into(),
                group: Some(group),
                remote: None,
            })
            .unwrap();
        assert!(effects.iter().any(|effect| matches!(effect, Effect::StartSession { cwd, .. } if cwd == std::path::Path::new("/provided"))));
    }

    #[test]
    fn grouped_restore_validates_ids_membership_and_retains_collapsed_state() {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        controller
            .dispatch(Command::AddWorkspace {
                cwd: "/fake".into(),
                name: "app".into(),
                remote: None,
                group: Some(group),
            })
            .unwrap();
        controller
            .dispatch(Command::SetWorkspaceGroupCollapsed {
                group,
                collapsed: true,
            })
            .unwrap();
        let specs = controller.model().specs();
        let groups = controller.model().group_specs();
        let restored = Model::restore_grouped(
            specs.clone(),
            groups.clone(),
            controller.model().active_workspace(),
            false,
            crate::Limits::default(),
        )
        .unwrap();
        assert_eq!(restored.group_specs(), groups);
        assert_eq!(restored.workspaces()[0].group(), Some(group));
        assert_eq!(
            Model::restore_grouped(
                specs.clone(),
                Vec::new(),
                None,
                true,
                crate::Limits::default()
            ),
            Err(Error::UnknownWorkspaceGroup(group))
        );
        let mut invalid = groups.clone();
        invalid.push(groups[0].clone());
        assert_eq!(
            Model::restore_grouped(specs.clone(), invalid, None, true, crate::Limits::default()),
            Err(Error::InvalidIdentity)
        );
        let mut invalid = groups;
        invalid[0].id = WorkspaceGroupId::new(0);
        assert_eq!(
            Model::restore_grouped(specs, invalid, None, true, crate::Limits::default()),
            Err(Error::InvalidIdentity)
        );
    }
}
