//! Quiet subscription summaries in the content footer, with one upward popover.
use super::{
    Action,
    helpers::{self, ButtonKind},
};
use crate::{
    icons::{self, Icon},
    runtime::usage::{self, Provider, Reading, Unavailable, Usage, Window},
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align2, Frame, Galley, Margin, Pos2, Rect, RectAlign, Sense, Stroke, Ui, Vec2,
    WidgetInfo, WidgetType,
};
use std::sync::Arc;

/// One allowance per line: name, track, percentage used and time to reset.
const ROW: f32 = 22.0;
const PERCENT: f32 = 38.0;
const RESET: f32 = 56.0;

pub fn button_id(ui: &Ui) -> egui::Id {
    ui.id().with("subscription-usage")
}

fn ink(p: Palette, used: f64) -> Option<egui::Color32> {
    if used >= 100.0 {
        Some(p.red)
    } else if used >= 80.0 {
        Some(p.attention)
    } else {
        None
    }
}
pub fn popup_id() -> egui::Id {
    egui::Id::new("subscription-usage-menu")
}

fn track(painter: &egui::Painter, rect: Rect, p: Palette, used: f64, fill: egui::Color32) {
    painter.rect_filled(rect, 2, p.control);
    if used > 0.0 {
        let width = (rect.width() * (used.min(100.0) / 100.0) as f32).max(rect.height());
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(width, rect.height())),
            2,
            fill,
        );
    }
}

/// Painted text keeps a label role and its whole wording for assistive
/// technology and inspection, also where it is drawn elided.
fn describe(ui: &Ui, rect: Rect, id: impl egui::AsIdSalt, text: &str) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(id), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, text));
    response
}

/// One provider in the footer: its name, most consumed allowance and a track.
struct Summary {
    name: Arc<Galley>,
    percent: Arc<Galley>,
    stale: Option<Arc<Galley>>,
    used: f64,
    fill: egui::Color32,
}
impl Summary {
    fn width(&self) -> f32 {
        self.name.size().x
            + 5.0
            + self.percent.size().x
            + 6.0
            + self.stale.as_ref().map_or(18.0, |stale| stale.size().x)
    }
}

