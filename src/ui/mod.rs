//! Widgets consume presentation data and emit targeted actions. Only the controller
//! may change durable workspace state; renderer caches remain desktop-owned.
pub mod agents;
pub mod attached;
pub mod changes;
pub mod chrome;
pub mod controls;
pub mod dialogs;
pub mod explorer;
pub mod helpers;
pub mod image_preview;
pub mod notifications;
pub mod palette;
pub mod panel;
pub(crate) mod ports;
pub mod preferences;
pub mod search;
pub mod theme_browser;
mod theme_editor;
pub mod updates;
pub mod workspace;
pub mod worktrees;
use crate::{config::Config, terminal::Cache};
use neptune_model::{
    Axis, Destination, PaneId, SidebarItem, SplitId, WorkspaceGroupId, WorkspaceId,
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    Pane(PaneId),
    Workspace(WorkspaceId),
    /// The SSH connection of a workspace: its terminals return to local shells.
    Connection(WorkspaceId),
    App,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OverlayState {
    #[default]
    None,
    Settings,
    Notifications,
    Update,
    Palette,
    Rename(WorkspaceId),
    GroupDefaultDirectory(WorkspaceGroupId),
    RenameGroup(WorkspaceGroupId),
    NewGroup,
    /// Connect the workspace over SSH, or create a connected one with `None`.
    Ssh(Option<WorkspaceId>),
    SshInGroup(WorkspaceGroupId),
    ConfirmClose(Close),
    /// A picture named in a terminal, as large as the window allows.
    Image,
    /// Confirm deleting the file or folder the explorer holds for it.
    DeleteFile,
    /// Name the branch of a new agent's worktree; its tab follows this terminal.
    NewWorktree(PaneId),
    /// Confirm removing the worktree the sheet's state holds.
    RemoveWorktree,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CloseStatus {
    #[default]
    General,
    Checking,
    Running {
        terminals: usize,
        unknown: usize,
    },
    Unknown,
}
#[derive(Default)]
pub struct UiState {
    pub overlay: OverlayState,
    pub close_status: CloseStatus,
    pub preferences: theme_browser::State,
    /// The Preferences pane in view and the settings search.
    pub preference_view: preferences::View,
    pub shells: preferences::ShellPicker,
    pub palette_query: String,
    /// Highlighted command; reset whenever the query changes.
    pub palette_selected: usize,
    pub rename_name: String,
    pub directory_path: String,
    /// Preserve a chosen folder's native path until the user edits its text.
    pub directory_selected: Option<PathBuf>,
    pub directory_group: String,
    pub directory_pending: bool,
    pub directory_browsing: bool,
    pub directory_error: Option<String>,
    pub ssh_host: String,
    /// A dialog field should take keyboard focus on its first frame.
    pub overlay_focus: bool,
    pub error: Option<String>,
    /// The font size or window zoom just stepped to, while its chip shows.
    pub level: Option<controls::Level>,
    pub search_open: bool,
    pub search: String,
    pub search_error: Option<String>,
    pub search_focus: bool,
    pub zoomed: bool,
    /// Width shown while the sidebar edge is dragged; saved on release.
    pub sidebar_drag: Option<f32>,
    /// A child workspace row being dragged within its group.
    pub workspace_drag: chrome::WorkspaceDrag,
    /// An ungrouped workspace or folder moving in the top-level sidebar list.
    pub item_drag: chrome::SidebarDrag,
    /// A sidebar toggle still sliding into place.
    pub sidebar_slide: Option<chrome::SidebarSlide>,
    /// The terminal being carried by its header, as of the last frame.
    pub pane_drag: Option<PaneId>,
    /// The panel at the trailing edge, which holds the tabs below.
    pub panel: panel::State,
    /// The file explorer tab and what is typed in it.
    pub explorer: explorer::State,
    /// The changes tab: what it compares and the file whose diff it shows.
    pub changes: changes::State,
    /// The sheets that make and remove an agent's git worktree.
    pub worktree: worktrees::State,
}
#[derive(Clone)]
pub enum Action {
    Split(PaneId, Axis),
    /// Open a terminal as a tab beside this one.
    NewTab(PaneId),
    MovePane(PaneId, Destination),
    ClosePane(PaneId),
    CloseWorkspace(WorkspaceId),
    SelectWorkspace(WorkspaceId),
    MoveWorkspace(WorkspaceId, usize),
    Focus(PaneId),
    Ratio(SplitId, f32),
    EvenSplit(SplitId),
    EvenSplits(WorkspaceId),
    Rename(WorkspaceId),
    New,
    NewInGroup(WorkspaceGroupId),
    NewGroup,
    CreateGroup(String),
    GroupDefaultDirectory(WorkspaceGroupId),
    SetGroupDefaultDirectory(WorkspaceGroupId, Option<String>),
    BrowseGroupDirectory(WorkspaceGroupId),
    RenameGroup(WorkspaceGroupId),
    SetGroupName(WorkspaceGroupId, String),
    SetGroupCollapsed(WorkspaceGroupId, bool),
    MoveSidebarItem(SidebarItem, usize),
    MoveToGroup(WorkspaceId, Option<WorkspaceGroupId>),
    RemoveGroup(WorkspaceGroupId),
    SshInGroup(WorkspaceGroupId),
    ConnectInGroup(WorkspaceGroupId, String),
    CreateInGroup(PathBuf, WorkspaceGroupId),
    /// Open the SSH sheet for a workspace, or for a new one with `None`.
    Ssh(Option<WorkspaceId>),
    Connect {
        workspace: Option<WorkspaceId>,
        destination: String,
    },
    Disconnect(WorkspaceId),
    Settings,
    OpenConfig,
    Themes,
    Notifications,
    Palette,
    ToggleSidebar,
    SidebarWidth(f32),
    Panel(panel::Event),
    Explorer(explorer::Event),
    Changes(changes::Event),
    Worktree(worktrees::Event),
    Find,
    CopyHints(PaneId),
    SearchChanged,
    FindNext {
        reverse: bool,
    },
    CloseSearch,
    Clear(PaneId),
    Restart(PaneId),
    Copy(PaneId),
    Paste(PaneId),
    /// Files from another application released over this terminal.
    DropFiles(PaneId, Vec<PathBuf>),
    Zoom,
    ZoomUiIn,
    ZoomUiOut,
    ResetUiZoom,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    WindowClose,
    Create(PathBuf, Option<String>),
    SetName(WorkspaceId, String),
    Preferences(Config),
    CheckUpdates,
    ReviewUpdate,
    DownloadUpdate(String),
    OpenUpdate(String),
    /// Replace this installation with the verified download, then restart.
    InstallUpdate(String),
    /// Restart into the version that was just installed.
    RestartUpdate,
    CancelUpdate,
    DismissUpdate,
    Confirm(Close),
    CancelClose,
    CloseOverlay,
    DismissError,
    Resize(PaneId, crate::terminal_view::geometry::ResizeRequest),
    Selection(PaneId, crate::terminal_view::SelectionInteraction),
    OpenLink(crate::platform::links::WebLink),
    OpenTerminalLink(PaneId, crate::terminal_view::LinkTarget),
    /// Show an attached picture at full size, or open another kind of file
    /// with its application.
    OpenAttachment(PaneId, PathBuf),
    /// Show an attached file in the file manager.
    RevealAttachment(PathBuf),
    /// Take one attached file, or all of them, off a terminal's list.
    RemoveAttachment(PaneId, PathBuf),
    ClearAttachments(PaneId),
    Port {
        pane: PaneId,
        generation: u64,
        listener: crate::platform::ports::Listener,
        stop: bool,
    },
    ScrollBottom(PaneId),
    OpenNotification(PaneId, u64),
    /// Reveal the terminal an agent runs in, if it is still that terminal.
    OpenAgent(PaneId, u64),
    DismissNotification(u64),
    ReadNotifications,
    ClearNotifications,
}
#[derive(Clone)]
pub struct WorkspaceView {
    pub id: WorkspaceId,
    pub group: Option<WorkspaceGroupId>,
    pub name: String,
    /// Current local directory of this workspace's focused terminal.
    pub cwd: PathBuf,
    /// SSH destination of a remote workspace.
    pub remote: Option<String>,
    /// The branch of the repository that folder is in.
    pub branch: Option<Branch>,
    pub panes: usize,
    pub unread: usize,
    /// The newest unread alert, shown in place of the path.
    pub alert: Option<String>,
    pub running: bool,
}
/// A folder's branch as the sidebar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// The branch, or the short name of a commit checked out without one.
    pub name: String,
    /// The folder has changes that are not committed.
    pub dirty: bool,
}
pub struct PaneRender {
    pub preedit: String,
    pub id: PaneId,
    pub cache: Cache,
    pub mouse_button: Option<u8>,
    pub mouse_cell: Option<(u16, u16, Option<u8>)>,
}
impl PaneRender {
    pub fn new(id: PaneId) -> Self {
        Self {
            id,
            preedit: String::new(),
            cache: Cache::default(),
            mouse_button: None,
            mouse_cell: None,
        }
    }
}
