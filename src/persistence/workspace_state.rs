use neptune_model::{
    Axis, Layout, Limits, Model, PaneId, PaneSpec, SidebarItem, SplitId, WorkspaceGroupId,
    WorkspaceGroupSpec, WorkspaceId, WorkspaceSpec,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
};

/// Version 11 adds the files an agent attached to its pane. Version 10 added
/// the git worktree made for a pane's agent, and OpenCode, Gemini CLI, pi and
/// Oh My Pi to the agents a pane's resume reference can name, which an
/// earlier build would take for damage. Version 9 added the pane whose agent
/// started a pane's agent. Version 8 added workspace group default
/// directories and the pull requests an agent linked to its pane. Versions
/// 1–10 remain readable.
pub const SCHEMA_VERSION: u32 = 11;
const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;

/// This DTO is the disk contract. Runtime layout serialization cannot change it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateSnapshot {
    pub version: u32,
    pub workspaces: Vec<SavedWorkspace>,
    #[serde(default)]
    pub groups: Vec<SavedWorkspaceGroup>,
    #[serde(default)]
    pub sidebar_order: Option<Vec<SavedSidebarItem>>,
    pub active: Option<WorkspaceId>,
    pub sidebar: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedSidebarItem {
    Workspace { workspace: WorkspaceId },
    Group { group: WorkspaceGroupId },
}

impl From<SavedSidebarItem> for SidebarItem {
    fn from(item: SavedSidebarItem) -> Self {
        match item {
            SavedSidebarItem::Workspace { workspace } => Self::Workspace(workspace),
            SavedSidebarItem::Group { group } => Self::Group(group),
        }
    }
}

