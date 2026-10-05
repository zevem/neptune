//! Painting consumes prepared draw data and emits selection and link interactions.

use super::cache::Cache;
use crate::{
    config::{Config, Cursor},
    theme::Palette,
};
use eframe::egui::{self, Pos2, Rect, Stroke, Vec2};
use terminal_core::{CursorShape, Mode as TermMode, Point, SelectionType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionInteraction {
    Start { point: Point, kind: SelectionType },
    Update(Point),
    Clear,
}

pub struct PaintResult {
    pub response: egui::Response,
    pub interaction: Option<SelectionInteraction>,
    pub open_link: Option<super::LinkTarget>,
    /// Readings of a picture path the pointer rests on, likeliest first.
    pub image_paths: Vec<super::ImagePath>,
}

impl Cache {
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        config: &Config,
        p: Palette,
        active: bool,
        search: &str,
        preedit: &str,
    ) -> PaintResult {
        let response = ui.interact(
            rect,
            ui.id().with("terminal"),
            egui::Sense::click_and_drag(),
        );
        let (hovered_link, open_link) = self.link_interaction(ui, &response, rect);
        let image_paths = if self.hints.is_none() {
            self.image_path_hover(ui, &response, rect)
        } else {
            Vec::new()
        };
        response.clone().on_hover_cursor(if hovered_link.is_some() {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::Text
        });
        let selection = self.selection;
        let font = crate::platform::fonts::terminal_font(config.font_size, false);
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0, self.background);
        for (y, row) in self.rows.iter().enumerate() {
            let top = rect.top() + y as f32 * self.cell.y;
            for &(start, end, color) in &row.backgrounds {
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(rect.left() + start as f32 * self.cell.x, top),
                        Vec2::new((end - start) as f32 * self.cell.x, self.cell.y),
                    ),
                    0,
                    color,
                );
            }
            if let Some(range) = selection {
                let line = y as i32 - self.display_offset as i32;
                for x in 0..self.columns as usize {
                    if range.contains(Point::new(line, x)) {
                        painter.rect_filled(
                            Rect::from_min_size(
                                Pos2::new(rect.left() + x as f32 * self.cell.x, top),
                                self.cell,
                            ),
                            0,
                            p.selection,
                        );
                    }
                }
            }
            if !search.is_empty() {
                // ASCII and Unicode character offsets are kept distinct from UTF-8 bytes.
                for (byte, _) in row.text.match_indices(search) {
                    let col = row
                        .text_columns
                        .iter()
                        .rev()
                        .find(|(offset, _)| *offset <= byte)
                        .map(|(_, col)| *col)
                        .unwrap_or(0);
                    let end = row
                        .text_columns
                        .iter()
                        .find(|(offset, _)| *offset >= byte + search.len())
                        .map(|(_, col)| *col)
                        .unwrap_or(self.columns as usize);
                    let len = end.saturating_sub(col);
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(rect.left() + col as f32 * self.cell.x, top),
                            Vec2::new(len as f32 * self.cell.x, self.cell.y),
                        ),
                        2,
                        p.accent.gamma_multiply(0.25),
                    );
                }
            }
            for run in &row.runs {
                painter.galley(
                    Pos2::new(
                        rect.left() + run.column as f32 * self.cell.x,
                        top + (self.cell.y - run.galley.size().y) * 0.5,
                    ),
                    run.galley.clone(),
                    p.terminal_fg,
                );
            }
        }
        // Repaint selected glyphs through a cell-aligned clip, preserving cached
        // shaping when selection changes (including wide and combining glyphs).
        if let (Some(range), Some(ink)) = (selection, p.selection_text) {
            for (y, row) in self.rows.iter().enumerate() {
                let line = y as i32 - self.display_offset as i32;
                let selected =
                    (0..usize::from(self.columns)).filter(|&x| range.contains(Point::new(line, x)));
                let mut spans = selected.peekable();
                while let Some(start) = spans.next() {
                    let mut end = start + 1;
                    while spans.peek() == Some(&end) {
                        spans.next();
                        end += 1;
                    }
                    let top = rect.top() + y as f32 * self.cell.y;
                    let clip = Rect::from_min_size(
                        Pos2::new(rect.left() + start as f32 * self.cell.x, top),
                        Vec2::new((end - start) as f32 * self.cell.x, self.cell.y),
                    )
                    .intersect(rect);
                    for run in &row.runs {
                        painter
                            .with_clip_rect(clip)
                            .galley_with_override_text_color(
                                Pos2::new(
                                    rect.left() + run.column as f32 * self.cell.x,
                                    top + (self.cell.y - run.galley.size().y) * 0.5,
                                ),
                                run.galley.clone(),
                                ink,
                            );
                    }
                }
            }
        }
        if let Some(link) = hovered_link {
            let columns = usize::from(self.columns);
            for row in link.start / columns..=link.end / columns {
                let start = if row == link.start / columns {
                    link.start % columns
                } else {
                    0
                };
                let end = if row == link.end / columns {
                    link.end % columns + 1
                } else {
                    columns
                };
                let y = rect.top() + (row + 1) as f32 * self.cell.y - 2.0;
                painter.line_segment(
                    [
                        Pos2::new(rect.left() + start as f32 * self.cell.x, y),
                        Pos2::new(rect.left() + end as f32 * self.cell.x, y),
                    ],
                    Stroke::new(1.0, p.terminal_fg),
                );
            }
        }
        if let Some((x, y, shape)) = self.cursor {
            let pos = rect.min + Vec2::new(x as f32 * self.cell.x, y as f32 * self.cell.y);
            let cursor = Rect::from_min_size(pos, self.cell);
            let focused = active && ui.input(|i| i.focused);
            let blink =
                !config.cursor_blink || ui.input(|i| ((i.time * 2.0) as u64).is_multiple_of(2));
            if focused && blink {
                let shape = match config.cursor {
                    Cursor::Beam => CursorShape::Beam,
                    Cursor::Underline => CursorShape::Underline,
                    Cursor::Block => shape,
                };
                match shape {
                    CursorShape::Beam => {
                        painter.rect_filled(
                            Rect::from_min_size(pos, Vec2::new(1.5, self.cell.y)),
                            0,
                            self.cursor_color,
                        );
                    }
                    CursorShape::Underline => {
                        painter.line_segment(
                            [cursor.left_bottom(), cursor.right_bottom()],
                            Stroke::new(2.0, self.cursor_color),
                        );
                    }
                    _ => {
                        if let Some(ink) = p.cursor_text {
                            painter.rect_filled(cursor, 1, self.cursor_color);
                            if let Some(row) = self.rows.get(y) {
                                for run in &row.runs {
                                    painter
                                        .with_clip_rect(cursor.intersect(rect))
                                        .galley_with_override_text_color(
                                            Pos2::new(
                                                rect.left() + run.column as f32 * self.cell.x,
                                                pos.y + (self.cell.y - run.galley.size().y) * 0.5,
                                            ),
                                            run.galley.clone(),
                                            ink,
                                        );
                                }
                            }
                        } else {
                            painter.rect_filled(cursor, 1, self.cursor_color.gamma_multiply(0.42));
                        }
                    }
                }
            } else if !focused {
                painter.rect_stroke(
                    cursor,
                    1,
                    Stroke::new(1.0, p.muted),
                    egui::StrokeKind::Inside,
                );
            }
            if focused {
                ui.ctx().output_mut(|o| {
                    o.ime = Some(egui::output::IMEOutput {
                        rect,
                        cursor_rect: cursor,
                        purpose: egui::IMEPurpose::Normal,
                        should_interrupt_composition: false,
                    })
                });
            }
            if focused && config.cursor_blink {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
            if !preedit.is_empty() {
                painter.text(
                    pos + Vec2::new(0.0, self.cell.y),
                    egui::Align2::LEFT_TOP,
                    preedit,
                    font,
                    p.accent,
                );
            }
        }
        self.paint_hints(ui, rect, p);
        let interaction = response.interact_pointer_pos().and_then(|pos| {
            if self.link_pointer_owned {
                return None;
            }
            if self.mode.intersects(TermMode::MOUSE_MODE) && !ui.input(|i| i.modifiers.shift) {
                return None;
            }
            let point = super::geometry::point_at(
                rect,
                self.cell,
                self.columns,
                self.lines,
                self.display_offset,
                pos,
            );
            if response.double_clicked() {
                Some(SelectionInteraction::Start {
                    point,
                    kind: SelectionType::Semantic,
                })
            } else if response.drag_started() {
                Some(SelectionInteraction::Start {
                    point,
                    kind: SelectionType::Simple,
                })
            } else if response.dragged() {
                Some(SelectionInteraction::Update(point))
            } else if response.clicked() {
                Some(SelectionInteraction::Clear)
            } else {
                None
            }
        });
        PaintResult {
            response,
            interaction,
            open_link,
            image_paths,
        }
    }
}
