use super::*;
use crate::input::{InputAction, PointerButton, PointerEvent, RoutingContext, WheelUnit};
impl App {
    pub(super) fn terminal_input(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        context: RoutingContext,
    ) {
        let Some(id) = self.controller.model().active_pane() else {
            return;
        };
        if self.hints.is_some() || !matches!(context, RoutingContext::TerminalPane(_)) {
            return;
        }
        if self
            .controller
            .model()
            .pane(id)
            .is_some_and(|p| matches!(p.lifecycle(), Lifecycle::Exited | Lifecycle::Failed(_)))
        {
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.action(ctx, Action::Restart(id));
            }
            return;
        }
        let Some(session) = self.sessions.get(id) else {
            return;
        };
        let events = self.terminal_events(ctx);
        let Some(pane) = self.renders.get_mut(&id) else {
            return;
        };
        let mode = session.modes();
        let normalized = crate::input::normalize_events(&events, ctx.input(|i| i.modifiers));
        for input in crate::input::route_events(context, &normalized, mode) {
            let result = match input.action {
                InputAction::Write(bytes) => {
                    self.notifications.acknowledge(Some(id));
                    if let Some(reply) = crate::agent_activity::Reply::from_bytes(&bytes) {
                        self.agents.reply(id, reply, Instant::now());
                    }
                    session.scroll_to_bottom();
                    session.write(&bytes)
                }
                InputAction::Paste(text) => {
                    self.notifications.acknowledge(Some(id));
                    session.paste(&text)
                }
                InputAction::Copy => {
                    if let Some(text) = session.selected_text() {
                        crate::platform::clipboard::copy(ctx, text);
                    }
                    continue;
                }
                InputAction::Preedit(text) => {
                    pane.preedit = text;
                    continue;
                }
                InputAction::ScrollPage { reverse } => {
                    session.scroll(if reverse {
                        pane.cache.lines as i32
                    } else {
                        -(pane.cache.lines as i32)
                    });
                    continue;
                }
                // Window focus is handled by poll even when a dialog owns
                // input or eframe skips UI for a minimized window.
                InputAction::Focus(_) => continue,
            };
            if let Err(error) = result {
                self.diagnostics.failure(
                    "input",
                    Some(id),
                    self.sessions.generation(id),
                    &format!("{:?}", error.kind()),
                );
                self.ui.error = Some(error.to_string());
                return;
            }
        }
        let pointer_events =
            crate::input::normalize_pointer_events(&events, ctx.input(|i| i.modifiers));
        for routed in crate::input::route_pointer_events(context, &pointer_events) {
            let result = match routed.event {
                PointerEvent::Wheel {
                    delta,
                    unit,
                    modifiers: m,
                    ..
                } => {
                    if !ctx.input(|i| i.pointer.hover_pos().is_some_and(|pos| rect.contains(pos))) {
                        continue;
                    }
                    let lines = match unit {
                        WheelUnit::Line => delta.y * 3.0,
                        WheelUnit::Page => delta.y * pane.cache.lines as f32,
                        WheelUnit::Point => delta.y / pane.cache.cell.y,
                    };
                    if mode.intersects(Mode::MOUSE_MODE) && !m.shift {
                        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
                            continue;
                        };
                        let col = ((pos.x - rect.left()) / pane.cache.cell.x).max(0.0) as u16;
                        let row = ((pos.y - rect.top()) / pane.cache.cell.y).max(0.0) as u16;
                        if let Some(bytes) = terminal_core::input::mouse(
                            if lines > 0.0 { 64 } else { 65 },
                            col,
                            row,
                            true,
                            m,
                            mode,
                        ) {
                            for _ in 0..lines.abs().ceil().min(12.0) as usize {
                                let _ = session.write(&bytes);
                            }
                        }
                    } else if mode.contains(Mode::ALT_SCREEN)
                        && mode.contains(Mode::ALTERNATE_SCROLL)
                        && !m.shift
                    {
                        let bytes = if lines > 0.0 { b"\x1b[A" } else { b"\x1b[B" };
                        for _ in 0..lines.abs().ceil().min(12.0) as usize {
                            let _ = session.write(bytes);
                        }
                    } else {
                        session.scroll(lines.round() as i32);
                    }
                    continue;
                }
                PointerEvent::Button {
                    position,
                    button,
                    pressed,
                    modifiers: m,
                } => {
                    let pos = egui::pos2(position.x, position.y);
                    if button == PointerButton::Primary && pane.cache.link_pointer_owned {
                        continue;
                    }
                    if (pressed && (!rect.contains(pos) || m.shift))
                        || (!pressed && pane.mouse_button.is_none())
                    {
                        continue;
                    }
                    let col = (((pos.x - rect.left()) / pane.cache.cell.x).max(0.0) as u16)
                        .min(pane.cache.columns.saturating_sub(1));
                    let row = (((pos.y - rect.top()) / pane.cache.cell.y).max(0.0) as u16)
                        .min(pane.cache.lines.saturating_sub(1));
                    let button = match button {
                        PointerButton::Primary => 0,
                        PointerButton::Middle => 1,
                        PointerButton::Secondary => 2,
                    };
                    if let Some(bytes) =
                        terminal_core::input::mouse(button, col, row, pressed, m, mode)
                    {
                        pane.mouse_button = if pressed { Some(button) } else { None };
                        pane.mouse_cell = None;
                        session.write(&bytes)
                    } else {
                        continue;
                    }
                }
                PointerEvent::Moved {
                    position,
                    modifiers: m,
                } => {
                    let pos = egui::pos2(position.x, position.y);
                    if !rect.contains(pos) || m.shift || pane.cache.link_pointer_owned {
                        continue;
                    }
                    let col = ((pos.x - rect.left()) / pane.cache.cell.x).max(0.0) as u16;
                    let row = ((pos.y - rect.top()) / pane.cache.cell.y).max(0.0) as u16;
                    let button = pane.mouse_button;
                    if pane.mouse_cell == Some((col, row, button)) {
                        continue;
                    }
                    pane.mouse_cell = Some((col, row, button));
                    if let Some(bytes) =
                        terminal_core::input::encode_mouse_motion(button, col, row, m, mode)
                    {
                        session.write(&bytes)
                    } else {
                        continue;
                    }
                }
            };
            if let Err(e) = result {
                self.diagnostics.failure(
                    "pointer",
                    Some(id),
                    self.sessions.generation(id),
                    &format!("{:?}", e.kind()),
                );
                self.ui.error = Some(e.to_string());
                break;
            }
        }
    }
}
impl App {
    pub(super) fn terminal_owns_shortcuts(&self, ctx: &egui::Context) -> bool {
        self.ui.overlay == OverlayState::None
            && !egui::Popup::is_any_open(ctx)
            && ctx.memory(|memory| {
                memory.focused().is_none() || memory.focused() == self.terminal_focus
            })
    }

