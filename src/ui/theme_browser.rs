//! The theme catalog: a screen of Preferences, and the way into the editor.
//!
//! One theme styles the window and every terminal, or one for each of the
//! system's light and dark modes. Choosing a card applies it at once; custom
//! themes are edited as drafts in `theme_editor`.
use super::helpers::{
    ButtonKind, Group, bare_text_edit, button, elided, field_frame, focus_ring, galley_at,
    menu_item, menu_layout, menu_separator, place, segmented, sheet_back_header, sheet_footer,
};
use super::theme_editor::{self, Draft, Outcome};
use crate::{
    config::{Accent, Appearance, Config, Theme},
    icons::{self, Icon},
    terminal_theme::{
        BUNDLED, BundledTheme, CustomTheme, HexColor, MAX_CUSTOM_THEMES, ThemeColors, bundled,
    },
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontId, Id, Layout, Painter, Pos2, Rect, Response,
    Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType,
    text::{LayoutJob, TextFormat},
    vec2,
};

/// Width of the sheet while the catalog or the editor is shown.
pub const WIDTH: f32 = 680.0;
pub(super) const GUTTER: f32 = 20.0;
const TILE_HEIGHT: f32 = 96.0;
const CARD_HEIGHT: f32 = TILE_HEIGHT + 32.0;
const CARD_GAP: f32 = 12.0;
const LABEL_HEIGHT: f32 = 30.0;
const TILE_RADIUS: u8 = 8;
const CUSTOM_LIMIT: &str = "Custom theme limit reached (128). Delete a theme to make room.";

#[derive(Default)]
pub struct State {
    /// The catalog replaces the general settings.
    pub open: bool,
    /// The mode whose theme the catalog chooses, or the single theme.
    target: Option<Appearance>,
    query: String,
    filter: Filter,
    draft: Option<Draft>,
    /// The custom theme whose removal awaits confirmation in the footer.
    deleting: Option<String>,
    /// The search field takes the keyboard when the catalog appears.
    focus_search: bool,
    /// Scroll the catalog to the theme in use.
    reveal: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Filter {
    #[default]
    All,
    Dark,
    Light,
    Custom,
    Favorites,
}

impl State {
    /// Shows the catalog, ready to search, at the theme chosen for `target`:
    /// a mode's, or the single theme for none.
    pub fn browse(&mut self, target: Option<Appearance>) {
        self.open = true;
        self.target = target;
        self.focus_search = true;
        self.reveal = true;
    }

    /// Escape leaves the innermost thing first: a confirmation, the editor,
    /// then the catalog. A changed draft asks before it is discarded.
    pub fn back(&mut self) -> bool {
        if self.deleting.take().is_some() {
            return true;
        }
        if let Some(draft) = &mut self.draft {
            if draft.discarding {
                draft.discarding = false;
            } else if draft.is_changed() {
                draft.discarding = true;
            } else {
                self.draft = None;
            }
            return true;
        }
        std::mem::take(&mut self.open)
    }

    pub fn editing(&self) -> bool {
        self.draft.is_some()
    }

    /// The editor holding a draft the user has changed.
    #[cfg(test)]
    pub fn with_changed_draft(config: &Config) -> Self {
        Self {
            open: true,
            draft: Some(Draft::changed(config)),
            ..Self::default()
        }
    }

    /// Whether the sheet may close now. A changed draft asks first.
    pub fn may_close(&mut self) -> bool {
        match &mut self.draft {
            Some(draft) if draft.is_changed() => {
                draft.discarding = true;
                false
            }
            _ => true,
        }
    }
}

/// The palette as a custom theme would store it.
pub(super) fn colors_of(look: &Palette) -> ThemeColors {
    let rgb = |c: Color32| {
        HexColor((u32::from(c.r()) << 16) | (u32::from(c.g()) << 8) | u32::from(c.b()))
    };
    ThemeColors {
        background: rgb(look.bg),
        foreground: rgb(look.terminal_fg),
        bold: rgb(look.terminal_bold),
        cursor: rgb(look.cursor),
        cursor_text: rgb(look.cursor_text.unwrap_or(look.bg)),
        selection: rgb(look.selection),
        selection_text: rgb(look.selection_text.unwrap_or(look.terminal_fg)),
        ansi: look.ansi.map(rgb),
    }
}

enum Source<'a> {
    Neptune(Theme),
    Custom(&'a CustomTheme),
    Bundled(&'static BundledTheme),
}

/// One theme of the catalog.
struct Entry<'a> {
    name: &'a str,
    source: Source<'a>,
    /// Listed among the favorites, apart from its place of origin.
    pinned: bool,
}

impl<'a> Entry<'a> {
    fn neptune((theme, name): (Theme, &'a str)) -> Self {
        Self {
            name,
            source: Source::Neptune(theme),
            pinned: false,
        }
    }

    fn custom(custom: &'a CustomTheme) -> Self {
        Self {
            name: &custom.name,
            source: Source::Custom(custom),
            pinned: false,
        }
    }

    fn bundled(bundled: &'static BundledTheme) -> Self {
        Self {
            name: bundled.name,
            source: Source::Bundled(bundled),
            pinned: false,
        }
    }

    /// A favorite, when its theme still exists.
    fn favorite(config: &'a Config, theme: &Theme) -> Option<Self> {
        let entry = match theme {
            Theme::Palette(id) => match id.strip_prefix("iterm:") {
                Some(name) => Self::bundled(bundled(name)?),
                None => Self::custom(config.custom_themes.iter().find(|t| &t.id == id)?),
            },
            own => Self::neptune(
                Theme::BUILTINS
                    .into_iter()
                    .find(|(theme, _)| theme == own)?,
            ),
        };
        Some(Self {
            pinned: true,
            ..entry
        })
    }

    fn theme(&self) -> Theme {
        match &self.source {
            Source::Neptune(theme) => theme.clone(),
            Source::Custom(custom) => Theme::Palette(custom.id.clone()),
            Source::Bundled(bundled) => Theme::Palette(format!("iterm:{}", bundled.name)),
        }
    }

    fn is(&self, theme: &Theme) -> bool {
        match &self.source {
            Source::Neptune(own) => own == theme,
            Source::Custom(custom) => theme.id() == custom.id,
            Source::Bundled(bundled) => theme.id().strip_prefix("iterm:") == Some(bundled.name),
        }
    }

    /// The window as this theme would draw it.
    fn look(&self, accent: Accent) -> Palette {
        match &self.source {
            Source::Neptune(theme) => Palette::with_accent(theme, accent),
            Source::Custom(custom) => Palette::from_colors(custom.colors),
            Source::Bundled(bundled) => Palette::from_colors(bundled.colors),
        }
    }

    fn is_dark(&self) -> bool {
        match &self.source {
            Source::Neptune(theme) => theme != &Theme::Light,
            Source::Custom(custom) => custom.colors.is_dark(),
            Source::Bundled(bundled) => bundled.colors.is_dark(),
        }
    }

    fn is_favorite(&self, config: &Config) -> bool {
        config.favorite_themes.iter().any(|theme| self.is(theme))
    }

    /// A favorite is listed twice while browsing; each card is its own widget.
    fn id(&self) -> Id {
        let id = match &self.source {
            Source::Neptune(theme) => Id::new(("theme-card", theme.id())),
            Source::Custom(custom) => Id::new(("theme-card", &custom.id)),
            Source::Bundled(bundled) => Id::new(("theme-card", "iterm", bundled.name)),
        };
        if self.pinned { id.with("favorite") } else { id }
    }
}

enum Row {
    Label(&'static str, usize),
    Cards(std::ops::Range<usize>),
}

/// The catalog as titled sections of card rows, each row at its offset.
struct Sections<'a> {
    entries: Vec<Entry<'a>>,
    rows: Vec<(f32, Row)>,
    height: f32,
    columns: usize,
}

impl<'a> Sections<'a> {
    /// The offset of the row that holds `theme`, when it is listed.
    fn row_of(&self, theme: &Theme) -> Option<f32> {
        let index = self.entries.iter().position(|entry| entry.is(theme))?;
        self.rows
            .iter()
            .find(|(_, row)| matches!(row, Row::Cards(range) if range.contains(&index)))
            .map(|(top, _)| *top)
    }

