//! The composer the creation form and a project's chat are written in: one
//! surface that holds the field and, at its foot, a row of controls. The
//! row leads with what the lead runs on, its CLI's mark beside the model,
//! then its effort, each opening its choices in place; the controls that
//! attach and send end it. A press on the surface outside its controls
//! writes in the field.
use super::settings::{Pick, choice_list};
use super::{Choices, Custom, Setting, choice_label, model_id};
use crate::{
    icons::{self, Icon},
    projects::settings::model_name,
    theme::{self, Palette},
    ui::{
        agents::kind_name,
        chat::{self, Editor},
        helpers::{
            bare_text_edit, elided, field_surface, focus_ring, galley_at, menu_item, menu_layout,
            menu_separator, place, viewport_id,
        },
    },
};
use eframe::egui::{
    self, Align, Color32, CursorIcon, FontId, Id, Key, Layout, Pos2, Rect, RectAlign, Response,
    Sense, Ui, Vec2, WidgetInfo, WidgetType, vec2,
};
use neptune_model::{AgentKind, ProjectId};

/// The row of controls at the surface's foot, with the room over it.
const BAR: f32 = 42.0;
/// The surface's corner: its controls' corner and their inset from its edge.
const RADIUS: u8 = 14;
const PILL_RADIUS: u8 = 6;
const INSET: f32 = 8.0;
/// A control of the row.
const PILL: f32 = 26.0;
/// The round controls at the trailing edge, set in from the corner so that
/// they sit on the row's line.
pub(super) const ROUND: f32 = 28.0;
/// The room over the words, and beside them: they begin where the marks
/// of the row do.
const TOP: f32 = 12.0;
const SIDE: f32 = INSET + PAD;
/// A control's room around its words, its CLI's mark and its chevron.
const PAD: f32 = 8.0;
const MARK: f32 = 14.0;
const CHEVRON: f32 = 14.0;
/// The fewest points of words a control keeps: fewer say nothing.
const LEAST: f32 = 28.0;
/// The hairline between two controls, with its room.
const DIVIDER: f32 = 9.0;
/// The width of the lists the row opens.
const LIST: f32 = 200.0;

/// The surface's height for `text` in a surface of `width`, the field
/// showing between `rows` lines.
pub(super) fn height(ui: &Ui, text: &str, width: f32, rows: (usize, usize)) -> f32 {
    // The field's own height less its padding, laid out as wide as here.
    chat::editor_height(ui, text, width - 2.0 * SIDE + 20.0, rows) - 14.0 + TOP + BAR
}
/// The least the surface takes: a line to write on and its row.
pub(super) const LEAST_HEIGHT: f32 = 17.0 + TOP + BAR;

/// Paints the surface in `rect` and the field in it. Returns the field's
/// response and whether Enter asked for its words to be used.
pub(super) fn surface(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    enabled: bool,
    editor: Editor,
) -> (Response, bool) {
    let id = editor.id;
    field_surface(ui, p, rect, id, (true, RADIUS));
    if enabled {
        // Under every control: it takes only what they leave, and is not a
        // stop of its own for the keyboard.
        let body = ui.interact(rect, id.with("composer-surface"), Sense::CLICK);
        if body.clicked() {
            let field = viewport_id(ui.ctx(), id);
            ui.memory_mut(|memory| memory.request_focus(field));
        }
        body.on_hover_cursor(CursorIcon::Text);
    }
    let words = Rect::from_min_max(
        rect.min + vec2(SIDE, TOP),
        Pos2::new(rect.right() - SIDE, rect.bottom() - BAR),
    );
    ui.add_enabled_ui(enabled, |ui| chat::editor_in(ui, words, editor))
        .inner
}

