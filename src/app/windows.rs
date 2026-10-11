//! Native windows share model identities, processes and the single state writer.
//! Only presentation and input state move in and out of the current frame.
use super::*;
use neptune_model::{WindowId, WorkspaceId};

pub(super) struct ProjectEdits {
    project: neptune_model::ProjectId,
    draft: Option<String>,
    attached: Option<Vec<crate::projects::transcript::Attachment>>,
    instructions: Option<String>,
}

pub(super) struct Windows {
    /// State for inactive windows; the current window's state lives on App.
    states: BTreeMap<WindowId, Presentation>,
    launches: BTreeMap<WindowId, LaunchWindow>,
    root_hidden: bool,
    current: WindowId,
    /// The model window displayed by eframe's required native root host.
    host: WindowId,
    viewports: BTreeMap<WindowId, egui::ViewportId>,
    keyboard: BTreeMap<WindowId, bool>,
    selections: BTreeMap<WindowId, Option<PaneId>>,
    focus_targets: BTreeMap<WindowId, Option<PaneId>>,
    pointer: crate::platform::workspace_drag::Monitor,
    drag: Option<WorkspaceDrag>,
    drop_zones: BTreeMap<WindowId, DropZones>,
    released: std::collections::BTreeSet<egui::ViewportId>,
    raise_after_mapping: Option<egui::ViewportId>,
    lower_after_raise: Option<egui::ViewportId>,
}

struct WorkspaceDrag {
    workspace: WorkspaceId,
    window: WindowId,
    /// Native surface that received the press; fixed for this entire gesture.
    viewport: egui::ViewportId,
    watch: crate::platform::workspace_drag::Watch,
    /// Original press relative to the row, in egui points.
    grip: Vec2,
    /// Last desktop pointer sample, in physical pixels.
    pointer: Pos2,
    mode: WorkspaceDragMode,
}

#[derive(Clone, Copy)]
enum WorkspaceDragMode {
    Floating { anchor: Option<Vec2> },
    Docked,
}