    /// Adds a section; one without themes takes no room.
    fn push(&mut self, title: &'static str, found: impl Iterator<Item = Entry<'a>>) {
        let first = self.entries.len();
        self.entries.extend(found);
        let last = self.entries.len();
        if first == last {
            return;
        }
        if !self.rows.is_empty() {
            self.height += 6.0;
        }
        self.rows
            .push((self.height, Row::Label(title, last - first)));
        self.height += LABEL_HEIGHT;
        for start in (first..last).step_by(self.columns) {
            let cards = Row::Cards(start..(start + self.columns).min(last));
            self.rows.push((self.height, cards));
            self.height += CARD_HEIGHT + CARD_GAP;
        }
    }
}

/// What a card or its menu asked for; applied once the catalog is drawn.
enum Command {
    Use(Theme),
    /// Adds a theme to the favorites or removes it, from a pinned card or not.
    Star(Theme, bool),
    Edit(String),
    Copy(String, ThemeColors),
    Delete(String),
}

/// Sample terminal text in a theme's colours, at most `lines` long.
/// Illustrative content only, never terminal output.
fn sample(look: &Palette, size: f32, lines: usize, full: bool) -> LayoutJob {
    let none = Color32::TRANSPARENT;
    let fg = look.terminal_fg;
    let [_, red, green, yellow, blue, magenta, cyan, ..] = look.ansi;
    let path = [("~/neptune", blue, none), (" main", magenta, none)];
    let cursor = (look.cursor_text.unwrap_or(look.bg), look.cursor);
    let text: Vec<Vec<(&str, Color32, Color32)>> = if full {
        vec![
            path.to_vec(),
            vec![("$", green, none), (" git status -s", fg, none)],
            vec![(" M", red, none), (" src/theme.rs", fg, none)],
            vec![("??", cyan, none), (" assets/themes/", fg, none)],
            vec![("$", green, none), (" cargo test", fg, none)],
            vec![("warning", yellow, none), (": unused import", fg, none)],
            vec![
                ("test result:", look.terminal_bold, none),
                (" ok.", green, none),
                (" 42 passed", fg, none),
            ],
            vec![
                ("$", green, none),
                (" echo ", fg, none),
                (
                    "selected",
                    look.selection_text.unwrap_or(fg),
                    look.selection,
                ),
                (" te", fg, none),
                ("x", cursor.0, cursor.1),
                ("t", fg, none),
            ],
        ]
    } else {
        vec![
            path.to_vec(),
            vec![("$", green, none), (" git status", fg, none)],
            vec![("M", yellow, none), (" theme.rs", fg, none)],
            vec![
                ("$", green, none),
                (" ", fg, none),
                (" ", cursor.0, cursor.1),
            ],
        ]
    };
    let mut job = LayoutJob::default();
    for (index, line) in text.iter().take(lines).enumerate() {
        if index > 0 {
            job.append("\n", 0.0, TextFormat::default());
        }
        for (run, ink, fill) in line {
            job.append(
                run,
                0.0,
                TextFormat {
                    font_id: FontId::monospace(size),
                    color: *ink,
                    background: *fill,
                    line_height: Some((size * 1.45).round()),
                    ..Default::default()
                },
            );
        }
    }
    job
}

/// A miniature of the window in a theme's own materials: the chrome and one
/// pane of sample text. `full` adds every ANSI colour, for the editor.
pub(super) fn preview(painter: &Painter, rect: Rect, look: &Palette, full: bool) {
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    painter.rect_filled(rect, TILE_RADIUS, look.chrome);
    for index in 0..3 {
        painter.circle_filled(
            rect.min + vec2(10.0 + index as f32 * 7.0, 8.5),
            2.0,
            theme::mix(look.chrome, look.fg, 0.26),
        );
    }
    let pane = Rect::from_min_max(rect.min + vec2(4.0, 17.0), rect.max - Vec2::splat(4.0));
    painter.rect_filled(pane, TILE_RADIUS - 3, look.bg);
    let left = pane.left() + 9.0;
    // The colours sit along the pane's bottom edge; text takes the rest.
    let swatches = if full {
        let gap = 3.0;
        let side = ((pane.width() - 18.0 - gap * 7.0) / 8.0).min(22.0);
        let height = 9.0;
        let top = pane.bottom() - 9.0 - height * 2.0 - gap;
        for (index, color) in look.ansi.iter().enumerate() {
            let cell = Rect::from_min_size(
                Pos2::new(
                    left + (index % 8) as f32 * (side + gap),
                    top + (index / 8) as f32 * (height + gap),
                ),
                vec2(side, height),
            );
            painter.rect_filled(cell, 2, *color);
        }
        top
    } else {
        let top = pane.bottom() - 13.0;
        for (index, color) in look.ansi[1..7].iter().enumerate() {
            painter.circle_filled(
                Pos2::new(left + 3.0 + index as f32 * 10.0, top + 4.0),
                3.0,
                *color,
            );
        }
        top
    };
    let size = if full { 11.0 } else { 9.5 };
    let line = (size * 1.45_f32).round();
    let lines = ((swatches - 5.0 - pane.top() - 6.0) / line)
        .floor()
        .max(0.0) as usize;
    if lines > 0 {
        let galley = painter.layout_job(sample(look, size, lines, full));
        painter
            .with_clip_rect(pane.intersect(painter.clip_rect()))
            .galley(Pos2::new(left, pane.top() + 6.0), galley, look.terminal_fg);
    }
}

pub(super) fn tile_outline(painter: &Painter, tile: Rect, p: Palette, emphasized: bool) {
    painter.rect_stroke(
        tile,
        TILE_RADIUS,
        Stroke::new(1.0, if emphasized { p.muted } else { p.border }),
        StrokeKind::Inside,
    );
}

/// Where a theme came from, for the general settings.
fn kind(theme: &Theme) -> &'static str {
    match theme {
        Theme::Palette(id) if id.starts_with("custom:") => "Custom theme",
        Theme::Palette(_) => "iTerm2 collection",
        _ => "Neptune theme",
    }
}

/// The theme chosen for `target`, drawn in its own colours: a mode's, or the
/// single theme for none. The whole row opens the catalog to choose it.
pub fn summary(
    ui: &mut Ui,
    p: Palette,
    config: &Config,
    rows: &mut Group,
    target: Option<Appearance>,
) -> bool {
    let inner = rows.row_with_height(ui, "", 112.0, |ui| ui.max_rect());
    // The row's controls are inset; the row itself spans the group.
    let row = inner.expand2(vec2(12.0, 0.0));
    let response = ui.interact(row, ui.id().with(("theme-summary", target)), Sense::click());
    let label = match target {
        None => "Browse themes…".to_owned(),
        Some(mode) => format!("Choose the {} theme…", mode.name().to_lowercase()),
    };
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
    let chosen = config.theme_for(target);
    let look = Palette::for_theme(config, chosen);
    let painter = ui.painter();
    let fill = if response.is_pointer_button_down_on() {
        Some(p.pressed)
    } else if response.hovered() {
        Some(p.hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        // The switch above takes the group's rounded corners.
        painter.rect_filled(row, 0, fill);
    }
    if response.has_focus() {
        focus_ring(painter, row.shrink(3.0), 7, p);
    }
    let tile = Rect::from_center_size(
        Pos2::new(row.left() + 12.0 + 66.0, row.center().y),
        vec2(132.0, 88.0),
    );
    preview(painter, tile, &look, false);
    tile_outline(painter, tile, p, false);
    let ink = if response.hovered() {
        p.fg
    } else {
        p.secondary
    };
    icons::paint(
        painter,
        Rect::from_center_size(
            Pos2::new(row.right() - 20.0, row.center().y),
            Vec2::splat(12.0),
        ),
        Icon::ChevronRight,
        ink,
    );
    // The row says where it leads while there is room beside the name.
    let mut trailing = row.right() - 34.0;
    if row.width() >= 400.0 {
        let label = painter.layout_no_wrap("Browse themes".to_owned(), theme::medium(12.5), ink);
        trailing -= label.size().x;
        galley_at(painter, Pos2::new(trailing, row.center().y), label);
        trailing -= 12.0;
    }
    let left = tile.right() + 14.0;
    let width = trailing - left;
    galley_at(
        painter,
        Pos2::new(left, row.center().y - 9.0),
        elided(
            painter,
            config.name_of(chosen),
            theme::medium(13.0),
            p.fg,
            width,
        ),
    );
    let tone = if look.dark { "Dark" } else { "Light" };
    let origin = match target {
        None => format!("{} · {tone}", kind(chosen)),
        Some(mode) if config.appearance() == Some(mode) => {
            format!("{} · In use · {}", mode.name(), kind(chosen))
        }
        Some(mode) => format!("{} · {}", mode.name(), kind(chosen)),
    };
    galley_at(
        painter,
        Pos2::new(left, row.center().y + 10.0),
        elided(painter, &origin, theme::regular(11.5), p.muted, width),
    );
    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// Draws the catalog or the editor inside the Preferences sheet: header, a
/// body `body_height` tall and the action bar. Returns true when the sheet was
/// asked to close.
pub fn show(
    ui: &mut Ui,
    p: Palette,
    config: &mut Config,
    state: &mut State,
    body_height: f32,
) -> bool {
    let target = state.target;
    let Some(draft) = &mut state.draft else {
        return catalog(ui, p, config, state, body_height);
    };
    let (outcome, close) = theme_editor::show(ui, p, config, draft, target, body_height);
    match outcome {
        Outcome::Editing => {}
        Outcome::Saved => {
            // Back in the catalog at the saved theme, whatever was filtered.
            *state = State::default();
            state.browse(target);
        }
        Outcome::Left => {
            // The catalog is as it was left, ready to search again.
            state.draft = None;
            state.focus_search = true;
        }
    }
    close
}

/// A search field: a magnifier leads the text and a clear control trails it.
/// `label` is its accessible name; the caller owns `id` so it can take focus.
pub(super) fn search_field(
    ui: &mut Ui,
    p: Palette,
    id: Id,
    text: &mut String,
    hint: &str,
    label: &str,
    width: f32,
) -> Response {
    let (_, rect) = ui.allocate_space(vec2(width, metrics::CONTROL_HEIGHT));
    field_frame(ui, p, rect, id, true);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(
            Pos2::new(rect.left() + 16.0, rect.center().y),
            Vec2::splat(13.0),
        ),
        Icon::Search,
        p.muted,
    );
    let clearable = !text.is_empty();
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 30.0, rect.top()),
        Pos2::new(
            rect.right() - if clearable { 30.0 } else { 10.0 },
            rect.bottom(),
        ),
    );
    let mut response = place(
        ui,
        inner,
        Layout::left_to_right(Align::Center),
        ("search-text", id),
        |ui| bare_text_edit(ui, id, text, hint, 13.0, inner.width()),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, label));
    if clearable {
        let target = Rect::from_center_size(
            Pos2::new(rect.right() - 16.0, rect.center().y),
            Vec2::splat(22.0),
        );
        let clear = ui.interact(target, id.with("clear"), Sense::click());
        clear.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Clear search"));
        let painter = ui.painter();
        if clear.hovered() || clear.has_focus() {
            painter.rect_filled(target.shrink(1.0), 6, p.hover);
        }
        if clear.has_focus() {
            focus_ring(painter, target.shrink(1.0), 6, p);
        }
        icons::paint(
            painter,
            Rect::from_center_size(target.center(), Vec2::splat(9.0)),
            Icon::Close,
            if clear.hovered() { p.fg } else { p.secondary },
        );
        if clear
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Clear search")
            .clicked()
        {
            text.clear();
            response.mark_changed();
            response.request_focus();
        }
    }
    response
}

