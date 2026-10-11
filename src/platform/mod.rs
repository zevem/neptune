//! Desktop side effects with deterministic substitutes for headless tests.

pub(crate) mod browser_import;
pub mod clipboard;
pub mod editor;
pub mod file_drag;
pub mod files;
pub(crate) mod folders;
pub mod fonts;
#[cfg(target_os = "linux")]
pub mod host_env;
pub(crate) mod keyboard;
pub mod links;
pub(crate) mod ports;
pub mod shells;
pub mod updates;
pub mod window;

pub mod notifications;
