use super::*;
use neptune_model::WorkspaceId;

pub(super) fn fixture(root: &std::path::Path) -> (App, mpsc::SyncSender<Startup>) {
    let (sender, receiver) = mpsc::sync_channel(1);
    let config = Config {
        shell: Some("/nonexistent/neptune-startup-test-shell".into()),
        ..Config::default()
    };
    let app = App {
        controller: Controller::new(Model::default()),
        sessions: SessionManager::new(ResourcePolicy::default(), false),
        renders: BTreeMap::new(),
        config,
        config_path: root.join("config.toml"),
        config_writable: true,
        fonts: crate::platform::fonts::Fonts::bundled(),
        state_path: root.join("workspaces.json"),
        window_path: root.join("window.json"),
        window_state: window_state::WindowState::default(),
        window_writable: true,
        window_generation: 0,
        restore_maximized: false,
        writer: None,
        state_writable: false,
        startup: Some(receiver),
        deferred_actions: Vec::new(),
        initial_cwd: Some(root.into()),
        initial_remote: None,
        ssh_client: "/nonexistent/neptune-startup-test-ssh".into(),
        ui: UiState::default(),
        search_point: None,
        search_query: None,
        search_task: None,
        started: Instant::now() - Duration::from_secs(1),
        frame_started: Instant::now(),
        command: Some("printf startup".into()),
        command_target: None,
        screenshot: None,
        capture_sent: false,
        exit_approved: false,
        pending_close: None,
        directory_check: None,
        folder_picker: Default::default(),
        ephemeral: true,
        preference_generation: 0,
        ime_composing: false,
        terminal_focus: None,
        window_focused: None,
        overlay_was_open: false,
        _font_shortcut_monitor: Default::default(),
        diagnostics: diagnostics::Diagnostics::new(false),
        link_opener: Default::default(),
        editor_opener: Default::default(),
        hints: None,
        hint_keys: Vec::new(),
        file_location: None,
        file_opener: Default::default(),
        ports: Default::default(),
        notifications: Default::default(),
        agents: Default::default(),
        delegation: Default::default(),
        desktop_notifier: Default::default(),
        updates: Default::default(),
        relaunch: false,
        relaunch_arguments: Vec::new(),
        pull_requests: Default::default(),
        attachments: attachments::Attachments::new(root.join("pasted-images")),
        image_preview: Default::default(),
        attached: Default::default(),
        explorer: Default::default(),
        changes: Default::default(),
        file_drag: Default::default(),
        paste_chord: Default::default(),
        worktrees: Default::default(),
        swallowed_paste: None,
        shortcut_keys: Default::default(),
        shortcut_modifiers: egui::Modifiers::NONE,
    };
    (app, sender)
}

fn loaded(config: Config, model: Model) -> Startup {
    Startup {
        config,
        report: LoadReport {
            model: Some(model),
            diagnostics: Vec::new(),
            can_write: true,
            migrated: false,
        },
        error: None,
    }
}

#[test]
fn opening_config_during_a_capture_does_not_create_storage() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = fixture(root.path());
    app.startup = None;
    app.action(&egui::Context::default(), Action::OpenConfig);
    assert!(
        app.ui
            .error
            .as_deref()
            .unwrap()
            .contains("screenshot capture")
    );
    assert!(!app.config_path.exists());
}

#[test]
fn explicit_startup_commands_replace_agent_restoration_with_a_shell() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    let mut controller = Controller::new(Model::default());
    controller
        .dispatch(Command::AddWorkspace {
            cwd: root.path().into(),
            name: "agent".into(),
            remote: None,
            group: None,
        })
        .unwrap();
    let pane = controller.model().active_pane().unwrap();
    controller
        .dispatch(Command::PaneAgentChanged {
            pane,
            generation: 1,
            agent: Some(neptune_model::AgentSession {
                kind: neptune_model::AgentKind::Claude,
                session_id: Some("019a1234-5678-7000-8000-123456789abc".into()),
                cwd: root.path().into(),
            }),
        })
        .unwrap();
    app.complete_startup(&ctx, loaded(app.config.clone(), controller.model().clone()));
    assert!(app.controller.model().pane(pane).unwrap().agent().is_none());
    assert_eq!(app.command_target, Some((pane, 1)));
}

#[test]
fn window_corners_follow_native_window_state() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::platform::fonts::bundled_definitions());
    let mut frame = eframe::Frame::_new_kittest();
    let restored_radius = if cfg!(target_os = "windows") {
        0
    } else {
        metrics::WINDOW_RADIUS
    };
    let maximized_radius = if cfg!(target_os = "macos") {
        metrics::WINDOW_RADIUS
    } else {
        0
    };
    for size in [egui::vec2(900.0, 640.0), egui::vec2(640.0, 480.0)] {
        for (maximized, fullscreen, radius) in [
            (false, false, restored_radius),
            (true, false, maximized_radius),
            (false, true, 0),
            (true, true, 0),
            (false, false, restored_radius),
        ] {
            let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let mut input = egui::RawInput {
                screen_rect: Some(bounds),
                ..Default::default()
            };
            let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            viewport.maximized = Some(maximized);
            viewport.fullscreen = Some(fullscreen);
            let mut output = ctx.run_ui(input, |ui| {
                eframe::App::ui(&mut app, ui, &mut frame);
            });
            output.textures_delta.clear();
            let chrome = Palette::for_config(&app.config).chrome;
            let background = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.rect == bounds && rect.fill == chrome => {
                        Some(rect)
                    }
                    _ => None,
                })
                .expect("the application paints its window background");
            assert_eq!(background.corner_radius, egui::CornerRadius::same(radius));
        }
    }
    assert_eq!(
        eframe::App::clear_color(&app, &egui::Visuals::default()),
        egui::Rgba::TRANSPARENT.to_array()
    );
}

#[test]
fn window_zoom_preserves_native_window_dimensions() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    let ctx = egui::Context::default();
    for zoom in [1.0, 1.5, 0.8] {
        ctx.set_zoom_factor(zoom);
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .maximized = Some(false);
        ctx.begin_pass(input);
        app.observe_window(&ctx, egui::vec2(900.0, 600.0) / zoom);
        ctx.end_pass().textures_delta.clear();
        assert_eq!(app.window_state.inner_size, [900.0, 600.0]);
    }
}

#[test]
fn maximized_restoration_waits_for_the_os_without_overwriting_normal_geometry() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    app.restore_maximized = true;
    app.window_state = window_state::WindowState {
        inner_size: [900.0, 640.0],
        maximized: true,
        ..Default::default()
    };
    let ctx = egui::Context::default();
    for maximized in [false, false, true] {
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .maximized = Some(maximized);
        ctx.begin_pass(input);
        app.observe_window(&ctx, egui::vec2(1920.0, 1080.0));
        let mut output = ctx.end_pass();
        if !maximized {
            assert!(
                output.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .contains(&egui::ViewportCommand::Maximized(true))
            );
        }
        output.textures_delta.clear();
        assert_eq!(app.window_state.inner_size, [900.0, 640.0]);
        assert!(app.window_state.maximized);
    }
    assert!(!app.restore_maximized);
}

#[test]
fn window_saves_flush_on_exit_even_when_workspace_writes_are_protected() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    let ctx = egui::Context::default();
    let mut startup = loaded(app.config.clone(), Model::default());
    startup.report.can_write = false;
    app.complete_startup(&ctx, startup);
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .maximized = Some(false);
    ctx.begin_pass(input);
    app.observe_window(&ctx, egui::vec2(900.0, 640.0));
    ctx.end_pass().textures_delta.clear();
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .maximized = Some(true);
    ctx.begin_pass(input);
    app.observe_window(&ctx, egui::vec2(1920.0, 1080.0));
    ctx.end_pass().textures_delta.clear();
    eframe::App::on_exit(&mut app);
    let saved = window_state::load(&app.window_path);
    assert_eq!(saved.state.inner_size, [900.0, 640.0]);
    assert!(saved.state.maximized);
    assert!(!app.state_path.exists());
}

/// One tick of a minimized, unfocused window: eframe runs `logic` but no `ui`.
fn hidden_tick(app: &mut App, ctx: &egui::Context, close: bool) -> Vec<egui::ViewportCommand> {
    let mut input = egui::RawInput {
        focused: false,
        ..Default::default()
    };
    let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
    viewport.minimized = Some(true);
    if close {
        viewport.events.push(egui::ViewportEvent::Close);
    }
    let mut frame = eframe::Frame::_new_kittest();
    ctx.run_logic(&input, |ctx| eframe::App::logic(app, ctx, &mut frame))
        .viewport_commands
        .remove(&egui::ViewportId::ROOT)
        .unwrap_or_default()
}

#[test]
#[cfg(unix)]
fn hidden_window_reports_focus_loss_even_when_a_dialog_owns_input() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = fixture(root.path());
    app.startup = None;
    app.command = None;
    app.config.shell = Some("/bin/sh".into());
    app.config.desktop_notifications = false;
    let ctx = egui::Context::default();
    app.dispatch(
        &ctx,
        Command::AddWorkspace {
            group: None,
            cwd: root.path().into(),
            name: "Notification probe".into(),
            remote: None,
        },
    );
    let pane = app.controller.model().active_pane().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.sessions.get(pane).is_none() {
        app.poll(&ctx);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    app.sessions.get(pane).unwrap().write(
        b"stty raw -echo; printf '\\033[?1004h'; while :; do bytes=$(dd bs=1 count=3 2>/dev/null | od -An -tx1 | tr -d ' \\n'); [ \"$bytes\" = 1b5b4f ] && break; done; printf '1b 5b 4f\\007'\r",
    ).unwrap();
    while !app
        .sessions
        .get(pane)
        .unwrap()
        .modes()
        .contains(terminal_core::TermMode::FOCUS_IN_OUT)
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    app.ui.overlay = OverlayState::Settings;
    while app.notifications.entries().count() == 0 {
        hidden_tick(&mut app, &ctx, false);
        assert!(
            Instant::now() < deadline,
            "Codex never received focus loss while minimized"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.sessions
            .get(pane)
            .unwrap()
            .screen_text()
            .contains("1b 5b 4f")
    );
    assert_eq!(app.notifications.unread(Some(pane)), 1);
}

#[test]
fn hidden_window_keeps_polling() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, sender) = fixture(root.path());
    let ctx = egui::Context::default();
    sender
        .send(loaded(app.config.clone(), Model::default()))
        .unwrap();
    hidden_tick(&mut app, &ctx, false);
    assert!(app.startup.is_none());
    assert_eq!(app.controller.model().workspaces().len(), 1);
}

#[test]
fn hidden_window_close_shows_the_confirmation() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    let ctx = egui::Context::default();
    let commands = hidden_tick(&mut app, &ctx, true);
    assert_eq!(app.ui.overlay, OverlayState::ConfirmClose(Close::App));
    assert!(!app.exit_approved);
    assert_eq!(
        commands,
        [
            egui::ViewportCommand::CancelClose,
            egui::ViewportCommand::Minimized(false),
            egui::ViewportCommand::Focus,
        ]
    );
}

#[test]
fn hidden_window_closes_when_nothing_needs_confirming() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.config.confirm_close = false;
    app.config.warn_running_processes = false;
    let ctx = egui::Context::default();
    let commands = hidden_tick(&mut app, &ctx, true);
    assert!(app.exit_approved);
    assert_eq!(
        commands,
        [
            egui::ViewportCommand::CancelClose,
            egui::ViewportCommand::Close
        ]
    );
    // The repeated request is no longer cancelled, so eframe closes.
    assert_eq!(
        hidden_tick(&mut app, &ctx, true),
        Vec::<egui::ViewportCommand>::new()
    );
}

#[test]
fn protected_window_state_does_not_block_preferences_saves() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    app.window_writable = false;
    let original = br#"{"version":999,"opaque":"future window"}"#;
    std::fs::write(&app.window_path, original).unwrap();
    let ctx = egui::Context::default();
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    let config = Config {
        font_size: 20.0,
        ..app.config.clone()
    };
    app.action(&ctx, Action::Preferences(config));
    eframe::App::on_exit(&mut app);
    assert_eq!(std::fs::read(&app.window_path).unwrap(), original);
    assert_eq!(Config::load(&app.config_path).unwrap().font_size, 20.0);
}

#[test]
fn startup_actions_wait_then_replay_on_restored_state_without_retargeting_the_cli_command() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    let mut restored = Controller::new(Model::default());
    restored
        .dispatch(Command::AddWorkspace {
            group: None,
            cwd: root.path().into(),
            name: "Restored".into(),
            remote: None,
        })
        .unwrap();
    let original = restored.model().active_pane().unwrap();
    let sidebar = restored.model().sidebar();
    app.action(&ctx, Action::New);
    app.action(&ctx, Action::ToggleSidebar);
    assert!(app.controller.model().workspaces().is_empty());
    assert_eq!(app.sessions.usage().starting, 0);
    app.complete_startup(&ctx, loaded(app.config.clone(), restored.model().clone()));
    assert_eq!(app.controller.model().workspaces().len(), 2);
    assert_eq!(app.controller.model().workspaces()[0].name(), "Restored");
    assert_eq!(
        app.controller.model().workspaces()[1].cwd(),
        directories::BaseDirs::new().unwrap().home_dir()
    );
    assert_eq!(app.ui.overlay, OverlayState::None);
    assert_eq!(app.controller.model().sidebar(), !sidebar);
    assert_eq!(app.command_target, Some((original, 1)));
    assert_ne!(app.controller.model().active_pane(), Some(original));
    assert!(app.deferred_actions.is_empty());
}

#[test]
fn startup_queue_is_bounded_and_does_not_start_sessions_early() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    for _ in 0..25 {
        app.action(&ctx, Action::New);
    }
    assert_eq!(app.deferred_actions.len(), 24);
    assert!(app.controller.model().workspaces().is_empty());
    assert_eq!(app.sessions.usage().starting, 0);
    assert!(app.ui.error.as_deref().unwrap().contains("Too many"));
}

#[test]
fn cancelling_close_preserves_state_and_confirmation_keeps_the_original_target() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    for name in ["First", "Second"] {
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: root.path().into(),
                name: name.into(),
                remote: None,
            })
            .unwrap();
    }
    let original = app.controller.model().workspaces()[0].active();
    let other = app.controller.model().active_pane().unwrap();
    let generation = app.controller.generation();
    app.action(&ctx, Action::ClosePane(original));
    assert_eq!(
        app.ui.overlay,
        OverlayState::ConfirmClose(Close::Pane(original))
    );
    app.action(&ctx, Action::CancelClose);
    assert_eq!(app.controller.generation(), generation);
    assert_eq!(app.controller.model().workspaces().len(), 2);
    assert_eq!(app.controller.model().active_pane(), Some(other));
    assert_eq!(app.ui.overlay, OverlayState::None);
    app.action(&ctx, Action::ClosePane(original));
    app.action(&ctx, Action::Confirm(Close::Pane(original)));
    assert!(app.controller.model().pane(original).is_none());
    assert!(app.controller.model().pane(other).is_some());
    assert_eq!(app.controller.model().active_pane(), Some(other));
    assert_eq!(app.controller.model().workspaces().len(), 1);
}

#[test]
fn pending_preferences_coalesce_without_closing_settings() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.ui.overlay = OverlayState::Settings;
    let mut config = app.config.clone();
    config.font_size = 18.0;
    app.action(&ctx, Action::Preferences(config.clone()));
    config.font_size = 20.0;
    app.action(&ctx, Action::Preferences(config));
    assert_eq!(app.deferred_actions.len(), 1);
    assert_eq!(app.ui.overlay, OverlayState::Settings);
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    assert_eq!(app.config.font_size, 20.0);
    assert_eq!(app.preference_generation, 1);
}

#[test]
fn failed_restoration_keeps_storage_read_only_and_replays_user_actions() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.action(
        &ctx,
        Action::Create(root.path().into(), Some("Queued".into())),
    );
    drop(sender);
    app.poll(&ctx);
    assert!(app.startup.is_none());
    assert!(!app.state_writable);
    assert_eq!(app.controller.model().workspaces().len(), 2);
    assert_eq!(app.controller.model().workspaces()[1].name(), "Queued");
    assert!(app.deferred_actions.is_empty());
    assert!(!app.state_path.exists());
}

