//! Desktop composition. Durable state belongs to neptune-model, processes to runtime,
//! file compatibility to persistence, and terminal drawing to terminal_view.
pub mod agent_activity;
pub mod app;
pub mod config;
pub mod icons;
pub mod input;
pub mod notifications;
pub mod persistence;
pub mod platform;
pub mod runtime;
pub mod terminal;
pub mod terminal_theme;
pub mod terminal_view;
pub mod theme;
pub mod ui;

use std::path::PathBuf;

#[derive(Default)]
pub struct Launch {
    pub cwd: Option<PathBuf>,
    /// Open a workspace connected to this SSH destination.
    pub ssh: Option<String>,
    pub config: Option<PathBuf>,
    pub data_root: Option<PathBuf>,
    pub command: Option<String>,
    pub screenshot: Option<PathBuf>,
    pub size: Option<[f32; 2]>,
    pub no_restore: bool,
    pub diagnostics: bool,
}
