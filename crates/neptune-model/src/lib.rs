//! Pure application state. No window, renderer, terminal engine or filesystem is
//! needed to validate layouts or run commands. Runtime owners execute returned
//! effects and deliver generation-tagged completions back to the controller.

mod agent;
mod controller;
mod ids;
mod layout;
mod remote;
mod workspace;
mod worktree;

pub use agent::{AgentKind, AgentSession, Attachment, PullRequest};
pub use controller::{Command, Completion, Controller, Destination, Effect};
pub use ids::{PaneId, SessionGeneration, SplitId, WorkspaceGroupId, WorkspaceId};
pub use layout::{Axis, Edge, FocusDirection, Layout};
pub use remote::Remote;
pub use workspace::{Error, Lifecycle, Limits, Model, Pane, PaneSpec, Workspace, WorkspaceSpec};
pub use workspace::{SidebarItem, WorkspaceGroup, WorkspaceGroupSpec};
pub use worktree::Worktree;
