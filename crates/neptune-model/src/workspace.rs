use crate::{Layout, PaneId, Remote, WorkspaceGroupId, WorkspaceId};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    UnknownWorkspace(WorkspaceId),
    UnknownWorkspaceGroup(WorkspaceGroupId),
    WorkspaceGroupLimit,
    UnknownPane(PaneId),
    UnknownSplit(crate::SplitId),
    WorkspaceLimit,
    PaneLimit,
    TotalPaneLimit,
    InvalidLayout(&'static str),
    InvalidRatio,
    InvalidIdentity,
    InvalidSidebarOrder,
    InvalidName,
    InvalidDirectory,
    InvalidRemote,
    RemoteMismatch,
    IdentityExhausted,
    SpawnLimit,
    SpawnDepth,
    InvalidWorktree,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownWorkspace(id) => write!(f, "Workspace {id} does not exist"),
            Self::UnknownWorkspaceGroup(id) => write!(f, "Workspace group {id} does not exist"),
            Self::WorkspaceGroupLimit => f.write_str("Workspace group limit reached"),
            Self::UnknownPane(id) => {
                write!(f, "Pane {id} does not exist in the targeted workspace")
            }
            Self::UnknownSplit(id) => write!(f, "Split {id} does not exist"),
            Self::WorkspaceLimit => f.write_str("Workspace limit reached"),
            Self::PaneLimit => f.write_str("Workspace pane limit reached"),
            Self::TotalPaneLimit => f.write_str("Total pane limit reached"),
            Self::InvalidLayout(reason) => write!(f, "Invalid layout: {reason}"),
            Self::InvalidRatio => f.write_str("Split ratio must be finite and between 0.1 and 0.9"),
            Self::InvalidIdentity => f.write_str("Identities must be nonzero and unique"),
            Self::InvalidSidebarOrder => f.write_str("Sidebar order must contain every group and ungrouped workspace exactly once"),
            Self::InvalidName => f.write_str("Name cannot be empty"),
            Self::InvalidDirectory => f.write_str("Default directory must be an absolute local path"),
            Self::InvalidRemote => f.write_str(
                "SSH host must be a destination such as user@host, without spaces or a leading dash",
            ),
            Self::RemoteMismatch => f.write_str(
                "A terminal keeps its session, so it cannot move between workspaces on different machines",
            ),
            Self::IdentityExhausted => f.write_str("Identity counter exhausted"),
            Self::SpawnLimit => write!(
                f,
                "An agent can have {} agents it started open at once",
                crate::AgentSession::MAX_SPAWNED
            ),
            Self::SpawnDepth => f.write_str(
                "An agent started by an agent that was itself started by one cannot start another",
            ),
            Self::InvalidWorktree => {
                f.write_str("A worktree needs a local workspace, a branch and its own directory")
            }
        }
    }
}
impl std::error::Error for Error {}