pub fn footer(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    usage: &Usage,
    enabled: bool,
    actions: &mut Vec<Action>,
) {
    let font = theme::medium(11.5);
    let layout = |text: String, color| ui.painter().layout_no_wrap(text, font.clone(), color);
    let summaries: Vec<_> = usage
        .readings
        .iter()
        .filter_map(|reading| {
            let used = reading.summary()?.used_percent;
            let stale = reading.unavailable.is_some();
            let fill = if stale {
                p.muted
            } else {
                ink(p, used).unwrap_or(p.secondary)
            };
            Some(Summary {
                name: layout(reading.provider.name().into(), p.muted),
                percent: layout(format!("{used:.0}%"), fill),
                // A reading that could not be renewed says so in place of its track.
                stale: stale.then(|| layout("stale".into(), p.muted)),
                used,
                fill,
            })
        })
        .collect();
    let width = summaries.iter().map(Summary::width).sum::<f32>()
        + 14.0 * summaries.len().saturating_sub(1) as f32
        + 20.0;
    let compact = summaries.is_empty() || width > rect.width() - 24.0;
    let fallback = layout("AI usage".into(), p.secondary);
    let width = if compact {
        fallback.size().x + 42.0
    } else {
        width
    };
    let height = metrics::CONTROL_HEIGHT - 4.0;
    let button = Rect::from_min_size(
        Pos2::new(rect.right() - 10.0 - width, rect.center().y - height * 0.5),
        Vec2::new(width, height),
    );
    let response = ui.interact(
        button,
        button_id(ui),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, "Subscription usage"));
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id());
    if response.hovered() || open || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            button,
            metrics::CONTROL_RADIUS,
            if response.is_pointer_button_down_on() {
                p.pressed
            } else {
                p.hover
            },
        );
    }
    if response.has_focus() {
        helpers::focus_ring(ui.painter(), button, metrics::CONTROL_RADIUS, p);
    }
    let y = button.center().y;
    if compact {
        icons::paint(
            ui.painter(),
            Rect::from_center_size(Pos2::new(button.left() + 17.0, y), Vec2::splat(15.0)),
            Icon::Agents,
            p.secondary,
        );
        helpers::galley_at(ui.painter(), Pos2::new(button.left() + 31.0, y), fallback);
    } else {
        let mut left = button.left() + 10.0;
        for summary in summaries {
            let next = left + summary.width() + 14.0;
            left = helpers::galley_at(ui.painter(), Pos2::new(left, y), summary.name).right() + 5.0;
            left =
                helpers::galley_at(ui.painter(), Pos2::new(left, y), summary.percent).right() + 6.0;
            match summary.stale {
                Some(stale) => {
                    helpers::galley_at(ui.painter(), Pos2::new(left, y), stale);
                }
                None => track(
                    ui.painter(),
                    Rect::from_min_size(Pos2::new(left, y - 1.5), Vec2::new(18.0, 3.0)),
                    p,
                    summary.used,
                    summary.fill,
                ),
            }
            left = next;
        }
    }
    let summary_hint = usage
        .readings
        .iter()
        .filter_map(|reading| {
            let window = reading.summary()?;
            Some(format!(
                "{} · {}: {:.0}% used{}",
                reading.provider.name(),
                window.label,
                window.used_percent,
                if reading.unavailable.is_some() {
                    " · stale"
                } else {
                    ""
                }
            ))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if summary_hint.is_empty() {
            "Subscription usage"
        } else {
            &summary_hint
        });
    if !enabled {
        return;
    }
    let bounds = ui.ctx().content_rect();
    egui::Popup::menu(&response)
        .id(popup_id())
        .align(RectAlign::TOP_END)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .gap(6.0)
        .width((bounds.width() - 40.0).clamp(160.0, 316.0))
        .frame(
            Frame::new()
                .fill(p.elevated)
                .corner_radius(metrics::PANE_RADIUS)
                .stroke(Stroke::new(1.0, p.border))
                .shadow(p.popup_shadow())
                .inner_margin(Margin {
                    left: 12,
                    right: 12,
                    top: 7,
                    bottom: 9,
                }),
        )
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            // The popover remembers its size; without room beyond it the list
            // could not grow when readings arrive while it is open.
            ui.set_max_height((bounds.height() - 80.0).max(104.0));
            header(ui, p, usage, actions);
            let measured = usage.readings.iter().any(|r| !r.windows.is_empty());
            // Every provider shares one grid, so tracks and figures line up.
            let name = usage
                .readings
                .iter()
                .flat_map(|reading| &reading.windows)
                .map(|window| {
                    ui.painter()
                        .layout_no_wrap(window.label.clone(), theme::regular(12.0), p.secondary)
                        .size()
                        .x
                })
                .fold(0.0, f32::max)
                .min(ui.available_width() * 0.4)
                .ceil();
            if measured {
                let (row, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), Sense::hover());
                for (x, text) in [(row.right() - RESET, "Used"), (row.right(), "Resets in")] {
                    let area = ui.painter().text(
                        Pos2::new(x, row.center().y),
                        Align2::RIGHT_CENTER,
                        text,
                        theme::medium(10.5),
                        p.muted,
                    );
                    describe(ui, area, text, text);
                }
            }
            egui::ScrollArea::vertical()
                .id_salt("subscription-usage-details")
                .max_height((bounds.height() - 124.0).max(60.0))
                .show(ui, |ui| {
                    for (index, reading) in usage.readings.iter().enumerate() {
                        if index > 0 {
                            ui.add_space(5.0);
                            let (line, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 1.0),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(line, 0, p.separator);
                            ui.add_space(4.0);
                        }
                        provider(ui, p, reading, usage.refreshing, name, actions);
                    }
                });
        });
    if open {
        // Ages and reset countdowns change only while the popover is visible.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(30));
    }
}