#[test]
fn startup_command_is_cancelled_if_its_original_pane_is_closed_restarted_or_failed() {
    let root = tempfile::tempdir().unwrap();
    for operation in 0..3 {
        let (mut app, _sender) = fixture(root.path());
        let ctx = egui::Context::default();
        app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
        let (pane, _) = app.command_target.unwrap();
        app.dispatch(
            &ctx,
            match operation {
                0 => Command::ClosePane(pane),
                1 => Command::RestartPane(pane),
                _ => Command::SessionFailed {
                    pane,
                    generation: app.command_target.unwrap().1,
                    error: "Missing shell".into(),
                },
            },
        );
        app.send_startup_command();
        assert!(app.command.is_none());
        assert!(
            app.ui
                .error
                .as_deref()
                .unwrap()
                .contains("Startup command cancelled")
        );
    }
}

#[cfg(unix)]
#[test]
fn startup_command_reaches_the_original_pty_once_after_focus_changes() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.config.shell = Some("/bin/sh".into());
    app.initial_cwd = Some(first.clone());
    // A shell can create a redirection target before writing its contents.
    app.command = Some(": >> launch-marker; sleep 0.1; printf x >> launch-marker".into());
    app.action(&ctx, Action::Create(second.clone(), Some("Second".into())));
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    let (target, _) = app.command_target.unwrap();
    assert_ne!(app.controller.model().active_pane(), Some(target));
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.sessions.usage().running != 2 {
        app.poll(&ctx);
        assert!(
            Instant::now() < deadline,
            "sessions failed to start: {:?}",
            app.ui.error
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    app.send_startup_command();
    app.send_startup_command();
    while !std::fs::read(first.join("launch-marker")).is_ok_and(|contents| !contents.is_empty()) {
        assert!(
            Instant::now() < deadline,
            "startup command did not reach its target"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read(first.join("launch-marker")).unwrap(), b"x");
    assert!(!second.join("launch-marker").exists());
    assert!(app.command.is_none());
}

fn press(app: &mut App, ctx: &egui::Context, event: egui::Event) -> bool {
    let mut consumed = false;
    let key = match &event {
        egui::Event::Key { key, .. } => *key,
        _ => unreachable!("key events only"),
    };
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![event],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            consumed = !ui.input(|input| input.key_pressed(key));
        },
    );
    output.textures_delta.clear();
    consumed
}

fn key(key: egui::Key, physical_key: Option<egui::Key>, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn navigation_fixture(root: &std::path::Path) -> (App, [PaneId; 4]) {
    let (mut app, _sender) = fixture(root);
    app.startup = None;
    app.controller
        .dispatch(Command::AddWorkspace {
            group: None,
            cwd: root.into(),
            name: "Navigation".into(),
            remote: None,
        })
        .unwrap();
    let workspace = app.controller.model().active_workspace().unwrap();
    let top_left = app.controller.model().active_pane().unwrap();
    app.controller
        .dispatch(Command::SplitPane {
            workspace,
            pane: top_left,
            axis: neptune_model::Axis::Vertical,
            cwd: root.into(),
        })
        .unwrap();
    let top_right = app.controller.model().active_pane().unwrap();
    let mut bottoms = Vec::new();
    for pane in [top_left, top_right] {
        app.controller
            .dispatch(Command::SplitPane {
                workspace,
                pane,
                axis: neptune_model::Axis::Horizontal,
                cwd: root.into(),
            })
            .unwrap();
        bottoms.push(app.controller.model().active_pane().unwrap());
    }
    app.controller
        .dispatch(Command::FocusPane {
            workspace,
            pane: top_left,
        })
        .unwrap();
    (app, [top_left, top_right, bottoms[0], bottoms[1]])
}

#[test]
fn pane_navigation_shortcuts_move_focus_in_all_directions_and_while_zoomed() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, [top_left, top_right, bottom_left, bottom_right]) =
        navigation_fixture(root.path());
    let ctx = egui::Context::default();
    app.ui.zoomed = true;
    app.ui.search_error = Some("No matches".into());
    // Navigation also reaches panes whose shell failed or exited, and ones
    // still starting. Focus does not depend on having a live session handle.
    app.controller
        .dispatch(Command::SessionFailed {
            pane: top_right,
            generation: 1,
            error: "Shell unavailable".into(),
        })
        .unwrap();
    app.controller
        .dispatch(Command::SessionExited {
            pane: bottom_right,
            generation: 1,
        })
        .unwrap();
    for (arrow, target) in [
        (egui::Key::ArrowRight, top_right),
        (egui::Key::ArrowDown, bottom_right),
        (egui::Key::ArrowLeft, bottom_left),
        (egui::Key::ArrowUp, top_left),
    ] {
        assert!(press(
            &mut app,
            &ctx,
            key(arrow, None, egui::Modifiers::CTRL | egui::Modifiers::SHIFT)
        ));
        assert_eq!(app.controller.model().active_pane(), Some(target));
    }
    assert!(app.ui.zoomed);
    assert!(app.ui.search_error.is_none());
}

#[test]
fn pane_navigation_consumes_press_and_release_at_an_outer_edge_without_mutating_state() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, [top_left, ..]) = navigation_fixture(root.path());
    let ctx = egui::Context::default();
    let generation = app.controller.generation();
    let down = key(
        egui::Key::ArrowLeft,
        None,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    );
    let mut up = down.clone();
    if let egui::Event::Key { pressed, .. } = &mut up {
        *pressed = false;
    }
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![down, up],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            assert!(
                ui.input(|input| input.events.is_empty()),
                "neither event reaches the shell"
            );
        },
    );
    output.textures_delta.clear();
    assert_eq!(app.controller.model().active_pane(), Some(top_left));
    assert_eq!(app.controller.generation(), generation);
}

#[test]
fn pane_navigation_preserves_overlay_and_editable_field_ownership() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, [top_left, ..]) = navigation_fixture(root.path());
    let ctx = egui::Context::default();
    for overlay in [
        OverlayState::Palette,
        OverlayState::Settings,
        OverlayState::Rename(app.controller.model().active_workspace().unwrap()),
        OverlayState::ConfirmClose(ui::Close::Pane(top_left)),
    ] {
        app.ui.overlay = overlay;
        assert!(!press(
            &mut app,
            &ctx,
            key(
                egui::Key::ArrowRight,
                None,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT
            )
        ));
        assert_eq!(app.controller.model().active_pane(), Some(top_left));
    }
    app.ui.overlay = OverlayState::None;
    for field in [ui::search::input_id(), egui::Id::new("editable-field")] {
        ctx.memory_mut(|memory| memory.request_focus(field));
        assert!(!press(
            &mut app,
            &ctx,
            key(
                egui::Key::ArrowRight,
                None,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT
            )
        ));
        assert_eq!(app.controller.model().active_pane(), Some(top_left));
    }
}

#[test]
fn pane_navigation_leaves_other_arrow_chords_for_the_shell() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, [top_left, ..]) = navigation_fixture(root.path());
    let ctx = egui::Context::default();
    for modifiers in [
        egui::Modifiers::NONE,
        egui::Modifiers::CTRL,
        egui::Modifiers::SHIFT,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT | egui::Modifiers::ALT,
    ] {
        assert!(!press(
            &mut app,
            &ctx,
            key(egui::Key::ArrowRight, None, modifiers)
        ));
        assert_eq!(app.controller.model().active_pane(), Some(top_left));
    }
}

#[test]
fn pane_navigation_repeated_keys_in_one_frame_advance_from_the_new_focus() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, [top_left, _, _, bottom_right]) = navigation_fixture(root.path());
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![
                key(
                    egui::Key::ArrowRight,
                    None,
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                ),
                key(
                    egui::Key::ArrowDown,
                    None,
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                ),
            ],
            ..Default::default()
        },
        |ui| app.shortcuts(ui.ctx()),
    );
    output.textures_delta.clear();
    assert_ne!(app.controller.model().active_pane(), Some(top_left));
    assert_eq!(app.controller.model().active_pane(), Some(bottom_right));
}

#[test]
fn app_zoom_shortcuts_scale_the_ui_without_changing_terminal_preferences() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    app.config.font_size = 19.0;
    for modifiers in [egui::Modifiers::CTRL, egui::Modifiers::MAC_CMD] {
        for overlay in [
            OverlayState::None,
            OverlayState::Settings,
            OverlayState::Palette,
        ] {
            app.ui.overlay = overlay;
            let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
            output.textures_delta.clear();
            assert!(press(&mut app, &ctx, key(egui::Key::Plus, None, modifiers)));
            let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
            output.textures_delta.clear();
            assert_eq!(ctx.zoom_factor(), 1.1);
            assert_eq!(app.config.window_zoom, 1.1);
            assert_eq!(app.config.font_size, 19.0);
            let generation = app.preference_generation;
            assert!(generation > 0);

            let primary = egui::Modifiers {
                shift: false,
                ..modifiers
            };
            assert!(press(&mut app, &ctx, key(egui::Key::Minus, None, primary)));
            let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
            output.textures_delta.clear();
            assert_eq!(ctx.zoom_factor(), 1.0);
            assert_eq!(app.config.window_zoom, 1.0);

            assert!(press(&mut app, &ctx, key(egui::Key::Equals, None, primary)));
            let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
            output.textures_delta.clear();
            assert_eq!(ctx.zoom_factor(), 1.1);
            assert!(press(&mut app, &ctx, key(egui::Key::Num0, None, primary)));
            let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
            output.textures_delta.clear();
            assert_eq!(ctx.zoom_factor(), 1.0);
            assert_eq!(app.config.window_zoom, 1.0);
            assert_eq!(app.preference_generation, generation + 3);
        }
    }
}

#[test]
fn window_zoom_preferences_and_reset_survive_shutdown_and_startup() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    let ctx = egui::Context::default();
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    app.action(
        &ctx,
        Action::Preferences(Config {
            window_zoom: 1.3,
            font_size: 19.0,
            font_family: "Installed Mono".into(),
            restore_workspaces: false,
            ..app.config.clone()
        }),
    );
    eframe::App::on_exit(&mut app);

    let saved = Config::load(&app.config_path).unwrap();
    let (mut reopened, _sender) = fixture(root.path());
    reopened.ephemeral = false;
    let reopened_ctx = egui::Context::default();
    reopened.complete_startup(&reopened_ctx, loaded(saved, Model::default()));
    reopened_ctx
        .run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    assert_eq!(reopened_ctx.zoom_factor(), 1.3);
    assert_eq!(reopened.config.font_size, 19.0);
    assert_eq!(reopened.config.font_family, "Installed Mono");
    assert!(!reopened.config.restore_workspaces);

    reopened.action(&reopened_ctx, Action::ResetUiZoom);
    eframe::App::on_exit(&mut reopened);
    let saved = Config::load(&reopened.config_path).unwrap();
    assert_eq!(saved.window_zoom, 1.0);
    assert_eq!(saved.font_size, 19.0);
    assert_eq!(saved.font_family, "Installed Mono");

    let (mut reset, _sender) = fixture(root.path());
    let reset_ctx = egui::Context::default();
    reset.complete_startup(&reset_ctx, loaded(saved, Model::default()));
    reset_ctx
        .run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    assert_eq!(reset_ctx.zoom_factor(), 1.0);
}

#[test]
fn pending_zoom_actions_adjust_restored_settings_and_save_the_latest_value() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    let ctx = egui::Context::default();
    app.action(&ctx, Action::ZoomUiIn);
    app.action(&ctx, Action::ZoomUiIn);
    app.action(&ctx, Action::ZoomUiOut);
    assert_eq!(app.preference_generation, 0);
    app.complete_startup(
        &ctx,
        loaded(
            Config {
                window_zoom: 1.3,
                font_size: 21.0,
                accent: config::Accent::Indigo,
                ..app.config.clone()
            },
            Model::default(),
        ),
    );
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    assert_eq!(ctx.zoom_factor(), 1.4);
    assert_eq!(app.preference_generation, 3);
    eframe::App::on_exit(&mut app);
    let saved = Config::load(&app.config_path).unwrap();
    assert_eq!(saved.window_zoom, 1.4);
    assert_eq!(saved.font_size, 21.0);
    assert_eq!(saved.accent, config::Accent::Indigo);
}

#[test]
fn zoom_actions_respect_limits_and_defaults_reset_the_saved_zoom() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    for (zoom, action) in [(0.2, Action::ZoomUiOut), (5.0, Action::ZoomUiIn)] {
        app.config.window_zoom = zoom;
        app.action(&ctx, action);
        assert_eq!(app.config.window_zoom, zoom);
        assert_eq!(app.preference_generation, 0);
    }
    app.action(&ctx, Action::Preferences(Config::default()));
    ctx.run_ui(egui::RawInput::default(), |_| {})
        .textures_delta
        .clear();
    assert_eq!(app.config.window_zoom, 1.0);
    assert_eq!(ctx.zoom_factor(), 1.0);
}

#[test]
fn stepping_font_size_or_zoom_names_the_new_value_until_it_fades() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    let shown = |app: &App| {
        let level = app.ui.level.as_ref().unwrap();
        format!("{} {}", level.name, level.value)
    };
    for (action, text) in [
        (Action::IncreaseFontSize, "Font size 15 pt"),
        (Action::ResetFontSize, "Font size 14 pt"),
        (Action::DecreaseFontSize, "Font size 13 pt"),
        (Action::ZoomUiIn, "Window zoom 110%"),
        (Action::ZoomUiOut, "Window zoom 100%"),
        (Action::ResetUiZoom, "Window zoom 100%"),
    ] {
        app.action(&ctx, action);
        assert_eq!(shown(&app), text);
    }
    // At the end of a range the chip still answers, with the value kept.
    app.config.window_zoom = 5.0;
    app.action(&ctx, Action::ZoomUiIn);
    assert_eq!(shown(&app), "Window zoom 500%");
    app.config.font_size = 9.0;
    app.action(&ctx, Action::DecreaseFontSize);
    assert_eq!(shown(&app), "Font size 9 pt");

    // A value set in Preferences is already in view there.
    app.ui.level = None;
    app.action(
        &ctx,
        Action::Preferences(Config {
            font_size: 20.0,
            ..Config::default()
        }),
    );
    assert_eq!(app.ui.level, None);

    app.action(&ctx, Action::IncreaseFontSize);
    ctx.set_fonts(crate::platform::fonts::bundled_definitions());
    let frame = |app: &mut App, time: f64| {
        let input = egui::RawInput {
            time: Some(time),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            ui::dialogs::show(
                ui.ctx(),
                Palette::for_config(&app.config),
                &mut app.ui,
                &mut vec![],
            );
        });
        output.textures_delta.clear();
    };
    let start = app.ui.level.as_ref().unwrap().shown;
    frame(&mut app, start + 1.0);
    assert_eq!(shown(&app), "Font size 21 pt");
    frame(&mut app, start + 2.0);
    assert_eq!(app.ui.level, None);
}

#[test]
fn terminal_font_shortcuts_use_primary_shift_in_preferences_and_respect_limits() {
    for modifiers in [
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT,
    ] {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _sender) = fixture(root.path());
        let ctx = egui::Context::default();
        app.startup = None;
        app.ui.overlay = OverlayState::Settings;
        for (key_code, size) in [
            (egui::Key::Plus, 15.0),
            (egui::Key::Equals, 16.0),
            (egui::Key::Minus, 15.0),
        ] {
            assert!(press(&mut app, &ctx, key(key_code, None, modifiers)));
            assert_eq!(app.config.font_size, size);
            assert_eq!(ctx.zoom_factor(), 1.0);
        }
        for (size, key_code) in [(32.0, egui::Key::Plus), (9.0, egui::Key::Minus)] {
            app.config.font_size = size;
            assert!(press(&mut app, &ctx, key(key_code, None, modifiers)));
            assert_eq!(app.config.font_size, size);
        }
        app.config.window_zoom = 1.4;
        ctx.set_zoom_factor(1.4);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        assert!(press(&mut app, &ctx, key(egui::Key::Num0, None, modifiers)));
        assert_eq!(app.config.font_size, Config::default().font_size);
        assert_eq!(ctx.zoom_factor(), 1.4);
    }
}

