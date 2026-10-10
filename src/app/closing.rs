//! One close transaction captures pane identities across asynchronous OS checks.
use super::*;
use terminal_core::ProcessActivity;
use ui::CloseStatus;

pub(super) struct PendingClose {
    target: Close,
    panes: Vec<(PaneId, u64)>,
    checks: Vec<mpsc::Receiver<ProcessActivity>>,
    since: Instant,
    running: usize,
    unknown: usize,
}

impl App {
    fn close_panes(&self, close: Close) -> Vec<(PaneId, u64)> {
        self.controller
            .model()
            .workspaces()
            .iter()
            .filter(|workspace| match close {
                Close::Workspace(id) | Close::Connection(id) => workspace.id() == id,
                _ => true,
            })
            .flat_map(|workspace| workspace.panes())
            .filter(|pane| !matches!(close, Close::Pane(id) if pane.id() != id))
            .map(|pane| (pane.id(), pane.generation()))
            .collect()
    }

    /// Quit through the usual confirmation, then start the installed update.
    /// Closing the window any other way leaves the restart to the user.
    pub(super) fn restart_updated(&mut self, ctx: &egui::Context) {
        if self.updates.installed().is_some() {
            self.relaunch = true;
            self.request_close(ctx, Close::App);
        }
    }

    /// Choosing to restart for an update is itself the decision to quit; only
    /// running processes are still worth a question.
    fn confirms_close(&self) -> bool {
        self.config.confirm_close && !self.relaunch
    }

    pub(super) fn request_close(&mut self, ctx: &egui::Context, target: Close) {
        if self
            .pending_close
            .as_ref()
            .is_some_and(|pending| pending.target == target)
            && self.ui.overlay == OverlayState::ConfirmClose(target)
        {
            return;
        }
        self.pending_close = None;
        self.ui.close_browser = match target {
            Close::Pane(id) => self
                .controller
                .model()
                .pane(id)
                .is_some_and(|p| p.kind() == neptune_model::PaneKind::Browser),
            _ => false,
        };
        if !self.confirms_close() && !self.config.warn_running_processes {
            self.finish_close(ctx, target);
            return;
        }
        let panes = self.close_panes(target);
        let mut pending = PendingClose {
            target,
            panes,
            checks: Vec::new(),
            since: Instant::now(),
            running: 0,
            unknown: usize::from(self.startup.is_some()),
        };
        if self.config.warn_running_processes {
            for &(pane, generation) in &pending.panes {
                if self
                    .controller
                    .model()
                    .pane(pane)
                    .is_some_and(|p| p.kind() == neptune_model::PaneKind::Browser)
                {
                    continue;
                }
                if let Some(session) = self.sessions.get(pane)
                    && self.sessions.generation(pane) == Some(generation)
                {
                    if matches!(session.metadata().status, SessionStatus::Exited { .. }) {
                        continue;
                    }
                    // A local SSH process cannot inspect the remote host's jobs.
                    // Closing it is itself destructive, even at a remote prompt.
                    if self.remote_of(pane).is_some() {
                        pending.running += 1;
                    } else {
                        pending.checks.push(session.check_process_activity());
                    }
                } else if self.controller.model().pane(pane).is_some_and(|pane| {
                    !matches!(pane.lifecycle(), Lifecycle::Exited | Lifecycle::Failed(_))
                }) {
                    pending.unknown += 1;
                }
            }
        }
        self.ui.overlay = OverlayState::ConfirmClose(target);
        self.ui.close_status = CloseStatus::Checking;
        self.pending_close = Some(pending);
        self.poll_close(ctx);
    }

