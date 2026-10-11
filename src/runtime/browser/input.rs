use super::{
    Browsers,
    protocol::{Command, Mouse, Target},
};
use eframe::egui::{self, Event, Key, Modifiers};
use neptune_model::PaneId;

impl Browsers {
    pub fn input(
        &mut self,
        target: Target,
        rect: egui::Rect,
        events: &[Event],
        keyboard: bool,
        modifiers: Modifiers,
        pointer: Option<egui::Pos2>,
    ) -> Result<(), &'static str> {
        let pane = PaneId::new(target.pane);
        let Some(entry) = self
            .entries
            .get_mut(&pane)
            .filter(|e| e.generation == target.generation && !e.failed && e.state.error.is_none())
        else {
            return Ok(());
        };
        let mut buttons = entry.buttons;
        let mut inside = entry.pointer_inside;
        let mut click = entry.click;
        let mut wheel_rest = entry.wheel;
        let mut commands = Vec::new();
        // Input proves that this web widget owns the interaction. Reconcile
        // focus before keys/clicks, including the first frame after a native
        // focus change; CEF ignores keyboard shortcuts on an unfocused widget.
        if entry.focused != Some(true)
            && events.iter().any(|event| match event {
                Event::Key { pressed: true, .. }
                | Event::Text(_)
                | Event::Paste(_)
                | Event::Ime(_) => keyboard,
                Event::PointerButton {
                    pos, pressed: true, ..
                } => rect.contains(*pos),
                _ => false,
            })
        {
            commands.push(Command::Focus {
                target,
                focused: true,
            });
        }
        for event in events {
            let command = match event {
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    modifiers,
                    ..
                } if keyboard => key_code(*key).map(|(code, native)| Command::Key {
                    target,
                    code,
                    native: native_code(physical_key.unwrap_or(*key), code, native, *pressed),
                    pressed: *pressed,
                    modifiers: flags(*modifiers),
                }),
                Event::Text(text) if keyboard => Some(Command::Text {
                    target,
                    text: text.clone(),
                }),
                Event::Paste(text) if keyboard => Some(Command::Paste {
                    target,
                    text: text.clone(),
                }),
                Event::Copy if keyboard => Some(Command::Copy { target }),
                Event::Cut if keyboard => Some(Command::Cut { target }),
                Event::Ime(egui::ImeEvent::Preedit { text, .. }) if keyboard => {
                    Some(Command::Ime {
                        target,
                        text: text.clone(),
                        commit: false,
                    })
                }
                Event::Ime(egui::ImeEvent::Commit(text)) if keyboard => Some(Command::Ime {
                    target,
                    text: text.clone(),
                    commit: true,
                }),
                Event::PointerMoved(position)
                    if rect.contains(*position) || buttons != 0 || inside =>
                {
                    inside = rect.contains(*position);
                    Some(Command::Mouse {
                        target,
                        x: (position.x - rect.left()) as i32,
                        y: (position.y - rect.top()) as i32,
                        modifiers: flags(modifiers) | buttons,
                        kind: Mouse::Move {
                            leave: !inside && buttons == 0,
                        },
                    })
                }
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } if rect.contains(*pos) || !pressed && buttons != 0 => {
                    let (button, flag) = match button {
                        egui::PointerButton::Primary => (0, 16),
                        egui::PointerButton::Middle => (1, 32),
                        egui::PointerButton::Secondary => (2, 64),
                        _ => continue,
                    };
                    if *pressed {
                        buttons |= flag;
                    } else {
                        buttons &= !flag;
                    }
                    let clicks = if *pressed {
                        let now = std::time::Instant::now();
                        let count = click
                            .filter(|(last, when, position, _)| {
                                *last == button
                                    && now.duration_since(*when).as_millis() < 400
                                    && position.distance(*pos) < 5.0
                            })
                            .map_or(1, |(_, _, _, count)| count % 3 + 1);
                        click = Some((button, now, *pos, count));
                        count
                    } else {
                        click.map_or(1, |(_, _, _, count)| count)
                    };
                    Some(Command::Mouse {
                        target,
                        x: (pos.x - rect.left()) as i32,
                        y: (pos.y - rect.top()) as i32,
                        modifiers: flags(*modifiers) | buttons,
                        kind: Mouse::Button {
                            button,
                            pressed: *pressed,
                            clicks,
                        },
                    })
                }
                Event::PointerGone if inside && buttons == 0 => {
                    inside = false;
                    Some(Command::Mouse {
                        target,
                        x: 0,
                        y: 0,
                        modifiers: flags(modifiers),
                        kind: Mouse::Move { leave: true },
                    })
                }
                Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } if pointer.is_some_and(|pos| rect.contains(pos)) => {
                    let (moved, precise) = wheel(*unit, *delta, rect.height());
                    // Chromium takes whole pixels. What a slow touchpad
                    // stroke moves short of one is kept for the next event
                    // rather than lost.
                    let whole = (wheel_rest + moved).round();
                    wheel_rest += moved - whole;
                    let position = pointer.unwrap_or(rect.min) - rect.min;
                    (whole != egui::Vec2::ZERO).then(|| Command::Mouse {
                        target,
                        x: position.x as i32,
                        y: position.y as i32,
                        modifiers: flags(*modifiers) | if precise { PRECISE_SCROLL } else { 0 },
                        kind: Mouse::Wheel {
                            dx: whole.x as i32,
                            dy: whole.y as i32,
                        },
                    })
                }
                _ => None,
            };
            if let Some(command) = command {
                commands.push(command);
            }
        }
        entry.buttons = buttons;
        entry.wheel = wheel_rest;
        entry.pointer_inside = inside;
        entry.click = click;
        for command in commands {
            let focuses = matches!(command, Command::Focus { focused: true, .. });
            self.send(command)?;
            if focuses && let Some(entry) = self.entries.get_mut(&pane) {
                entry.focused = Some(true);
            }
        }
        Ok(())
    }
}