#[test]
fn latin_american_font_shortcuts_use_the_labeled_keys_in_terminal_and_preferences() {
    for overlay in [OverlayState::None, OverlayState::Settings] {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _sender) = fixture(root.path());
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
        app.startup = None;
        app.ui.overlay = overlay;
        let modifiers =
            egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        for (logical, physical, text, expected) in [
            (egui::Key::CloseBracket, egui::Key::CloseBracket, "*", 15.0),
            (egui::Key::Slash, egui::Key::Slash, "_", 14.0),
            (egui::Key::Equals, egui::Key::Num0, "=", 14.0),
        ] {
            if physical == egui::Key::Num0 {
                app.config.font_size = 20.0;
            }
            ctx.run_ui(
                egui::RawInput {
                    events: vec![
                        key(logical, Some(physical), modifiers),
                        egui::Event::Text(text.into()),
                        egui::Event::Key {
                            key: logical,
                            physical_key: Some(physical),
                            pressed: false,
                            repeat: false,
                            modifiers,
                        },
                        egui::Event::Text("other input".into()),
                    ],
                    ..Default::default()
                },
                |ui| {
                    app.shortcuts_with_keymap(ui.ctx(), |key| match key {
                        egui::Key::CloseBracket => Some(egui::Key::Plus),
                        egui::Key::Slash => Some(egui::Key::Minus),
                        egui::Key::Num0 => Some(egui::Key::Num0),
                        _ => None,
                    });
                    assert_eq!(app.config.font_size, expected, "{physical:?}");
                    assert_eq!(app.config.window_zoom, 1.0);
                    assert_eq!(
                        ui.input(|input| input.events.clone()),
                        [egui::Event::Text("other input".into())]
                    );
                },
            )
            .textures_delta
            .clear();
        }
    }
}

#[test]
fn repeated_font_shortcuts_in_one_frame_accumulate_and_do_not_change_app_zoom() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    let modifiers = egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT;
    ctx.run_ui(
        egui::RawInput {
            events: vec![
                key(egui::Key::Plus, None, modifiers),
                key(egui::Key::Plus, None, modifiers),
                key(egui::Key::Minus, None, modifiers),
            ],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            assert_eq!(app.config.font_size, 15.0);
            assert_eq!(app.config.window_zoom, 1.0);
            assert!(ui.input(|input| input.events.is_empty()));
        },
    )
    .textures_delta
    .clear();
}

#[test]
fn font_shortcuts_preserve_ordinary_symbol_text_in_the_same_frame() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    app.startup = None;
    let modifiers = egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT;
    let ordinary = vec![
        egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
        key(
            egui::Key::CloseBracket,
            Some(egui::Key::CloseBracket),
            egui::Modifiers::SHIFT,
        ),
        egui::Event::Text("*".into()),
    ];
    let mut events = vec![
        key(
            egui::Key::CloseBracket,
            Some(egui::Key::CloseBracket),
            modifiers,
        ),
        egui::Event::Text("*".into()),
        egui::Event::Key {
            key: egui::Key::CloseBracket,
            physical_key: Some(egui::Key::CloseBracket),
            pressed: false,
            repeat: false,
            modifiers,
        },
    ];
    events.extend(ordinary.clone());
    ctx.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ui| {
            app.shortcuts_with_keymap(ui.ctx(), |_| Some(egui::Key::Plus));
            assert_eq!(app.config.font_size, 15.0);
            assert_eq!(ui.input(|input| input.events.clone()), ordinary);
        },
    )
    .textures_delta
    .clear();
}

#[test]
fn font_keymap_resolution_leaves_unrelated_modifiers_and_keys_with_their_owner() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    app.startup = None;
    for modifiers in [
        egui::Modifiers::NONE,
        egui::Modifiers::SHIFT,
        egui::Modifiers::MAC_CMD | egui::Modifiers::ALT | egui::Modifiers::SHIFT,
        egui::Modifiers::MAC_CMD | egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
    ] {
        let event = key(
            egui::Key::CloseBracket,
            Some(egui::Key::CloseBracket),
            modifiers,
        );
        ctx.run_ui(
            egui::RawInput {
                events: vec![event.clone()],
                ..Default::default()
            },
            |ui| {
                let original = ui.input(|input| input.events.clone());
                app.shortcuts_with_keymap(ui.ctx(), |_| panic!("not a font shortcut"));
                assert_eq!(ui.input(|input| input.events.clone()), original);
            },
        )
        .textures_delta
        .clear();
    }
    let event = key(
        egui::Key::Questionmark,
        Some(egui::Key::Slash),
        egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT,
    );
    ctx.run_ui(
        egui::RawInput {
            events: vec![event.clone()],
            ..Default::default()
        },
        |ui| {
            let original = ui.input(|input| input.events.clone());
            // On a US layout this key is /, rather than Latin America's -.
            app.shortcuts_with_keymap(ui.ctx(), |_| None);
            assert_eq!(ui.input(|input| input.events.clone()), original);
        },
    )
    .textures_delta
    .clear();
    assert_eq!(app.config.font_size, 14.0);
    assert_eq!(app.config.window_zoom, 1.0);
}

#[test]
fn zoom_shortcuts_consume_key_and_text_events_before_terminal_input() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    for (key_code, modifiers, text) in [
        (egui::Key::Plus, egui::Modifiers::CTRL, "+"),
        (egui::Key::Equals, egui::Modifiers::CTRL, "="),
        (egui::Key::Minus, egui::Modifiers::CTRL, "-"),
        (egui::Key::Num0, egui::Modifiers::CTRL, "0"),
        (
            egui::Key::Plus,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            "+",
        ),
        (
            egui::Key::Minus,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            "_",
        ),
        (
            egui::Key::Num0,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            ")",
        ),
        (
            egui::Key::Plus,
            egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT,
            "+",
        ),
        (
            egui::Key::Equals,
            egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT,
            "+",
        ),
        (
            egui::Key::Minus,
            egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT,
            "_",
        ),
        (
            egui::Key::Num0,
            egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT,
            ")",
        ),
    ] {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![
                    key(key_code, None, modifiers),
                    egui::Event::Text(text.into()),
                    egui::Event::Text("other input".into()),
                ],
                ..Default::default()
            },
            |ui| {
                app.shortcuts(ui.ctx());
                assert_eq!(
                    ui.input(|input| input.events.clone()),
                    [egui::Event::Text("other input".into())]
                );
            },
        );
        output.textures_delta.clear();
    }
}

#[test]
fn ordinary_and_alt_modified_keys_keep_their_input_owner() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    app.startup = None;
    for modifiers in [
        egui::Modifiers::NONE,
        egui::Modifiers::CTRL | egui::Modifiers::ALT,
    ] {
        for key_code in [
            egui::Key::Plus,
            egui::Key::Equals,
            egui::Key::Minus,
            egui::Key::Num0,
        ] {
            assert!(!press(&mut app, &ctx, key(key_code, None, modifiers)));
        }
    }
    assert_eq!(ctx.zoom_factor(), 1.0);
    assert_eq!(app.config.font_size, 14.0);
    assert_eq!(app.preference_generation, 0);
}

#[test]
fn escape_leaves_one_surface_at_a_time_and_otherwise_belongs_to_the_shell() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    let escape = || key(egui::Key::Escape, None, egui::Modifiers::NONE);
    app.startup = None;
    app.ui.overlay = OverlayState::Settings;
    app.ui.search_open = true;
    app.ui.error = Some("Storage is read-only".into());

    assert!(press(&mut app, &ctx, escape()));
    assert_eq!(app.ui.overlay, OverlayState::None);
    assert!(app.ui.search_open && app.ui.error.is_some());

    // With no terminal to receive it, Escape dismisses the message; search,
    // whose field is not focused, stays.
    assert!(press(&mut app, &ctx, escape()));
    assert!(app.ui.error.is_none() && app.ui.search_open);

    app.controller
        .dispatch(Command::AddWorkspace {
            group: None,
            cwd: root.path().into(),
            name: "Shell".into(),
            remote: None,
        })
        .unwrap();
    assert!(app.controller.model().active_pane().is_some());

    // A message never takes a key the shell is waiting for.
    app.ui.error = Some("Storage is read-only".into());
    assert!(!press(&mut app, &ctx, escape()));
    assert!(app.ui.error.is_some() && app.ui.search_open);

    ctx.memory_mut(|memory| memory.request_focus(ui::search::input_id()));
    assert!(press(&mut app, &ctx, escape()));
    assert!(!app.ui.search_open);
    assert!(app.ui.error.is_some(), "one surface per Escape");
}

#[test]
fn escape_cancels_a_terminal_drag_instead_of_reaching_the_shell() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    let escape = || key(egui::Key::Escape, None, egui::Modifiers::NONE);
    app.startup = None;
    app.controller
        .dispatch(Command::AddWorkspace {
            group: None,
            cwd: root.path().into(),
            name: "Shell".into(),
            remote: None,
        })
        .unwrap();
    let generation = app.controller.generation();
    app.ui.pane_drag = app.controller.model().active_pane();

    assert!(press(&mut app, &ctx, escape()));
    assert!(app.ui.pane_drag.is_none());
    assert_eq!(app.controller.generation(), generation);
    // With nothing left to cancel, the key is the shell's again.
    assert!(!press(&mut app, &ctx, escape()));
}

#[cfg(unix)]
#[test]
fn a_terminal_moved_to_another_workspace_keeps_its_running_shell() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    app.config.shell = Some("/bin/sh".into());
    let add = |app: &mut App, name: &str| {
        app.dispatch(
            &ctx,
            Command::AddWorkspace {
                group: None,
                cwd: root.path().into(),
                name: name.into(),
                remote: None,
            },
        );
        app.controller.model().active_workspace().unwrap()
    };
    let home = add(&mut app, "Home");
    let stays = app.controller.model().active_pane().unwrap();
    app.action(&ctx, Action::Split(stays, neptune_model::Axis::Vertical));
    let moved = app.controller.model().active_pane().unwrap();
    let other = add(&mut app, "Other");
    app.action(&ctx, Action::SelectWorkspace(home));
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.sessions.usage().running != 3 {
        app.poll(&ctx);
        assert!(
            Instant::now() < deadline,
            "sessions failed to start: {:?}",
            app.ui.error
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let process = app.sessions.get(moved).unwrap().metadata().process_id;
    assert!(process.is_some());

    app.action(
        &ctx,
        Action::MovePane(moved, neptune_model::Destination::Workspace(other)),
    );
    let model = app.controller.model();
    assert_eq!(model.workspace_for_pane(moved), Some(other));
    assert_eq!(model.active_workspace(), Some(home));
    assert_eq!(model.active_pane(), Some(stays));
    assert_eq!(model.pane(moved).unwrap().generation(), 1);
    assert!(app.ui.error.is_none(), "{:?}", app.ui.error);

    // The same process, still accepting input while its workspace is hidden.
    let usage = app.sessions.usage();
    assert_eq!((usage.running, usage.starting, usage.closing), (3, 0, 0));
    assert!(app.renders.contains_key(&moved));
    let session = app.sessions.get(moved).unwrap();
    assert_eq!(session.metadata().process_id, process);
    session.write(b": > moved-marker\r").unwrap();
    while !root.path().join("moved-marker").exists() {
        assert!(
            Instant::now() < deadline,
            "the moved shell stopped accepting input"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn find_returns_to_an_open_search_field_before_it_closes() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    app.action(&ctx, Action::Find);
    assert!(app.ui.search_open && app.ui.search_focus);

    // The field took focus and the user then clicked back into the terminal.
    app.ui.search_focus = false;
    app.action(&ctx, Action::Find);
    assert!(app.ui.search_open, "search stays open");
    assert!(app.ui.search_focus, "the field is focused again");

    ctx.memory_mut(|memory| memory.request_focus(ui::search::input_id()));
    app.action(&ctx, Action::Find);
    assert!(!app.ui.search_open);
}

#[test]
fn toggling_the_sidebar_starts_a_slide_from_where_it_is() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    assert!(app.controller.model().sidebar());
    assert_eq!(
        app.ui.sidebar_slide, None,
        "a sidebar that was not toggled rests in place"
    );

    app.action(&ctx, Action::ToggleSidebar);
    assert!(!app.controller.model().sidebar());
    let hide = app.ui.sidebar_slide.unwrap();
    assert_eq!(hide.reveal(false, 0.0), Some(1.0));
    assert_eq!(hide.reveal(false, 1.0), None);

    // Toggled back before the first slide moved: it starts fully shown.
    app.action(&ctx, Action::ToggleSidebar);
    assert!(app.controller.model().sidebar());
    assert_eq!(app.ui.sidebar_slide.unwrap().reveal(true, 0.0), Some(1.0));
}

#[test]
fn sidebar_width_is_clamped_and_saved_as_one_preference_change() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    app.action(&ctx, Action::SidebarWidth(900.0));
    assert_eq!(app.config.sidebar_width, 360.0);
    app.action(&ctx, Action::SidebarWidth(f32::NAN));
    assert_eq!(
        app.config.sidebar_width, 360.0,
        "invalid widths are refused"
    );
    app.action(&ctx, Action::SidebarWidth(12.0));
    assert_eq!(app.config.sidebar_width, 170.0);
    assert_eq!(app.preference_generation, 2);
}

#[test]
fn rename_requests_field_focus_and_cancels_without_touching_workspaces() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    app.controller
        .dispatch(Command::AddWorkspace {
            group: None,
            cwd: root.path().into(),
            name: "Only".into(),
            remote: None,
        })
        .unwrap();
    let workspace = app.controller.model().active_workspace().unwrap();
    let generation = app.controller.generation();

    app.action(&ctx, Action::Rename(workspace));
    assert_eq!(app.ui.overlay, OverlayState::Rename(workspace));
    assert!(app.ui.overlay_focus);
    assert_eq!(app.ui.rename_name, "Only");
    app.action(&ctx, Action::CloseOverlay);
    assert_eq!(app.ui.overlay, OverlayState::None);

    app.ui.error = Some("Could not save".into());
    app.action(&ctx, Action::DismissError);
    assert!(app.ui.error.is_none());
    assert_eq!(app.controller.generation(), generation);
    assert_eq!(app.controller.model().workspaces().len(), 1);
}

#[test]
fn workspace_sidebar_follows_the_panes_current_directory() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let home = directories::BaseDirs::new()
        .unwrap()
        .home_dir()
        .to_path_buf();
    app.controller
        .dispatch(Command::AddWorkspace {
            group: None,
            cwd: home.clone(),
            name: "biggabo".into(),
            remote: None,
        })
        .unwrap();
    let pane = app.controller.model().active_pane().unwrap();
    let project = home.join("Documents/Projects/kreando/kreando");
    app.controller
        .dispatch(Command::PaneCwdChanged {
            pane,
            generation: 1,
            cwd: project.clone(),
        })
        .unwrap();

    assert_eq!(app.controller.model().pane(pane).unwrap().cwd(), project);
    assert_eq!(app.views()[0].cwd, project);
    assert_eq!(app.controller.model().workspaces()[0].cwd(), home);

    let saved =
        crate::persistence::workspace_state::StateSnapshot::from_model(app.controller.model());
    app.controller = Controller::new(saved.into_model(neptune_model::Limits::default()).unwrap());
    assert_eq!(app.views()[0].cwd, project);
    app.controller
        .dispatch(Command::PaneCwdChanged {
            pane,
            generation: 1,
            cwd: home.clone(),
        })
        .unwrap();
    assert_eq!(app.views()[0].cwd, home);
}

#[test]
fn workspace_sidebar_uses_each_workspaces_focused_pane() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let workspace = add_workspace(&mut app, root.path(), None);
    let first = app.controller.model().active_pane().unwrap();
    let second_directory = root.path().join("second");
    app.controller
        .dispatch(Command::AddTab {
            workspace,
            pane: first,
            cwd: second_directory.clone(),
        })
        .unwrap();
    let second = app.controller.model().active_pane().unwrap();
    assert_eq!(app.views()[0].cwd, second_directory);

    let first_directory = root.path().join("first");
    app.controller
        .dispatch(Command::PaneCwdChanged {
            pane: first,
            generation: 1,
            cwd: first_directory.clone(),
        })
        .unwrap();
    assert_eq!(app.views()[0].cwd, second_directory);
    add_workspace(&mut app, root.path(), None);
    assert_eq!(app.views()[0].cwd, second_directory);
    assert_eq!(app.views()[1].cwd, root.path());

    app.controller
        .dispatch(Command::FocusPane {
            workspace,
            pane: first,
        })
        .unwrap();
    assert_eq!(app.views()[0].cwd, first_directory);
    app.controller.dispatch(Command::ClosePane(first)).unwrap();
    assert_eq!(app.views()[0].cwd, second_directory);
    app.controller
        .dispatch(Command::RestartPane(second))
        .unwrap();
    assert_eq!(app.views()[0].cwd, second_directory);
}