    pub(super) fn poll_close(&mut self, ctx: &egui::Context) {
        let Some(mut pending) = self.pending_close.take() else {
            return;
        };
        if self.ui.overlay != OverlayState::ConfirmClose(pending.target) {
            return;
        }
        if self.close_panes(pending.target) != pending.panes {
            // Restart, movement or restoration changed the affected sessions.
            // Old observations (and old confirmations) cannot authorize them.
            self.request_close(ctx, pending.target);
            return;
        }
        pending.checks.retain(|check| {
            match check.try_recv() {
                Ok(ProcessActivity::Running) => pending.running += 1,
                Ok(ProcessActivity::Unknown) | Err(mpsc::TryRecvError::Disconnected) => {
                    pending.unknown += 1
                }
                Ok(ProcessActivity::Idle) => {}
                Err(mpsc::TryRecvError::Empty)
                    if pending.since.elapsed() < Duration::from_secs(2) =>
                {
                    return true;
                }
                Err(mpsc::TryRecvError::Empty) => pending.unknown += 1,
            }
            false
        });
        if pending.checks.is_empty() && self.ui.close_status == CloseStatus::Checking {
            ctx.request_repaint();
        }
        if !pending.checks.is_empty() {
            // Workers wake on completion; the deadline also covers stalled I/O.
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if pending.running > 0 {
            self.ui.close_status = CloseStatus::Running {
                terminals: pending.running,
                unknown: pending.unknown,
            };
        } else if pending.unknown > 0 && self.config.warn_running_processes {
            self.ui.close_status = CloseStatus::Unknown;
        } else if self.confirms_close() {
            self.ui.close_status = CloseStatus::General;
        } else {
            self.finish_close(ctx, pending.target);
            return;
        }
        self.pending_close = Some(pending);
    }

    pub(super) fn confirm_close(&mut self, ctx: &egui::Context, target: Close) {
        let Some(pending) = self.pending_close.as_ref() else {
            return;
        };
        if pending.target != target
            || self.ui.overlay != OverlayState::ConfirmClose(target)
            || self.ui.close_status == CloseStatus::Checking
        {
            return;
        }
        if self.close_panes(target) != pending.panes {
            self.pending_close = None;
            self.request_close(ctx, target);
            return;
        }
        self.finish_close(ctx, target);
    }

    fn finish_close(&mut self, ctx: &egui::Context, target: Close) {
        self.pending_close = None;
        self.ui.overlay = OverlayState::None;
        match target {
            Close::App => {
                self.exit_approved = true;
                crate::platform::window::send(ctx, crate::platform::window::WindowOperation::Close);
            }
            Close::Pane(pane) => self.dispatch(ctx, Command::ClosePane(pane)),
            Close::Workspace(workspace) => self.dispatch(ctx, Command::CloseWorkspace(workspace)),
            Close::Connection(workspace) => self.dispatch(
                ctx,
                Command::SetWorkspaceRemote {
                    workspace,
                    remote: None,
                },
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (App, egui::Context, tempfile::TempDir, PaneId) {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _sender) = crate::app::tests::fixture(root.path());
        app.startup = None;
        app.controller
            .dispatch(Command::AddWorkspace {
                cwd: root.path().into(),
                name: "Close test".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let pane = app.controller.model().active_pane().unwrap();
        (app, egui::Context::default(), root, pane)
    }

    fn observation(app: &mut App, target: Close) -> mpsc::SyncSender<ProcessActivity> {
        let (sender, receiver) = mpsc::sync_channel(1);
        app.ui.overlay = OverlayState::ConfirmClose(target);
        app.ui.close_status = CloseStatus::Checking;
        app.pending_close = Some(PendingClose {
            target,
            panes: app.close_panes(target),
            checks: vec![receiver],
            since: Instant::now(),
            running: 0,
            unknown: 0,
        });
        sender
    }

    fn close_frame(
        app: &mut App,
        ctx: &egui::Context,
        size: Vec2,
        time: f64,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let mut frame = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| eframe::App::ui(app, ui, &mut frame),
        );
        output.textures_delta.clear();
        output.shapes
    }

    fn has_scrim(shapes: &[egui::epaint::ClippedShape], size: Vec2) -> bool {
        shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Rect(rect)
                if rect.rect == Rect::from_min_size(Pos2::ZERO, size)
                    && rect.fill.a() > 0
                    && rect.fill.r() == 0 && rect.fill.g() == 0 && rect.fill.b() == 0)
        })
    }

    fn has_close_sheet(shapes: &[egui::epaint::ClippedShape], target: Close) -> bool {
        shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text)
                if text.galley.text() == ui::dialogs::close_copy(target).0)
        })
    }

    #[test]
    fn close_idle_checks_never_dim_the_window_including_the_completion_frame() {
        for size in [egui::vec2(900.0, 640.0), egui::vec2(640.0, 400.0)] {
            for kind in 0..4 {
                for cancel in [false, true] {
                    let (mut app, ctx, _root, pane) = setup();
                    app.config.confirm_close = false;
                    let workspace = app.controller.model().active_workspace().unwrap();
                    let target = match kind {
                        0 => Close::Pane(pane),
                        1 => Close::Workspace(workspace),
                        2 => Close::Connection(workspace),
                        _ => Close::App,
                    };
                    ctx.set_fonts(crate::platform::fonts::bundled_definitions());
                    theme::apply(&ctx, &app.config);
                    close_frame(&mut app, &ctx, size, 0.0, vec![]);
                    let sender = observation(&mut app, target);
                    // Keep the worker pending longer than the scrim animation.
                    for tick in 1..=4 {
                        let shapes = close_frame(&mut app, &ctx, size, tick as f64 * 0.1, vec![]);
                        assert!(!has_scrim(&shapes, size), "{target:?}: checking");
                        assert!(!has_close_sheet(&shapes, target));
                        assert!(app.pending_close.is_some());
                        assert!(app.controller.model().pane(pane).is_some());
                    }
                    sender.send(ProcessActivity::Idle).unwrap();
                    let events = if cancel {
                        vec![egui::Event::Key {
                            key: egui::Key::Escape,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }]
                    } else {
                        vec![]
                    };
                    // The completed result is polled after this frame paints.
                    let shapes = close_frame(&mut app, &ctx, size, 0.5, events);
                    assert!(!has_scrim(&shapes, size), "{target:?}: completion");
                    assert!(!has_close_sheet(&shapes, target));
                    assert!(app.pending_close.is_none());
                    assert_eq!(app.exit_approved, target == Close::App && !cancel);
                    if cancel {
                        assert!(app.controller.model().pane(pane).is_some());
                    } else if matches!(target, Close::Pane(_) | Close::Workspace(_)) {
                        assert!(app.controller.model().pane(pane).is_none());
                    }
                    for tick in 6..=9 {
                        let shapes = close_frame(&mut app, &ctx, size, tick as f64 * 0.1, vec![]);
                        assert!(!has_scrim(&shapes, size), "{target:?}: after completion");
                        assert!(!has_close_sheet(&shapes, target));
                    }
                }
            }
        }
    }

    #[test]
    fn close_confirmation_dims_only_after_a_check_requires_a_sheet() {
        for result in [
            ProcessActivity::Idle,
            ProcessActivity::Running,
            ProcessActivity::Unknown,
        ] {
            let (mut app, ctx, _root, pane) = setup();
            app.config.confirm_close = result == ProcessActivity::Idle;
            let target = Close::Pane(pane);
            let size = egui::vec2(640.0, 400.0);
            ctx.set_fonts(crate::platform::fonts::bundled_definitions());
            theme::apply(&ctx, &app.config);
            close_frame(&mut app, &ctx, size, 0.0, vec![]);
            let sender = observation(&mut app, target);
            for tick in 1..=4 {
                let shapes = close_frame(&mut app, &ctx, size, tick as f64 * 0.1, vec![]);
                assert!(!has_scrim(&shapes, size));
                assert!(!has_close_sheet(&shapes, target));
            }
            sender.send(result).unwrap();
            let shapes = close_frame(&mut app, &ctx, size, 0.5, vec![]);
            assert!(!has_scrim(&shapes, size));
            assert!(!has_close_sheet(&shapes, target));
            assert!(app.controller.model().pane(pane).is_some());
            for tick in 6..=9 {
                close_frame(&mut app, &ctx, size, tick as f64 * 0.1, vec![]);
            }
            let shapes = close_frame(&mut app, &ctx, size, 1.0, vec![]);
            assert!(has_scrim(&shapes, size), "{result:?}: confirmed warning");
            assert!(has_close_sheet(&shapes, target));
            app.action(&ctx, Action::CancelClose);
            for tick in 11..=14 {
                close_frame(&mut app, &ctx, size, tick as f64 * 0.1, vec![]);
            }
            let shapes = close_frame(&mut app, &ctx, size, 1.5, vec![]);
            assert!(!has_scrim(&shapes, size));
            assert!(!has_close_sheet(&shapes, target));
            assert!(app.controller.model().pane(pane).is_some());
        }
    }

    #[test]
    fn close_idle_app_and_pane_wait_for_checks_and_honor_escape() {
        for whole_app in [false, true] {
            for cancel in [false, true] {
                let (mut app, ctx, _root, pane) = setup();
                app.config.confirm_close = false;
                let target = if whole_app {
                    Close::App
                } else {
                    Close::Pane(pane)
                };
                let sender = observation(&mut app, target);
                app.poll_close(&ctx);
                assert!(!app.exit_approved);
                assert!(app.controller.model().pane(pane).is_some());
                assert_eq!(app.ui.close_status, CloseStatus::Checking);
                sender.send(ProcessActivity::Idle).unwrap();
                if cancel {
                    // Escape is applied before polling a completed check.
                    app.action(&ctx, Action::CloseOverlay);
                }
                app.poll_close(&ctx);
                assert_eq!(app.exit_approved, whole_app && !cancel);
                assert_eq!(
                    app.controller.model().pane(pane).is_some(),
                    whole_app || cancel
                );
                assert_eq!(app.ui.overlay, OverlayState::None);
                assert!(app.pending_close.is_none());
            }
        }
    }

    #[test]
    fn close_process_warning_is_independent_and_idle_results_obey_general_preference() {
        for confirm in [false, true] {
            for result in [
                ProcessActivity::Idle,
                ProcessActivity::Running,
                ProcessActivity::Unknown,
            ] {
                let (mut app, ctx, _root, pane) = setup();
                app.config.confirm_close = confirm;
                let sender = observation(&mut app, Close::Pane(pane));
                sender.send(result).unwrap();
                app.poll_close(&ctx);
                let warns = confirm || result != ProcessActivity::Idle;
                assert_eq!(app.controller.model().pane(pane).is_some(), warns);
                assert_eq!(
                    matches!(app.ui.overlay, OverlayState::ConfirmClose(_)),
                    warns
                );
                if warns {
                    app.action(&ctx, Action::CancelClose);
                    assert!(app.controller.model().pane(pane).is_some());
                    assert!(app.pending_close.is_none());
                }
            }
        }
    }

    #[test]
    fn close_disabled_preferences_skip_checks_and_starting_sessions_otherwise_warn() {
        for warn in [false, true] {
            let (mut app, ctx, _root, pane) = setup();
            app.config.confirm_close = false;
            app.config.warn_running_processes = warn;
            app.action(&ctx, Action::ClosePane(pane));
            assert_eq!(app.controller.model().pane(pane).is_some(), warn);
            if warn {
                assert_eq!(app.ui.close_status, CloseStatus::Unknown);
            }
        }
    }

    #[test]
    fn close_failed_and_exited_sessions_do_not_trigger_process_warnings() {
        for failed in [false, true] {
            let (mut app, ctx, _root, pane) = setup();
            app.config.confirm_close = false;
            let completion = if failed {
                Command::SessionFailed {
                    pane,
                    generation: 1,
                    error: "spawn failed".into(),
                }
            } else {
                app.controller
                    .dispatch(Command::SessionStarted {
                        pane,
                        generation: 1,
                    })
                    .unwrap();
                Command::SessionExited {
                    pane,
                    generation: 1,
                }
            };
            app.controller.dispatch(completion).unwrap();
            app.action(&ctx, Action::ClosePane(pane));
            assert!(app.controller.model().pane(pane).is_none());
            assert_eq!(app.ui.overlay, OverlayState::None);
        }
    }

    #[test]
    fn close_general_confirmation_can_run_without_a_process_check() {
        let (mut app, ctx, _root, pane) = setup();
        app.config.warn_running_processes = false;
        app.action(&ctx, Action::ClosePane(pane));
        assert_eq!(app.ui.close_status, CloseStatus::General);
        assert!(app.pending_close.as_ref().unwrap().checks.is_empty());
        app.action(&ctx, Action::Confirm(Close::Pane(pane)));
        assert!(app.controller.model().pane(pane).is_none());
    }

    #[test]
    fn close_late_results_cannot_close_cancelled_or_replaced_panes() {
        let (mut app, ctx, _root, pane) = setup();
        app.config.confirm_close = false;
        let sender = observation(&mut app, Close::Pane(pane));
        app.action(&ctx, Action::CancelClose);
        let _ = sender.send(ProcessActivity::Idle);
        app.poll_close(&ctx);
        assert!(app.controller.model().pane(pane).is_some());
        let sender = observation(&mut app, Close::Pane(pane));
        app.controller.dispatch(Command::RestartPane(pane)).unwrap();
        sender.send(ProcessActivity::Idle).unwrap();
        app.poll_close(&ctx);
        assert!(app.controller.model().pane(pane).is_some());
        assert_eq!(app.ui.close_status, CloseStatus::Unknown);
    }

    #[test]
    fn close_timeout_disconnect_and_confirmation_during_check_never_auto_close() {
        for disconnect in [false, true] {
            let (mut app, ctx, _root, pane) = setup();
            app.config.confirm_close = false;
            let sender = observation(&mut app, Close::Pane(pane));
            app.action(&ctx, Action::Confirm(Close::Pane(pane)));
            assert!(app.controller.model().pane(pane).is_some());
            if disconnect {
                drop(sender);
            } else {
                app.pending_close.as_mut().unwrap().since -= Duration::from_secs(3);
            }
            app.poll_close(&ctx);
            assert_eq!(app.ui.close_status, CloseStatus::Unknown);
            app.action(&ctx, Action::Confirm(Close::Pane(pane)));
            assert!(app.controller.model().pane(pane).is_none());
        }
    }

    #[test]
    fn close_app_and_workspace_include_hidden_panes_and_validate_confirmed_generations() {
        let (mut app, ctx, root, hidden) = setup();
        let workspace = app.controller.model().active_workspace().unwrap();
        app.controller
            .dispatch(Command::AddWorkspace {
                cwd: root.path().into(),
                name: "Visible".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        assert_eq!(app.close_panes(Close::App).len(), 2);
        assert_eq!(
            app.close_panes(Close::Workspace(workspace)),
            vec![(hidden, 1)]
        );
        app.config.confirm_close = false;
        let sender = observation(&mut app, Close::App);
        sender.send(ProcessActivity::Running).unwrap();
        app.poll_close(&ctx);
        assert!(!app.exit_approved);
        app.controller
            .dispatch(Command::RestartPane(hidden))
            .unwrap();
        app.action(&ctx, Action::Confirm(Close::App));
        assert!(!app.exit_approved);
        app.action(&ctx, Action::Confirm(Close::App));
        assert!(app.exit_approved);
    }

    #[test]
    fn restarting_for_an_update_asks_only_about_processes_and_a_plain_close_forgets_it() {
        let (mut app, ctx, _root, _pane) = setup();
        app.config.warn_running_processes = false;
        // Nothing is installed, so there is nothing to restart into.
        app.action(&ctx, Action::RestartUpdate);
        assert!(!app.relaunch && !app.exit_approved);
        crate::runtime::updates::tests::installed(&mut app.updates);
        app.action(&ctx, Action::RestartUpdate);
        assert!(app.config.confirm_close);
        assert!(app.relaunch && app.exit_approved);

        let (mut app, ctx, _root, _pane) = setup();
        crate::runtime::updates::tests::installed(&mut app.updates);
        app.action(&ctx, Action::RestartUpdate);
        assert_eq!(app.ui.overlay, OverlayState::ConfirmClose(Close::App));
        assert_eq!(app.ui.close_status, CloseStatus::Unknown);
        assert!(app.relaunch && !app.exit_approved);
        // Closing the window instead quits without starting the new version.
        app.action(&ctx, Action::WindowClose);
        app.action(&ctx, Action::Confirm(Close::App));
        assert!(!app.relaunch && app.exit_approved);
    }
}
