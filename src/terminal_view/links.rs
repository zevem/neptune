//! Link hit testing uses only the visible, owned cells and terminal columns.

use super::Cache;
use crate::platform::{
    editor::FileLocation,
    links::{MAX_URL_BYTES, WebLink},
};
use eframe::egui::{self, Rect};
use terminal_core::{Cell, Flags, Point};

/// Cells of one logical line read on each side of the pointer.
const MAX_PATH_CELLS: usize = 1024;
const MAX_PATH_BYTES: usize = 4096;
const MAX_IMAGE_PATHS: usize = 8;
const IMAGE_EXTENSIONS: [&str; 6] = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"];

/// Text, its UTF-8-byte/terminal-cell positions, and its final visible cell.
pub(super) type Token = (String, Vec<(usize, usize)>, usize);

/// Text under the pointer that may name a picture. Whether it does is for
/// the reader of the file to say; the grid only knows how it is spelled.
#[derive(Clone, Debug, PartialEq)]
pub struct ImagePath {
    pub text: String,
    /// The cells it occupies, one rectangle for each row.
    pub rows: Vec<Rect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Web(WebLink),
    File(FileLocation),
}

impl LinkTarget {
    #[cfg(test)]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Web(url) => url.as_str(),
            Self::File(file) => &file.path,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Link {
    pub target: LinkTarget,
    /// Inclusive linear cell indices in the visible viewport.
    pub start: usize,
    pub end: usize,
}

impl Cache {
    pub(super) fn link_interaction(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
    ) -> (Option<Link>, Option<LinkTarget>) {
        if self.hints.is_some() {
            self.pressed_link = None;
            self.link_pointer_owned = true;
            return (None, None);
        }
        self.link_pointer_owned = self.pressed_link.is_some();
        let hit =
            |cache: &Self, pos: egui::Pos2| cache.link_at_inner(cache.grid_point(rect, pos)?, true);
        let mut open = None;
        let contains_pointer = response.contains_pointer();
        let clicked = response.clicked_by(egui::PointerButton::Primary);
        ui.input(|input| {
            for event in &input.events {
                let egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                } = event
                else {
                    continue;
                };
                if *pressed && contains_pointer && link_modifier(*modifiers) {
                    self.pressed_link = hit(self, *pos);
                    self.link_pointer_owned |= self.pressed_link.is_some();
                } else if !pressed && let Some(pressed) = self.pressed_link.take() {
                    self.link_pointer_owned = true;
                    if clicked
                        && link_modifier(*modifiers)
                        && hit(self, *pos).as_ref() == Some(&pressed)
                    {
                        open = Some(pressed.target);
                    }
                }
            }
        });
        if !ui.input(|input| input.focused && input.pointer.primary_down()) {
            self.pressed_link = None;
        }
        let hovered =
            if response.contains_pointer() && ui.input(|input| link_modifier(input.modifiers)) {
                ui.input(|input| input.pointer.hover_pos())
                    .and_then(|pos| self.link_at(self.grid_point(rect, pos)?))
            } else {
                None
            };
        (hovered, open)
    }

    /// The grid point under `pos`, when it is over a cell of the grid.
    fn grid_point(&self, rect: Rect, pos: egui::Pos2) -> Option<Point> {
        if !rect.contains(pos)
            || pos.x >= rect.left() + f32::from(self.columns) * self.cell.x
            || pos.y >= rect.top() + f32::from(self.lines) * self.cell.y
        {
            return None;
        }
        Some(super::geometry::point_at(
            rect,
            self.cell,
            self.columns,
            self.lines,
            self.display_offset,
            pos,
        ))
    }

    /// Linear index of the visible cell at `point`; a wide character's
    /// spacer stands for the character.
    fn cell_index(&self, point: Point) -> Option<usize> {
        let row = usize::try_from(point.line + self.display_offset as i32).ok()?;
        let columns = usize::from(self.columns);
        if row >= self.sources.len() || point.column >= columns {
            return None;
        }
        let index = row * columns + point.column;
        if self
            .source_cell(index)?
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
        {
            return index.checked_sub(1);
        }
        Some(index)
    }

    /// The spellings of a picture's path the resting pointer may be on,
    /// likeliest first. A button held down is a selection, not a look.
    pub(super) fn image_path_hover(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
    ) -> Vec<ImagePath> {
        if !response.contains_pointer() || ui.input(|input| input.pointer.any_down()) {
            return Vec::new();
        }
        let Some(point) = ui
            .input(|input| input.pointer.hover_pos())
            .and_then(|pos| self.grid_point(rect, pos))
        else {
            return Vec::new();
        };
        let columns = usize::from(self.columns);
        self.image_paths_at(point)
            .into_iter()
            .map(|(text, start, end)| ImagePath {
                text,
                rows: (start / columns..=end / columns)
                    .map(|row| {
                        let from = if row == start / columns {
                            start % columns
                        } else {
                            0
                        };
                        let to = if row == end / columns {
                            end % columns + 1
                        } else {
                            columns
                        };
                        Rect::from_min_max(
                            rect.min + egui::vec2(from as f32, row as f32) * self.cell,
                            rect.min + egui::vec2(to as f32, (row + 1) as f32) * self.cell,
                        )
                    })
                    .collect(),
            })
            .collect()
    }

