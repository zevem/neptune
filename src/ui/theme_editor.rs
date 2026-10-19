//! The custom theme editor: an isolated draft with a live preview.
//!
//! Nothing reaches the configuration before Save, and a changed draft asks
//! before it is thrown away.
use super::helpers::{
    ButtonKind, bare_text_edit, button, caption, field_frame, focus_ring, group, padded, place,
    section_label, sheet_back_header, sheet_footer, text_field,
};
use super::theme_browser::{GUTTER, footer_note, preview, tile_outline};
use crate::{
    config::{Appearance, Config, Theme},
    terminal_theme::{CustomTheme, HexColor, MAX_CUSTOM_THEMES, MAX_NAME_CHARS, ThemeColors},
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Color32, CursorIcon, Id, Layout, Painter, Pos2, Rect, Sense, Stroke, StrokeKind,
    Ui, Vec2, WidgetInfo, WidgetType, ecolor::Hsva, vec2,
};

const WELL: f32 = 26.0;
const HEX_WIDTH: f32 = 84.0;
/// A colour well and its hex field.
const CELL: f32 = WELL + 6.0 + HEX_WIDTH;
const CELL_GAP: f32 = 10.0;

/// An unsaved custom theme. Nothing reaches the configuration before Save.
pub(super) struct Draft {
    id: String,
    /// Editing a saved theme rather than creating one.
    saved: bool,
    name: String,
    fields: [String; 23],
    /// The last usable value of every field; drives the preview.
    colors: ThemeColors,
    original: (String, ThemeColors),
    /// The open picker's field and colour. Kept as HSV so a grey keeps its hue.
    picking: Option<(usize, Hsva)>,
    /// A changed draft asks before it is thrown away.
    pub(super) discarding: bool,
    /// What the configuration rejected on the last save.
    error: Option<String>,
    focus_name: bool,
}

impl Draft {
    fn new(id: String, saved: bool, name: String, colors: ThemeColors) -> Self {
        Self {
            id,
            saved,
            fields: colors.values().map(String::from),
            colors,
            original: (name.clone(), colors),
            name,
            picking: None,
            discarding: false,
            error: None,
            focus_name: true,
        }
    }

    /// Editing a saved custom theme in place.
    pub(super) fn edit(theme: &CustomTheme) -> Self {
        Self::new(theme.id.clone(), true, theme.name.clone(), theme.colors)
    }

    /// A new custom theme that starts from `colors`.
    pub(super) fn copy(config: &Config, base: &str, colors: ThemeColors) -> Self {
        let number = (1..=MAX_CUSTOM_THEMES as u64 + 1)
            .find(|n| {
                !config
                    .custom_themes
                    .iter()
                    .any(|t| t.id == format!("custom:{n}"))
            })
            .unwrap_or(1);
        Self::new(
            format!("custom:{number}"),
            false,
            unused_name(config, base),
            colors,
        )
    }

    /// A new theme the user has already renamed.
    #[cfg(test)]
    pub(super) fn changed(config: &Config) -> Self {
        let colors = super::theme_browser::colors_of(&Palette::for_config(config));
        let mut draft = Self::copy(config, "My theme", colors);
        draft.name = "Renamed".into();
        draft
    }

    pub(super) fn is_changed(&self) -> bool {
        self.name.trim() != self.original.0
            || self
                .fields
                .iter()
                .zip(self.original.1.values())
                .any(|(text, value)| parse_hex(text) != Some(value))
    }

    /// Why the draft cannot be saved as it stands.
    fn problem(&self, config: &Config) -> Option<String> {
        let name = self.name.trim().to_lowercase();
        if name.is_empty() {
            return Some("Name the theme to save it.".into());
        }
        if config
            .custom_themes
            .iter()
            .any(|t| t.id != self.id && t.name.trim().to_lowercase() == name)
        {
            return Some("Another custom theme already has this name.".into());
        }
        self.fields
            .iter()
            .position(|text| parse_hex(text).is_none())
            .map(|index| format!("{}: use #RRGGBB.", ThemeColors::LABELS[index]))
            .or_else(|| self.error.clone())
    }

