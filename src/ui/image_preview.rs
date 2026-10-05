//! The picture a terminal path names, floated beside the path while the
//! pointer rests on it.
use super::helpers::{animate, capsule, elided, galley_at};
use crate::{
    icons::{self, Icon},
    theme::{self, Palette},
};
use eframe::egui::{
    self, Color32, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType,
    vec2,
};

/// The largest a picture is shown, in points.
pub const MAX_SIZE: Vec2 = vec2(480.0, 360.0);
const PADDING: f32 = 6.0;
const CAPTION: f32 = 24.0;
const GAP: f32 = 6.0;
const MARGIN: f32 = 8.0;
const MIN_WIDTH: f32 = 150.0;
const RADIUS: u8 = 10;

pub struct Preview<'a> {
    pub texture: &'a egui::TextureHandle,
    pub name: &'a str,
    /// Pixels of the file itself, which may exceed the texture's.
    pub pixels: [u32; 2],
    /// The cells of the path, one rectangle for each row.
    pub rows: &'a [Rect],
}

/// The card and the picture inside it: below the path where the picture
/// fits, else above, else on the roomier side and scaled down to fit.
fn place(rows: &[Rect], picture: Vec2, bounds: Rect) -> Option<(Rect, Rect)> {
    let (first, last) = (rows.first()?, rows.last()?);
    let chrome = vec2(PADDING * 2.0, PADDING * 2.0 + CAPTION);
    let room_below = bounds.bottom() - MARGIN - last.bottom() - GAP - chrome.y;
    let room_above = first.top() - GAP - bounds.top() - MARGIN - chrome.y;
    let room_across = bounds.width() - MARGIN * 2.0 - chrome.x;
    let below = room_below >= picture.y || room_below >= room_above;
    let room = vec2(room_across, if below { room_below } else { room_above });
    let scale = (room.x / picture.x).min(room.y / picture.y).min(1.0);
    let picture = picture * scale;
    if picture.x < 24.0 || picture.y < 24.0 {
        return None;
    }
    let size = vec2(picture.x.max(MIN_WIDTH.min(room.x)), picture.y) + chrome;
    let left = first
        .left()
        .min(bounds.right() - MARGIN - size.x)
        .max(bounds.left() + MARGIN);
    let top = if below {
        last.bottom() + GAP
    } else {
        first.top() - GAP - size.y
    };
    let card = Rect::from_min_size(Pos2::new(left.round(), top.round()), size);
    let picture = Rect::from_center_size(
        Pos2::new(card.center().x, card.top() + PADDING + picture.y * 0.5),
        picture,
    );
    Some((card, picture))
}

/// What became of the card this frame.
#[derive(Default)]
pub struct Shown {
    /// Where the card is, so the pointer can travel from the path onto it.
    pub card: Option<Rect>,
    /// The card was clicked, asking for the picture at full size.
    pub clicked: bool,
}

/// How far around the card the pointer still counts as on its way there.
pub const REACH: f32 = GAP + 2.0;