    /// Candidate picture paths at `point` with their inclusive cell spans.
    /// A path may hold spaces, so each candidate ends at the nearest picture
    /// extension and starts at a word further back along the logical line.
    pub(super) fn image_paths_at(&self, point: Point) -> Vec<(String, usize, usize)> {
        let mut paths = Vec::new();
        let Some(index) = self.cell_index(point) else {
            return paths;
        };
        let Some(cell) = self.source_cell(index) else {
            return paths;
        };
        if blank(cell) {
            return paths;
        }
        if let Some(target) = &cell.hyperlink
            && target
                .get(..7)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
            && target.len() <= MAX_PATH_BYTES
            && image_end(target, target.len() - 1) == Some(target.len())
        {
            let same_link = |index| {
                self.source_cell(index).is_some_and(|cell| {
                    !cell.flags.contains(Flags::HIDDEN)
                        && cell.hyperlink.as_deref() == Some(target.as_ref())
                })
            };
            let (mut start, mut end) = (index, index);
            while start > 0 && index - start < MAX_PATH_CELLS && same_link(start - 1) {
                start -= 1;
            }
            while end - index < MAX_PATH_CELLS && same_link(end + 1) {
                end += 1;
            }
            paths.push((target.to_string(), start, end));
        }

        let (mut start, mut end) = (index, index);
        while start > 0 && index - start < MAX_PATH_CELLS && self.connected(start - 1, start) {
            start -= 1;
        }
        while end - index < MAX_PATH_CELLS
            && self.connected(end, end + 1)
            && self.source_cell(end + 1).is_some()
        {
            end += 1;
        }
        let mut text = String::new();
        let mut positions = Vec::new();
        for index in start..=end {
            let Some(cell) = self.source_cell(index) else {
                return paths;
            };
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            positions.push((text.len(), index));
            if blank(cell) {
                text.push(' ');
            } else {
                text.push(cell.c);
                text.extend(&cell.extra);
            }
        }
        let Some(&(at, _)) = positions.iter().find(|(_, cell)| *cell == index) else {
            return paths;
        };
        let Some(limit) = image_end(&text, at) else {
            return paths;
        };
        let Some(end) = positions
            .iter()
            .rev()
            .find(|(byte, _)| *byte < limit)
            .map(|&(_, cell)| {
                let wide = self
                    .source_cell(cell)
                    .is_some_and(|cell| cell.flags.contains(Flags::WIDE_CHAR));
                cell + usize::from(wide)
            })
        else {
            return paths;
        };
        for &(offset, cell) in positions.iter().rev() {
            if offset > at {
                continue;
            }
            if limit - offset > MAX_PATH_BYTES || paths.len() == MAX_IMAGE_PATHS {
                break;
            }
            let c = text[offset..].chars().next().unwrap_or(' ');
            let opens = text[..offset].chars().next_back().is_none_or(path_boundary);
            if opens && !path_boundary(c) {
                paths.push((text[offset..limit].to_owned(), cell, end));
            }
        }
        paths
    }

    pub(super) fn link_at(&self, point: Point) -> Option<Link> {
        self.link_at_inner(point, false)
    }