    /// Saves the draft as the theme chosen for `target`: a mode's, or the
    /// single theme for none. The configuration's own rules decide; nothing is
    /// applied when they reject it.
    fn save(&mut self, config: &mut Config, target: Option<Appearance>) -> bool {
        let candidate = CustomTheme {
            id: self.id.clone(),
            name: self.name.trim().to_owned(),
            colors: self.colors,
        };
        let mut updated = config.clone();
        *updated.theme_for_mut(target) = Theme::Palette(candidate.id.clone());
        match updated
            .custom_themes
            .iter_mut()
            .find(|t| t.id == candidate.id)
        {
            Some(existing) => *existing = candidate,
            None => updated.custom_themes.push(candidate),
        }
        match updated.validate() {
            Ok(()) => {
                *config = updated;
                true
            }
            Err(error) => {
                self.error = Some(error.to_string());
                false
            }
        }
    }
}

/// What a hex field means. The `#` is optional and `#RGB` is accepted while
/// typing; saved themes always hold `#RRGGBB`.
fn parse_hex(text: &str) -> Option<HexColor> {
    let text = text.trim();
    let hex = text.strip_prefix('#').unwrap_or(text);
    if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 => {
            let short = u32::from_str_radix(hex, 16).ok()?;
            let channel = |shift: u32| ((short >> shift) & 0xf) * 0x11;
            Some(HexColor(
                (channel(8) << 16) | (channel(4) << 8) | channel(0),
            ))
        }
        6 => u32::from_str_radix(hex, 16).ok().map(HexColor),
        _ => None,
    }
}

/// `base`, numbered when a custom theme already has that name.
fn unused_name(config: &Config, base: &str) -> String {
    let taken = |name: &str| {
        config
            .custom_themes
            .iter()
            .any(|t| t.name.trim().to_lowercase() == name.to_lowercase())
    };
    let base: String = base.chars().take(MAX_NAME_CHARS - 4).collect();
    if !taken(&base) {
        return base;
    }
    (2..=MAX_CUSTOM_THEMES + 2)
        .map(|n| format!("{base} {n}"))
        .find(|name| !taken(name))
        .unwrap_or(base)
}

/// What a frame of the editor left behind.
#[derive(Debug, PartialEq)]
pub(super) enum Outcome {
    Editing,
    /// The draft was saved and applied.
    Saved,
    /// The draft was dropped: unchanged, or discarded on request.
    Left,
}

