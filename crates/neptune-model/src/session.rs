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