/// Shows the preview above everything in the window. The card takes clicks;
/// everywhere else the pointer keeps belonging to the terminal underneath.
pub fn show(ui: &Ui, p: Palette, bounds: Rect, preview: Option<Preview<'_>>) -> Shown {
    let ctx = ui.ctx();
    let id = Id::new("image-preview");
    let placed = preview.and_then(|preview| {
        let picture = preview.texture.size_vec2() / ctx.pixels_per_point();
        let scale = (MAX_SIZE.x / picture.x)
            .min(MAX_SIZE.y / picture.y)
            .min(1.0);
        place(preview.rows, picture * scale, bounds).map(|placed| (preview, placed))
    });
    let opacity = animate(ctx, id, placed.is_some(), 0.12);
    let Some((preview, (card, picture))) = placed else {
        return Shown::default();
    };
    let area = egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .fixed_pos(card.min)
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| {
            let response = ui
                .allocate_rect(card, Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            response.widget_info(|| {
                WidgetInfo::labeled(
                    WidgetType::Button,
                    true,
                    format!("View {} at full size", preview.name),
                )
            });
            response.clicked()
        });
    let mut painter = ctx.layer_painter(area.response.layer_id);
    painter.set_opacity(opacity);
    for row in preview.rows {
        let y = row.bottom() - 2.0;
        painter.line_segment(
            [Pos2::new(row.left(), y), Pos2::new(row.right(), y)],
            Stroke::new(1.0, p.terminal_fg),
        );
    }
    painter.add(p.popup_shadow().as_shape(card, RADIUS));
    painter.rect_filled(card, RADIUS, p.elevated);
    painter.rect_stroke(card, RADIUS, Stroke::new(1.0, p.border), StrokeKind::Inside);
    let inner = RADIUS - PADDING as u8;
    // A transparent picture sits on a quiet well, not on the card itself.
    painter.rect_filled(picture, inner, p.control);
    painter.add(textured(picture, inner, preview.texture));
    let caption = Rect::from_min_max(
        Pos2::new(card.left() + PADDING + 4.0, picture.bottom()),
        Pos2::new(card.right() - PADDING - 4.0, card.bottom()),
    );
    let pixels = painter.layout_no_wrap(
        format!("{} × {}", preview.pixels[0], preview.pixels[1]),
        theme::regular(11.0),
        p.secondary,
    );
    let name = elided(
        &painter,
        preview.name,
        theme::medium(12.0),
        p.fg,
        caption.width() - pixels.size().x - 10.0,
    );
    galley_at(&painter, caption.left_center(), name);
    galley_at(
        &painter,
        Pos2::new(caption.right() - pixels.size().x, caption.center().y),
        pixels,
    );
    Shown {
        card: Some(card),
        clicked: area.inner,
    }
}

fn textured(rect: Rect, radius: u8, texture: &egui::TextureHandle) -> egui::epaint::RectShape {
    egui::epaint::RectShape::filled(rect, radius, Color32::WHITE).with_texture(
        texture.id(),
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
    )
}

pub struct View<'a> {
    pub texture: &'a egui::TextureHandle,
    pub name: &'a str,
    pub pixels: [u32; 2],
    /// Which of several pictures this is, counted from one, and of how many.
    pub position: Option<(usize, usize)>,
}

/// What the person asked of a full view this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verdict {
    #[default]
    Stay,
    Close,
    /// Show the picture before or after this one.
    Previous,
    Next,
    /// Show the file in the file manager.
    Reveal,
}

/// How far a full view is magnified past fitting the window, and where the
/// picture's centre has been moved to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    pub factor: f32,
    pub offset: Vec2,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            factor: 1.0,
            offset: Vec2::ZERO,
        }
    }
}

impl Zoom {
    /// Magnifies by `by` up to `most`, keeping the point of the picture at
    /// `anchor`, measured from the centre of the fitted picture, in place.
    fn magnify(&mut self, by: f32, anchor: Vec2, most: f32) {
        let factor = (self.factor * by).clamp(1.0, most.max(1.0));
        self.offset = anchor - (anchor - self.offset) * (factor / self.factor);
        self.factor = factor;
    }

    /// Where the picture is drawn. It is kept from leaving `room`: an axis
    /// that fits stays centred, and a larger one stops at its edges.
    fn place(&mut self, fitted: Rect, room: Rect) -> Rect {
        let size = fitted.size() * self.factor;
        let slack = ((size - room.size()) * 0.5).max(Vec2::ZERO);
        self.offset = self.offset.clamp(-slack, slack);
        Rect::from_center_size(fitted.center() + self.offset, size)
    }
}

