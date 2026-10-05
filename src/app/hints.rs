//! Hint input has explicit ownership and a captured pane/session generation.
use super::*;
use crate::terminal_view::HintInput;

pub(super) struct PendingLocation {
    pane: PaneId,
    generation: u64,
    location: crate::platform::editor::FileLocation,
    reply: mpsc::Receiver<Result<PathBuf, &'static str>>,
}

impl App {
    pub(super) fn open_file_location(
        &mut self,
        ctx: &egui::Context,
        pane: PaneId,
        location: crate::platform::editor::FileLocation,
    ) {
        let directories = self.file_directories(pane);
        if !self.config.editor.is_empty() {
            if let Err(error) = self.editor_opener.start(
                crate::platform::files::Handoff::Edit {
                    location,
                    directories,
                    command: self.config.editor.clone(),
                },
                ctx.clone(),
            ) {
                self.ui.error = Some(error.into());
            }
            return;
        }
        if self.file_location.is_some() {
            self.ui.error =
                Some("A file location is already opening. Try again in a moment.".into());
            return;
        }
        let Some(generation) = self.sessions.generation(pane) else {
            return;
        };
        let (sender, reply) = mpsc::sync_channel(1);
        let target = location.clone();
        let wake = ctx.clone();
        if std::thread::Builder::new()
            .name("neptune-file-location".into())
            .spawn(move || {
                let _ = sender.send(crate::platform::editor::resolve(&target, &directories));
                wake.request_repaint();
            })
            .is_ok()
        {
            self.file_location = Some(PendingLocation {
                pane,
                generation,
                location,
                reply,
            });
        } else {
            self.ui.error = Some("Could not start the file reader.".into());
        }
    }

