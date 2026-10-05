//! Renderer-owned row hashes, galleys and preparation.

use crate::{config::Config, theme::Palette};
use eframe::egui::{self, Color32, Rect, Stroke, Vec2};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
};
use terminal_core::{
    Color, Colors, CursorShape, Flags, Mode as TermMode, NamedColor, ViewportSnapshot,
};
#[derive(Default)]
pub struct Cache {
    pub(super) rows: Vec<CachedRow>,
    pub(super) sources: Vec<Arc<[terminal_core::Cell]>>,
    colors: Option<Colors>,
    palette_colors: Option<[Color32; 19]>,
    revision: u64,
    key: Option<CacheKey>,
    pub columns: u16,
    pub lines: u16,
    pub display_offset: usize,
    pub cursor: Option<(usize, usize, CursorShape)>,
    pub mode: TermMode,
    pub cell: Vec2,
    pub render_rebuilds: u64,
    pub cells_prepared: u64,
    pub(super) selection: Option<terminal_core::SelectionRange>,
    pub resize_error: Option<String>,
    pub(super) background: Color32,
    pub(super) cursor_color: Color32,
    pub(super) pressed_link: Option<super::links::Link>,
    /// The current primary gesture belongs to a link, including its release frame.
    pub link_pointer_owned: bool,
    pub(super) hints: Option<super::hints::Hints>,
}

#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    geometry: super::geometry::ResizeRequest,
    font_size: u32,
    line_height: u32,
    display_scale: u32,
}

impl CacheKey {
    fn same_layout(self, other: Self) -> bool {
        self.geometry.columns == other.geometry.columns
            && self.geometry.lines == other.geometry.lines
            && self.font_size == other.font_size
            && self.line_height == other.line_height
            && self.display_scale == other.display_scale
    }
}
#[derive(Default)]
pub(super) struct CachedRow {
    pub(super) hash: u64,
    pub(super) runs: Vec<Run>,
    pub(super) backgrounds: Vec<(usize, usize, Color32)>,
    pub(super) text: String,
    pub(super) text_columns: Vec<(usize, usize)>,
}
pub(super) struct Run {
    pub(super) column: usize,
    pub(super) galley: Arc<egui::Galley>,
}
#[derive(Clone)]
struct Cell {
    pub(super) column: usize,
    c: char,
    extra: Vec<char>,
    fg: Color32,
    bg: Color32,
    flags: Flags,
}

fn resolve(c: Color, p: Palette, bold: bool, colors: &Colors) -> Color32 {
    let index = match c {
        Color::Named(n) => Some(if bold && (n as usize) < 8 {
            n as usize + 8
        } else {
            n as usize
        }),
        Color::Indexed(n) => Some(n as usize),
        _ => None,
    };
    if let Some(index) = index
        && let Some(rgb) = colors[index]
    {
        return Color32::from_rgb(rgb.r, rgb.g, rgb.b);
    }
    match c {
        Color::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        Color::Named(NamedColor::Foreground) => {
            if bold {
                p.terminal_bold
            } else {
                p.terminal_fg
            }
        }
        Color::Named(NamedColor::BrightForeground) => p.terminal_bold,
        Color::Named(NamedColor::DimForeground) => p.terminal_fg.gamma_multiply(0.65),
        Color::Named(NamedColor::Background) => p.bg,
        Color::Named(NamedColor::Cursor) => p.cursor,
        Color::Named(n) => {
            let n = n as usize;
            if n < 16 {
                p.ansi[if bold && n < 8 { n + 8 } else { n }]
            } else {
                p.secondary
            }
        }
        Color::Indexed(n) => {
            if n < 16 {
                p.ansi[n as usize]
            } else if n >= 232 {
                Color32::from_gray(8 + (n - 232) * 10)
            } else {
                let n = n - 16;
                let f = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Color32::from_rgb(f(n / 36), f((n / 6) % 6), f(n % 6))
            }
        }
    }
}

impl Cache {
    /// Releases retained draw allocations for hidden panes. The next preparation
    /// recreates rows from the owned snapshot without changing terminal state.
    pub fn evict(&mut self) {
        self.rows = Vec::new();
        self.sources = Vec::new();
        self.colors = None;
        self.key = None;
        self.revision = u64::MAX;
        self.hints = None;
        self.pressed_link = None;
        self.link_pointer_owned = false;
    }