/// Text in the room a footer's buttons leave, cut with an ellipsis.
pub(super) fn footer_note(ui: &mut Ui, text: &str, color: Color32) {
    let rest = ui.available_rect_before_wrap();
    let rest = Rect::from_min_max(rest.min + vec2(4.0, 0.0), rest.max - vec2(8.0, 0.0));
    if rest.width() < 40.0 {
        return;
    }
    place(
        ui,
        rest,
        Layout::left_to_right(Align::Center),
        "footer-note",
        |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(text).size(12.5).color(color))
                    .truncate()
                    .selectable(false),
            );
        },
    );
}

fn catalog(
    ui: &mut Ui,
    p: Palette,
    config: &mut Config,
    state: &mut State,
    body_height: f32,
) -> bool {
    let title = match state.target {
        None => "Themes".to_owned(),
        Some(mode) => format!("{} theme", mode.name()),
    };
    let (back, mut close) =
        sheet_back_header(ui, p, &title, "Back to preferences", "Close preferences");
    let chosen = config.theme_for(state.target).clone();
    let width = ui.available_width();
    let inner = width - GUTTER * 2.0;
    let (changed, toolbar_height) = toolbar(ui, p, state);
    ui.add_space(12.0);
    let list_height = (body_height - toolbar_height - 12.0).max(80.0);

    // The column count follows the width the sheet settles at, so cards grow
    // with the sheet instead of regrouping while it opens.
    let settled = WIDTH.min(ui.ctx().content_rect().width() - 24.0) - GUTTER * 2.0;
    let columns = if settled >= 520.0 {
        3
    } else if settled >= 340.0 {
        2
    } else {
        1
    };
    let list = sections(config, state, columns);
    let mut command = None;
    let mut scrolled = None;
    if list.entries.is_empty() {
        let (_, area) = ui.allocate_space(vec2(width, list_height));
        nothing_found(ui.painter(), p, area, state);
    } else {
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("theme-catalog")
            .min_scrolled_height(list_height)
            .max_height(list_height)
            .auto_shrink([false, false]);
        if changed {
            scroll = scroll.vertical_scroll_offset(0.0);
        } else if state.reveal
            && let Some(top) = list.row_of(&chosen)
        {
            // Centre the row of the theme in use; near the top, keep its heading.
            let offset = top - ((list_height - CARD_HEIGHT) * 0.5).max(4.0);
            scroll = scroll.vertical_scroll_offset(if offset > LABEL_HEIGHT {
                offset.min(list.height)
            } else {
                0.0
            });
        }
        let card_width = (inner - (columns - 1) as f32 * CARD_GAP) / columns as f32;
        let output = scroll.show_viewport(ui, |ui, viewport| {
            let origin = ui.cursor().min + vec2(GUTTER, 0.0);
            ui.allocate_space(vec2(ui.available_width(), list.height));
            // Only the rows in view are laid out and painted.
            for (top, row) in &list.rows {
                let height = match row {
                    Row::Label(..) => LABEL_HEIGHT,
                    Row::Cards(_) => CARD_HEIGHT,
                };
                if top + height < viewport.top() || *top > viewport.bottom() {
                    continue;
                }
                match row {
                    Row::Label(title, count) => {
                        section_heading(ui.painter(), p, origin + vec2(0.0, *top), title, *count);
                    }
                    Row::Cards(range) => {
                        for (column, entry) in list.entries[range.clone()].iter().enumerate() {
                            let rect = Rect::from_min_size(
                                origin + vec2(column as f32 * (card_width + CARD_GAP), *top),
                                vec2(card_width, CARD_HEIGHT),
                            );
                            card(ui, p, rect, entry, config, &chosen, &mut command);
                        }
                    }
                }
            }
        });
        scrolled = Some((output.id, output.state));
    }
    let height = list.height;
    drop(list);
    // Revealing waits for the first laid-out frame; the sizing pass has no scroll.
    if !ui.is_sizing_pass() {
        state.reveal = false;
    }

    close |= footer(ui, p, config, state);
    match command {
        Some(Command::Use(theme)) => {
            *config.theme_for_mut(state.target) = theme;
            state.deleting = None;
        }
        Some(Command::Star(theme, pinned)) => {
            if let Some(index) = config.favorite_themes.iter().position(|t| t == &theme) {
                config.favorite_themes.remove(index);
            } else {
                config.favorite_themes.push(theme);
            }
            // A card starred where it stands stays under the pointer while
            // the favorites above it grow or shrink.
            if !pinned && let Some((id, mut scroll)) = scrolled {
                let grown = sections(config, state, columns).height - height;
                scroll.offset.y = (scroll.offset.y + grown).max(0.0);
                scroll.store(ui.ctx(), id);
            }
        }
        Some(Command::Edit(id)) => {
            state.draft = config
                .custom_themes
                .iter()
                .find(|t| t.id == id)
                .map(Draft::edit);
        }
        Some(Command::Copy(name, colors)) => {
            state.draft = Some(Draft::copy(config, &format!("{name} copy"), colors));
        }
        Some(Command::Delete(id)) => state.deleting = Some(id),
        None => {}
    }
    // A confirmation belongs to the catalog as it was asked.
    if back || state.draft.is_some() {
        state.deleting = None;
    }
    if back {
        state.open = false;
    }
    close
}

