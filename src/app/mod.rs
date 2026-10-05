//! Thin eframe composition: widgets emit actions, the controller owns transitions,
//! workers own processes/storage, and caches consume immutable terminal snapshots.
mod attached;
mod attachments;
mod changes;
mod closing;
mod coordinator;
mod delegation;
mod diagnostics;
mod directory;
mod explorer;
mod git;
mod hints;
mod image_preview;
mod input;
mod panel;
mod ports;
mod ssh;
#[cfg(test)]
mod tests;
mod worktrees;
use crate::{
    Launch,
    config::{self, Config},
    persistence::{
        window_state,
        workspace_state::{LoadReport, load_state},
    },
    runtime::{
        persistence::PersistenceWriter,
        sessions::{ResourcePolicy, SessionCompletion, SessionManager},
    },
    theme::{self, Palette, metrics},
    ui::{
        self, Action, Close, OverlayState, PaneRender, UiState, WorkspaceView,
        workspace::PanePresentation,
    },
};
use eframe::egui::{self, Pos2, Rect, Stroke, Vec2, emath::GuiRounding as _};
use neptune_model::{Command, Controller, Effect, Lifecycle, Limits, Model, PaneId, Remote};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use terminal_core::{
    Direction, Mode, Point, SearchBudget, SearchProgress, SearchQuery, SearchTask, SessionMetadata,
    SessionOptions, SessionStatus, ViewportSnapshot,
};