struct DropZones {
    sidebar: Option<Rect>,
    toolbar: Rect,
    last_focused: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &std::path::Path) -> (App, Vec<WorkspaceId>) {
        let (mut app, _) = super::super::tests::fixture(root);
        app.startup = None;
        app.command = None;
        let mut workspaces = Vec::new();
        for name in ["One", "Two", "Three"] {
            app.controller
                .dispatch(Command::AddWorkspace {
                    cwd: root.into(),
                    name: name.into(),
                    group: None,
                    remote: None,
                })
                .unwrap();
            workspaces.push(app.controller.model().active_workspace().unwrap());
        }
        (app, workspaces)
    }

    fn detach(app: &mut App, ctx: &egui::Context, workspace: WorkspaceId, held: bool) -> WindowId {
        ctx.set_embed_viewports(false);
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
            ..Default::default()
        };
        input.events = vec![egui::Event::PointerMoved(Pos2::new(300.0, 90.0))];
        if held {
            input.events.push(egui::Event::PointerButton {
                pos: Pos2::new(300.0, 90.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
        }
        ctx.run_ui(input, |ui| {
            app.detach_workspace(ui.ctx(), workspace, Some(Pos2::new(300.0, 90.0)), None)
        })
        .textures_delta
        .clear();
        app.controller
            .model()
            .workspace(workspace)
            .unwrap()
            .window()
    }

    #[test]
    fn held_tear_off_keeps_the_pressed_native_window_and_creates_an_inactive_replacement() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        let before = app
            .controller
            .model()
            .workspace(workspaces[2])
            .unwrap()
            .clone();
        let child = detach(&mut app, &ctx, workspaces[2], true);
        assert_eq!(app.viewport_of(child), egui::ViewportId::ROOT);
        assert_ne!(app.viewport_of(WindowId::ROOT), egui::ViewportId::ROOT);
        assert_eq!(app.host_window(), child);
        assert!(!app.windows.launches[&WindowId::ROOT].first_frame);
        assert!(app.windows.launches[&WindowId::ROOT].defer_creation);
        assert_eq!(
            app.controller
                .model()
                .workspace(workspaces[2])
                .unwrap()
                .panes(),
            before.panes()
        );
        assert_eq!(
            app.controller
                .model()
                .workspace(workspaces[2])
                .unwrap()
                .layout(),
            before.layout()
        );
    }

    #[test]
    fn a_sole_workspace_does_not_leave_a_creation_deferral_on_the_empty_source() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        for workspace in &workspaces[..2] {
            app.controller
                .dispatch(Command::CloseWorkspace(*workspace))
                .unwrap();
        }
        let child = detach(&mut app, &ctx, workspaces[2], true);
        assert!(!app.windows.launches[&WindowId::ROOT].defer_creation);
        app.enter_window(child);
        app.move_workspace_to_window(&ctx, workspaces[2], WindowId::ROOT);
        assert_eq!(app.controller.model().workspaces().len(), 1);
        assert!(!app.windows.launches[&WindowId::ROOT].defer_creation);
    }

    #[test]
    fn moving_to_a_minimized_destination_reveals_it_before_requesting_focus() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        let child = detach(&mut app, &ctx, workspaces[2], false);
        let target = app.viewport_of(child);
        let mut input = egui::RawInput::default();
        input.viewports.insert(
            target,
            egui::ViewportInfo {
                minimized: Some(true),
                focused: Some(false),
                ..Default::default()
            },
        );
        let output = ctx.run_ui(input, |ui| {
            ui.ctx()
                .show_viewport_deferred(target, Default::default(), |_, _| {});
            app.move_workspace_to_window(ui.ctx(), workspaces[1], child)
        });
        let commands = &output.viewport_output[&target].commands;
        let unminimize = commands
            .iter()
            .position(|command| *command == egui::ViewportCommand::Minimized(false))
            .unwrap();
        let show = commands
            .iter()
            .position(|command| *command == egui::ViewportCommand::Visible(true))
            .unwrap();
        let focus = commands
            .iter()
            .position(|command| *command == egui::ViewportCommand::Focus)
            .unwrap();
        assert!(unminimize < show && show < focus);
        assert_eq!(
            app.controller
                .model()
                .workspace(workspaces[1])
                .unwrap()
                .window(),
            child
        );
    }

    #[test]
    fn project_message_files_and_context_edits_follow_detach_and_rejoin() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        for (workspace, key) in [
            (workspaces[0], "0000000000000001"),
            (workspaces[2], "0000000000000002"),
        ] {
            app.controller
                .dispatch(Command::AddProject {
                    workspace,
                    name: "Project".into(),
                    key: neptune_model::ProjectKey::parse(key).unwrap(),
                    lead: neptune_model::AgentKind::Codex,
                })
                .unwrap();
        }
        let moved = app
            .controller
            .model()
            .project_of(workspaces[2])
            .unwrap()
            .id();
        let retained = app
            .controller
            .model()
            .project_of(workspaces[0])
            .unwrap()
            .id();
        let file = crate::projects::transcript::Attachment {
            path: "notes.txt".into(),
            bytes: 5,
        };
        app.ui
            .project
            .drafts
            .insert(retained, "Other workspace's draft".into());
        app.ui.project.drafts.insert(moved, "Unsent message".into());
        app.ui.project.attached.insert(moved, vec![file.clone()]);
        app.ui
            .project
            .instructions
            .insert(moved, "Unsaved context".into());
        app.dispatch(
            &ctx,
            Command::MoveWorkspaceToWindow {
                workspace: workspaces[2],
                window: WindowId::new(999),
            },
        );
        assert_eq!(app.ui.project.drafts[&moved], "Unsent message");
        let child = detach(&mut app, &ctx, workspaces[2], false);
        assert_eq!(app.ui.project.drafts[&retained], "Other workspace's draft");
        assert!(!app.ui.project.drafts.contains_key(&moved));
        app.enter_window(child);
        assert_eq!(app.ui.project.drafts[&moved], "Unsent message");
        assert_eq!(
            app.ui.project.attached[&moved].as_slice(),
            std::slice::from_ref(&file)
        );
        assert_eq!(app.ui.project.instructions[&moved], "Unsaved context");
        app.move_workspace_to_window(&ctx, workspaces[2], WindowId::ROOT);
        assert!(app.controller.model().window(child).is_none());
        app.enter_window(WindowId::ROOT);
        assert_eq!(app.ui.project.drafts[&moved], "Unsent message");
        assert_eq!(app.ui.project.attached[&moved], [file]);
        assert_eq!(app.ui.project.instructions[&moved], "Unsaved context");
        assert_eq!(app.ui.project.drafts[&retained], "Other workspace's draft");
    }

    #[test]
    fn menu_tear_off_creates_another_viewport_and_preserves_the_original_host() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        let child = detach(&mut app, &ctx, workspaces[1], false);
        assert_ne!(app.viewport_of(child), egui::ViewportId::ROOT);
        assert_eq!(app.host_window(), WindowId::ROOT);
    }

    #[test]
    fn sheets_search_and_keyboard_state_belong_to_each_window_even_when_model_context_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        let child = detach(&mut app, &ctx, workspaces[1], false);
        app.ui.search = "root search".into();
        app.ui.overlay = OverlayState::Settings;
        app.enter_window(child);
        assert!(app.ui.search.is_empty());
        assert_eq!(app.ui.overlay, OverlayState::None);
        app.ui.search = "child search".into();
        app.controller
            .dispatch(Command::CloseWindow(child))
            .unwrap();
        app.enter_window(WindowId::ROOT);
        assert_eq!(app.ui.search, "root search");
        assert_eq!(app.ui.overlay, OverlayState::Settings);
    }

    #[test]
    fn closing_the_physical_host_keeps_its_siblings_and_the_final_window_preserves_restoration() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        let child = detach(&mut app, &ctx, workspaces[2], true);
        app.enter_window(child);
        app.close_window(&ctx, child);
        assert!(!app.host_exists());
        assert!(!app.exit_approved);
        assert_eq!(app.controller.model().workspaces().len(), 2);
        app.enter_window(WindowId::ROOT);
        app.close_window(&ctx, WindowId::ROOT);
        assert!(app.exit_approved);
        assert_eq!(app.controller.model().workspaces().len(), 2);
    }

    #[test]
    fn reorder_indices_follow_visible_workspaces_instead_of_hidden_siblings() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        detach(&mut app, &ctx, workspaces[1], false);
        app.action(&ctx, Action::MoveWorkspace(workspaces[0], 1));
        let visible: Vec<_> = app.views().iter().map(|w| w.id).collect();
        assert_eq!(visible, [workspaces[2], workspaces[0]]);
        app.action(
            &ctx,
            Action::MoveSidebarItem(neptune_model::SidebarItem::Workspace(workspaces[0]), 0),
        );
        assert_eq!(
            app.views().iter().map(|w| w.id).collect::<Vec<_>>(),
            [workspaces[0], workspaces[2]]
        );
    }

    #[test]
    fn visible_browser_panes_and_focus_are_aggregated_across_native_windows() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        let child = detach(&mut app, &ctx, workspaces[1], false);
        let pane = app
            .controller
            .model()
            .workspace(workspaces[1])
            .unwrap()
            .active();
        let child_viewport = app.viewport_of(child);
        app.windows.keyboard.insert(child, true);
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .minimized = Some(true);
        input.viewports.insert(
            child_viewport,
            egui::ViewportInfo {
                focused: Some(true),
                minimized: Some(false),
                ..Default::default()
            },
        );
        ctx.run_ui(input, |ui| {
            assert!(app.visible_panes(ui.ctx()).contains(&pane));
            assert_eq!(app.focused_pane(ui.ctx()), Some(pane));
        })
        .textures_delta
        .clear();
    }

    #[test]
    fn minimizing_every_window_updates_inactive_terminal_focus_without_child_frames() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        let child = detach(&mut app, &ctx, workspaces[1], false);
        app.enter_window(child);
        app.window_focused = Some(true);
        app.enter_window(WindowId::ROOT);
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .minimized = Some(true);
        input.viewports.insert(
            app.viewport_of(child),
            egui::ViewportInfo {
                focused: Some(false),
                minimized: Some(true),
                ..Default::default()
            },
        );
        ctx.run_ui(input, |ui| app.reconcile_native_focus(ui.ctx()))
            .textures_delta
            .clear();
        assert_eq!(app.window_focused, Some(false));
        assert_eq!(app.windows.states[&child].window_focused, Some(false));
    }

    #[test]
    fn final_pane_close_after_held_detach_restores_a_usable_native_root() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        for workspace in &workspaces[..2] {
            app.controller
                .dispatch(Command::CloseWorkspace(*workspace))
                .unwrap();
        }
        let child = detach(&mut app, &ctx, workspaces[2], true);
        app.enter_window(child);
        let pane = app.controller.model().active_pane().unwrap();
        app.controller.dispatch(Command::ClosePane(pane)).unwrap();
        assert!(!app.host_exists());
        app.reconcile_empty_host(&ctx);
        assert!(app.host_exists());
        assert_eq!(app.windows.current, WindowId::ROOT);
        assert_eq!(app.viewport_of(WindowId::ROOT), egui::ViewportId::ROOT);
        assert!(app.controller.model().workspaces().is_empty());
        assert!(!app.exit_approved);
    }

    #[test]
    fn minimized_current_child_still_completes_close_after_its_native_host_was_removed() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, workspaces) = fixture(root.path());
        let ctx = egui::Context::default();
        let child = detach(&mut app, &ctx, workspaces[2], true);
        app.enter_window(child);
        app.close_window(&ctx, child);
        app.enter_window(WindowId::ROOT);
        app.config.warn_running_processes = false;
        app.config.confirm_close = true;
        app.request_close(&ctx, Close::Window(WindowId::ROOT));
        // Simulate a pending check becoming auto-approved while minimized.
        app.config.confirm_close = false;
        let mut input = egui::RawInput::default();
        input.viewports.insert(
            app.viewport_of(WindowId::ROOT),
            egui::ViewportInfo {
                focused: Some(false),
                minimized: Some(true),
                ..Default::default()
            },
        );
        ctx.run_ui(input, |ui| app.poll_hidden_windows(ui.ctx()))
            .textures_delta
            .clear();
        assert!(app.exit_approved);
    }
}
impl Default for Windows {
    fn default() -> Self {
        Self {
            states: BTreeMap::new(),
            launches: BTreeMap::new(),
            root_hidden: false,
            current: WindowId::ROOT,
            host: WindowId::ROOT,
            viewports: BTreeMap::new(),
            keyboard: BTreeMap::new(),
            selections: BTreeMap::new(),
            focus_targets: BTreeMap::new(),
            pointer: Default::default(),
            drag: None,
            drop_zones: BTreeMap::new(),
            released: Default::default(),
            raise_after_mapping: None,
            lower_after_raise: None,
        }
    }
}

