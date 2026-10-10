//! Search for the focused pane, without covering content or resizing it.
use super::helpers::{bare_text_edit, place};
use super::{Action, UiState};
use crate::{
    icons::{self, Icon},
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Id, Layout, Pos2, Rect, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo,
    WidgetType,
};

/// The search field's stable identity; input routing checks it for focus.
pub fn input_id() -> Id {
    Id::new("terminal-search-input")
}

pub fn show(
    ui: &mut Ui,
    field: Rect,
    p: Palette,
    kind: neptune_model::PaneKind,
    state: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let focused = ui.memory(|memory| memory.has_focus(input_id()));
    let radius = metrics::CONTROL_RADIUS;
    let painter = ui.painter().clone();
    painter.rect_filled(field, radius, p.control);
    if focused {
        painter.rect_stroke(
            field.expand(2.5),
            radius.saturating_add(2),
            Stroke::new(3.0, theme::tint(p.accent, 0.28)),
            StrokeKind::Inside,
        );
        painter.rect_stroke(
            field,
            radius,
            Stroke::new(1.0, p.accent),
            StrokeKind::Inside,
        );
    }
    // The field owns the pointer over its whole surface, so a press between
    // its controls never starts a window drag.
    ui.interact(
        field,
        ui.id().with("terminal-search-surface"),
        egui::Sense::click(),
    );
    icons::paint(
        &painter,
        Rect::from_center_size(
            Pos2::new(field.left() + 16.0, field.center().y),
            Vec2::splat(13.0),
        ),
        Icon::Search,
        p.muted,
    );
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("terminal-search")
            .max_rect(Rect::from_min_max(
                Pos2::new(field.left() + 30.0, field.top()),
                Pos2::new(field.right() - 1.0, field.bottom()),
            ))
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if icons::button_with_hint(ui, Icon::Close, "Close search", "Esc").clicked() {
                actions.push(Action::CloseSearch);
            }
            if icons::button_with_hint(ui, Icon::ArrowDown, "Next match", "Enter").clicked() {
                actions.push(Action::FindNext { reverse: false });
                // Pressing a control took focus from the field; Enter must
                // keep meaning "next match", not reach the shell.
                state.search_focus = true;
            }
            if icons::button_with_hint(ui, Icon::ArrowUp, "Previous match", "Shift+Enter").clicked()
            {
                actions.push(Action::FindNext { reverse: true });
                state.search_focus = true;
            }
            ui.add_space(4.0);
            if let Some(status) = &state.search_error {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(status)
                            .font(theme::regular(11.5))
                            .color(p.muted),
                    )
                    .truncate()
                    .selectable(false),
                );
                ui.add_space(6.0);
            }
            let input = ui.available_rect_before_wrap();
            let response = place(
                ui,
                input,
                Layout::left_to_right(Align::Center),
                "terminal-search-field",
                |ui| {
                    bare_text_edit(
                        ui,
                        input_id(),
                        &mut state.search,
                        match kind {
                            neptune_model::PaneKind::Terminal => "Find in scrollback",
                            neptune_model::PaneKind::Browser => "Find in page",
                        },
                        12.5,
                        input.width(),
                    )
                },
            );
            response.widget_info(|| {
                WidgetInfo::labeled(
                    WidgetType::TextEdit,
                    true,
                    match kind {
                        neptune_model::PaneKind::Terminal => "Terminal search",
                        neptune_model::PaneKind::Browser => "Page search",
                    },
                )
            });
            if state.search_focus {
                response.request_focus();
                state.search_focus = false;
            }
            if response.changed() {
                actions.push(Action::SearchChanged);
            }
            // A single-line field releases focus on Enter; searching keeps it.
            if (response.has_focus() || response.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
            {
                actions.push(Action::FindNext {
                    reverse: ui.input(|input| input.modifiers.shift),
                });
                response.request_focus();
            }
        },
    );
}