impl From<SidebarItem> for SavedSidebarItem {
    fn from(item: SidebarItem) -> Self {
        match item {
            SidebarItem::Workspace(workspace) => Self::Workspace { workspace },
            SidebarItem::Group(group) => Self::Group { group },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedWorkspaceGroup {
    pub id: WorkspaceGroupId,
    pub name: String,
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_directory: Option<PathBuf>,
}
impl SavedWorkspaceGroup {
    fn into_spec(self) -> WorkspaceGroupSpec {
        WorkspaceGroupSpec {
            id: self.id,
            name: self.name,
            collapsed: self.collapsed,
            default_directory: self.default_directory,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedWorkspace {
    pub id: WorkspaceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<WorkspaceGroupId>,
    pub name: String,
    pub cwd: PathBuf,
    /// SSH destination of a remote workspace. Never a credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<String>,
    pub panes: Vec<SavedPane>,
    pub layout: SavedLayout,
    pub active: PaneId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedPane {
    pub id: PaneId,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<neptune_model::AgentSession>,
    /// Addresses of the pull requests the agent linked, from schema version 8.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pull_requests: Vec<String>,
    /// The pane whose agent started this pane's agent, from schema version 9.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<PaneId>,
    /// The git worktree made for the pane's agent, from schema version 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<neptune_model::Worktree>,
    /// The files the agent attached, from schema version 11.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SavedAttachment>,
}

/// Where an attached file is and what the agent called it; never its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedAttachment {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}
impl SavedAttachment {
    fn restore(&self) -> Option<neptune_model::Attachment> {
        neptune_model::Attachment::new(self.path.clone(), self.title.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedLayout {
    Pane {
        pane: PaneId,
    },
    /// Several terminals in one place, written from schema version 7.
    Tabs {
        panes: Vec<PaneId>,
        shown: PaneId,
    },
    Split {
        id: SplitId,
        axis: SavedAxis,
        ratio: f32,
        first: Box<SavedLayout>,
        second: Box<SavedLayout>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedAxis {
    Vertical,
    Horizontal,
}

impl StateSnapshot {
    pub fn from_model(model: &Model) -> Self {
        Self {
            version: SCHEMA_VERSION,
            workspaces: model
                .workspaces()
                .iter()
                .map(|workspace| SavedWorkspace {
                    id: workspace.id(),
                    group: workspace.group(),
                    name: workspace.name().into(),
                    cwd: workspace.cwd().into(),
                    ssh: workspace
                        .remote()
                        .map(|remote| remote.destination().to_owned()),
                    panes: workspace
                        .panes()
                        .iter()
                        .map(|pane| SavedPane {
                            id: pane.id(),
                            cwd: pane.cwd().into(),
                            remote_cwd: pane.remote_cwd().map(Path::to_path_buf),
                            agent: pane.agent().cloned(),
                            pull_requests: pane
                                .pull_requests()
                                .iter()
                                .map(|link| link.url().to_owned())
                                .collect(),
                            // An agent that has not opened yet cannot be
                            // resumed, so neither can its link.
                            spawned_by: pane.spawned_by().filter(|_| pane.agent().is_some()),
                            worktree: pane.worktree().cloned(),
                            attachments: pane
                                .attachments()
                                .iter()
                                .map(|file| SavedAttachment {
                                    path: file.path().into(),
                                    title: file.title().map(str::to_owned),
                                })
                                .collect(),
                        })
                        .collect(),
                    layout: SavedLayout::from_layout(workspace.layout()),
                    active: workspace.active(),
                })
                .collect(),
            groups: model
                .groups()
                .iter()
                .map(|group| SavedWorkspaceGroup {
                    id: group.id(),
                    name: group.name().into(),
                    collapsed: group.collapsed(),
                    default_directory: group.default_directory().map(Path::to_path_buf),
                })
                .collect(),
            active: model.active_workspace(),
            sidebar: model.sidebar(),
            sidebar_order: Some(
                model
                    .sidebar_order()
                    .iter()
                    .copied()
                    .map(Into::into)
                    .collect(),
            ),
        }
    }

    /// Conversion enforces the same invariants as new commands without checking
    /// directories; callers can use it for headless snapshots and round trips.
    pub fn into_model(self, limits: Limits) -> Result<Model, neptune_model::Error> {
        Model::restore_ordered(
            self.workspaces
                .into_iter()
                .map(SavedWorkspace::into_spec)
                .collect(),
            self.groups
                .into_iter()
                .map(SavedWorkspaceGroup::into_spec)
                .collect(),
            self.sidebar_order
                .map(|items| items.into_iter().map(Into::into).collect()),
            self.active,
            self.sidebar,
            limits,
        )
    }
}

impl SavedWorkspace {
    fn into_spec(self) -> WorkspaceSpec {
        WorkspaceSpec {
            group: self.group,
            id: self.id,
            name: self.name,
            cwd: self.cwd,
            remote: self.ssh,
            panes: self
                .panes
                .into_iter()
                .map(|pane| PaneSpec {
                    id: pane.id,
                    cwd: pane.cwd,
                    remote_cwd: pane.remote_cwd,
                    pull_requests: pane
                        .pull_requests
                        .iter()
                        .filter_map(|url| neptune_model::PullRequest::parse(url))
                        .collect(),
                    spawned_by: pane.spawned_by,
                    worktree: pane.worktree,
                    attachments: pane
                        .attachments
                        .iter()
                        .filter_map(SavedAttachment::restore)
                        .collect(),
                    agent: pane.agent,
                })
                .collect(),
            layout: self.layout.into_layout(),
            active: self.active,
        }
    }
}

impl SavedLayout {
    fn from_layout(layout: &Layout) -> Self {
        match layout {
            Layout::Tabs { panes, shown } => match panes[..] {
                [pane] => Self::Pane { pane },
                _ => Self::Tabs {
                    panes: panes.clone(),
                    shown: *shown,
                },
            },
            Layout::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => Self::Split {
                id: *id,
                axis: match axis {
                    Axis::Vertical => SavedAxis::Vertical,
                    Axis::Horizontal => SavedAxis::Horizontal,
                },
                ratio: *ratio,
                first: Box::new(Self::from_layout(first)),
                second: Box::new(Self::from_layout(second)),
            },
        }
    }
    fn into_layout(self) -> Layout {
        match self {
            Self::Pane { pane } => Layout::pane(pane),
            Self::Tabs { panes, shown } => Layout::Tabs { panes, shown },
            Self::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => Layout::Split {
                id,
                axis: match axis {
                    SavedAxis::Vertical => Axis::Vertical,
                    SavedAxis::Horizontal => Axis::Horizontal,
                },
                ratio,
                first: Box::new(first.into_layout()),
                second: Box::new(second.into_layout()),
            },
        }
    }
}

#[derive(Debug)]
pub struct LoadReport {
    pub model: Option<Model>,
    pub diagnostics: Vec<String>,
    /// False for unreadable state, unsupported formats or failed recovery copy.
    /// The desktop may continue with defaults, but must not save over this file.
    pub can_write: bool,
    pub migrated: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyState {
    workspaces: Vec<LegacyWorkspace>,
    active: usize,
    sidebar: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyWorkspace {
    name: String,
    cwd: PathBuf,
    #[serde(default)]
    layout: Option<LegacyLayout>,
    #[serde(default)]
    panes: Vec<LegacyPane>,
    #[serde(default)]
    active: u64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyPane {
    id: u64,
    cwd: PathBuf,
}
#[derive(Debug, Deserialize)]
enum LegacyLayout {
    Leaf(u64),
    Split {
        vertical: bool,
        ratio: f32,
        first: Box<LegacyLayout>,
        second: Box<LegacyLayout>,
    },
}

pub fn load_state(path: &Path, limits: Limits) -> LoadReport {
    let mut report = LoadReport {
        model: None,
        diagnostics: Vec::new(),
        can_write: true,
        migrated: false,
    };
    let bytes = match read_state(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return report,
        Err(error) => {
            report.can_write = false;
            report.diagnostics.push(format!(
                "Cannot read workspace state {}: {error}; saving this state file is disabled",
                path.display()
            ));
            return report;
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            report.diagnostics.push(format!(
                "Invalid workspace state {}: {error}",
                path.display()
            ));
            preserve_original(path, &bytes, &mut report);
            return report;
        }
    };
    if let Some(version) = value.get("version") {
        let Some(version_number) = version.as_u64() else {
            report.diagnostics.push(format!(
                "Invalid workspace schema version {version} in {}",
                path.display()
            ));
            preserve_original(path, &bytes, &mut report);
            return report;
        };
        if !(1..=u64::from(SCHEMA_VERSION)).contains(&version_number) {
            report.can_write = false;
            report.diagnostics.push(format!("Unsupported workspace schema version {version} in {}; preserving original and disabling state saves", path.display()));
            return report;
        }
        match serde_json::from_value::<StateSnapshot>(value) {
            Ok(snapshot) => {
                report.migrated = snapshot.version != SCHEMA_VERSION;
                restore_versioned(snapshot, limits, &mut report);
            }
            Err(error) => report.diagnostics.push(format!(
                "Invalid workspace schema in {}: {error}",
                path.display()
            )),
        }
    } else {
        match serde_json::from_value::<LegacyState>(value) {
            Ok(legacy) => {
                report.migrated = true;
                restore_legacy(legacy, limits, &mut report);
            }
            Err(error) => report.diagnostics.push(format!(
                "Invalid legacy workspace state in {}: {error}",
                path.display()
            )),
        }
    }
    // Migration itself preserves semantics. Invalid/missing entries or a broken
    // schema require a recovery copy before any startup save can replace them.
    if !report.diagnostics.is_empty() {
        preserve_original(path, &bytes, &mut report);
    }
    report
}

fn read_state(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "workspace state exceeds the 8 MiB restoration limit",
        ));
    }
    Ok(bytes)
}

fn restore_versioned(snapshot: StateSnapshot, limits: Limits, report: &mut LoadReport) {
    let requested_active = snapshot.active;
    let mut groups = Vec::new();
    for group in snapshot.groups {
        if let Some(directory) = &group.default_directory
            && !directory.is_dir()
        {
            report.diagnostics.push(format!(
                "Workspace group {:?}: default directory {} is unavailable; retained it so it can be changed before creating new workspaces",
                group.name, directory.display()
            ));
        }
        let name = group.name.clone();
        let mut candidate = groups.clone();
        candidate.push(group.into_spec());
        match Model::restore_grouped(
            Vec::new(),
            candidate.clone(),
            None,
            snapshot.sidebar,
            limits,
        ) {
            Ok(_) => groups = candidate,
            Err(error) => report
                .diagnostics
                .push(format!("Skipped invalid workspace group {name:?}: {error}")),
        }
    }
    let mut specs = Vec::new();
    let mut total = 0;
    // Links between agents cross workspaces, so they are checked once every
    // workspace has been; until then each workspace is validated without them.
    let mut spawned = Vec::new();
    let mut background = Vec::new();
    for mut workspace in snapshot.workspaces {
        if let Some(group) = workspace.group
            && !groups.iter().any(|folder| folder.id == group)
        {
            report.diagnostics.push(format!(
                "Workspace {:?}: group {group} was unavailable; restored ungrouped",
                workspace.name
            ));
            workspace.group = None;
        }
        if !workspace.cwd.is_dir() {
            report.diagnostics.push(format!(
                "Skipped workspace {:?}: directory {} does not exist",
                workspace.name,
                workspace.cwd.display()
            ));
            continue;
        }
        if specs.len() >= limits.workspaces || total + workspace.panes.len() > limits.total_panes {
            report.diagnostics.push(format!(
                "Skipped workspace {:?}: configured resource limit exceeded",
                workspace.name
            ));
            continue;
        }
        for pane in &mut workspace.panes {
            if pane
                .agent
                .as_ref()
                .is_some_and(|a| !a.is_valid() || !a.cwd.is_dir() || workspace.ssh.is_some())
            {
                report.diagnostics.push(format!(
                    "Pane {}: unavailable agent resume reference; opening a shell",
                    pane.id
                ));
                pane.agent = None;
            }
            let links = pane.pull_requests.len();
            pane.pull_requests
                .retain(|url| neptune_model::PullRequest::parse(url).is_some());
            pane.pull_requests
                .truncate(neptune_model::PullRequest::MAX_PER_PANE);
            if pane.agent.is_none() {
                pane.pull_requests.clear();
            }
            if pane.pull_requests.len() != links {
                report.diagnostics.push(format!(
                    "Pane {}: left out pull request links that could not be restored",
                    pane.id
                ));
            }
            if pane.worktree.as_ref().is_some_and(|worktree| {
                !worktree.is_valid() || !worktree.path.is_dir() || workspace.ssh.is_some()
            }) {
                report.diagnostics.push(format!(
                    "Pane {}: its worktree is unavailable; opening an ordinary terminal",
                    pane.id
                ));
                pane.worktree = None;
            }
            // A file that has since been moved or deleted stays listed: the
            // list says so when it is opened, and reading it here would not
            // keep it true.
            let files = pane.attachments.len();
            pane.attachments.retain(|file| file.restore().is_some());
            pane.attachments
                .truncate(neptune_model::Attachment::MAX_PER_PANE);
            if pane.agent.is_none() {
                pane.attachments.clear();
            }
            if pane.attachments.len() != files {
                report.diagnostics.push(format!(
                    "Pane {}: left out attached files that could not be restored",
                    pane.id
                ));
            }
            if !pane.cwd.is_dir() {
                report.diagnostics.push(format!(
                    "Pane {} in {:?}: missing directory {}; using {}",
                    pane.id,
                    workspace.name,
                    pane.cwd.display(),
                    workspace.cwd.display()
                ));
                pane.cwd.clone_from(&workspace.cwd);
            }
            if let Some(parent) = pane.spawned_by.take() {
                spawned.push((pane.id, parent));
            }
        }
        let name = workspace.name.clone();
        let mut spec = workspace.into_spec();
        // A terminal saved without a tab is checked with one, and loses it
        // again once the agent that started its agent is found.
        let placed = spec.layout.panes();
        for pane in &spec.panes {
            if !placed.contains(&pane.id) && park(&mut spec.layout, Some(spec.active), pane.id) {
                background.push(pane.id);
            }
        }
        let mut candidate = specs.clone();
        candidate.push(spec.clone());
        match Model::restore_grouped(candidate, groups.clone(), None, snapshot.sidebar, limits) {
            Ok(_) => {
                total += spec.panes.len();
                specs.push(spec);
            }
            Err(error) => report
                .diagnostics
                .push(format!("Skipped invalid workspace {name:?}: {error}")),
        }
    }
    for (pane, parent) in spawned {
        let mut linked = specs.clone();
        if let Some(spec) = linked
            .iter_mut()
            .flat_map(|workspace| &mut workspace.panes)
            .find(|spec| spec.id == pane)
        {
            spec.spawned_by = Some(parent);
        }
        if background.contains(&pane) {
            for workspace in &mut linked {
                unpark(&mut workspace.layout, pane);
            }
        }
        match Model::restore_grouped(
            linked.clone(),
            groups.clone(),
            None,
            snapshot.sidebar,
            limits,
        ) {
            Ok(_) => specs = linked,
            Err(_) => report.diagnostics.push(format!(
                "Pane {pane}: the agent that started its agent was unavailable; restored on its own"
            )),
        }
    }
    let active = requested_active.filter(|id| specs.iter().any(|workspace| workspace.id == *id));
    if requested_active.is_some() && active.is_none() {
        report.diagnostics.push(
            "Restored active workspace was unavailable; selected the first remaining workspace"
                .into(),
        );
    }
    match Model::restore_grouped(
        specs.clone(),
        groups.clone(),
        active,
        snapshot.sidebar,
        limits,
    ) {
        Ok(model) => {
            let order = snapshot.sidebar_order.map(|saved| {
                let mut order = Vec::new();
                for item in saved.into_iter().map(SidebarItem::from) {
                    if model.sidebar_order().contains(&item) && !order.contains(&item) {
                        order.push(item);
                    } else {
                        report.diagnostics.push(format!(
                            "Skipped unavailable or duplicate sidebar entry {item:?}"
                        ));
                    }
                }
                for item in model.sidebar_order() {
                    if !order.contains(item) {
                        report
                            .diagnostics
                            .push(format!("Restored missing sidebar entry {item:?}"));
                        order.push(*item);
                    }
                }
                order
            });
            if snapshot.version >= 6 && order.is_none() {
                report
                    .diagnostics
                    .push("Missing sidebar order; restored the historical workspace order".into());
            }
            match Model::restore_ordered(specs, groups, order, active, snapshot.sidebar, limits) {
                Ok(model) => report.model = Some(model),
                Err(error) => report
                    .diagnostics
                    .push(format!("Sidebar order validation failed: {error}")),
            }
        }
        Err(error) => report
            .diagnostics
            .push(format!("Workspace state validation failed: {error}")),
    }
}

/// Adds `pane` as the last tab beside `anchor`, or of the first place when
/// the anchor has none.
fn park(layout: &mut Layout, anchor: Option<PaneId>, pane: PaneId) -> bool {
    match layout {
        Layout::Tabs { panes, .. } => {
            let here = anchor.is_none_or(|anchor| panes.contains(&anchor));
            if here {
                panes.push(pane);
            }
            here
        }
        Layout::Split { first, second, .. } => {
            park(first, anchor, pane)
                || park(second, anchor, pane)
                || (anchor.is_some() && park(first, None, pane))
        }
    }
}
/// Takes back the tab `park` gave; another tab is in view in its place.
fn unpark(layout: &mut Layout, pane: PaneId) {
    match layout {
        Layout::Tabs { panes, shown } => {
            if *shown != pane {
                panes.retain(|id| *id != pane);
            }
        }
        Layout::Split { first, second, .. } => {
            unpark(first, pane);
            unpark(second, pane);
        }
    }
}

fn restore_legacy(legacy: LegacyState, limits: Limits, report: &mut LoadReport) {
    let mut specs = Vec::new();
    let mut next_pane = 1u64;
    let mut next_split = 1u64;
    let mut active = None;
    let mut total = 0;
    for (position, workspace) in legacy.workspaces.into_iter().enumerate() {
        if !workspace.cwd.is_dir() {
            report.diagnostics.push(format!(
                "Skipped workspace {:?}: directory {} does not exist",
                workspace.name,
                workspace.cwd.display()
            ));
            continue;
        }
        if specs.len() >= limits.workspaces || total >= limits.total_panes {
            report.diagnostics.push(format!(
                "Skipped workspace {:?}: configured resource limit exceeded",
                workspace.name
            ));
            continue;
        }
        if workspace.name.trim().is_empty() {
            report
                .diagnostics
                .push("Skipped legacy workspace with an empty name".into());
            continue;
        }
        let id = WorkspaceId::new(position as u64 + 1);
        let mut mapping = HashMap::new();
        let mut panes = Vec::new();
        for pane in workspace.panes {
            if panes.len() >= limits.panes_per_workspace
                || total + panes.len() >= limits.total_panes
            {
                report.diagnostics.push(format!(
                    "Workspace {:?}: omitted panes beyond configured resource limit",
                    workspace.name
                ));
                break;
            }
            if mapping.contains_key(&pane.id) {
                report.diagnostics.push(format!(
                    "Workspace {:?}: omitted duplicate legacy pane {}",
                    workspace.name, pane.id
                ));
                continue;
            }
            let pane_id = PaneId::new(next_pane);
            next_pane += 1;
            mapping.insert(pane.id, pane_id);
            let cwd = if pane.cwd.is_dir() {
                pane.cwd
            } else {
                report.diagnostics.push(format!(
                    "Pane {} in {:?}: missing directory {}; using {}",
                    pane.id,
                    workspace.name,
                    pane.cwd.display(),
                    workspace.cwd.display()
                ));
                workspace.cwd.clone()
            };
            panes.push(PaneSpec {
                id: pane_id,
                cwd,
                remote_cwd: None,
                agent: None,
                pull_requests: Vec::new(),
                attachments: Vec::new(),
                spawned_by: None,
                worktree: None,
            });
        }
        if panes.is_empty() {
            if limits.panes_per_workspace == 0 {
                report.diagnostics.push(format!(
                    "Skipped workspace {:?}: pane limit is zero",
                    workspace.name
                ));
                continue;
            }
            panes.push(PaneSpec {
                id: PaneId::new(next_pane),
                cwd: workspace.cwd.clone(),
                remote_cwd: None,
                agent: None,
                pull_requests: Vec::new(),
                attachments: Vec::new(),
                spawned_by: None,
                worktree: None,
            });
            next_pane += 1;
        }
        let mut layout = Layout::pane(panes[0].id);
        for pane in panes.iter().skip(1) {
            layout = Layout::Split {
                id: SplitId::new(next_split),
                axis: Axis::Vertical,
                ratio: 0.5,
                first: Box::new(layout),
                second: Box::new(Layout::pane(pane.id)),
            };
            next_split += 1;
        }
        if let Some(saved_layout) = workspace.layout {
            let mut seen = HashSet::new();
            match migrate_layout(saved_layout, &mapping, &mut seen, &mut next_split) {
                Some(restored) if seen.len() == panes.len() => layout = restored,
                _ => report.diagnostics.push(format!(
                    "Workspace {:?}: repaired invalid legacy split layout",
                    workspace.name
                )),
            }
        }
        let selected = mapping
            .get(&workspace.active)
            .copied()
            .unwrap_or(panes[0].id);
        total += panes.len();
        if position == legacy.active {
            active = Some(id);
        }
        specs.push(WorkspaceSpec {
            group: None,
            id,
            name: workspace.name,
            cwd: workspace.cwd,
            remote: None,
            panes,
            layout,
            active: selected,
        });
    }
    match Model::restore(specs, active, legacy.sidebar, limits) {
        Ok(model) => report.model = Some(model),
        Err(error) => report
            .diagnostics
            .push(format!("Legacy workspace validation failed: {error}")),
    }
}

fn migrate_layout(
    layout: LegacyLayout,
    mapping: &HashMap<u64, PaneId>,
    seen: &mut HashSet<PaneId>,
    next_split: &mut u64,
) -> Option<Layout> {
    match layout {
        LegacyLayout::Leaf(old) => {
            let pane = *mapping.get(&old)?;
            seen.insert(pane).then_some(Layout::pane(pane))
        }
        LegacyLayout::Split {
            vertical,
            ratio,
            first,
            second,
        } => {
            if !ratio.is_finite() || !(0.1..=0.9).contains(&ratio) {
                return None;
            }
            let id = SplitId::new(*next_split);
            *next_split += 1;
            Some(Layout::Split {
                id,
                axis: if vertical {
                    Axis::Vertical
                } else {
                    Axis::Horizontal
                },
                ratio,
                first: Box::new(migrate_layout(*first, mapping, seen, next_split)?),
                second: Box::new(migrate_layout(*second, mapping, seen, next_split)?),
            })
        }
    }
}

fn preserve_original(path: &Path, bytes: &[u8], report: &mut LoadReport) {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let result = (|| -> std::io::Result<PathBuf> {
        let mut backup = tempfile::Builder::new()
            .prefix("neptune-workspaces-recovery-")
            .suffix(".json")
            .tempfile_in(parent)?;
        backup.write_all(bytes)?;
        backup.as_file().sync_all()?;
        let (_file, backup_path) = backup.keep().map_err(|error| error.error)?;
        Ok(backup_path)
    })();
    match result {
        Ok(backup) => report.diagnostics.push(format!(
            "Original workspace state preserved at {}",
            backup.display()
        )),
        Err(error) => {
            report.can_write = false;
            report.diagnostics.push(format!(
                "Could not preserve {}: {error}; state saving is disabled",
                path.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neptune_model::{Command, Controller};

    fn fixture(directory: &Path) -> Vec<u8> {
        include_str!("fixtures/workspaces-unversioned.json")
            .replace(
                "@DIRECTORY@",
                &directory.to_string_lossy().replace('\\', "\\\\"),
            )
            .into_bytes()
    }
    pub(super) fn sample(directory: &Path) -> StateSnapshot {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: directory.into(),
                name: "fixture".into(),
                remote: None,
            })
            .unwrap();
        let workspace = controller.model().active_workspace().unwrap();
        let pane = controller.model().active_pane().unwrap();
        controller
            .dispatch(Command::SplitPane {
                workspace,
                pane,
                axis: Axis::Vertical,
                cwd: directory.into(),
            })
            .unwrap();
        StateSnapshot::from_model(controller.model())
    }

    #[test]
    fn agent_references_round_trip_and_invalid_references_preserve_recovery_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspaces.json");
        let mut snapshot = sample(root.path());
        let reference = neptune_model::AgentSession {
            kind: neptune_model::AgentKind::Codex,
            session_id: Some("019a1234-5678-7000-8000-123456789abc".into()),
            cwd: root.path().into(),
        };
        snapshot.workspaces[0].panes[0].agent = Some(reference.clone());
        snapshot.workspaces[0].panes[0].pull_requests =
            vec!["https://github.com/zevem/neptune/pull/83".into()];
        // A file that is gone by now is still listed where it was.
        let attached = SavedAttachment {
            path: root.path().join("after.png"),
            title: Some("After".into()),
        };
        snapshot.workspaces[0].panes[0].attachments = vec![attached.clone()];
        std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        let model = report.model.unwrap();
        let panes = model.workspaces()[0].panes();
        assert_eq!(panes[0].agent(), Some(&reference));
        assert_eq!(
            StateSnapshot::from_model(&model).workspaces[0].panes[0].attachments,
            std::slice::from_ref(&attached)
        );
        assert_eq!(
            StateSnapshot::from_model(&model).workspaces[0].panes[0].pull_requests,
            ["https://github.com/zevem/neptune/pull/83"]
        );
        // Unreadable links, and links without their agent, are left out.
        snapshot.workspaces[0].panes[0]
            .pull_requests
            .push("javascript:alert(1)".into());
        snapshot.workspaces[0].panes[1].pull_requests =
            vec!["https://github.com/zevem/neptune/pull/84".into()];
        // So are files that are not absolute paths, or without their agent.
        snapshot.workspaces[0].panes[0]
            .attachments
            .push(SavedAttachment {
                path: "relative.png".into(),
                title: None,
            });
        snapshot.workspaces[0].panes[1].attachments = vec![attached.clone()];
        std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        // Two notes for each pane, and one for the recovery copy.
        assert_eq!(report.diagnostics.len(), 5, "{:?}", report.diagnostics);
        let model = report.model.unwrap();
        let panes = model.workspaces()[0].panes();
        assert_eq!(panes[0].pull_requests().len(), 1);
        assert!(panes[1].pull_requests().is_empty());
        assert_eq!(panes[0].attachments().len(), 1);
        assert!(panes[1].attachments().is_empty());
        // A link between two resumable agents returns; one to a terminal
        // without an agent, to itself or to a missing terminal does not.
        let (first, second) = (
            snapshot.workspaces[0].panes[0].id,
            snapshot.workspaces[0].panes[1].id,
        );
        let mut linked = snapshot.clone();
        linked.workspaces[0].panes[0].pull_requests.pop();
        linked.workspaces[0].panes[1].pull_requests.clear();
        linked.workspaces[0].panes[0].attachments.pop();
        linked.workspaces[0].panes[1].attachments.clear();
        linked.workspaces[0].panes[1].agent = Some(reference.clone());
        linked.workspaces[0].panes[1].spawned_by = Some(first);
        std::fs::write(&path, serde_json::to_vec(&linked).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        let model = report.model.unwrap();
        assert_eq!(model.pane(second).unwrap().spawned_by(), Some(first));
        assert_eq!(
            StateSnapshot::from_model(&model).workspaces[0].panes[1].spawned_by,
            Some(first)
        );
        // A started agent's terminal without a tab returns without one, and
        // with one when its link does not.
        let mut hidden = linked.clone();
        hidden.workspaces[0].layout = SavedLayout::Pane { pane: first };
        hidden.workspaces[0].active = first;
        std::fs::write(&path, serde_json::to_vec(&hidden).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        let model = report.model.unwrap();
        assert!(model.workspaces()[0].is_background(second));
        assert!(matches!(
            &StateSnapshot::from_model(&model).workspaces[0].layout,
            SavedLayout::Pane { pane } if *pane == first
        ));
        hidden.workspaces[0].panes[1].spawned_by = Some(PaneId::new(99));
        std::fs::write(&path, serde_json::to_vec(&hidden).unwrap()).unwrap();
        let model = load_state(&path, Limits::default()).model.unwrap();
        assert_eq!(model.workspaces()[0].layout().panes(), [first, second]);
        assert_eq!(model.workspaces()[0].layout().shown(), [first]);
        for (child, parent, agent) in [
            (1, first, None),
            (1, second, Some(reference.clone())),
            (1, PaneId::new(99), Some(reference.clone())),
        ] {
            let mut broken = linked.clone();
            broken.workspaces[0].panes[child].agent = agent;
            broken.workspaces[0].panes[child].spawned_by = Some(parent);
            std::fs::write(&path, serde_json::to_vec(&broken).unwrap()).unwrap();
            let report = load_state(&path, Limits::default());
            assert_eq!(report.diagnostics.len(), 2, "{:?}", report.diagnostics);
            let model = report.model.unwrap();
            assert!(
                model.workspaces()[0]
                    .panes()
                    .iter()
                    .all(|pane| pane.spawned_by().is_none())
            );
        }
        snapshot.workspaces[0].panes[0]
            .agent
            .as_mut()
            .unwrap()
            .session_id = Some("--last; evil".into());
        let original = serde_json::to_vec(&snapshot).unwrap();
        std::fs::write(&path, &original).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write);
        assert!(
            report.model.unwrap().workspaces()[0].panes()[0]
                .agent()
                .is_none()
        );
        assert!(!report.diagnostics.is_empty());
        assert!(
            std::fs::read_dir(root.path())
                .unwrap()
                .flatten()
                .filter(|entry| entry.path() != path)
                .any(|entry| std::fs::read(entry.path()).unwrap() == original)
        );
    }

    #[test]
    fn a_worktree_round_trips_and_one_that_is_gone_leaves_an_ordinary_terminal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let tree = directory.path().join("tree");
        std::fs::create_dir(&tree).unwrap();
        let mut snapshot = sample(directory.path());
        let worktree = neptune_model::Worktree {
            repository: directory.path().into(),
            path: tree.clone(),
            branch: "fix/login".into(),
            start: "0123456789abcdef0123456789abcdef01234567".into(),
        };
        snapshot.workspaces[0].panes[0].cwd = tree.clone();
        snapshot.workspaces[0].panes[0].worktree = Some(worktree.clone());
        std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        let model = report.model.unwrap();
        assert_eq!(model.workspaces()[0].panes()[0].worktree(), Some(&worktree));
        assert_eq!(
            StateSnapshot::from_model(&model).workspaces[0].panes[0].worktree,
            Some(worktree)
        );

        // Removed outside Neptune: the terminal opens in the workspace's
        // directory, and the original bytes are kept before any save.
        std::fs::remove_dir(&tree).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write && report.diagnostics.len() == 3);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
        let model = report.model.unwrap();
        let pane = &model.workspaces()[0].panes()[0];
        assert!(pane.worktree().is_none());
        assert_eq!(pane.cwd(), directory.path());
    }

    #[test]
    fn current_unversioned_fixture_migrates_without_losing_layout_or_selection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        std::fs::write(&path, fixture(directory.path())).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.migrated);
        assert!(report.can_write);
        assert!(report.diagnostics.is_empty());
        let model = report.model.unwrap();
        let workspace = &model.workspaces()[0];
        assert_eq!(workspace.panes().len(), 2);
        assert_eq!(workspace.active(), workspace.panes()[1].id());
        assert!(
            matches!(workspace.layout(), Layout::Split { axis: Axis::Vertical, ratio, .. } if *ratio == 0.65)
        );
    }

    #[test]
    fn independent_versioned_dto_round_trip_retains_split_identities() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = sample(directory.path());
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let parsed: StateSnapshot = serde_json::from_slice(&bytes).unwrap();
        let model = parsed.into_model(Limits::default()).unwrap();
        assert_eq!(
            serde_json::to_value(StateSnapshot::from_model(&model)).unwrap(),
            serde_json::to_value(snapshot).unwrap()
        );
    }

    #[test]
    fn a_grouped_remote_workspace_round_trips_its_directory_and_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let mut controller = Controller::new(
            sample(directory.path())
                .into_model(Limits::default())
                .unwrap(),
        );
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Servers".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        controller
            .dispatch(Command::AddWorkspace {
                group: Some(group),
                cwd: directory.path().into(),
                name: "devbox".into(),
                remote: Some("me@devbox".into()),
            })
            .unwrap();
        let pane = controller.model().active_pane().unwrap();
        let remote_cwd = PathBuf::from("/neptune-test-remote-only/project ' % λ");
        assert!(!remote_cwd.exists());
        controller
            .dispatch(Command::PaneRemoteCwdChanged {
                pane,
                generation: 1,
                cwd: remote_cwd.clone(),
            })
            .unwrap();
        controller
            .dispatch(Command::SetWorkspaceGroupCollapsed {
                group,
                collapsed: true,
            })
            .unwrap();
        let saved = serde_json::to_value(StateSnapshot::from_model(controller.model())).unwrap();
        assert_eq!(saved["version"], SCHEMA_VERSION);
        assert!(saved["workspaces"][0].get("ssh").is_none());
        assert_eq!(saved["workspaces"][1]["ssh"], "me@devbox");
        assert_eq!(
            saved["workspaces"][1]["panes"][0]["remote_cwd"],
            remote_cwd.to_str().unwrap()
        );
        assert!(
            saved["workspaces"][0]["panes"][0]
                .get("remote_cwd")
                .is_none()
        );
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write && !report.migrated);
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        let model = report.model.unwrap();
        assert_eq!(model.groups()[0].id(), group);
        assert_eq!(model.groups()[0].name(), "Servers");
        assert!(model.groups()[0].collapsed());
        assert_eq!(model.workspaces()[0].group(), None);
        assert_eq!(model.workspaces()[1].group(), Some(group));
        assert_eq!(
            model.pane(pane).unwrap().remote_cwd(),
            Some(remote_cwd.as_path())
        );
        assert_eq!(model.workspaces()[0].remote(), None);
        assert_eq!(
            model.workspaces()[1]
                .remote()
                .map(|remote| remote.destination()),
            Some("me@devbox")
        );
    }

    #[test]
    fn tabs_round_trip_while_a_terminal_alone_in_its_place_keeps_the_earlier_form() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let mut controller = Controller::new(
            sample(directory.path())
                .into_model(Limits::default())
                .unwrap(),
        );
        let workspace = controller.model().active_workspace().unwrap();
        let first = controller.model().workspaces()[0].panes()[0].id();
        controller
            .dispatch(Command::AddTab {
                workspace,
                pane: first,
                cwd: directory.path().into(),
            })
            .unwrap();
        let tab = controller.model().active_pane().unwrap();
        // The focused terminal is restored in view, so focus the other place.
        let beside = controller.model().workspaces()[0].panes()[1].id();
        controller
            .dispatch(Command::FocusPane {
                workspace,
                pane: beside,
            })
            .unwrap();
        let saved = serde_json::to_value(StateSnapshot::from_model(controller.model())).unwrap();
        let layout = &saved["workspaces"][0]["layout"];
        assert_eq!(
            layout["first"],
            serde_json::json!({ "kind": "tabs", "panes": [first, tab], "shown": tab })
        );
        assert_eq!(
            layout["second"],
            serde_json::json!({ "kind": "pane", "pane": beside })
        );
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write && !report.migrated);
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(
            report.model.unwrap().workspaces(),
            controller.model().workspaces()
        );
    }

    #[test]
    fn earlier_schema_versions_are_read_without_loss_and_saved_as_the_current_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        for version in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
            let mut saved = serde_json::to_value(sample(directory.path())).unwrap();
            saved["version"] = version.into();
            saved.as_object_mut().unwrap().remove("groups");
            if version >= 2 {
                saved["workspaces"][0]["ssh"] = "devbox".into();
            }
            if version == 3 {
                saved["workspaces"][0]["panes"][0]["remote_cwd"] =
                    "/neptune-test-remote-only/project".into();
            }
            std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
            let report = load_state(&path, Limits::default());
            assert!(report.can_write && report.migrated);
            assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
            // No recovery copy: nothing was repaired or dropped.
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
            let resaved =
                serde_json::to_value(StateSnapshot::from_model(&report.model.unwrap())).unwrap();
            saved["version"] = SCHEMA_VERSION.into();
            saved["groups"] = serde_json::json!([]);
            assert_eq!(resaved, saved);
        }
    }

    #[test]
    fn an_unusable_saved_destination_skips_that_workspace_and_archives_the_original() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let mut snapshot = sample(directory.path());
        snapshot.workspaces[0].ssh = Some("-oProxyCommand=id".into());
        std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write);
        assert!(report.model.unwrap().workspaces().is_empty());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|message| message.contains("invalid workspace \"fixture\"")
                    && message.contains("SSH host"))
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn corrupt_state_is_preserved_before_overwrite_is_allowed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let original = b"{broken state";
        std::fs::write(&path, original).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write);
        assert!(report.model.is_none());
        let backup = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|entry| *entry != path)
            .unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), original);
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|message| message.contains("preserved"))
        );
    }

    #[test]
    fn unsupported_future_schema_cannot_be_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let original = br#"{"version":99,"future_data":{"keep":"everything"}}"#;
        std::fs::write(&path, original).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(!report.can_write);
        assert!(report.model.is_none());
        assert!(report.diagnostics[0].contains("Unsupported"));
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn malformed_version_is_corrupt_and_archived_instead_of_reported_as_future() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        std::fs::write(&path, br#"{"version":"typo"}"#).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|message| message.contains("Invalid workspace schema version"))
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn oversized_state_is_read_only_and_never_allocated_or_overwritten_in_full() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_STATE_BYTES + 1).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(!report.can_write);
        assert!(report.diagnostics[0].contains("8 MiB"));
        assert_eq!(std::fs::metadata(path).unwrap().len(), MAX_STATE_BYTES + 1);
    }

    #[test]
    fn unreadable_state_path_disables_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let report = load_state(directory.path(), Limits::default());
        assert!(!report.can_write);
        assert!(report.diagnostics[0].contains("Cannot read"));
    }

    #[test]
    fn missing_pane_directory_falls_back_and_archives_original_with_context() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let mut snapshot = sample(directory.path());
        snapshot.workspaces[0].panes[1].cwd = directory.path().join("missing");
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|message| message.contains("Pane 2") && message.contains("missing"))
        );
        assert_eq!(
            report.model.unwrap().workspaces()[0].panes()[1].cwd(),
            directory.path()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn restore_skips_missing_workspace_and_selects_valid_successor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let mut snapshot = sample(directory.path());
        let mut second = snapshot.workspaces[0].clone();
        second.id = WorkspaceId::new(2);
        second.name = "valid successor".into();
        second.panes = vec![SavedPane {
            id: PaneId::new(3),
            cwd: directory.path().into(),
            remote_cwd: None,
            agent: None,
            pull_requests: Vec::new(),
            attachments: Vec::new(),
            spawned_by: None,
            worktree: None,
        }];
        second.layout = SavedLayout::Pane {
            pane: PaneId::new(3),
        };
        second.active = PaneId::new(3);
        snapshot.workspaces[0].cwd = directory.path().join("missing");
        snapshot.workspaces.push(second);
        std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        let model = report.model.unwrap();
        assert_eq!(model.active_workspace(), Some(WorkspaceId::new(2)));
        assert_eq!(model.workspaces().len(), 1);
    }

    #[test]
    fn invalid_new_schema_workspace_is_skipped_without_constructing_invalid_model() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let mut snapshot = sample(directory.path());
        snapshot.workspaces[0].active = PaneId::new(999);
        std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.model.unwrap().workspaces().is_empty());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|message| message.contains("invalid workspace"))
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn legacy_duplicate_layout_is_repaired_and_archived() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let text = String::from_utf8(fixture(directory.path()))
            .unwrap()
            .replace("\"Leaf\": 12", "\"Leaf\": 9");
        std::fs::write(&path, text).unwrap();
        let report = load_state(&path, Limits::default());
        let model = report.model.unwrap();
        assert_eq!(model.workspaces()[0].layout().panes().len(), 2);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|message| message.contains("repaired invalid legacy"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_recovery_copy_keeps_corrupt_file_read_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        let original = b"broken";
        std::fs::write(&path, original).unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let report = load_state(&path, Limits::default());
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        // Privileged test environments can bypass directory permissions; the
        // injected no-write path is meaningful only when backup creation fails.
        if report
            .diagnostics
            .iter()
            .any(|message| message.contains("Could not preserve"))
        {
            assert!(!report.can_write);
        }
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use neptune_model::{Command, Controller};

    #[test]
    fn mixed_sidebar_order_round_trips_and_version_four_keeps_its_original_order() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspaces.json");
        let mut controller = Controller::new(Model::default());
        for name in ["first", "second"] {
            controller
                .dispatch(Command::AddWorkspace {
                    cwd: root.path().into(),
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
                cwd: root.path().into(),
                name: "child".into(),
                remote: None,
                group: Some(group),
            })
            .unwrap();
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Empty".into(),
            })
            .unwrap();
        for index in [0, 1, 3] {
            controller
                .dispatch(Command::MoveSidebarItem {
                    item: SidebarItem::Group(group),
                    index,
                })
                .unwrap();
            let snapshot = StateSnapshot::from_model(controller.model());
            std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
            let report = load_state(&path, Limits::default());
            assert!(report.can_write && !report.migrated);
            assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
            let restored = report.model.unwrap();
            assert_eq!(restored.sidebar_order(), controller.model().sidebar_order());
            assert_eq!(restored.workspaces(), controller.model().workspaces());
        }
        let mut saved =
            serde_json::to_value(StateSnapshot::from_model(controller.model())).unwrap();
        saved["version"] = 4.into();
        saved.as_object_mut().unwrap().remove("sidebar_order");
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write && report.migrated && report.diagnostics.is_empty());
        let restored = report.model.unwrap();
        assert_eq!(
            restored
                .workspaces()
                .iter()
                .map(|w| w.name())
                .collect::<Vec<_>>(),
            ["first", "second", "child"]
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn damaged_sidebar_order_is_repaired_only_after_preserving_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspaces.json");
        let mut snapshot = super::tests::sample(root.path());
        let item = snapshot.sidebar_order.as_ref().unwrap()[0];
        snapshot.sidebar_order = Some(vec![
            item,
            item,
            SavedSidebarItem::Group {
                group: WorkspaceGroupId::new(99),
            },
        ]);
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write);
        assert_eq!(report.model.unwrap().sidebar_order().len(), 1);
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|message| message.contains("sidebar entry"))
                .count(),
            2
        );
        let recovery = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|file| file != &path)
            .unwrap();
        assert_eq!(std::fs::read(recovery).unwrap(), bytes);
    }

    #[test]
    fn folder_order_membership_and_collapsed_state_survive_disk_restoration() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspaces.json");
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        controller
            .dispatch(Command::SetWorkspaceGroupDefaultDirectory {
                group,
                directory: Some(root.path().into()),
            })
            .unwrap();
        controller
            .dispatch(Command::AddWorkspace {
                cwd: root.path().into(),
                name: "app".into(),
                remote: Some("me@host".into()),
                group: Some(group),
            })
            .unwrap();
        controller
            .dispatch(Command::SetWorkspaceGroupCollapsed {
                group,
                collapsed: true,
            })
            .unwrap();
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Empty".into(),
            })
            .unwrap();
        controller
            .dispatch(Command::MoveWorkspaceGroup { group, index: 1 })
            .unwrap();
        let snapshot = StateSnapshot::from_model(controller.model());
        std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write && !report.migrated);
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        let restored = report.model.unwrap();
        assert_eq!(restored.group_specs(), controller.model().group_specs());
        assert_eq!(restored.workspaces()[0].group(), Some(group));
        assert_eq!(
            serde_json::to_value(StateSnapshot::from_model(&restored)).unwrap(),
            serde_json::to_value(snapshot).unwrap()
        );
    }

    #[test]
    fn version_seven_groups_without_defaults_migrate_without_changing_directories() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspaces.json");
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        controller
            .dispatch(Command::AddWorkspace {
                cwd: root.path().into(),
                name: "app".into(),
                group: Some(group),
                remote: None,
            })
            .unwrap();
        let mut saved =
            serde_json::to_value(StateSnapshot::from_model(controller.model())).unwrap();
        assert!(saved["groups"][0].get("default_directory").is_none());
        assert!(saved["workspaces"][0].get("default_directory").is_none());
        saved["version"] = 7.into();
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write && report.migrated);
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        let restored = report.model.unwrap();
        assert!(restored.group(group).unwrap().default_directory().is_none());
        assert_eq!(
            serde_json::to_value(StateSnapshot::from_model(&restored)).unwrap()["workspaces"],
            serde_json::to_value(StateSnapshot::from_model(controller.model())).unwrap()["workspaces"]
        );
    }

    #[test]
    fn missing_group_default_preserves_the_group_existing_workspaces_and_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspaces.json");
        let missing = root.path().join("missing");
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspaceGroup {
                name: "Projects".into(),
            })
            .unwrap();
        let group = controller.model().groups()[0].id();
        controller
            .dispatch(Command::AddWorkspace {
                cwd: root.path().into(),
                name: "app".into(),
                group: Some(group),
                remote: None,
            })
            .unwrap();
        controller
            .dispatch(Command::SetWorkspaceGroupDefaultDirectory {
                group,
                directory: Some(missing.clone()),
            })
            .unwrap();
        let bytes = serde_json::to_vec(&StateSnapshot::from_model(controller.model())).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write);
        assert!(report.diagnostics.iter().any(
            |message| message.contains("default directory") && message.contains("unavailable")
        ));
        let restored = report.model.unwrap();
        assert_eq!(
            restored.group(group).unwrap().default_directory(),
            Some(missing.as_path())
        );
        assert_eq!(
            serde_json::to_value(StateSnapshot::from_model(&restored)).unwrap()["workspaces"],
            serde_json::to_value(StateSnapshot::from_model(controller.model())).unwrap()["workspaces"]
        );
        let recovery = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|file| file != &path)
            .unwrap();
        assert_eq!(std::fs::read(recovery).unwrap(), bytes);
    }

    #[test]
    fn missing_or_invalid_folders_recover_workspaces_and_preserve_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspaces.json");
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: root.path().into(),
                name: "app".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let mut snapshot = StateSnapshot::from_model(controller.model());
        snapshot.workspaces[0].group = Some(WorkspaceGroupId::new(7));
        snapshot.groups.push(SavedWorkspaceGroup {
            id: WorkspaceGroupId::new(7),
            name: " ".into(),
            collapsed: true,
            default_directory: None,
        });
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let report = load_state(&path, Limits::default());
        assert!(report.can_write);
        let restored = report.model.unwrap();
        assert!(restored.groups().is_empty());
        assert_eq!(restored.workspaces()[0].group(), None);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|m| m.contains("restored ungrouped"))
        );
        let recovery = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|file| file != &path)
            .unwrap();
        assert_eq!(std::fs::read(recovery).unwrap(), bytes);
    }

    #[test]
    fn earlier_versions_without_folder_fields_restore_ungrouped() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("workspaces.json");
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: root.path().into(),
                name: "app".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let mut saved =
            serde_json::to_value(StateSnapshot::from_model(controller.model())).unwrap();
        saved.as_object_mut().unwrap().remove("groups");
        for version in [1, 2, 3] {
            saved["version"] = version.into();
            std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
            let report = load_state(&path, Limits::default());
            assert!(report.can_write && report.migrated);
            assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
            let model = report.model.unwrap();
            assert!(model.groups().is_empty());
            assert_eq!(model.workspaces()[0].group(), None);
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