    /// Whether text of a panel is selected, such as a reply of a project's
    /// lead. A copy is then that text's: labels take no keyboard focus, so
    /// the terminal would otherwise get the chord as a key.
    fn text_selected(&self, ctx: &egui::Context) -> bool {
        self.explorer_text_selected()
            || ctx
                .with_plugin(|labels: &mut egui::text_selection::LabelSelectionState| {
                    labels.has_selection()
                })
                .unwrap_or(false)
    }

    /// The frame's events as a terminal takes them. A copy left for the text
    /// selected in a panel is not also the terminal's interrupt.
    pub(super) fn terminal_events(&self, ctx: &egui::Context) -> Vec<egui::Event> {
        let mut events = ctx.input(|input| input.events.clone());
        if self.text_selected(ctx) {
            events.retain(|event| !matches!(event, egui::Event::Copy | egui::Event::Cut));
        }
        events
    }

    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        self.shortcuts_with_keymap(ctx, crate::platform::keyboard::unshifted_zoom_key);
    }

    pub(super) fn shortcuts_with_keymap(
        &mut self,
        ctx: &egui::Context,
        mut unshifted_key: impl FnMut(egui::Key) -> Option<egui::Key>,
    ) {
        use crate::keybindings::BindingAction as Binding;
        if self.hints.is_some() {
            ctx.input(|input| {
                for event in &input.events {
                    if let egui::Event::Key {
                        key,
                        physical_key,
                        pressed: false,
                        ..
                    } = event
                    {
                        self.shortcut_keys.remove(&physical_key.unwrap_or(*key));
                    }
                }
            });
        }
        if self.hint_input(ctx) {
            return;
        }
        // A clipboard without text may leave only V's release. Recover the
        // swallowed press before resolving it so remaps apply here as well.
        if let Some(modifiers) = self.swallowed_paste.take()
            && self.terminal_owns_shortcuts(ctx)
        {
            ctx.input_mut(|input| {
                input.events.insert(
                    0,
                    egui::Event::Key {
                        key: egui::Key::V,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                )
            });
        }
        if !ctx.input(|input| input.focused) {
            self.shortcut_keys.clear();
            self.shortcut_modifiers = egui::Modifiers::NONE;
        }
        let events = ctx.input(|input| input.events.clone());
        let mut frame_modifiers = self.shortcut_modifiers;
        for event in events {
            if let egui::Event::ModifiersChanged(modifiers) = event {
                frame_modifiers = modifiers;
                continue;
            }
            if matches!(
                event,
                egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
            ) && !ctx.input(|input| input.events.contains(&event))
            {
                continue;
            }
            let (key, physical_key, pressed, m) = match &event {
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed,
                    modifiers,
                    ..
                } => (*key, *physical_key, *pressed, *modifiers),
                egui::Event::Copy
                    if !self.text_selected(ctx) && self.terminal_owns_shortcuts(ctx) =>
                {
                    (
                        if frame_modifiers.ctrl || frame_modifiers.mac_cmd {
                            egui::Key::C
                        } else {
                            egui::Key::Copy
                        },
                        None,
                        true,
                        frame_modifiers,
                    )
                }
                egui::Event::Cut
                    if !self.text_selected(ctx) && self.terminal_owns_shortcuts(ctx) =>
                {
                    (
                        if frame_modifiers.ctrl || frame_modifiers.mac_cmd {
                            egui::Key::X
                        } else if cfg!(windows) && frame_modifiers.shift {
                            egui::Key::Delete
                        } else {
                            egui::Key::Cut
                        },
                        None,
                        true,
                        frame_modifiers,
                    )
                }
                egui::Event::Paste(_) if self.terminal_owns_shortcuts(ctx) => (
                    if frame_modifiers.ctrl || frame_modifiers.mac_cmd {
                        egui::Key::V
                    } else if cfg!(windows) && frame_modifiers.shift {
                        egui::Key::Insert
                    } else {
                        egui::Key::Paste
                    },
                    None,
                    true,
                    frame_modifiers,
                ),
                _ => continue,
            };
            frame_modifiers = m;
            if !pressed && self.shortcut_keys.remove(&physical_key.unwrap_or(key)) {
                ctx.input_mut(|input| {
                    consume_binding(&mut input.events, &event, key, physical_key, m)
                });
                continue;
            }
            if pressed && key == egui::Key::Escape {
                // Escape cancels a terminal drag, then leaves the topmost
                // transient surface. With neither it belongs to the terminal:
                // a message never takes a key the shell is waiting for, and is
                // dismissed by Escape only when there is no terminal to
                // receive it.
                let search = ui::search::input_id();
                let searching = self.ui.search_open
                    && ctx.memory(|memory| {
                        memory.has_focus(search) || memory.had_focus_last_frame(search)
                    });
                // A menu or list open over a sheet that holds them is left
                // before the sheet is.
                if egui::Popup::is_any_open(ctx) {
                    egui::Popup::close_all(ctx);
                } else if self.ui.pane_drag.is_some() {
                    self.cancel_pane_drag(ctx);
                } else if self.ui.overlay != OverlayState::None {
                    // Preferences leaves the theme screens, then a search, first.
                    if self.ui.overlay != OverlayState::Settings
                        || !(self.ui.preferences.back() || self.ui.preference_view.back())
                    {
                        self.action(ctx, Action::CloseOverlay);
                    }
                } else if searching {
                    self.action(ctx, Action::CloseSearch);
                } else if self.explorer_escape(ctx) {
                    // A name being typed, a search or its field was left.
                } else if self.project_escape(ctx) || self.pull_request_escape(ctx) {
                    // A message being written keeps its text; the keyboard
                    // returns to the terminal.
                } else if self.ui.error.is_some() && self.controller.model().active_pane().is_none()
                {
                    self.ui.error = None;
                } else {
                    continue;
                }
                ctx.input_mut(|i| {
                    i.consume_key(m, key);
                });
                continue;
            }

            let font_key = if m.shift && !m.alt && (m.ctrl ^ m.mac_cmd) {
                physical_key.and_then(&mut unshifted_key).unwrap_or(key)
            } else {
                key
            };
            let Some(binding) = self
                .config
                .keybindings
                .resolve(key, physical_key, m, font_key)
            else {
                // egui's clipboard events stand for shortcuts it swallowed.
                // Unbound chords must still reach the terminal as keys, rather
                // than falling back to the old host copy/paste policy.
                if self.terminal_owns_shortcuts(ctx)
                    && (m.ctrl
                        || m.mac_cmd
                        || matches!(key, egui::Key::Copy | egui::Key::Cut | egui::Key::Paste)
                        || (cfg!(windows) && matches!(key, egui::Key::Insert | egui::Key::Delete)))
                {
                    ctx.input_mut(|input| {
                        let mirrored = input
                            .events
                            .iter()
                            .take_while(|value| *value != &event)
                            .filter_map(|value| match value {
                                egui::Event::Key {
                                    key,
                                    modifiers,
                                    pressed: true,
                                    ..
                                } => Some((*key, *modifiers)),
                                _ => None,
                            })
                            .last()
                            == Some((key, m));
                        if mirrored {
                            input.events.retain(|value| value != &event);
                            return;
                        }
                        for value in &mut input.events {
                            if *value == event
                                && matches!(
                                    value,
                                    egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
                                )
                            {
                                *value = egui::Event::Key {
                                    key,
                                    physical_key: None,
                                    pressed: true,
                                    repeat: false,
                                    modifiers: m,
                                };
                            }
                        }
                    });
                }
                continue;
            };
            // Directional navigation always belongs to terminals, including
            // at an outer edge. Custom terminal bindings also respect fields,
            // dialogs and the command palette.
            let directional = matches!(
                binding,
                Binding::FocusLeft | Binding::FocusRight | Binding::FocusUp | Binding::FocusDown
            );
            let terminal_only = directional
                || matches!(binding, Binding::CopyHints | Binding::NewWorktree)
                || (self.config.keybindings.overridden(binding)
                    && !(binding.global()
                        && (m.ctrl
                            || m.mac_cmd
                            || key
                                .name()
                                .strip_prefix('F')
                                .is_some_and(|number| number.parse::<u8>().is_ok()))));
            if terminal_only && !self.terminal_owns_shortcuts(ctx) {
                continue;
            }
            // The copy chord is the selected text's too, as it is the
            // terminal selection's.
            if binding == Binding::Copy && self.text_selected(ctx) {
                if pressed {
                    self.shortcut_keys.insert(physical_key.unwrap_or(key));
                }
                ctx.input_mut(|input| {
                    consume_binding(&mut input.events, &event, key, physical_key, m);
                    if pressed {
                        input.events.push(egui::Event::Copy);
                    }
                });
                continue;
            }
            let action = self.binding_action(binding);
            // Preserve defaults that belong to the shell when their target
            // does not exist. Explicit overrides reserve the chosen chord.
            let reserves_direction = directional && self.controller.model().active_pane().is_some();
            if action.is_none()
                && !reserves_direction
                && !self.config.keybindings.overridden(binding)
            {
                continue;
            }
            if pressed {
                self.shortcut_keys.insert(physical_key.unwrap_or(key));
                if binding == Binding::Paste
                    && let egui::Event::Paste(text) = &event
                {
                    if let Some(pane) = self.controller.model().active_pane() {
                        self.paste_text(pane, text);
                    }
                } else if let Some(action) = action {
                    self.action(ctx, action);
                }
            }
            ctx.input_mut(|input| consume_binding(&mut input.events, &event, key, physical_key, m));
        }
        self.shortcut_modifiers = ctx.input(|input| input.modifiers);
    }

    fn binding_action(&self, binding: crate::keybindings::BindingAction) -> Option<Action> {
        use crate::keybindings::BindingAction::*;
        let pane = self.controller.model().active_pane();
        let workspace = self
            .controller
            .model()
            .active_workspace()
            .and_then(|id| self.controller.model().workspace(id));
        let direction = match binding {
            FocusLeft => Some(neptune_model::FocusDirection::Left),
            FocusRight => Some(neptune_model::FocusDirection::Right),
            FocusUp => Some(neptune_model::FocusDirection::Up),
            FocusDown => Some(neptune_model::FocusDirection::Down),
            _ => None,
        };
        if let Some(direction) = direction {
            let workspace = workspace?;
            return workspace
                .layout()
                .adjacent(workspace.active(), direction)
                .map(Action::Focus);
        }
        if let Some(index) = binding.workspace_index() {
            return self
                .controller
                .model()
                .workspaces()
                .get(index)
                .map(|workspace| Action::SelectWorkspace(workspace.id()));
        }
        Some(match binding {
            NewWorkspace => Action::New,
            NewWorktree => Action::Worktree(ui::worktrees::Event::New(pane?)),
            NewTab => pane.map_or(Action::New, Action::NewTab),
            NewBrowser => Action::NewBrowser(pane?, None, None),
            BrowserAddress => {
                let pane = pane?;
                if self.controller.model().pane(pane)?.kind() != neptune_model::PaneKind::Browser {
                    return None;
                }
                Action::BrowserAddress(pane)
            }
            BrowserReload | BrowserBack | BrowserForward | BrowserDevTools => {
                let pane = pane?;
                let item = self.controller.model().pane(pane)?;
                if item.kind() != neptune_model::PaneKind::Browser {
                    return None;
                }
                let generation = item.generation();
                let target = crate::runtime::browser::protocol::Target {
                    pane: pane.get(),
                    generation,
                };
                use crate::runtime::browser::protocol::Command as BrowserCommand;
                let command = match binding {
                    BrowserReload => BrowserCommand::Reload { target },
                    BrowserBack => BrowserCommand::Back { target },
                    BrowserForward => BrowserCommand::Forward { target },
                    _ => BrowserCommand::DevTools { target },
                };
                Action::Browser(pane, generation, command)
            }
            SplitRight => Action::Split(pane?, neptune_model::Axis::Vertical),
            SplitBelow => Action::Split(pane?, neptune_model::Axis::Horizontal),
            ClosePane => Action::ClosePane(pane?),
            CloseWorkspace => Action::CloseWorkspace(workspace?.id()),
            Find => Action::Find,
            CommandPalette => Action::Palette,
            ToggleSidebar => Action::ToggleSidebar,
            ToggleRightPanel => Action::Panel(ui::panel::Event::Toggle),
            ZoomPane => Action::Zoom,
            Copy => Action::Copy(pane?),
            CopyHints => Action::CopyHints(pane?),
            Paste => Action::Paste(pane?),
            Preferences => Action::Settings,
            NextTab | PreviousTab => {
                let workspace = workspace?;
                Action::Focus(
                    workspace
                        .layout()
                        .next_tab(workspace.active(), binding == NextTab)?,
                )
            }
            NextWorkspace | PreviousWorkspace => {
                let workspaces = self.controller.model().workspaces();
                if workspaces.is_empty() {
                    return None;
                }
                let current = workspaces
                    .iter()
                    .position(|workspace| {
                        Some(workspace.id()) == self.controller.model().active_workspace()
                    })
                    .unwrap_or(0);
                let index = if binding == NextWorkspace {
                    (current + 1) % workspaces.len()
                } else {
                    (current + workspaces.len() - 1) % workspaces.len()
                };
                Action::SelectWorkspace(workspaces[index].id())
            }
            ZoomIn => Action::ZoomUiIn,
            ZoomOut => Action::ZoomUiOut,
            ResetZoom => Action::ResetUiZoom,
            IncreaseFontSize => Action::IncreaseFontSize,
            DecreaseFontSize => Action::DecreaseFontSize,
            ResetFontSize => Action::ResetFontSize,
            ClearScrollback => Action::Clear(pane?),
            RestartPane => Action::Restart(pane?),
            BackgroundPane => Action::Background(pane?),
            Notifications => Action::Notifications,
            ShowFiles => Action::Panel(ui::panel::Event::Show(ui::panel::Tab::Files)),
            ShowAgents => Action::Panel(ui::panel::Event::Show(ui::panel::Tab::Agents)),
            ShowProject => Action::Panel(ui::panel::Event::Show(ui::panel::Tab::Project)),
            BrowseThemes => Action::Themes,
            _ => return None,
        })
    }
}

/// Remove the chord's press/repeat/release and its paired text/clipboard event.
/// An ordinary character later in the same frame keeps its own input ownership.
fn consume_binding(
    events: &mut Vec<egui::Event>,
    original: &egui::Event,
    key: egui::Key,
    physical: Option<egui::Key>,
    modifiers: egui::Modifiers,
) {
    let mut paired = false;
    events.retain(|event| match event {
        egui::Event::Key {
            key: value,
            physical_key,
            modifiers: m,
            pressed,
            ..
        } => {
            let matched = *value == key && *physical_key == physical && *m == modifiers;
            paired = matched && *pressed;
            !matched
        }
        egui::Event::Text(_) => {
            let keep = !paired;
            paired = false;
            keep
        }
        egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) => {
            let keep = !(paired || event == original);
            paired = false;
            keep
        }
        _ => {
            paired = false;
            true
        }
    });
}
