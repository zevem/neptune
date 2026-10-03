//! Executes bounded effects and feeds completions to the same controller used by
//! shortcuts, sidebar, dialogs and pane widgets.
use super::*;
use crate::persistence::workspace_state::StateSnapshot;
use crate::runtime::persistence::SaveKind;

/// The system OpenSSH client, resolved through `PATH`.
pub(super) const SSH_CLIENT: &str = "ssh";

/// A local pane runs the configured shell. A remote pane runs the SSH client
/// in the same local directory and gets the login shell of its host. A remote
/// starting directory is passed as data in the remote bootstrap command.
pub(super) fn session_options(
    config: &Config,
    client: &str,
    cwd: PathBuf,
    remote: Option<&Remote>,
    remote_cwd: Option<&std::path::Path>,
) -> SessionOptions {
    let (shell, args) = match remote {
        Some(remote) => (Some(client.into()), ssh::arguments(remote, remote_cwd)),
        None => (config.shell.clone(), config.shell_args.clone()),
    };
    SessionOptions {
        cwd,
        cols: 100,
        rows: 32,
        scrollback: config.scrollback,
        shell,
        args,
        ..Default::default()
    }
}

/// Names a workspace after its host: `me@devbox` and `ssh://me@devbox` are
/// both `devbox`.
fn remote_label(destination: &str) -> String {
    let host = destination
        .strip_prefix("ssh://")
        .unwrap_or(destination)
        .rsplit('@')
        .next()
        .unwrap_or_default();
    if host.is_empty() { destination } else { host }.to_owned()
}