const VIEW_MARGIN: f32 = 24.0;
const VIEW_CAPTION: f32 = 40.0;
/// A small picture fills the window only up to this many times its size.
const VIEW_FILL: f32 = 4.0;
/// Zooming in stops at this many screen pixels for each pixel of the file.
const VIEW_PIXEL: f32 = 8.0;
const VIEW_STEP: f32 = 1.25;

/// The part of the window a fitted picture may use, above its caption.
fn room(bounds: Rect) -> Rect {
    Rect::from_min_max(
        bounds.min + Vec2::splat(VIEW_MARGIN),
        (bounds.max - vec2(VIEW_MARGIN, VIEW_MARGIN + VIEW_CAPTION))
            .max(bounds.min + Vec2::splat(VIEW_MARGIN + 1.0)),
    )
}

/// Where the picture of a full view goes before any zoom: centred above its
/// caption and as large as the window allows, short of blowing a small
/// picture up to mush.
fn fit(pixels: Vec2, bounds: Rect) -> Rect {
    let room = room(bounds);
    let scale = (room.width() / pixels.x)
        .min(room.height() / pixels.y)
        .min(VIEW_FILL);
    Rect::from_center_size(room.center(), pixels * scale)
}

/// The picture over the dimmed window. The wheel, a pinch and the plus and
/// minus keys zoom, a drag moves a zoomed picture and 0 fits it again. One of
/// several is left for its neighbours with the arrow keys or the controls at
/// the window's sides. A click anywhere else closes it, as Escape does.
pub fn view(ui: &Ui, p: Palette, bounds: Rect, view: View<'_>, zoom: &mut Zoom) -> Verdict {
    let ctx = ui.ctx();
    egui::Area::new(Id::new("image-view"))
        .order(egui::Order::Foreground)
        .fixed_pos(bounds.min)
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| view_contents(ui, p, bounds, view, zoom))
        .inner
}