/// Where the round controls at the trailing edge are: the one that sends,
/// then the one beside it.
pub(super) fn controls(card: Rect) -> (Rect, Rect) {
    let send = Rect::from_center_size(
        Pos2::new(card.right() - INSET - ROUND * 0.5, line(card)),
        Vec2::splat(ROUND),
    );
    (send, send.translate(vec2(-(ROUND + 2.0), 0.0)))
}
/// The row's middle line.
fn line(card: Rect) -> f32 {
    card.bottom() - INSET - ROUND * 0.5
}
/// Where a mark of the row is centred, from the surface's leading edge.
pub(super) const MARK_CENTRE: f32 = INSET + PAD + MARK * 0.5;

/// Who leads, as the row says it.
#[derive(Clone, Copy)]
pub(super) enum Who<'a> {
    /// No CLI can lead yet; the words say why.
    Missing(&'a str),
    /// The CLI picked to lead the next project, among those installed here.
    Pick(AgentKind, &'a [AgentKind]),
    /// A project's lead, which stays its own.
    Kept(AgentKind),
}

pub(super) struct Bar<'a> {
    pub who: Who<'a>,
    /// Whose settings these are: none for the next project.
    pub owner: Option<ProjectId>,
    pub choices: Option<&'a Choices>,
    /// The model and effort chosen; none leaves each to the CLI.
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    /// The model its CLI named when it last started, which stands in for
    /// one that is not chosen.
    pub reported: Option<&'a str>,
    /// How many round controls end the row.
    pub rounds: usize,
    /// Its lists open over the row rather than under it.
    pub above: bool,
    /// An input method is composing: Enter belongs to it.
    pub composing: bool,
}

/// What the row was told.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Told {
    pub lead: Option<AgentKind>,
    pub model: Option<Option<String>>,
    pub effort: Option<Option<String>>,
    /// The project's settings were asked for.
    pub settings: bool,
}

/// The widths of the model's and the effort's controls in `room`, and
/// whether the hairline stands between them: it goes first, then the two
/// narrow alike, each keeping a few letters.
fn fit([model, effort]: [f32; 2], least: [f32; 2], room: f32) -> ([f32; 2], bool) {
    if model + DIVIDER + effort <= room {
        return ([model, effort], true);
    }
    let room = room - 2.0;
    if model + effort <= room {
        return ([model, effort], false);
    }
    // As wide as the narrower is where both can be; past that, alike.
    let cap = if model.min(effort) * 2.0 <= room {
        room - model.min(effort)
    } else {
        room * 0.5
    };
    (
        [model.min(cap).max(least[0]), effort.min(cap).max(least[1])],
        false,
    )
}

/// The width of a control for `words`, with a CLI's mark before them and a
/// chevron after them where it opens a list.
fn width(ui: &Ui, words: &str, font: FontId, mark: bool, opens: bool) -> f32 {
    let words = ui
        .painter()
        .layout_no_wrap(words.to_owned(), font, Color32::PLACEHOLDER)
        .size()
        .x;
    chrome(mark, opens) + words
}
fn chrome(mark: bool, opens: bool) -> f32 {
    2.0 * PAD + if mark { MARK + 6.0 } else { 0.0 } + if opens { CHEVRON } else { 0.0 }
}

