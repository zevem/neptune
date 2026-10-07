//! What a project's lead and Neptune say to each other, as rules without
//! I/O: the chat's entries, what waits for the lead and when it is sent, the
//! words a turn is wrapped in, what a project keeps of its agents, what its
//! agents share, and its watches with the times they are due.
//! Processes and files belong to `runtime`, and the application owns each
//! project's state.
pub mod context;
pub mod inbox;
pub mod prompt;
pub mod registry;
pub mod schedule;
pub mod settings;
pub mod subscription;
pub mod transcript;