#[test]
fn new_workspace_opens_at_home_without_a_dialog_and_can_be_renamed() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    app.controller
        .dispatch(Command::AddWorkspace {
            group: None,
            cwd: root.path().into(),
            name: "Existing".into(),
            remote: None,
        })
        .unwrap();
    let existing = app.controller.model().active_workspace().unwrap();
    app.ui.overlay = OverlayState::Palette;

    app.action(&ctx, Action::New);

    let workspace = app.controller.model().active_workspace().unwrap();
    let pane = app.controller.model().active_pane().unwrap();
    let home = directories::BaseDirs::new().unwrap();
    assert_ne!(workspace, existing);
    assert_eq!(app.controller.model().workspaces().len(), 2);
    assert_eq!(
        app.controller.model().workspace(workspace).unwrap().cwd(),
        home.home_dir()
    );
    assert_eq!(
        app.controller.model().pane(pane).unwrap().cwd(),
        home.home_dir()
    );
    assert_eq!(app.ui.overlay, OverlayState::None);
    assert_eq!(app.sessions.usage().starting, 1);

    app.action(&ctx, Action::Rename(workspace));
    assert_eq!(app.ui.overlay, OverlayState::Rename(workspace));
    assert!(app.ui.overlay_focus);
    // A later focus change cannot retarget the rename.
    app.action(&ctx, Action::SelectWorkspace(existing));
    app.action(&ctx, Action::SetName(workspace, "Renamed".into()));
    app.action(&ctx, Action::CloseOverlay);
    let renamed = app.controller.model().workspace(workspace).unwrap();
    assert_eq!(renamed.name(), "Renamed");
    assert_eq!(renamed.cwd(), home.home_dir());
    assert_eq!(renamed.active(), pane);
    assert_eq!(
        app.controller.model().workspace(existing).unwrap().name(),
        "Existing"
    );
}

#[test]
fn new_workspace_shortcut_works_with_no_existing_workspace() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    let command = if cfg!(target_os = "macos") {
        egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND
    } else {
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT
    };

    assert!(press(&mut app, &ctx, key(egui::Key::T, None, command)));
    let workspace = app.controller.model().workspaces().first().unwrap();
    assert_eq!(
        workspace.cwd(),
        directories::BaseDirs::new().unwrap().home_dir()
    );
    assert_eq!(
        app.controller.model().active_workspace(),
        Some(workspace.id())
    );
    assert_eq!(app.ui.overlay, OverlayState::None);
}

#[test]
fn command_digits_select_workspaces_by_position_even_when_shift_changes_the_symbol() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    for name in ["First", "Second", "Third"] {
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: root.path().into(),
                name: name.into(),
                remote: None,
            })
            .unwrap();
    }
    let ids: Vec<_> = app
        .controller
        .model()
        .workspaces()
        .iter()
        .map(|workspace| workspace.id())
        .collect();
    assert_eq!(app.controller.model().active_workspace(), Some(ids[2]));
    let command = if cfg!(target_os = "macos") {
        egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND
    } else {
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT
    };
    // On a US layout Shift+1 arrives as "!" with the digit as its physical key.
    assert!(press(
        &mut app,
        &ctx,
        key(egui::Key::Exclamationmark, Some(egui::Key::Num1), command)
    ));
    assert_eq!(app.controller.model().active_workspace(), Some(ids[0]));
    assert!(press(&mut app, &ctx, key(egui::Key::Num2, None, command)));
    assert_eq!(app.controller.model().active_workspace(), Some(ids[1]));
    // No ninth workspace: the key is left for the terminal and nothing changes.
    assert!(!press(&mut app, &ctx, key(egui::Key::Num9, None, command)));
    assert_eq!(app.controller.model().active_workspace(), Some(ids[1]));
    // A plain digit is ordinary typing.
    assert!(!press(
        &mut app,
        &ctx,
        key(egui::Key::Num1, None, egui::Modifiers::NONE)
    ));
    assert_eq!(app.controller.model().active_workspace(), Some(ids[1]));
}

#[test]
fn tab_shortcuts_open_and_step_through_the_tabs_of_the_focused_place() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    let command = if cfg!(target_os = "macos") {
        egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND
    } else {
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT
    };
    // Without a terminal to open a tab beside, there is nothing to step to.
    assert!(!press(
        &mut app,
        &ctx,
        key(egui::Key::PageDown, None, command)
    ));
    app.controller
        .dispatch(Command::AddWorkspace {
            group: None,
            cwd: root.path().into(),
            name: "First".into(),
            remote: None,
        })
        .unwrap();
    let first = app.controller.model().active_pane().unwrap();
    assert!(!press(
        &mut app,
        &ctx,
        key(egui::Key::PageDown, None, command)
    ));
    assert!(press(&mut app, &ctx, key(egui::Key::T, None, command)));
    let second = app.controller.model().active_pane().unwrap();
    assert!(press(&mut app, &ctx, key(egui::Key::T, None, command)));
    let third = app.controller.model().active_pane().unwrap();
    let workspace = &app.controller.model().workspaces()[0];
    assert_eq!(app.controller.model().workspaces().len(), 1);
    assert_eq!(
        workspace.layout().tabs(first),
        Some((&[first, second, third][..], third))
    );
    for (pressed, focused) in [
        (egui::Key::PageDown, first),
        (egui::Key::PageUp, third),
        (egui::Key::PageUp, second),
    ] {
        assert!(press(&mut app, &ctx, key(pressed, None, command)));
        assert_eq!(app.controller.model().active_pane(), Some(focused));
    }
    // Only the tab in view is drawn; the others contribute their titles.
    let presentations = app.presentations();
    assert_eq!(app.shown(), [second]);
    for pane in [first, second, third] {
        assert_eq!(presentations[&pane].snapshot.is_some(), pane == second);
    }
    // Zooming keeps the focused terminal's tabs within reach.
    app.ui.zoomed = true;
    assert!(press(
        &mut app,
        &ctx,
        key(egui::Key::PageDown, None, command)
    ));
    assert_eq!(app.shown(), [third]);
    // Without the command chord the keys belong to the terminal.
    assert!(!press(
        &mut app,
        &ctx,
        key(egui::Key::PageDown, None, egui::Modifiers::NONE)
    ));
    assert_eq!(app.controller.model().active_pane(), Some(third));
}

#[test]
fn moving_a_workspace_keeps_focus_and_renumbers_the_position_shortcuts() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    for name in ["First", "Second", "Third"] {
        app.controller
            .dispatch(Command::AddWorkspace {
                group: None,
                cwd: root.path().into(),
                name: name.into(),
                remote: None,
            })
            .unwrap();
    }
    let names = |app: &App| -> Vec<String> {
        let workspaces = app.controller.model().workspaces();
        workspaces.iter().map(|w| w.name().to_owned()).collect()
    };
    let third = app.controller.model().active_workspace().unwrap();
    let pane = app.controller.model().active_pane();
    app.action(&ctx, Action::MoveWorkspace(third, 0));
    assert_eq!(names(&app), ["Third", "First", "Second"]);
    assert_eq!(app.controller.model().active_workspace(), Some(third));
    assert_eq!(app.controller.model().active_pane(), pane);
    assert!(app.ui.error.is_none());

    let command = if cfg!(target_os = "macos") {
        egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND
    } else {
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT
    };
    assert!(press(&mut app, &ctx, key(egui::Key::Num2, None, command)));
    let active = app.controller.model().active_workspace().unwrap();
    assert_eq!(
        app.controller.model().workspace(active).unwrap().name(),
        "First"
    );

    // A workspace closed before its queued move arrives is reported, not moved.
    app.controller
        .dispatch(Command::CloseWorkspace(third))
        .unwrap();
    app.action(&ctx, Action::MoveWorkspace(third, 1));
    assert_eq!(names(&app), ["First", "Second"]);
    assert!(app.ui.error.is_some());
}

fn add_workspace(app: &mut App, root: &std::path::Path, remote: Option<&str>) -> WorkspaceId {
    // Dispatched on the controller alone, so no session is started.
    app.controller
        .dispatch(Command::AddWorkspace {
            group: None,
            cwd: root.into(),
            name: "Workspace".into(),
            remote: remote.map(str::to_owned),
        })
        .unwrap();
    app.controller.model().active_workspace().unwrap()
}

fn remote_of(app: &App, workspace: WorkspaceId) -> Option<&str> {
    app.controller
        .model()
        .workspace(workspace)
        .unwrap()
        .remote()
        .map(|remote| remote.destination())
}

#[test]
fn a_remote_terminal_runs_the_ssh_client_with_a_pty_and_a_quoted_bootstrap() {
    let config = Config {
        shell: Some("/bin/zsh".into()),
        scrollback: 500,
        ..Config::default()
    };
    let remote = Remote::parse("me@devbox").unwrap();
    let options =
        coordinator::session_options(&config, "ssh", "/srv/app".into(), Some(&remote), None);
    assert_eq!(options.shell.as_deref(), Some("ssh"));
    // `--` ends option parsing, so the destination can only be a host.
    assert_eq!(options.args[..3], ["-t", "--", "me@devbox"]);
    assert_eq!(options.args.len(), 4);
    assert!(options.args[3].starts_with("sh -c '"));
    assert_eq!(options.cwd, std::path::Path::new("/srv/app"));
    assert_eq!(options.scrollback, 500);

    let local = coordinator::session_options(&config, "ssh", "/srv/app".into(), None, None);
    assert_eq!(local.shell.as_deref(), Some("/bin/zsh"));
    assert!(local.args.is_empty());

    // Shell arguments apply to local terminals, never to the SSH client.
    let config = Config {
        shell: Some("/bin/zsh".into()),
        shell_args: vec!["-l".into()],
        ..Config::default()
    };
    let local = coordinator::session_options(&config, "ssh", "/srv/app".into(), None, None);
    assert_eq!(local.args, ["-l"]);
    let options =
        coordinator::session_options(&config, "ssh", "/srv/app".into(), Some(&remote), None);
    assert_eq!(options.args[..3], ["-t", "--", "me@devbox"]);
}

#[test]
fn the_ssh_sheet_opens_for_local_workspaces_and_connecting_replaces_their_terminals() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    app.action(&ctx, Action::Create(root.path().into(), None));
    let workspace = app.controller.model().active_workspace().unwrap();
    let pane = app.controller.model().active_pane().unwrap();
    app.ui.ssh_host = "stale".into();

    app.action(&ctx, Action::Ssh(Some(workspace)));
    assert_eq!(app.ui.overlay, OverlayState::Ssh(Some(workspace)));
    assert!(app.ui.overlay_focus);
    assert!(app.ui.ssh_host.is_empty());

    app.action(
        &ctx,
        Action::Connect {
            workspace: Some(workspace),
            destination: "me@devbox".into(),
        },
    );
    assert_eq!(app.ui.overlay, OverlayState::None);
    assert_eq!(remote_of(&app, workspace), Some("me@devbox"));
    assert_eq!(app.controller.model().pane(pane).unwrap().generation(), 2);
    assert_eq!(app.sessions.generation(pane), Some(2));
    assert_eq!(app.controller.model().workspaces().len(), 1);

    // A connected workspace is disconnected before it can be pointed elsewhere.
    app.action(&ctx, Action::Ssh(Some(workspace)));
    assert_eq!(app.ui.overlay, OverlayState::None);
    // A workspace that no longer exists has nothing to connect.
    app.action(&ctx, Action::Ssh(Some(WorkspaceId::new(99))));
    assert_eq!(app.ui.overlay, OverlayState::None);

    app.action(&ctx, Action::Ssh(None));
    assert_eq!(app.ui.overlay, OverlayState::Ssh(None));
    app.action(
        &ctx,
        Action::Connect {
            workspace: None,
            destination: "ssh://me@buildbox".into(),
        },
    );
    assert_eq!(app.ui.overlay, OverlayState::None);
    let created = app.controller.model().workspaces().last().unwrap();
    assert_eq!(created.name(), "buildbox");
    assert_eq!(
        created.remote().map(|remote| remote.destination()),
        Some("ssh://me@buildbox")
    );
    assert_eq!(
        app.controller.model().active_workspace(),
        Some(created.id())
    );
}

#[test]
fn disconnecting_asks_first_and_cancelling_keeps_the_connection() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    let connect = |app: &mut App| {
        app.action(
            &ctx,
            Action::Connect {
                workspace: None,
                destination: "me@devbox".into(),
            },
        );
        app.controller.model().active_workspace().unwrap()
    };
    let workspace = connect(&mut app);
    let pane = app.controller.model().active_pane().unwrap();
    let generation = app.controller.generation();

    app.action(&ctx, Action::Disconnect(workspace));
    assert_eq!(
        app.ui.overlay,
        OverlayState::ConfirmClose(Close::Connection(workspace))
    );
    app.action(&ctx, Action::CancelClose);
    assert_eq!(app.ui.overlay, OverlayState::None);
    assert_eq!(remote_of(&app, workspace), Some("me@devbox"));
    assert_eq!(app.controller.generation(), generation);
    assert_eq!(app.controller.model().pane(pane).unwrap().generation(), 1);

    app.action(&ctx, Action::Disconnect(workspace));
    app.action(&ctx, Action::Confirm(Close::Connection(workspace)));
    assert_eq!(app.ui.overlay, OverlayState::None);
    assert_eq!(remote_of(&app, workspace), None);
    assert_eq!(app.controller.model().pane(pane).unwrap().generation(), 2);
    assert_eq!(app.sessions.generation(pane), Some(2));
    assert_eq!(app.controller.model().workspaces().len(), 1);

    // Without confirmation the same action applies at once.
    let workspace = connect(&mut app);
    app.config.confirm_close = false;
    app.config.warn_running_processes = false;
    app.action(&ctx, Action::Disconnect(workspace));
    assert_eq!(app.ui.overlay, OverlayState::None);
    assert_eq!(remote_of(&app, workspace), None);
}

#[test]
fn a_connection_requested_during_restoration_waits_and_the_cli_host_opens_first() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.command = None;
    app.initial_remote = Some("me@devbox".into());
    app.ui.overlay = OverlayState::Ssh(None);
    app.action(
        &ctx,
        Action::Connect {
            workspace: None,
            destination: "buildbox".into(),
        },
    );
    assert_eq!(app.ui.overlay, OverlayState::None);
    assert!(app.controller.model().workspaces().is_empty());
    assert_eq!(app.sessions.usage().starting, 0);

    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    let workspaces = app.controller.model().workspaces();
    assert_eq!(workspaces.len(), 2);
    assert_eq!(workspaces[0].name(), "devbox");
    assert_eq!(workspaces[0].cwd(), root.path());
    assert_eq!(remote_of(&app, workspaces[0].id()), Some("me@devbox"));
    assert_eq!(workspaces[1].name(), "buildbox");
    assert_eq!(remote_of(&app, workspaces[1].id()), Some("buildbox"));
    assert!(app.initial_remote.is_none() && app.deferred_actions.is_empty());
}

#[test]
fn a_startup_command_is_never_typed_into_an_ssh_connection() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    add_workspace(&mut app, root.path(), Some("me@devbox"));
    let pane = app.controller.model().active_pane().unwrap();
    app.command_target = Some((pane, 1));
    app.send_startup_command();
    assert!(app.command.is_none());
    assert!(
        app.ui
            .error
            .as_deref()
            .unwrap()
            .contains("connected over SSH")
    );
}

#[test]
fn remote_workspaces_are_presented_by_host_instead_of_a_local_folder() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    add_workspace(&mut app, root.path(), None);
    add_workspace(&mut app, root.path(), Some("me@devbox"));
    let pane = app.controller.model().active_pane().unwrap();
    let views = app.views();
    assert_eq!(views[0].remote, None);
    assert_eq!(views[1].remote.as_deref(), Some("me@devbox"));
    let presentation = &app.presentations()[&pane];
    assert_eq!(presentation.location(), "me@devbox");
    assert_eq!(
        presentation.metadata.shell,
        "/nonexistent/neptune-startup-test-ssh"
    );
    assert!(presentation.starting);
}