/// A control of the row: a CLI's mark, its words and a chevron where it
/// opens a list. It rests in the secondary ink and is lit under the
/// pointer and while its list is open. `name` is its accessible name.
fn pill(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    id: Id,
    (mark, words, font): (Option<AgentKind>, &str, FontId),
    name: &str,
    opens: bool,
) -> Response {
    let id = viewport_id(ui.ctx(), id);
    let live = opens && ui.is_enabled();
    let response = ui.interact(rect, id, if live { Sense::click() } else { Sense::hover() });
    response.widget_info(|| {
        WidgetInfo::labeled(
            if opens {
                WidgetType::ComboBox
            } else {
                WidgetType::Label
            },
            live,
            name,
        )
    });
    let open =
        opens && egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    let lit = live && (response.hovered() || response.has_focus() || open);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if live && (open || response.is_pointer_button_down_on()) {
            painter.rect_filled(rect, PILL_RADIUS, p.pressed);
        } else if live && response.hovered() {
            painter.rect_filled(rect, PILL_RADIUS, p.hover);
        }
        if response.has_focus() {
            focus_ring(painter, rect, PILL_RADIUS, p);
        }
        let ink = if lit { p.fg } else { p.secondary };
        let mut x = rect.left() + PAD;
        if let Some(kind) = mark {
            let at = Rect::from_center_size(
                Pos2::new(x + MARK * 0.5, rect.center().y),
                Vec2::splat(MARK),
            );
            icons::paint_provider(painter, at, kind, ink);
            x += MARK + 6.0;
        }
        let room = rect.right() - PAD - if opens { CHEVRON } else { 0.0 } - x;
        let shown = galley_at(
            painter,
            Pos2::new(x, rect.center().y),
            elided(painter, words, font, ink, room.max(0.0)),
        );
        if opens {
            icons::paint(
                painter,
                Rect::from_center_size(
                    Pos2::new(shown.right() + 4.0 + 5.0, rect.center().y + 0.5),
                    Vec2::splat(10.0),
                ),
                Icon::ChevronDown,
                if lit { p.fg } else { p.muted },
            );
        }
    }
    if live {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}

/// A muted line that heads a list, as a section label heads its rows.
fn heading(ui: &mut Ui, p: Palette, text: &str) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 24.0));
    galley_at(
        ui.painter(),
        Pos2::new(rect.left() + 10.0, rect.center().y + 1.0),
        elided(
            ui.painter(),
            text,
            theme::medium(11.5),
            p.muted,
            rect.width() - 20.0,
        ),
    );
}