/// Search and filter. They share a line while there is room for both. Returns
/// whether either changed, and the height taken.
fn toolbar(ui: &mut Ui, p: Palette, state: &mut State) -> (bool, f32) {
    let width = ui.available_width();
    let inner = width - GUTTER * 2.0;
    let stacked = inner < 550.0;
    let filter_width = if stacked { inner } else { 364.0 };
    let height = if stacked { 68.0 } else { 30.0 };
    let (_, bar) = ui.allocate_space(vec2(width, height));
    let bar = bar.shrink2(vec2(GUTTER, 0.0));
    let search = Rect::from_min_size(
        bar.min,
        vec2(
            if stacked {
                inner
            } else {
                inner - filter_width - 10.0
            },
            30.0,
        ),
    );
    let filter = Rect::from_min_size(
        if stacked {
            bar.min + vec2(0.0, 40.0)
        } else {
            Pos2::new(bar.right() - filter_width, bar.top() + 1.0)
        },
        vec2(filter_width, 28.0),
    );
    let field = place(
        ui,
        search,
        Layout::left_to_right(Align::Center),
        "theme-search",
        |ui| {
            search_field(
                ui,
                p,
                Id::new("theme-search"),
                &mut state.query,
                "Search themes",
                "Search themes",
                search.width(),
            )
        },
    );
    // A sheet measures itself in a hidden first pass, where focus cannot be held.
    if state.focus_search && ui.is_enabled() && !ui.is_sizing_pass() {
        field.request_focus();
        state.focus_search = false;
    }
    if state.query.chars().count() > 256 {
        state.query = state.query.chars().take(256).collect();
    }
    let filtered = place(
        ui,
        filter,
        Layout::left_to_right(Align::Center),
        "theme-filter",
        |ui| {
            segmented(
                ui,
                p,
                "theme-filter",
                &mut state.filter,
                &[
                    (Filter::All, "All"),
                    (Filter::Dark, "Dark"),
                    (Filter::Light, "Light"),
                    (Filter::Custom, "Custom"),
                    (Filter::Favorites, "Favorites"),
                ],
                filter_width,
            )
        },
    );
    (field.changed() || filtered, height)
}