/// Draws the editor inside the Preferences sheet: header, a body
/// `body_height` tall and the action bar. Also returns whether the sheet was
/// asked to close.
pub(super) fn show(
    ui: &mut Ui,
    p: Palette,
    config: &mut Config,
    draft: &mut Draft,
    target: Option<Appearance>,
    body_height: f32,
) -> (Outcome, bool) {
    let title = if draft.saved {
        "Edit theme"
    } else {
        "New theme"
    };
    let (back, close) = sheet_back_header(ui, p, title, "Back to themes", "Close preferences");
    let width = ui.available_width();
    let (_, body) = ui.allocate_space(vec2(width, body_height));
    let colors = |ui: &mut Ui, draft: &mut Draft, content: &mut dyn FnMut(&mut Ui, &mut Draft)| {
        egui::ScrollArea::vertical()
            .id_salt("theme-editor")
            .max_height(body.height())
            .auto_shrink([false, false])
            .show(ui, |ui| content(ui, draft));
    };
    // The preview stays beside the colours while there is room for both.
    if width - GUTTER * 2.0 >= 560.0 {
        let side = Rect::from_min_size(body.min + vec2(GUTTER, 0.0), vec2(232.0, body.height()));
        place(
            ui,
            side,
            Layout::top_down(Align::Min),
            "theme-editor-side",
            |ui| {
                name_field(ui, p, draft);
                ui.add_space(14.0);
                section_label(ui, p, "Preview");
                let height = (side.height() - 154.0).clamp(96.0, 284.0);
                sample(ui, p, draft, height);
                if side.height() >= 300.0 {
                    caption(
                        ui,
                        p,
                        "The toolbar, sidebar and sheets take their surfaces from the background.",
                    );
                }
            },
        );
        place(
            ui,
            Rect::from_min_max(Pos2::new(side.right() + 20.0, body.top()), body.max),
            Layout::top_down(Align::Min),
            "theme-editor-colors",
            |ui| {
                colors(ui, draft, &mut |ui, draft| {
                    // Clear of the scroll bar at the sheet's edge.
                    ui.set_max_width(ui.available_width() - GUTTER);
                    color_groups(ui, p, draft);
                });
            },
        );
    } else {
        place(
            ui,
            body,
            Layout::top_down(Align::Min),
            "theme-editor-column",
            |ui| {
                colors(ui, draft, &mut |ui, draft| {
                    padded(ui, GUTTER, |ui| {
                        name_field(ui, p, draft);
                        ui.add_space(14.0);
                        sample(ui, p, draft, 132.0);
                        ui.add_space(10.0);
                        color_groups(ui, p, draft);
                    });
                });
            },
        );
    }
    // Every usable field feeds the preview; an unusable one keeps its last colour.
    let mut values = draft.colors.values();
    for (value, text) in values.iter_mut().zip(&draft.fields) {
        if let Some(parsed) = parse_hex(text) {
            *value = parsed;
        }
    }
    draft.colors = ThemeColors::from_rgb(values.map(|value| value.0));

    let problem = draft.problem(config);
    let bar = sheet_footer(ui, p);
    let (mut save, mut cancel, mut discard, mut keep) = (false, false, false, false);
    place(
        ui,
        bar,
        Layout::right_to_left(Align::Center),
        "theme-editor-actions",
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if draft.discarding {
                discard = button(ui, p, "Discard changes", ButtonKind::Destructive).clicked();
                keep = button(ui, p, "Keep editing", ButtonKind::Secondary).clicked();
                footer_note(ui, "Discard the changes to this theme?", p.fg);
            } else {
                save = ui
                    .add_enabled_ui(problem.is_none(), |ui| {
                        button(ui, p, "Save theme", ButtonKind::Primary)
                    })
                    .inner
                    .clicked();
                cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
                match &problem {
                    Some(problem) => footer_note(ui, problem, p.red),
                    None => footer_note(
                        ui,
                        "Saving applies it to the window and every terminal.",
                        p.muted,
                    ),
                }
            }
        },
    );

    let outcome = if save && draft.save(config, target) {
        Outcome::Saved
    } else if discard || ((cancel || back) && !draft.is_changed()) {
        Outcome::Left
    } else {
        if keep {
            draft.discarding = false;
        } else if cancel || back {
            draft.discarding = true;
        }
        Outcome::Editing
    };
    (outcome, close)
}

/// The draft as the window would draw it.
fn sample(ui: &mut Ui, p: Palette, draft: &Draft, height: f32) {
    let (_, tile) = ui.allocate_space(vec2(ui.available_width(), height));
    preview(
        ui.painter(),
        tile,
        &Palette::from_colors(draft.colors),
        true,
    );
    tile_outline(ui.painter(), tile, p, false);
}

fn name_field(ui: &mut Ui, p: Palette, draft: &mut Draft) {
    section_label(ui, p, "Name");
    let field = text_field(
        ui,
        p,
        Id::new("custom-theme-name"),
        &mut draft.name,
        "My theme",
        "Theme name",
        ui.available_width(),
    );
    // A sheet measures itself in a hidden first pass, where focus cannot be held.
    if draft.focus_name && ui.is_enabled() && !ui.is_sizing_pass() {
        field.request_focus();
        draft.focus_name = false;
    }
    if field.changed() {
        draft.error = None;
    }
    if draft.name.chars().count() > MAX_NAME_CHARS {
        draft.name = draft.name.chars().take(MAX_NAME_CHARS).collect();
    }
}