/// A round control over a full view; painted once the picture is.
fn view_control(ui: &Ui, rect: Rect, label: &str) -> egui::Response {
    let response = ui
        .interact(rect, Id::new(("image-view", label)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    response
}

fn view_contents(
    ui: &mut Ui,
    p: Palette,
    bounds: Rect,
    view: View<'_>,
    zoom: &mut Zoom,
) -> Verdict {
    let ctx = ui.ctx().clone();
    let several = view.position.is_some_and(|(_, count)| count > 1);
    let response = ui.allocate_rect(bounds, Sense::click_and_drag());
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Window,
            true,
            format!(
                "{}. Scroll to zoom, {}click or press Escape to close",
                view.name,
                if several {
                    "use the arrow keys for the other pictures, "
                } else {
                    ""
                }
            ),
        )
    });
    let scale = ctx.pixels_per_point();
    let size = vec2(view.pixels[0] as f32, view.pixels[1] as f32) / scale;
    let fitted = fit(size, bounds);
    let most = VIEW_PIXEL * size.x / fitted.width();
    let (by, reset, step) = ctx.input(|input| {
        let keys = |keys: &[egui::Key]| keys.iter().any(|key| input.key_pressed(*key));
        let mut by = input.zoom_delta() * (input.smooth_scroll_delta.y * 0.004).exp();
        if keys(&[egui::Key::Plus, egui::Key::Equals]) {
            by *= VIEW_STEP;
        }
        if keys(&[egui::Key::Minus]) {
            by /= VIEW_STEP;
        }
        let step = if !several {
            Verdict::Stay
        } else if keys(&[egui::Key::ArrowLeft]) {
            Verdict::Previous
        } else if keys(&[egui::Key::ArrowRight]) {
            Verdict::Next
        } else {
            Verdict::Stay
        };
        (by, keys(&[egui::Key::Num0]), step)
    });
    if reset {
        *zoom = Zoom::default();
    } else if by != 1.0 {
        // Under the pointer for the wheel; about the centre for the keys.
        let anchor = response
            .hover_pos()
            .map_or(zoom.offset, |pointer| pointer - fitted.center());
        zoom.magnify(by, anchor, most);
    }
    if zoom.factor > 1.0 {
        zoom.offset += response.drag_delta();
        response.clone().on_hover_cursor(if response.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    } else {
        zoom.offset = Vec2::ZERO;
    }
    let picture = zoom.place(fitted, room(bounds));
    let painter = ui.painter();
    // A zoomed picture stops short of the window's edge and rounded corners.
    let framed = painter.with_clip_rect(bounds.shrink(VIEW_MARGIN * 0.5));
    framed.add(p.sheet_shadow().as_shape(picture, 8));
    framed.rect_filled(picture, 8, p.elevated);
    framed.add(textured(picture, 8, view.texture));
    let percent = (picture.width() / size.x * 100.0).round();
    let place = view
        .position
        .filter(|_| several)
        .map(|(index, count)| format!("{index} of {count}   "))
        .unwrap_or_default();
    // The control that shows the file in its folder ends the caption.
    const REVEAL: f32 = 26.0;
    let label = elided(
        painter,
        &format!(
            "{place}{}   {} × {}   {percent}%",
            view.name, view.pixels[0], view.pixels[1]
        ),
        theme::medium(12.0),
        p.fg,
        bounds.width() - VIEW_MARGIN * 2.0 - 28.0 - REVEAL,
    );
    // Under a fitted picture; over a zoomed one, at the foot of the window.
    let foot = bounds.bottom() - VIEW_MARGIN - VIEW_CAPTION * 0.5 + 4.0;
    let chip = Rect::from_center_size(
        Pos2::new(
            bounds.center().x,
            (picture.bottom() + VIEW_CAPTION * 0.5 + 4.0).min(foot),
        ),
        vec2(label.size().x + 24.0 + REVEAL, 28.0),
    );
    // The caption is not the picture: a click on it does not close the view.
    ui.interact(chip, Id::new("image-view-caption"), Sense::click());
    capsule(painter, chip, p);
    galley_at(
        painter,
        Pos2::new(chip.left() + 14.0, chip.center().y),
        label,
    );
    let reveal = view_control(
        ui,
        Rect::from_center_size(
            Pos2::new(chip.right() - 15.0, chip.center().y),
            Vec2::splat(22.0),
        ),
        crate::platform::files::REVEAL_LABEL,
    );
    if reveal.hovered() {
        painter.rect_filled(reveal.rect, 11, theme::tint(p.fg, 0.12));
    }
    icons::paint(
        painter,
        Rect::from_center_size(reveal.rect.center(), Vec2::splat(13.0)),
        Icon::Folder,
        if reveal.hovered() { p.fg } else { p.secondary },
    );
    let mut verdict = step;
    if reveal
        .on_hover_text(crate::platform::files::REVEAL_LABEL)
        .clicked()
    {
        verdict = Verdict::Reveal;
    }
    if several {
        let middle = room(bounds).center().y;
        for (x, icon, label, to) in [
            (
                bounds.left() + VIEW_MARGIN + 8.0,
                Icon::ChevronLeft,
                "Previous picture",
                Verdict::Previous,
            ),
            (
                bounds.right() - VIEW_MARGIN - 8.0,
                Icon::ChevronRight,
                "Next picture",
                Verdict::Next,
            ),
        ] {
            let rect = Rect::from_center_size(Pos2::new(x, middle), Vec2::splat(34.0));
            let control = view_control(ui, rect, label);
            capsule(painter, rect, p);
            if control.hovered() {
                painter.rect_filled(rect, 17, theme::tint(p.fg, 0.12));
            }
            icons::paint(
                painter,
                Rect::from_center_size(rect.center(), Vec2::splat(14.0)),
                icon,
                if control.hovered() { p.fg } else { p.secondary },
            );
            if control.on_hover_text(label).clicked() {
                verdict = to;
            }
        }
    }
    if verdict == Verdict::Stay && response.clicked() {
        verdict = Verdict::Close;
    }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 600.0));

    fn row(left: f32, top: f32) -> Rect {
        Rect::from_min_size(Pos2::new(left, top), vec2(160.0, 20.0))
    }

    #[test]
    fn the_card_sits_below_the_path_and_flips_above_near_the_bottom() {
        let (card, picture) = place(&[row(100.0, 60.0)], vec2(300.0, 200.0), BOUNDS).unwrap();
        assert_eq!(card.min, Pos2::new(100.0, 86.0));
        assert_eq!(picture.size(), vec2(300.0, 200.0));
        assert!(card.contains_rect(picture));
        let (card, _) = place(&[row(100.0, 500.0)], vec2(300.0, 200.0), BOUNDS).unwrap();
        assert_eq!(card.bottom(), 494.0);
        assert!(BOUNDS.contains_rect(card));
    }

    #[test]
    fn the_card_stays_inside_a_narrow_window_and_scales_the_picture() {
        let bounds = Rect::from_min_max(Pos2::ZERO, Pos2::new(320.0, 240.0));
        let rows = [row(150.0, 100.0), row(0.0, 120.0)];
        let (card, picture) = place(&rows, vec2(480.0, 360.0), bounds).unwrap();
        assert!(bounds.shrink(MARGIN - 0.5).contains_rect(card), "{card:?}");
        assert!(card.contains_rect(picture));
        assert!((picture.aspect_ratio() - 480.0 / 360.0).abs() < 0.01);
        // Too little room on either side shows nothing rather than a sliver.
        let short = Rect::from_min_max(Pos2::ZERO, Pos2::new(320.0, 90.0));
        assert!(place(&[row(0.0, 30.0)], vec2(480.0, 360.0), short).is_none());
    }

    #[test]
    fn a_full_view_fills_the_window_but_enlarges_small_pictures_only_so_far() {
        let small = fit(vec2(20.0, 10.0), BOUNDS);
        assert_eq!(small.size(), vec2(80.0, 40.0));
        assert_eq!(small.center().x, 400.0);
        let large = fit(vec2(4000.0, 1000.0), BOUNDS);
        assert_eq!(large.width(), 800.0 - VIEW_MARGIN * 2.0);
        assert!((large.aspect_ratio() - 4.0).abs() < 0.01);
        let tall = fit(vec2(1000.0, 4000.0), BOUNDS);
        assert!(tall.bottom() + VIEW_CAPTION <= 600.0 - VIEW_MARGIN + 0.5);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer_and_stays_within_its_limits() {
        let fitted = fit(vec2(400.0, 300.0), BOUNDS);
        let mut zoom = Zoom::default();
        // The picture's point 100 right of centre stays under the pointer.
        let anchor = vec2(100.0, 0.0);
        zoom.magnify(2.0, anchor, 8.0);
        assert_eq!((zoom.factor, zoom.offset), (2.0, vec2(-100.0, 0.0)));
        zoom.magnify(100.0, anchor, 8.0);
        assert_eq!(zoom.factor, 8.0);
        // Zooming all the way out returns to the fitted picture.
        zoom.magnify(0.001, anchor, 8.0);
        assert_eq!(zoom.factor, 1.0);
        let picture = zoom.place(fitted, room(BOUNDS));
        assert!((picture.min - fitted.min).length() < 0.01, "{picture:?}");
        assert!(zoom.offset.length() < 0.01);
    }

    #[test]
    fn a_zoomed_picture_cannot_be_dragged_out_of_the_window() {
        let room = room(BOUNDS);
        let fitted = fit(vec2(400.0, 300.0), BOUNDS);
        let mut zoom = Zoom {
            factor: 2.0,
            offset: vec2(9000.0, -9000.0),
        };
        let picture = zoom.place(fitted, room);
        assert_eq!(picture.size(), fitted.size() * 2.0);
        assert_eq!(picture.left(), room.left());
        assert_eq!(picture.bottom(), room.bottom());
    }

    #[test]
    fn the_card_takes_a_click_and_the_full_view_closes_on_one() {
        let ctx = egui::Context::default();
        theme::fonts(&ctx);
        let p = Palette::new(crate::config::Theme::Graphite);
        let mut texture = None;
        let rows = [row(100.0, 60.0)];
        let mut frame = |events: Vec<egui::Event>, full: bool| {
            let mut clicked = false;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(BOUNDS),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let texture = texture.get_or_insert_with(|| {
                        ui.ctx().load_texture(
                            "test",
                            egui::ColorImage::filled([300, 200], Color32::RED),
                            Default::default(),
                        )
                    });
                    clicked = if full {
                        let full = View {
                            texture,
                            name: "a.png",
                            pixels: [300, 200],
                            position: None,
                        };
                        view(ui, p, BOUNDS, full, &mut Zoom::default()) == Verdict::Close
                    } else {
                        let preview = Preview {
                            texture,
                            name: "a.png",
                            pixels: [300, 200],
                            rows: &rows,
                        };
                        let shown = show(ui, p, BOUNDS, Some(preview));
                        assert!(shown.card.unwrap().contains(Pos2::new(200.0, 150.0)));
                        shown.clicked
                    };
                },
            );
            output.textures_delta.clear();
            clicked
        };
        let pos = Pos2::new(200.0, 150.0);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for full in [false, true] {
            assert!(!frame(vec![egui::Event::PointerMoved(pos)], full));
            assert!(!frame(Vec::new(), full));
            assert!(!frame(vec![button(true)], full));
            assert!(frame(vec![button(false)], full), "full view: {full}");
        }
    }

    #[test]
    fn one_of_several_pictures_steps_by_key_and_by_its_controls() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let p = Palette::for_config(&crate::config::Config::default());
        let mut texture = None;
        let mut frame = |events: Vec<egui::Event>, position| {
            let mut verdict = Verdict::Stay;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(BOUNDS),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let texture = texture.get_or_insert_with(|| {
                        ui.ctx().load_texture(
                            "test",
                            egui::ColorImage::filled([300, 200], Color32::RED),
                            Default::default(),
                        )
                    });
                    let full = View {
                        texture,
                        name: "a.png",
                        pixels: [300, 200],
                        position,
                    };
                    verdict = view(ui, p, BOUNDS, full, &mut Zoom::default());
                },
            );
            output.textures_delta.clear();
            verdict
        };
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let several = Some((2, 3));
        assert_eq!(
            frame(vec![key(egui::Key::ArrowRight)], several),
            Verdict::Next
        );
        assert_eq!(
            frame(vec![key(egui::Key::ArrowLeft)], several),
            Verdict::Previous
        );
        // A picture on its own, or the only one of its list, has no neighbour.
        assert_eq!(frame(vec![key(egui::Key::ArrowRight)], None), Verdict::Stay);
        assert_eq!(
            frame(vec![key(egui::Key::ArrowRight)], Some((1, 1))),
            Verdict::Stay
        );
        // The controls at the sides step; they do not close the view.
        let mut click = |pos: Pos2, position| {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(vec![egui::Event::PointerMoved(pos)], position);
            frame(Vec::new(), position);
            frame(vec![button(true)], position);
            frame(vec![button(false)], position)
        };
        let middle = room(BOUNDS).center().y;
        let left = Pos2::new(BOUNDS.left() + VIEW_MARGIN + 8.0, middle);
        let right = Pos2::new(BOUNDS.right() - VIEW_MARGIN - 8.0, middle);
        assert_eq!(click(left, several), Verdict::Previous);
        assert_eq!(click(right, several), Verdict::Next);
        // Without neighbours the same place is the dimmed window: it closes.
        assert_eq!(click(left, None), Verdict::Close);
    }
}