    fn link_at_inner(&self, point: Point, allow_bare: bool) -> Option<Link> {
        let index = self.cell_index(point)?;
        let cell = self.source_cell(index)?;
        if cell
            .flags
            .intersects(Flags::HIDDEN | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            return None;
        }
        if let Some(target) = &cell.hyperlink {
            let target = WebLink::new(target).map(LinkTarget::Web).or_else(|| {
                target
                    .get(..7)
                    .filter(|scheme| scheme.eq_ignore_ascii_case("file://"))
                    .and_then(|_| FileLocation::parse(target))
                    .map(LinkTarget::File)
            })?;
            let spelling = cell.hyperlink.as_deref();
            let same_link = |index| {
                self.source_cell(index).is_some_and(|cell| {
                    !cell.flags.contains(Flags::HIDDEN) && cell.hyperlink.as_deref() == spelling
                })
            };
            let mut start = index;
            let mut end = index;
            while start > 0 && index - start < MAX_URL_BYTES && same_link(start - 1) {
                start -= 1;
            }
            while end - start < MAX_URL_BYTES && same_link(end + 1) {
                end += 1;
            }
            return Some(Link { target, start, end });
        }

        let quoted = self.quoted_token_at(index);
        let is_quoted = quoted.is_some();
        let (text, positions, _) = quoted.or_else(|| self.token_at(index))?;
        let byte = positions.iter().find(|(_, cell)| *cell == index)?.0;
        for (offset, _) in text.char_indices() {
            let candidate = &text[offset..];
            if !candidate
                .get(..7)
                .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
                && !candidate
                    .get(..8)
                    .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
            {
                continue;
            }
            if text[..offset]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
            {
                continue;
            }
            let candidate = trim_url(candidate);
            let limit = offset + candidate.len();
            if byte < offset || byte >= limit {
                continue;
            }
            let url = WebLink::new(candidate)?;
            let start = positions.iter().find(|(byte, _)| *byte == offset)?.1;
            let end = positions.iter().rev().find(|(byte, _)| *byte < limit)?.1;
            let end = end + usize::from(self.source_cell(end)?.flags.contains(Flags::WIDE_CHAR));
            return Some(Link {
                target: LinkTarget::Web(url),
                start,
                end,
            });
        }
        // Strip prose punctuation and balanced enclosing delimiters. Quoted
        // spellings may contain spaces; token_at retains those quotes.
        let (offset, candidate) = if is_quoted {
            (0, text.as_str())
        } else {
            plain_candidate(&text)
        };
        let file = FileLocation::parse(candidate)
            .or_else(|| allow_bare.then(|| FileLocation::path(candidate)).flatten())?;
        if byte < offset || byte >= offset + candidate.len() {
            return None;
        }
        let start = positions.iter().find(|(byte, _)| *byte == offset)?.1;
        let end = positions
            .iter()
            .rev()
            .find(|(byte, _)| *byte < offset + candidate.len())?
            .1;
        Some(Link {
            target: LinkTarget::File(file),
            start,
            end: end + usize::from(self.source_cell(end)?.flags.contains(Flags::WIDE_CHAR)),
        })
    }

    pub(super) fn token_at(&self, index: usize) -> Option<Token> {
        let columns = usize::from(self.columns);
        let cell = self.source_cell(index)?;
        if matches!(cell.c, '"' | '\'' | '`') {
            return self.quoted_token_at(index);
        }
        if delimiter(cell) {
            return None;
        }
        let mut start = index;
        while start > 0
            && index - start < MAX_URL_BYTES
            && self.connected(start - 1, start)
            && self
                .source_cell(start - 1)
                .is_some_and(|cell| !delimiter(cell))
        {
            start -= 1;
        }
        if index - start == MAX_URL_BYTES {
            return None;
        }
        let mut end = index;
        while end - start < MAX_URL_BYTES
            && self.connected(end, end + 1)
            && self
                .source_cell(end + 1)
                .is_some_and(|cell| !delimiter(cell))
        {
            end += 1;
        }
        if end - start == MAX_URL_BYTES {
            return None;
        }
        if (end + 1).is_multiple_of(columns)
            && self.source_cell(end)?.flags.contains(Flags::WRAPLINE)
            && self.source_cell(end + 1).is_none()
        {
            return None;
        }
        // A hard break in the middle of nonblank text is not a complete path
        // boundary. In particular, never turn the suffix of a clipped URL or
        // filename into a new relative file link.
        if start.is_multiple_of(columns)
            && start > 0
            && !self.connected(start - 1, start)
            && self
                .source_cell(start - 1)
                .is_some_and(|cell| !delimiter(cell))
            || (end + 1).is_multiple_of(columns)
                && !self.connected(end, end + 1)
                && self
                    .source_cell(end + 1)
                    .is_some_and(|cell| !delimiter(cell))
        {
            return None;
        }
        let mut text = String::new();
        let mut positions = Vec::new();
        for index in start..=end {
            let cell = self.source_cell(index)?;
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            positions.push((text.len(), index));
            text.push(cell.c);
            text.extend(&cell.extra);
            if text.len() > MAX_URL_BYTES {
                return None;
            }
        }
        Some((text, positions, end))
    }

