//! Desktop side effects with deterministic substitutes for headless tests.

pub mod clipboard;
pub mod file_drag;
pub mod files;
pub(crate) mod folders;
pub mod fonts;
#[cfg(target_os = "linux")]
pub mod host_env;
pub(crate) mod keyboard;
pub mod links;
pub mod shells;
pub mod updates;
pub mod window;

pub mod notifications;