/// A CLI that can lead, as a row of a list of choices: the check while it
/// leads, then its mark and name. The hovered row takes the accent.
fn lead_row(ui: &mut Ui, p: Palette, kind: AgentKind, checked: bool) -> Response {
    let name = kind_name(kind);
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 28.0));
    let response = ui.interact(rect, ui.id().with(("lead-row", name)), Sense::click());
    response.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, checked, name));
    let painter = ui.painter();
    let ink = if response.hovered() || response.has_focus() {
        painter.rect_filled(rect, 6, p.accent);
        p.on_accent
    } else {
        p.fg
    };
    if checked {
        icons::paint(
            painter,
            Rect::from_center_size(
                Pos2::new(rect.left() + 15.0, rect.center().y),
                Vec2::splat(14.0),
            ),
            Icon::Check,
            ink,
        );
    }
    icons::paint_provider(
        painter,
        Rect::from_center_size(
            Pos2::new(rect.left() + 32.0 + 7.0, rect.center().y),
            Vec2::splat(MARK),
        ),
        kind,
        ink,
    );
    galley_at(
        painter,
        Pos2::new(rect.left() + 32.0 + MARK + 7.0, rect.center().y),
        elided(
            painter,
            name,
            theme::regular(13.0),
            ink,
            rect.width() - 42.0 - MARK - 7.0,
        ),
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// The row at the foot of the surface in `card`: what the lead runs on and
/// with. The model typed for a lead (`custom`) takes the row's place while
/// it is written. Returns what the row was told.
pub(super) fn bar(
    ui: &mut Ui,
    p: Palette,
    card: Rect,
    bar: Bar,
    custom: &mut Option<Custom>,
) -> Told {
    let mut told = Told::default();
    let centre = line(card);
    let left = card.left() + INSET;
    let rounds = bar.rounds as f32;
    let right = card.right() - INSET - rounds * ROUND - (rounds - 1.0).max(0.0) * 2.0 - 6.0;
    let row = Rect::from_min_max(
        Pos2::new(left, centre - PILL * 0.5),
        Pos2::new(right.max(left), centre + PILL * 0.5),
    );
    let kind = match bar.who {
        Who::Missing(words) => {
            galley_at(
                ui.painter(),
                Pos2::new(left + PAD, centre),
                elided(
                    ui.painter(),
                    words,
                    theme::regular(12.0),
                    p.secondary,
                    row.width() - PAD,
                ),
            );
            return told;
        }
        Who::Pick(kind, _) | Who::Kept(kind) => kind,
    };
    if let Some(typed) = custom
        .as_mut()
        .filter(|typed| typed.owner == bar.owner && typed.setting == Setting::Lead)
    {
        if typed_model(ui, p, row, &bar, typed, &mut told) {
            *custom = None;
        }
        return told;
    }
    let Some(choices) = bar.choices else {
        pill(
            ui,
            p,
            Rect::from_min_size(
                row.min,
                vec2(
                    width(ui, kind_name(kind), theme::medium(12.0), true, false).min(row.width()),
                    PILL,
                ),
            ),
            Id::new(("project-composer-lead", bar.owner)),
            (Some(kind), kind_name(kind), theme::medium(12.0)),
            "Lead",
            false,
        );
        return told;
    };

    // The model chosen, or the one its CLI named, or else the CLI itself,
    // which runs on its own choice.
    let model = bar
        .model
        .or(bar.reported)
        .map(|model| choice_label(Some(choices), model, true));
    let model_words = model.as_deref().unwrap_or(kind_name(kind));
    let effort = bar
        .effort
        .map(|effort| choice_label(Some(choices), effort, false));
    let effort_words = effort.as_deref().unwrap_or("Effort");
    let model_font = theme::medium(12.0);
    let effort_font = theme::medium(12.0);
    let ([model_width, effort_width], divided) = fit(
        [
            width(ui, model_words, model_font.clone(), true, true),
            width(ui, effort_words, effort_font.clone(), false, true),
        ],
        [chrome(true, true) + LEAST, chrome(false, true) + LEAST],
        row.width(),
    );
    let align = if bar.above {
        RectAlign::TOP_START
    } else {
        RectAlign::BOTTOM_START
    };

    let at = Rect::from_min_size(row.min, vec2(model_width, PILL));
    let response = pill(
        ui,
        p,
        at,
        Id::new(("project-composer-model", bar.owner)),
        (Some(kind), model_words, model_font),
        "Lead model",
        true,
    );
    let tip = match bar.who {
        Who::Pick(_, among) if among.len() > 1 => "Choose the CLI that leads and its model",
        _ => "Choose the lead's model",
    };
    let response = if egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response))
    {
        response
    } else {
        response.on_hover_text(format!("{} · {tip}", kind_name(kind)))
    };
    let listed = bar
        .model
        .is_none_or(|value| choices.models.iter().any(|choice| choice.value == value));
    let mut picked = None;
    // A row that is pressed closes it, but the CLI picked leaves it open
    // on its models.
    egui::Popup::menu(&response)
        .align(align)
        .gap(4.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            menu_layout(ui, LIST);
            // Where more than one CLI can lead, it is picked first; the list
            // stays open on the models of the one picked.
            if let Who::Pick(_, among) = bar.who
                && among.len() > 1
            {
                heading(ui, p, "Lead");
                for other in among {
                    if lead_row(ui, p, *other, *other == kind).clicked() && *other != kind {
                        told.lead = Some(*other);
                    }
                }
                menu_separator(ui, p);
            }
            heading(ui, p, "Model");
            picked = choice_list(
                ui,
                p,
                ("Default", "What your CLI is set to"),
                bar.model,
                &choices.models,
                Some(!listed),
            );
            if bar.owner.is_some() {
                menu_separator(ui, p);
                if menu_item(ui, p, Icon::Settings, "Lead settings…", "", false) {
                    told.settings = true;
                    ui.close();
                }
            }
        });
    match picked {
        Some(Pick::Value(model)) => told.model = Some(model),
        Some(Pick::Custom) => {
            // What is in force and in no list is what the field starts with.
            *custom = Some(Custom {
                owner: bar.owner,
                setting: Setting::Lead,
                text: bar.model.filter(|_| !listed).unwrap_or_default().to_owned(),
                focus: true,
            });
        }
        None => {}
    }

    let mut x = at.right();
    if divided {
        ui.painter().vline(
            x + DIVIDER * 0.5,
            (centre - 7.0)..=(centre + 7.0),
            p.hairline(),
        );
        x += DIVIDER;
    } else {
        x += 2.0;
    }
    let at = Rect::from_min_size(Pos2::new(x, row.top()), vec2(effort_width, PILL));
    let response = pill(
        ui,
        p,
        at,
        Id::new(("project-composer-effort", bar.owner)),
        (None, effort_words, effort_font),
        "Lead effort",
        true,
    );
    let response = if egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response))
    {
        response
    } else {
        response.on_hover_text("Choose how hard the lead thinks")
    };
    egui::Popup::menu(&response)
        .align(align)
        .gap(4.0)
        .show(|ui| {
            menu_layout(ui, LIST);
            heading(ui, p, "Effort");
            if let Some(Pick::Value(effort)) = choice_list(
                ui,
                p,
                ("Default", "What your CLI is set to"),
                bar.effort,
                &choices.efforts,
                None,
            ) {
                told.effort = Some(effort);
            }
        });
    told
}