#[cfg(unix)]
#[test]
fn a_remote_split_inherits_the_reported_host_directory_and_keeps_its_local_directory() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("local");
    std::fs::create_dir(&cwd).unwrap();
    // Stands in for the SSH client: records how it was started, then reports
    // a directory on the "host" the way a remote shell does.
    let client = root.path().join("fake-ssh");
    std::fs::write(
        &client,
        "#!/bin/sh\nwhile [ \"$1\" = -o ]; do shift 2; done\n[ \"$1\" = -t ] || exit 1\nprintf '%s %s %s\\n' \"$1\" \"$2\" \"$3\" >> \"$0.args\"\n\
         printf '%s\\0' \"$4\" >> \"$0.commands\"\npwd >> \"$0.cwd\"\n\
         printf '\\033]7;file://devbox/srv/on-the-host\\007'\nexec sleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();
    let lines = |suffix: &str| {
        std::fs::read_to_string(format!("{}.{suffix}", client.display())).unwrap_or_default()
    };

    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.command = None;
    app.ssh_client = client.to_string_lossy().into_owned();
    app.initial_cwd = Some(cwd.clone());
    app.initial_remote = Some("me@devbox".into());
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    let pane = app.controller.model().active_pane().unwrap();
    let generation = app.controller.generation();

    let deadline = Instant::now() + Duration::from_secs(10);
    let reported = std::path::Path::new("/srv/on-the-host");
    let mut seen = false;
    // The host's directory must stay separate from the local client directory.
    while !seen {
        app.poll(&ctx);
        seen = app
            .sessions
            .get(pane)
            .is_some_and(|session| session.metadata().cwd == reported);
        assert!(
            Instant::now() < deadline,
            "the client never reported its directory: {:?}",
            app.ui.error
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    app.poll(&ctx);
    assert_eq!(lines("args"), "-t -- me@devbox\n");
    assert_eq!(
        std::path::Path::new(lines("cwd").trim_end())
            .canonicalize()
            .unwrap(),
        cwd.canonicalize().unwrap()
    );
    assert_eq!(app.controller.model().pane(pane).unwrap().cwd(), cwd);
    assert!(app.controller.generation() > generation);
    assert_eq!(
        app.controller.model().pane(pane).unwrap().remote_cwd(),
        Some(reported)
    );

    // The SSH client's local process polling must not become the remote path.
    std::thread::sleep(Duration::from_millis(1200));
    app.poll(&ctx);
    assert_eq!(
        app.controller.model().pane(pane).unwrap().remote_cwd(),
        Some(reported)
    );
    // A split opens another connection from the same local directory while
    // passing the host's reported path in the remote bootstrap.
    app.action(&ctx, Action::Split(pane, neptune_model::Axis::Vertical));
    let second = app.controller.model().active_pane().unwrap();
    assert_ne!(second, pane);
    assert_eq!(app.controller.model().pane(second).unwrap().cwd(), cwd);
    // The argument header is written first. Wait for both complete bootstrap
    // records before checking the split's remote directory.
    while app.sessions.usage().running != 2
        || lines("args").lines().count() != 2
        || lines("commands").matches('\0').count() != 2
    {
        app.poll(&ctx);
        assert!(
            Instant::now() < deadline,
            "the split did not connect: {:?}",
            app.ui.error
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        lines("commands").contains("/srv/on-the-host"),
        "the split starts in the host's home directory instead of /srv/on-the-host: {:?}",
        lines("commands")
    );
}

#[cfg(unix)]
#[test]
fn remote_directories_survive_shutdown_and_restoration_per_terminal() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("local");
    let home = root.path().join("fake-ssh.home");
    let project = home.join("Documents/Projects/neptune ' % λ");
    for directory in [&local, &project] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(home.join(".zshenv"), "unsetopt GLOBAL_RCS\n").unwrap();
    std::fs::write(home.join(".zshrc"), "PROMPT='NEPTUNE> '\n").unwrap();
    // Keep the real remote bootstrap and PTY, with a test-owned SSH stand-in.
    let client = root.path().join("fake-ssh");
    std::fs::write(&client, "#!/bin/sh\nwhile [ \"$1\" = -o ]; do shift 2; done\n[ \"$1\" = -t ] || exit 1\nexport HOME=\"$0.home\" SHELL=zsh ZDOTDIR=\"$0.home\" TMPDIR=\"$0.home\"\ncd \"$HOME\"\nexec /bin/sh -c \"$4\"\n").unwrap();
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = egui::Context::default();
    let wait_for_directory = |app: &mut App, pane, directory: &std::path::Path| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.poll(&ctx);
            if app.sessions.get(pane).is_some_and(|session| {
                session.metadata().reported_cwd.as_deref() == Some(directory)
            }) {
                // Adopt metadata that might have arrived after this poll.
                app.poll(&ctx);
                return;
            }
            assert!(
                Instant::now() < deadline,
                "terminal did not restore directory {directory:?}: {:?}",
                app.ui.error
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    app.command = None;
    app.ssh_client = client.to_str().unwrap().into();
    app.initial_cwd = Some(local.clone());
    app.initial_remote = Some("devbox".into());
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    let first = app.controller.model().active_pane().unwrap();
    wait_for_directory(&mut app, first, &home);
    app.action(&ctx, Action::Split(first, neptune_model::Axis::Vertical));
    let second = app.controller.model().active_pane().unwrap();
    wait_for_directory(&mut app, second, &home);
    app.sessions
        .get(first)
        .unwrap()
        .write(b"cd Documents/Projects/neptune*\r")
        .unwrap();
    // The report arrives after the final frame. Shutdown still must save it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while app
        .sessions
        .get(first)
        .unwrap()
        .metadata()
        .reported_cwd
        .as_deref()
        != Some(project.as_path())
    {
        assert!(
            Instant::now() < deadline,
            "the remote shell did not change directory"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    eframe::App::on_exit(&mut app);

    let report = load_state(&app.state_path, Limits::default());
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let (mut reopened, _sender) = fixture(root.path());
    reopened.command = None;
    reopened.ssh_client = client.to_str().unwrap().into();
    reopened.complete_startup(
        &ctx,
        Startup {
            config: app.config.clone(),
            report,
            error: None,
        },
    );
    wait_for_directory(&mut reopened, first, &project);
    wait_for_directory(&mut reopened, second, &home);
    for pane in [first, second] {
        assert_eq!(reopened.controller.model().pane(pane).unwrap().cwd(), local);
    }
    eframe::App::on_exit(&mut reopened);
}

#[cfg(unix)]
#[test]
fn remote_splits_follow_their_source_pane_after_focus_and_directory_changes() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("local");
    let home = root.path().join("fake-ssh.home");
    let first = home.join("first project");
    let next = home.join("next ' % λ project");
    for directory in [&local, &first, &next] {
        std::fs::create_dir_all(directory).unwrap();
    }
    // Exercise runner-like completion paths without letting global compinit
    // block these test-owned shells with an interactive security prompt.
    let completions = home.join("insecure-completions");
    std::fs::create_dir(&completions).unwrap();
    std::fs::set_permissions(&completions, std::fs::Permissions::from_mode(0o777)).unwrap();
    std::fs::write(
        home.join(".zshenv"),
        "unsetopt GLOBAL_RCS\nfpath=(\"$ZDOTDIR/insecure-completions\" $fpath)\n",
    )
    .unwrap();
    std::fs::write(home.join(".zshrc"), "PROMPT='NEPTUNE> '\n").unwrap();
    let client = root.path().join("fake-ssh");
    std::fs::write(&client, "#!/bin/sh\nwhile [ \"$1\" = -o ]; do shift 2; done\n[ \"$1\" = -t ] || exit 1\nexport HOME=\"$0.home\" SHELL=zsh ZDOTDIR=\"$0.home\" TMPDIR=\"$0.home\"\ncd \"$HOME\"\nexec /bin/sh -c \"$4\"\n").unwrap();
    std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.command = None;
    app.ssh_client = client.to_str().unwrap().into();
    app.initial_cwd = Some(local.clone());
    app.initial_remote = Some("devbox".into());
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    let source = app.controller.model().active_pane().unwrap();
    let wait_for_directory = |app: &mut App, pane, directory: &std::path::Path| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.poll(&ctx);
            if app
                .sessions
                .get(pane)
                .is_some_and(|s| s.metadata().reported_cwd.as_deref() == Some(directory))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "pane did not enter its remote directory: {:?}",
                app.ui.error
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait_for_directory(&mut app, source, &home);
    app.sessions
        .get(source)
        .unwrap()
        .write(b"cd 'first project'\r")
        .unwrap();
    wait_for_directory(&mut app, source, &first);
    app.action(&ctx, Action::Split(source, neptune_model::Axis::Vertical));
    let right = app.controller.model().active_pane().unwrap();
    wait_for_directory(&mut app, right, &first);
    // The source is now unfocused, but its parser and directory hooks stay live.
    app.sessions
        .get(source)
        .unwrap()
        .write(b"cd ../next*\r")
        .unwrap();
    wait_for_directory(&mut app, source, &next);
    app.action(&ctx, Action::Split(source, neptune_model::Axis::Horizontal));
    let below = app.controller.model().active_pane().unwrap();
    wait_for_directory(&mut app, below, &next);
    assert_eq!(
        app.sessions
            .get(right)
            .unwrap()
            .metadata()
            .reported_cwd
            .as_deref(),
        Some(first.as_path())
    );
    for pane in [source, right, below] {
        assert_eq!(app.controller.model().pane(pane).unwrap().cwd(), local);
    }
}

#[test]
fn group_reordering_uses_the_captured_folder_and_keeps_active_sessions() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    for name in ["First", "Second", "Third"] {
        app.action(&ctx, Action::CreateGroup(name.into()));
    }
    let first = app.controller.model().groups()[0].id();
    let second = app.controller.model().groups()[1].id();
    app.controller
        .dispatch(Command::AddWorkspace {
            cwd: root.path().into(),
            name: "app".into(),
            remote: None,
            group: Some(second),
        })
        .unwrap();
    let before = app.controller.model().workspaces().to_owned();
    let active = app.controller.model().active_workspace();
    app.action(
        &ctx,
        Action::MoveSidebarItem(neptune_model::SidebarItem::Group(first), 3),
    );
    assert_eq!(
        app.controller
            .model()
            .groups()
            .iter()
            .map(|g| g.name())
            .collect::<Vec<_>>(),
        ["Second", "Third", "First"]
    );
    assert_eq!(app.controller.model().workspaces(), before);
    assert_eq!(app.controller.model().active_workspace(), active);
    assert!(app.ui.error.is_none());
    app.action(&ctx, Action::RemoveGroup(first));
    app.action(
        &ctx,
        Action::MoveSidebarItem(neptune_model::SidebarItem::Group(first), 1),
    );
    assert!(app.ui.error.as_ref().unwrap().contains("group"));
    assert_eq!(
        app.controller
            .model()
            .groups()
            .iter()
            .map(|g| g.name())
            .collect::<Vec<_>>(),
        ["Second", "Third"]
    );
}

#[test]
fn group_creation_and_ssh_dialog_keep_the_captured_group_target() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.startup = None;
    app.action(&ctx, Action::NewGroup);
    assert_eq!(app.ui.overlay, OverlayState::NewGroup);
    app.action(&ctx, Action::CreateGroup("Projects".into()));
    let group = app.controller.model().groups()[0].id();
    app.action(&ctx, Action::NewInGroup(group));
    let local = app.controller.model().active_workspace().unwrap();
    assert_eq!(
        app.controller.model().workspace(local).unwrap().group(),
        Some(group)
    );
    app.action(&ctx, Action::SshInGroup(group));
    assert_eq!(app.ui.overlay, OverlayState::SshInGroup(group));
    app.action(&ctx, Action::CreateGroup("Other".into()));
    app.action(&ctx, Action::ConnectInGroup(group, "me@host".into()));
    let remote = app.controller.model().workspaces().last().unwrap();
    assert_eq!(remote.group(), Some(group));
    assert_eq!(remote.remote().unwrap().destination(), "me@host");
    let remote_id = remote.id();
    app.action(&ctx, Action::SetGroupCollapsed(group, true));
    app.action(&ctx, Action::SelectWorkspace(local));
    assert!(!app.controller.model().group(group).unwrap().collapsed());
    let panes: Vec<_> = app
        .controller
        .model()
        .workspaces()
        .iter()
        .map(|w| (w.active(), w.panes()[0].generation()))
        .collect();
    app.action(&ctx, Action::RemoveGroup(group));
    assert_eq!(
        app.controller.model().workspace(remote_id).unwrap().group(),
        None
    );
    assert_eq!(
        panes,
        app.controller
            .model()
            .workspaces()
            .iter()
            .map(|w| (w.active(), w.panes()[0].generation()))
            .collect::<Vec<_>>()
    );
    // A queued creation must not silently fall back to another folder.
    let count = app.controller.model().workspaces().len();
    app.action(&ctx, Action::CreateInGroup(root.path().into(), group));
    assert_eq!(app.controller.model().workspaces().len(), count);
    assert!(app.ui.error.as_ref().unwrap().contains("group"));
}

#[test]
fn group_creation_waits_for_restoration_instead_of_being_replaced() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.action(&ctx, Action::CreateGroup("Queued".into()));
    assert!(app.controller.model().groups().is_empty());
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    assert_eq!(app.controller.model().groups()[0].name(), "Queued");
    assert!(app.deferred_actions.is_empty());
}

#[test]
fn custom_theme_preferences_survive_writer_shutdown_and_reopen() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    let ctx = egui::Context::default();
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    let mut configured = app.config.clone();
    configured
        .custom_themes
        .push(crate::terminal_theme::CustomTheme {
            id: "custom:1".into(),
            name: "Saved theme".into(),
            colors: crate::terminal_theme::bundled("Dracula").unwrap().colors,
        });
    configured.theme = config::Theme::Palette("custom:1".into());
    configured.favorite_themes = vec![configured.theme.clone(), config::Theme::Dusk];
    app.action(&ctx, Action::Preferences(configured.clone()));
    eframe::App::on_exit(&mut app);
    let saved = Config::load(&app.config_path).unwrap();
    assert_eq!(saved, configured);
    let (mut reopened, _sender) = fixture(root.path());
    reopened.ephemeral = false;
    reopened.complete_startup(&ctx, loaded(saved, Model::default()));
    assert_eq!(
        Palette::for_config(&reopened.config).bg,
        crate::theme::color(0x282a36)
    );
    let mut removed = reopened.config.clone();
    removed.custom_themes.clear();
    removed.theme = config::Theme::Graphite;
    reopened.action(&ctx, Action::Preferences(removed));
    eframe::App::on_exit(&mut reopened);
    let saved = Config::load(&reopened.config_path).unwrap();
    assert!(saved.custom_themes.is_empty());
    assert_eq!(saved.theme, config::Theme::Graphite);
    // The deleted theme took its star with it.
    assert_eq!(saved.favorite_themes, [config::Theme::Dusk]);
}

#[test]
fn theme_browser_escape_returns_to_preferences_before_closing_overlay() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.ui.overlay = OverlayState::Settings;
    app.ui.preferences.open = true;
    let ctx = egui::Context::default();
    assert!(press(
        &mut app,
        &ctx,
        key(egui::Key::Escape, None, egui::Modifiers::NONE)
    ));
    assert_eq!(app.ui.overlay, OverlayState::Settings);
    assert!(!app.ui.preferences.open);
    press(
        &mut app,
        &ctx,
        key(egui::Key::Escape, None, egui::Modifiers::NONE),
    );
    assert_eq!(app.ui.overlay, OverlayState::None);
}

#[test]
fn escape_leaves_a_settings_search_before_closing_preferences() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.ui.overlay = OverlayState::Settings;
    app.ui.preference_view = ui::preferences::View::searching("font");
    let ctx = egui::Context::default();
    press(
        &mut app,
        &ctx,
        key(egui::Key::Escape, None, egui::Modifiers::NONE),
    );
    assert_eq!(app.ui.overlay, OverlayState::Settings);
    press(
        &mut app,
        &ctx,
        key(egui::Key::Escape, None, egui::Modifiers::NONE),
    );
    assert_eq!(app.ui.overlay, OverlayState::None);
}