pub(crate) fn validate_default_directory(directory: Option<&Path>) -> Result<(), Error> {
    if let Some(directory) = directory
        && (!directory.is_absolute() || directory.as_os_str().as_encoded_bytes().contains(&0))
    {
        return Err(Error::InvalidDirectory);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub workspaces: usize,
    pub panes_per_workspace: usize,
    pub total_panes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            workspaces: 24,
            panes_per_workspace: 12,
            total_panes: 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lifecycle {
    Starting,
    Running,
    Closing,
    Exited,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub(crate) id: PaneId,
    pub(crate) cwd: PathBuf,
    pub(crate) remote_cwd: Option<PathBuf>,
    pub(crate) agent: Option<crate::AgentSession>,
    pub(crate) pull_requests: Vec<crate::PullRequest>,
    pub(crate) spawned_by: Option<PaneId>,
    pub(crate) worktree: Option<crate::Worktree>,
    pub(crate) generation: u64,
    pub(crate) lifecycle: Lifecycle,
}
impl Pane {
    pub fn agent(&self) -> Option<&crate::AgentSession> {
        self.agent.as_ref()
    }
    /// Pull requests the agent linked to this terminal, oldest first.
    pub fn pull_requests(&self) -> &[crate::PullRequest] {
        &self.pull_requests
    }
    /// The terminal whose agent started this one's, while both agents last.
    pub fn spawned_by(&self) -> Option<PaneId> {
        self.spawned_by
    }
    /// The git worktree Neptune made for this terminal's agent.
    pub fn worktree(&self) -> Option<&crate::Worktree> {
        self.worktree.as_ref()
    }
    pub fn id(&self) -> PaneId {
        self.id
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    /// Last reported directory on the SSH host, independent of the local cwd.
    pub fn remote_cwd(&self) -> Option<&Path> {
        self.remote_cwd.as_deref()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }
}

/// Construction data; model adoption validates all identity and layout invariants.
#[derive(Debug, Clone)]
pub struct PaneSpec {
    pub id: PaneId,
    pub cwd: PathBuf,
    pub remote_cwd: Option<PathBuf>,
    pub agent: Option<crate::AgentSession>,
    pub pull_requests: Vec<crate::PullRequest>,
    /// The terminal whose agent started this one's.
    pub spawned_by: Option<PaneId>,
    /// The git worktree Neptune made for this terminal's agent.
    pub worktree: Option<crate::Worktree>,
}
#[derive(Debug, Clone)]
pub struct WorkspaceSpec {
    pub id: WorkspaceId,
    pub group: Option<WorkspaceGroupId>,
    pub name: String,
    pub cwd: PathBuf,
    /// An SSH destination; validated when the model adopts the workspace.
    pub remote: Option<String>,
    pub panes: Vec<PaneSpec>,
    pub layout: Layout,
    pub active: PaneId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub(crate) id: WorkspaceId,
    pub(crate) group: Option<WorkspaceGroupId>,
    pub(crate) name: String,
    pub(crate) cwd: PathBuf,
    pub(crate) remote: Option<Remote>,
    pub(crate) panes: Vec<Pane>,
    pub(crate) layout: Layout,
    pub(crate) active: PaneId,
}
impl Workspace {
    pub fn id(&self) -> WorkspaceId {
        self.id
    }
    pub fn group(&self) -> Option<WorkspaceGroupId> {
        self.group
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The local directory its terminals start in. A remote workspace runs
    /// its SSH client there; each pane tracks its remote directory separately.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    /// Set when every terminal of this workspace runs on another machine.
    pub fn remote(&self) -> Option<&Remote> {
        self.remote.as_ref()
    }
    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }
    pub fn layout(&self) -> &Layout {
        &self.layout
    }
    pub fn active(&self) -> PaneId {
        self.active
    }
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    /// A terminal that runs without a tab: one an agent started for another
    /// agent, until someone opens it.
    pub fn is_background(&self, pane: PaneId) -> bool {
        self.pane(pane).is_some() && !self.layout.contains(pane)
    }

    /// Gives a background terminal a tab, after the terminal whose agent
    /// started it or else in the focused place. What is in view stays.
    pub(crate) fn reveal(&mut self, pane: PaneId) -> bool {
        if !self.is_background(pane) {
            return false;
        }
        let target = self
            .pane(pane)
            .and_then(|pane| pane.spawned_by)
            .filter(|parent| self.layout.contains(*parent))
            .unwrap_or(self.active);
        let shown = self.layout.tabs(target).map(|(_, shown)| shown);
        if !self.layout.add_tab(target, pane, None) {
            return false;
        }
        if let Some(shown) = shown {
            self.layout.show(shown);
        }
        true
    }

    /// Takes a pane that is leaving out of the layout. Focus on it passes to
    /// whatever is then in view in its place: the tab that followed it, or the
    /// neighbouring tab group once its own is empty. Background terminals
    /// take the place of the last one in view.
    pub(crate) fn remove_from_layout(&mut self, pane: PaneId) -> Result<(), Error> {
        if !self.layout.contains(pane) {
            return Ok(());
        }
        if self.layout.panes().len() == 1 {
            let background: Vec<PaneId> = self
                .panes
                .iter()
                .map(|item| item.id)
                .filter(|id| *id != pane && !self.layout.contains(*id))
                .collect();
            self.active = pane;
            for id in background {
                self.reveal(id);
            }
        }
        let place = self.layout.shown().iter().position(|id| *id == pane);
        self.layout = self
            .layout
            .clone()
            .remove(pane)
            .ok_or(Error::InvalidLayout("removed every leaf"))?;
        if self.active == pane {
            let shown = self.layout.shown();
            self.active = shown[place.unwrap_or(0).min(shown.len() - 1)];
        }
        Ok(())
    }
}

/// Construction data for a sidebar folder. Groups contain workspaces, never groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGroupSpec {
    pub id: WorkspaceGroupId,
    pub name: String,
    pub collapsed: bool,
    pub default_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGroup {
    pub(crate) id: WorkspaceGroupId,
    pub(crate) name: String,
    pub(crate) collapsed: bool,
    pub(crate) default_directory: Option<PathBuf>,
}
impl WorkspaceGroup {
    pub fn id(&self) -> WorkspaceGroupId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn collapsed(&self) -> bool {
        self.collapsed
    }
    /// Starting directory for new local workspaces in this group. Terminals
    /// within each workspace continue to inherit their source pane's directory.
    pub fn default_directory(&self) -> Option<&Path> {
        self.default_directory.as_deref()
    }
}

/// A top-level sidebar entry. Group children are ordered by `Model::workspaces`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SidebarItem {
    Workspace(WorkspaceId),
    Group(WorkspaceGroupId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub(crate) workspaces: Vec<Workspace>,
    pub(crate) groups: Vec<WorkspaceGroup>,
    pub(crate) sidebar_order: Vec<SidebarItem>,
    pub(crate) active: Option<WorkspaceId>,
    pub(crate) sidebar: bool,
    pub(crate) next_workspace: u64,
    pub(crate) next_group: u64,
    pub(crate) next_pane: u64,
    pub(crate) next_split: u64,
    pub(crate) limits: Limits,
}
impl Default for Model {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}
impl Model {
    pub fn new(limits: Limits) -> Self {
        Self {
            workspaces: Vec::new(),
            groups: Vec::new(),
            sidebar_order: Vec::new(),
            active: None,
            sidebar: true,
            next_workspace: 1,
            next_group: 1,
            next_pane: 1,
            next_split: 1,
            limits,
        }
    }
    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }
    pub fn groups(&self) -> &[WorkspaceGroup] {
        &self.groups
    }
    pub fn sidebar_order(&self) -> &[SidebarItem] {
        &self.sidebar_order
    }
    pub fn group(&self, id: WorkspaceGroupId) -> Option<&WorkspaceGroup> {
        self.groups.iter().find(|group| group.id == id)
    }
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }
    pub fn active_workspace(&self) -> Option<WorkspaceId> {
        self.active
    }
    pub fn active_pane(&self) -> Option<PaneId> {
        self.active
            .and_then(|id| self.workspace(id))
            .map(|workspace| workspace.active)
    }
    pub fn sidebar(&self) -> bool {
        self.sidebar
    }
    pub fn limits(&self) -> Limits {
        self.limits
    }
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.workspaces
            .iter()
            .find_map(|workspace| workspace.pane(id))
    }
    /// The terminals whose agents the agent in `parent` started, oldest first.
    pub fn spawned(&self, parent: PaneId) -> impl Iterator<Item = &Pane> {
        self.workspaces
            .iter()
            .flat_map(|workspace| &workspace.panes)
            .filter(move |pane| pane.spawned_by == Some(parent))
    }
    /// How many agents stand between `pane` and one a person started. A chain
    /// longer than the limit, as a loop would be, counts as past it.
    pub(crate) fn spawn_depth(&self, pane: PaneId) -> usize {
        let mut depth = 0;
        let mut next = self.pane(pane).and_then(|pane| pane.spawned_by);
        while let Some(parent) = next {
            depth += 1;
            if depth > crate::AgentSession::MAX_SPAWN_DEPTH {
                break;
            }
            next = self.pane(parent).and_then(|pane| pane.spawned_by);
        }
        depth
    }
    /// Ends every link to a terminal that is gone or no longer runs an agent.
    /// A background terminal whose link ended gets a tab: nothing runs out
    /// of view that no agent answers for.
    pub(crate) fn release_spawned(&mut self) -> bool {
        let orphans: Vec<PaneId> = self
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panes)
            .filter(|pane| {
                pane.spawned_by.is_some_and(|parent| {
                    self.pane(parent)
                        .is_none_or(|parent| parent.agent.is_none())
                })
            })
            .map(|pane| pane.id)
            .collect();
        for pane in &orphans {
            if let Ok(pane) = self.pane_mut(*pane) {
                pane.spawned_by = None;
            }
        }
        let mut changed = !orphans.is_empty();
        for workspace in &mut self.workspaces {
            let unlinked: Vec<PaneId> = workspace
                .panes
                .iter()
                .filter(|pane| pane.spawned_by.is_none())
                .map(|pane| pane.id)
                .collect();
            for pane in unlinked {
                changed |= workspace.reveal(pane);
            }
        }
        changed
    }
    pub fn pane_count(&self) -> usize {
        self.workspaces
            .iter()
            .map(|workspace| workspace.panes.len())
            .sum()
    }
    pub fn workspace_for_pane(&self, id: PaneId) -> Option<WorkspaceId> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.pane(id).is_some())
            .map(|workspace| workspace.id)
    }

    /// Filesystem checks are deliberately left to the persistence/runtime owner.
    /// Validation is atomic: a rejected restore never partially changes a model.
    pub fn restore(
        specs: Vec<WorkspaceSpec>,
        active: Option<WorkspaceId>,
        sidebar: bool,
        limits: Limits,
    ) -> Result<Self, Error> {
        Self::restore_grouped(specs, Vec::new(), active, sidebar, limits)
    }

    /// Restores organization along with workspaces, validating membership atomically.
    /// Empty groups are retained; their count shares the workspace resource limit.
    pub fn restore_grouped(
        specs: Vec<WorkspaceSpec>,
        groups: Vec<WorkspaceGroupSpec>,
        active: Option<WorkspaceId>,
        sidebar: bool,
        limits: Limits,
    ) -> Result<Self, Error> {
        if specs.len() > limits.workspaces {
            return Err(Error::WorkspaceLimit);
        }
        let mut model = Self::new(limits);
        if groups.len() > limits.workspaces {
            return Err(Error::WorkspaceGroupLimit);
        }
        let mut group_ids = HashSet::new();
        for group in groups {
            if group.id.get() == 0 || !group_ids.insert(group.id) {
                return Err(Error::InvalidIdentity);
            }
            if group.name.trim().is_empty() {
                return Err(Error::InvalidName);
            }
            validate_default_directory(group.default_directory.as_deref())?;
            model.next_group = model.next_group.max(
                group
                    .id
                    .get()
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?,
            );
            model.groups.push(WorkspaceGroup {
                id: group.id,
                name: group.name,
                collapsed: group.collapsed,
                default_directory: group.default_directory,
            });
        }
        let mut workspace_ids = HashSet::new();
        let mut pane_ids = HashSet::new();
        let mut split_ids = HashSet::new();
        for spec in specs {
            if let Some(group) = spec.group
                && !group_ids.contains(&group)
            {
                return Err(Error::UnknownWorkspaceGroup(group));
            }
            if spec.id.get() == 0 || !workspace_ids.insert(spec.id) {
                return Err(Error::InvalidIdentity);
            }
            if spec.name.trim().is_empty() {
                return Err(Error::InvalidName);
            }
            let remote = spec.remote.as_deref().map(Remote::parse).transpose()?;
            if spec.panes.is_empty() {
                return Err(Error::InvalidLayout("workspace has no panes"));
            }
            if spec.panes.len() > limits.panes_per_workspace {
                return Err(Error::PaneLimit);
            }
            let mut members = HashSet::new();
            for pane in &spec.panes {
                if pane
                    .agent
                    .as_ref()
                    .is_some_and(|agent| !agent.is_valid() || remote.is_some())
                {
                    return Err(Error::InvalidLayout("invalid agent resume reference"));
                }
                if pane.pull_requests.len() > crate::PullRequest::MAX_PER_PANE
                    || (pane.agent.is_none() && !pane.pull_requests.is_empty())
                {
                    return Err(Error::InvalidLayout("invalid pull request links"));
                }
                if pane
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| !worktree.is_valid() || remote.is_some())
                {
                    return Err(Error::InvalidWorktree);
                }
                if remote.is_none() && pane.remote_cwd.is_some() {
                    return Err(Error::InvalidLayout("local pane has a remote directory"));
                }
                if pane.id.get() == 0 || !pane_ids.insert(pane.id) {
                    return Err(Error::InvalidIdentity);
                }
                members.insert(pane.id);
                model.next_pane = model.next_pane.max(
                    pane.id
                        .get()
                        .checked_add(1)
                        .ok_or(Error::IdentityExhausted)?,
                );
            }
            // Only a terminal an agent started for another runs without a tab.
            let placed: HashSet<PaneId> = spec.layout.panes().into_iter().collect();
            for pane in &spec.panes {
                if pane.spawned_by.is_some() && !placed.contains(&pane.id) {
                    members.remove(&pane.id);
                }
            }
            if !members.contains(&spec.active) {
                return Err(Error::UnknownPane(spec.active));
            }
            spec.layout.validate(&members, &mut split_ids)?;
            // The focused pane is always the tab in view in its place.
            let mut layout = spec.layout;
            layout.show(spec.active);
            model.next_split = model.next_split.max(
                layout
                    .max_split_id()
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?,
            );
            model.next_workspace = model.next_workspace.max(
                spec.id
                    .get()
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)?,
            );
            model.workspaces.push(Workspace {
                id: spec.id,
                group: spec.group,
                name: spec.name,
                cwd: spec.cwd,
                remote,
                panes: spec
                    .panes
                    .into_iter()
                    .map(|pane| Pane {
                        id: pane.id,
                        cwd: pane.cwd,
                        remote_cwd: pane.remote_cwd,
                        agent: pane.agent,
                        pull_requests: pane.pull_requests,
                        spawned_by: pane.spawned_by,
                        worktree: pane.worktree,
                        generation: 1,
                        lifecycle: Lifecycle::Starting,
                    })
                    .collect(),
                layout,
                active: spec.active,
            });
        }
        if model.pane_count() > limits.total_panes {
            return Err(Error::TotalPaneLimit);
        }
        // An agent is another's only while both can be resumed, and the
        // chain ends: no terminal is its own ancestor.
        for pane in model
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panes)
        {
            let Some(parent) = pane.spawned_by else {
                continue;
            };
            if pane.agent.is_none()
                || model
                    .pane(parent)
                    .is_none_or(|parent| parent.agent.is_none())
                || model.spawn_depth(pane.id) > crate::AgentSession::MAX_SPAWN_DEPTH
                || model.spawned(parent).count() > crate::AgentSession::MAX_SPAWNED
            {
                return Err(Error::InvalidLayout("invalid spawned agent"));
            }
        }
        if let Some(id) = active
            && model.workspace(id).is_none()
        {
            return Err(Error::UnknownWorkspace(id));
        }
        model.order_workspaces();
        model.active = active.or_else(|| model.workspaces.first().map(Workspace::id));
        model.sidebar = sidebar;
        Ok(model)
    }

    pub fn specs(&self) -> Vec<WorkspaceSpec> {
        self.workspaces
            .iter()
            .map(|workspace| WorkspaceSpec {
                id: workspace.id,
                group: workspace.group,
                name: workspace.name.clone(),
                cwd: workspace.cwd.clone(),
                remote: workspace
                    .remote
                    .as_ref()
                    .map(|remote| remote.destination().to_owned()),
                panes: workspace
                    .panes
                    .iter()
                    .map(|pane| PaneSpec {
                        id: pane.id,
                        cwd: pane.cwd.clone(),
                        remote_cwd: pane.remote_cwd.clone(),
                        agent: pane.agent.clone(),
                        pull_requests: pane.pull_requests.clone(),
                        spawned_by: pane.spawned_by,
                        worktree: pane.worktree.clone(),
                    })
                    .collect(),
                layout: workspace.layout.clone(),
                active: workspace.active,
            })
            .collect()
    }

    /// Restores a mixed top-level order, or the historical root-first order
    /// when loading a format that predates sidebar ordering.
    pub fn restore_ordered(
        specs: Vec<WorkspaceSpec>,
        groups: Vec<WorkspaceGroupSpec>,
        order: Option<Vec<SidebarItem>>,
        active: Option<WorkspaceId>,
        sidebar: bool,
        limits: Limits,
    ) -> Result<Self, Error> {
        let mut model = Self::restore_grouped(specs, groups, active, sidebar, limits)?;
        if let Some(order) = order {
            let expected: HashSet<_> = model.sidebar_order.iter().copied().collect();
            if order.len() != expected.len()
                || order.iter().copied().collect::<HashSet<_>>() != expected
            {
                return Err(Error::InvalidSidebarOrder);
            }
            model.sidebar_order = order;
            model.order_workspaces();
            model.active = active.or_else(|| model.workspaces.first().map(Workspace::id));
        }
        Ok(model)
    }

    pub fn group_specs(&self) -> Vec<WorkspaceGroupSpec> {
        self.groups
            .iter()
            .map(|group| WorkspaceGroupSpec {
                id: group.id,
                name: group.name.clone(),
                collapsed: group.collapsed,
                default_directory: group.default_directory.clone(),
            })
            .collect()
    }

    pub(crate) fn group_mut(&mut self, id: WorkspaceGroupId) -> Result<&mut WorkspaceGroup, Error> {
        self.groups
            .iter_mut()
            .find(|group| group.id == id)
            .ok_or(Error::UnknownWorkspaceGroup(id))
    }
    /// Reconcile created/closed entries and keep navigation identical to the
    /// sidebar, including children of collapsed groups. Stable sorting retains
    /// the order within a group; terminal/session state is untouched.
    pub(crate) fn order_workspaces(&mut self) {
        let roots: Vec<_> = self
            .workspaces
            .iter()
            .filter(|w| w.group.is_none())
            .map(|w| SidebarItem::Workspace(w.id))
            .collect();
        let folders: Vec<_> = self
            .groups
            .iter()
            .map(|g| SidebarItem::Group(g.id))
            .collect();
        self.sidebar_order
            .retain(|item| roots.contains(item) || folders.contains(item));
        for item in roots {
            if !self.sidebar_order.contains(&item) {
                // New ungrouped workspaces follow the last ungrouped workspace.
                let index = self
                    .sidebar_order
                    .iter()
                    .rposition(|entry| matches!(entry, SidebarItem::Workspace(_)))
                    .map_or(0, |index| index + 1);
                self.sidebar_order.insert(index, item);
            }
        }
        for item in folders {
            if !self.sidebar_order.contains(&item) {
                self.sidebar_order.push(item);
            }
        }
        let order = &self.sidebar_order;
        self.groups.sort_by_key(|group| {
            order
                .iter()
                .position(|item| *item == SidebarItem::Group(group.id))
        });
        self.workspaces.sort_by_key(|workspace| {
            let entry = workspace
                .group
                .map_or(SidebarItem::Workspace(workspace.id), SidebarItem::Group);
            order.iter().position(|item| *item == entry)
        });
    }

    pub(crate) fn workspace_mut(&mut self, id: WorkspaceId) -> Result<&mut Workspace, Error> {
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)
            .ok_or(Error::UnknownWorkspace(id))
    }
    pub(crate) fn pane_mut(&mut self, id: PaneId) -> Result<&mut Pane, Error> {
        self.workspaces
            .iter_mut()
            .flat_map(|workspace| workspace.panes.iter_mut())
            .find(|pane| pane.id == id)
            .ok_or(Error::UnknownPane(id))
    }
}