fn color_groups(ui: &mut Ui, p: Palette, draft: &mut Draft) {
    let labels = ThemeColors::LABELS;
    let mut rows_of = |ui: &mut Ui, title: &str, range: std::ops::Range<usize>| {
        section_label(ui, p, title);
        group(ui, p, |ui, rows| {
            for index in range {
                rows.row(ui, labels[index], |ui| cells(ui, p, draft, &[index]));
            }
        });
        ui.add_space(14.0);
    };
    rows_of(ui, "Text and background", 0..3);
    rows_of(ui, "Cursor and selection", 3..7);
    // Each colour beside its bright variant, while a row can hold both.
    if ui.available_width() < 14.0 + 70.0 + CELL * 2.0 + CELL_GAP + 12.0 {
        rows_of(ui, "ANSI colors", 7..15);
        rows_of(ui, "Bright ANSI colors", 15..23);
    } else {
        let (_, heading) = ui.allocate_space(vec2(ui.available_width(), 24.0));
        let bright = heading.right() - 12.0 - CELL;
        for (x, text, ink) in [
            (heading.left() + 4.0, "ANSI colors", p.secondary),
            (bright - CELL_GAP - CELL, "Normal", p.muted),
            (bright, "Bright", p.muted),
        ] {
            ui.painter().text(
                Pos2::new(x, heading.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                theme::medium(11.5),
                ink,
            );
        }
        group(ui, p, |ui, rows| {
            for hue in 0..8 {
                rows.row(ui, labels[7 + hue], |ui| {
                    cells(ui, p, draft, &[7 + hue, 15 + hue]);
                });
            }
        });
        ui.add_space(14.0);
    }
    ui.add_space(4.0);
}

/// The wells and hex fields of a row, trailing in it.
fn cells(ui: &mut Ui, p: Palette, draft: &mut Draft, indexes: &[usize]) {
    let count = indexes.len() as f32;
    let (_, rect) = ui.allocate_space(vec2(
        count * CELL + (count - 1.0) * CELL_GAP,
        metrics::CONTROL_HEIGHT,
    ));
    // Laid out leading to trailing, so Tab follows the reading order.
    place(
        ui,
        rect,
        Layout::left_to_right(Align::Center),
        ("theme-color-cells", indexes[0]),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (position, &index) in indexes.iter().enumerate() {
                if position > 0 {
                    ui.add_space(CELL_GAP);
                }
                well(ui, p, draft, index);
                ui.add_space(6.0);
                hex_field(ui, p, draft, index);
            }
        },
    );
}

/// A colour swatch that opens a picker.
fn well(ui: &mut Ui, p: Palette, draft: &mut Draft, index: usize) {
    let label = ThemeColors::LABELS[index];
    let (_, rect) = ui.allocate_space(Vec2::splat(WELL));
    let response = ui.interact(rect, Id::new(("theme-color-well", index)), Sense::click());
    response.widget_info(|| {
        WidgetInfo::labeled(WidgetType::ColorButton, true, format!("{label} color"))
    });
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    let color = theme::color(draft.colors.values()[index].0);
    let painter = ui.painter();
    painter.rect_filled(rect, 7, color);
    painter.rect_stroke(
        rect,
        7,
        Stroke::new(
            1.0,
            if response.hovered() || open {
                p.muted
            } else {
                p.border
            },
        ),
        StrokeKind::Inside,
    );
    if response.has_focus() {
        focus_ring(painter, rect, 7, p);
    }
    if !open && draft.picking.is_some_and(|(picked, _)| picked == index) {
        draft.picking = None;
    }
    egui::Popup::menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .frame(egui::Frame::popup(ui.style()).inner_margin(10))
        .show(|ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let picked = match &mut draft.picking {
                Some((picked, hsva)) if *picked == index => hsva,
                slot => {
                    &mut slot
                        .insert((index, Hsva::from_srgb([color.r(), color.g(), color.b()])))
                        .1
                }
            };
            if picker(ui, p, picked) {
                let [r, g, b] = picked.to_srgb();
                draft.fields[index] = format!("#{r:02x}{g:02x}{b:02x}");
                draft.error = None;
            }
        });
    if response.gained_focus() {
        response.scroll_to_me_animation(None, egui::style::ScrollAnimation::none());
    }
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(label);
}