#[test]
fn theme_color_popup_escape_keeps_preferences_open() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.ui.overlay = OverlayState::Settings;
    app.ui.preferences.open = true;
    let ctx = egui::Context::default();
    egui::Popup::open_id(&ctx, egui::Id::new("test-color-picker"));
    assert!(press(
        &mut app,
        &ctx,
        key(egui::Key::Escape, None, egui::Modifiers::NONE)
    ));
    assert!(!egui::Popup::is_any_open(&ctx));
    assert_eq!(app.ui.overlay, OverlayState::Settings);
    assert!(app.ui.preferences.open);
}

#[test]
fn a_changed_theme_draft_survives_closing_and_other_overlays() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    let ctx = egui::Context::default();
    app.ui.overlay = OverlayState::Settings;
    app.ui.preferences = ui::theme_browser::State::with_changed_draft(&app.config);
    // The preferences shortcut asks about the draft instead of closing.
    app.action(&ctx, Action::Settings);
    assert_eq!(app.ui.overlay, OverlayState::Settings);
    assert!(app.ui.preferences.editing());
    // Escape answers with "keep editing"; it never discards by itself.
    for _ in 0..3 {
        press(
            &mut app,
            &ctx,
            key(egui::Key::Escape, None, egui::Modifiers::NONE),
        );
        assert_eq!(app.ui.overlay, OverlayState::Settings);
        assert!(app.ui.preferences.editing());
    }
    // Another overlay sets the draft aside; both ways back return to it.
    app.action(&ctx, Action::Palette);
    assert_eq!(app.ui.overlay, OverlayState::Palette);
    app.action(&ctx, Action::Settings);
    assert_eq!(app.ui.overlay, OverlayState::Settings);
    assert!(app.ui.preferences.editing());
    app.action(&ctx, Action::Palette);
    app.action(&ctx, Action::Themes);
    assert_eq!(app.ui.overlay, OverlayState::Settings);
    assert!(app.ui.preferences.editing());
}

#[test]
fn preferences_reopen_on_the_general_settings_and_themes_on_the_catalog() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    let ctx = egui::Context::default();
    app.action(&ctx, Action::Themes);
    assert_eq!(app.ui.overlay, OverlayState::Settings);
    assert!(app.ui.preferences.open);
    assert_eq!(
        app.ui.preference_view.pane,
        ui::preferences::Pane::Appearance
    );
    // Without a draft the shortcut closes, and the next opening starts over.
    app.action(&ctx, Action::Settings);
    assert_eq!(app.ui.overlay, OverlayState::None);
    app.action(&ctx, Action::Settings);
    assert_eq!(app.ui.overlay, OverlayState::Settings);
    assert!(!app.ui.preferences.open);
}

#[test]
fn notifications_navigate_to_the_captured_pane_and_ignore_restarted_sessions() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = fixture(root.path());
    app.startup = None;
    let first = add_workspace(&mut app, root.path(), None);
    let pane = app.controller.model().workspace(first).unwrap().active();
    let generation = app.controller.model().pane(pane).unwrap().generation();
    app.controller
        .dispatch(Command::SplitPane {
            workspace: first,
            pane,
            axis: neptune_model::Axis::Vertical,
            cwd: root.path().into(),
        })
        .unwrap();
    let sibling = app.controller.model().workspace(first).unwrap().active();
    assert_ne!(sibling, pane);
    let second = add_workspace(&mut app, root.path(), None);
    let ctx = egui::Context::default();
    for (target, title) in [(pane, "Review"), (sibling, "Build complete")] {
        app.notifications.push(
            target,
            app.controller.model().pane(target).unwrap().generation(),
            terminal_core::Notification {
                title: title.into(),
                ..Default::default()
            },
        );
    }
    let view = app.views().into_iter().find(|w| w.id == first).unwrap();
    assert_eq!(view.unread, 2);
    assert_eq!(view.alert.as_deref(), Some("Build complete"));
    app.ui.overlay = OverlayState::Notifications;
    app.ui.zoomed = true;
    app.action(&ctx, Action::OpenNotification(pane, generation));
    assert_eq!(app.controller.model().active_pane(), Some(pane));
    // Its workspace's other pane was never focused, so it stays unread.
    assert_eq!(app.notifications.unread(Some(pane)), 0);
    assert_eq!(app.notifications.unread(Some(sibling)), 1);
    assert_eq!(app.ui.overlay, OverlayState::None);
    assert!(!app.ui.zoomed);
    app.controller.dispatch(Command::RestartPane(pane)).unwrap();
    app.controller
        .dispatch(Command::SelectWorkspace(second))
        .unwrap();
    app.action(&ctx, Action::OpenNotification(pane, generation));
    assert_eq!(app.controller.model().active_workspace(), Some(second));
    // Only the restarted session's alert is discarded.
    app.notifications.retain_sessions(app.controller.model());
    assert!(
        app.notifications
            .entries()
            .all(|entry| entry.pane == sibling)
    );
    assert_eq!(app.notifications.entries().count(), 1);
}

#[test]
fn update_channel_preferences_are_saved_and_stale_buttons_cannot_download() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    let ctx = egui::Context::default();
    app.complete_startup(&ctx, loaded(app.config.clone(), Model::default()));
    let config = Config {
        release_channel: crate::runtime::updates::ReleaseChannel::Beta,
        check_updates: false,
        ..app.config.clone()
    };
    app.action(&ctx, Action::Preferences(config));
    app.action(&ctx, Action::DownloadUpdate("0.2.0".into()));
    assert!(!app.updates.busy());
    assert_eq!(
        app.config.release_channel,
        crate::runtime::updates::ReleaseChannel::Beta
    );
    eframe::App::on_exit(&mut app);
    let saved = Config::load(&app.config_path).unwrap();
    assert_eq!(
        saved.release_channel,
        crate::runtime::updates::ReleaseChannel::Beta
    );
    assert!(!saved.check_updates);
    app.action(&ctx, Action::CloseOverlay);
    assert_eq!(app.ui.overlay, OverlayState::None);
}

/// A real Neptune window using authenticated, deterministic test release data.
/// Deliberately ignored in headless CI; no fixture switch exists in production.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "Manual native visual QA; needs a desktop and NEPTUNE_UPDATE_CAPTURE"]
fn capture_update_native() {
    use winit::platform::x11::EventLoopBuilderExtX11;
    let output = PathBuf::from(
        std::env::var("NEPTUNE_UPDATE_CAPTURE").expect("Set a task-owned capture path"),
    );
    let screen = std::env::var("NEPTUNE_UPDATE_SCREEN").unwrap_or_else(|_| "update".into());
    let narrow = std::env::var_os("NEPTUNE_UPDATE_NARROW").is_some();
    let data = tempfile::tempdir().unwrap();
    let data_path = data.path().to_path_buf();
    let size = if narrow {
        [640.0, 400.0]
    } else {
        [900.0, 640.0]
    };
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_decorations(false),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    struct NativeCapture {
        app: App,
        overlay: OverlayState,
        applied: bool,
        size: [f32; 2],
    }
    impl eframe::App for NativeCapture {
        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            eframe::App::logic(&mut self.app, ctx, frame);
        }
        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            if self.app.startup.is_none() && !self.applied {
                self.app.ui.overlay = self.overlay;
                self.applied = true;
            }
            eframe::App::ui(&mut self.app, ui, frame);
        }
        fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
            if self.applied
                && self.overlay == OverlayState::Settings
                && self.app.started.elapsed() > Duration::from_millis(1500)
                && self.app.started.elapsed() < Duration::from_millis(2500)
            {
                input.events.push(egui::Event::PointerMoved(egui::pos2(
                    self.size[0] * 0.5,
                    self.size[1] * 0.5,
                )));
                input.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: egui::vec2(0.0, -400.0),
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
        fn on_exit(&mut self) {
            eframe::App::on_exit(&mut self.app);
        }
    }
    eframe::run_native(
        "Neptune update visual QA",
        options,
        Box::new(move |cc| {
            let mut app = App::new(
                cc,
                Launch {
                    data_root: Some(data_path),
                    screenshot: Some(output),
                    ..Default::default()
                },
                window_state::LoadReport::default(),
            );
            if screen == "preferences-update" {
                app.updates.release = Some(crate::runtime::updates::tests::visual_release());
                app.updates.status = crate::runtime::updates::UpdateStatus::Available;
            }
            if screen == "preferences-installed" {
                crate::runtime::updates::tests::installed(&mut app.updates);
            }
            if screen.starts_with("preferences") {
                use ui::preferences::Pane;
                if let Ok(query) = std::env::var("NEPTUNE_PREFERENCES_SEARCH") {
                    app.ui.preference_view = ui::preferences::View::searching(&query);
                }
                let pane = std::env::var("NEPTUNE_PREFERENCES_PANE").unwrap_or_default();
                app.ui.preference_view.pane = Pane::ALL
                    .into_iter()
                    .find(|entry| entry.title().eq_ignore_ascii_case(&pane))
                    .unwrap_or(Pane::Updates);
            } else {
                let mut release = crate::runtime::updates::tests::visual_release();
                if let Some(notes) = std::env::var_os("NEPTUNE_UPDATE_NOTES") {
                    release.notes = std::fs::read_to_string(notes).unwrap();
                }
                app.updates.release = Some(release);
                use crate::runtime::updates::UpdateStatus;
                use crate::runtime::updates::tests::prepare;
                match screen.as_str() {
                    "ready" => prepare(&mut app.updates, false),
                    "restart" | "installed" => prepare(&mut app.updates, true),
                    _ => {}
                }
                app.updates.status = match screen.as_str() {
                    "ready" | "restart" => UpdateStatus::Ready,
                    "installed" => UpdateStatus::Installed,
                    "error" => UpdateStatus::Error("Could not complete the update. Check your connection and try again. Unverified downloads are never opened.".into()),
                    _ => UpdateStatus::Available,
                };
            }
            let overlay = match screen.as_str() {
                "preferences" | "preferences-update" | "preferences-installed" => {
                    OverlayState::Settings
                }
                "notification" => OverlayState::None,
                _ => OverlayState::Update,
            };
            Ok(Box::new(NativeCapture {
                app,
                overlay,
                applied: false,
                size,
            }))
        }),
    )
    .unwrap();
}

/// A real Neptune window with files held over it, as another application's
/// drag would leave them. Ignored in headless CI.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "Manual native visual QA; needs a desktop and NEPTUNE_DROP_CAPTURE"]
fn capture_file_drop_native() {
    use winit::platform::x11::EventLoopBuilderExtX11;
    let output = PathBuf::from(
        std::env::var("NEPTUNE_DROP_CAPTURE").expect("Set a task-owned capture path"),
    );
    let split = std::env::var_os("NEPTUNE_DROP_SPLIT").is_some();
    let agent = std::env::var_os("NEPTUNE_DROP_AGENT").is_some();
    let size = if std::env::var_os("NEPTUNE_DROP_NARROW").is_some() {
        [640.0, 400.0]
    } else {
        [900.0, 640.0]
    };
    let data = tempfile::tempdir().unwrap();
    let data_path = data.path().to_path_buf();
    std::fs::write(data_path.join("config.toml"), "shell = \"/bin/sh\"\n").unwrap();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_decorations(false),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    struct NativeCapture {
        app: App,
        split: bool,
        agent: bool,
        applied: bool,
    }
    impl eframe::App for NativeCapture {
        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            eframe::App::logic(&mut self.app, ctx, frame);
            let Some(pane) = self.app.controller.model().active_pane() else {
                return;
            };
            if self.applied || self.app.sessions.get(pane).is_none() {
                return;
            }
            self.applied = true;
            if self.split {
                self.app
                    .action(ctx, Action::Split(pane, neptune_model::Axis::Vertical));
            }
            if self.agent {
                let generation = self.app.sessions.generation(pane).unwrap();
                self.app.dispatch(
                    ctx,
                    Command::PaneAgentChanged {
                        pane,
                        generation,
                        agent: Some(neptune_model::AgentSession {
                            kind: neptune_model::AgentKind::Claude,
                            session_id: None,
                            cwd: std::env::temp_dir(),
                        }),
                    },
                );
                self.app.action(ctx, Action::Focus(pane));
            }
        }
        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            eframe::App::ui(&mut self.app, ui, frame);
        }
        fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
            if self.applied {
                input.hovered_files.push(egui::HoveredFile {
                    path: Some("/tmp/Screenshot.png".into()),
                    ..Default::default()
                });
            }
        }
        fn on_exit(&mut self) {
            eframe::App::on_exit(&mut self.app);
        }
    }
    eframe::run_native(
        "Neptune file drop visual QA",
        options,
        Box::new(move |cc| {
            let mut app = App::new(
                cc,
                Launch {
                    data_root: Some(data_path),
                    screenshot: Some(output),
                    ..Default::default()
                },
                window_state::LoadReport::default(),
            );
            // The capture shows the focused terminal, not wherever the
            // desktop's pointer happens to be.
            app.file_drag = crate::platform::file_drag::FileDragSource::detached();
            Ok(Box::new(NativeCapture {
                app,
                split,
                agent,
                applied: false,
            }))
        }),
    )
    .unwrap();
}

/// A real Neptune window with the pointer resting on a picture's path in
/// terminal output, as after a CLI agent reports the file it wrote. Ignored in
/// headless CI.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "Manual native visual QA; needs a desktop and NEPTUNE_PREVIEW_CAPTURE"]
fn capture_image_preview_native() {
    use winit::platform::x11::EventLoopBuilderExtX11;
    let output = PathBuf::from(
        std::env::var("NEPTUNE_PREVIEW_CAPTURE").expect("Set a task-owned capture path"),
    );
    let bottom = std::env::var_os("NEPTUNE_PREVIEW_BOTTOM").is_some();
    let zoom = if std::env::var_os("NEPTUNE_PREVIEW_ZOOM").is_some() {
        4
    } else {
        0
    };
    let open = zoom > 0 || std::env::var_os("NEPTUNE_PREVIEW_OPEN").is_some();
    let size = if std::env::var_os("NEPTUNE_PREVIEW_NARROW").is_some() {
        [640.0, 400.0]
    } else {
        [900.0, 640.0]
    };
    let data = tempfile::tempdir().unwrap();
    let data_path = data.path().to_path_buf();
    std::fs::write(data_path.join("config.toml"), "shell = \"/bin/sh\"\n").unwrap();
    // The path is printed relative to the terminal's directory unless a
    // picture of the reviewer's own is named.
    let project = data_path.join("project");
    std::fs::create_dir_all(project.join("out")).unwrap();
    let printed = match std::env::var("NEPTUNE_PREVIEW_IMAGE") {
        Ok(path) => path,
        Err(_) => {
            let bars = [0.35, 0.5, 0.42, 0.68, 0.8, 0.74, 0.93];
            image::RgbImage::from_fn(960, 600, |x, y| {
                let column = (x as usize).saturating_sub(60) / 120;
                let inside = x >= 60 && (x - 60) % 120 < 84 && column < bars.len();
                if inside && (540 - y.min(540)) as f32 <= bars[column] * 460.0 && y < 540 {
                    image::Rgb([64, 120, 242])
                } else if (540..543).contains(&y) {
                    image::Rgb([150, 154, 164])
                } else {
                    image::Rgb([247, 248, 250])
                }
            })
            .save(project.join("out/revenue-chart.png"))
            .unwrap();
            "out/revenue-chart.png".into()
        }
    };
    let name = std::path::Path::new(&printed)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let lead = if bottom {
        "i=0; while [ $i -lt 60 ]; do echo; i=$((i+1)); done; "
    } else {
        ""
    };
    let command = format!(
        " clear; {lead}printf '\\n  Wrote the chart to %s and updated the report.\\n\\n' '{printed}'"
    );
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_decorations(false),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    struct NativeCapture {
        app: App,
        name: String,
        /// Click the card for the full view once it is showing.
        open: bool,
        steps: u8,
        /// Zoom steps still to take in the full view.
        zoom: u8,
    }
    impl NativeCapture {
        /// The middle of the printed file name, once the shell has shown it.
        fn pointer(&self, ctx: &egui::Context) -> Option<egui::Pos2> {
            let pane = self.app.controller.model().active_pane()?;
            let body = ctx.read_response(self.app.terminal_focus?)?.rect;
            let cell = self.app.renders.get(&pane)?.cache.cell;
            let snapshot = self.app.sessions.get(pane)?.viewport();
            snapshot.rows.iter().enumerate().rev().find_map(|(y, row)| {
                let text: String = row.iter().map(|cell| cell.c).collect();
                let x = text.find(&self.name).filter(|_| !text.contains("printf"))?;
                Some(body.min + egui::vec2((x as f32 + 4.5) * cell.x, (y as f32 + 0.5) * cell.y))
            })
        }
    }
    impl eframe::App for NativeCapture {
        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            eframe::App::logic(&mut self.app, ctx, frame);
        }
        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            eframe::App::ui(&mut self.app, ui, frame);
        }
        fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
            eframe::App::raw_input_hook(&mut self.app, ctx, input);
            // Sent every frame, after whatever the desktop's pointer did.
            if let Some(card) = self.app.image_preview.card.filter(|_| self.open) {
                // Travel onto the card, settle, then click it.
                let pos = card.center();
                self.steps += 1;
                input.events.push(match self.steps {
                    3 | 4 => egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.steps == 3,
                        modifiers: egui::Modifiers::NONE,
                    },
                    _ => egui::Event::PointerMoved(pos),
                });
            } else if self.zoom > 0 && self.app.ui.overlay == OverlayState::Image {
                // Zoom the full view in with the keyboard, a step a frame.
                self.zoom -= 1;
                input.events.push(egui::Event::Key {
                    key: egui::Key::Plus,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            } else if let Some(pos) = self.pointer(ctx) {
                input.events.push(egui::Event::PointerMoved(pos));
            }
        }
        fn on_exit(&mut self) {
            eframe::App::on_exit(&mut self.app);
        }
    }
    eframe::run_native(
        "Neptune image preview visual QA",
        options,
        Box::new(move |cc| {
            let mut app = App::new(
                cc,
                Launch {
                    cwd: Some(project),
                    data_root: Some(data_path),
                    command: Some(command),
                    screenshot: Some(output),
                    ..Default::default()
                },
                window_state::LoadReport::default(),
            );
            app.file_drag = crate::platform::file_drag::FileDragSource::detached();
            Ok(Box::new(NativeCapture {
                app,
                name,
                open,
                steps: 0,
                zoom,
            }))
        }),
    )
    .unwrap();
}