struct Startup {
    config: Config,
    report: LoadReport,
    error: Option<String>,
}
struct ActiveSearch {
    pane: PaneId,
    generation: u64,
    reverse: bool,
    task: SearchTask,
}
pub struct App {
    controller: Controller,
    sessions: SessionManager,
    renders: BTreeMap<PaneId, PaneRender>,
    config: Config,
    config_path: PathBuf,
    fonts: crate::platform::fonts::Fonts,
    state_path: PathBuf,
    window_path: PathBuf,
    window_state: window_state::WindowState,
    window_writable: bool,
    window_generation: u64,
    restore_maximized: bool,
    writer: Option<PersistenceWriter>,
    state_writable: bool,
    startup: Option<mpsc::Receiver<Startup>>,
    deferred_actions: Vec<Action>,
    initial_cwd: Option<PathBuf>,
    /// SSH destination of the workspace opened when nothing is restored.
    initial_remote: Option<String>,
    /// Program that opens remote terminals.
    ssh_client: String,
    ui: UiState,
    search_point: Option<Point>,
    search_query: Option<(String, Arc<SearchQuery>)>,
    search_task: Option<ActiveSearch>,
    started: Instant,
    /// When this frame's `logic` began, so diagnostics time the whole frame.
    frame_started: Instant,
    command: Option<String>,
    command_target: Option<(PaneId, u64)>,
    screenshot: Option<PathBuf>,
    capture_sent: bool,
    exit_approved: bool,
    pending_close: Option<closing::PendingClose>,
    directory_check: Option<directory::PendingDirectory>,
    folder_picker: crate::platform::folders::Picker,
    ephemeral: bool,
    preference_generation: u64,
    /// An input-method composition is in progress.
    ime_composing: bool,
    /// The terminal widget that owned the keyboard on the previous frame.
    terminal_focus: Option<egui::Id>,
    /// Window focus is reconciled during logic, including minimized frames.
    window_focused: Option<bool>,
    /// A sheet or the palette was open when focus was last reconciled.
    overlay_was_open: bool,
    _font_shortcut_monitor: crate::platform::keyboard::FontShortcutMonitor,
    diagnostics: diagnostics::Diagnostics,
    link_opener: crate::platform::links::LinkOpener,
    editor_opener: crate::platform::files::FileOpener,
    hints: Option<(PaneId, u64)>,
    hint_keys: Vec<egui::Key>,
    file_location: Option<hints::PendingLocation>,
    ports: crate::runtime::ports::Ports,
    notifications: crate::notifications::Notifications,
    /// What each running CLI agent is doing, for the agents tab.
    agents: crate::agent_activity::AgentActivities,
    /// What agents asked for the agents they start, between frames.
    delegation: delegation::Delegation,
    desktop_notifier: crate::platform::notifications::DesktopNotifier,
    updates: crate::runtime::updates::Updates,
    /// Start the installed update once this approved exit completes.
    relaunch: bool,
    relaunch_arguments: Vec<std::ffi::OsString>,
    /// The live state of the pull requests linked in the workspace in view.
    pull_requests: crate::runtime::pull_requests::Watcher,
    attachments: attachments::Attachments,
    /// What is known of the files agents attached to their terminals.
    attached: attached::Attached,
    image_preview: image_preview::ImagePreview,
    explorer: explorer::Explorer,
    /// What git says about the folders in view.
    changes: changes::Changes,
    file_drag: crate::platform::file_drag::FileDragSource,
    paste_chord: crate::input::PasteChord,
    /// Git worktrees made for agents, and the worker that runs git for them.
    worktrees: worktrees::Worktrees,
    /// A paste chord pressed this frame that the toolkit did not deliver.
    swallowed_paste: Option<egui::Modifiers>,
}
impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        launch: Launch,
        window: window_state::LoadReport,
    ) -> Self {
        // Neptune routes zoom before terminal input and reserves Ctrl+Shift for
        // terminal font size. The toolkit's permissive shortcuts overlap it.
        cc.egui_ctx
            .options_mut(|options| options.zoom_with_keyboard = false);
        theme::fonts(&cc.egui_ctx);
        if launch.diagnostics
            && let Some(render) = &cc.wgpu_render_state
        {
            let a = render.adapter.get_info();
            eprintln!(
                "{}",
                serde_json::json!({"operation":"renderer","backend":format!("{:?}",a.backend),"adapter":a.name,"scale":cc.egui_ctx.pixels_per_point(),"platform":std::env::consts::OS,"version":env!("CARGO_PKG_VERSION")})
            );
        }
        let data = launch.data_root.unwrap_or_else(config::data_dir);
        let config_path = launch.config.unwrap_or_else(|| data.join("config.toml"));
        let state_path = data.join("workspaces.json");
        let ephemeral = launch.screenshot.is_some();
        let restore =
            !launch.no_restore && launch.cwd.is_none() && launch.ssh.is_none() && !ephemeral;
        let (sender, startup) = mpsc::sync_channel(1);
        let settings = config_path.clone();
        let state = state_path.clone();
        let wake = cc.egui_ctx.clone();
        let thread_result = std::thread::Builder::new()
            .name("neptune-restore".into())
            .spawn(move || {
                let (config, error) = match Config::load(&settings) {
                    Ok(config) => (config, None),
                    Err(error) => (Config::default(), Some(error.to_string())),
                };
                let report = if restore && config.restore_workspaces {
                    load_state(&state, Limits::default())
                } else {
                    LoadReport {
                        model: None,
                        diagnostics: Vec::new(),
                        can_write: true,
                        migrated: false,
                    }
                };
                let _ = sender.send(Startup {
                    config,
                    report,
                    error,
                });
                wake.request_repaint();
            });
        let mut ui = UiState {
            error: window.error,
            ..UiState::default()
        };
        if let Err(error) = thread_result {
            ui.error = Some(format!("Could not start restoration worker: {error}"));
        }
        Self {
            _font_shortcut_monitor: crate::platform::keyboard::FontShortcutMonitor::install(),
            controller: Controller::new(Model::default()),
            sessions: SessionManager::new(ResourcePolicy::default(), launch.diagnostics),
            renders: BTreeMap::new(),
            config: Config::default(),
            config_path,
            fonts: Default::default(),
            state_path,
            window_path: data.join("window.json"),
            window_state: window.state,
            window_writable: window.can_write,
            window_generation: 0,
            restore_maximized: window.state.maximized,
            writer: None,
            state_writable: false,
            startup: Some(startup),
            deferred_actions: Vec::new(),
            initial_cwd: launch.cwd,
            initial_remote: launch.ssh,
            ssh_client: coordinator::SSH_CLIENT.into(),
            ui,
            search_point: None,
            search_query: None,
            search_task: None,
            started: Instant::now(),
            frame_started: Instant::now(),
            command: launch.command,
            command_target: None,
            screenshot: launch.screenshot,
            capture_sent: false,
            exit_approved: false,
            pending_close: None,
            directory_check: None,
            folder_picker: crate::platform::folders::Picker::new(cc),
            ephemeral,
            preference_generation: 0,
            ime_composing: false,
            terminal_focus: None,
            window_focused: None,
            overlay_was_open: false,
            diagnostics: diagnostics::Diagnostics::new(launch.diagnostics),
            link_opener: Default::default(),
            editor_opener: Default::default(),
            hints: None,
            hint_keys: Vec::new(),
            file_location: None,
            ports: Default::default(),
            notifications: Default::default(),
            agents: Default::default(),
            delegation: Default::default(),
            desktop_notifier: Default::default(),
            updates: Default::default(),
            relaunch: false,
            relaunch_arguments: launch.relaunch_arguments,
            pull_requests: Default::default(),
            attachments: attachments::Attachments::new(data.join("pasted-images")),
            image_preview: Default::default(),
            attached: Default::default(),
            explorer: Default::default(),
            changes: Default::default(),
            file_drag: Default::default(),
            paste_chord: Default::default(),
            worktrees: Default::default(),
            swallowed_paste: None,
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        let focused = Self::window_has_focus(ctx);
        if self.window_focused != Some(focused) {
            let reported = self
                .controller
                .model()
                .active_pane()
                .and_then(|pane| self.sessions.get(pane))
                .is_none_or(|session| session.focus(focused).is_ok());
            if reported {
                self.window_focused = Some(focused);
            }
        }
        self.updates.configure(self.config.release_channel);
        if self.updates.poll(
            ctx,
            self.startup.is_none()
                && !self.ephemeral
                && self.config.check_updates
                && self.ui.overlay != OverlayState::Update,
        ) {
            self.restart_updated(ctx);
        }
        self.poll_file_location(ctx);
        if let Some(Err(error)) = self.editor_opener.poll() {
            self.ui.error = Some(error.into());
        }
        if let Some(Err(error)) = self.link_opener.poll() {
            self.ui.error = Some(error.into());
        }
        let startup = self
            .startup
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(startup) => Some(startup),
                Err(mpsc::TryRecvError::Disconnected) => Some(Startup {
                    config: self.config.clone(),
                    report: LoadReport {
                        model: None,
                        diagnostics: Vec::new(),
                        // Preserve existing storage if restoration never completed.
                        can_write: false,
                        migrated: false,
                    },
                    error: Some("Restoration worker stopped before returning state".into()),
                }),
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some(startup) = startup {
            self.complete_startup(ctx, startup);
        }
        if self.fonts.poll(ctx, &self.config.font_family) {
            for render in self.renders.values_mut() {
                render.cache.retry_resize();
            }
        }
        for event in self.sessions.poll() {
            match event {
                SessionCompletion::Started {
                    pane,
                    generation,
                    elapsed,
                } => {
                    if let Some(render) = self.renders.get_mut(&pane) {
                        render.cache.retry_resize();
                        render.cache.invalidate();
                    }
                    if let Some(session) = self.sessions.get(pane) {
                        set_session_palette(session, Palette::for_config(&self.config));
                        let _ = session
                            .focus(focused && self.controller.model().active_pane() == Some(pane));
                    }
                    self.dispatch(ctx, Command::SessionStarted { pane, generation });
                    self.diagnostics
                        .operation("spawn", pane, generation, elapsed);
                }
                SessionCompletion::Failed {
                    pane,
                    generation,
                    message,
                } => {
                    self.diagnostics
                        .failure("spawn", Some(pane), Some(generation), "Spawn");
                    self.dispatch(
                        ctx,
                        Command::SessionFailed {
                            pane,
                            generation,
                            error: message.clone(),
                        },
                    );
                    self.ui.error = Some(format!("Pane {pane}, session {generation}: {message}"));
                }
            }
        }
        for (pane, generation, agent) in self.sessions.agent_changes() {
            self.dispatch(
                ctx,
                Command::PaneAgentChanged {
                    pane,
                    generation,
                    agent,
                },
            );
        }
        for (pane, generation, pull_request) in self.sessions.pull_request_links() {
            self.dispatch(
                ctx,
                Command::PanePullRequestLinked {
                    pane,
                    generation,
                    pull_request,
                },
            );
        }
        // The workspace in view is the one whose pull requests have chips.
        let model = self.controller.model();
        self.pull_requests.watch(
            model
                .active_workspace()
                .and_then(|id| model.workspace(id))
                .into_iter()
                .flat_map(|workspace| workspace.panes())
                .flat_map(|pane| pane.pull_requests().iter().cloned()),
            ctx,
        );
        for (pane, generation, attachment) in self.sessions.attached_files() {
            self.attached.forget(attachment.path());
            self.dispatch(
                ctx,
                Command::PaneFileAttached {
                    pane,
                    generation,
                    attachment,
                },
            );
        }
        let metadata: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(id, session)| {
                self.controller.model().pane(id).map(|pane| {
                    (
                        id,
                        pane.generation(),
                        pane.lifecycle().clone(),
                        session.metadata(),
                    )
                })
            })
            .collect();
        for (pane, generation, lifecycle, metadata) in metadata {
            self.sync_directory(ctx, pane, &metadata);
            if !matches!(metadata.status, SessionStatus::Running) && lifecycle == Lifecycle::Running
            {
                self.dispatch(ctx, Command::SessionExited { pane, generation });
            }
        }
        // The coordinator consumes bounded clipboard and process notification events.
        // Selection stores are unsupported by the portable desktop clipboard.
        self.notifications.retain_sessions(self.controller.model());
        self.desktop_notifier.poll();
        for (pane, session) in self.sessions.iter() {
            let Some(item) = self.controller.model().pane(pane) else {
                continue;
            };
            let generation = item.generation();
            if self.sessions.generation(pane) != Some(generation) {
                continue;
            }
            for event in session.drain_events() {
                match event {
                    terminal_core::TerminalEvent::Notification(notification) => {
                        let focused = ctx.input(|i| i.focused)
                            && self.controller.model().active_pane() == Some(pane);
                        let visible = ctx
                            .input(|i| i.focused && !i.viewport().minimized.unwrap_or(false))
                            && self.shown().contains(&pane);
                        if matches!(
                            notification.occasion,
                            terminal_core::NotificationOccasion::Unfocused
                        ) && focused
                            || matches!(
                                notification.occasion,
                                terminal_core::NotificationOccasion::Invisible
                            ) && visible
                        {
                            continue;
                        }
                        if self.config.desktop_notifications {
                            self.desktop_notifier.show(
                                pane,
                                notification.title.clone(),
                                notification.body.clone(),
                                ctx,
                            );
                        }
                        self.notifications.push(pane, generation, notification);
                    }
                    terminal_core::TerminalEvent::CloseNotification { id } => {
                        self.notifications.close(pane, generation, &id)
                    }
                    terminal_core::TerminalEvent::ClipboardStore {
                        selection: false,
                        text,
                    } => crate::platform::clipboard::copy(ctx, text),
                    terminal_core::TerminalEvent::ClipboardStore {
                        selection: true, ..
                    } => {}
                    // What an agent on an SSH host is doing. The bridge
                    // holds the credential that tells it from other output.
                    terminal_core::TerminalEvent::Report(line) => {
                        self.sessions.agents().report(pane, generation, &line);
                    }
                }
            }
        }
        self.poll_agents(ctx);
        self.poll_ports(ctx);
        self.serve_agents(ctx);
        self.poll_attachments();
        self.poll_attached(ctx);
        self.poll_explorer(ctx);
        self.poll_changes(ctx);
        self.poll_saves(ctx);
        self.poll_search(ctx);
        if self.diagnostics.enabled() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if self.startup.is_some()
            || self.sessions.usage().starting > 0
            || self.sessions.usage().closing > 0
        {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
    fn window_has_focus(ctx: &egui::Context) -> bool {
        ctx.input(|input| input.focused && !input.viewport().minimized.unwrap_or(false))
    }
    fn complete_startup(&mut self, ctx: &egui::Context, startup: Startup) {
        self.startup = None;
        self.config = startup.config;
        ctx.set_zoom_factor(self.config.window_zoom);
        theme::apply(ctx, &self.config);
        self.state_writable = startup.report.can_write;
        let mut errors = startup.report.diagnostics;
        if let Some(error) = startup.error {
            errors.push(error);
        }
        if !errors.is_empty() {
            self.ui.error = Some(errors.join("\n"));
        }
        self.controller = Controller::new(startup.report.model.unwrap_or_default());
        let repaint = ctx.clone();
        match PersistenceWriter::new(
            self.state_path.clone(),
            self.config_path.clone(),
            self.window_path.clone(),
            !self.ephemeral,
            Arc::new(move || repaint.request_repaint()),
        ) {
            Ok(writer) => self.writer = Some(writer),
            Err(error) => {
                self.diagnostics
                    .failure("persistence_worker", None, None, "Persistence");
                self.ui.error = Some(format!("Persistence worker: {error}"));
            }
        }
        self.execute(ctx, self.controller.start_effects());
        if self.controller.model().workspaces().is_empty() {
            let cwd = self.initial_cwd.take().unwrap_or_else(default_cwd);
            let remote = self.initial_remote.take();
            self.create_workspace(ctx, cwd, None, remote, None);
        }
        // Bind the CLI command before queued user actions can change focus.
        self.command_target = self.controller.model().active_pane().and_then(|id| {
            self.controller
                .model()
                .pane(id)
                .map(|pane| (id, pane.generation()))
        });
        for action in std::mem::take(&mut self.deferred_actions) {
            self.action(ctx, action);
        }
        self.save_state();
    }
    /// The SSH destination of the workspace that holds `pane`.
    fn remote_of(&self, pane: PaneId) -> Option<&Remote> {
        let model = self.controller.model();
        model
            .workspace_for_pane(pane)
            .and_then(|id| model.workspace(id))
            .and_then(|workspace| workspace.remote())
    }
    fn views(&self) -> Vec<WorkspaceView> {
        self.controller
            .model()
            .workspaces()
            .iter()
            .map(|w| {
                let (unread, latest) = self
                    .notifications
                    .attention(|pane| w.panes().iter().any(|p| p.id() == pane));
                let cwd = w.pane(w.active()).map_or(w.cwd(), |pane| pane.cwd());
                WorkspaceView {
                    id: w.id(),
                    group: w.group(),
                    name: w.name().into(),
                    cwd: cwd.into(),
                    branch: self
                        .changes
                        .branch(std::path::Path::new(cwd))
                        .filter(|_| w.remote().is_none()),
                    remote: w.remote().map(|remote| remote.destination().to_owned()),
                    // Terminals without a tab are counted where they are listed.
                    panes: w.layout().panes().len(),
                    unread,
                    // A row shows one line; the popover has the whole alert.
                    alert: latest
                        .map(|entry| entry.headline().chars().take(120).collect::<String>())
                        .filter(|headline| !headline.is_empty()),
                    running: w
                        .panes()
                        .iter()
                        .any(|p| matches!(p.lifecycle(), Lifecycle::Starting | Lifecycle::Running)),
                }
            })
            .collect()
    }
    /// The terminals in view: the tab shown in each place of the active
    /// workspace, or only the focused one while it is zoomed.
    fn shown(&self) -> Vec<PaneId> {
        let model = self.controller.model();
        match model.active_workspace().and_then(|id| model.workspace(id)) {
            Some(workspace) if self.ui.zoomed => vec![workspace.active()],
            Some(workspace) => workspace.layout().shown(),
            None => Vec::new(),
        }
    }
    fn presentations(&self) -> BTreeMap<PaneId, PanePresentation> {
        let Some(workspace) = self
            .controller
            .model()
            .active_workspace()
            .and_then(|id| self.controller.model().workspace(id))
        else {
            return BTreeMap::new();
        };
        let remote = workspace
            .remote()
            .map(|remote| remote.destination().to_owned());
        // Tabs out of view contribute a title, not their content.
        let shown = self.shown();
        workspace
            .panes()
            .iter()
            .map(|pane| {
                let shown = shown.contains(&pane.id());
                let presentation = if let Some(session) = self.sessions.get(pane.id()) {
                    if shown {
                        session.acknowledge_repaint();
                    }
                    PanePresentation {
                        generation: pane.generation(),
                        ports: self.ports.view(pane.id(), pane.generation()).to_vec(),
                        agent: pane.agent().map(|agent| agent.kind),
                        pull_requests: pane
                            .pull_requests()
                            .iter()
                            .map(|link| ui::helpers::LinkedPullRequest {
                                link: link.clone(),
                                lookup: self.pull_requests.lookup(link),
                            })
                            .collect(),
                        attached: self.attached_files(pane),
                        spawned: self.spawned_agents(pane.id()),
                        worktree: self.worktree_tab(pane),
                        unread: self.notifications.unread(Some(pane.id())),
                        metadata: session.metadata(),
                        snapshot: shown.then(|| session.viewport()),
                        starting: false,
                        remote: remote.clone(),
                    }
                } else {
                    let title = match pane.lifecycle() {
                        Lifecycle::Failed(error) => format!("Failed: {error}"),
                        _ => "Starting shell…".into(),
                    };
                    PanePresentation {
                        generation: pane.generation(),
                        ports: Vec::new(),
                        agent: None,
                        pull_requests: Vec::new(),
                        attached: Vec::new(),
                        spawned: Vec::new(),
                        worktree: self.worktree_tab(pane),
                        unread: self.notifications.unread(Some(pane.id())),
                        metadata: SessionMetadata {
                            title,
                            shell: if remote.is_some() {
                                self.ssh_client.clone()
                            } else {
                                self.config.shell.clone().unwrap_or_else(|| "shell".into())
                            },
                            cwd: pane.cwd().into(),
                            reported_cwd: None,
                            process_id: None,
                            remote_process_id: None,
                            status: match pane.lifecycle() {
                                Lifecycle::Failed(error) => SessionStatus::Error(error.clone()),
                                _ => SessionStatus::Running,
                            },
                            bell_count: 0,
                        },
                        // A placeholder has no shell, so it shows no cursor.
                        snapshot: shown.then(|| ViewportSnapshot {
                            mode: Mode::NONE,
                            ..ViewportSnapshot::blank(80, 24)
                        }),
                        starting: !matches!(pane.lifecycle(), Lifecycle::Failed(_)),
                        remote: remote.clone(),
                    }
                };
                (pane.id(), presentation)
            })
            .collect()
    }
    fn find_next(&mut self, reverse: bool) {
        if self.ui.search.is_empty() {
            self.search_task = None;
            return;
        }
        let Some(pane) = self.controller.model().active_pane() else {
            return;
        };
        let Some(session) = self.sessions.get(pane) else {
            return;
        };
        if self
            .search_query
            .as_ref()
            .is_none_or(|(query, _)| query != &self.ui.search)
        {
            match SearchQuery::compile(&ui::helpers::regex_escape(&self.ui.search)) {
                Ok(query) => self.search_query = Some((self.ui.search.clone(), query)),
                Err(error) => {
                    self.ui.search_error = Some(error.to_string());
                    return;
                }
            }
        }
        let Some((_, query)) = &self.search_query else {
            return;
        };
        let direction = if reverse {
            Direction::Left
        } else {
            Direction::Right
        };
        let origin = self
            .search_point
            .map(|point| session.next_search_point(point, direction))
            .unwrap_or_else(|| Point::new(-(session.viewport().display_offset as i32), 0));
        let generation = self
            .controller
            .model()
            .pane(pane)
            .map(|p| p.generation())
            .unwrap_or(0);
        self.search_task = Some(ActiveSearch {
            pane,
            generation,
            reverse,
            task: session.begin_search(
                query.clone(),
                origin,
                if reverse {
                    Direction::Left
                } else {
                    Direction::Right
                },
            ),
        });
        self.ui.search_error = Some("Searching…".into());
    }
    /// When a sheet or the palette closes, its focused control is gone but
    /// would keep the keyboard for one more frame. Release it at once so the
    /// terminal can take over without dropping keys.
    fn release_closed_overlay_focus(&mut self, ctx: &egui::Context) {
        let open = self.ui.overlay != OverlayState::None;
        if self.overlay_was_open && !open {
            ctx.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
        }
        self.overlay_was_open = open;
    }
    fn poll_search(&mut self, ctx: &egui::Context) {
        let Some(mut search) = self.search_task.take() else {
            return;
        };
        if self.controller.model().active_pane() != Some(search.pane)
            || self
                .controller
                .model()
                .pane(search.pane)
                .is_none_or(|p| p.generation() != search.generation)
        {
            return;
        }
        let Some(session) = self.sessions.get(search.pane) else {
            return;
        };
        match session.search_step(
            &mut search.task,
            SearchBudget {
                max_rows: 32,
                max_duration: Duration::from_millis(2),
            },
        ) {
            SearchProgress::Pending => {
                self.search_task = Some(search);
                ctx.request_repaint();
            }
            SearchProgress::Found(range) => {
                session.scroll_to_point(range.start);
                session.set_selection(range.start, range.end);
                self.search_point = Some(if search.reverse {
                    range.start
                } else {
                    range.end
                });
                self.ui.search_error = None;
            }
            SearchProgress::LimitExceeded => {
                self.ui.search_error =
                    Some("Search stopped: a logical line exceeds the 1 MiB search limit".into());
            }
            SearchProgress::NotFound => {
                self.search_point = None;
                self.ui.search_error = Some("No matches".into());
            }
            SearchProgress::Stale => {
                self.find_next(search.reverse);
                ctx.request_repaint_after(Duration::from_millis(30));
            }
            SearchProgress::Cancelled => {}
        }
    }
}
impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }
    fn raw_input_hook(&mut self, _: &egui::Context, raw_input: &mut egui::RawInput) {
        crate::input::drop_redundant_preedits(&mut raw_input.events, &mut self.ime_composing);
        self.swallowed_paste = self.paste_chord.swallowed(&raw_input.events);
    }
    /// Runs before every `ui`, and alone while the window is minimized or
    /// covered: eframe shows no UI then, but terminals keep working.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        use crate::platform::window::{self, WindowOperation};
        self.frame_started = Instant::now();
        self.file_drag.attach(frame, ctx);
        self.poll_directory(ctx);
        self.poll_worktrees(ctx);
        window::sync_minimized(
            ctx,
            frame
                .winit_window()
                .and_then(|window| window.is_minimized()),
        );
        self.poll(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && !self.exit_approved {
            window::send(ctx, WindowOperation::CancelClose);
            self.relaunch = false;
            self.request_close(ctx, Close::App);
            // A close from the taskbar can reach a hidden window. Show the
            // confirmation rather than leave it unanswerable.
            if self.ui.overlay == OverlayState::ConfirmClose(Close::App)
                && !ctx.input(|i| i.focused)
            {
                window::send(ctx, WindowOperation::Show);
            }
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.observe_window(&ctx, ui.max_rect().size());
        self.shortcuts(&ctx);
        self.release_closed_overlay_focus(&ctx);
        // Sampled before widgets run: a menu that closes on this frame's key
        // press still owns that key.
        let menu_open = egui::Popup::is_any_open(&ctx);
        // Tab and the arrow keys move focus only inside sheets and menus.
        // Everywhere else they belong to the terminal or the focused field,
        // so the toolkit must not walk focus into the chrome. The palette and
        // the notification popover move their own highlight.
        let sheet_open = !matches!(
            self.ui.overlay,
            OverlayState::None | OverlayState::Palette | OverlayState::Notifications
        );
        if !sheet_open && !menu_open {
            ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
        }
        // A sheet or the palette opening ends a terminal drag without a move.
        if self.ui.pane_drag.is_some() && self.ui.overlay != OverlayState::None {
            self.cancel_pane_drag(&ctx);
        }
        let p = Palette::for_config(&self.config);
        let bounds = ui.max_rect();
        // Windows 11 clips the window to its own smaller rounded corner and
        // draws the border; a painted corner would leave gaps inside it.
        let radius = if !cfg!(target_os = "windows")
            && ctx.input(|i| {
                !i.viewport().fullscreen.unwrap_or(false)
                    && (cfg!(target_os = "macos") || !i.viewport().maximized.unwrap_or(false))
            }) {
            metrics::WINDOW_RADIUS
        } else {
            0
        };
        ui.painter().rect_filled(bounds, radius, p.chrome);
        let mut actions = Vec::new();
        let views = self.views();
        let active = self.controller.model().active_workspace();
        let active_pane = self.controller.model().active_pane();
        let presentations = self.presentations();
        let subtitle = active_pane
            .and_then(|pane| presentations.get(&pane))
            .map(|presentation| {
                let label = ui::workspace::pane_label(&presentation.metadata);
                // A worktree is known by its branch, as its tab would be.
                match &presentation.worktree {
                    Some(worktree) => format!("{} — {label}", worktree.branch),
                    None => format!("{label} — {}", presentation.location()),
                }
            })
            .unwrap_or_default();
        // A terminal alone in view has no tab, so the toolbar carries its links.
        let tabs = active
            .and_then(|id| self.controller.model().workspace(id))
            .map_or(0, |workspace| {
                match workspace.layout().tabs(workspace.active()) {
                    Some((tabs, _)) if self.ui.zoomed => tabs.len(),
                    _ => workspace.layout().panes().len(),
                }
            });
        let pull_requests = active_pane
            .and_then(|pane| presentations.get(&pane))
            .filter(|_| tabs <= 1)
            .map_or(&[][..], |presentation| &presentation.pull_requests);
        let spawned = active_pane
            .and_then(|pane| presentations.get(&pane))
            .filter(|_| tabs <= 1)
            .map_or(&[][..], |presentation| &presentation.spawned);
        let worktree = active_pane
            .filter(|_| tabs <= 1)
            .and_then(|pane| Some((pane, presentations.get(&pane)?.worktree.as_ref()?)));
        let attached = active_pane
            .and_then(|pane| presentations.get(&pane))
            .filter(|_| tabs <= 1)
            .map_or(&[][..], |presentation| &presentation.attached);
        let sidebar_available = bounds.width() >= metrics::SIDEBAR_MIN_WINDOW;
        let sidebar_open = self.controller.model().sidebar() && sidebar_available;
        // Only a toggle slides. A window too narrow for the sidebar, like
        // restored state, takes its place at once.
        let sliding = self
            .ui
            .sidebar_slide
            .and_then(|slide| slide.reveal(sidebar_open, ctx.input(|input| input.time)))
            .filter(|_| sidebar_available);
        if sliding.is_some() {
            ctx.request_repaint();
        } else {
            self.ui.sidebar_slide = None;
        }
        if !sidebar_open {
            // Neither an edge drag nor a row drag can finish once the sidebar
            // is leaving.
            self.ui.sidebar_drag = None;
            self.ui.workspace_drag = Default::default();
            self.ui.item_drag = Default::default();
        }
        let sidebar_width = self
            .ui
            .sidebar_drag
            .unwrap_or(self.config.sidebar_width)
            .min(bounds.width() - 420.0);
        // Where the sidebar's trailing edge rests, and where it is this frame.
        let rest = if sidebar_open { sidebar_width } else { 0.0 };
        let edge = sliding.map_or(rest, |reveal| {
            (reveal * sidebar_width).round_to_pixels(ctx.pixels_per_point())
        });
        let reveal = sliding.unwrap_or(if sidebar_open { 1.0 } else { 0.0 });
        // The panel takes the trailing edge the way the sidebar takes the
        // leading one, leaving the terminals a usable width between them.
        let panel_width = self
            .ui
            .panel
            .width
            .min(bounds.width() - rest - ui::panel::MIN_STAGE);
        let panel_available = panel_width >= *ui::panel::WIDTH.start();
        let panel_reveal = self.panel_reveal(&ctx, panel_available);
        let panel_open = self.ui.panel.open && panel_available;
        let panel_rest = if panel_open { panel_width } else { 0.0 };
        let panel_edge = (panel_reveal * panel_width).round_to_pixels(ctx.pixels_per_point());
        let panel_tab = self.ui.panel.tab;
        // The explorer reads folders only while its tab is the one in view.
        if panel_reveal > 0.0 && panel_tab == ui::panel::Tab::Files {
            self.sync_explorer(&ctx);
        } else {
            self.rest_explorer(&ctx);
        }
        let agents_shown = panel_reveal > 0.0 && panel_tab == ui::panel::Tab::Agents;
        // Git is asked about what is in view: the sidebar's rows and the tab.
        self.sync_changes(
            &ctx,
            edge > 0.0,
            panel_reveal > 0.0 && panel_tab == ui::panel::Tab::Changes,
        );
        let chrome = ui::chrome::ChromeView {
            workspaces: &views,
            groups: self.controller.model().groups(),
            sidebar_order: self.controller.model().sidebar_order(),
            active,
            pane: active_pane,
            subtitle: &subtitle,
            pull_requests,
            attached,
            spawned,
            worktree,
            ports: active_pane
                .and_then(|pane| presentations.get(&pane))
                .filter(|_| tabs <= 1)
                .map_or(&[], |p| p.ports.as_slice()),
            pane_generation: active_pane
                .and_then(|pane| self.controller.model().pane(pane))
                .map_or(0, |p| p.generation()),
            zoomed: self.ui.zoomed,
            window: bounds,
            sidebar: reveal,
            sidebar_open,
            sidebar_width,
            sidebar_available,
            pane_drag: self.ui.pane_drag,
            panel: panel_available.then_some(self.ui.panel.open),
            panel_attention: !agents_shown && self.agents.waiting_count() > 0,
        };
        if edge > 0.0 {
            let side = Rect::from_min_size(
                Pos2::new(bounds.left() + edge - sidebar_width, bounds.top()),
                Vec2::new(sidebar_width, bounds.height()),
            );
            ui::chrome::sidebar(ui, side, p, &chrome, &mut self.ui, &mut actions);
        }
        let toolbar = Rect::from_min_max(
            Pos2::new(bounds.left() + edge, bounds.top()),
            Pos2::new(bounds.right(), bounds.top() + metrics::TOOLBAR_HEIGHT),
        );
        ui::chrome::toolbar(ui, toolbar, p, &chrome, &mut self.ui, &mut actions);
        ui::chrome::leading_controls(ui, p, &chrome, &mut actions);
        // Panes sit in the chrome like inset content; the sidebar supplies its
        // own trailing margin.
        let stage_from = |edge: f32, trailing: f32| {
            Rect::from_min_max(
                Pos2::new(bounds.left() + edge.max(metrics::GUTTER), toolbar.bottom()),
                Pos2::new(
                    bounds.right() - trailing.max(metrics::GUTTER),
                    bounds.bottom() - metrics::GUTTER,
                ),
            )
        };
        let stage = ui::workspace::Placement {
            drawn: stage_from(edge, panel_edge),
            settled: stage_from(rest, panel_rest),
        };
        let visible = self.shown();
        let cache_limit =
            self.sessions.policy().max_sessions * self.sessions.policy().cache_bytes_per_session;
        let mut cache_bytes = self
            .renders
            .values()
            .map(|render| render.cache.estimated_bytes())
            .sum::<usize>();
        for (id, render) in &mut self.renders {
            if cache_bytes <= cache_limit {
                break;
            }
            if !visible.contains(id) {
                let bytes = render.cache.estimated_bytes();
                render.cache.evict();
                cache_bytes = cache_bytes.saturating_sub(bytes);
            }
        }
        let overlay = self.ui.overlay != OverlayState::None;
        // Polled every frame, so a drop over a sheet is not delivered later.
        let file_drag = self.file_drag.poll(&ctx);
        let mut output = ui::workspace::StageOutput::default();
        if let Some(workspace) = active.and_then(|id| self.controller.model().workspace(id)) {
            // A zoomed terminal keeps the tabs it shares its place with.
            let layout = match workspace.layout().tabs(workspace.active()) {
                Some((tabs, shown)) if self.ui.zoomed => neptune_model::Layout::Tabs {
                    panes: tabs.to_vec(),
                    shown,
                },
                _ => workspace.layout().clone(),
            };
            let stage_view = ui::workspace::Stage {
                presentations: &presentations,
                active: workspace.active(),
                multiple: layout.panes().len() > 1,
                zoomed: self.ui.zoomed,
                keyboard: !overlay,
                previous_terminal: self.terminal_focus,
                config: &self.config,
                p,
                search: if self.ui.search_open {
                    &self.ui.search
                } else {
                    ""
                },
                drag: self.ui.pane_drag,
            };
            ui::workspace::draw_node(
                ui,
                &layout,
                stage,
                &mut self.renders,
                &stage_view,
                &mut actions,
                &mut output,
            );
            ui::workspace::pane_drag(ui, &stage_view, &output, &mut actions);
            ui::workspace::file_drop(ui, &stage_view, &output, file_drag, &mut actions);
        } else {
            ui::workspace::empty_state(ui, stage.drawn, p, self.startup.is_some(), &mut actions);
        }
        // A drag lasts only while its header reports it, so it cannot outlive a
        // release, a workspace switch or the pane itself.
        self.ui.pane_drag = output.dragging;
        // Drawn over the stage's edge, so its resize handle is reachable.
        if panel_edge > 0.0 {
            let panel = Rect::from_min_max(
                Pos2::new(bounds.right() - panel_edge, toolbar.bottom()),
                Pos2::new(bounds.right() - panel_edge + panel_width, bounds.bottom()),
            );
            let rows = if agents_shown {
                self.agent_rows()
            } else {
                Vec::new()
            };
            if !rows.is_empty() {
                Self::tick_agents(&ctx);
            }
            let files = self.explorer.view(
                !self.ui.explorer.query.trim().is_empty(),
                panel_reveal,
                bounds,
            );
            let selected = self.ui.changes.selected.clone();
            let changes = self.changes.view(
                (self.ui.changes.scope, selected.as_deref()),
                panel_reveal,
                bounds,
            );
            let view = ui::panel::View {
                files: &files,
                changes: &changes,
                agents: &ui::agents::View {
                    rows: &rows,
                    window: bounds,
                    reveal: panel_reveal,
                },
                waiting: self.agents.waiting_count(),
                window: bounds,
                reveal: panel_reveal,
            };
            ui::panel::show(
                ui,
                panel,
                p,
                &view,
                &mut self.ui.panel,
                ui::panel::Contents {
                    files: &mut self.ui.explorer,
                    changes: &mut self.ui.changes,
                },
                &mut actions,
            );
        }
        self.preview_image(ui, p, bounds, output.image_paths.take());
        for action in actions.drain(..) {
            self.action(&ctx, action);
        }
        // The focused terminal holds keyboard focus itself. Focus still on the
        // terminal that owned the keyboard a frame ago (after a split, close or
        // workspace switch) is in transit to the new one, so keys are not
        // dropped; any other focused widget owns typing.
        let focused = ctx.memory(|memory| memory.focused());
        let in_transit = focused.is_some() && focused == self.terminal_focus;
        let context = if self.ui.overlay != OverlayState::None || menu_open {
            crate::input::RoutingContext::Overlay
        } else if focused == Some(ui::search::input_id()) {
            crate::input::RoutingContext::TerminalSearch
        } else if focused.is_some() && focused != output.active_terminal && !in_transit {
            crate::input::RoutingContext::TextField
        } else if let Some(pane) = self.controller.model().active_pane() {
            crate::input::RoutingContext::TerminalPane(pane.get())
        } else {
            crate::input::RoutingContext::Overlay
        };
        let swallowed_paste = self.swallowed_paste.take();
        if let Some(rect) = output.active_body {
            self.terminal_input(&ctx, rect, context, swallowed_paste);
        }
        self.terminal_focus = output.active_terminal;
        // Sheets dim the whole window, following its rounded shape. A close
        // check owns input but has no sheet until confirmation is needed.
        let dim = ui::helpers::animate(
            &ctx,
            egui::Id::new("overlay-scrim"),
            !matches!(
                self.ui.overlay,
                OverlayState::None | OverlayState::Palette | OverlayState::Notifications
            ) && !(matches!(self.ui.overlay, OverlayState::ConfirmClose(_))
                && self.ui.close_status == ui::CloseStatus::Checking),
            0.16,
        );
        let dim = if self.ui.overlay == OverlayState::Palette {
            // Opened from the keyboard many times a day: no transition.
            1.0
        } else {
            dim
        };
        ui::helpers::scrim(ui.painter(), bounds, radius, p, dim);
        let zoomed = self.ui.zoomed;
        let message = self.ui.error.is_some();
        match self.ui.overlay {
            OverlayState::Notifications => {
                let model = self.controller.model();
                let mut programs = BTreeMap::new();
                let items: Vec<_> = self
                    .notifications
                    .entries()
                    .rev()
                    .map(|entry| {
                        let workspace = model
                            .workspace_for_pane(entry.pane)
                            .and_then(|id| model.workspace(id));
                        ui::notifications::NotificationItem {
                            sequence: entry.sequence,
                            pane: entry.pane,
                            generation: entry.generation,
                            identity: workspace.map_or(0, |w| w.id().get()),
                            workspace: workspace.map_or("Terminal", |w| w.name()),
                            program: programs
                                .entry(entry.pane)
                                .or_insert_with(|| {
                                    self.sessions.get(entry.pane).map_or_else(String::new, |s| {
                                        ui::workspace::pane_label(&s.metadata())
                                    })
                                })
                                .clone(),
                            title: &entry.notification.title,
                            body: &entry.notification.body,
                            unread: entry.unread,
                            age: entry.received.elapsed(),
                        }
                    })
                    .collect();
                ui::notifications::show(
                    &ctx,
                    p,
                    ui::notifications::NotificationView {
                        items: &items,
                        unavailable: self.desktop_notifier.unavailable,
                        first_frame: &mut self.ui.overlay_focus,
                    },
                    &mut actions,
                );
            }
            OverlayState::Settings => ui::preferences::show(
                &ctx,
                &self.config,
                &self.updates,
                &self.fonts,
                &mut self.ui.preferences,
                &mut self.ui.preference_view,
                &mut self.ui.shells,
                &mut actions,
            ),
            OverlayState::Update => ui::updates::show(&ctx, p, &self.updates, &mut actions),
            OverlayState::Palette => ui::palette::show(
                &ctx,
                p,
                &mut self.ui,
                &ui::palette::PaletteView {
                    pane: self.controller.model().active_pane(),
                    local: self
                        .controller
                        .model()
                        .active_workspace()
                        .and_then(|id| self.controller.model().workspace(id))
                        .is_some_and(|workspace| workspace.remote().is_none()),
                    worktree: self
                        .controller
                        .model()
                        .active_pane()
                        .and_then(|pane| self.controller.model().pane(pane))
                        .and_then(|pane| pane.worktree())
                        .map(|worktree| worktree.branch.as_str()),
                    pane_generation: active_pane
                        .and_then(|pane| self.controller.model().pane(pane))
                        .map_or(0, |p| p.generation()),
                    ports: active_pane
                        .and_then(|pane| presentations.get(&pane))
                        .map_or(&[], |p| p.ports.as_slice()),
                    layout: self
                        .controller
                        .model()
                        .active_workspace()
                        .and_then(|id| self.controller.model().workspace(id))
                        .map(|workspace| workspace.layout()),
                    workspaces: &views,
                    groups: self.controller.model().groups(),
                    active: self.controller.model().active_workspace(),
                    config: &self.config,
                    zoomed,
                    message,
                },
                &mut actions,
            ),
            _ => {}
        }
        ui::dialogs::show(&ctx, p, &mut self.ui, &mut actions);
        if self.ui.overlay == OverlayState::None && self.ui.error.is_none() {
            ui::updates::notification(&ctx, p, &self.updates, &mut actions);
        }
        for action in actions {
            self.action(&ctx, action);
        }
        // Apply Cancel/Escape before an asynchronous idle result can close a pane.
        self.poll_close(&ctx);
        self.send_startup_command();
        if self.command.is_some() || self.screenshot.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.screenshot.is_some() {
            if self.started.elapsed() > Duration::from_secs(3) && !self.capture_sent {
                crate::platform::window::send(
                    &ctx,
                    crate::platform::window::WindowOperation::Screenshot,
                );
                self.capture_sent = true;
            }
            for event in ctx.input(|i| i.events.clone()) {
                if let egui::Event::Screenshot { image, .. } = event
                    && let Some(path) = self.screenshot.take()
                {
                    match save_png(&path, &image) {
                        Ok(()) => println!("Screenshot saved to {}", path.display()),
                        Err(error) => eprintln!("Screenshot: {error:#}"),
                    };
                    self.exit_approved = true;
                    crate::platform::window::send(
                        &ctx,
                        crate::platform::window::WindowOperation::Close,
                    );
                }
            }
        }
        self.diagnostics.frame(
            self.frame_started.elapsed(),
            &self.sessions,
            &self.renders,
            self.controller.model(),
        );
        if radius > 0 {
            ui.painter().rect_stroke(
                bounds,
                radius,
                Stroke::new(1.0, p.border),
                egui::StrokeKind::Inside,
            );
        }
        ui::helpers::resize_edges(ui, bounds);
        self.release_closed_overlay_focus(&ctx);
    }
    fn on_exit(&mut self) {
        self.ports.shutdown(Duration::from_secs(8));
        // Capture reports received since the final frame before flushing state.
        let ctx = egui::Context::default();
        let metadata: Vec<_> = self
            .sessions
            .iter()
            .map(|(pane, session)| (pane, session.metadata()))
            .collect();
        for (pane, metadata) in metadata {
            self.sync_directory(&ctx, pane, &metadata);
        }
        for (pane, generation, agent) in self.sessions.agent_changes() {
            self.dispatch(
                &ctx,
                Command::PaneAgentChanged {
                    pane,
                    generation,
                    agent,
                },
            );
        }
        for (pane, generation, pull_request) in self.sessions.pull_request_links() {
            self.dispatch(
                &ctx,
                Command::PanePullRequestLinked {
                    pane,
                    generation,
                    pull_request,
                },
            );
        }
        for (pane, generation, attachment) in self.sessions.attached_files() {
            self.attached.forget(attachment.path());
            self.dispatch(
                &ctx,
                Command::PaneFileAttached {
                    pane,
                    generation,
                    attachment,
                },
            );
        }
        self.save_state();
        self.save_window();
        if let Some(writer) = &self.writer
            && let Err(error) = writer.flush()
        {
            eprintln!("Could not flush latest persisted state: {error}");
        }
        let complete = self.sessions.shutdown(Duration::from_secs(2));
        self.diagnostics.shutdown(complete);
        if self.relaunch
            && let Some(target) = self.updates.installed()
            && let Err(error) = crate::platform::updates::relaunch(target, &self.relaunch_arguments)
        {
            eprintln!("Could not restart Neptune after updating: {error}");
        }
    }
}
/// Where a workspace opens when no directory was chosen for it.
fn default_cwd() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().into())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}
fn set_session_palette(session: &terminal_core::TerminalSession, p: Palette) {
    use terminal_core::{NamedColor, Rgb};
    for (index, color) in p.ansi.iter().copied().enumerate().chain([
        (NamedColor::Foreground as usize, p.terminal_fg),
        (NamedColor::Background as usize, p.bg),
        (NamedColor::Cursor as usize, p.cursor),
    ]) {
        session.set_color(
            index,
            Rgb {
                r: color.r(),
                g: color.g(),
                b: color.b(),
            },
        );
    }
}
fn save_png(path: &std::path::Path, image: &egui::ColorImage) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(std::fs::File::create(path)?),
        image.size[0] as u32,
        image.size[1] as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(
        &image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect::<Vec<_>>(),
    )?;
    Ok(())
}