/// The points one notch of a wheel scrolls a page, as Chromium's own
/// windows count it.
const NOTCH: f32 = 53.0;
/// `cef_event_flags_t`: the wheel deltas are a touchpad's own pixels, to
/// follow as they come rather than to animate as the notches of a wheel.
const PRECISE_SCROLL: u32 = 1 << 14;

/// What a wheel event moves a page, in points, and whether it is a
/// touchpad's own distance. A touchpad is followed as it comes; a wheel's
/// notches are left to Chromium to ease.
fn wheel(unit: egui::MouseWheelUnit, delta: egui::Vec2, page: f32) -> (egui::Vec2, bool) {
    match unit {
        egui::MouseWheelUnit::Point => (delta, true),
        egui::MouseWheelUnit::Line => (delta * NOTCH, false),
        egui::MouseWheelUnit::Page => (delta * page, false),
    }
}

fn flags(modifiers: Modifiers) -> u32 {
    // cef_event_flags_t: Shift, Control, Alt and platform command.
    (u32::from(modifiers.shift) << 1)
        | (u32::from(modifiers.ctrl) << 2)
        | (u32::from(modifiers.alt) << 3)
        | (u32::from(modifiers.mac_cmd) << 7)
}
fn key_code(key: Key) -> Option<(i32, i32)> {
    let (code, keysym) = match key {
        Key::Backspace => (8, 0xff08),
        Key::Tab => (9, 0xff09),
        Key::Enter => (13, 0xff0d),
        Key::Escape => (27, 0xff1b),
        Key::Space => (32, 32),
        Key::PageUp => (33, 0xff55),
        Key::PageDown => (34, 0xff56),
        Key::End => (35, 0xff57),
        Key::Home => (36, 0xff50),
        Key::ArrowLeft => (37, 0xff51),
        Key::ArrowUp => (38, 0xff52),
        Key::ArrowRight => (39, 0xff53),
        Key::ArrowDown => (40, 0xff54),
        Key::Insert => (45, 0xff63),
        Key::Delete => (46, 0xffff),
        Key::Num0 => (48, 48),
        Key::Num1 => (49, 49),
        Key::Num2 => (50, 50),
        Key::Num3 => (51, 51),
        Key::Num4 => (52, 52),
        Key::Num5 => (53, 53),
        Key::Num6 => (54, 54),
        Key::Num7 => (55, 55),
        Key::Num8 => (56, 56),
        Key::Num9 => (57, 57),
        Key::Minus => (189, 45),
        Key::Equals => (187, 61),
        Key::Plus => (187, 43),
        Key::OpenBracket => (219, 91),
        Key::CloseBracket => (221, 93),
        Key::Backslash => (220, 92),
        Key::IntlBackslash => (226, 92),
        Key::Semicolon => (186, 59),
        Key::Quote => (222, 39),
        Key::Comma => (188, 44),
        Key::Period => (190, 46),
        Key::Slash => (191, 47),
        Key::Backtick => (192, 96),
        Key::A => (65, 97),
        Key::B => (66, 98),
        Key::C => (67, 99),
        Key::D => (68, 100),
        Key::E => (69, 101),
        Key::F => (70, 102),
        Key::G => (71, 103),
        Key::H => (72, 104),
        Key::I => (73, 105),
        Key::J => (74, 106),
        Key::K => (75, 107),
        Key::L => (76, 108),
        Key::M => (77, 109),
        Key::N => (78, 110),
        Key::O => (79, 111),
        Key::P => (80, 112),
        Key::Q => (81, 113),
        Key::R => (82, 114),
        Key::S => (83, 115),
        Key::T => (84, 116),
        Key::U => (85, 117),
        Key::V => (86, 118),
        Key::W => (87, 119),
        Key::X => (88, 120),
        Key::Y => (89, 121),
        Key::Z => (90, 122),
        Key::F1 => (112, 0xffbe),
        Key::F2 => (113, 0xffbf),
        Key::F3 => (114, 0xffc0),
        Key::F4 => (115, 0xffc1),
        Key::F5 => (116, 0xffc2),
        Key::F6 => (117, 0xffc3),
        Key::F7 => (118, 0xffc4),
        Key::F8 => (119, 0xffc5),
        Key::F9 => (120, 0xffc6),
        Key::F10 => (121, 0xffc7),
        Key::F11 => (122, 0xffc8),
        Key::F12 => (123, 0xffc9),
        _ => return None,
    };
    Some((
        code,
        if cfg!(target_os = "linux") {
            keysym
        } else {
            code
        },
    ))
}

// Platform native codes supplement the portable Windows virtual-key code.
#[cfg(target_os = "macos")]
fn native_code(physical: Key, _code: i32, _keysym: i32, _pressed: bool) -> i32 {
    crate::platform::keyboard::browser_keycode(physical).unwrap_or(0)
}
#[cfg(windows)]
fn native_code(_physical: Key, code: i32, _keysym: i32, pressed: bool) -> i32 {
    #[link(name = "user32")]
    unsafe extern "system" {
        #[link_name = "MapVirtualKeyW"]
        fn map_virtual_key(code: u32, kind: u32) -> u32;
    }
    // CEF expects the WM_KEYDOWN/UP LPARAM, including the extended scan bit.
    let scan = unsafe { map_virtual_key(code as u32, 4) };
    let extended = u32::from(scan & 0xff00 != 0) << 24;
    let release = if pressed { 0 } else { 3 << 30 };
    ((scan & 0xff) << 16 | extended | release | 1) as i32
}
#[cfg(not(any(target_os = "macos", windows)))]
fn native_code(_physical: Key, _code: i32, keysym: i32, _pressed: bool) -> i32 {
    keysym
}