    /// Conservative retained CPU allocation estimate for aggregate diagnostics.
    /// Shared galleys may be counted by several panes; terminal-owned source rows,
    /// global font atlases, GPU textures and allocator overhead are excluded.
    pub fn estimated_bytes(&self) -> usize {
        let storage = std::mem::size_of::<Self>()
            + self.rows.capacity() * std::mem::size_of::<CachedRow>()
            + self.sources.capacity() * std::mem::size_of::<Arc<[terminal_core::Cell]>>()
            + self
                .hints
                .as_ref()
                .map_or(0, super::hints::Hints::estimated_bytes);
        storage + self.rows.iter().map(|row| {
            row.text.capacity()
                + row.text_columns.capacity() * std::mem::size_of::<(usize, usize)>()
                + row.backgrounds.capacity() * std::mem::size_of::<(usize, usize, Color32)>()
                + row.runs.capacity() * std::mem::size_of::<Run>()
                + row.runs.iter().map(|run| {
                    let galley = &run.galley;
                    std::mem::size_of::<egui::Galley>()
                        + std::mem::size_of::<egui::text::LayoutJob>()
                        + galley.job.text.capacity()
                        + galley.job.sections.capacity() * std::mem::size_of::<egui::text::LayoutSection>()
                        + galley.rows.capacity() * std::mem::size_of::<egui::epaint::text::PlacedRow>()
                        + galley.rows.iter().map(|placed| {
                            std::mem::size_of::<egui::epaint::text::Row>()
                                + placed.glyphs.capacity() * std::mem::size_of::<egui::epaint::text::Glyph>()
                                + placed.visuals.mesh.vertices.capacity() * std::mem::size_of::<egui::epaint::Vertex>()
                                + placed.visuals.mesh.indices.capacity() * std::mem::size_of::<u32>()
                        }).sum::<usize>()
                }).sum::<usize>()
        }).sum::<usize>()
    }

    pub fn invalidate(&mut self) {
        self.revision = u64::MAX;
    }
    /// The terminal's resolved background after the latest preparation, so the
    /// surrounding pane can match a program that changes it.
    pub fn background(&self) -> Color32 {
        self.background
    }
    pub fn retry_resize(&mut self) {
        self.key = None;
    }