/// The field a model no list names is typed in, over the row: Enter uses
/// what was typed where a CLI takes it as a name, and the field given up
/// gives the row back. Returns whether it is done with.
fn typed_model(
    ui: &mut Ui,
    p: Palette,
    row: Rect,
    bar: &Bar,
    typed: &mut Custom,
    told: &mut Told,
) -> bool {
    let id = model_id(Setting::Lead);
    let named = model_name(&typed.text).map(str::to_owned);
    let valid = typed.text.trim().is_empty() || named.is_some();
    field_surface(ui, p, row, id, (valid, PILL_RADIUS));
    let inner = row.shrink2(vec2(PAD, 0.0));
    let response = place(
        ui,
        inner,
        Layout::left_to_right(Align::Center),
        ("project-composer-custom", bar.owner),
        |ui| bare_text_edit(ui, id, &mut typed.text, "Model id", 12.5, inner.width()),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, "Lead model id"));
    if typed.focus && !ui.is_sizing_pass() {
        response.request_focus();
        typed.focus = false;
    }
    // One line: a break that was pasted does not stay.
    if typed.text.contains(['\n', '\r']) {
        typed.text = typed.text.replace(['\n', '\r'], " ");
    }
    if !response.lost_focus() {
        return false;
    }
    let entered = !bar.composing && ui.input(|input| input.key_pressed(Key::Enter));
    match named {
        Some(model) if entered => {
            // Choosing what is in force already changes nothing.
            told.model = Some(Some(model)).filter(|model| model.as_deref() != bar.model);
            true
        }
        // A name no CLI takes is not used, and the field stays to mend it.
        None if entered && !typed.text.trim().is_empty() => {
            response.request_focus();
            false
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hairline_goes_first_then_both_narrow_alike_keeping_a_few_letters() {
        let least = [50.0, 40.0];
        assert_eq!(fit([90.0, 60.0], least, 200.0), ([90.0, 60.0], true));
        assert_eq!(fit([90.0, 60.0], least, 155.0), ([90.0, 60.0], false));
        // The narrower keeps its width while the wider gives way.
        assert_eq!(fit([90.0, 60.0], least, 142.0), ([80.0, 60.0], false));
        assert_eq!(fit([90.0, 60.0], least, 102.0), ([50.0, 50.0], false));
        assert_eq!(fit([90.0, 60.0], least, 60.0), ([50.0, 40.0], false));
    }
}