/// A real Neptune window with the file explorer open on a small project.
/// Ignored in headless CI.
///
/// `NEPTUNE_EXPLORER_CAPTURE` names the picture to write. `NEPTUNE_EXPLORER_STATE`
/// picks what is shown: the tree by default, or `preview`, `image`, `binary`,
/// `search`, `search-all` (no exclusions), `filters`, `menu`, `create`,
/// `rename`, `rename-taken`, `delete`, `remote` or `sliding` (midway through
/// the toggle: `NEPTUNE_EXPLORER_SLIDE_MS` after it began; 100 by default).
/// `agents` shows the agents tab with an agent in each state across three
/// workspaces, `agents-empty` shows it with none, and `agents-closed` closes
/// the panel on them, leaving the toolbar's mark for an agent that waits.
/// `NEPTUNE_EXPLORER_NARROW=1` uses a 640×400 window,
/// `NEPTUNE_EXPLORER_SIDEBAR=closed` hides the sidebar and
/// `NEPTUNE_EXPLORER_THEME` names a theme. The desktop's pointer and keyboard
/// are kept out of it; a real secondary click, typing and dragging the
/// dividers need a hand-driven native check.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "Manual native visual QA; needs a desktop and NEPTUNE_EXPLORER_CAPTURE"]
fn capture_explorer_native() {
    use ui::explorer::Event;
    use winit::platform::x11::EventLoopBuilderExtX11;
    let output = PathBuf::from(
        std::env::var("NEPTUNE_EXPLORER_CAPTURE").expect("Set a task-owned capture path"),
    );
    let state = std::env::var("NEPTUNE_EXPLORER_STATE").unwrap_or_default();
    let size = if std::env::var_os("NEPTUNE_EXPLORER_NARROW").is_some() {
        [640.0, 400.0]
    } else {
        [1000.0, 680.0]
    };
    let data = tempfile::tempdir().unwrap();
    let data_path = data.path().to_path_buf();
    let theme = std::env::var("NEPTUNE_EXPLORER_THEME").unwrap_or_else(|_| "graphite".into());
    std::fs::write(
        data_path.join("config.toml"),
        format!("shell = \"/bin/sh\"\ntheme = \"{theme}\"\n"),
    )
    .unwrap();
    let project = data_path.join("orbit");
    for (path, text) in [
        (".env", "API_URL=http://localhost:8080\n"),
        (".gitignore", "target\nnode_modules\n"),
        (".git/HEAD", "ref: refs/heads/main\n"),
        (
            "Cargo.toml",
            "[package]\nname = \"orbit\"\nversion = \"0.1.0\"\n",
        ),
        ("README.md", "# Orbit\n\nA small service.\n"),
        (
            "src/main.rs",
            "use std::net::SocketAddr;\n\nmod routes;\n\n/// Starts the service on the configured address.\nfn main() {\n\tlet address: SocketAddr = \"127.0.0.1:8080\".parse().unwrap();\n\tprintln!(\"listening on {address}\");\n\troutes::serve(address);\n}\n",
        ),
        (
            "src/routes.rs",
            "pub fn serve(_address: std::net::SocketAddr) {}\n",
        ),
        ("src/main_test.rs", "#[test]\nfn starts() {}\n"),
        (
            "node_modules/left-pad/main.js",
            "module.exports = () => {};\n",
        ),
        ("target/debug/main", "\0"),
        ("docs/guide.md", "# Guide\n"),
    ] {
        let path = project.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    std::fs::create_dir_all(project.join("assets")).unwrap();
    let bars = [0.35, 0.5, 0.42, 0.68, 0.8, 0.74, 0.93];
    image::RgbImage::from_fn(960, 600, |x, y| {
        let column = (x as usize).saturating_sub(60) / 120;
        let inside = x >= 60 && (x - 60) % 120 < 84 && column < bars.len();
        if inside && (540 - y.min(540)) as f32 <= bars[column] * 460.0 && y < 540 {
            image::Rgb([64, 120, 242])
        } else if (540..543).contains(&y) {
            image::Rgb([150, 154, 164])
        } else {
            image::Rgb([247, 248, 250])
        }
    })
    .save(project.join("assets/revenue-chart.png"))
    .unwrap();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_decorations(false),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    struct NativeCapture {
        app: App,
        project: PathBuf,
        state: String,
        applied: bool,
        /// The state under review has been set up.
        staged: bool,
        steps: u8,
        slid: bool,
        /// Milliseconds the slide has run when the capture is taken.
        lead: u64,
        /// The terminals that stand in for agents, and whether each is set up.
        fleet: Vec<(PaneId, bool)>,
    }
    impl NativeCapture {
        fn act(&mut self, ctx: &egui::Context, event: Event) {
            self.app.action(ctx, Action::Explorer(event));
        }
        /// Three workspaces whose terminals report as agents in every state.
        fn stage_agents(&mut self, ctx: &egui::Context) {
            use crate::agent_activity::{Activity, Attention};
            use neptune_model::{AgentKind, AgentSession};
            if self.fleet.is_empty() {
                let first = self.app.controller.model().active_pane().unwrap();
                let home = self.app.controller.model().active_workspace().unwrap();
                self.app
                    .action(ctx, Action::Split(first, neptune_model::Axis::Vertical));
                for name in ["website", "billing-api"] {
                    let cwd = self.project.join(name);
                    std::fs::create_dir_all(&cwd).unwrap();
                    self.app.dispatch(
                        ctx,
                        Command::AddWorkspace {
                            cwd,
                            name: name.into(),
                            remote: None,
                            group: None,
                        },
                    );
                }
                self.app.dispatch(ctx, Command::SelectWorkspace(home));
                self.app.action(ctx, Action::Focus(first));
                self.fleet = self
                    .app
                    .controller
                    .model()
                    .workspaces()
                    .iter()
                    .flat_map(|workspace| workspace.panes().iter().map(|pane| (pane.id(), false)))
                    .collect();
            }
            let agents = [
                (
                    AgentKind::Claude,
                    "\u{25d0} Refactor the retry queue",
                    Activity::Working,
                    200,
                ),
                (
                    AgentKind::Codex,
                    "orbit",
                    Activity::NeedsInput(Attention::Permission),
                    45,
                ),
                (
                    AgentKind::Claude,
                    "\u{2733} Landing page copy and pricing table",
                    Activity::NeedsInput(Attention::Question),
                    700,
                ),
                (AgentKind::Codex, "billing-api", Activity::Idle, 4000),
            ];
            for (index, (pane, done)) in self.fleet.clone().into_iter().enumerate() {
                let Some((kind, title, activity, age)) = agents.get(index).copied() else {
                    continue;
                };
                let Some(session) = self.app.sessions.get(pane).filter(|_| !done) else {
                    continue;
                };
                session
                    .write(format!(" printf '\\033]0;{title}\\007'; clear\n").as_bytes())
                    .unwrap();
                let generation = self.app.sessions.generation(pane).unwrap();
                let cwd = self.app.controller.model().pane(pane).unwrap().cwd().into();
                self.app.dispatch(
                    ctx,
                    Command::PaneAgentChanged {
                        pane,
                        generation,
                        agent: Some(AgentSession {
                            kind,
                            session_id: None,
                            cwd,
                        }),
                    },
                );
                self.app.agents.report(
                    pane,
                    generation,
                    Some(activity),
                    Instant::now() - Duration::from_secs(age),
                );
                // The title printed above is the agent's, not its shell's.
                self.app.agents.title(pane, kind, "", Instant::now());
                self.fleet[index].1 = true;
            }
        }
    }
    impl eframe::App for NativeCapture {
        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            eframe::App::logic(&mut self.app, ctx, frame);
            let Some(pane) = self.app.controller.model().active_pane() else {
                return;
            };
            // The slide is caught midway by the capture three seconds in.
            if self.state == "sliding"
                && !self.slid
                && self.app.started.elapsed() > Duration::from_millis(3000 - self.lead)
            {
                self.slid = true;
                self.app
                    .action(ctx, Action::Panel(ui::panel::Event::Toggle));
            }
            if self.app.sessions.get(pane).is_none() {
                return;
            }
            let project = self.project.clone();
            if !self.applied {
                self.applied = true;
                if std::env::var("NEPTUNE_EXPLORER_SIDEBAR").is_ok_and(|side| side == "closed") {
                    self.app.action(ctx, Action::ToggleSidebar);
                    self.app.ui.sidebar_slide = None;
                }
                if self.state != "sliding" {
                    self.app
                        .action(ctx, Action::Panel(ui::panel::Event::Toggle));
                    // Shown at rest, not on its way in.
                    self.app.ui.panel.slide = None;
                }
                self.act(ctx, Event::Expand(project.join("src"), true));
                self.app.ui.explorer.selected = None;
            }
            if self.staged && matches!(self.state.as_str(), "agents" | "agents-closed") {
                self.stage_agents(ctx);
            }
            // The rest is asked of a panel that is showing the folder.
            if self.staged || self.app.explorer_rows() < 3 {
                return;
            }
            self.staged = true;
            match self.state.as_str() {
                "agents" | "agents-empty" => self.app.action(
                    ctx,
                    Action::Panel(ui::panel::Event::Show(ui::panel::Tab::Agents)),
                ),
                "agents-closed" => {
                    self.app
                        .action(ctx, Action::Panel(ui::panel::Event::Toggle));
                    self.app.ui.panel.slide = None;
                }
                "preview" => self.act(ctx, Event::Select(project.join("src/main.rs"))),
                "image" => {
                    self.act(ctx, Event::Expand(project.join("assets"), true));
                    self.act(ctx, Event::Select(project.join("assets/revenue-chart.png")));
                }
                "binary" => self.act(ctx, Event::Select(project.join("target/debug/main"))),
                "search" | "search-all" => {
                    self.app.ui.explorer.query = "main".into();
                    if self.state == "search-all" {
                        self.app.ui.explorer.exclude.clear();
                    }
                    self.act(ctx, Event::SearchChanged);
                }
                "filters" => self.app.ui.explorer.filters = true,
                "create" => self.act(
                    ctx,
                    Event::BeginCreate {
                        parent: Some(project.join("src")),
                        folder: false,
                    },
                ),
                "rename" => self.act(ctx, Event::BeginRename(project.join("src/routes.rs"))),
                "rename-taken" => {
                    self.act(ctx, Event::BeginRename(project.join("src/routes.rs")));
                    self.app.ui.explorer.edit.as_mut().unwrap().text = "main.rs".into();
                }
                "delete" => self.act(ctx, Event::Delete(project.join("docs"))),
                "remote" => self.app.action(
                    ctx,
                    Action::Connect {
                        workspace: None,
                        destination: "deploy@example.com".into(),
                    },
                ),
                _ => {}
            }
        }
        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            eframe::App::ui(&mut self.app, ui, frame);
            // A name that is taken is refused once the folder has been read.
            if self.state == "rename-taken"
                && self.staged
                && self.steps == 0
                && self.app.started.elapsed() > Duration::from_millis(1500)
            {
                self.steps = 1;
                self.act(ui.ctx(), Event::Commit { explicit: true });
            }
        }
        fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
            eframe::App::raw_input_hook(&mut self.app, ctx, input);
            // The desktop's own pointer and keyboard have no part in the capture.
            input.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::PointerMoved(_)
                        | egui::Event::MouseMoved(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::PointerGone
                        | egui::Event::MouseWheel { .. }
                )
            });
            if self.state != "menu" || !self.staged {
                return;
            }
            // A secondary click on the third row, a step a frame.
            let window = ctx.content_rect();
            let pos = egui::pos2(
                window.right() - self.app.ui.panel.width + 90.0,
                44.0 + ui::panel::TABS + 68.0 + 26.0 * 2.5,
            );
            self.steps = self.steps.saturating_add(1);
            input.events.push(match self.steps {
                3 | 4 => egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Secondary,
                    pressed: self.steps == 3,
                    modifiers: egui::Modifiers::NONE,
                },
                _ if self.steps < 3 => egui::Event::PointerMoved(pos),
                _ => egui::Event::PointerMoved(pos + egui::vec2(60.0, 150.0)),
            });
        }
        fn on_exit(&mut self) {
            eframe::App::on_exit(&mut self.app);
        }
    }
    let root = project.clone();
    eframe::run_native(
        "Neptune file explorer visual QA",
        options,
        Box::new(move |cc| {
            let mut app = App::new(
                cc,
                Launch {
                    cwd: Some(project),
                    data_root: Some(data_path),
                    screenshot: Some(output),
                    ..Default::default()
                },
                window_state::LoadReport::default(),
            );
            app.file_drag = crate::platform::file_drag::FileDragSource::detached();
            Ok(Box::new(NativeCapture {
                app,
                project: root,
                state,
                applied: false,
                staged: false,
                steps: 0,
                slid: false,
                fleet: Vec::new(),
                lead: std::env::var("NEPTUNE_EXPLORER_SLIDE_MS")
                    .ok()
                    .and_then(|lead| lead.parse().ok())
                    .unwrap_or(100),
            }))
        }),
    )
    .unwrap();
}

