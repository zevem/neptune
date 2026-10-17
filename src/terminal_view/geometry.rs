//! Pure terminal geometry, independent of PTY resize side effects.

use eframe::egui::{Pos2, Rect, Vec2};
use terminal_core::Point;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeRequest {
    pub columns: u16,
    pub lines: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

pub fn calculate(rect: Rect, cell: Vec2, scale: f32) -> ResizeRequest {
    ResizeRequest {
        columns: ((rect.width() / cell.x.max(1.0)).floor() as u16).max(2),
        lines: ((rect.height() / cell.y.max(1.0)).floor() as u16).max(1),
        pixel_width: (rect.width().max(0.0) * scale.max(0.0)) as u16,
        pixel_height: (rect.height().max(0.0) * scale.max(0.0)) as u16,
    }
}

pub fn point_at(
    rect: Rect,
    cell: Vec2,
    columns: u16,
    lines: u16,
    display_offset: usize,
    pos: Pos2,
) -> Point {
    let column = (((pos.x - rect.left()) / cell.x.max(1.0)).floor().max(0.0) as usize)
        .min(usize::from(columns.saturating_sub(1)));
    let row = (((pos.y - rect.top()) / cell.y.max(1.0)).floor().max(0.0) as i32)
        .min(i32::from(lines.saturating_sub(1)));
    Point::new(row - display_offset as i32, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_clamps_minimum_grid_and_scales_physical_pixels() {
        assert_eq!(
            calculate(
                Rect::from_min_size(Pos2::ZERO, Vec2::new(1.0, 1.0)),
                Vec2::new(8.0, 16.0),
                2.0
            ),
            ResizeRequest {
                columns: 2,
                lines: 1,
                pixel_width: 2,
                pixel_height: 2
            }
        );
    }

    #[test]
    fn pointer_at_or_before_the_left_edge_reaches_column_zero() {
        let rect = Rect::from_min_size(Pos2::new(29.0, 19.0), Vec2::new(80.0, 48.0));
        for x in [-100.0, 28.9, 29.0, 29.1, 36.9] {
            assert_eq!(
                point_at(rect, Vec2::new(8.0, 16.0), 10, 3, 7, Pos2::new(x, 27.0)),
                Point::new(-7, 0),
            );
        }
    }

    #[test]
    fn pointer_selection_is_clamped_and_translates_scrollback() {
        assert_eq!(
            point_at(
                Rect::from_min_size(Pos2::ZERO, Vec2::new(80.0, 48.0)),
                Vec2::new(8.0, 16.0),
                10,
                3,
                7,
                Pos2::new(500.0, 500.0)
            ),
            Point::new(-5, 9)
        );
    }
}