    /// Quoted file spellings can contain spaces. Read at most one bounded
    /// logical line, then pair quotes so prose before/after a quote stays out.
    fn quoted_token_at(&self, index: usize) -> Option<Token> {
        let mut start = index;
        while start > 0 && index - start < MAX_PATH_CELLS && self.connected(start - 1, start) {
            start -= 1;
        }
        let mut at = start;
        let limit = start + MAX_PATH_CELLS * 2;
        while at < limit {
            let cell = self.source_cell(at)?;
            if cell.flags.contains(Flags::HIDDEN) {
                return None;
            }
            if matches!(cell.c, '"' | '\'' | '`') {
                let quote = cell.c;
                let opening = at;
                at += 1;
                let mut text = String::new();
                let mut positions = Vec::new();
                while at < limit && self.connected(at - 1, at) {
                    let cell = self.source_cell(at)?;
                    if cell.flags.contains(Flags::HIDDEN) {
                        return None;
                    }
                    if cell.c == quote {
                        if (opening..=at).contains(&index) && FileLocation::path(&text).is_some() {
                            return Some((text, positions, at));
                        }
                        break;
                    }
                    if !cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        positions.push((text.len(), at));
                        text.push(cell.c);
                        text.extend(&cell.extra);
                        if text.len() > MAX_PATH_BYTES {
                            break;
                        }
                    }
                    at += 1;
                }
            }
            if at >= index && self.source_cell(at).is_none() {
                return None;
            }
            if !self.connected(at, at + 1) {
                return None;
            }
            at += 1;
        }
        None
    }

    pub(super) fn source_cell(&self, index: usize) -> Option<&Cell> {
        let columns = usize::from(self.columns);
        if columns == 0 {
            return None;
        }
        self.sources.get(index / columns)?.get(index % columns)
    }

    pub(super) fn connected(&self, left: usize, right: usize) -> bool {
        let columns = usize::from(self.columns);
        columns > 0
            && (!right.is_multiple_of(columns)
                || self
                    .source_cell(left)
                    .is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE)))
    }
}

fn link_modifier(modifiers: egui::Modifiers) -> bool {
    (modifiers.ctrl || modifiers.mac_cmd) && !modifiers.shift && !modifiers.alt
}

pub(super) fn delimiter(cell: &Cell) -> bool {
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
    {
        return false;
    }
    cell.flags.contains(Flags::HIDDEN)
        || cell.c.is_whitespace()
        || cell.c.is_control()
        || matches!(cell.c, '<' | '>' | '"' | '\'' | '`' | '|')
}

/// A cell that separates words: empty, concealed or a control character.
fn blank(cell: &Cell) -> bool {
    cell.flags.contains(Flags::HIDDEN) || cell.c.is_whitespace() || cell.c.is_control()
}

/// Characters a path is written after: spaces, quotes, brackets and the
/// punctuation of `key=value`, `label: value` and lists.
fn path_boundary(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\''
                | '`'
                | '<'
                | '>'
                | '|'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '='
                | ','
                | ':'
                | ';'
        )
}

