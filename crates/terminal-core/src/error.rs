//! Expected public failures have stable categories and retain detailed context.
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionErrorKind {
    Backpressure,
    Closed,
    InvalidGeometry,
    InvalidOptions,
    Spawn,
    InvalidSearch,
    /// No session daemon is listening, or the connection to it was lost.
    Unavailable,
    /// The session daemon speaks another protocol version.
    Incompatible,
    /// The session daemon has no session with that id.
    NotFound,
}

#[derive(Debug)]
pub struct SessionError {
    kind: SessionErrorKind,
    detail: String,
    source: Option<anyhow::Error>,
}
impl SessionError {
    pub(crate) fn new(kind: SessionErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            source: None,
        }
    }
    pub(crate) fn spawn(source: anyhow::Error) -> Self {
        Self {
            kind: SessionErrorKind::Spawn,
            detail: format!("{source:#}"),
            source: Some(source),
        }
    }
    pub fn kind(&self) -> SessionErrorKind {
        self.kind
    }
}
impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|source| source.as_ref())
    }
}