/// Real GPU/native captures of the hint overlay, file-location preview and
/// Preferences picker, in isolated storage. Input injection here proves app
/// routing; actual OS key/pointer/clipboard integration is checked separately.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "Native visual QA; needs a desktop and NEPTUNE_HINT_CAPTURE"]
fn capture_file_locations_native() {
    use winit::platform::x11::EventLoopBuilderExtX11;
    let output = PathBuf::from(
        std::env::var("NEPTUNE_HINT_CAPTURE").expect("Set a task-owned capture path"),
    );
    let screen = std::env::var("NEPTUNE_HINT_SCREEN").unwrap_or_else(|_| "hints".into());
    let narrow = std::env::var_os("NEPTUNE_HINT_NARROW").is_some();
    let split = std::env::var_os("NEPTUNE_HINT_SPLIT").is_some();
    let data = tempfile::tempdir().unwrap();
    let data_path = data.path().to_path_buf();
    let project = data_path.join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(
        project.join("src/foo.rs"),
        (1..=100)
            .map(|i| {
                if i == 42 {
                    "fn main() { println!(\"Hello, Neptune\"); }\n".into()
                } else {
                    format!("// source line {i}\n")
                }
            })
            .collect::<String>(),
    )
    .unwrap();
    std::fs::write(
        data_path.join("config.toml"),
        "shell = \"/bin/sh\"\ncheck_updates = false\n",
    )
    .unwrap();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(if narrow {
                [640.0, 400.0]
            } else {
                [1000.0, 640.0]
            })
            .with_decorations(false),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    struct Capture {
        app: App,
        output: Option<PathBuf>,
        screen: String,
        applied: bool,
        ready_at: Option<Instant>,
        click: u8,
        split: bool,
        split_created: bool,
    }
    impl eframe::App for Capture {
        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            eframe::App::logic(&mut self.app, ctx, frame);
        }
        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            let before = self.app.hints.map(|(pane, _)| {
                let cache = &self.app.renders[&pane].cache;
                (pane, cache.hinting(), cache.columns, cache.lines)
            });
            eframe::App::ui(&mut self.app, ui, frame);
            if let Some((pane, hinting, columns, lines)) = before
                && (!self.app.renders[&pane].cache.hinting() || self.app.hints.is_none())
            {
                println!(
                    "Hints ended: cached {hinting}, grid {columns}x{lines} -> {}x{}, focused {}, active {:?}",
                    self.app.renders[&pane].cache.columns,
                    self.app.renders[&pane].cache.lines,
                    ui.input(|i| i.focused),
                    self.app.hints
                );
            }
            if self.applied && self.screen == "hints" && self.app.capture_sent {
                assert!(self.app.hints.is_some(), "Hints closed before capture");
            }
            if self.applied && self.screen == "explorer" && self.app.capture_sent {
                assert_eq!(self.app.ui.explorer.preview_location, Some((42, 4)));
                println!(
                    "Explorer target {:?}; pending scroll {}; panel open {}",
                    self.app.ui.explorer.preview_location,
                    self.app.ui.explorer.preview_jump,
                    self.app.ui.panel.open
                );
            }
            if !self.applied
                && self
                    .app
                    .controller
                    .model()
                    .active_pane()
                    .and_then(|pane| self.app.sessions.get(pane))
                    .is_some_and(|session| {
                        session.viewport().rows.iter().any(|row| {
                            row.iter().map(|cell| cell.c).collect::<String>().trim()
                                == "hint-fixture-ready"
                        })
                    })
            {
                // A startup output event can precede the viewport's final
                // resize. Start hints after the native layout has settled.
                let ready_at = self.ready_at.get_or_insert_with(Instant::now);
                if ready_at.elapsed() < Duration::from_millis(500) {
                    ui.ctx().request_repaint_after(Duration::from_millis(50));
                    return;
                }
                let pane = self.app.controller.model().active_pane().unwrap();
                if self.split && !self.split_created {
                    self.app
                        .action(ui.ctx(), Action::Split(pane, neptune_model::Axis::Vertical));
                    self.app.action(ui.ctx(), Action::Focus(pane));
                    self.split_created = true;
                    self.ready_at = None;
                    ui.ctx().request_repaint();
                    return;
                }
                if self.screen.starts_with("preferences") {
                    self.app.action(ui.ctx(), Action::Settings);
                    if self.screen != "preferences-general" {
                        self.app.ui.preference_view =
                            ui::preferences::View::searching("file locations");
                    }
                } else if self.screen == "palette" {
                    self.app.action(ui.ctx(), Action::Palette);
                    self.app.ui.palette_query = "hints".into();
                } else if self.screen == "explorer" {
                    self.app.action(
                        ui.ctx(),
                        Action::OpenTerminalLink(
                            pane,
                            crate::terminal_view::LinkTarget::File(
                                crate::platform::editor::FileLocation::parse("src/foo.rs:42:4")
                                    .unwrap(),
                            ),
                        ),
                    );
                } else if self.screen != "link" {
                    self.app.action(ui.ctx(), Action::CopyHints(pane));
                }
                self.app.screenshot = self.output.take();
                self.app.started = Instant::now();
                self.applied = true;
                ui.ctx().request_repaint();
            }
        }
        fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
            eframe::App::raw_input_hook(&mut self.app, ctx, input);
            // Desktop focus is shared. These visual fixtures own their input;
            // preserve screenshot replies, then add only this fixture's events.
            input
                .events
                .retain(|event| matches!(event, egui::Event::Screenshot { .. }));
            input.focused = true;
            if self.screen == "link"
                && self.applied
                && let Some(pane) = self.app.controller.model().active_pane()
                && let Some(body) = self
                    .app
                    .terminal_focus
                    .and_then(|id| ctx.read_response(id))
                    .map(|r| r.rect)
                && let Some(session) = self.app.sessions.get(pane)
                && let Some(render) = self.app.renders.get(&pane)
            {
                let snapshot = session.viewport();
                for (y, row) in snapshot.rows.iter().enumerate() {
                    let text: String = row.iter().map(|cell| cell.c).collect();
                    if let Some(x) = text.find("src/foo.rs:42:4") {
                        input
                            .events
                            .push(egui::Event::ModifiersChanged(egui::Modifiers::CTRL));
                        input.events.push(egui::Event::PointerMoved(
                            body.min
                                + egui::vec2(
                                    (x as f32 + 4.5) * render.cache.cell.x,
                                    (y as f32 + 0.5) * render.cache.cell.y,
                                ),
                        ));
                        break;
                    }
                }
            }
            if self.screen == "preferences-menu"
                && self.applied
                && self.click < 3
                && self.app.started.elapsed() > Duration::from_millis(800)
                && let Some(response) = ctx.read_response(egui::Id::new("preferences-file-editor"))
            {
                let pos = response.rect.center();
                self.click += 1;
                input.events.push(if self.click == 1 {
                    egui::Event::PointerMoved(pos)
                } else {
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.click == 2,
                        modifiers: egui::Modifiers::NONE,
                    }
                });
            }
        }
        fn on_exit(&mut self) {
            eframe::App::on_exit(&mut self.app);
        }
    }
    eframe::run_native("Neptune file locations visual QA", options, Box::new(move |cc| {
        let mut app = App::new(cc, Launch { cwd: Some(project), data_root: Some(data_path), command: Some(" clear; printf '\\n  error: expected expression\\n    --> src/foo.rs:42:4\\n\\n  Updated Cargo.toml and /tmp/build.log\\n  commit bd61a05e7a52c118d9e77d869a5d9ecb266ada55\\n  https://example.com/build/42\\n\\n  hint-fixture-ready\\n'".into()), screenshot: Some(output), ..Default::default() }, window_state::LoadReport::default());
        app.file_drag = crate::platform::file_drag::FileDragSource::detached();
        let output = app.screenshot.take();
        Ok(Box::new(Capture { app, output, screen, applied: false, ready_at: None, click: 0, split, split_created: false }))
    })).unwrap();
}

fn shortcut_events(ctx: &egui::Context) -> Vec<egui::Event> {
    ctx.input(|input| {
        input
            .events
            .iter()
            .filter(|event| !matches!(event, egui::Event::ModifiersChanged(_)))
            .cloned()
            .collect()
    })
}

#[test]
fn custom_keybindings_remap_navigation_and_preserve_overlay_ownership() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, [top_left, top_right, ..]) = navigation_fixture(root.path());
    let ctx = egui::Context::default();
    app.config.keybindings = toml::from_str::<Config>(
        r#"[keybindings]
        focus-right = ["Alt+H", "F12"]
    "#,
    )
    .unwrap()
    .keybindings;
    app.config.validate().unwrap();
    assert!(!press(
        &mut app,
        &ctx,
        key(
            egui::Key::ArrowRight,
            None,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT
        )
    ));
    assert_eq!(app.controller.model().active_pane(), Some(top_left));
    app.ui.overlay = OverlayState::Palette;
    assert!(!press(
        &mut app,
        &ctx,
        key(egui::Key::H, None, egui::Modifiers::ALT)
    ));
    assert_eq!(app.controller.model().active_pane(), Some(top_left));
    app.ui.overlay = OverlayState::None;
    assert!(press(
        &mut app,
        &ctx,
        key(egui::Key::F12, None, egui::Modifiers::NONE)
    ));
    assert_eq!(app.controller.model().active_pane(), Some(top_right));
}

#[test]
fn custom_keybindings_consume_text_releases_and_mirrored_clipboard_once() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.config.keybindings = toml::from_str::<Config>(
        r#"[keybindings]
        increase-font-size = ["Ctrl+C", "Alt+H"]
    "#,
    )
    .unwrap()
    .keybindings;
    let ctx = egui::Context::default();
    let down = key(egui::Key::C, None, egui::Modifiers::CTRL);
    let mut up = down.clone();
    if let egui::Event::Key { pressed, .. } = &mut up {
        *pressed = false;
    }
    ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                down,
                egui::Event::Copy,
                up,
            ],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            assert!(shortcut_events(ui.ctx()).is_empty());
        },
    )
    .textures_delta
    .clear();
    assert_eq!(app.config.font_size, 15.0);
    ctx.run_ui(
        egui::RawInput {
            events: vec![
                key(egui::Key::H, None, egui::Modifiers::ALT),
                egui::Event::Text("h".into()),
                key(egui::Key::A, None, egui::Modifiers::NONE),
                egui::Event::Text("a".into()),
            ],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            assert_eq!(
                shortcut_events(ui.ctx()),
                [
                    key(egui::Key::A, None, egui::Modifiers::NONE),
                    egui::Event::Text("a".into())
                ]
            );
        },
    )
    .textures_delta
    .clear();
    assert_eq!(app.config.font_size, 16.0);
}

#[test]
fn custom_keybindings_resolve_swallowed_clipboard_events_and_empty_paste() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.config.keybindings = toml::from_str::<Config>(
        r#"[keybindings]
        increase-font-size = ["Ctrl+C", "Ctrl+V", "Ctrl+X"]
    "#,
    )
    .unwrap()
    .keybindings;
    let ctx = egui::Context::default();
    for event in [
        egui::Event::Copy,
        egui::Event::Paste("clipboard".into()),
        egui::Event::Cut,
    ] {
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::ModifiersChanged(egui::Modifiers::CTRL), event],
                ..Default::default()
            },
            |ui| {
                app.shortcuts(ui.ctx());
                assert!(shortcut_events(ui.ctx()).is_empty());
            },
        )
        .textures_delta
        .clear();
    }
    app.swallowed_paste = Some(egui::Modifiers::CTRL);
    ctx.run_ui(egui::RawInput::default(), |ui| {
        app.shortcuts(ui.ctx());
        assert!(shortcut_events(ui.ctx()).is_empty());
    })
    .textures_delta
    .clear();
    assert_eq!(app.config.font_size, 18.0);
}

#[test]
fn custom_keybindings_disabled_clipboard_chords_reach_the_terminal_protocol() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.config.keybindings = toml::from_str::<Config>("[keybindings]\ncopy = []\npaste = []")
        .unwrap()
        .keybindings;
    let ctx = egui::Context::default();
    let modifiers = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
    ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Copy,
                egui::Event::Paste("secret".into()),
            ],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            let events = shortcut_events(ui.ctx());
            assert_eq!(
                events,
                [
                    key(egui::Key::C, None, modifiers),
                    key(egui::Key::V, None, modifiers)
                ]
            );
            let normalized = crate::input::normalize_events(&events, modifiers);
            let routed = crate::input::route_events(
                crate::input::RoutingContext::TerminalPane(1),
                &normalized,
                terminal_core::Mode::REPORT_ALL_KEYS_AS_ESC,
            );
            assert_eq!(routed.len(), 2);
            assert!(
                routed
                    .iter()
                    .all(|event| matches!(event.action, crate::input::InputAction::Write(_)))
            );
        },
    )
    .textures_delta
    .clear();
}

#[test]
fn custom_keybindings_invalid_config_cannot_be_overwritten_by_preferences() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.ephemeral = false;
    let source = "[keybindings]\nfind = [\"invalid-key\"]";
    std::fs::write(&app.config_path, source).unwrap();
    let error = Config::load(&app.config_path).unwrap_err();
    let ctx = egui::Context::default();
    app.complete_startup(
        &ctx,
        Startup {
            config: Config::default(),
            report: LoadReport {
                model: Some(Model::default()),
                diagnostics: vec![],
                can_write: true,
                migrated: false,
            },
            error: Some(format!("{error:#}")),
        },
    );
    app.action(
        &ctx,
        Action::Preferences(Config {
            font_size: 20.0,
            ..app.config.clone()
        }),
    );
    eframe::App::on_exit(&mut app);
    assert_eq!(std::fs::read_to_string(&app.config_path).unwrap(), source);
    assert_eq!(app.preference_generation, 0);
    assert!(
        app.ui
            .error
            .as_deref()
            .unwrap()
            .contains("Fix the config file")
    );
}

#[test]
fn custom_keybindings_release_stays_consumed_after_modifiers_and_overlay_change() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.config.keybindings =
        toml::from_str::<Config>("[keybindings]\ncommand-palette = [\"Ctrl+K\"]")
            .unwrap()
            .keybindings;
    let ctx = egui::Context::default();
    assert!(press(
        &mut app,
        &ctx,
        key(egui::Key::K, Some(egui::Key::K), egui::Modifiers::CTRL)
    ));
    assert_eq!(app.ui.overlay, OverlayState::Palette);
    let mut release = key(egui::Key::K, Some(egui::Key::K), egui::Modifiers::NONE);
    if let egui::Event::Key { pressed, .. } = &mut release {
        *pressed = false;
    }
    ctx.run_ui(
        egui::RawInput {
            events: vec![release],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            assert!(shortcut_events(ui.ctx()).is_empty());
        },
    )
    .textures_delta
    .clear();
}

#[test]
fn custom_keybindings_survive_font_shortcuts_queued_during_startup() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, sender) = fixture(root.path());
    let ctx = egui::Context::default();
    app.action(&ctx, Action::IncreaseFontSize);
    app.action(&ctx, Action::IncreaseFontSize);
    let mut config: Config =
        toml::from_str("font_size = 19\n[keybindings]\nfind = [\"F12\"]").unwrap();
    config.validate().unwrap();
    let bindings = config.keybindings.clone();
    sender.send(loaded(config, Model::default())).unwrap();
    app.poll(&ctx);
    assert_eq!(app.config.font_size, 21.0);
    assert_eq!(app.config.keybindings, bindings);
}

#[test]
fn custom_keybindings_unbound_clipboard_mirrors_do_not_duplicate_terminal_keys() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    let ctx = egui::Context::default();
    ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                key(egui::Key::C, None, egui::Modifiers::CTRL),
                egui::Event::Copy,
                key(egui::Key::V, None, egui::Modifiers::CTRL),
                egui::Event::Paste("clipboard".into()),
            ],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            assert_eq!(
                shortcut_events(ui.ctx()),
                [
                    key(egui::Key::C, None, egui::Modifiers::CTRL),
                    key(egui::Key::V, None, egui::Modifiers::CTRL)
                ]
            );
        },
    )
    .textures_delta
    .clear();
}

#[test]
fn custom_keybindings_cover_dedicated_clipboard_keys_and_disable_their_defaults() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.config.keybindings = toml::from_str::<Config>(
        "[keybindings]\ncopy = []\npaste = []\nincrease-font-size = [\"Cut\"]",
    )
    .unwrap()
    .keybindings;
    app.config.validate().unwrap();
    let ctx = egui::Context::default();
    ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::Copy,
                egui::Event::Paste("clipboard".into()),
                egui::Event::Cut,
            ],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            assert_eq!(
                shortcut_events(ui.ctx()),
                [
                    key(egui::Key::Copy, None, egui::Modifiers::NONE),
                    key(egui::Key::Paste, None, egui::Modifiers::NONE)
                ]
            );
        },
    )
    .textures_delta
    .clear();
    assert_eq!(app.config.font_size, 15.0);
}

#[test]
fn custom_keybindings_clipboard_uses_modifiers_at_the_event_even_if_released_in_that_frame() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _sender) = fixture(root.path());
    app.startup = None;
    app.config.keybindings =
        toml::from_str::<Config>("[keybindings]\nincrease-font-size = [\"Ctrl+C\"]")
            .unwrap()
            .keybindings;
    let ctx = egui::Context::default();
    ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::ModifiersChanged(egui::Modifiers::CTRL)],
            ..Default::default()
        },
        |ui| app.shortcuts(ui.ctx()),
    )
    .textures_delta
    .clear();
    ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::Copy,
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            ],
            ..Default::default()
        },
        |ui| {
            app.shortcuts(ui.ctx());
            assert!(shortcut_events(ui.ctx()).is_empty());
        },
    )
    .textures_delta
    .clear();
    assert_eq!(app.config.font_size, 15.0);
}