/// Where the nearest picture file name covering byte `at` ends.
fn image_end(text: &str, at: usize) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    IMAGE_EXTENSIONS
        .iter()
        .flat_map(|extension| {
            lower
                .match_indices(extension)
                .map(|(offset, _)| offset + extension.len())
        })
        .filter(|&end| {
            end > at
                && !text[end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
        .min()
}

pub(super) fn trim_url(mut url: &str) -> &str {
    let mut unmatched = [0_i32; 3];
    for c in url.chars() {
        match c {
            '(' => unmatched[0] -= 1,
            ')' => unmatched[0] += 1,
            '[' => unmatched[1] -= 1,
            ']' => unmatched[1] += 1,
            '{' => unmatched[2] -= 1,
            '}' => unmatched[2] += 1,
            _ => {}
        }
    }
    loop {
        let Some(last) = url.chars().next_back() else {
            return url;
        };
        let closing = match last {
            ')' => Some(0),
            ']' => Some(1),
            '}' => Some(2),
            _ => None,
        };
        if closing.is_some_and(|index| unmatched[index] > 0)
            || matches!(last, '.' | ',' | ';' | ':' | '!')
        {
            if let Some(index) = closing {
                unmatched[index] -= 1;
            }
            url = &url[..url.len() - last.len_utf8()];
        } else {
            return url;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn cache(text: &str, columns: usize) -> Cache {
        let mut rows = Vec::new();
        let cells: Vec<_> = text
            .chars()
            .enumerate()
            .map(|(column, c)| Cell {
                column: column % columns,
                c,
                ..Default::default()
            })
            .collect();
        for cells in cells.chunks(columns) {
            let mut row = cells.to_vec();
            row.resize_with(columns, Cell::default);
            rows.push(Arc::from(row));
        }
        let mut cache = Cache::default();
        cache.columns = columns as u16;
        cache.lines = rows.len() as u16;
        cache.sources = rows;
        cache
    }

    #[test]
    fn plain_links_keep_queries_and_balanced_parentheses_and_exclude_prose() {
        let cache = cache("See (https://example.com/a_(b)?x=1&y=2#part). next", 80);
        assert_eq!(
            cache.link_at(Point::new(0, 12)).unwrap().target.as_str(),
            "https://example.com/a_(b)?x=1&y=2#part"
        );
        for column in [0, 4, 43, 44, 45, 48] {
            assert!(
                cache.link_at(Point::new(0, column)).is_none(),
                "column {column}"
            );
        }
    }

    #[test]
    fn soft_wrapped_links_and_scrollback_use_grid_columns() {
        let mut cache = cache("https://example.com/wrapped/path rest", 16);
        for row in &mut cache.sources[..2] {
            Arc::make_mut(row)[15].flags.insert(Flags::WRAPLINE);
        }
        cache.display_offset = 9;
        let link = cache.link_at(Point::new(-8, 5)).unwrap();
        assert_eq!(link.target.as_str(), "https://example.com/wrapped/path");
        assert_eq!((link.start, link.end), (0, 31));
        Arc::make_mut(&mut cache.sources[0])[15]
            .flags
            .remove(Flags::WRAPLINE);
        assert!(cache.link_at(Point::new(-8, 5)).is_none());
    }

    #[test]
    fn wide_and_combining_characters_preserve_hit_regions() {
        let mut cache = cache("https://example.com/界 e rest", 48);
        let row = Arc::make_mut(&mut cache.sources[0]);
        row[20].flags.insert(Flags::WIDE_CHAR);
        row[21].flags.insert(Flags::WIDE_CHAR_SPACER);
        row[22].extra.push('\u{301}');
        let link = cache.link_at(Point::new(0, 21)).unwrap();
        assert_eq!(link.target.as_str(), "https://example.com/界e\u{301}");
        assert_eq!(link.end, 22);
        assert!(cache.link_at(Point::new(0, 23)).is_none());
    }

    #[test]
    fn explicit_link_targets_override_their_label_and_hidden_cells_are_ignored() {
        let mut cache = cache("https://label.example/", 32);
        let row = Arc::make_mut(&mut cache.sources[0]);
        for cell in &mut row[..22] {
            cell.hyperlink = Some(Arc::from("https://target.example/"));
        }
        assert_eq!(
            cache.link_at(Point::new(0, 9)).unwrap().target.as_str(),
            "https://target.example/"
        );
        Arc::make_mut(&mut cache.sources[0])[9].hyperlink = Some(Arc::from("file:///tmp/file"));
        assert_eq!(
            cache.link_at(Point::new(0, 9)).unwrap().target.as_str(),
            "/tmp/file"
        );
        for target in [
            "javascript:alert(1)",
            "mailto:someone@example.com",
            "file:///tmp/%xx",
        ] {
            Arc::make_mut(&mut cache.sources[0])[9].hyperlink = Some(Arc::from(target));
            assert!(cache.link_at(Point::new(0, 9)).is_none(), "{target}");
        }
        Arc::make_mut(&mut cache.sources[0])[10]
            .flags
            .insert(Flags::HIDDEN);
        assert!(cache.link_at(Point::new(0, 10)).is_none());
    }

    #[test]
    fn hit_testing_bounds_long_tokens_and_does_not_join_hard_lines() {
        let long = cache(
            &format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES + 1)),
            10000,
        );
        assert!(long.link_at(Point::new(0, 20)).is_none());
        let cache = cache("https://example.com/path", 16);
        assert!(cache.link_at(Point::new(1, 2)).is_none());
    }

    fn image_paths(cache: &Cache, column: usize) -> Vec<String> {
        cache
            .image_paths_at(Point::new(0, column))
            .into_iter()
            .map(|(text, _, _)| text)
            .collect()
    }

    #[test]
    fn picture_paths_are_read_out_of_prose_markdown_and_assignments() {
        let cache = cache(
            "Saved ![shot](out/shot.PNG). See path=/tmp/a.png, b.pngx",
            80,
        );
        assert_eq!(
            image_paths(&cache, 16)[..2],
            ["out/shot.PNG", "shot](out/shot.PNG"]
        );
        assert_eq!(
            cache.image_paths_at(Point::new(0, 16))[0],
            ("out/shot.PNG".into(), 14, 25)
        );
        assert_eq!(image_paths(&cache, 40)[0], "/tmp/a.png");
        // Neither blank cells nor a longer extension name a picture.
        assert!(image_paths(&cache, 5).is_empty());
        assert!(image_paths(&cache, 50).is_empty());
    }

    #[test]
    fn picture_paths_with_spaces_offer_longer_readings_and_stop_at_hard_lines() {
        let cache = cache("at /tmp/My Shots/Screenshot from 1.png now", 80);
        assert_eq!(
            image_paths(&cache, 20),
            [
                "Shots/Screenshot from 1.png",
                "/tmp/My Shots/Screenshot from 1.png",
                "at /tmp/My Shots/Screenshot from 1.png"
            ]
        );
        assert!(image_paths(&cache, 40).is_empty());
        let mut wrapped = self::cache("/tmp/wrapped/shot.png", 12);
        assert!(wrapped.image_paths_at(Point::new(0, 3)).is_empty());
        Arc::make_mut(&mut wrapped.sources[0])[11]
            .flags
            .insert(Flags::WRAPLINE);
        assert_eq!(
            wrapped.image_paths_at(Point::new(1, 3)),
            [("/tmp/wrapped/shot.png".into(), 0, 20)]
        );
    }

    #[test]
    fn file_hyperlinks_name_pictures_whatever_their_label() {
        let mut cache = cache("the screenshot", 32);
        for cell in &mut Arc::make_mut(&mut cache.sources[0])[4..14] {
            cell.hyperlink = Some(Arc::from("file:///tmp/a%20b.png"));
        }
        assert_eq!(
            cache.image_paths_at(Point::new(0, 6)),
            [("file:///tmp/a%20b.png".into(), 4, 13)]
        );
        assert!(cache.image_paths_at(Point::new(0, 1)).is_empty());
    }

    #[test]
    fn a_resting_pointer_reports_the_cells_of_a_picture_path() {
        let mut gesture = Gesture::new();
        gesture.cache = cache("see /tmp/a.png", 80);
        gesture.cache.cell = egui::vec2(8.0, 20.0);
        let (painted, _) = gesture.frame(Vec::new(), egui::Modifiers::NONE);
        // The path alone is the likeliest reading; the whole line is the other.
        assert_eq!(painted.image_paths.len(), 2);
        assert_eq!(painted.image_paths[0].text, "/tmp/a.png");
        assert_eq!(
            painted.image_paths[0].rows,
            [Rect::from_min_max(
                egui::pos2(42.0, 10.0),
                egui::pos2(122.0, 30.0)
            )]
        );
        let pos = egui::pos2(50.0, 20.0);
        let (pressed, _) = gesture.frame(
            vec![Gesture::button(pos, true, egui::Modifiers::NONE)],
            egui::Modifiers::NONE,
        );
        assert!(pressed.image_paths.is_empty());
    }

    struct Gesture {
        ctx: egui::Context,
        cache: Cache,
        time: f64,
    }

    impl Gesture {
        fn new() -> Self {
            let mut gesture = Self {
                ctx: egui::Context::default(),
                cache: cache("https://example.com/ rest", 80),
                time: 0.0,
            };
            gesture.cache.cell = egui::vec2(8.0, 20.0);
            gesture.frame(Vec::new(), egui::Modifiers::NONE);
            gesture.frame(
                vec![egui::Event::PointerMoved(egui::pos2(50.0, 20.0))],
                egui::Modifiers::NONE,
            );
            gesture
        }

        fn button(pos: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            }
        }

        fn frame(
            &mut self,
            mut events: Vec<egui::Event>,
            modifiers: egui::Modifiers,
        ) -> (super::super::PaintResult, egui::CursorIcon) {
            self.time += 0.1;
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            let mut painted = None;
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700.0, 120.0),
                    )),
                    time: Some(self.time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    painted = Some(self.cache.paint(
                        ui,
                        Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(640.0, 20.0)),
                        &crate::config::Config::default(),
                        crate::theme::Palette::new(crate::config::Theme::Graphite),
                        true,
                        "",
                        "",
                    ));
                },
            );
            output.textures_delta.clear();
            (painted.unwrap(), output.platform_output.cursor_icon)
        }
    }

    #[test]
    fn ctrl_and_command_click_open_once_and_own_the_tui_mouse_gesture() {
        for modifiers in [egui::Modifiers::CTRL, egui::Modifiers::MAC_CMD] {
            let mut gesture = Gesture::new();
            gesture.cache.mode = terminal_core::Mode::MOUSE_REPORT_CLICK;
            let (_, cursor) = gesture.frame(Vec::new(), modifiers);
            assert_eq!(cursor, egui::CursorIcon::PointingHand);
            let pos = egui::pos2(50.0, 20.0);
            let (press, _) = gesture.frame(vec![Gesture::button(pos, true, modifiers)], modifiers);
            assert!(press.open_link.is_none());
            assert!(press.interaction.is_none());
            assert!(gesture.cache.link_pointer_owned);
            let (release, _) =
                gesture.frame(vec![Gesture::button(pos, false, modifiers)], modifiers);
            assert_eq!(release.open_link.unwrap().as_str(), "https://example.com/");
            assert!(release.interaction.is_none());
            assert!(gesture.cache.link_pointer_owned);
            let (next, _) = gesture.frame(Vec::new(), modifiers);
            assert!(next.open_link.is_none());
            assert!(!gesture.cache.link_pointer_owned);
        }
    }

    #[test]
    fn ctrl_click_opens_bare_filenames_in_agent_output() {
        let mut gesture = Gesture::new();
        gesture.cache = cache("  └ Read AGENTS.md", 80);
        gesture.cache.cell = egui::vec2(8.0, 20.0);
        let modifiers = egui::Modifiers::CTRL;
        let pos = egui::pos2(94.0, 20.0);
        let (_, cursor) = gesture.frame(vec![egui::Event::PointerMoved(pos)], modifiers);
        assert_eq!(cursor, egui::CursorIcon::PointingHand);
        gesture.frame(vec![Gesture::button(pos, true, modifiers)], modifiers);
        let (release, _) = gesture.frame(vec![Gesture::button(pos, false, modifiers)], modifiers);
        assert_eq!(
            release.open_link,
            Some(LinkTarget::File(FileLocation {
                path: "AGENTS.md".into(),
                line: 1,
                column: 1,
            }))
        );
    }

    #[test]
    fn extensionless_clicks_are_checked_without_turning_prose_into_hover_links() {
        let mut gesture = Gesture::new();
        gesture.cache = cache("  └ Read launch-helper", 80);
        gesture.cache.cell = egui::vec2(8.0, 20.0);
        let modifiers = egui::Modifiers::CTRL;
        let pos = egui::pos2(94.0, 20.0);
        let (_, cursor) = gesture.frame(vec![egui::Event::PointerMoved(pos)], modifiers);
        assert_eq!(cursor, egui::CursorIcon::Text);
        gesture.frame(vec![Gesture::button(pos, true, modifiers)], modifiers);
        let (release, _) = gesture.frame(vec![Gesture::button(pos, false, modifiers)], modifiers);
        assert_eq!(release.open_link.unwrap().as_str(), "launch-helper");
        assert!(release.interaction.is_none());
    }

    #[test]
    fn ordinary_clicks_and_shift_selection_do_not_open_links() {
        for modifiers in [
            egui::Modifiers::NONE,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        ] {
            let mut gesture = Gesture::new();
            let pos = egui::pos2(50.0, 20.0);
            gesture.frame(vec![Gesture::button(pos, true, modifiers)], modifiers);
            let (release, cursor) =
                gesture.frame(vec![Gesture::button(pos, false, modifiers)], modifiers);
            assert!(release.open_link.is_none());
            assert_eq!(
                release.interaction,
                Some(super::super::SelectionInteraction::Clear)
            );
            assert_eq!(cursor, egui::CursorIcon::Text);
            assert!(!gesture.cache.link_pointer_owned);
        }
    }

    #[test]
    fn dragging_or_changing_the_target_cancels_link_activation() {
        let mut gesture = Gesture::new();
        let modifiers = egui::Modifiers::CTRL;
        let start = egui::pos2(50.0, 20.0);
        let end = egui::pos2(110.0, 20.0);
        gesture.frame(vec![Gesture::button(start, true, modifiers)], modifiers);
        let (drag, _) = gesture.frame(vec![egui::Event::PointerMoved(end)], modifiers);
        assert!(drag.interaction.is_none());
        let (release, _) = gesture.frame(vec![Gesture::button(end, false, modifiers)], modifiers);
        assert!(release.open_link.is_none());
        assert!(release.interaction.is_none());

        let mut gesture = Gesture::new();
        gesture.frame(vec![Gesture::button(start, true, modifiers)], modifiers);
        Arc::make_mut(&mut gesture.cache.sources[0])[8].c = 'z';
        let (release, _) = gesture.frame(vec![Gesture::button(start, false, modifiers)], modifiers);
        assert!(release.open_link.is_none());
    }

    #[test]
    fn clipped_soft_wraps_are_not_opened_as_truncated_urls() {
        let mut cache = cache("https://example.com/", 16);
        cache.sources.truncate(1);
        Arc::make_mut(&mut cache.sources[0])[15]
            .flags
            .insert(Flags::WRAPLINE);
        assert!(cache.link_at(Point::new(0, 5)).is_none());
    }
}

