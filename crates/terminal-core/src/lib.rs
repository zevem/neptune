//! UI-independent terminal sessions with bounded I/O and owned viewport contracts.
//!
//! A renderer acknowledges repaint before requesting [`TerminalSession::viewport`].
//! Backend state and mutex guards never cross this crate boundary. Grid resizing
//! happens before PTY notification; shutdown is nonblocking and remains observable
//! through [`ShutdownCompletion`] after the session handle is released.
mod error;
pub mod input;
mod search;
mod session;
mod view;
pub use error::{SessionError, SessionErrorKind};
pub use search::{SearchBudget, SearchProgress, SearchQuery, SearchTask};
#[cfg(unix)]
pub use session::daemon;
pub use session::{
    MAX_GRID_CELLS, ProcessActivity, Repaint, SessionMetadata, SessionMetrics, SessionOptions,
    SessionStatus, ShutdownCompletion, TRANSPORT_BUFFER_BUDGET, TerminalSession,
};
pub use view::*;