/// Title, freshness of the readings and the one control.
fn header(ui: &mut Ui, p: Palette, usage: &Usage, actions: &mut Vec<Action>) {
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
    // The glyph, not its target, sits on the content's trailing edge.
    let target = Rect::from_center_size(
        Pos2::new(row.right() - 8.0, row.center().y),
        Vec2::splat(28.0),
    );
    let response = ui.interact(
        target,
        ui.id().with("refresh"),
        if usage.refreshing {
            Sense::hover()
        } else {
            Sense::click()
        },
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, !usage.refreshing, "Refresh"));
    if !usage.refreshing && (response.hovered() || response.is_pointer_button_down_on()) {
        ui.painter().rect_filled(
            target.shrink(2.0),
            metrics::CONTROL_RADIUS - 2,
            if response.is_pointer_button_down_on() {
                p.pressed
            } else {
                p.hover
            },
        );
    }
    if response.has_focus() {
        helpers::focus_ring(
            ui.painter(),
            target.shrink(2.0),
            metrics::CONTROL_RADIUS - 2,
            p,
        );
    }
    icons::paint(
        ui.painter(),
        Rect::from_center_size(target.center(), Vec2::splat(15.0)),
        Icon::Refresh,
        if usage.refreshing {
            p.muted
        } else {
            p.secondary
        },
    );
    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Refresh usage")
        .clicked()
    {
        actions.push(Action::RefreshUsage);
    }
    let title = ui.painter().text(
        row.left_center(),
        Align2::LEFT_CENTER,
        "Subscription usage",
        theme::semibold(13.0),
        p.fg,
    );
    describe(ui, title, "title", "Subscription usage");
    // The oldest reading still shown as current; stale ones carry their own age.
    let oldest = usage
        .readings
        .iter()
        .filter(|reading| reading.unavailable.is_none())
        .filter_map(|reading| reading.checked_at?.elapsed().ok())
        .max();
    let status = if usage.refreshing {
        "Checking accounts…".into()
    } else if let Some(age) = oldest {
        format!("Updated {}", ago(age.as_secs()))
    } else {
        return;
    };
    let galley = helpers::elided(
        ui.painter(),
        &status,
        theme::regular(11.0),
        p.muted,
        target.left() - 2.0 - title.right() - 10.0,
    );
    let area = Rect::from_min_size(
        Pos2::new(
            target.left() - 2.0 - galley.size().x,
            row.center().y - galley.size().y * 0.5,
        ),
        galley.size(),
    );
    // Exposed whole even where a narrow window leaves it no room.
    describe(ui, area.expand(1.0), "status", &status);
    ui.painter().galley(area.min, galley, p.muted);
}

fn provider(
    ui: &mut Ui,
    p: Palette,
    reading: &Reading,
    refreshing: bool,
    name: f32,
    actions: &mut Vec<Action>,
) {
    ui.push_id(reading.provider.name(), |ui| {
        let stale = !reading.windows.is_empty() && reading.unavailable.is_some();
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
        let title = ui.painter().text(
            row.left_center(),
            Align2::LEFT_CENTER,
            reading.provider.name(),
            theme::semibold(12.5),
            p.fg,
        );
        // What the header's shared freshness does not cover for this provider.
        let note = if stale {
            let age = reading
                .checked_at
                .and_then(|at| at.elapsed().ok())
                .map_or("unknown age".into(), |age| ago(age.as_secs()));
            Some((format!("Stale · {age}"), p.attention))
        } else if reading.windows.is_empty() && reading.unavailable.is_none() {
            Some((
                if refreshing {
                    "Checking…"
                } else {
                    "Not checked"
                }
                .into(),
                p.muted,
            ))
        } else {
            None
        };
        let mut right = row.right();
        if let Some((text, color)) = note {
            let area = ui.painter().text(
                row.right_center(),
                Align2::RIGHT_CENTER,
                &text,
                theme::regular(11.0),
                color,
            );
            right = area.left() - 8.0;
            let response = describe(
                ui,
                area,
                "note",
                &format!("{}: {text}", reading.provider.name()),
            );
            if stale {
                response
                    .on_hover_text("The last refresh failed. These figures may be out of date.");
            }
        }
        let mut heading = reading.provider.name().to_owned();
        let mut area = title;
        if let Some(plan) = &reading.plan {
            heading = format!("{heading} · {plan}");
            let plan = helpers::elided(
                ui.painter(),
                plan,
                theme::regular(11.0),
                p.muted,
                right - title.right() - 6.0,
            );
            // Both sizes share a baseline rather than a centre.
            area = area.union(helpers::galley_at(
                ui.painter(),
                Pos2::new(title.right() + 6.0, row.center().y + 0.5),
                plan,
            ));
        }
        describe(ui, area, "heading", &heading);
        if reading.windows.is_empty() {
            let Some(state) = reading.unavailable else {
                return;
            };
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.label(
                egui::RichText::new(state.message(reading.provider))
                    .font(theme::regular(11.5))
                    .color(p.secondary),
            );
            ui.add_space(4.0);
            if state == Unavailable::Keychain {
                ui.add_space(4.0);
                if helpers::button(ui, p, "Allow Cursor Keychain", ButtonKind::Secondary).clicked()
                {
                    actions.push(Action::EnableCursorUsage);
                }
                ui.add_space(4.0);
            }
            return;
        }
        for (index, window) in reading.windows.iter().enumerate() {
            allowance(ui, p, reading.provider, index, window, stale, name);
        }
    });
}