/// The token without compiler trailing colons and prose wrappers.
pub(super) fn plain_candidate(text: &str) -> (usize, &str) {
    let text = text.trim_end_matches(['.', ',', ';', ':', '!']);
    let start = text.len() - text.trim_start_matches(['(', '[', '{', '"', '\'']).len();
    let candidate = text[start..].trim_end_matches([')', ']', '}', '"', '\'']);
    (start, candidate)
}

#[cfg(test)]
mod file_location_tests {
    use super::*;
    use std::sync::Arc;

    fn cache(text: &str, columns: usize) -> Cache {
        let mut cache = Cache::default();
        cache.columns = columns as u16;
        cache.sources = text
            .chars()
            .collect::<Vec<_>>()
            .chunks(columns)
            .map(|chars| {
                let mut row: Vec<_> = chars
                    .iter()
                    .enumerate()
                    .map(|(column, c)| Cell {
                        column,
                        c: *c,
                        ..Default::default()
                    })
                    .collect();
                row.resize_with(columns, Cell::default);
                Arc::from(row)
            })
            .collect();
        cache.lines = cache.sources.len() as u16;
        cache
    }

    #[test]
    fn plain_paths_have_no_extension_allowlist_and_keep_their_hit_boundaries() {
        for path in [
            "AGENTS.md",
            "Cargo.toml",
            ".env",
            "README",
            "LICENSE",
            "Makefile",
            "Dockerfile",
            "foo.any-new-extension",
            "foo.c++",
            "file.未知",
            "src/file",
            "./script",
            "../other/file.txt",
            "~/config",
            "/tmp/file",
            "C:\\src\\file",
            "文件.md",
        ] {
            let text = format!("Read ({path}), done");
            let cache = cache(&text, 160);
            for column in 6..6 + path.chars().count() {
                let link = cache.link_at(Point::new(0, column)).expect(path);
                assert_eq!(
                    link.target,
                    LinkTarget::File(FileLocation::path(path).unwrap()),
                    "{path}"
                );
                assert_eq!(
                    (link.start, link.end),
                    (6, 5 + path.chars().count()),
                    "{path}"
                );
            }
            for column in [0, 5, 6 + path.chars().count(), 7 + path.chars().count()] {
                assert!(
                    cache.link_at(Point::new(0, column)).is_none(),
                    "{path}, col {column}"
                );
            }
        }
    }