impl App {
    pub(super) fn dispatch(&mut self, ctx: &egui::Context, command: Command) {
        match self.controller.dispatch(command) {
            Ok(effects) => self.execute(ctx, effects),
            Err(error) => self.ui.error = Some(error.to_string()),
        }
    }
    pub(super) fn sync_directory(
        &mut self,
        ctx: &egui::Context,
        pane: PaneId,
        metadata: &SessionMetadata,
    ) {
        let Some(item) = self.controller.model().pane(pane) else {
            return;
        };
        let generation = item.generation();
        if self.sessions.generation(pane) != Some(generation) {
            return;
        }
        let command = if self.remote_of(pane).is_some() {
            metadata
                .reported_cwd
                .as_ref()
                .filter(|cwd| item.remote_cwd() != Some(cwd.as_path()))
                .map(|cwd| Command::PaneRemoteCwdChanged {
                    pane,
                    generation,
                    cwd: cwd.clone(),
                })
        } else {
            (metadata.cwd != item.cwd()).then(|| Command::PaneCwdChanged {
                pane,
                generation,
                cwd: metadata.cwd.clone(),
            })
        };
        if let Some(command) = command {
            self.dispatch(ctx, command);
        }
    }
    pub(super) fn execute(&mut self, ctx: &egui::Context, effects: Vec<Effect>) {
        let replacements: std::collections::HashSet<_> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::StartSession {
                    pane,
                    replacement: true,
                    ..
                } => Some(*pane),
                _ => None,
            })
            .collect();
        for effect in effects {
            match effect {
                Effect::StartSession {
                    pane,
                    generation,
                    cwd,
                    remote,
                    remote_cwd,
                    replacement,
                } => {
                    self.renders.insert(pane, PaneRender::new(pane));
                    let wake = ctx.clone();
                    let options = session_options(
                        &self.config,
                        &self.ssh_client,
                        cwd,
                        remote.as_ref(),
                        remote_cwd.as_deref(),
                    );
                    // A --command launch targets a shell, never a restored agent prompt.
                    if self.command.is_some() && self.controller.model().active_pane() == Some(pane)
                    {
                        self.dispatch(
                            ctx,
                            Command::PaneAgentChanged {
                                pane,
                                generation,
                                agent: None,
                            },
                        );
                    }
                    let launch = crate::runtime::sessions::SessionLaunch {
                        terminal: options,
                        agent: if remote.is_none() {
                            crate::runtime::sessions::AgentLaunch::Local {
                                resume: self
                                    .controller
                                    .model()
                                    .pane(pane)
                                    .and_then(|pane| pane.agent())
                                    .cloned(),
                            }
                        } else {
                            crate::runtime::sessions::AgentLaunch::Disabled
                        },
                    };
                    if let Err(error) = self.sessions.start(
                        pane,
                        generation,
                        replacement,
                        launch,
                        Arc::new(move || wake.request_repaint()),
                    ) {
                        self.diagnostics
                            .failure("start", Some(pane), Some(generation), "Runtime");
                        self.dispatch(
                            ctx,
                            Command::SessionFailed {
                                pane,
                                generation,
                                error: error.clone(),
                            },
                        );
                        self.ui.error = Some(format!("Pane {pane}: {error}"));
                    }
                }
                Effect::StopSession { pane, .. } => {
                    self.desktop_notifier.cancel(pane);
                    if !replacements.contains(&pane) {
                        self.sessions.close(pane);
                    }
                    self.renders.remove(&pane);
                }
                Effect::Focus { old, new } => {
                    if let Some(pane) = new {
                        self.notifications.acknowledge(Some(pane));
                    }
                    if let Some(id) = old
                        && let Some(session) = self.sessions.get(id)
                    {
                        let _ = session.focus(false);
                        if let Some(render) = self.renders.get_mut(&id)
                            && let Some(button) = render.mouse_button.take()
                        {
                            let (col, row, _) = render.mouse_cell.unwrap_or((0, 0, None));
                            if let Some(bytes) = crate::input::mouse(
                                button,
                                col,
                                row,
                                false,
                                Default::default(),
                                render.cache.mode,
                            ) {
                                let _ = session.write(&bytes);
                            }
                        }
                    }
                    if let Some(id) = new
                        && let Some(session) = self.sessions.get(id)
                    {
                        let _ = session.focus(Self::window_has_focus(ctx));
                    }
                }
                Effect::ResetSearch => {
                    self.search_point = None;
                    self.search_task = None;
                    self.ui.search_error = None;
                }
                Effect::Persist { .. } => self.save_state(),
                Effect::SavePreferences => {
                    self.preference_generation += 1;
                    if let Some(writer) = &self.writer
                        && let Err(error) =
                            writer.submit_config(self.preference_generation, self.config.clone())
                    {
                        self.ui.error = Some(error);
                    }
                }
            }
        }
    }
    pub(super) fn save_state(&mut self) {
        if self.ephemeral || !self.state_writable {
            return;
        }
        if let Some(writer) = &self.writer
            && let Err(error) = writer.submit_state(
                self.controller.generation(),
                StateSnapshot::from_model(self.controller.model()),
            )
        {
            self.ui.error = Some(format!("Cannot queue workspace save: {error}"));
        }
    }
    pub(super) fn observe_window(&mut self, ctx: &egui::Context, size: egui::Vec2) {
        if self.ephemeral || !self.window_writable {
            return;
        }
        if self.restore_maximized {
            // eframe maps the initially hidden window after its first render.
            // Some window managers ignore the builder's pre-map maximize
            // request. Reapply it after rendering and keep startup observations
            // from replacing the saved geometry while the OS handles it.
            if ctx.input(|input| input.viewport().maximized) != Some(true)
                && self.started.elapsed() < Duration::from_secs(2)
            {
                crate::platform::window::send(
                    ctx,
                    crate::platform::window::WindowOperation::SetMaximized(true),
                );
                ctx.request_repaint_after(Duration::from_millis(30));
                return;
            }
            self.restore_maximized = false;
        }
        // Save native logical dimensions, not the zoomed content area,
        // so reopening a zoomed window does not shrink it on every launch.
        let size = size * ctx.zoom_factor();
        let changed = ctx.input(|input| {
            let viewport = input.viewport();
            self.window_state.observe(
                [size.x, size.y],
                viewport.maximized,
                viewport.minimized.unwrap_or(false),
                viewport.fullscreen.unwrap_or(false),
            )
        });
        if changed {
            self.window_generation += 1;
            self.save_window();
        }
    }
    pub(super) fn save_window(&mut self) {
        if self.ephemeral || !self.window_writable {
            return;
        }
        if let Some(writer) = &self.writer
            && let Err(error) = writer.submit_window(self.window_generation, self.window_state)
        {
            self.ui.error = Some(format!("Cannot queue window save: {error}"));
        }
    }
    pub(super) fn poll_saves(&mut self, ctx: &egui::Context) {
        let events = self
            .writer
            .as_ref()
            .map(|writer| writer.drain_events())
            .unwrap_or_default();
        for event in events {
            match event.result {
                Ok(()) => {
                    if event.kind == SaveKind::State {
                        self.dispatch(
                            ctx,
                            Command::AcknowledgeSave {
                                generation: event.generation,
                            },
                        );
                    }
                }
                Err(error) => {
                    self.diagnostics.failure(
                        match event.kind {
                            SaveKind::State => "save_state",
                            SaveKind::Config => "save_preferences",
                            SaveKind::Window => "save_window",
                        },
                        None,
                        Some(event.generation),
                        "Persistence",
                    );
                    self.ui.error = Some(format!(
                        "{:?} save {} failed: {error}",
                        event.kind, event.generation
                    ))
                }
            }
        }
    }
    pub(super) fn action(&mut self, ctx: &egui::Context, action: Action) {
        if self.startup.is_some()
            && matches!(
                action,
                Action::Create(..)
                    | Action::CreateInGroup(..)
                    | Action::CreateGroup(_)
                    | Action::ConnectInGroup(..)
                    | Action::Connect { .. }
                    | Action::Preferences(_)
                    | Action::ZoomUiIn
                    | Action::ZoomUiOut
                    | Action::ResetUiZoom
                    | Action::ToggleSidebar
            )
        {
            if matches!(action, Action::Preferences(_)) {
                self.deferred_actions
                    .retain(|pending| !matches!(pending, Action::Preferences(_)));
            }
            if self.deferred_actions.len() >= 24 {
                self.ui.error =
                    Some("Too many workspace operations are waiting for restoration".into());
                return;
            }
            if matches!(
                action,
                Action::Create(..)
                    | Action::CreateInGroup(..)
                    | Action::CreateGroup(_)
                    | Action::ConnectInGroup(..)
                    | Action::Connect { .. }
            ) {
                self.ui.overlay = OverlayState::None;
            }
            self.deferred_actions.push(action);
            ctx.request_repaint();
            return;
        }
        match action {
            Action::CheckUpdates => self.updates.check(ctx),
            Action::ReviewUpdate => {
                if self.updates.release.is_some() {
                    self.ui.overlay = OverlayState::Update;
                }
            }
            Action::DownloadUpdate(version) => {
                if self
                    .updates
                    .release
                    .as_ref()
                    .is_some_and(|release| release.version == version)
                {
                    self.updates.download(ctx);
                }
            }
            Action::OpenUpdate(version) => {
                if self
                    .updates
                    .release
                    .as_ref()
                    .is_some_and(|release| release.version == version)
                {
                    self.updates.open(ctx);
                }
            }
            Action::CancelUpdate => self.updates.cancel(),
            Action::DismissUpdate => self.updates.dismiss(),
            Action::Create(cwd, name) => self.create_workspace(ctx, cwd, name, None, None),
            Action::CreateInGroup(cwd, group) => {
                self.create_workspace(ctx, cwd, None, None, Some(group))
            }
            Action::NewInGroup(group) => {
                if let Some(dirs) = directories::BaseDirs::new() {
                    self.action(ctx, Action::CreateInGroup(dirs.home_dir().into(), group));
                } else {
                    self.ui.error = Some("Could not determine your home directory".into());
                }
            }
            Action::NewGroup => {
                self.ui.rename_name.clear();
                self.ui.overlay = OverlayState::NewGroup;
                self.ui.overlay_focus = true;
            }
            Action::CreateGroup(name) => {
                self.dispatch(ctx, Command::AddWorkspaceGroup { name });
                self.ui.overlay = OverlayState::None;
            }
            Action::RenameGroup(group) => {
                if let Some(folder) = self.controller.model().group(group) {
                    self.ui.rename_name = folder.name().into();
                    self.ui.overlay = OverlayState::RenameGroup(group);
                    self.ui.overlay_focus = true;
                }
            }
            Action::SetGroupName(group, name) => {
                self.dispatch(ctx, Command::RenameWorkspaceGroup { group, name })
            }
            Action::GroupDefaultDirectory(group) => self.edit_directory(group),
            Action::BrowseGroupDirectory(group) => self.browse_directory(ctx, group),
            Action::SetGroupDefaultDirectory(group, directory) => {
                self.set_directory(ctx, group, directory)
            }
            Action::SetGroupCollapsed(group, collapsed) => self.dispatch(
                ctx,
                Command::SetWorkspaceGroupCollapsed { group, collapsed },
            ),
            Action::MoveToGroup(workspace, group) => {
                self.dispatch(ctx, Command::SetWorkspaceGroup { workspace, group })
            }
            Action::RemoveGroup(group) => self.dispatch(ctx, Command::RemoveWorkspaceGroup(group)),
            Action::MoveSidebarItem(item, index) => {
                self.dispatch(ctx, Command::MoveSidebarItem { item, index })
            }
            Action::SshInGroup(group) => {
                if self.controller.model().group(group).is_some() {
                    self.ui.ssh_host.clear();
                    self.ui.overlay = OverlayState::SshInGroup(group);
                    self.ui.overlay_focus = true;
                }
            }
            Action::ConnectInGroup(group, destination) => {
                self.create_workspace(ctx, default_cwd(), None, Some(destination), Some(group))
            }
            Action::Ssh(workspace) => {
                // Only a local workspace can be connected; a remote one is
                // disconnected first.
                if workspace.is_some_and(|id| {
                    self.controller
                        .model()
                        .workspace(id)
                        .is_none_or(|workspace| workspace.remote().is_some())
                }) {
                    return;
                }
                self.ui.ssh_host.clear();
                self.ui.overlay = OverlayState::Ssh(workspace);
                self.ui.overlay_focus = true;
            }
            Action::Connect {
                workspace: None,
                destination,
            } => self.create_workspace(ctx, default_cwd(), None, Some(destination), None),
            Action::Connect {
                workspace: Some(workspace),
                destination,
            } => {
                self.dispatch(
                    ctx,
                    Command::SetWorkspaceRemote {
                        workspace,
                        remote: Some(destination),
                    },
                );
                self.ui.overlay = OverlayState::None;
            }
            Action::Disconnect(workspace) => self.request_close(ctx, Close::Connection(workspace)),
            Action::Split(pane, axis) => self.open_beside(ctx, pane, Some(axis)),
            Action::NewTab(pane) => self.open_beside(ctx, pane, None),
            Action::SelectWorkspace(id) => {
                self.dispatch(ctx, Command::SelectWorkspace(id));
                if self.controller.model().active_workspace() == Some(id)
                    && let Some(pane) = self.controller.model().active_pane()
                {
                    self.notifications.acknowledge(Some(pane));
                }
            }
            Action::MoveWorkspace(workspace, index) => {
                self.dispatch(ctx, Command::MoveWorkspace { workspace, index })
            }
            Action::Focus(pane) => {
                self.notifications.acknowledge(Some(pane));
                if let Some(workspace) = self.controller.model().workspace_for_pane(pane) {
                    self.dispatch(ctx, Command::FocusPane { workspace, pane });
                }
            }
            Action::Ratio(split, ratio) => {
                self.dispatch(ctx, Command::SetSplitRatio { split, ratio })
            }
            Action::MovePane(pane, destination) => {
                self.dispatch(ctx, Command::MovePane { pane, destination })
            }
            Action::ClosePane(pane) => self.request_close(ctx, Close::Pane(pane)),
            Action::CloseWorkspace(id) => self.request_close(ctx, Close::Workspace(id)),
            Action::WindowClose => self.request_close(ctx, Close::App),
            Action::New => {
                if let Some(dirs) = directories::BaseDirs::new() {
                    self.action(ctx, Action::Create(dirs.home_dir().into(), None));
                } else {
                    self.ui.error = Some("Could not determine your home directory".into());
                }
            }
            Action::Rename(id) => {
                if let Some(w) = self.controller.model().workspace(id) {
                    self.ui.rename_name = w.name().into();
                    self.ui.overlay = OverlayState::Rename(id);
                    self.ui.overlay_focus = true;
                }
            }
            Action::SetName(workspace, name) => {
                self.dispatch(ctx, Command::RenameWorkspace { workspace, name })
            }
            Action::Notifications => {
                self.ui.overlay = if self.ui.overlay == OverlayState::Notifications {
                    OverlayState::None
                } else {
                    OverlayState::Notifications
                };
                self.ui.overlay_focus = true;
            }
            Action::OpenNotification(pane, generation) => {
                if self
                    .controller
                    .model()
                    .pane(pane)
                    .is_some_and(|p| p.generation() == generation)
                {
                    self.ui.overlay = OverlayState::None;
                    self.ui.zoomed = false;
                    if let Some(workspace) = self.controller.model().workspace_for_pane(pane) {
                        // Focus the pane before revealing its workspace, so the
                        // workspace's previous pane is never focused in passing
                        // and keeps its own unread alerts.
                        self.action(ctx, Action::Focus(pane));
                        self.dispatch(ctx, Command::SelectWorkspace(workspace));
                    }
                }
            }
            Action::DismissNotification(sequence) => self.notifications.dismiss(sequence),
            Action::ReadNotifications => self.notifications.acknowledge(None),
            Action::ClearNotifications => self.notifications.clear(),
            Action::Settings => {
                if self.ui.overlay == OverlayState::Settings {
                    // A changed theme draft asks before the sheet closes.
                    if self.ui.preferences.may_close() {
                        self.ui.overlay = OverlayState::None;
                    }
                } else {
                    // A draft set aside by another overlay is still there.
                    if !self.ui.preferences.editing() {
                        self.ui.preferences = Default::default();
                    }
                    self.ui.shells = Default::default();
                    self.ui.preference_view.opened();
                    self.ui.overlay = OverlayState::Settings;
                }
            }
            Action::Themes => {
                if !self.ui.preferences.editing() {
                    self.ui.preferences = Default::default();
                    self.ui.preferences.browse();
                }
                // Leaving the catalog returns to the pane that opens it.
                self.ui.preference_view.pane = ui::preferences::Pane::Appearance;
                self.ui.shells = Default::default();
                self.ui.overlay = OverlayState::Settings;
            }
            Action::Palette => {
                self.ui.palette_query.clear();
                self.ui.palette_selected = 0;
                self.ui.overlay = if self.ui.overlay == OverlayState::Palette {
                    OverlayState::None
                } else {
                    OverlayState::Palette
                };
            }
            Action::ToggleSidebar => {
                let shown = self.controller.model().sidebar();
                self.ui.sidebar_slide = Some(ui::chrome::SidebarSlide::toggled(
                    self.ui.sidebar_slide,
                    shown,
                    ctx.input(|input| input.time),
                ));
                self.dispatch(ctx, Command::SetSidebar(!shown));
                ctx.request_repaint();
            }
            Action::Explorer(event) => self.explorer_event(ctx, event),
            Action::SidebarWidth(width) => {
                let config = Config {
                    sidebar_width: width.clamp(170.0, 360.0),
                    ..self.config.clone()
                };
                self.action(ctx, Action::Preferences(config));
            }
            Action::Zoom => self.ui.zoomed = !self.ui.zoomed,
            action @ (Action::ZoomUiIn | Action::ZoomUiOut | Action::ResetUiZoom) => {
                let zoom = match action {
                    Action::ZoomUiIn => ((self.config.window_zoom + 0.1) * 10.0).round() / 10.0,
                    Action::ZoomUiOut => ((self.config.window_zoom - 0.1) * 10.0).round() / 10.0,
                    _ => Config::default().window_zoom,
                };
                let zoom = zoom.clamp(
                    *Config::WINDOW_ZOOM_RANGE.start(),
                    *Config::WINDOW_ZOOM_RANGE.end(),
                );
                if zoom != self.config.window_zoom {
                    self.action(
                        ctx,
                        Action::Preferences(Config {
                            window_zoom: zoom,
                            ..self.config.clone()
                        }),
                    );
                }
            }
            Action::Find => {
                let editing = ctx.memory(|memory| memory.has_focus(ui::search::input_id()));
                if self.ui.search_open && !editing {
                    // Search is open but the terminal has the keyboard: return
                    // to the field instead of closing it.
                    self.ui.search_focus = true;
                } else {
                    self.ui.search_open = !self.ui.search_open;
                    self.ui.search_focus = self.ui.search_open;
                    self.search_point = None;
                    self.search_task = None;
                }
            }
            Action::SearchChanged => {
                self.search_point = None;
                self.find_next(false);
            }
            Action::FindNext { reverse } => self.find_next(reverse),
            Action::CloseSearch => {
                self.ui.search_open = false;
                self.search_task = None;
            }
            Action::CloseOverlay => {
                self.cancel_directory_check();
                self.pending_close = None;
                if self.ui.overlay == OverlayState::Update {
                    self.updates.dismiss();
                }
                self.ui.explorer.delete = None;
                self.ui.overlay = OverlayState::None;
            }
            Action::DismissError => self.ui.error = None,
            Action::Clear(pane) => {
                if let Some(session) = self.sessions.get(pane) {
                    session.clear_history();
                    session.scroll_to_bottom();
                    if let Err(error) = session.write(b"\x0c") {
                        self.ui.error = Some(error.to_string());
                    }
                }
            }
            Action::Restart(pane) => {
                if let Some(metadata) = self.sessions.get(pane).map(|session| session.metadata()) {
                    self.sync_directory(ctx, pane, &metadata);
                }
                self.dispatch(ctx, Command::RestartPane(pane));
            }
            Action::Copy(pane) => {
                if let Some(text) = self
                    .sessions
                    .get(pane)
                    .and_then(|session| session.selected_text())
                {
                    crate::platform::clipboard::copy(ctx, text);
                }
            }
            Action::Paste(pane) => self.paste_clipboard(ctx, pane, false),
            Action::DropFiles(pane, paths) => {
                self.action(ctx, Action::Focus(pane));
                self.paste_paths(pane, &paths);
            }
            Action::Preferences(mut config) => {
                if let Err(error) = config.validate() {
                    self.ui.error = Some(error.to_string());
                    return;
                }
                if !config.desktop_notifications {
                    self.desktop_notifier.cancel_all();
                }
                self.config = config;
                self.updates.configure(self.config.release_channel);
                ctx.set_zoom_factor(self.config.window_zoom);
                theme::apply(ctx, &self.config);
                for (_, session) in self.sessions.iter() {
                    set_session_palette(session, Palette::for_config(&self.config));
                }
                self.dispatch(ctx, Command::UpdatePreferences);
            }
            Action::Confirm(close) => self.confirm_close(ctx, close),
            Action::CancelClose => {
                self.pending_close = None;
                self.ui.overlay = OverlayState::None;
            }
            Action::Resize(pane, geometry) => {
                let result = self.sessions.resize(
                    pane,
                    geometry.columns,
                    geometry.lines,
                    geometry.pixel_width,
                    geometry.pixel_height,
                    self.config.scrollback,
                );
                if let Some(render) = self.renders.get_mut(&pane) {
                    render.cache.resize_error = result.err();
                }
                ctx.request_repaint();
            }
            Action::Selection(pane, interaction) => {
                if let Some(session) = self.sessions.get(pane) {
                    use crate::terminal_view::SelectionInteraction;
                    match interaction {
                        SelectionInteraction::Start { point, kind } => {
                            session.start_selection_at(point, kind)
                        }
                        SelectionInteraction::Update(point) => session.update_selection_at(point),
                        SelectionInteraction::Clear => session.clear_selection(),
                    }
                }
            }
            Action::OpenLink(link) => {
                if let Err(error) = self.link_opener.open(link, ctx.clone()) {
                    self.ui.error = Some(error.into());
                }
            }
            Action::ScrollBottom(pane) => {
                if let Some(session) = self.sessions.get(pane) {
                    session.scroll_to_bottom();
                }
            }
        }
    }

    /// Opens a terminal beside `pane`, in the directory that terminal is in:
    /// across a split, or as a tab in the same place without an axis.
    fn open_beside(
        &mut self,
        ctx: &egui::Context,
        pane: PaneId,
        axis: Option<neptune_model::Axis>,
    ) {
        let Some(workspace) = self.controller.model().workspace_for_pane(pane) else {
            return;
        };
        let local = self.remote_of(pane).is_none();
        let metadata = self.sessions.get(pane).map(|session| session.metadata());
        if let Some(metadata) = &metadata {
            self.sync_directory(ctx, pane, metadata);
        }
        let cwd = self
            .controller
            .model()
            .pane(pane)
            .map(|p| p.cwd().to_path_buf());
        let cwd = metadata
            .as_ref()
            .filter(|_| local)
            .map(|metadata| metadata.cwd.clone())
            .or(cwd)
            .unwrap_or_default();
        self.dispatch(
            ctx,
            match axis {
                Some(axis) => Command::SplitPane {
                    workspace,
                    pane,
                    axis,
                    cwd,
                },
                None => Command::AddTab {
                    workspace,
                    pane,
                    cwd,
                },
            },
        );
    }

    pub(super) fn create_workspace(
        &mut self,
        ctx: &egui::Context,
        cwd: PathBuf,
        name: Option<String>,
        remote: Option<String>,
        group: Option<neptune_model::WorkspaceGroupId>,
    ) {
        let cwd = if remote.is_none() {
            group
                .and_then(|id| self.controller.model().group(id))
                .and_then(neptune_model::WorkspaceGroup::default_directory)
                .map(PathBuf::from)
                .unwrap_or(cwd)
        } else {
            cwd
        };
        let name = name.unwrap_or_else(|| match &remote {
            Some(destination) => remote_label(destination),
            None => cwd
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("Home"))
                .to_string_lossy()
                .into_owned(),
        });
        self.dispatch(
            ctx,
            Command::AddWorkspace {
                cwd,
                name,
                remote,
                group,
            },
        );
        self.ui.overlay = OverlayState::None;
    }

    pub(super) fn send_startup_command(&mut self) {
        if self.command.is_none() || self.started.elapsed() < Duration::from_millis(650) {
            return;
        }
        let Some((pane, generation)) = self.command_target else {
            return;
        };
        if self
            .controller
            .model()
            .pane(pane)
            .is_none_or(|target| target.generation() != generation)
        {
            self.command = None;
            self.ui.error = Some(format!(
                "Startup command cancelled: pane {pane} was closed or restarted"
            ));
            return;
        }
        // Typed text could answer a password or host-key prompt instead of
        // reaching a shell.
        if self.remote_of(pane).is_some() {
            self.command = None;
            self.ui.error.get_or_insert_with(|| {
                format!("Startup command cancelled: pane {pane} is connected over SSH")
            });
            return;
        }
        if self.controller.model().pane(pane).is_some_and(|target| {
            matches!(target.lifecycle(), Lifecycle::Failed(_) | Lifecycle::Exited)
        }) {
            self.command = None;
            self.ui.error.get_or_insert_with(|| {
                format!("Startup command cancelled: pane {pane} is unavailable")
            });
            return;
        }
        if let Some(session) = self.sessions.get(pane)
            && let Some(command) = self.command.take()
            && let Err(error) = session.write(format!("{command}\r").as_bytes())
        {
            self.diagnostics.failure(
                "launch_command",
                Some(pane),
                Some(generation),
                &format!("{:?}", error.kind()),
            );
            self.ui.error = Some(error.to_string());
        }
    }
    /// Ends a terminal drag without moving anything.
    pub(super) fn cancel_pane_drag(&mut self, ctx: &egui::Context) {
        self.ui.pane_drag = None;
        ctx.stop_dragging();
    }
}