struct LaunchWindow {
    position: Option<Pos2>,
    size: Vec2,
    maximized: bool,
    fullscreen: bool,
    defer_creation: bool,
    first_frame: bool,
    fallback: Option<WindowId>,
}

// Keep frame-local fields together here, rather than sharing overlays, input
// composition, search tasks or panel workers between native windows.
macro_rules! presentation {
    ($($field:ident: $ty:ty),* $(,)?) => {
        #[derive(Default)]
        struct Presentation { $($field: $ty,)* }
        impl Presentation {
            fn swap(&mut self, app: &mut App) {
                $(std::mem::swap(&mut self.$field, &mut app.$field);)*
            }
        }
    };
}
presentation! {
    ui: UiState,
    search_point: Option<Point>,
    search_query: Option<(String, Arc<SearchQuery>)>,
    search_task: Option<ActiveSearch>,
    pending_close: Option<closing::PendingClose>,
    directory_check: Option<directory::PendingDirectory>,
    ime_composing: bool,
    terminal_focus: Option<egui::Id>,
    window_focused: Option<bool>,
    overlay_was_open: bool,
    hints: Option<(PaneId, u64)>,
    hint_keys: Vec<egui::Key>,
    file_location: Option<hints::PendingLocation>,
    image_preview: image_preview::ImagePreview,
    explorer: explorer::Explorer,
    changes: changes::Changes,
    pull_request: pull_request::PullRequestTab,
    browser_tools: browser::BrowserTools,
    paste_chord: crate::input::PasteChord,
    swallowed_paste: Option<egui::Modifiers>,
    shortcut_keys: std::collections::HashSet<egui::Key>,
    shortcut_modifiers: egui::Modifiers,
}

pub(super) fn viewport(window: WindowId) -> egui::ViewportId {
    if window == WindowId::ROOT {
        egui::ViewportId::ROOT
    } else {
        egui::ViewportId::from_hash_of(("neptune-window", window.get()))
    }
}

impl App {
    pub(super) fn attach_workspace_pointer(&mut self, frame: &eframe::Frame) {
        self.windows.pointer.attach(frame);
    }