    #[test]
    fn file_locations_exclude_diagnostic_punctuation_and_keep_terminal_columns() {
        let cache = cache("--> (src/foo.rs:42:7): next", 60);
        assert_eq!(
            cache.link_at(Point::new(0, 10)).unwrap().target,
            LinkTarget::File(FileLocation {
                path: "src/foo.rs".into(),
                line: 42,
                column: 7
            })
        );
        for col in [0, 4, 20, 21, 22, 23] {
            assert!(cache.link_at(Point::new(0, col)).is_none(), "col {col}");
        }
    }

    #[test]
    fn quoted_locations_with_spaces_and_unicode_stay_whole() {
        let cache = cache("at \"/tmp/My Sources/é.rs:42:3\" done", 80);
        for col in [5, 9, 15, 24] {
            assert_eq!(
                cache.link_at(Point::new(0, col)).unwrap().target,
                LinkTarget::File(FileLocation {
                    path: "/tmp/My Sources/é.rs".into(),
                    line: 42,
                    column: 3
                })
            );
        }
        assert!(cache.link_at(Point::new(0, 31)).is_none());
    }

    #[test]
    fn quoted_plain_paths_preserve_spaces_and_literal_punctuation() {
        for path in [
            "/tmp/My Sources/é.rs",
            "notes.md,",
            "file.md.",
            "[literal].txt",
        ] {
            let text = format!("at \"{path}\" done");
            let cache = cache(&text, 100);
            for column in 4..4 + path.chars().count() {
                assert_eq!(
                    cache.link_at(Point::new(0, column)).unwrap().target,
                    LinkTarget::File(FileLocation::path(path).unwrap()),
                    "{path}, col {column}"
                );
            }
        }
    }