fn allowance(
    ui: &mut Ui,
    p: Palette,
    provider: Provider,
    index: usize,
    window: &Window,
    stale: bool,
    name: f32,
) {
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::hover());
    let now = usage::epoch();
    let (reset, spoken, due) = match window.resets_at {
        Some(at) if at <= now => ("due".into(), "reset due, refresh to update".into(), true),
        Some(at) => {
            let left = wait(at - now);
            (left.clone(), format!("resets in {left}"), false)
        }
        None => ("—".into(), "reset time unavailable".into(), false),
    };
    // Each allowance names its provider: the heading is a separate node.
    let summary = format!(
        "{} · {}: {:.0}% used, {spoken}",
        provider.name(),
        window.label,
        window.used_percent
    );
    describe(ui, row, ("allowance", index), &summary).on_hover_text(&summary);
    let y = row.center().y;
    let figure = row.right() - RESET;
    let bar = Rect::from_min_max(
        Pos2::new(row.left() + name + 10.0, y - 2.0),
        Pos2::new(figure - PERCENT, y + 2.0),
    );
    // Too narrow for a track: the name takes its room instead.
    let fits = bar.width() >= 24.0;
    let label = helpers::elided(
        ui.painter(),
        &window.label,
        theme::regular(12.0),
        p.secondary,
        if fits {
            name
        } else {
            figure - PERCENT - row.left()
        },
    );
    helpers::galley_at(ui.painter(), row.left_center(), label);
    let warning = ink(p, window.used_percent).filter(|_| !stale);
    if fits {
        track(
            ui.painter(),
            bar,
            p,
            window.used_percent,
            warning.unwrap_or(if stale { p.muted } else { p.secondary }),
        );
    }
    ui.painter().text(
        Pos2::new(figure, y),
        Align2::RIGHT_CENTER,
        format!("{:.0}%", window.used_percent),
        theme::medium(12.0),
        warning.unwrap_or(if stale { p.secondary } else { p.fg }),
    );
    ui.painter().text(
        Pos2::new(row.right(), y),
        Align2::RIGHT_CENTER,
        reset,
        theme::regular(11.0),
        if due { p.attention } else { p.muted },
    );
}

fn ago(seconds: u64) -> String {
    if seconds < 60 {
        "just now".into()
    } else {
        format!("{} ago", wait(seconds))
    }
}