/// The themes that match the search and filter, grouped by where they come
/// from. Favorites lead while browsing; a search lists each theme once.
fn sections<'a>(config: &'a Config, state: &State, columns: usize) -> Sections<'a> {
    let query = state.query.trim().to_lowercase();
    let shown = |entry: &Entry| {
        (match state.filter {
            Filter::All | Filter::Favorites => true,
            Filter::Dark => entry.is_dark(),
            Filter::Light => !entry.is_dark(),
            Filter::Custom => matches!(entry.source, Source::Custom(_)),
        }) && (query.is_empty() || entry.name.to_lowercase().contains(&query))
    };
    let mut list = Sections {
        entries: Vec::new(),
        rows: Vec::new(),
        height: 0.0,
        columns,
    };
    let favorites = state.filter == Filter::Favorites;
    if favorites || query.is_empty() {
        list.push(
            "Favorites",
            config
                .favorite_themes
                .iter()
                .filter_map(|theme| Entry::favorite(config, theme))
                .filter(shown),
        );
    }
    if favorites {
        return list;
    }
    list.push(
        "Your themes",
        config.custom_themes.iter().map(Entry::custom).filter(shown),
    );
    list.push(
        "Neptune",
        Theme::BUILTINS
            .into_iter()
            .map(Entry::neptune)
            .filter(shown),
    );
    list.push(
        "iTerm2 collection",
        BUNDLED.iter().map(Entry::bundled).filter(shown),
    );
    list
}

fn section_heading(painter: &Painter, p: Palette, origin: Pos2, title: &str, count: usize) {
    let line = origin + vec2(4.0, LABEL_HEIGHT * 0.5 + 1.0);
    let title = galley_at(
        painter,
        line,
        painter.layout_no_wrap(title.to_owned(), theme::medium(11.5), p.secondary),
    );
    painter.text(
        Pos2::new(title.right() + 7.0, line.y),
        Align2::LEFT_CENTER,
        count.to_string(),
        theme::regular(11.5),
        p.muted,
    );
}

/// What the list says when nothing matches.
fn nothing_found(painter: &Painter, p: Palette, area: Rect, state: &State) {
    let (icon, title, hint) = match state.filter {
        Filter::Custom if state.query.trim().is_empty() => (
            Icon::Pencil,
            "No custom themes yet",
            "New theme starts one from the colors in use.",
        ),
        Filter::Favorites if state.query.trim().is_empty() => (
            Icon::Star,
            "No favorites yet",
            "Star a theme to keep it here.",
        ),
        _ => (
            Icon::Search,
            "No themes found",
            "Try another name or filter.",
        ),
    };
    let center = area.center();
    icons::paint(
        painter,
        Rect::from_center_size(center - vec2(0.0, 34.0), Vec2::splat(22.0)),
        icon,
        p.muted,
    );
    painter.text(
        center - vec2(0.0, 4.0),
        Align2::CENTER_CENTER,
        title,
        theme::medium(13.0),
        p.fg,
    );
    painter.text(
        center + vec2(0.0, 16.0),
        Align2::CENTER_CENTER,
        hint,
        theme::regular(12.0),
        p.muted,
    );
}

