use super::*;
use crate::input::{InputAction, PointerButton, PointerEvent, RoutingContext, WheelUnit};
impl App {
    pub(super) fn terminal_input(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        context: RoutingContext,
        swallowed_paste: Option<egui::Modifiers>,
    ) {
        let Some(id) = self.controller.model().active_pane() else {
            return;
        };
        if !matches!(context, RoutingContext::TerminalPane(_)) {
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
        // The toolkit keeps a paste chord to itself when the clipboard has no
        // text. Ctrl+V still belongs to the program, which may read a picture
        // from the clipboard itself; the host chord pastes the picture's path.
        let terminal_chord =
            swallowed_paste.filter(|chord| chord.ctrl && !chord.shift && !chord.mac_cmd);
        if swallowed_paste.is_some() && terminal_chord.is_none() {
            self.paste_clipboard(ctx, id, true);
        }
        let Some(session) = self.sessions.get(id) else {
            return;
        };
        let Some(pane) = self.renders.get_mut(&id) else {
            return;
        };
        let mode = session.modes();
        let events = ctx.input(|i| i.events.clone());
        let mut normalized = crate::input::normalize_events(&events, ctx.input(|i| i.modifiers));
        if let Some(chord) = terminal_chord {
            normalized.insert(
                0,
                crate::input::InputEvent::Key {
                    key: terminal_core::input::Key::V,
                    physical_key: None,
                    modifiers: crate::input::modifiers(chord),
                    pressed: true,
                    repeat: false,
                },
            );
        }
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
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        self.shortcuts_with_keymap(ctx, crate::platform::keyboard::unshifted_zoom_key);
    }

    pub(super) fn shortcuts_with_keymap(
        &mut self,
        ctx: &egui::Context,
        mut unshifted_key: impl FnMut(egui::Key) -> Option<egui::Key>,
    ) {
        let mut actions = Vec::new();
        for event in ctx.input(|i| i.events.clone()) {
            let egui::Event::Key {
                key,
                physical_key,
                pressed,
                modifiers: m,
                ..
            } = event
            else {
                continue;
            };
            let direction = if m.ctrl && m.shift && !m.alt && !m.mac_cmd {
                match key {
                    egui::Key::ArrowLeft => Some(neptune_model::FocusDirection::Left),
                    egui::Key::ArrowRight => Some(neptune_model::FocusDirection::Right),
                    egui::Key::ArrowUp => Some(neptune_model::FocusDirection::Up),
                    egui::Key::ArrowDown => Some(neptune_model::FocusDirection::Down),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(direction) = direction {
                // Apply earlier shortcuts before checking ownership and capturing
                // the target. Repeated arrows advance from the new focus.
                for action in actions.drain(..) {
                    self.action(ctx, action);
                }
                let terminal_owns_keys = self.ui.overlay == OverlayState::None
                    && !egui::Popup::is_any_open(ctx)
                    && ctx.memory(|memory| {
                        memory.focused().is_none() || memory.focused() == self.terminal_focus
                    });
                if terminal_owns_keys && self.controller.model().active_pane().is_some() {
                    if pressed
                        && let Some(workspace) = self
                            .controller
                            .model()
                            .active_workspace()
                            .and_then(|id| self.controller.model().workspace(id))
                        && let Some(target) =
                            workspace.layout().adjacent(workspace.active(), direction)
                    {
                        self.action(ctx, Action::Focus(target));
                    }
                    // Consume the chord even at an outer edge, including releases
                    // that a terminal using the Kitty protocol could otherwise see.
                    ctx.input_mut(|input| {
                        input.events.retain(|event| {
                            !matches!(event, egui::Event::Key { key: value, modifiers, .. }
                                if *value == key && *modifiers == m)
                        });
                    });
                }
                continue;
            }
            let zoom_key = if m.shift && !m.alt && (m.ctrl ^ m.mac_cmd) {
                physical_key.and_then(&mut unshifted_key).unwrap_or(key)
            } else {
                key
            };
            if let Some(action) = zoom_shortcut(zoom_key, m, &self.config) {
                if pressed {
                    for pending in actions.drain(..) {
                        self.action(ctx, pending);
                    }
                    self.action(ctx, action);
                }
                ctx.input_mut(|input| {
                    // Consume the original event, including releases, rather
                    // than the labeled key recovered from the keyboard layout.
                    // Text belongs to the immediately preceding key press;
                    // ordinary symbols typed later in this frame must survive.
                    let mut shortcut_text = false;
                    input.events.retain(|event| match event {
                        egui::Event::Key {
                            key: value,
                            physical_key: physical,
                            pressed,
                            modifiers,
                            ..
                        } => {
                            let matched =
                                *value == key && *physical == physical_key && *modifiers == m;
                            shortcut_text = matched && *pressed;
                            !matched
                        }
                        egui::Event::Text(text) => {
                            let consume = shortcut_text
                                && match zoom_key {
                                    egui::Key::Plus | egui::Key::Equals => {
                                        matches!(text.as_str(), "+" | "=" | "*")
                                    }
                                    egui::Key::Minus => matches!(text.as_str(), "-" | "_"),
                                    egui::Key::Num0 => matches!(text.as_str(), "0" | ")" | "="),
                                    _ => false,
                                };
                            shortcut_text = false;
                            !consume
                        }
                        _ => {
                            shortcut_text = false;
                            true
                        }
                    });
                });
                continue;
            }
            if !pressed {
                continue;
            }
            if key == egui::Key::Escape {
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
                if self.ui.overlay == OverlayState::Settings && egui::Popup::is_any_open(ctx) {
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
                    self.ui.search_open = false;
                    self.search_task = None;
                } else if self.explorer_escape(ctx) {
                    // A name being typed, a search or its field was left.
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
            let command = if cfg!(target_os = "macos") {
                m.mac_cmd
            } else {
                m.ctrl && m.shift
            };
            let pane = self.controller.model().active_pane();
            let action = if command {
                match key {
                    egui::Key::T => Some(pane.map_or(Action::New, Action::NewTab)),
                    egui::Key::N => Some(Action::New),
                    egui::Key::PageDown | egui::Key::PageUp => self
                        .controller
                        .model()
                        .active_workspace()
                        .and_then(|id| self.controller.model().workspace(id))
                        .and_then(|workspace| {
                            let forward = key == egui::Key::PageDown;
                            workspace.layout().next_tab(workspace.active(), forward)
                        })
                        .map(Action::Focus),
                    egui::Key::D => pane.map(|id| Action::Split(id, neptune_model::Axis::Vertical)),
                    egui::Key::E => {
                        pane.map(|id| Action::Split(id, neptune_model::Axis::Horizontal))
                    }
                    egui::Key::W => pane.map(Action::ClosePane),
                    egui::Key::F => Some(Action::Find),
                    egui::Key::P => Some(Action::Palette),
                    egui::Key::B => Some(Action::ToggleSidebar),
                    egui::Key::O => Some(Action::Panel(ui::panel::Event::Toggle)),
                    egui::Key::Enter => Some(Action::Zoom),
                    egui::Key::C => pane.map(Action::Copy),
                    _ => None,
                }
            } else {
                None
            };
            // Workspaces are numbered in sidebar order. With Shift held the
            // logical key is a symbol, so the digit comes from the physical key.
            let action = action.or_else(|| {
                let index = workspace_digit(physical_key.unwrap_or(key)).filter(|_| command)?;
                let workspace = self.controller.model().workspaces().get(index)?;
                Some(Action::SelectWorkspace(workspace.id()))
            });
            if let Some(action) = action {
                actions.push(action);
                ctx.input_mut(|i| {
                    i.consume_key(m, key);
                });
            }
            if (m.command || m.ctrl) && key == egui::Key::Comma {
                actions.push(Action::Settings);
                ctx.input_mut(|i| {
                    i.consume_key(m, key);
                });
            }
            if m.ctrl && key == egui::Key::Tab {
                let workspaces = self.controller.model().workspaces();
                if !workspaces.is_empty() {
                    let current = workspaces
                        .iter()
                        .position(|w| Some(w.id()) == self.controller.model().active_workspace())
                        .unwrap_or(0);
                    let index = if m.shift {
                        (current + workspaces.len() - 1) % workspaces.len()
                    } else {
                        (current + 1) % workspaces.len()
                    };
                    actions.push(Action::SelectWorkspace(workspaces[index].id()));
                    ctx.input_mut(|i| {
                        i.consume_key(m, key);
                    });
                }
            }
        }
        for action in actions {
            self.action(ctx, action);
        }
    }
}

fn zoom_shortcut(key: egui::Key, modifiers: egui::Modifiers, current: &Config) -> Option<Action> {
    use egui::Key;
    if modifiers.alt
        || !(modifiers.ctrl || modifiers.mac_cmd)
        || (modifiers.ctrl && modifiers.mac_cmd)
    {
        return None;
    }
    if modifiers.shift {
        let font_size = match key {
            Key::Plus | Key::Equals => (current.font_size + 1.0).min(32.0),
            Key::Minus => (current.font_size - 1.0).max(9.0),
            Key::Num0 => Config::default().font_size,
            _ => return None,
        };
        return Some(Action::Preferences(Config {
            font_size,
            ..current.clone()
        }));
    }
    match key {
        Key::Plus | Key::Equals => Some(Action::ZoomUiIn),
        Key::Minus if !modifiers.shift => Some(Action::ZoomUiOut),
        Key::Num0 if !modifiers.shift => Some(Action::ResetUiZoom),
        _ => None,
    }
}

fn workspace_digit(key: egui::Key) -> Option<usize> {
    use egui::Key::*;
    [Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9]
        .iter()
        .position(|digit| *digit == key)
}