    pub(super) fn poll_file_location(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.file_location else {
            return;
        };
        let result = match pending.reply.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("The file reader stopped unexpectedly."),
        };
        let pending = self.file_location.take().unwrap();
        if self.sessions.generation(pending.pane) != Some(pending.generation)
            || self.controller.model().pane(pending.pane).is_none()
        {
            return;
        }
        match result {
            Ok(path) => {
                self.action(ctx, Action::Focus(pending.pane));
                self.panel_event(ctx, ui::panel::Event::Show(ui::panel::Tab::Files));
                self.sync_explorer(ctx);
                self.explorer_event(ctx, ui::explorer::Event::ShowInTree(path.clone(), false));
                self.explorer.show_preview(ctx, path, true);
                self.ui.explorer.preview_location =
                    Some((pending.location.line, pending.location.column));
                self.ui.explorer.preview_jump = true;
            }
            Err(error) => self.ui.error = Some(error.into()),
        }
    }
    pub(super) fn file_directories(&self, pane: PaneId) -> Vec<PathBuf> {
        let mut directories = Vec::new();
        if let Some(agent) = self.controller.model().pane(pane).and_then(|p| p.agent()) {
            directories.push(agent.cwd.clone());
        }
        if let Some(session) = self.sessions.get(pane) {
            let metadata = session.metadata();
            directories.extend(metadata.reported_cwd);
            directories.push(metadata.cwd);
        } else if let Some(pane) = self.controller.model().pane(pane) {
            directories.push(pane.cwd().into());
        }
        directories.dedup();
        directories
    }

    fn cancel_hints(&mut self) {
        if let Some((pane, _)) = self.hints.take()
            && let Some(render) = self.renders.get_mut(&pane)
        {
            render.cache.cancel_hints();
        }
    }

    pub(super) fn start_hints(&mut self, ctx: &egui::Context, pane: PaneId) {
        self.cancel_hints();
        let Some(generation) = self.sessions.generation(pane) else {
            return;
        };
        self.action(ctx, Action::Focus(pane));
        self.ui.overlay = OverlayState::None;
        self.ui.search_open = false;
        self.search_task = None;
        ctx.memory_mut(|memory| {
            if let Some(focused) = memory.focused() {
                memory.surrender_focus(focused);
            }
        });
        if self
            .renders
            .get_mut(&pane)
            .is_some_and(|render| render.cache.begin_hints())
        {
            self.hints = Some((pane, generation));
        } else {
            self.ui.error = Some("No paths, hashes or URLs in the visible terminal.".into());
        }
        ctx.request_repaint();
    }

    pub(super) fn hint_input(&mut self, ctx: &egui::Context) -> bool {
        // Release events for the entry chord and chosen label still belong to
        // hints after it closes, including terminals using the Kitty protocol.
        ctx.input_mut(|input| {
            input.events.retain(|event| {
                if let egui::Event::Key {
                    key,
                    pressed: false,
                    ..
                } = event
                    && let Some(index) = self.hint_keys.iter().position(|k| k == key)
                {
                    self.hint_keys.swap_remove(index);
                    return false;
                }
                true
            })
        });
        let Some((pane, generation)) = self.hints else {
            return false;
        };
        let events = ctx.input(|input| input.events.clone());
        for event in &events {
            if let egui::Event::Key { key, pressed, .. } = event {
                if *pressed {
                    if !self.hint_keys.contains(key) {
                        self.hint_keys.push(*key);
                    }
                } else {
                    self.hint_keys.retain(|value| value != key);
                }
            }
        }
        // The whole frame belongs to hints, including a paste, repeat, IME or
        // an extra key after a label, even if focus/generation invalidated it.
        ctx.input_mut(|input| {
            input.events.retain(|event| {
                matches!(
                    event,
                    egui::Event::Screenshot { .. }
                        | egui::Event::WindowFocused(_)
                        | egui::Event::ModifiersChanged(_)
                        | egui::Event::PointerMoved(_)
                        | egui::Event::PointerGone
                )
            })
        });
        self.swallowed_paste = None;
        if self.sessions.generation(pane) != Some(generation)
            || self.controller.model().active_pane() != Some(pane)
            || self.ui.overlay != OverlayState::None
            || !self.renders.get(&pane).is_some_and(|r| r.cache.hinting())
            || !ctx.input(|i| i.focused)
        {
            self.cancel_hints();
            return true;
        }
        let mut outcome = HintInput::Pending;
        for event in &events {
            match event {
                egui::Event::Key {
                    key, pressed: true, ..
                } => {
                    if *key == egui::Key::Escape {
                        outcome = HintInput::Cancel;
                        break;
                    }
                    if *key == egui::Key::Backspace
                        && let Some(render) = self.renders.get_mut(&pane)
                    {
                        render.cache.hint_backspace();
                    }
                }
                egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                    if let Some(render) = self.renders.get_mut(&pane) {
                        outcome = render.cache.hint_text(text);
                    }
                    if outcome != HintInput::Pending {
                        break;
                    }
                }
                egui::Event::PointerButton { pressed: true, .. }
                | egui::Event::MouseWheel { .. } => {
                    outcome = HintInput::Cancel;
                    break;
                }
                _ => {}
            }
        }
        match outcome {
            HintInput::Copy(text) => {
                crate::platform::clipboard::copy(ctx, text);
                self.cancel_hints();
            }
            HintInput::Cancel => self.cancel_hints(),
            HintInput::Pending => {}
        }
        true
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn key(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        let mut host = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| {
                app.poll(ui.ctx());
                eframe::App::ui(app, ui, &mut host);
            },
        );
        output.textures_delta.clear();
        output
    }

    fn opened(root: &std::path::Path) -> (App, egui::Context, PaneId) {
        let (mut app, _sender) = super::super::tests::fixture(root);
        app.startup = None;
        app.command = None;
        app.config.shell = Some("/bin/sh".into());
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        app.action(&ctx, Action::Create(root.into(), Some("Hints".into())));
        let pane = app.controller.model().active_pane().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.sessions.get(pane).is_none() {
            frame(&mut app, &ctx, Vec::new());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        app.sessions
            .get(pane)
            .unwrap()
            .write(b"printf '\\033[2J\\033[Hsrc/foo.rs:42 deadbee https://example.com/\\n'\n")
            .unwrap();
        while !app
            .sessions
            .get(pane)
            .unwrap()
            .viewport()
            .rows
            .iter()
            .any(|row| {
                row.iter()
                    .map(|c| c.c)
                    .collect::<String>()
                    .contains("src/foo.rs:42 deadbee")
                    && !row
                        .iter()
                        .map(|c| c.c)
                        .collect::<String>()
                        .contains("printf")
            })
        {
            frame(&mut app, &ctx, Vec::new());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        frame(&mut app, &ctx, Vec::new());
        (app, ctx, pane)
    }

    #[test]
    fn hint_keys_copy_and_cancel_without_reaching_the_shell_and_release_is_consumed() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx, pane) = opened(root.path());
        let chord = if cfg!(target_os = "macos") {
            egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT
        };
        frame(
            &mut app,
            &ctx,
            vec![
                key(egui::Key::H, true, chord),
                egui::Event::Text("H".into()),
            ],
        );
        assert_eq!(
            app.hints,
            Some((pane, app.sessions.generation(pane).unwrap()))
        );
        let out = frame(
            &mut app,
            &ctx,
            vec![
                key(egui::Key::H, false, chord),
                key(egui::Key::S, true, egui::Modifiers::NONE),
                egui::Event::Text("s".into()),
            ],
        );
        assert!(
            out.platform_output
                .commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == "deadbee"))
        );
        assert!(app.hints.is_none());
        let out = frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::S, false, egui::Modifiers::NONE)],
        );
        assert!(out.platform_output.commands.is_empty());
        app.start_hints(&ctx, pane);
        frame(
            &mut app,
            &ctx,
            vec![
                key(egui::Key::Escape, true, egui::Modifiers::NONE),
                egui::Event::Paste("touch should-not-exist\n".into()),
            ],
        );
        assert!(app.hints.is_none());
        app.sessions
            .get(pane)
            .unwrap()
            .write(b"printf 'shell-still-ready\\n'\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            frame(&mut app, &ctx, Vec::new());
            let snapshot = app.sessions.get(pane).unwrap().viewport();
            if snapshot.rows.iter().any(|r| {
                r.iter().map(|c| c.c).collect::<String>().trim_end() == "shell-still-ready"
            }) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!root.path().join("should-not-exist").exists());
    }

    #[test]
    fn ctrl_click_plain_and_quoted_files_resolves_real_files_from_pty_output() {
        let root = tempfile::tempdir().unwrap();
        let cases = [
            ("AGENTS.md", "AGENTS.md"),
            ("Cargo.toml", "Cargo.toml"),
            (".env", ".env"),
            ("README", "README"),
            ("Makefile", "Makefile"),
            ("launch-helper", "launch-helper"),
            ("asset.pkg+unknown", "asset.pkg+unknown"),
            ("世界.md", "世界.md"),
            ("my notes.md", "\"my notes.md\""),
            ("notes with no extension", "`notes with no extension`"),
            ("punctuation.md,", "\"punctuation.md,\""),
        ];
        for (name, _) in cases {
            std::fs::write(root.path().join(name), "fixture file\n").unwrap();
        }
        let (mut app, ctx, pane) = opened(root.path());
        for (name, printed) in cases {
            let label = format!("  └ Read {printed}");
            let command = format!(
                "printf '\\033[2J\\033[H%s\\n' '{}'\n",
                label.replace('\'', "'\\''")
            );
            app.sessions
                .get(pane)
                .unwrap()
                .write(command.as_bytes())
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let row = loop {
                frame(&mut app, &ctx, Vec::new());
                let snapshot = app.sessions.get(pane).unwrap().viewport();
                if let Some(row) = snapshot.rows.iter().position(|row| {
                    row.iter()
                        .filter(|cell| !cell.flags.contains(terminal_core::Flags::WIDE_CHAR_SPACER))
                        .map(|cell| cell.c)
                        .collect::<String>()
                        .trim_end()
                        == label
                }) {
                    break row;
                }
                assert!(Instant::now() < deadline, "PTY output for {name}");
                std::thread::sleep(Duration::from_millis(5));
            };
            frame(&mut app, &ctx, Vec::new());
            let rect = ctx.read_response(app.terminal_focus.unwrap()).unwrap().rect;
            let cell = app.renders[&pane].cache.cell;
            let column = 10 + usize::from(printed.starts_with(['"', '`']));
            let pos =
                rect.min + egui::vec2((column as f32 + 0.5) * cell.x, (row as f32 + 0.5) * cell.y);
            frame(
                &mut app,
                &ctx,
                vec![
                    egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                    egui::Event::PointerMoved(pos),
                ],
            );
            for pressed in [true, false] {
                frame(
                    &mut app,
                    &ctx,
                    vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::CTRL,
                    }],
                );
            }
            assert!(
                app.file_location.is_some(),
                "Ctrl-click did not request {name}"
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            while app.file_location.is_some() {
                frame(&mut app, &ctx, Vec::new());
                assert!(Instant::now() < deadline, "Opening {name}");
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(
                app.ui.explorer.selected.as_ref().map(|(path, _)| path),
                Some(&root.path().join(name).canonicalize().unwrap()),
                "{name}; error: {:?}",
                app.ui.error
            );
            assert_eq!(app.ui.explorer.preview_location, Some((1, 1)), "{name}");
            // Let the panel settle before measuring the next terminal body.
            for _ in 0..25 {
                frame(&mut app, &ctx, Vec::new());
            }
        }
    }

    #[test]
    fn built_in_locations_open_the_captured_pane_and_ignore_replacement_completions() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(
            root.path().join("src/foo.rs"),
            (1..=100).map(|i| format!("line {i}\n")).collect::<String>(),
        )
        .unwrap();
        let (mut app, ctx, pane) = opened(root.path());
        app.open_file_location(
            &ctx,
            pane,
            crate::platform::editor::FileLocation::parse("src/foo.rs:42:3").unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.file_location.is_some() {
            frame(&mut app, &ctx, Vec::new());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.ui.panel.open);
        assert_eq!(app.ui.panel.tab, ui::panel::Tab::Files);
        assert_eq!(app.ui.explorer.preview_location, Some((42, 3)));
        // Opening the panel briefly rests its folder watch while its slide is
        // at zero. Rebuilding that watch must not forget the requested line.
        for _ in 0..25 {
            frame(&mut app, &ctx, Vec::new());
        }
        assert_eq!(app.ui.explorer.preview_location, Some((42, 3)));
        assert_eq!(
            app.ui.explorer.selected.as_ref().unwrap().0,
            root.path().join("src/foo.rs").canonicalize().unwrap()
        );
        app.file_location = Some(PendingLocation {
            pane,
            generation: app.sessions.generation(pane).unwrap() + 1,
            location: crate::platform::editor::FileLocation::parse("src/foo.rs:99").unwrap(),
            reply: {
                let (s, r) = mpsc::sync_channel(1);
                s.send(Ok(root.path().join("src/foo.rs"))).unwrap();
                r
            },
        });
        app.poll_file_location(&ctx);
        assert_eq!(app.ui.explorer.preview_location, Some((42, 3)));
        app.start_hints(&ctx, pane);
        app.ui.overlay = OverlayState::Settings;
        frame(&mut app, &ctx, Vec::new());
        assert!(app.hints.is_none());
    }

    #[test]
    fn invalidated_hints_cannot_deliver_label_text_to_another_pane() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx, pane) = opened(root.path());
        app.action(&ctx, Action::Split(pane, neptune_model::Axis::Vertical));
        let other = app.controller.model().active_pane().unwrap();
        assert_ne!(other, pane);
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.sessions.get(other).is_none() {
            frame(&mut app, &ctx, Vec::new());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        app.action(&ctx, Action::Focus(pane));
        frame(&mut app, &ctx, Vec::new());
        app.start_hints(&ctx, pane);
        assert!(app.hints.is_some());
        app.action(&ctx, Action::Focus(other));
        let output = frame(
            &mut app,
            &ctx,
            vec![
                key(egui::Key::A, true, egui::Modifiers::NONE),
                egui::Event::Text("touch focus-leak\n".into()),
            ],
        );
        assert!(app.hints.is_none());
        assert!(output.platform_output.commands.is_empty());
        frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::A, false, egui::Modifiers::NONE)],
        );
        assert!(!ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Key { .. }))
        }));
        app.sessions
            .get(other)
            .unwrap()
            .write(b"printf 'ready-after-focus-change\\n'\n")
            .unwrap();
        loop {
            frame(&mut app, &ctx, Vec::new());
            if app
                .sessions
                .get(other)
                .unwrap()
                .viewport()
                .rows
                .iter()
                .any(|r| {
                    r.iter().map(|c| c.c).collect::<String>().trim_end()
                        == "ready-after-focus-change"
                })
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!root.path().join("focus-leak").exists());
    }

    #[test]
    fn output_keeps_processing_behind_stable_hints_and_returns_on_cancel() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, ctx, pane) = opened(root.path());
        app.start_hints(&ctx, pane);
        // Waiting for a label must sleep between native input/output events.
        let mut delay = Duration::ZERO;
        for _ in 0..4 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(app.hint_input(ui.ctx()));
            });
            output.textures_delta.clear();
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        }
        assert!(delay > Duration::ZERO, "Idle hints requested another frame");
        let rebuilds = app.renders[&pane].cache.render_rebuilds;
        app.sessions
            .get(pane)
            .unwrap()
            .write(b"printf 'output-behind-hints\\n'\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            frame(&mut app, &ctx, Vec::new());
            if app
                .sessions
                .get(pane)
                .unwrap()
                .viewport()
                .rows
                .iter()
                .any(|r| {
                    r.iter().map(|c| c.c).collect::<String>().trim_end() == "output-behind-hints"
                })
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(app.renders[&pane].cache.render_rebuilds, rebuilds);
        assert!(app.renders[&pane].cache.hinting());
        frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::Escape, true, egui::Modifiers::NONE)],
        );
        assert!(app.renders[&pane].cache.render_rebuilds > rebuilds);
        app.start_hints(&ctx, pane);
        app.action(&ctx, Action::Restart(pane));
        frame(&mut app, &ctx, Vec::new());
        assert!(app.hints.is_none());
    }
}