    /// Measures desired geometry. The controller owns applying the resize.
    pub fn geometry(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        config: &Config,
    ) -> Option<super::geometry::ResizeRequest> {
        let font = crate::platform::fonts::terminal_font(config.font_size, false);
        self.cell = Vec2::new(
            ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'M')),
            (config.font_size * config.line_height).round(),
        );
        let geometry = super::geometry::calculate(rect, self.cell, ui.ctx().pixels_per_point());
        let key = CacheKey {
            geometry,
            font_size: config.font_size.to_bits(),
            line_height: config.line_height.to_bits(),
            display_scale: ui.ctx().pixels_per_point().to_bits(),
        };
        if self.key == Some(key) {
            return None;
        }
        if self.key.is_none_or(|previous| !previous.same_layout(key)) {
            self.hints = None;
            self.rows.clear();
            self.sources.clear();
            self.revision = u64::MAX;
        }
        self.key = Some(key);
        Some(geometry)
    }

    /// Shapes changed rows from owned snapshots after the engine lock is released.
    pub fn prepare(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &ViewportSnapshot,
        config: &Config,
        p: Palette,
    ) {
        if self.hints.is_some() {
            return;
        }
        let font = crate::platform::fonts::terminal_font(config.font_size, false);
        self.columns = snapshot.columns as u16;
        self.lines = snapshot.screen_lines as u16;
        self.display_offset = snapshot.display_offset;
        self.mode = snapshot.mode;
        self.selection = snapshot.selection;
        self.background = resolve(
            Color::Named(NamedColor::Background),
            p,
            false,
            &snapshot.colors,
        );
        self.cursor_color = resolve(Color::Named(NamedColor::Cursor), p, false, &snapshot.colors);
        let point = snapshot.cursor.point;
        let cursor_row = point.line + snapshot.display_offset as i32;
        self.cursor = if snapshot.cursor.shape != CursorShape::Hidden
            && snapshot.mode.contains(TermMode::SHOW_CURSOR)
            && (0..self.lines as i32).contains(&cursor_row)
        {
            Some((point.column, cursor_row as usize, snapshot.cursor.shape))
        } else {
            None
        };
        let mut palette_colors = [p.bg; 19];
        palette_colors[..16].copy_from_slice(&p.ansi);
        palette_colors[16] = p.bg;
        palette_colors[17] = p.terminal_fg;
        palette_colors[18] = p.terminal_bold;
        let palette_changed = self.palette_colors != Some(palette_colors);
        self.palette_colors = Some(palette_colors);
        if !palette_changed
            && snapshot.revision == self.revision
            && self.rows.len() == snapshot.screen_lines
        {
            return;
        }
        let lines = self.lines;
        let revision = snapshot.revision;
        let colors_changed = palette_changed || self.colors.as_ref() != Some(&snapshot.colors);
        self.rows.resize_with(lines as usize, CachedRow::default);
        for (idx, source) in snapshot.rows.iter().enumerate().take(lines as usize) {
            if !colors_changed
                && self
                    .sources
                    .get(idx)
                    .is_some_and(|old| Arc::ptr_eq(old, source))
            {
                continue;
            }
            self.cells_prepared += source.len() as u64;
            let cells = source
                .iter()
                .map(|c| {
                    let bold = c.flags.contains(Flags::BOLD);
                    let mut fg = resolve(c.fg, p, bold, &snapshot.colors);
                    let mut bg = resolve(c.bg, p, false, &snapshot.colors);
                    if c.flags.contains(Flags::INVERSE) {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    if c.flags.contains(Flags::DIM) {
                        fg = fg.gamma_multiply(0.65);
                    }
                    Cell {
                        column: c.column,
                        c: c.c,
                        extra: c.extra.clone(),
                        fg,
                        bg,
                        flags: c.flags,
                    }
                })
                .collect::<Vec<_>>();
            let mut hasher = DefaultHasher::new();
            for c in &cells {
                c.column.hash(&mut hasher);
                c.c.hash(&mut hasher);
                c.extra.hash(&mut hasher);
                c.fg.hash(&mut hasher);
                c.bg.hash(&mut hasher);
                c.flags.bits().hash(&mut hasher);
            }
            let hash = hasher.finish();
            if self.rows[idx].hash == hash {
                continue;
            }
            self.render_rebuilds += 1;
            let mut row = CachedRow {
                hash,
                ..Default::default()
            };
            let mut job = egui::text::LayoutJob::default();
            let mut start = 0;
            let mut last_col = usize::MAX;
            let mut last_format = None;
            for c in cells {
                if c.bg != self.background {
                    if row
                        .backgrounds
                        .last()
                        .is_some_and(|(_, end, color)| *end == c.column && *color == c.bg)
                    {
                        if let Some(last) = row.backgrounds.last_mut() {
                            last.1 += 1;
                        }
                    } else {
                        row.backgrounds.push((c.column, c.column + 1, c.bg));
                    }
                }
                if c.flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                // Concealed glyphs must remain hidden when selection/cursor ink
                // overrides a galley's color. Keep cell offsets, not secret glyphs.
                let mut text = if c.flags.contains(Flags::HIDDEN) {
                    " ".to_owned()
                } else {
                    c.c.to_string()
                };
                if !c.flags.contains(Flags::HIDDEN) {
                    for ch in c.extra {
                        text.push(ch);
                    }
                }
                row.text_columns.push((row.text.len(), c.column));
                row.text.push_str(&text);
                if c.c == ' ' && job.text.is_empty() {
                    continue;
                }
                let format = egui::text::TextFormat {
                    font_id: if c.flags.contains(Flags::BOLD) {
                        crate::platform::fonts::terminal_font(config.font_size, true)
                    } else {
                        font.clone()
                    },
                    color: if c.flags.contains(Flags::HIDDEN) {
                        c.bg
                    } else {
                        c.fg
                    },
                    italics: c.flags.contains(Flags::ITALIC),
                    underline: if c.flags.intersects(
                        Flags::UNDERLINE
                            | Flags::DOUBLE_UNDERLINE
                            | Flags::UNDERCURL
                            | Flags::DOTTED_UNDERLINE
                            | Flags::DASHED_UNDERLINE,
                    ) {
                        Stroke::new(1.0, c.fg)
                    } else {
                        Stroke::NONE
                    },
                    strikethrough: if c.flags.contains(Flags::STRIKEOUT) {
                        Stroke::new(1.0, c.fg)
                    } else {
                        Stroke::NONE
                    },
                    ..Default::default()
                };
                let ascii = c.c.is_ascii() && text.len() == 1;
                if !job.text.is_empty()
                    && (!ascii || c.column != last_col + 1 || last_format.as_ref() != Some(&format))
                {
                    row.runs.push(Run {
                        column: start,
                        galley: ui.painter().layout_job(std::mem::take(&mut job)),
                    });
                }
                if job.text.is_empty() {
                    start = c.column;
                }
                job.append(&text, 0.0, format.clone());
                last_col = c.column;
                last_format = Some(format);
                if !ascii {
                    row.runs.push(Run {
                        column: start,
                        galley: ui.painter().layout_job(std::mem::take(&mut job)),
                    });
                }
            }
            if !job.text.is_empty() {
                row.runs.push(Run {
                    column: start,
                    galley: ui.painter().layout_job(job),
                });
            }
            self.rows[idx] = row;
        }
        self.sources = snapshot.rows.clone();
        self.colors = Some(snapshot.colors);
        self.revision = revision;
    }
    pub fn matches(&self, search: &str) -> usize {
        if search.is_empty() {
            0
        } else {
            self.rows
                .iter()
                .map(|r| r.text.matches(search).count())
                .sum()
        }
    }
}