/// Saturation and brightness on a square with the hue on a strip beneath.
/// Pointer only: the hex field beside the well is the keyboard path.
fn picker(ui: &mut Ui, p: Palette, color: &mut Hsva) -> bool {
    const WIDTH: f32 = 216.0;
    let mut changed = false;
    let handle = |painter: &Painter, at: Pos2, fill: Color32| {
        painter.circle_filled(at, 7.5, Color32::from_black_alpha(70));
        painter.circle_filled(at, 6.5, Color32::WHITE);
        painter.circle_filled(at, 4.5, fill);
    };
    // The meshes have square corners; a ring in the surface colour rounds them.
    let rounded = |painter: &Painter, rect: Rect, radius: u8| {
        painter.rect_stroke(
            rect.expand(4.0),
            radius + 4,
            Stroke::new(4.0, p.elevated),
            StrokeKind::Inside,
        );
        painter.rect_stroke(rect, radius, Stroke::new(1.0, p.border), StrokeKind::Inside);
    };

    let (_, square) = ui.allocate_space(vec2(WIDTH, 148.0));
    let shade = ui.interact(square, ui.id().with("shade"), Sense::click_and_drag());
    if let Some(pointer) = shade.interact_pointer_pos() {
        color.s = ((pointer.x - square.left()) / square.width()).clamp(0.0, 1.0);
        color.v = 1.0 - ((pointer.y - square.top()) / square.height()).clamp(0.0, 1.0);
        changed = true;
    }
    const STEPS: u32 = 16;
    let mut mesh = egui::Mesh::default();
    for y in 0..=STEPS {
        for x in 0..=STEPS {
            let (s, v) = (x as f32 / STEPS as f32, 1.0 - y as f32 / STEPS as f32);
            mesh.colored_vertex(
                square.lerp_inside(vec2(s, 1.0 - v)),
                Hsva::new(color.h, s, v, 1.0).into(),
            );
            if x < STEPS && y < STEPS {
                let corner = y * (STEPS + 1) + x;
                mesh.add_triangle(corner, corner + 1, corner + STEPS + 1);
                mesh.add_triangle(corner + 1, corner + STEPS + 1, corner + STEPS + 2);
            }
        }
    }
    let painter = ui.painter();
    painter.add(mesh);
    rounded(painter, square, 6);
    handle(
        &painter.with_clip_rect(square.expand(4.0)),
        square.lerp_inside(vec2(color.s, 1.0 - color.v)),
        Hsva::new(color.h, color.s, color.v, 1.0).into(),
    );

    ui.add_space(12.0);
    let (_, strip) = ui.allocate_space(vec2(WIDTH, 12.0));
    let hue = ui.interact(
        strip.expand2(vec2(0.0, 4.0)),
        ui.id().with("hue"),
        Sense::click_and_drag(),
    );
    if let Some(pointer) = hue.interact_pointer_pos() {
        color.h = ((pointer.x - strip.left()) / strip.width()).clamp(0.0, 1.0);
        changed = true;
    }
    const HUES: u32 = 36;
    let mut mesh = egui::Mesh::default();
    for step in 0..=HUES {
        let at = step as f32 / HUES as f32;
        let ink = Hsva::new(at, 1.0, 1.0, 1.0).into();
        mesh.colored_vertex(strip.lerp_inside(vec2(at, 0.0)), ink);
        mesh.colored_vertex(strip.lerp_inside(vec2(at, 1.0)), ink);
        if step < HUES {
            let corner = step * 2;
            mesh.add_triangle(corner, corner + 1, corner + 2);
            mesh.add_triangle(corner + 1, corner + 2, corner + 3);
        }
    }
    let painter = ui.painter();
    painter.add(mesh);
    rounded(painter, strip, 6);
    handle(
        painter,
        Pos2::new(
            strip.left() + (strip.width() * color.h).clamp(6.0, strip.width() - 6.0),
            strip.center().y,
        ),
        Hsva::new(color.h, 1.0, 1.0, 1.0).into(),
    );
    ui.add_space(2.0);
    changed
}