/// The catalog's action bar, or the question a removal asks first. Returns
/// true when Done closes the sheet.
fn footer(ui: &mut Ui, p: Palette, config: &mut Config, state: &mut State) -> bool {
    let bar = sheet_footer(ui, p);
    let deleting = state.deleting.take().and_then(|id| {
        let name = config
            .custom_themes
            .iter()
            .find(|t| t.id == id)?
            .name
            .clone();
        Some((id, name))
    });
    let Some((id, name)) = deleting else {
        let room = config.custom_themes.len() < MAX_CUSTOM_THEMES;
        place(
            ui,
            bar,
            Layout::left_to_right(Align::Center),
            "theme-new",
            |ui| {
                let new = ui
                    .add_enabled_ui(room, |ui| button(ui, p, "New theme", ButtonKind::Secondary))
                    .inner
                    .on_hover_text(if room {
                        "Create a custom theme from the colors in use"
                    } else {
                        CUSTOM_LIMIT
                    });
                if new.clicked() {
                    let chosen = config.theme_for(state.target);
                    let colors = config
                        .colors_of(chosen)
                        .unwrap_or_else(|| colors_of(&Palette::for_theme(config, chosen)));
                    state.draft = Some(Draft::copy(config, "My theme", colors));
                }
            },
        );
        return place(
            ui,
            bar,
            Layout::right_to_left(Align::Center),
            "theme-done",
            |ui| button(ui, p, "Done", ButtonKind::Primary).clicked(),
        );
    };
    let consequence = deletion_consequence(config, &id);
    let (mut delete, mut cancel) = (false, false);
    place(
        ui,
        bar,
        Layout::right_to_left(Align::Center),
        "theme-delete",
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            delete = button(ui, p, "Delete theme", ButtonKind::Destructive).clicked();
            cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
            footer_note(ui, &format!("Delete “{name}”? {consequence}"), p.fg);
        },
    );
    if delete {
        delete_custom(config, &id);
    } else if !cancel {
        state.deleting = Some(id);
    }
    false
}

/// Removes the custom theme `id`. Whatever chose it returns to its default,
/// shown or not.
fn delete_custom(config: &mut Config, id: &str) {
    for target in [None, Some(Appearance::Light), Some(Appearance::Dark)] {
        let theme = config.theme_for_mut(target);
        if theme.id() == id {
            *theme = target.map_or(Theme::Graphite, Appearance::default_theme);
        }
    }
    config.custom_themes.retain(|t| t.id != id);
}

/// What deleting the custom theme `id` changes, among the choices shown.
fn deletion_consequence(config: &Config, id: &str) -> String {
    let targets: &[Option<Appearance>] = if config.match_system_appearance {
        &[Some(Appearance::Light), Some(Appearance::Dark)]
    } else {
        &[None]
    };
    let changes: Vec<String> = targets
        .iter()
        .filter(|&&target| config.theme_for(target).id() == id)
        .map(|&target| match target {
            None => "Neptune switches to Graphite.".to_owned(),
            Some(mode) => format!(
                "{} switches to {}.",
                mode.name(),
                config.name_of(&mode.default_theme())
            ),
        })
        .collect();
    if changes.is_empty() {
        "This cannot be undone.".to_owned()
    } else {
        changes.join(" ")
    }
}

/// A theme of the catalog: its preview, its name and, for a favorite, its
/// star in the accent. On hover or focus it shows a control for the same menu
/// as a secondary click, and the star of a theme that is not yet a favorite.
fn card(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    entry: &Entry,
    config: &Config,
    chosen: &Theme,
    command: &mut Option<Command>,
) {
    let id = entry.id();
    let selected = entry.is(chosen);
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::RadioButton,
            true,
            selected,
            format!("{} theme", entry.name),
        )
    });
    let tile = Rect::from_min_size(rect.min, vec2(rect.width(), TILE_HEIGHT));
    let more_rect = Rect::from_min_size(
        Pos2::new(tile.right() - 28.0, tile.top() + 6.0),
        Vec2::splat(22.0),
    );
    let more = ui.interact(more_rect, id.with("more"), Sense::click());
    more.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!("Actions for {} theme", entry.name),
        )
    });
    let favorite = entry.is_favorite(config);
    let line = tile.bottom() + 17.0;
    let star_rect = Rect::from_center_size(Pos2::new(rect.right() - 10.0, line), Vec2::splat(22.0));
    let star = ui.interact(star_rect, id.with("star"), Sense::click());
    star.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::Checkbox,
            true,
            favorite,
            format!("Favorite {} theme", entry.name),
        )
    });
    let menu_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&more))
        || response.context_menu_opened();
    let hovered = ui.rect_contains_pointer(rect) || menu_open;
    let mut truncated = false;
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let look = entry.look(config.accent);
        preview(painter, tile, &look, false);
        if selected {
            // The ring stands clear of the tile, as the focused pane's does.
            painter.rect_stroke(
                tile.expand(3.0),
                TILE_RADIUS + 3,
                Stroke::new(2.0, p.accent),
                StrokeKind::Inside,
            );
        } else {
            tile_outline(painter, tile, p, hovered);
        }
        if response.has_focus() {
            focus_ring(painter, tile.expand(3.0), TILE_RADIUS + 3, p);
        }
        // The star keeps its room, so a name reads the same hovered or not.
        let name = elided(
            painter,
            entry.name,
            theme::medium(12.5),
            if selected || hovered {
                p.fg
            } else {
                p.secondary
            },
            rect.width() - 28.0 - if selected { 22.0 } else { 0.0 },
        );
        truncated = name.elided;
        galley_at(painter, Pos2::new(rect.left() + 2.0, line), name);
        if favorite || hovered || star.has_focus() {
            if star.hovered() {
                painter.rect_filled(star_rect.shrink(1.0), 6, p.hover);
            }
            if star.has_focus() {
                focus_ring(painter, star_rect.shrink(1.0), 6, p);
            }
            icons::paint(
                painter,
                Rect::from_center_size(star_rect.center(), Vec2::splat(14.0)),
                Icon::Star,
                if favorite {
                    p.accent
                } else if star.hovered() || star.has_focus() {
                    p.fg
                } else {
                    p.secondary
                },
            );
        }
        if selected {
            let badge = Pos2::new(rect.right() - 34.0, line);
            painter.circle_filled(badge, 8.0, p.accent);
            icons::paint(
                painter,
                Rect::from_center_size(badge, Vec2::splat(9.0)),
                Icon::Check,
                p.on_accent,
            );
        }
        if hovered || more.has_focus() {
            // The control sits on the theme's colours, so it brings its own surface.
            painter.rect_filled(more_rect, 6, p.elevated);
            if more.hovered() || menu_open {
                painter.rect_filled(more_rect, 6, p.pressed);
            }
            painter.rect_stroke(more_rect, 6, Stroke::new(1.0, p.border), StrokeKind::Inside);
            if more.has_focus() {
                focus_ring(painter, more_rect, 6, p);
            }
            icons::paint(
                painter,
                more_rect.shrink(5.0),
                Icon::Ellipsis,
                if more.hovered() || menu_open {
                    p.fg
                } else {
                    p.secondary
                },
            );
        }
    }
    egui::Popup::menu(&more).show(|ui| card_menu(ui, p, entry, config, chosen, command));
    response.context_menu(|ui| card_menu(ui, p, entry, config, chosen, command));
    if response.gained_focus() || more.gained_focus() || star.gained_focus() {
        response.scroll_to_me(None);
    }
    more.on_hover_cursor(CursorIcon::PointingHand);
    if star
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(favorite_label(favorite))
        .clicked()
    {
        *command = Some(Command::Star(entry.theme(), entry.pinned));
    }
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let response = if truncated {
        response.on_hover_text(entry.name)
    } else {
        response
    };
    if response.clicked() {
        *command = Some(Command::Use(entry.theme()));
    }
}