#[cfg(test)]
mod synthetic_tests {
    use super::*;
    use eframe::egui::Pos2;
    use terminal_core::{Cell as SnapshotCell, Cursor, Point, SelectionRange};

    struct Fixture {
        context: egui::Context,
        config: Config,
        cache: Cache,
        snapshot: ViewportSnapshot,
        rect: Rect,
    }

    impl Fixture {
        fn new() -> Self {
            let context = egui::Context::default();
            context.set_fonts(crate::platform::fonts::bundled_definitions());
            let mut fixture = Self {
                context,
                config: Config::default(),
                cache: Cache::default(),
                snapshot: ViewportSnapshot::blank(32, 3),
                rect: Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(320.0, 90.0)),
            };
            fixture.row(0, "alpha");
            fixture.row(1, "bravo");
            fixture.frame("");
            fixture.frame("");
            fixture
        }

        fn row(&mut self, index: usize, text: &str) {
            let mut row = self.snapshot.rows[index].to_vec();
            for (cell, character) in row.iter_mut().zip(text.chars()) {
                cell.c = character;
            }
            self.snapshot.rows[index] = row.into();
            self.snapshot.revision += 1;
        }

        fn frame(&mut self, search: &str) -> Vec<egui::epaint::ClippedShape> {
            let mut output = self.context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 160.0))),
                    ..Default::default()
                },
                |ui| {
                    self.cache.geometry(ui, self.rect, &self.config);
                    self.cache.prepare(
                        ui,
                        &self.snapshot,
                        &self.config,
                        Palette::for_config(&self.config),
                    );
                    self.cache.paint(
                        ui,
                        self.rect,
                        &self.config,
                        Palette::for_config(&self.config),
                        false,
                        search,
                        "",
                    );
                },
            );
            output.textures_delta.clear();
            output.shapes
        }
    }

    #[test]
    fn font_reload_reshapes_quiet_rows_and_remeasures_the_grid() {
        let mut fixture = Fixture::new();
        let revision = fixture.snapshot.revision;
        let original = fixture.cache.rows[0].runs[0].galley.clone();
        let cell = fixture.cache.cell;
        let mut definitions = crate::platform::fonts::bundled_definitions();
        definitions.families.insert(
            egui::FontFamily::Name("Terminal".into()),
            egui::FontDefinitions::default().families[&egui::FontFamily::Monospace].clone(),
        );
        fixture.context.set_fonts(definitions);
        fixture.cache.retry_resize();
        fixture.frame("");
        assert_eq!(fixture.snapshot.revision, revision);
        assert_ne!(fixture.cache.cell.x, cell.x);
        assert!(!Arc::ptr_eq(
            &original,
            &fixture.cache.rows[0].runs[0].galley
        ));
        let refreshed = fixture.cache.rows[0].runs[0].galley.clone();
        fixture.frame("");
        assert!(Arc::ptr_eq(
            &refreshed,
            &fixture.cache.rows[0].runs[0].galley
        ));
    }

    #[test]
    fn theme_changes_recolor_quiet_rows_and_preserve_explicit_terminal_colors() {
        let mut fixture = Fixture::new();
        let revision = fixture.snapshot.revision;
        let original = fixture.cache.rows[0].runs[0].galley.clone();
        fixture.config.theme = crate::config::Theme::Palette("iterm:Dracula".into());
        fixture.frame("");
        assert_eq!(fixture.snapshot.revision, revision);
        assert!(!Arc::ptr_eq(
            &original,
            &fixture.cache.rows[0].runs[0].galley
        ));
        assert_eq!(fixture.cache.background(), crate::theme::color(0x282a36));
        assert_eq!(
            fixture.cache.rows[0].runs[0].galley.job.sections[0]
                .format
                .color,
            crate::theme::color(0xf8f8f2)
        );
        fixture.snapshot.colors[NamedColor::Foreground as usize] =
            Some(terminal_core::Rgb { r: 1, g: 2, b: 3 });
        fixture.snapshot.revision += 1;
        fixture.frame("");
        fixture.config.theme = crate::config::Theme::Palette("iterm:3024 Day".into());
        fixture.frame("");
        assert_eq!(
            fixture.cache.rows[0].runs[0].galley.job.sections[0]
                .format
                .color,
            Color32::from_rgb(1, 2, 3)
        );
        fixture.snapshot.colors[NamedColor::Foreground as usize] = None;
        fixture.snapshot.revision += 1;
        fixture.frame("");
        assert_eq!(
            fixture.cache.rows[0].runs[0].galley.job.sections[0]
                .format
                .color,
            Palette::for_config(&fixture.config).terminal_fg
        );
    }

    #[test]
    fn custom_edits_invalidate_same_theme_id_without_terminal_output() {
        let mut fixture = Fixture::new();
        fixture
            .config
            .custom_themes
            .push(crate::terminal_theme::CustomTheme {
                id: "custom:1".into(),
                name: "Test".into(),
                colors: crate::terminal_theme::bundled("Dracula").unwrap().colors,
            });
        fixture.config.theme = crate::config::Theme::Palette("custom:1".into());
        fixture.frame("");
        fixture.config.custom_themes[0].colors.foreground =
            crate::terminal_theme::HexColor(0x123456);
        fixture.frame("");
        assert_eq!(
            fixture.cache.rows[0].runs[0].galley.job.sections[0]
                .format
                .color,
            crate::theme::color(0x123456)
        );
        let rebuilt = fixture.cache.render_rebuilds;
        fixture.frame("");
        assert_eq!(fixture.cache.render_rebuilds, rebuilt);
    }

    #[test]
    fn selection_ink_never_reveals_concealed_glyphs() {
        let mut fixture = Fixture::new();
        let row = Arc::make_mut(&mut fixture.snapshot.rows[0]);
        row[0].flags.insert(Flags::HIDDEN);
        fixture.snapshot.revision += 1;
        fixture.config.theme = crate::config::Theme::Palette("iterm:Dracula".into());
        fixture.frame("");
        assert!(fixture.cache.rows[0].text.starts_with(" lpha"));
        assert!(
            fixture.cache.rows[0]
                .runs
                .iter()
                .all(|run| !run.galley.job.text.contains("alpha"))
        );
    }

    #[test]
    fn shared_rows_skip_preparation_and_only_changed_row_is_shaped() {
        let mut fixture = Fixture::new();
        let first = fixture.cache.rows[0].runs[0].galley.clone();
        let prepared = fixture.cache.cells_prepared;
        let rebuilds = fixture.cache.render_rebuilds;
        fixture.row(1, "changed");
        fixture.frame("");
        assert_eq!(fixture.cache.cells_prepared - prepared, 32);
        assert_eq!(fixture.cache.render_rebuilds - rebuilds, 1);
        assert!(Arc::ptr_eq(&first, &fixture.cache.rows[0].runs[0].galley));
    }

    #[test]
    fn pixel_only_resize_requests_update_without_reshaping_text() {
        let mut fixture = Fixture::new();
        fixture.rect.max.x = fixture.rect.left() + fixture.cache.cell.x * 40.0 + 2.0;
        fixture.frame("");
        let galley = fixture.cache.rows[0].runs[0].galley.clone();
        let rebuilds = fixture.cache.render_rebuilds;
        let previous = fixture.cache.key.unwrap().geometry;
        fixture.rect.max.x += 1.0;
        let mut request = None;
        let _ = fixture.context.run_ui(egui::RawInput::default(), |ui| {
            request = fixture.cache.geometry(ui, fixture.rect, &fixture.config);
        });
        let request = request.expect("pixel dimensions changed within the same grid");
        assert_eq!(
            (request.columns, request.lines),
            (previous.columns, previous.lines)
        );
        assert_eq!(request.pixel_width, previous.pixel_width + 1);
        fixture.frame("");
        assert_eq!(fixture.cache.render_rebuilds, rebuilds);
        assert!(Arc::ptr_eq(&galley, &fixture.cache.rows[0].runs[0].galley));
        assert!(fixture.cache.estimated_bytes() > Cache::default().estimated_bytes());
    }

    #[test]
    fn evicted_hidden_cache_releases_allocations_and_restores_visible_text() {
        let mut fixture = Fixture::new();
        let used = fixture.cache.estimated_bytes();
        let rebuilds = fixture.cache.render_rebuilds;
        fixture.cache.evict();
        assert!(fixture.cache.estimated_bytes() < used);
        fixture.frame("");
        assert!(fixture.cache.rows[0].text.starts_with("alpha"));
        assert!(fixture.cache.rows[1].text.starts_with("bravo"));
        assert!(fixture.cache.render_rebuilds > rebuilds);
    }

    #[test]
    fn cursor_and_selection_damage_preserve_prepared_text() {
        let mut fixture = Fixture::new();
        let prepared = fixture.cache.cells_prepared;
        let rebuilds = fixture.cache.render_rebuilds;
        fixture.snapshot.cursor = Cursor {
            point: Point::new(1, 4),
            shape: CursorShape::Beam,
        };
        fixture.snapshot.selection = Some(SelectionRange {
            start: Point::new(0, 1),
            end: Point::new(0, 3),
            is_block: false,
        });
        fixture.snapshot.revision += 1;
        let shapes = fixture.frame("");
        assert_eq!(fixture.cache.cells_prepared, prepared);
        assert_eq!(fixture.cache.render_rebuilds, rebuilds);
        assert_eq!(fixture.cache.cursor, Some((4, 1, CursorShape::Beam)));
        let selection = Palette::for_config(&fixture.config).selection;
        assert_eq!(shapes.iter().filter(|shape|matches!(&shape.shape,egui::epaint::Shape::Rect(rect) if rect.fill==selection)).count(),3);
    }

    #[test]
    fn unicode_highlights_include_wide_and_combining_cell_columns() {
        let mut fixture = Fixture::new();
        let mut row = fixture.snapshot.rows[0].to_vec();
        row[0].c = 'A';
        row[1].c = '界';
        row[1].flags = Flags::WIDE_CHAR;
        row[2].c = ' ';
        row[2].flags = Flags::WIDE_CHAR_SPACER;
        row[3].c = 'e';
        row[3].extra = vec!['\u{301}'];
        row[4].c = 'B';
        fixture.snapshot.rows[0] = row.into();
        fixture.snapshot.revision += 1;
        for (search, column, width) in [("界", 1.0, 2.0), ("e\u{301}", 3.0, 1.0)] {
            let fill = Palette::for_config(&fixture.config)
                .accent
                .gamma_multiply(0.25);
            let shapes = fixture.frame(search);
            let highlights: Vec<_> = shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Rect(rect) if rect.fill == fill => Some(rect.rect),
                    _ => None,
                })
                .collect();
            assert_eq!(
                highlights,
                vec![Rect::from_min_size(
                    fixture.rect.min + Vec2::new(column * fixture.cache.cell.x, 0.0),
                    Vec2::new(width * fixture.cache.cell.x, fixture.cache.cell.y)
                )]
            );
        }
    }

    #[test]
    fn scale_theme_and_session_replacement_invalidate_layouts() {
        let mut fixture = Fixture::new();
        let before = fixture.cache.render_rebuilds;
        fixture.context.set_pixels_per_point(2.0);
        fixture.frame("");
        assert!(fixture.cache.render_rebuilds > before);
        let before = fixture.cache.render_rebuilds;
        fixture.config.theme = crate::config::Theme::Light;
        fixture.frame("");
        assert!(fixture.cache.render_rebuilds > before);
        fixture.snapshot = ViewportSnapshot::blank(32, 3);
        fixture.snapshot.rows[0] = vec![SnapshotCell {
            column: 0,
            c: 'Z',
            ..SnapshotCell::default()
        }]
        .into();
        fixture.cache.invalidate();
        fixture.frame("");
        assert_eq!(fixture.cache.rows[0].text, "Z");
    }

    #[test]
    fn palette_updates_recolor_shared_immutable_rows() {
        let mut fixture = Fixture::new();
        let before = fixture.cache.rows[0].hash;
        fixture.snapshot.colors[NamedColor::Foreground as usize] = Some(terminal_core::Rgb {
            r: 200,
            g: 20,
            b: 30,
        });
        fixture.snapshot.revision += 1;
        fixture.frame("");
        assert_ne!(fixture.cache.rows[0].hash, before);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, Rect};
    use std::time::{Duration, Instant};
    use terminal_core::{SessionOptions, SessionStatus, TerminalSession};

    /// Exercise the real PTY/parser boundary without a login shell, prompt,
    /// command echo, timing-dependent sleeps, or a window/GPU requirement.
    struct RendererFixture {
        session: TerminalSession,
        ctx: egui::Context,
        config: Config,
        cache: Cache,
        rect: Rect,
        deadline: Instant,
    }

    impl RendererFixture {
        fn new() -> Self {
            let deadline = Instant::now() + Duration::from_secs(10);
            let session = TerminalSession::spawn(
                SessionOptions {
                    shell: Some("/bin/sh".into()),
                    args: vec![
                        "-c".into(),
                        "stty -echo; printf '\\033[2J\\033[H\\033]0;cache-ready\\007'; \
                         while IFS= read -r command; do eval \"$command\"; done"
                            .into(),
                    ],
                    cols: 80,
                    rows: 8,
                    ..SessionOptions::default()
                },
                Arc::new(|| {}),
            )
            .expect("start renderer PTY fixture");
            let ctx = egui::Context::default();
            let config = Config::default();
            crate::theme::fonts(&ctx);
            crate::theme::apply(&ctx, &config);
            let mut fixture = Self {
                session,
                ctx,
                config,
                cache: Cache::default(),
                rect: Rect::from_min_size(Pos2::new(16.0, 16.0), Vec2::new(640.0, 160.0)),
                deadline,
            };
            fixture.wait_for_title("cache-ready");
            // Establish the renderer's cell-derived PTY dimensions before text
            // is written, then settle the initial cache/font frame.
            fixture.frame("");
            fixture.frame("");
            fixture.output(
                "\\033[2J\\033[Halpha\\033[2;1Hbravo\\033[3;1Hcharlie\\033[1;1H",
                "seeded",
            );
            fixture.frame("");
            fixture.frame("");
            assert!(fixture.cache.rows[0].text.starts_with("alpha"));
            assert!(fixture.cache.rows[1].text.starts_with("bravo"));
            assert!(fixture.cache.rows[2].text.starts_with("charlie"));
            fixture
        }

        fn wait_for_title(&self, title: &str) {
            loop {
                let metadata = self.session.metadata();
                if metadata.title == title {
                    // OSC events happen inside parsing. Taking the terminal
                    // lock ensures the chunk containing the marker is complete.
                    let _ = self.session.viewport();
                    return;
                }
                assert!(
                    matches!(metadata.status, SessionStatus::Running),
                    "PTY fixture stopped before {title}: {:?}",
                    metadata.status
                );
                assert!(
                    Instant::now() < self.deadline,
                    "PTY fixture did not reach {title} within ten seconds; title={:?}, metrics={:?}",
                    metadata.title,
                    self.session.metrics()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn output(&self, payload: &str, marker: &str) {
            // All inputs are test-owned printf formats, never external text.
            assert!(!payload.contains('\''));
            assert!(
                marker
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            );
            self.session
                .write(format!("printf '{payload}'; printf '\\033]0;{marker}\\007'\n").as_bytes())
                .expect("write fixture command");
            self.wait_for_title(marker);
        }

        fn frame(&mut self, search: &str) -> Vec<egui::epaint::ClippedShape> {
            let Self {
                ctx,
                config,
                cache,
                session,
                rect,
                ..
            } = self;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(704.0, 224.0))),
                    ..Default::default()
                },
                |ui| {
                    if let Some(size) = cache.geometry(ui, *rect, config) {
                        session
                            .resize(
                                size.columns,
                                size.lines,
                                size.pixel_width,
                                size.pixel_height,
                            )
                            .unwrap();
                    }
                    let snapshot = session.viewport();
                    cache.prepare(ui, &snapshot, config, Palette::for_config(config));
                    cache.paint(
                        ui,
                        *rect,
                        config,
                        Palette::for_config(config),
                        false,
                        search,
                        "",
                    );
                },
            );
            // Headless tests inspect shapes and never upload font textures.
            output.textures_delta.clear();
            std::mem::take(&mut output.shapes)
        }

        fn galleys(&self) -> Vec<Vec<Arc<egui::Galley>>> {
            self.cache
                .rows
                .iter()
                .map(|row| row.runs.iter().map(|run| run.galley.clone()).collect())
                .collect()
        }
    }

    fn assert_retained_galleys(before: &[Arc<egui::Galley>], after: &CachedRow) {
        assert_eq!(before.len(), after.runs.len());
        for (previous, run) in before.iter().zip(&after.runs) {
            assert!(
                Arc::ptr_eq(previous, &run.galley),
                "unchanged text was reshaped"
            );
        }
    }

    #[test]
    fn unchanged_frame_retains_shaped_rows_without_rebuilds() {
        let mut fixture = RendererFixture::new();
        let rebuilds = fixture.cache.render_rebuilds;
        let galleys = fixture.galleys();
        assert!(rebuilds > 0);
        assert!(!galleys[0].is_empty());

        fixture.frame("");

        assert_eq!(fixture.cache.render_rebuilds, rebuilds);
        for (before, after) in galleys.iter().zip(&fixture.cache.rows) {
            assert_retained_galleys(before, after);
        }
    }

    #[test]
    fn display_scale_change_rebuilds_galleys_then_returns_to_cache_hits() {
        let mut fixture = RendererFixture::new();
        let galleys = fixture.galleys();
        let rebuilds = fixture.cache.render_rebuilds;
        let dimensions = (fixture.cache.columns, fixture.cache.lines);

        fixture.ctx.set_pixels_per_point(2.0);
        fixture.frame("");

        assert_eq!(fixture.ctx.pixels_per_point(), 2.0);
        // Keep the same grid: invalidation must come from display scale rather
        // than a resize or changed terminal cells.
        assert_eq!((fixture.cache.columns, fixture.cache.lines), dimensions);
        assert!(fixture.cache.render_rebuilds > rebuilds);
        for (before, after) in galleys.iter().take(3).zip(&fixture.cache.rows) {
            assert!(!before.is_empty());
            assert!(!Arc::ptr_eq(&before[0], &after.runs[0].galley));
        }

        let rebuilt_galleys = fixture.galleys();
        let rebuilt_count = fixture.cache.render_rebuilds;
        fixture.frame("");

        assert_eq!(fixture.cache.render_rebuilds, rebuilt_count);
        for (before, after) in rebuilt_galleys.iter().zip(&fixture.cache.rows) {
            assert_retained_galleys(before, after);
        }
    }

    #[test]
    fn revision_without_cell_changes_preserves_all_row_layouts() {
        let mut fixture = RendererFixture::new();
        let rebuilds = fixture.cache.render_rebuilds;
        let galleys = fixture.galleys();
        let revision = fixture.session.revision();

        // The real session notifies renderers, but a zero scroll changes no cells.
        fixture.session.scroll(0);
        assert!(fixture.session.revision() > revision);
        fixture.frame("");

        assert_eq!(fixture.cache.revision, fixture.session.revision());
        assert_eq!(fixture.cache.render_rebuilds, rebuilds);
        for (before, after) in galleys.iter().zip(&fixture.cache.rows) {
            assert_retained_galleys(before, after);
        }
    }

    #[test]
    fn pty_output_rebuilds_only_the_changed_row() {
        let mut fixture = RendererFixture::new();
        let rebuilds = fixture.cache.render_rebuilds;
        let galleys = fixture.galleys();
        let hashes: Vec<_> = fixture.cache.rows.iter().map(|row| row.hash).collect();

        fixture.output("\\033[2;1Hchanged\\033[K\\033[1;1H", "changed");
        fixture.frame("");

        assert_eq!(fixture.cache.render_rebuilds, rebuilds + 1);
        assert!(fixture.cache.rows[1].text.starts_with("changed"));
        assert_ne!(fixture.cache.rows[1].hash, hashes[1]);
        assert!(!Arc::ptr_eq(
            &galleys[1][0],
            &fixture.cache.rows[1].runs[0].galley
        ));
        for (index, before) in galleys.iter().enumerate() {
            if index != 1 {
                assert_eq!(fixture.cache.rows[index].hash, hashes[index]);
                assert_retained_galleys(before, &fixture.cache.rows[index]);
            }
        }
    }

    #[test]
    fn unicode_search_highlights_terminal_columns_not_utf8_offsets() {
        let mut fixture = RendererFixture::new();
        fixture.output("\\033[2J\\033[HA界e\u{301}B\\033[1;1H", "unicode");
        fixture.frame("");
        let row = &fixture.cache.rows[0];
        assert!(row.text.starts_with("A界e\u{301}B"));
        // Wide-character spacers consume a terminal column but no UTF-8 bytes;
        // the combining accent belongs to its preceding cell.
        assert_eq!(&row.text_columns[..4], &[(0, 0), (1, 1), (4, 3), (7, 4)]);

        let rebuilds = fixture.cache.render_rebuilds;
        for (search, column, width) in [("界", 1.0, 2.0), ("e\u{301}", 3.0, 1.0)] {
            let shapes = fixture.frame(search);
            let fill = Palette::for_config(&fixture.config)
                .accent
                .gamma_multiply(0.25);
            let highlights: Vec<_> = shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Rect(rect) if rect.fill == fill => Some(rect.rect),
                    _ => None,
                })
                .collect();
            assert_eq!(fixture.cache.matches(search), 1);
            assert_eq!(
                highlights,
                vec![Rect::from_min_size(
                    fixture.rect.min + Vec2::new(column * fixture.cache.cell.x, 0.0),
                    Vec2::new(width * fixture.cache.cell.x, fixture.cache.cell.y),
                )],
                "incorrect search bounds for {search:?}"
            );
        }
        assert_eq!(
            fixture.cache.render_rebuilds, rebuilds,
            "search reshaped terminal text"
        );
    }
}
