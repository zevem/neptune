//! Selection uses file coordinates, so scrolling does not discard its ends
//! and only the visible source lines need text layout.
use crate::{
    icons::Icon,
    platform::clipboard,
    theme::Palette,
    ui::helpers::{menu_item, menu_layout},
};
use eframe::egui::{self, Galley, Pos2, Rect, Response, Ui, text::CCursor, vec2};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Point {
    line: usize,
    column: usize,
}

#[derive(Default)]
pub struct Selection {
    file: Option<(PathBuf, u64)>,
    anchor: Point,
    cursor: Point,
    dragging: bool,
}

impl Selection {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn sync(&mut self, path: &Path, revision: u64) {
        if !self.matches(path, revision) {
            self.reset();
            self.file = Some((path.to_owned(), revision));
        }
    }

    fn matches(&self, path: &Path, revision: u64) -> bool {
        self.file
            .as_ref()
            .is_some_and(|(file, version)| file == path && *version == revision)
    }

    pub fn has_selection(&self, path: &Path, revision: u64) -> bool {
        self.matches(path, revision) && self.anchor != self.cursor
    }

    fn range(&self) -> (Point, Point) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }

    fn text(&self, lines: &[String]) -> String {
        let (start, end) = self.range();
        let mut text = String::new();
        for index in start.line..=end.line {
            let Some(line) = lines.get(index) else { break };
            if index > start.line {
                text.push('\n');
            }
            let from = if index == start.line { start.column } else { 0 };
            let to = if index == end.line {
                end.column
            } else {
                line.chars().count()
            };
            text.extend(line.chars().skip(from).take(to.saturating_sub(from)));
        }
        text
    }

    /// The content's response stays present even when either end is offscreen.
    pub fn update(
        &mut self,
        ui: &Ui,
        response: &Response,
        lines: &[String],
        origin: Pos2,
        height: f32,
        font: &egui::FontId,
    ) {
        if lines.is_empty() {
            return;
        }
        let pointer = ui.input(|input| input.pointer.clone());
        let dragging = self.dragging && (pointer.primary_down() || pointer.primary_released());
        if let Some(pos) = pointer.interact_pos()
            && (dragging || pointer.primary_pressed() && response.hovered())
        {
            let line =
                (((pos.y - origin.y) / height).floor().max(0.0) as usize).min(lines.len() - 1);
            let galley = ui.painter().layout_no_wrap(
                lines[line].clone(),
                font.clone(),
                egui::Color32::PLACEHOLDER,
            );
            let column = galley
                .cursor_from_pos(vec2(pos.x - origin.x, 0.0))
                .index
                .into();
            let point = Point { line, column };
            if pointer.primary_pressed() && response.hovered() {
                if !ui.input(|input| input.modifiers.shift) {
                    self.anchor = point;
                }
                self.cursor = point;
                self.dragging = true;
                ui.ctx()
                    .with_plugin(|labels: &mut egui::text_selection::LabelSelectionState| {
                        labels.clear_selection()
                    });
            } else if self.dragging {
                self.cursor = point;
                if pointer.primary_down() {
                    let clip = ui.clip_rect();
                    let delta = if pos.y < clip.top() {
                        8.0
                    } else if pos.y > clip.bottom() {
                        -8.0
                    } else {
                        0.0
                    };
                    if delta != 0.0 {
                        ui.scroll_with_delta(vec2(0.0, delta));
                        ui.ctx().request_repaint();
                    }
                }
            }
        }
        if !pointer.primary_down() {
            self.dragging = false;
        }
        if response.triple_clicked() {
            self.anchor = Point {
                column: 0,
                ..self.cursor
            };
            self.cursor.column = lines[self.cursor.line].chars().count();
        } else if response.double_clicked() {
            let chars: Vec<_> = lines[self.cursor.line].chars().collect();
            let word = |c: char| c.is_alphanumeric() || c == '_';
            let mut from = self.cursor.column.min(chars.len());
            let mut to = from;
            while from > 0 && word(chars[from - 1]) {
                from -= 1;
            }
            while to < chars.len() && word(chars[to]) {
                to += 1;
            }
            self.anchor = Point {
                column: from,
                ..self.cursor
            };
            self.cursor.column = to;
        }
        if self.anchor != self.cursor {
            let copy = ui.input_mut(|input| {
                let copy = input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Copy));
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Copy | egui::Event::Cut));
                copy
            });
            if copy {
                clipboard::copy(ui.ctx(), self.text(lines));
            }
        }
    }

    pub fn menu(&mut self, response: &Response, lines: &[String], p: Palette) {
        response.context_menu(|ui| {
            menu_layout(ui, 160.0);
            ui.add_enabled_ui(self.anchor != self.cursor, |ui| {
                if menu_item(ui, p, Icon::Copy, "Copy", "", false) {
                    clipboard::copy(ui.ctx(), self.text(lines));
                    ui.close();
                }
            });
            if menu_item(ui, p, Icon::TextSize, "Select all", "", false) {
                self.anchor = Point::default();
                self.cursor = Point {
                    line: lines.len().saturating_sub(1),
                    column: lines.last().map_or(0, |line| line.chars().count()),
                };
                ui.close();
            }
        });
    }

    pub fn paint(
        &self,
        ui: &Ui,
        index: usize,
        origin: Pos2,
        galley: &Galley,
        height: f32,
        p: Palette,
    ) {
        let (start, end) = self.range();
        if start == end || index < start.line || index > end.line {
            return;
        }
        let from = if index == start.line { start.column } else { 0 };
        let to = if index == end.line {
            end.column
        } else {
            galley.text().chars().count()
        };
        let x = |column| galley.pos_from_cursor(CCursor::new(column)).left();
        let extra = if index < end.line { 6.0 } else { 0.0 };
        ui.painter().rect_filled(
            Rect::from_min_max(
                origin + vec2(x(from), 0.0),
                origin + vec2(x(to) + extra, height),
            ),
            0,
            p.accent.gamma_multiply(0.3),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversed_selection_copies_unicode_across_lines_without_gutters() {
        let selection = Selection {
            anchor: Point { line: 2, column: 2 },
            cursor: Point { line: 0, column: 1 },
            ..Default::default()
        };
        assert_eq!(
            selection.text(&["héllo".into(), String::new(), "世界!".into()]),
            "éllo\n\n世界"
        );
    }

    #[test]
    fn a_new_file_or_revision_clears_selection() {
        let mut selection = Selection::default();
        let path = Path::new("/tmp/source.rs");
        selection.sync(path, 1);
        selection.cursor.column = 3;
        selection.sync(path, 1);
        assert!(selection.has_selection(path, 1));
        selection.sync(path, 2);
        assert!(!selection.has_selection(path, 2));
        selection.cursor.column = 3;
        selection.sync(Path::new("/tmp/other.rs"), 2);
        assert!(!selection.has_selection(path, 2));
    }
}