    pub(super) fn finish_workspace_raise(&mut self, ctx: &egui::Context) {
        if let Some(viewport) = self.windows.lower_after_raise.take()
            && !self.windows.drag.as_ref().is_some_and(|drag| {
                drag.viewport == viewport && matches!(drag.mode, WorkspaceDragMode::Floating { .. })
            })
        {
            ctx.send_viewport_cmd_to(
                viewport,
                egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal),
            );
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
        }
    }

    pub(super) fn workspace_drag_active(&self) -> bool {
        self.windows.drag.is_some()
    }

    pub(super) fn take_workspace_drag_release(&mut self, ctx: &egui::Context) -> bool {
        let released = self.windows.released.remove(&ctx.viewport_id());
        if released {
            // Clear the toolkit before terminal motion can use button state
            // left over from the carried workspace gesture.
            ctx.input_mut(|input| input.pointer = Default::default());
            ctx.stop_dragging();
        }
        released
    }

    pub(super) fn cancel_workspace_drag(&mut self, ctx: &egui::Context) {
        if let Some(drag) = self.windows.drag.take() {
            self.end_workspace_drag(ctx, &drag, false);
        }
    }

    fn end_workspace_drag(&mut self, ctx: &egui::Context, drag: &WorkspaceDrag, released: bool) {
        ctx.send_viewport_cmd_to(
            drag.viewport,
            egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal),
        );
        self.windows.released.insert(drag.viewport);
        if let Some(ui) = self.window_ui_state_mut(drag.window) {
            if released && matches!(drag.mode, WorkspaceDragMode::Docked) {
                ui.finish_carried_workspace();
            } else {
                ui.cancel_carried_workspace();
            }
        }
        ctx.request_repaint_of(drag.viewport);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Draw the same held row after its native surface adopts another sidebar.
    /// Widget identities change with the sidebar; the desktop watch owns the
    /// continuing press.
    pub(super) fn sync_carried_workspace(&mut self, ctx: &egui::Context) {
        let Some(drag) = &self.windows.drag else {
            return;
        };
        if drag.window != self.presentation_window()
            || !matches!(drag.mode, WorkspaceDragMode::Docked)
        {
            return;
        }
        self.carry_workspace_row(ctx, drag.window, drag.workspace, drag.pointer, drag.grip);
    }

    fn carry_workspace_row(
        &mut self,
        ctx: &egui::Context,
        window: WindowId,
        workspace: WorkspaceId,
        pointer: Pos2,
        grip: Vec2,
    ) {
        let Some(pointer) = self.workspace_pointer_local(ctx, window, pointer) else {
            return;
        };
        let grouped = self
            .controller
            .model()
            .workspace(workspace)
            .is_some_and(|workspace| workspace.group().is_some());
        if let Some(ui) = self.window_ui_state_mut(window) {
            ui.carry_workspace(workspace, grouped, pointer, grip);
        }
    }

    fn workspace_pointer_local(
        &self,
        ctx: &egui::Context,
        window: WindowId,
        pointer: Pos2,
    ) -> Option<Pos2> {
        let viewport = self.viewport_of(window);
        // Context accessors must run before input's context lock is held.
        let zoom = ctx.zoom_factor();
        ctx.input(|input| {
            let info = input.raw.viewports.get(&viewport)?;
            let scale = info.native_pixels_per_point? * zoom;
            Some(pointer / scale - info.inner_rect?.min.to_vec2())
        })
    }

    /// Anchor the floating window to the row actually drawn in it, rather than
    /// a guessed titlebar offset or its former position in a longer sidebar.
    pub(super) fn position_carried_workspace(&mut self, ctx: &egui::Context) {
        let window = self.presentation_window();
        let Some(drag) = &mut self.windows.drag else {
            return;
        };
        if drag.window != window {
            return;
        }
        let WorkspaceDragMode::Floating { anchor } = &mut drag.mode else {
            return;
        };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some((_, row)) = self
            .ui
            .workspace_rows
            .iter()
            .find(|(id, _)| *id == drag.workspace)
        {
            *anchor = Some(row.min.to_vec2() + drag.grip);
        } else if let Some(zones) = self.windows.drop_zones.get(&window) {
            // A narrow window or hidden sidebar has no workspace row to
            // anchor. Keep its toolbar under the grip so the held window can
            // still move and rejoin; a visible row takes over when available.
            *anchor = Some(zones.toolbar.clamp(zones.toolbar.min + drag.grip).to_vec2());
        }
        Self::position_workspace_window(ctx, drag);
    }

    fn position_workspace_window(ctx: &egui::Context, drag: &WorkspaceDrag) {
        let WorkspaceDragMode::Floating {
            anchor: Some(anchor),
        } = drag.mode
        else {
            return;
        };
        let zoom = ctx.zoom_factor();
        let position = ctx.input(|input| {
            let info = input.raw.viewports.get(&drag.viewport)?;
            let scale = info.native_pixels_per_point? * zoom;
            let frame = info.outer_rect?.min - info.inner_rect?.min;
            Some(drag.pointer / scale - anchor + frame)
        });
        if let Some(position) = position {
            ctx.send_viewport_cmd_to(
                drag.viewport,
                egui::ViewportCommand::OuterPosition(position),
            );
        }
    }

    pub(super) fn workspace_drop_zones(
        &mut self,
        ctx: &egui::Context,
        sidebar: Option<Rect>,
        toolbar: Rect,
    ) {
        let window = self.presentation_window();
        if self.ui.overlay != OverlayState::None || egui::Popup::is_any_open(ctx) {
            self.windows.drop_zones.remove(&window);
            return;
        }
        let focused = ctx.input(|input| input.focused.then_some(input.time));
        let last_focused = focused.unwrap_or_else(|| {
            self.windows
                .drop_zones
                .get(&window)
                .map_or(0.0, |zones| zones.last_focused)
        });
        self.windows.drop_zones.insert(
            window,
            DropZones {
                sidebar,
                toolbar,
                last_focused,
            },
        );
    }

    fn workspace_drop_available(&self, window: WindowId) -> bool {
        if self.controller.model().window(window).is_none() {
            return false;
        }
        if window == self.presentation_window() {
            self.pending_close.is_none() && self.ui.overlay == OverlayState::None
        } else {
            self.windows.states.get(&window).is_none_or(|state| {
                state.pending_close.is_none() && state.ui.overlay == OverlayState::None
            })
        }
    }

    fn workspace_drop_destination(
        &self,
        ctx: &egui::Context,
        source: WindowId,
        pointer: Pos2,
    ) -> Option<WindowId> {
        if !pointer.x.is_finite() || !pointer.y.is_finite() {
            return None;
        }
        let zoom = ctx.zoom_factor();
        ctx.input(|input| {
            self.windows
                .drop_zones
                .iter()
                .filter_map(|(window, zones)| {
                    if *window == source || !self.workspace_drop_available(*window) {
                        return None;
                    }
                    let info = input.raw.viewports.get(&self.viewport_of(*window))?;
                    if info.minimized == Some(true) || info.visible() == Some(false) {
                        return None;
                    }
                    // Each viewport reports screen positions in its own egui
                    // points. Convert both its origin and zones to desktop pixels.
                    let scale = info.native_pixels_per_point? * zoom;
                    let origin = info.inner_rect?.min * scale;
                    let contains =
                        |rect: Rect| (rect * scale).translate(origin.to_vec2()).contains(pointer);
                    contains(zones.sidebar.unwrap_or(zones.toolbar))
                        .then_some((*window, zones.last_focused))
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(window, _)| window)
        })
    }

    fn dock_workspace_drag(
        &mut self,
        ctx: &egui::Context,
        drag: &mut WorkspaceDrag,
        target: WindowId,
    ) {
        let source = drag.window;
        let target_viewport = self.viewport_of(target);
        let (carried_info, target_info) = ctx.input(|input| {
            (
                input.raw.viewports.get(&drag.viewport).cloned(),
                input.raw.viewports.get(&target_viewport).cloned(),
            )
        });
        let Some((carried_info, target_info)) = carried_info.zip(target_info) else {
            return;
        };
        let zoom = ctx.zoom_factor();
        let Some((carried_scale, target_scale)) = carried_info
            .native_pixels_per_point
            .zip(target_info.native_pixels_per_point)
            .map(|(carried, target)| (carried * zoom, target * zoom))
        else {
            return;
        };
        let placement = |info: &egui::ViewportInfo, scale: f32, new_scale: f32| {
            info.inner_rect.map(|inner| LaunchWindow {
                position: Some(info.outer_rect.unwrap_or(inner).min * scale / new_scale),
                size: inner.size() * scale / new_scale,
                maximized: info.maximized == Some(true),
                fullscreen: info.fullscreen == Some(true),
                defer_creation: false,
                first_frame: false,
                fallback: None,
            })
        };
        let Some((target_launch, source_launch)) = placement(
            &target_info,
            target_scale,
            carried_scale,
        )
        .zip(placement(&carried_info, carried_scale, target_scale)) else {
            return;
        };

        self.enter_window(source);
        self.ui.cancel_sidebar_drag(ctx);
        // Use the shared durable transfer path so drafts, panes and session
        // generations retain their identities, just as for menu transfers.
        self.dispatch(
            ctx,
            Command::MoveWorkspaceToWindow {
                workspace: drag.workspace,
                window: target,
            },
        );
        if self
            .controller
            .model()
            .workspace(drag.workspace)
            .is_none_or(|workspace| workspace.window() != target)
        {
            return;
        }

        // The press belongs to a native surface, not a model WindowId. Do not
        // close or hide that surface on joining: X11 ungrabs an unmapped press
        // window, and XWayland can then report a release while the user holds
        // the button. Give the receiving sidebar the pressed surface instead.
        self.windows.viewports.insert(target, drag.viewport);
        self.windows.viewports.insert(source, target_viewport);
        if drag.viewport == egui::ViewportId::ROOT {
            self.windows.host = target;
        } else if target_viewport == egui::ViewportId::ROOT {
            self.windows.host = source;
        }
        let source_occupied = self
            .controller
            .model()
            .window(source)
            .is_some_and(|window| window.active_workspace().is_some());
        for (viewport, launch) in [
            (drag.viewport, &target_launch),
            (target_viewport, &source_launch),
        ] {
            if viewport == target_viewport && !source_occupied {
                continue;
            }
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Fullscreen(false));
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Maximized(false));
            if let Some(position) = launch.position {
                ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::OuterPosition(position));
            }
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::InnerSize(launch.size));
            if launch.maximized {
                ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Maximized(true));
            }
            if launch.fullscreen {
                ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Fullscreen(true));
            }
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Visible(true));
        }
        self.windows.launches.insert(target, target_launch);
        self.windows.launches.insert(source, source_launch);
        ctx.send_viewport_cmd_to(
            drag.viewport,
            egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal),
        );
        ctx.send_viewport_cmd_to(drag.viewport, egui::ViewportCommand::Focus);
        drag.window = target;
        drag.mode = WorkspaceDragMode::Docked;
        self.enter_window(target);
    }

    pub(super) fn poll_workspace_drag(&mut self, ctx: &egui::Context) {
        let Some(mut drag) = self.windows.drag.take() else {
            return;
        };
        if self.exit_approved
            || self
                .controller
                .model()
                .workspace(drag.workspace)
                .is_none_or(|workspace| workspace.window() != drag.window)
            || !self.workspace_drop_available(drag.window)
            || ctx.input(|input| {
                input.raw.viewports.get(&drag.viewport).is_some_and(|info| {
                    // Joining changes stacking before native visibility events
                    // catch up. Occlusion is a rendering hint, not the end of
                    // the held press; the desktop watch remains authoritative.
                    info.minimized == Some(true)
                })
            })
        {
            self.end_workspace_drag(ctx, &drag, false);
            return;
        }
        let sample = match drag.watch.poll() {
            Ok(Some(pointer)) => pointer,
            Ok(None) => {
                self.windows.drag = Some(drag);
                ctx.request_repaint_after_for(Duration::from_millis(16), egui::ViewportId::ROOT);
                return;
            }
            Err(()) => {
                self.end_workspace_drag(ctx, &drag, false);
                return;
            }
        };
        drag.pointer = sample.position;
        let previous = self.presentation_window();
        match drag.mode {
            WorkspaceDragMode::Floating { .. } => {
                Self::position_workspace_window(ctx, &drag);
                if let Some(target) =
                    self.workspace_drop_destination(ctx, drag.window, drag.pointer)
                {
                    self.dock_workspace_drag(ctx, &mut drag, target);
                }
            }
            WorkspaceDragMode::Docked => {
                let inside = self
                    .workspace_pointer_local(ctx, drag.window, drag.pointer)
                    .zip(self.windows.drop_zones.get(&drag.window))
                    .is_some_and(|(pointer, zones)| {
                        zones.sidebar.unwrap_or(zones.toolbar).contains(pointer)
                    });
                if sample.primary_down
                    && !inside
                    && let Some(pointer) =
                        self.workspace_pointer_local(ctx, drag.window, drag.pointer)
                {
                    // This is a transition within the same held press. Keep
                    // its watch and grip; release/cancel cleanup would clear
                    // toolkit input and lower the surface being carried again.
                    self.enter_window(drag.window);
                    self.detach_workspace_with_watch(
                        ctx,
                        drag.workspace,
                        Some(pointer),
                        Some(drag.grip),
                        Some(drag.watch),
                    );
                    self.restore_drag_context(previous);
                    return;
                }
            }
        }
        if matches!(drag.mode, WorkspaceDragMode::Docked) {
            // Include the final native sample before the sidebar computes its
            // release slot; the toolkit may never see this button release.
            self.carry_workspace_row(ctx, drag.window, drag.workspace, drag.pointer, drag.grip);
        }
        if sample.primary_down {
            ctx.request_repaint_of(drag.viewport);
            self.windows.drag = Some(drag);
            ctx.request_repaint_after_for(Duration::from_millis(16), egui::ViewportId::ROOT);
        } else {
            self.end_workspace_drag(ctx, &drag, true);
            if matches!(drag.mode, WorkspaceDragMode::Floating { .. }) {
                // Fast release can precede replacement creation. Raise after
                // new surfaces map, then return to the normal level.
                self.windows.raise_after_mapping = Some(drag.viewport);
            }
        }
        self.restore_drag_context(previous);
    }

    fn restore_drag_context(&mut self, previous: WindowId) {
        self.enter_window(if self.controller.model().window(previous).is_some() {
            previous
        } else {
            self.host_window()
        });
    }

    pub(super) fn take_workspace_project_edits(
        &mut self,
        workspace: WorkspaceId,
    ) -> Option<ProjectEdits> {
        let model = self.controller.model();
        let project = model.project_of(workspace)?.id();
        let window = model.workspace(workspace)?.window();
        let state = &mut self.window_ui_state_mut(window)?.project;
        Some(ProjectEdits {
            project,
            draft: state.drafts.remove(&project),
            attached: state.attached.remove(&project),
            instructions: state.instructions.remove(&project),
        })
    }

    pub(super) fn restore_workspace_project_edits(&mut self, edits: Option<ProjectEdits>) {
        let Some(edits) = edits else { return };
        let model = self.controller.model();
        let Some(window) = model
            .project(edits.project)
            .and_then(|project| model.workspace(project.workspace()))
            .map(|workspace| workspace.window())
        else {
            return;
        };
        let Some(ui) = self.window_ui_state_mut(window) else {
            return;
        };
        if let Some(draft) = edits.draft {
            ui.project.drafts.insert(edits.project, draft);
        }
        if let Some(attached) = edits.attached {
            ui.project.attached.insert(edits.project, attached);
        }
        if let Some(instructions) = edits.instructions {
            ui.project.instructions.insert(edits.project, instructions);
        }
    }

    pub(super) fn host_window(&self) -> WindowId {
        if self.controller.model().window(self.windows.host).is_some() {
            self.windows.host
        } else {
            self.controller
                .model()
                .windows()
                .iter()
                .find(|window| window.active_workspace().is_some())
                .map_or(WindowId::ROOT, |window| window.id())
        }
    }
    pub(super) fn host_exists(&self) -> bool {
        self.controller.model().window(self.windows.host).is_some()
    }
    pub(super) fn adopt_restored_host(&mut self) {
        let model = self.controller.model();
        if model
            .window(WindowId::ROOT)
            .is_some_and(|root| root.active_workspace().is_none())
            && let Some(window) = model
                .windows()
                .iter()
                .find(|window| window.active_workspace().is_some())
                .map(|window| window.id())
        {
            let error = self.ui.error.take();
            self.windows.host = window;
            self.windows
                .viewports
                .insert(window, egui::ViewportId::ROOT);
            self.windows
                .viewports
                .insert(WindowId::ROOT, viewport(window));
            self.enter_window(window);
            self.ui.error = error;
        }
    }

    pub(super) fn viewport_of(&self, window: WindowId) -> egui::ViewportId {
        self.windows
            .viewports
            .get(&window)
            .copied()
            .unwrap_or_else(|| viewport(window))
    }
    pub(super) fn presentation_window(&self) -> WindowId {
        self.windows.current
    }

    pub(super) fn window_ui_state(&self, window: WindowId) -> Option<&UiState> {
        self.controller.model().window(window)?;
        if window == self.windows.current {
            Some(&self.ui)
        } else {
            self.windows.states.get(&window).map(|state| &state.ui)
        }
    }

    pub(super) fn window_ui_state_mut(&mut self, window: WindowId) -> Option<&mut UiState> {
        self.controller.model().window(window)?;
        if window == self.windows.current {
            Some(&mut self.ui)
        } else {
            Some(&mut self.windows.states.entry(window).or_default().ui)
        }
    }

    /// Closing the final pane still leaves the usual usable empty window.
    /// A held tear-off may have transferred the native root to a model window
    /// that is pruned when its last workspace disappears.
    pub(super) fn reconcile_empty_host(&mut self, ctx: &egui::Context) {
        if !self.host_exists() && self.controller.model().workspaces().is_empty() {
            self.windows.host = WindowId::ROOT;
            self.windows
                .viewports
                .insert(WindowId::ROOT, egui::ViewportId::ROOT);
            self.enter_window(WindowId::ROOT);
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    pub(super) fn move_visible_workspace(
        &mut self,
        ctx: &egui::Context,
        workspace: WorkspaceId,
        index: usize,
    ) {
        let model = self.controller.model();
        let Some(owner) = model.workspace(workspace).map(|w| w.window()) else {
            self.dispatch(ctx, Command::MoveWorkspace { workspace, index });
            return;
        };
        let visible: Vec<_> = model
            .workspaces()
            .iter()
            .filter(|w| w.window() == owner)
            .map(|w| w.id())
            .collect();
        let Some(target) = visible.get(index.min(visible.len().saturating_sub(1))) else {
            return;
        };
        let Some(index) = model.workspaces().iter().position(|w| w.id() == *target) else {
            return;
        };
        self.dispatch(ctx, Command::MoveWorkspace { workspace, index });
    }

    pub(super) fn move_visible_sidebar_item(
        &mut self,
        ctx: &egui::Context,
        item: neptune_model::SidebarItem,
        index: usize,
    ) {
        let model = self.controller.model();
        let owner = match item {
            neptune_model::SidebarItem::Workspace(id) => model.workspace(id).map(|w| w.window()),
            neptune_model::SidebarItem::Group(_) => Some(model.current_window()),
        };
        let Some(owner) = owner else {
            self.dispatch(ctx, Command::MoveSidebarItem { item, index });
            return;
        };
        let visible: Vec<_> = model
            .sidebar_order()
            .iter()
            .copied()
            .filter(|item| match item {
                neptune_model::SidebarItem::Workspace(id) => {
                    model.workspace(*id).is_some_and(|w| w.window() == owner)
                }
                neptune_model::SidebarItem::Group(_) => true,
            })
            .collect();
        let Some(target) = visible.get(index.min(visible.len().saturating_sub(1))) else {
            return;
        };
        let Some(index) = model.sidebar_order().iter().position(|item| item == target) else {
            return;
        };
        self.dispatch(ctx, Command::MoveSidebarItem { item, index });
    }

    pub(super) fn reconcile_window_selection(&mut self) {
        let selected = self.controller.model().active_pane();
        if self
            .windows
            .selections
            .insert(self.windows.current, selected)
            .is_some_and(|old| old != selected)
        {
            self.search_task = None;
            self.search_point = None;
            // hint_input must consume the pending frame and key releases before
            // cancelling hints whose captured pane is no longer selected.
            self.ui.search_error = None;
        }
    }

    pub(super) fn record_window_keyboard(&mut self, ctx: &egui::Context) {
        self.windows
            .keyboard
            .insert(self.windows.current, self.terminal_owns_shortcuts(ctx));
    }

    pub(super) fn reveal_workspace_window(&mut self, ctx: &egui::Context, workspace: WorkspaceId) {
        let Some(owner) = self
            .controller
            .model()
            .workspace(workspace)
            .map(|w| w.window())
        else {
            return;
        };
        if owner != self.windows.current {
            if let Some(state) = self.windows.states.get_mut(&owner) {
                state.ui.overlay = OverlayState::None;
                state.ui.zoomed = false;
                state.ui.workspace_drag = Default::default();
                state.ui.item_drag = Default::default();
            }
            let target = self.viewport_of(owner);
            ctx.send_viewport_cmd_to(target, egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(target, egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd_to(target, egui::ViewportCommand::Focus);
        }
    }

    pub(super) fn enter_window(&mut self, window: WindowId) {
        let previous = self.windows.current;
        if self.controller.model().window(window).is_none() {
            return;
        }
        if self
            .controller
            .dispatch(Command::SetWindowContext(window))
            .is_err()
        {
            return;
        }
        if previous == window {
            return;
        }
        let mut state = self.windows.states.remove(&window).unwrap_or_default();
        state.swap(self);
        self.windows.states.insert(previous, state);
        self.windows.current = window;
    }

    /// Panes visible in any native window, so a sibling's browser is not
    /// hidden and its render cache is not evicted by the current window.
    pub(super) fn visible_panes(&self, ctx: &egui::Context) -> Vec<PaneId> {
        let model = self.controller.model();
        model
            .windows()
            .iter()
            .flat_map(|window| {
                let hidden = ctx.input(|input| {
                    input
                        .raw
                        .viewports
                        .get(&self.viewport_of(window.id()))
                        .is_some_and(|info| info.visible() == Some(false))
                });
                if hidden {
                    return Vec::new();
                }
                let Some(workspace) = window.active_workspace().and_then(|id| model.workspace(id))
                else {
                    return Vec::new();
                };
                let zoomed = if window.id() == self.windows.current {
                    self.ui.zoomed
                } else {
                    self.windows
                        .states
                        .get(&window.id())
                        .is_some_and(|state| state.ui.zoomed)
                };
                if zoomed {
                    vec![workspace.active()]
                } else {
                    workspace.layout().shown()
                }
            })
            .collect()
    }

    pub(super) fn focused_pane(&self, ctx: &egui::Context) -> Option<PaneId> {
        let model = self.controller.model();
        model.windows().iter().find_map(|window| {
            let focused = self.native_window_focused(ctx, window.id());
            if !focused {
                return None;
            }
            let overlay = if window.id() == self.windows.current {
                self.ui.overlay
            } else {
                self.windows
                    .states
                    .get(&window.id())
                    .map_or(OverlayState::None, |state| state.ui.overlay)
            };
            let keyboard = if window.id() == self.windows.current
                && self.viewport_of(window.id()) == ctx.viewport_id()
            {
                self.terminal_owns_shortcuts(ctx)
            } else {
                self.windows
                    .keyboard
                    .get(&window.id())
                    .copied()
                    .unwrap_or(true)
            };
            if overlay != OverlayState::None || !keyboard {
                return None;
            }
            window
                .active_workspace()
                .and_then(|id| model.workspace(id))
                .map(|workspace| workspace.active())
        })
    }

    fn native_window_focused(&self, ctx: &egui::Context, window: WindowId) -> bool {
        let viewport = self.viewport_of(window);
        let current = ctx.viewport_id();
        ctx.input(|input| {
            input.raw.viewports.get(&viewport).is_some_and(|info| {
                info.focused.unwrap_or(input.focused && viewport == current)
                    && info.minimized != Some(true)
                    && info.visible() != Some(false)
            })
        })
    }

    pub(super) fn pane_native_focused(&self, ctx: &egui::Context, pane: PaneId) -> bool {
        let model = self.controller.model();
        model
            .workspace_for_pane(pane)
            .and_then(|id| model.workspace(id))
            .is_some_and(|workspace| {
                workspace.active() == pane
                    && model
                        .window(workspace.window())
                        .is_some_and(|window| window.active_workspace() == Some(workspace.id()))
                    && self.native_window_focused(ctx, workspace.window())
            })
    }

    /// Root logic runs even when every native window is minimized. Reconcile
    /// all selected sessions there instead of relying on child UI callbacks.
    pub(super) fn reconcile_native_focus(&mut self, ctx: &egui::Context) {
        let windows: Vec<_> = self
            .controller
            .model()
            .windows()
            .iter()
            .map(|window| {
                let pane = window
                    .active_workspace()
                    .and_then(|id| self.controller.model().workspace(id))
                    .map(|workspace| workspace.active());
                (
                    window.id(),
                    pane,
                    self.native_window_focused(ctx, window.id()),
                )
            })
            .collect();
        for (window, pane, focused) in windows {
            let previous_pane = self.windows.focus_targets.get(&window).copied().flatten();
            let old = if window == self.windows.current {
                self.window_focused
            } else {
                self.windows
                    .states
                    .get(&window)
                    .and_then(|state| state.window_focused)
            };
            if old == Some(focused) && previous_pane == pane {
                continue;
            }
            if previous_pane != pane
                && let Some(previous_pane) = previous_pane
                && let Some(session) = self.sessions.get(previous_pane)
            {
                let _ = session.focus(self.pane_native_focused(ctx, previous_pane));
            }
            if pane
                .and_then(|pane| self.sessions.get(pane))
                .is_none_or(|session| session.focus(focused).is_ok())
            {
                if window == self.windows.current {
                    self.window_focused = Some(focused);
                } else {
                    self.windows
                        .states
                        .entry(window)
                        .or_default()
                        .window_focused = Some(focused);
                }
                self.windows.focus_targets.insert(window, pane);
            }
        }
    }

    pub(super) fn detach_workspace(
        &mut self,
        ctx: &egui::Context,
        workspace: WorkspaceId,
        pointer: Option<Pos2>,
        grip: Option<Vec2>,
    ) {
        self.detach_workspace_with_watch(ctx, workspace, pointer, grip, None);
    }

    fn detach_workspace_with_watch(
        &mut self,
        ctx: &egui::Context,
        workspace: WorkspaceId,
        pointer: Option<Pos2>,
        grip: Option<Vec2>,
        watch: Option<crate::platform::workspace_drag::Watch>,
    ) {
        if self.startup.is_some() || ctx.embed_viewports() {
            return;
        }
        let Some(source) = self
            .controller
            .model()
            .workspace(workspace)
            .map(|w| w.window())
        else {
            return;
        };
        if source != self.controller.model().current_window() {
            return;
        }
        let source_viewport = self.viewport_of(source);
        // A continued gesture may be polled in the hidden root host while its
        // row is in a child. Read the source surface's geometry explicitly.
        let info = ctx.input(|input| input.raw.viewports.get(&source_viewport).cloned());
        let origin = info
            .as_ref()
            .and_then(|info| info.inner_rect.map(|rect| rect.min));
        let scale = info
            .as_ref()
            .and_then(|info| info.native_pixels_per_point)
            .map_or(ctx.pixels_per_point(), |scale| scale * ctx.zoom_factor());
        let desktop_pointer = origin
            .zip(pointer)
            .map(|(origin, pointer)| (origin + pointer.to_vec2()) * scale);
        let position = origin.map(|origin| {
            origin + pointer.unwrap_or(Pos2::new(48.0, 48.0)).to_vec2() - Vec2::new(80.0, 20.0)
        });
        let maximized = info
            .as_ref()
            .is_some_and(|info| info.maximized == Some(true));
        let fullscreen = info
            .as_ref()
            .is_some_and(|info| info.fullscreen == Some(true));
        let size = info
            .as_ref()
            .and_then(|info| info.inner_rect)
            .map_or(ctx.content_rect().size(), |rect| rect.size());
        let drag = watch.is_some()
            || (pointer.is_some() && ctx.input(|input| input.pointer.primary_down()));
        if !drag
            && let Some(target) = desktop_pointer
                .and_then(|pointer| self.workspace_drop_destination(ctx, source, pointer))
        {
            self.move_workspace_to_window(ctx, workspace, target);
            return;
        }
        self.cancel_workspace_drag(ctx);
        self.ui.cancel_sidebar_drag(ctx);
        let edits = self.take_workspace_project_edits(workspace);
        let effects = match self
            .controller
            .dispatch(Command::DetachWorkspace(workspace))
        {
            Ok(effects) => effects,
            Err(error) => {
                self.restore_workspace_project_edits(edits);
                self.ui.error = Some(error.to_string());
                return;
            }
        };
        self.restore_workspace_project_edits(edits);
        let Some(window) = self
            .controller
            .model()
            .workspace(workspace)
            .map(|w| w.window())
            .filter(|id| *id != source)
        else {
            return;
        };
        if drag {
            // A Wayland move needs the original seat press serial. Keep the
            // pressed native window with the carried workspace and create a
            // replacement for the workspaces it leaves behind.
            let replacement = viewport(window);
            self.windows.viewports.insert(window, source_viewport);
            self.windows.viewports.insert(source, replacement);
            if source_viewport == egui::ViewportId::ROOT {
                self.windows.host = window;
            }
            self.windows.launches.insert(
                window,
                LaunchWindow {
                    position: origin,
                    size,
                    maximized: false,
                    fullscreen: false,
                    defer_creation: false,
                    first_frame: false,
                    fallback: None,
                },
            );
            self.windows.launches.insert(
                source,
                LaunchWindow {
                    position: origin,
                    size,
                    maximized,
                    fullscreen,
                    defer_creation: self
                        .controller
                        .model()
                        .window(source)
                        .is_some_and(|window| window.active_workspace().is_some()),
                    first_frame: false,
                    fallback: Some(window),
                },
            );
            if fullscreen {
                ctx.send_viewport_cmd_to(source_viewport, egui::ViewportCommand::Fullscreen(false));
            }
            if maximized {
                ctx.send_viewport_cmd_to(source_viewport, egui::ViewportCommand::Maximized(false));
            }
            if let Some(watch) = watch.or_else(|| self.windows.pointer.watch(ctx))
                && let Some(desktop_pointer) = desktop_pointer
            {
                self.windows.drag = Some(WorkspaceDrag {
                    workspace,
                    window,
                    viewport: source_viewport,
                    watch,
                    grip: grip.unwrap_or(Vec2::new(80.0, 20.0)),
                    pointer: desktop_pointer,
                    mode: WorkspaceDragMode::Floating { anchor: None },
                });
                // Application-controlled positioning can join another sidebar
                // during this same press; a compositor move would own it until
                // release. The row's actual geometry supplies the first anchor.
                ctx.send_viewport_cmd_to(
                    source_viewport,
                    egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop),
                );
            } else {
                // Backends without desktop pointer observation retain their
                // native move and the stacking fix.
                self.windows.raise_after_mapping = Some(source_viewport);
                ctx.send_viewport_cmd_to(source_viewport, egui::ViewportCommand::StartDrag);
            }
            ctx.send_viewport_cmd_to(source_viewport, egui::ViewportCommand::Focus);
        } else {
            self.windows.launches.insert(
                window,
                LaunchWindow {
                    position,
                    size,
                    maximized: false,
                    fullscreen: false,
                    defer_creation: false,
                    first_frame: true,
                    fallback: Some(source),
                },
            );
        }
        self.execute(ctx, effects);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    pub(super) fn move_workspace_to_window(
        &mut self,
        ctx: &egui::Context,
        workspace: WorkspaceId,
        window: WindowId,
    ) {
        self.cancel_workspace_drag(ctx);
        self.ui.cancel_sidebar_drag(ctx);
        self.dispatch(ctx, Command::MoveWorkspaceToWindow { workspace, window });
        if self
            .controller
            .model()
            .workspace(workspace)
            .is_some_and(|w| w.window() == window)
        {
            let target = self.viewport_of(window);
            ctx.send_viewport_cmd_to(target, egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(target, egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd_to(target, egui::ViewportCommand::Focus);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    pub(super) fn close_window(&mut self, ctx: &egui::Context, window: WindowId) {
        if self
            .windows
            .drag
            .as_ref()
            .is_some_and(|drag| drag.window == window)
        {
            self.cancel_workspace_drag(ctx);
        }
        // The final window exits using the existing save/restore contract.
        // Closing a sibling removes only that sibling's workspaces/sessions.
        let other = self
            .controller
            .model()
            .workspaces()
            .iter()
            .any(|w| w.window() != window);
        if !other {
            self.exit_approved = true;
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            return;
        }
        self.dispatch(ctx, Command::CloseWindow(window));
        if self.viewport_of(window) == egui::ViewportId::ROOT {
            self.windows.root_hidden = true;
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Visible(false),
            );
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    pub(super) fn show_windows(&mut self, ctx: &egui::Context) {
        self.reconcile_empty_host(ctx);
        let children: Vec<_> = self
            .controller
            .model()
            .windows()
            .iter()
            .filter(|window| {
                self.viewport_of(window.id()) != egui::ViewportId::ROOT
                    && window.active_workspace().is_some()
            })
            .map(|window| window.id())
            .collect();
        let root_empty = self
            .controller
            .model()
            .window(self.windows.host)
            .is_none_or(|window| window.active_workspace().is_none());
        let hide = root_empty && !children.is_empty() && self.startup.is_none();
        if hide != self.windows.root_hidden {
            self.windows.root_hidden = hide;
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Visible(!hide),
            );
        }
        let mut creation_deferred = false;
        for window in children {
            if self.controller.model().window(window).is_none() {
                continue;
            }
            let default_fallback = self.host_window();
            let launch = self.windows.launches.entry(window).or_insert(LaunchWindow {
                position: None,
                size: Vec2::from(self.window_state.inner_size) / ctx.zoom_factor(),
                maximized: false,
                fullscreen: false,
                defer_creation: false,
                first_frame: true,
                fallback: Some(default_fallback),
            });
            // Position or start the move on the carried surface before mapping
            // its replacement. Some compositors focus a newly mapped window even
            // when the builder asks it to remain inactive.
            if std::mem::take(&mut launch.defer_creation) {
                creation_deferred = true;
                ctx.request_repaint_of(egui::ViewportId::ROOT);
                continue;
            }
            let mut builder = egui::ViewportBuilder::default()
                .with_title("Neptune")
                .with_app_id("rs.neptune.terminal")
                .with_active(false)
                .with_window_level(
                    if self.windows.drag.as_ref().is_some_and(|drag| {
                        drag.window == window
                            && matches!(drag.mode, WorkspaceDragMode::Floating { .. })
                    }) {
                        egui::WindowLevel::AlwaysOnTop
                    } else {
                        egui::WindowLevel::Normal
                    },
                )
                .with_inner_size(launch.size)
                .with_maximized(launch.maximized)
                .with_fullscreen(launch.fullscreen)
                .with_min_inner_size(Vec2::new(640.0, 400.0) / ctx.zoom_factor())
                .with_decorations(false)
                .with_transparent(!cfg!(target_os = "windows"));
            if let Some(position) = launch.position {
                builder = builder.with_position(position);
            }
            let first = std::mem::replace(&mut launch.first_frame, false);
            let fallback = launch.fallback;
            let mut rendered = false;
            ctx.show_viewport_immediate(self.viewport_of(window), builder, |ui, class| {
                if class == egui::ViewportClass::EmbeddedWindow {
                    return;
                }
                rendered = true;
                self.enter_window(window);
                let child_ctx = ui.ctx().clone();
                // eframe calls raw_input_hook and logic only on the root.
                child_ctx.input_mut(|input| {
                    crate::input::drop_redundant_preedits(
                        &mut input.events,
                        &mut self.ime_composing,
                    );
                    self.swallowed_paste = self.paste_chord.swallowed(&input.events);
                });
                if first {
                    child_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                if child_ctx.input(|input| input.viewport().close_requested())
                    && !self.exit_approved
                {
                    child_ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.request_close(&child_ctx, Close::Window(window));
                }
                self.poll_directory(&child_ctx);
                self.poll(&child_ctx);
                self.window_ui(ui);
            });
            if !rendered {
                self.recover_window(ctx, window, fallback);
            }
            self.enter_window(self.host_window());
        }
        let model = self.controller.model();
        self.windows
            .states
            .retain(|id, _| model.window(*id).is_some());
        self.windows
            .launches
            .retain(|id, _| model.window(*id).is_some());
        self.windows
            .viewports
            .retain(|id, _| model.window(*id).is_some() || *id == self.windows.host);
        self.windows
            .keyboard
            .retain(|id, _| model.window(*id).is_some());
        self.windows
            .selections
            .retain(|id, _| model.window(*id).is_some());
        self.windows
            .focus_targets
            .retain(|id, _| model.window(*id).is_some());
        self.windows
            .drop_zones
            .retain(|id, _| model.window(*id).is_some());
        let live_viewports: std::collections::BTreeSet<_> = model
            .windows()
            .iter()
            .map(|window| self.viewport_of(window.id()))
            .chain([egui::ViewportId::ROOT])
            .collect();
        self.windows
            .released
            .retain(|viewport| live_viewports.contains(viewport));
        if !creation_deferred
            && let Some(viewport) = self.windows.raise_after_mapping.take()
            && live_viewports.contains(&viewport)
        {
            ctx.send_viewport_cmd_to(
                viewport,
                egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop),
            );
            self.windows.lower_after_raise = Some(viewport);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    fn recover_window(
        &mut self,
        ctx: &egui::Context,
        failed: WindowId,
        fallback: Option<WindowId>,
    ) {
        let destination = fallback
            .filter(|id| self.controller.model().window(*id).is_some())
            .unwrap_or_else(|| self.host_window());
        let workspaces: Vec<_> = self
            .controller
            .model()
            .workspaces()
            .iter()
            .filter(|workspace| workspace.window() == failed)
            .map(|workspace| workspace.id())
            .collect();
        for workspace in workspaces {
            self.dispatch(
                ctx,
                Command::MoveWorkspaceToWindow {
                    workspace,
                    window: destination,
                },
            );
        }
        if !self.host_exists() {
            self.windows.host = destination;
            self.windows
                .viewports
                .insert(destination, egui::ViewportId::ROOT);
        }
        self.enter_window(destination);
        self.ui.error = Some(
            "Could not open another native window. Its workspaces were kept in this window.".into(),
        );
        ctx.send_viewport_cmd_to(
            self.viewport_of(destination),
            egui::ViewportCommand::Visible(true),
        );
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// A taskbar close can arrive while all viewports are minimized. Keep
    /// pending close checks alive and reveal the captured window's sheet.
    pub(super) fn poll_hidden_windows(&mut self, ctx: &egui::Context) {
        let windows: Vec<_> = self
            .controller
            .model()
            .windows()
            .iter()
            .filter(|window| self.viewport_of(window.id()) != egui::ViewportId::ROOT)
            .map(|window| window.id())
            .collect();
        for window in windows {
            let child_viewport = self.viewport_of(window);
            let close = ctx.input(|input| {
                input
                    .raw
                    .viewports
                    .get(&child_viewport)
                    .is_some_and(|info| info.close_requested())
            });
            let restored = ctx.input(|input| {
                input
                    .raw
                    .viewports
                    .get(&child_viewport)
                    .is_some_and(|info| info.focused == Some(true) && info.minimized == Some(true))
            });
            if restored {
                ctx.send_viewport_cmd_to(child_viewport, egui::ViewportCommand::Minimized(false));
            }
            let pending = if window == self.windows.current {
                self.pending_close.is_some()
            } else {
                self.windows
                    .states
                    .get(&window)
                    .is_some_and(|state| state.pending_close.is_some())
            };
            let hidden = ctx.input(|input| {
                input
                    .raw
                    .viewports
                    .get(&child_viewport)
                    .is_some_and(|info| info.visible() == Some(false))
            });
            if close || (pending && hidden) {
                self.enter_window(window);
                if close {
                    ctx.send_viewport_cmd_to(child_viewport, egui::ViewportCommand::CancelClose);
                    self.request_close(ctx, Close::Window(window));
                    ctx.send_viewport_cmd_to(
                        child_viewport,
                        egui::ViewportCommand::Minimized(false),
                    );
                    ctx.send_viewport_cmd_to(child_viewport, egui::ViewportCommand::Focus);
                }
                self.poll_close(ctx);
                self.enter_window(self.host_window());
            }
        }
    }
}
