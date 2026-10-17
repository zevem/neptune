//! Private Alacritty adapter; snapshots copy only damaged rows and share all others.
use super::*;
use crate::view;
use alacritty_terminal::term::TermDamage;
use std::collections::HashMap;

pub(super) fn selection_type(kind: view::SelectionType) -> SelectionType {
    match kind {
        view::SelectionType::Simple => SelectionType::Simple,
        view::SelectionType::Block => SelectionType::Block,
        view::SelectionType::Semantic => SelectionType::Semantic,
        view::SelectionType::Lines => SelectionType::Lines,
    }
}
fn point(point: Point) -> view::Point {
    view::Point::new(point.line.0, point.column.0)
}
fn color(color: alacritty_terminal::vte::ansi::Color) -> view::Color {
    use alacritty_terminal::vte::ansi::{Color, NamedColor};
    match color {
        Color::Spec(rgb) => view::Color::Spec(view::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }),
        Color::Indexed(index) => view::Color::Indexed(index),
        Color::Named(named) => view::Color::Named(match named {
            NamedColor::Black => view::NamedColor::Black,
            NamedColor::Red => view::NamedColor::Red,
            NamedColor::Green => view::NamedColor::Green,
            NamedColor::Yellow => view::NamedColor::Yellow,
            NamedColor::Blue => view::NamedColor::Blue,
            NamedColor::Magenta => view::NamedColor::Magenta,
            NamedColor::Cyan => view::NamedColor::Cyan,
            NamedColor::White => view::NamedColor::White,
            NamedColor::BrightBlack => view::NamedColor::BrightBlack,
            NamedColor::BrightRed => view::NamedColor::BrightRed,
            NamedColor::BrightGreen => view::NamedColor::BrightGreen,
            NamedColor::BrightYellow => view::NamedColor::BrightYellow,
            NamedColor::BrightBlue => view::NamedColor::BrightBlue,
            NamedColor::BrightMagenta => view::NamedColor::BrightMagenta,
            NamedColor::BrightCyan => view::NamedColor::BrightCyan,
            NamedColor::BrightWhite => view::NamedColor::BrightWhite,
            NamedColor::Foreground => view::NamedColor::Foreground,
            NamedColor::Background => view::NamedColor::Background,
            NamedColor::Cursor => view::NamedColor::Cursor,
            NamedColor::DimBlack => view::NamedColor::DimBlack,
            NamedColor::DimRed => view::NamedColor::DimRed,
            NamedColor::DimGreen => view::NamedColor::DimGreen,
            NamedColor::DimYellow => view::NamedColor::DimYellow,
            NamedColor::DimBlue => view::NamedColor::DimBlue,
            NamedColor::DimMagenta => view::NamedColor::DimMagenta,
            NamedColor::DimCyan => view::NamedColor::DimCyan,
            NamedColor::DimWhite => view::NamedColor::DimWhite,
            NamedColor::BrightForeground => view::NamedColor::BrightForeground,
            NamedColor::DimForeground => view::NamedColor::DimForeground,
        }),
    }
}
fn cursor_shape(shape: alacritty_terminal::vte::ansi::CursorShape) -> view::CursorShape {
    use alacritty_terminal::vte::ansi::CursorShape;
    match shape {
        CursorShape::Block => view::CursorShape::Block,
        CursorShape::Underline => view::CursorShape::Underline,
        CursorShape::Beam => view::CursorShape::Beam,
        CursorShape::HollowBlock => view::CursorShape::HollowBlock,
        CursorShape::Hidden => view::CursorShape::Hidden,
    }
}
impl TerminalSession {
    /// Returns immutable rows and a revision captured while holding the grid lock.
    /// Damage resets here, so the runtime owns viewport extraction and renderers
    /// retain the shared rows. A new renderer can consume a snapshot at any time.
    pub fn viewport(&self) -> view::ViewportSnapshot {
        let started = Instant::now();
        let mut terminal = self.terminal.lock();
        self.shared
            .snapshot_lock_nanoseconds
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let revision = self.revision();
        let mut cached = self.viewport_cache.lock();
        if let Some(snapshot) = cached
            .as_ref()
            .filter(|snapshot| snapshot.revision == revision)
        {
            let mut snapshot = snapshot.clone();
            snapshot.damage = view::ViewportDamage::Rows(Vec::new());
            return snapshot;
        }
        let columns = terminal.columns();
        let screen_lines = terminal.screen_lines();
        let display_offset = terminal.grid().display_offset();
        let colors = std::array::from_fn(|index| {
            terminal.colors()[index].map(|rgb| view::Rgb {
                r: rgb.r,
                g: rgb.g,
                b: rgb.b,
            })
        });
        let full = cached.as_ref().is_none_or(|snapshot| {
            snapshot.columns != columns
                || snapshot.screen_lines != screen_lines
                || snapshot.display_offset != display_offset
                || snapshot.colors != colors
        });
        let damage = if full {
            view::ViewportDamage::Full
        } else {
            match terminal.damage() {
                TermDamage::Full => view::ViewportDamage::Full,
                TermDamage::Partial(rows) => {
                    view::ViewportDamage::Rows(rows.map(|row| row.line).collect())
                }
            }
        };
        let mut rows = cached
            .as_ref()
            .map(|snapshot| snapshot.rows.clone())
            .unwrap_or_default();
        rows.resize_with(screen_lines, || Arc::from([]));
        let changed: Vec<usize> = match &damage {
            view::ViewportDamage::Full => (0..screen_lines).collect(),
            view::ViewportDamage::Rows(rows) => rows.clone(),
        };
        let mut hyperlinks = HashMap::new();
        for &row in &changed {
            if row >= screen_lines {
                continue;
            }
            let line = Line(row as i32 - display_offset as i32);
            rows[row] = (0..columns)
                .map(|column| {
                    let cell = &terminal.grid()[Point::new(line, Column(column))];
                    view::Cell {
                        column,
                        c: cell.c,
                        extra: cell.zerowidth().unwrap_or_default().to_vec(),
                        fg: color(cell.fg),
                        bg: color(cell.bg),
                        flags: view::Flags::from_bits_retain(cell.flags.bits()),
                        hyperlink: cell.hyperlink().map(|link| {
                            // The grid holds each target alive for this extraction.
                            // Key by allocation so long URIs aren't hashed per cell.
                            hyperlinks
                                .entry(link.uri().as_ptr())
                                .or_insert_with(|| Arc::<str>::from(link.uri()))
                                .clone()
                        }),
                    }
                })
                .collect();
        }
        let content = terminal.renderable_content();
        let snapshot = view::ViewportSnapshot {
            revision,
            columns,
            screen_lines,
            display_offset,
            rows,
            colors,
            damage,
            cursor: view::Cursor {
                point: point(content.cursor.point),
                shape: cursor_shape(content.cursor.shape),
            },
            mode: view::Mode::from_bits_retain(content.mode.bits()),
            selection: content.selection.map(|range| view::SelectionRange {
                start: point(range.start),
                end: point(range.end),
                is_block: range.is_block,
            }),
        };
        terminal.reset_damage();
        *cached = Some(snapshot.clone());
        self.shared
            .snapshot_nanoseconds
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.shared
            .snapshot_rows
            .fetch_add(changed.len() as u64, Ordering::Relaxed);
        self.shared
            .snapshot_cells
            .fetch_add((changed.len() * columns) as u64, Ordering::Relaxed);
        self.shared.snapshot_count.fetch_add(1, Ordering::Relaxed);
        snapshot
    }
    pub fn modes(&self) -> view::Mode {
        view::Mode::from_bits_retain(self.terminal.lock().mode().bits())
    }
    pub fn history_size(&self) -> usize {
        self.terminal.lock().history_size()
    }
    /// Visible screen text, retaining Unicode and wrapped-line semantics.
    pub fn screen_text(&self) -> String {
        self.viewport().text()
    }
    /// Explicit diagnostic capture; callers should avoid full history on frames.
    pub fn history_text(&self) -> String {
        let terminal = self.terminal.lock();
        terminal.bounds_to_string(
            Point::new(terminal.topmost_line(), Column(0)),
            Point::new(terminal.bottommost_line(), terminal.last_column()),
        )
    }
    pub fn clear_history(&self) {
        let mut terminal = self.terminal.lock();
        terminal.grid_mut().clear_history();
        self.shared.changed();
    }
    pub fn select_all(&self) {
        let mut terminal = self.terminal.lock();
        let start = Point::new(terminal.topmost_line(), Column(0));
        let end = Point::new(terminal.bottommost_line(), terminal.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        terminal.selection = Some(selection);
        self.shared.changed();
    }
    pub fn set_selection(&self, start: view::Point, end: view::Point) {
        let mut terminal = self.terminal.lock();
        let clamp = |point: view::Point| {
            Point::new(
                Line(
                    point
                        .line
                        .clamp(terminal.topmost_line().0, terminal.bottommost_line().0),
                ),
                Column(point.column.min(terminal.columns() - 1)),
            )
        };
        let start = clamp(start);
        let end = clamp(end);
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        terminal.selection = Some(selection);
        self.shared.changed();
    }
    pub fn scroll_to_point(&self, point: view::Point) {
        let mut terminal = self.terminal.lock();
        let point = Point::new(
            Line(
                point
                    .line
                    .clamp(terminal.topmost_line().0, terminal.bottommost_line().0),
            ),
            Column(point.column.min(terminal.columns() - 1)),
        );
        terminal.scroll_to_point(point);
        self.shared.changed();
    }
    /// Update focus and send a report only when requested by the application.
    pub fn focus(&self, focused: bool) -> std::result::Result<(), crate::SessionError> {
        let reporting = {
            let mut terminal = self.terminal.lock();
            terminal.is_focused = focused;
            let reporting = terminal.mode().contains(TermMode::FOCUS_IN_OUT);
            self.shared.changed();
            reporting
        };
        if reporting {
            self.write(if focused { b"\x1b[I" } else { b"\x1b[O" })?;
        }
        Ok(())
    }
}

impl TerminalSession {
    pub(crate) fn search_bounds(&self) -> (u64, i32, i32, usize) {
        let terminal = self.terminal.lock();
        (
            self.revision(),
            terminal.topmost_line().0,
            terminal.bottommost_line().0,
            terminal.columns(),
        )
    }
    pub(crate) fn search_row(
        &self,
        line: i32,
        revision: u64,
    ) -> Option<(Vec<view::Cell>, bool, bool)> {
        let terminal = self.terminal.lock();
        if self.revision() != revision
            || line < terminal.topmost_line().0
            || line > terminal.bottommost_line().0
        {
            return None;
        }
        let cells = (0..terminal.columns())
            .map(|column| {
                let cell = &terminal.grid()[Point::new(Line(line), Column(column))];
                view::Cell {
                    column,
                    c: cell.c,
                    extra: cell.zerowidth().unwrap_or_default().to_vec(),
                    fg: color(cell.fg),
                    bg: color(cell.bg),
                    flags: view::Flags::from_bits_retain(cell.flags.bits()),
                    // Search consumes characters and columns, not link targets.
                    hyperlink: None,
                }
            })
            .collect();
        let wrapped = terminal.grid()[Point::new(Line(line), terminal.last_column())]
            .flags
            .contains(alacritty_terminal::term::cell::Flags::WRAPLINE);
        let previous = line > terminal.topmost_line().0
            && terminal.grid()[Point::new(Line(line - 1), terminal.last_column())]
                .flags
                .contains(alacritty_terminal::term::cell::Flags::WRAPLINE);
        Some((cells, wrapped, previous))
    }
}

impl TerminalSession {
    pub fn start_selection_at(&self, point: view::Point, kind: view::SelectionType) {
        let mut terminal = self.terminal.lock();
        let point = Point::new(
            Line(
                point
                    .line
                    .clamp(terminal.topmost_line().0, terminal.bottommost_line().0),
            ),
            Column(point.column.min(terminal.columns() - 1)),
        );
        let kind = selection_type(kind);
        let mut selection = Selection::new(kind, point, Side::Left);
        if matches!(kind, SelectionType::Semantic | SelectionType::Lines) {
            selection.include_all();
        }
        terminal.selection = Some(selection);
        self.shared.changed();
    }
    pub fn update_selection_at(&self, point: view::Point) {
        let mut terminal = self.terminal.lock();
        let point = Point::new(
            Line(
                point
                    .line
                    .clamp(terminal.topmost_line().0, terminal.bottommost_line().0),
            ),
            Column(point.column.min(terminal.columns() - 1)),
        );
        if let Some(selection) = &mut terminal.selection {
            selection.update(point, Side::Right);
            // These coordinates name whole cells, even when the drag reverses.
            selection.include_all();
        }
        self.shared.changed();
    }
}

impl TerminalSession {
    /// Advance a completed search endpoint by one grid cell, wrapping history.
    /// `begin_search` includes its origin; advancing avoids repeatedly finding
    /// a one-cell match at the previous endpoint.
    pub fn next_search_point(&self, point: view::Point, direction: view::Direction) -> view::Point {
        let terminal = self.terminal.lock();
        let line = point
            .line
            .clamp(terminal.topmost_line().0, terminal.bottommost_line().0);
        let column = point.column.min(terminal.columns() - 1);
        match direction {
            view::Direction::Right if column + 1 < terminal.columns() => {
                view::Point::new(line, column + 1)
            }
            view::Direction::Right => view::Point::new(
                if line < terminal.bottommost_line().0 {
                    line + 1
                } else {
                    terminal.topmost_line().0
                },
                0,
            ),
            view::Direction::Left if column > 0 => view::Point::new(line, column - 1),
            view::Direction::Left => view::Point::new(
                if line > terminal.topmost_line().0 {
                    line - 1
                } else {
                    terminal.bottommost_line().0
                },
                terminal.columns() - 1,
            ),
        }
    }
}