    #[test]
    fn plain_file_links_cross_soft_wraps_and_include_wide_character_spacers() {
        let mut cache = cache("src/a_long_file.rs", 12);
        Arc::make_mut(&mut cache.sources[0])[11]
            .flags
            .insert(Flags::WRAPLINE);
        cache.display_offset = 6;
        let link = cache.link_at(Point::new(-5, 1)).unwrap();
        assert_eq!(link.target.as_str(), "src/a_long_file.rs");
        assert_eq!((link.start, link.end), (0, 17));

        let mut cache = self::cache("src/界 ", 20);
        let row = Arc::make_mut(&mut cache.sources[0]);
        row[4].flags.insert(Flags::WIDE_CHAR);
        row[5].flags.insert(Flags::WIDE_CHAR_SPACER);
        let link = cache.link_at(Point::new(0, 5)).unwrap();
        assert_eq!(link.target.as_str(), "src/界");
        assert_eq!((link.start, link.end), (0, 5));
    }

    #[test]
    fn file_locations_cross_soft_wraps_and_never_hard_lines() {
        let mut cache = cache("src/a_long_file.rs:42:9", 12);
        assert!(cache.link_at(Point::new(0, 7)).is_none());
        Arc::make_mut(&mut cache.sources[0])[11]
            .flags
            .insert(Flags::WRAPLINE);
        cache.display_offset = 6;
        assert_eq!(
            cache.link_at(Point::new(-5, 7)).unwrap().target,
            LinkTarget::File(FileLocation {
                path: "src/a_long_file.rs".into(),
                line: 42,
                column: 9
            })
        );
    }
}