fn favorite_label(favorite: bool) -> &'static str {
    if favorite {
        "Remove from favorites"
    } else {
        "Add to favorites"
    }
}

fn card_menu(
    ui: &mut Ui,
    p: Palette,
    entry: &Entry,
    config: &Config,
    chosen: &Theme,
    command: &mut Option<Command>,
) {
    menu_layout(ui, 190.0);
    let room = config.custom_themes.len() < MAX_CUSTOM_THEMES;
    let custom = match &entry.source {
        Source::Custom(custom) => Some(*custom),
        _ => None,
    };
    if !entry.is(chosen) && menu_item(ui, p, Icon::Check, "Use theme", "", false) {
        *command = Some(Command::Use(entry.theme()));
        ui.close();
    }
    let favorite = entry.is_favorite(config);
    if menu_item(ui, p, Icon::Star, favorite_label(favorite), "", false) {
        *command = Some(Command::Star(entry.theme(), entry.pinned));
        ui.close();
    }
    if let Some(custom) = custom
        && menu_item(ui, p, Icon::Pencil, "Edit…", "", false)
    {
        *command = Some(Command::Edit(custom.id.clone()));
        ui.close();
    }
    // A copy is how an imported theme becomes editable.
    if ui
        .add_enabled_ui(room, |ui| {
            menu_item(ui, p, Icon::Copy, "Duplicate…", "", false)
        })
        .inner
    {
        let colors = match &entry.source {
            Source::Custom(custom) => custom.colors,
            Source::Bundled(bundled) => bundled.colors,
            Source::Neptune(_) => colors_of(&entry.look(config.accent)),
        };
        *command = Some(Command::Copy(entry.name.to_owned(), colors));
        ui.close();
    }
    if let Some(custom) = custom {
        menu_separator(ui, p);
        if menu_item(ui, p, Icon::Close, "Delete…", "", true) {
            *command = Some(Command::Delete(custom.id.clone()));
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        theme::apply(&ctx, &Config::default());
        ctx
    }

    /// One frame of the catalog or the editor in a sheet-sized window.
    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        config: &mut Config,
        state: &mut State,
    ) -> bool {
        let p = Palette::for_config(config);
        let mut close = false;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(WIDTH, 640.0))),
                events,
                ..Default::default()
            },
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                close = show(ui, p, config, state, 460.0);
            },
        );
        output.textures_delta.clear();
        close
    }

    fn click(ctx: &egui::Context, pos: Pos2, config: &mut Config, state: &mut State) -> bool {
        frame(ctx, vec![egui::Event::PointerMoved(pos)], config, state);
        let mut close = false;
        for pressed in [true, false] {
            close |= frame(
                ctx,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                config,
                state,
            );
        }
        close
    }

    fn bundled_card(name: &'static str) -> Id {
        Entry::bundled(bundled(name).unwrap()).id()
    }

    #[test]
    fn the_catalog_lays_out_only_rows_in_view_and_applies_a_clicked_theme() {
        let ctx = context();
        let mut config = Config::default();
        let mut state = State::default();
        state.browse(None);
        frame(&ctx, vec![], &mut config, &mut state);
        let dusk = Entry::neptune((Theme::Dusk, "Dusk")).id();
        let target = ctx.read_response(dusk).expect("Dusk is in view").rect;
        assert!(
            ctx.read_response(bundled_card("Zenburn")).is_none(),
            "a theme far down the list is not laid out"
        );
        click(&ctx, target.center(), &mut config, &mut state);
        assert_eq!(config.theme, Theme::Dusk);
        assert!(state.open && !state.editing());

        // Searching brings a distant theme into view; choosing it applies it.
        state.query = "zenburn".into();
        // Responses are remembered for one frame after a widget is gone.
        frame(&ctx, vec![], &mut config, &mut state);
        frame(&ctx, vec![], &mut config, &mut state);
        let target = ctx
            .read_response(bundled_card("Zenburn"))
            .expect("the match is laid out")
            .rect;
        assert!(ctx.read_response(dusk).is_none());
        click(&ctx, target.center(), &mut config, &mut state);
        assert_eq!(config.theme, Theme::Palette("iterm:Zenburn".into()));
    }

    #[test]
    fn a_star_keeps_a_theme_without_applying_it_and_the_card_stays_put() {
        let ctx = context();
        let mut config = Config::default();
        let mut state = State::default();
        state.browse(None);
        frame(&ctx, vec![], &mut config, &mut state);
        let dusk = Entry::neptune((Theme::Dusk, "Dusk")).id();
        let pinned = Entry::favorite(&config, &Theme::Dusk).unwrap().id();
        let card = ctx.read_response(dusk).unwrap().rect;
        let star = ctx.read_response(dusk.with("star")).unwrap().rect;
        click(&ctx, star.center(), &mut config, &mut state);
        assert_eq!(config.favorite_themes, [Theme::Dusk]);
        assert_eq!(config.theme, Theme::Graphite, "a star does not choose");

        // The favorites now lead the catalog, above a card that has not moved.
        frame(&ctx, vec![], &mut config, &mut state);
        assert_eq!(ctx.read_response(dusk).unwrap().rect, card);
        assert!(ctx.read_response(pinned).is_none(), "scrolled out above");

        // The filter lists them alone; the star there lets one go.
        state.filter = Filter::Favorites;
        frame(&ctx, vec![], &mut config, &mut state);
        frame(&ctx, vec![], &mut config, &mut state);
        assert!(ctx.read_response(dusk).is_none());
        let star = ctx.read_response(pinned.with("star")).unwrap().rect;
        click(&ctx, star.center(), &mut config, &mut state);
        assert!(config.favorite_themes.is_empty());
        frame(&ctx, vec![], &mut config, &mut state);
        frame(&ctx, vec![], &mut config, &mut state);
        assert!(ctx.read_response(pinned).is_none());
    }

    #[test]
    fn favorites_lead_while_browsing_and_a_search_lists_each_theme_once() {
        let mut config = Config {
            favorite_themes: vec![
                Theme::Palette("iterm:Zenburn".into()),
                Theme::Light,
                // Left behind by a theme that no longer exists.
                Theme::Palette("custom:9".into()),
            ],
            ..Config::default()
        };
        let mut state = State::default();
        let names = |config: &Config, state: &State| -> Vec<(String, bool)> {
            sections(config, state, 3)
                .entries
                .iter()
                .map(|entry| (entry.name.to_owned(), entry.pinned))
                .collect()
        };
        let all = names(&config, &state);
        assert_eq!(all.len(), 2 + 3 + BUNDLED.len());
        assert_eq!(
            all[..3],
            [
                ("Zenburn".into(), true),
                ("Light".into(), true),
                ("Graphite".into(), false)
            ]
        );
        let list = sections(&config, &state, 3);
        assert!(matches!(list.rows[0].1, Row::Label("Favorites", 2)));
        // The theme in use is revealed among the favorites when it is one.
        assert_eq!(list.row_of(&Theme::Light), Some(LABEL_HEIGHT));

        state.filter = Filter::Light;
        assert_eq!(
            names(&config, &state)[..2],
            [("Light".into(), true), ("Light".into(), false)]
        );
        state.filter = Filter::All;
        state.query = "zenburn".into();
        assert_eq!(
            names(&config, &state),
            [("Zenburn".into(), false), ("Zenburned".into(), false)]
        );

        state.filter = Filter::Favorites;
        assert_eq!(names(&config, &state), [("Zenburn".into(), true)]);
        state.query.clear();
        assert_eq!(names(&config, &state).len(), 2);
        config.favorite_themes.clear();
        assert!(names(&config, &state).is_empty());
    }

    #[test]
    fn the_catalog_opens_at_the_theme_in_use() {
        let ctx = context();
        let mut config = Config {
            theme: Theme::Palette("iterm:Zenburn".into()),
            ..Config::default()
        };
        let mut state = State::default();
        state.browse(None);
        frame(&ctx, vec![], &mut config, &mut state);
        frame(&ctx, vec![], &mut config, &mut state);
        assert!(ctx.read_response(bundled_card("Zenburn")).is_some());
        assert!(!state.reveal, "revealed once, then scrolling is the user's");
    }

    #[test]
    fn a_mode_catalog_chooses_the_theme_for_that_mode_alone() {
        let ctx = context();
        let mut config = Config {
            match_system_appearance: true,
            system_appearance: Some(Appearance::Light),
            ..Config::default()
        };
        let mut state = State::default();
        state.browse(Some(Appearance::Dark));
        frame(&ctx, vec![], &mut config, &mut state);
        let dusk = Entry::neptune((Theme::Dusk, "Dusk")).id();
        let target = ctx.read_response(dusk).expect("Dusk is in view").rect;
        click(&ctx, target.center(), &mut config, &mut state);
        assert_eq!(config.dark_theme, Theme::Dusk);
        assert_eq!(config.light_theme, Theme::Light);
        assert_eq!(config.theme, Theme::Graphite, "the single theme is kept");
        assert_eq!(config.active_theme(), &Theme::Light, "the system is light");
    }

    #[test]
    fn deleting_a_custom_theme_returns_each_choice_of_it_to_its_default() {
        let custom = CustomTheme {
            id: "custom:1".into(),
            name: "Paper".into(),
            colors: bundled("3024 Day").unwrap().colors,
        };
        let chosen = Theme::Palette(custom.id.clone());
        let mut config = Config {
            theme: chosen.clone(),
            light_theme: chosen.clone(),
            dark_theme: Theme::Dusk,
            custom_themes: vec![custom],
            ..Config::default()
        };
        assert_eq!(
            deletion_consequence(&config, "custom:1"),
            "Neptune switches to Graphite."
        );
        config.match_system_appearance = true;
        assert_eq!(
            deletion_consequence(&config, "custom:1"),
            "Light mode switches to Light."
        );
        config.dark_theme = chosen;
        assert_eq!(
            deletion_consequence(&config, "custom:1"),
            "Light mode switches to Light. Dark mode switches to Graphite."
        );
        assert_eq!(
            deletion_consequence(&config, "custom:2"),
            "This cannot be undone."
        );
        delete_custom(&mut config, "custom:1");
        assert_eq!(
            (&config.theme, &config.light_theme, &config.dark_theme),
            (&Theme::Graphite, &Theme::Light, &Theme::Graphite)
        );
        config.validate().unwrap();
    }

    #[test]
    fn escape_asks_before_discarding_a_changed_draft() {
        let config = Config::default();
        let mut state = State::default();
        state.browse(None);
        state.draft = Some(Draft::copy(
            &config,
            "My theme",
            colors_of(&Palette::for_config(&config)),
        ));
        // Unchanged: nothing to lose.
        assert!(state.may_close());
        assert!(state.back());
        assert!(!state.editing() && state.open);

        state.draft = Some(Draft::changed(&config));
        assert!(!state.may_close());
        assert!(state.draft.as_ref().unwrap().discarding);
        // Escape answers the question with "keep editing".
        assert!(state.back());
        assert!(state.editing() && !state.draft.as_ref().unwrap().discarding);
        assert!(state.back());
        assert!(state.editing() && state.draft.as_ref().unwrap().discarding);

        state.draft = None;
        state.deleting = Some("custom:1".into());
        assert!(state.back());
        assert!(state.deleting.is_none() && state.open);
        assert!(state.back());
        assert!(!state.open && !state.back());
    }

    #[test]
    fn built_in_palettes_survive_the_trip_into_a_custom_theme() {
        for (theme, _) in Theme::BUILTINS {
            let look = Palette::with_accent(&theme, Accent::default());
            let copy = Palette::from_colors(colors_of(&look));
            assert_eq!(copy.bg, look.bg);
            assert_eq!(copy.terminal_fg, look.terminal_fg);
            assert_eq!(copy.cursor, look.cursor);
            assert_eq!(copy.selection, look.selection);
            assert_eq!(copy.ansi, look.ansi);
        }
    }
}