/// A `#RRGGBB` field, outlined in the problem colour while it is unusable.
fn hex_field(ui: &mut Ui, p: Palette, draft: &mut Draft, index: usize) {
    let label = ThemeColors::LABELS[index];
    let id = Id::new(("theme-color", index));
    let (_, rect) = ui.allocate_space(vec2(HEX_WIDTH, metrics::CONTROL_HEIGHT));
    let parsed = parse_hex(&draft.fields[index]);
    field_frame(ui, p, rect, id, parsed.is_some());
    let inner = rect.shrink2(vec2(9.0, 0.0));
    let text = &mut draft.fields[index];
    let response = place(
        ui,
        inner,
        Layout::left_to_right(Align::Center),
        ("theme-color-text", index),
        |ui| bare_text_edit(ui, id, text, "#RRGGBB", 12.5, inner.width()),
    );
    response
        .widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, format!("{label} hex")));
    if response.changed() {
        draft.error = None;
    }
    if text.chars().count() > 16 {
        *text = text.chars().take(16).collect();
    }
    // Settle on the stored spelling once the field is left.
    if response.lost_focus()
        && let Some(parsed) = parsed
    {
        *text = String::from(parsed);
    }
    if response.gained_focus() {
        response.scroll_to_me_animation(None, egui::style::ScrollAnimation::none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(id: u64, name: &str) -> CustomTheme {
        CustomTheme {
            id: format!("custom:{id}"),
            name: name.into(),
            colors: crate::terminal_theme::bundled("Dracula").unwrap().colors,
        }
    }

    #[test]
    fn hex_fields_accept_what_people_type_and_nothing_else() {
        assert_eq!(parse_hex("#ABcdEF"), Some(HexColor(0xabcdef)));
        assert_eq!(parse_hex(" 282a36 "), Some(HexColor(0x282a36)));
        assert_eq!(parse_hex("#f80"), Some(HexColor(0xff8800)));
        for text in ["", "#", "#12345", "#1234567", "#zzzzzz", "+12345", "#é1234"] {
            assert_eq!(parse_hex(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_new_theme_takes_the_first_free_id_and_name() {
        let mut config = Config {
            custom_themes: vec![custom(1, "My theme"), custom(3, "my theme 2")],
            ..Config::default()
        };
        let colors = config.custom_themes[0].colors;
        let draft = Draft::copy(&config, "My theme", colors);
        assert_eq!(draft.id, "custom:2");
        assert_eq!(draft.name, "My theme 3");
        assert!(!draft.saved && !draft.is_changed());
        assert_eq!(draft.problem(&config), None);
        // A long base still leaves room for its number.
        let long = "n".repeat(MAX_NAME_CHARS);
        config
            .custom_themes
            .push(custom(4, &long[..MAX_NAME_CHARS - 4]));
        let draft = Draft::copy(&config, &long, colors);
        assert!(draft.name.chars().count() <= MAX_NAME_CHARS);
        assert_eq!(draft.problem(&config), None);
    }

    #[test]
    fn a_draft_names_what_stops_it_from_being_saved() {
        let config = Config {
            custom_themes: vec![custom(1, "Night"), custom(2, "Day")],
            ..Config::default()
        };
        let mut draft = Draft::edit(&config.custom_themes[0]);
        assert!(draft.saved && !draft.is_changed());
        assert_eq!(draft.problem(&config), None);
        draft.name = "  day ".into();
        assert!(draft.is_changed());
        assert!(draft.problem(&config).unwrap().contains("already"));
        draft.name = " ".into();
        assert!(draft.problem(&config).unwrap().contains("Name"));
        draft.name = "Night".into();
        draft.fields[8] = "#12".into();
        assert_eq!(draft.problem(&config).unwrap(), "Red: use #RRGGBB.");
        // The same colour spelled another way is not a change.
        draft.fields[8] = String::from(draft.original.1.values()[8]).to_uppercase();
        assert!(!draft.is_changed());
    }

    #[test]
    fn saving_applies_the_draft_and_a_rejected_draft_changes_nothing() {
        let mut config = Config::default();
        let colors = crate::terminal_theme::bundled("Dracula").unwrap().colors;
        let mut draft = Draft::copy(&config, "Night", colors);
        assert!(draft.save(&mut config, None));
        assert_eq!(config.theme, Theme::Palette("custom:1".into()));
        assert_eq!(config.custom_themes, [custom(1, "Night")]);

        // Editing replaces the saved theme instead of adding another.
        let mut draft = Draft::edit(&config.custom_themes[0]);
        draft.name = "Dusk night".into();
        assert!(draft.save(&mut config, None));
        assert_eq!(config.custom_themes, [custom(1, "Dusk night")]);

        let before = config.clone();
        let mut draft = Draft::copy(&config, "Other", colors);
        draft.name = "bad\u{7}name".into();
        assert!(!draft.save(&mut config, None));
        assert_eq!(config, before);
        assert!(draft.problem(&config).is_some());
    }

    #[test]
    fn a_theme_saved_for_a_mode_is_chosen_for_that_mode_alone() {
        let mut config = Config::default();
        config.set_match_system_appearance(true);
        config.system_appearance = Some(Appearance::Light);
        let colors = crate::terminal_theme::bundled("3024 Day").unwrap().colors;
        let mut draft = Draft::copy(&config, "Paper", colors);
        assert!(draft.save(&mut config, Some(Appearance::Light)));
        assert_eq!(config.light_theme, Theme::Palette("custom:1".into()));
        assert_eq!(config.active_theme(), &config.light_theme);
        assert_eq!(config.dark_theme, Theme::Graphite);
        assert_eq!(config.theme, Theme::Graphite);
    }
}