fn wait(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60);
    if minutes >= 1440 {
        format!("{}d {}h", minutes / 1440, (minutes % 1440) / 60)
    } else if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usage_popover_is_contained_in_wide_and_short_windows() {
        for size in [
            Vec2::new(1000.0, 700.0),
            Vec2::new(640.0, 400.0),
            Vec2::new(300.0, 280.0),
        ] {
            let ctx = egui::Context::default();
            let config = crate::config::Config::default();
            theme::fonts(&ctx);
            theme::apply(&ctx, &config);
            let mut usage = Usage::default();
            for reading in &mut usage.readings {
                reading.windows = vec![
                    Window {
                        label: "Weekly · A model with a long allowance name".into(),
                        used_percent: 83.0,
                        resets_at: Some(usage::epoch() + 3600)
                    };
                    12
                ];
            }
            egui::Popup::open_id(&ctx, popup_id());
            for _ in 0..4 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ui| {
                        let rect = Rect::from_min_max(
                            Pos2::new(0.0, size.y - metrics::TOOLBAR_HEIGHT),
                            Pos2::new(size.x, size.y),
                        );
                        footer(
                            ui,
                            rect,
                            Palette::for_config(&config),
                            &usage,
                            true,
                            &mut Vec::new(),
                        );
                    },
                );
                output.textures_delta.clear();
            }
            let rect = ctx.memory(|memory| memory.area_rect(popup_id())).unwrap();
            assert!(
                rect.left() >= 0.0
                    && rect.top() >= 0.0
                    && rect.right() <= size.x
                    && rect.bottom() <= size.y - 20.0,
                "{size:?}: {rect:?}"
            );
        }
    }

    /// Labels a narrow, loading popover exposes: painted text keeps its role,
    /// provider and whole wording where it is drawn elided or not at all.
    #[test]
    fn painted_usage_text_stays_labelled_by_provider_in_a_narrow_window() {
        let size = Vec2::new(220.0, 280.0);
        let ctx = egui::Context::default();
        let config = crate::config::Config::default();
        theme::fonts(&ctx);
        theme::apply(&ctx, &config);
        ctx.enable_accesskit();
        let mut usage = Usage::default();
        usage.refreshing = true;
        usage.readings[0].plan = Some("Plus".into());
        usage.readings[0].checked_at = Some(std::time::SystemTime::now());
        usage.readings[0].windows = vec![Window {
            label: "Weekly".into(),
            used_percent: 26.0,
            resets_at: Some(usage::epoch() + 3600),
        }];
        usage.readings[1].unavailable = Some(Unavailable::Failed);
        usage.readings[1].windows = vec![Window {
            label: "Session".into(),
            used_percent: 83.0,
            resets_at: Some(usage::epoch() - 60),
        }];
        egui::Popup::open_id(&ctx, popup_id());
        let mut labels = Vec::new();
        for _ in 0..4 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    let rect = Rect::from_min_max(
                        Pos2::new(0.0, size.y - metrics::TOOLBAR_HEIGHT),
                        Pos2::new(size.x, size.y),
                    );
                    footer(
                        ui,
                        rect,
                        Palette::for_config(&config),
                        &usage,
                        true,
                        &mut Vec::new(),
                    );
                },
            );
            output.textures_delta.clear();
            labels = output
                .platform_output
                .accesskit_update
                .iter()
                .flat_map(|update| &update.nodes)
                // A label is read by its value, a control by its name.
                .filter_map(|(_, node)| {
                    Some((node.role(), node.label().or(node.value())?.to_owned()))
                })
                .collect();
        }
        use egui::accesskit::Role;
        for (role, label) in [
            (Role::Button, "Subscription usage"),
            (Role::Button, "Refresh"),
            (Role::Label, "Subscription usage"),
            (Role::Label, "Checking accounts…"),
            (Role::Label, "Used"),
            (Role::Label, "Resets in"),
            (Role::Label, "Codex · Plus"),
            (Role::Label, "Codex · Weekly: 26% used, resets in 1h 0m"),
            (Role::Label, "Claude Code"),
            (
                Role::Label,
                "Claude Code · Session: 83% used, reset due, refresh to update",
            ),
            (Role::Label, "Cursor"),
            (Role::Label, "Cursor: Checking…"),
        ] {
            assert!(
                labels.contains(&(role, label.to_owned())),
                "{role:?} {label:?} missing from {labels:#?}"
            );
        }
        assert!(
            labels
                .iter()
                .any(|(_, label)| label.starts_with("Claude Code: Stale · ")),
            "{labels:#?}"
        );
    }

    /// Readings that arrive while the popover is open are shown whole without
    /// waiting for another event: the app sleeps between them.
    #[test]
    fn open_popover_grows_for_arriving_readings_without_further_input() {
        let size = Vec2::new(1000.0, 700.0);
        let ctx = egui::Context::default();
        let config = crate::config::Config::default();
        theme::fonts(&ctx);
        theme::apply(&ctx, &config);
        let mut usage = Usage::default();
        usage.refreshing = true;
        egui::Popup::open_id(&ctx, popup_id());
        let frame = |usage: &Usage| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    let rect = Rect::from_min_max(
                        Pos2::new(0.0, size.y - metrics::TOOLBAR_HEIGHT),
                        Pos2::new(size.x, size.y),
                    );
                    footer(
                        ui,
                        rect,
                        Palette::for_config(&config),
                        usage,
                        true,
                        &mut Vec::new(),
                    );
                },
            );
            output.textures_delta.clear();
            let repaint = output
                .viewport_output
                .values()
                .any(|viewport| viewport.repaint_delay.is_zero());
            (
                ctx.memory(|memory| memory.area_rect(popup_id())).unwrap(),
                repaint,
            )
        };
        for _ in 0..4 {
            frame(&usage);
        }
        usage.refreshing = false;
        for reading in &mut usage.readings {
            reading.windows = vec![
                Window {
                    label: "Weekly".into(),
                    used_percent: 30.0,
                    resets_at: None,
                };
                3
            ];
        }
        let mut frames = 0;
        let rect = loop {
            let (rect, repaint) = frame(&usage);
            frames += 1;
            if !repaint {
                break rect;
            }
            assert!(frames < 6, "The popover keeps repainting");
        };
        // A title, the column captions, three headings and nine allowances.
        assert!(rect.height() > 28.0 + 16.0 + 12.0 * ROW, "{rect:?}");
        // It stays above the footer's control and inside the window.
        let control = size.y - (metrics::TOOLBAR_HEIGHT + metrics::CONTROL_HEIGHT - 4.0) * 0.5;
        assert!(rect.top() >= 0.0 && rect.bottom() < control, "{rect:?}");
    }
}
