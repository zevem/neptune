use serde::{Deserialize, Serialize};

/// Where a terminal pane's session keeps running when Neptune is closed: the
/// session daemon of a protocol version, and the session's id within it. Both
/// are opaque here; the desktop reaches the daemon and reads the id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRef {
    /// The protocol version of the session daemon that runs the session.
    pub daemon: u32,
    pub id: String,
}

impl SessionRef {
    /// The longest id a reference may carry.
    pub const MAX_ID: usize = 64;

    pub fn is_valid(&self) -> bool {
        self.daemon > 0
            && !self.id.is_empty()
            && self.id.len() <= Self::MAX_ID
            && self.id.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }
}

/// How a launch that keeps sessions reconciles its saved terminals with the
/// sessions still running in the session daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReattachPlan {
    /// Terminals whose kept session is running: they reattach to it.
    pub attach: Vec<crate::PaneId>,
    /// Terminals whose kept session is gone: they are restored with a fresh
    /// shell (see [`crate::Command::SessionLost`]).
    pub restore: Vec<crate::PaneId>,
    /// Running sessions no terminal refers to: they become Recovered sessions
    /// (see [`crate::Command::AdoptSessions`]), never ended silently.
    pub recovered: Vec<SessionRef>,
}

impl crate::Model {
    /// Match every terminal's kept session against the sessions `live` in
    /// the session daemon. Terminals without a kept session start as usual
    /// and are not part of the plan.
    pub fn plan_reattach(&self, live: &[SessionRef]) -> ReattachPlan {
        let mut plan = ReattachPlan::default();
        let panes = self
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panes);
        for pane in panes.clone() {
            if let Some(session) = &pane.session {
                match live.contains(session) {
                    true => plan.attach.push(pane.id),
                    false => plan.restore.push(pane.id),
                }
            }
        }
        for session in live {
            if !plan.recovered.contains(session)
                && !panes
                    .clone()
                    .any(|pane| pane.session.as_ref() == Some(session))
            {
                plan.recovered.push(session.clone());
            }
        }
        plan
    }
}
