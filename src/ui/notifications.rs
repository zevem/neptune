//! Notification popover and the unread pill shared with the sidebar. Every row
//! captures its originating pane and generation.
use super::{
    Action,
    helpers::{ButtonKind, button, elided, galley_at, place},
};
use crate::{
    icons::{self, Icon},
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Galley, Id, Key, Layout, Order, Painter, Pos2,
    Rect, Sense, Stroke, Ui, Vec2, WidgetInfo, WidgetType, vec2,
};
use neptune_model::PaneId;
use std::{sync::Arc, time::Duration};

const WIDTH: f32 = 360.0;
/// The popover floats where messages do: inset from the window's trailing edge.
const EDGE: f32 = 14.0;
const HEADER: f32 = 44.0;
/// Rows show at most this much of an alert, so a flood stays cheap to lay out.
const HEADLINE_CHARS: usize = 240;
const BODY_CHARS: usize = 320;

pub struct NotificationItem<'a> {
    pub sequence: u64,
    pub pane: PaneId,
    pub generation: u64,
    /// Seeds the identity colour of the workspace's sidebar tile.
    pub identity: u64,
    pub workspace: &'a str,
    /// The program that asked, as its pane header names it.
    pub program: String,
    pub title: &'a str,
    pub body: &'a str,
    pub unread: bool,
    pub age: Duration,
}

pub struct NotificationView<'a> {
    /// Newest first.
    pub items: &'a [NotificationItem<'a>],
    /// Desktop banners could not be delivered.
    pub unavailable: bool,
    pub first_frame: &'a mut bool,
}

pub fn show(
    ctx: &egui::Context,
    p: Palette,
    mut view: NotificationView<'_>,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let first_frame = *view.first_frame;
    let bounds = ctx.content_rect();
    let width = WIDTH.min(bounds.width() - EDGE * 2.0).max(160.0);
    let top = bounds.top() + metrics::TOOLBAR_HEIGHT + 6.0;
    let max_height = (bounds.bottom() - top - EDGE).clamp(120.0, 520.0);
    let response = egui::Area::new(Id::new("notification-popover"))
        .order(Order::Foreground)
        .movable(false)
        .fixed_pos(Pos2::new(bounds.right() - width - EDGE, top))
        .show(ctx, |ui| {
            // Areas remember their measured size. Offering the full height
            // again lets a once-empty popover grow with new alerts.
            ui.set_max_height(max_height + 2.0);
            Frame::new()
                .fill(p.elevated)
                .corner_radius(12)
                .stroke(Stroke::new(1.0, p.border))
                .shadow(p.popup_shadow())
                .show(ui, |ui| {
                    ui.set_width(width - 2.0);
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    panel(ui, p, &mut view, max_height, actions);
                });
        })
        .response;
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Window, true, "Notification history"));
    if !first_frame && response.clicked_elsewhere() {
        actions.push(Action::CloseOverlay);
    }
    if !view.items.is_empty() {
        // Keeps "now" honest while the popover stays open.
        ctx.request_repaint_after(Duration::from_secs(30));
    }
    response
}

fn panel(
    ui: &mut Ui,
    p: Palette,
    view: &mut NotificationView<'_>,
    max_height: f32,
    actions: &mut Vec<Action>,
) {
    let (_, header) = ui.allocate_space(vec2(ui.available_width(), HEADER));
    let title = ui.painter().text(
        Pos2::new(header.left() + 16.0, header.center().y),
        Align2::LEFT_CENTER,
        "Notifications",
        theme::semibold(13.0),
        p.fg,
    );
    let unread = view.items.iter().filter(|item| item.unread).count();
    if unread > 0 {
        pill(
            ui.painter(),
            Pos2::new(title.right() + 8.0, header.center().y),
            Align2::LEFT_CENTER,
            unread,
            p,
        );
    }
    if !view.items.is_empty() {
        place(
            ui,
            header.shrink2(vec2(6.0, 0.0)),
            Layout::right_to_left(Align::Center),
            "notification-actions",
            |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if button(ui, p, "Clear all", ButtonKind::Quiet).clicked() {
                    actions.push(Action::ClearNotifications);
                }
                ui.add_enabled_ui(unread > 0, |ui| {
                    if button(ui, p, "Mark all read", ButtonKind::Quiet).clicked() {
                        actions.push(Action::ReadNotifications);
                    }
                });
            },
        );
    }
    ui.painter()
        .line_segment([header.left_bottom(), header.right_bottom()], p.hairline());

    let note = view.unavailable.then(|| {
        wrapped(
            ui.painter(),
            "Desktop banners are unavailable. Check your system notification settings.",
            theme::regular(11.5),
            p.muted,
            ui.available_width() - 48.0,
            3,
        )
    });
    let footer = note.as_ref().map_or(0.0, |note| note.size().y + 20.0);
    egui::ScrollArea::vertical()
        .id_salt("notification-list")
        .max_height((max_height - HEADER - footer).max(60.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if !ui.is_sizing_pass() {
                *view.first_frame = false;
            }
            if view.items.is_empty() {
                empty(ui, p);
                return;
            }
            ui.add_space(6.0);
            // The arrow keys walk the alerts as they do a menu: nothing is
            // highlighted until one is pressed, Enter opens the highlighted
            // alert and Delete dismisses it, moving on to its neighbour.
            let ids: Vec<Id> = view
                .items
                .iter()
                .map(|item| ui.id().with(("notification", item.sequence)))
                .collect();
            let focused = ui.memory(|memory| memory.focused());
            let current = ids
                .iter()
                .position(|id| focused == Some(*id) || focused == Some(id.with("dismiss")));
            let (down, up, delete) = ui.input(|input| {
                (
                    input.key_pressed(Key::ArrowDown),
                    input.key_pressed(Key::ArrowUp),
                    input.key_pressed(Key::Delete) || input.key_pressed(Key::Backspace),
                )
            });
            let last = ids.len() - 1;
            let target = match (current, down, up) {
                (Some(index), true, false) => Some((index + 1).min(last)),
                (Some(index), false, true) => Some(index.saturating_sub(1)),
                (None, true, false) => Some(0),
                (None, false, true) => Some(last),
                _ => None,
            };
            // The pointer owns the highlight while it is over the list.
            let pointed = ui.rect_contains_pointer(ui.clip_rect());
            for (index, item) in view.items.iter().enumerate() {
                let response = row(ui, p, ids[index], item, pointed, actions);
                if target == Some(index) {
                    response.request_focus();
                    response.scroll_to_me(None);
                } else if delete && current == Some(index) {
                    actions.push(Action::DismissNotification(item.sequence));
                    let neighbour = if index < last {
                        index + 1
                    } else {
                        index.wrapping_sub(1)
                    };
                    if let Some(next) = ids.get(neighbour) {
                        ui.memory_mut(|memory| memory.request_focus(*next));
                    }
                }
            }
            ui.add_space(6.0);
        });
    if let Some(note) = note {
        let (_, rect) = ui.allocate_space(vec2(ui.available_width(), footer));
        ui.painter()
            .line_segment([rect.left_top(), rect.right_top()], p.hairline());
        icons::paint(
            ui.painter(),
            Rect::from_center_size(
                Pos2::new(rect.left() + 22.0, rect.top() + 18.0),
                Vec2::splat(13.0),
            ),
            Icon::Warning,
            p.muted,
        );
        ui.painter().galley(
            Pos2::new(rect.left() + 36.0, rect.top() + 10.0),
            note,
            Color32::PLACEHOLDER,
        );
    }
}

fn empty(ui: &mut Ui, p: Palette) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 176.0));
    let painter = ui.painter();
    let centre = Pos2::new(rect.center().x, rect.top() + 58.0);
    painter.circle_filled(centre, 21.0, p.control);
    icons::paint(
        painter,
        Rect::from_center_size(centre, Vec2::splat(18.0)),
        Icon::Bell,
        p.secondary,
    );
    for (text, font, color, y) in [
        ("No notifications", theme::medium(13.0), p.fg, 102.0),
        (
            "Alerts from your terminals appear here.",
            theme::regular(12.0),
            p.secondary,
            122.0,
        ),
    ] {
        let line = elided(painter, text, font, color, rect.width() - 32.0);
        galley_at(
            painter,
            Pos2::new(rect.center().x - line.size().x * 0.5, rect.top() + y),
            line,
        );
    }
}

fn row(
    ui: &mut Ui,
    p: Palette,
    id: Id,
    item: &NotificationItem<'_>,
    pointed: bool,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let (headline, body) = match (item.title.is_empty(), item.body.is_empty()) {
        (false, _) => (item.title, item.body),
        (true, false) => (item.body, ""),
        (true, true) => ("Notification", ""),
    };
    let left = ui.cursor().left() + 6.0;
    let right = left + ui.available_width() - 12.0;
    let text_left = left + 44.0;
    let painter = ui.painter().clone();
    let time = painter.layout_no_wrap(relative_time(item.age), theme::regular(11.0), p.muted);
    let headline = wrapped(
        &painter,
        clip(headline, HEADLINE_CHARS),
        theme::medium(13.0),
        if item.unread {
            p.fg
        } else {
            theme::mix(p.secondary, p.fg, 0.3)
        },
        // The dismiss control takes the time's place, so reserve the wider.
        right - 12.0 - time.size().x.max(20.0) - 8.0 - text_left,
        2,
    );
    let body = (!body.is_empty()).then(|| {
        wrapped(
            &painter,
            clip(body, BODY_CHARS),
            theme::regular(12.0),
            p.secondary,
            right - 12.0 - text_left,
            2,
        )
    });
    let source = elided(
        &painter,
        &if item.program.is_empty() {
            item.workspace.to_owned()
        } else {
            format!("{} · {}", item.workspace, item.program)
        },
        theme::regular(11.0),
        p.muted,
        right - 12.0 - text_left,
    );
    let line = headline
        .rows
        .first()
        .map_or(17.0, |row| row.rect().height());
    let height = 10.0
        + headline.size().y
        + body.as_ref().map_or(0.0, |body| 2.0 + body.size().y)
        + 4.0
        + source.size().y
        + 10.0;

    let (_, slot) = ui.allocate_space(vec2(ui.available_width(), height.max(44.0)));
    let rect = Rect::from_min_max(Pos2::new(left, slot.top()), Pos2::new(right, slot.bottom()));
    let response = ui.interact(rect, id, Sense::click());
    let label = format!(
        "{}, {}{}",
        headline.text(),
        item.workspace,
        if item.unread { ", unread" } else { "" }
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));

    // Dismissing keeps one identity and place; it shows, in the time's stead,
    // while the row is pointed at.
    let middle = rect.top() + 10.0 + line * 0.5;
    let control = Rect::from_center_size(Pos2::new(rect.right() - 18.0, middle), Vec2::splat(28.0));
    let dismiss = ui.interact(control, id.with("dismiss"), Sense::click());
    dismiss.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Dismiss notification"));
    let within = ui.rect_contains_pointer(rect);
    let dismissing = within || dismiss.has_focus();
    let highlighted = dismissing || (response.has_focus() && !pointed);
    // Enter still opens a highlighted alert that has been scrolled away.
    if dismiss.clicked() {
        actions.push(Action::DismissNotification(item.sequence));
    } else if response.clicked() {
        actions.push(Action::OpenNotification(item.pane, item.generation));
    }
    if !ui.is_rect_visible(slot) {
        return response;
    }
    if highlighted {
        painter.rect_filled(
            rect,
            metrics::ROW_RADIUS,
            if response.is_pointer_button_down_on() {
                p.pressed
            } else {
                p.hover
            },
        );
    }

    let identity = theme::identity_color(item.identity, p.dark);
    let tile = Rect::from_min_size(
        Pos2::new(rect.left() + 10.0, rect.top() + 11.0),
        Vec2::splat(24.0),
    );
    painter.rect_filled(
        tile,
        7,
        theme::tint(identity, if p.dark { 0.2 } else { 0.16 }),
    );
    painter.text(
        tile.center(),
        Align2::CENTER_CENTER,
        super::chrome::initial(item.workspace),
        theme::semibold(11.0),
        if p.dark {
            identity
        } else {
            theme::mix(identity, Color32::BLACK, 0.18)
        },
    );

    if item.unread {
        // The bell's dot, on the workspace that rang it.
        let dot = tile.right_top() + vec2(-1.0, 1.0);
        painter.circle_filled(dot, 5.0, p.elevated);
        if highlighted {
            painter.circle_filled(dot, 5.0, p.hover);
        }
        icons::paint(
            &painter,
            Rect::from_center_size(dot, Vec2::splat(8.4)),
            Icon::CircleFilled,
            p.attention,
        );
    }

    let mut y = rect.top() + 10.0;
    painter.galley(
        Pos2::new(text_left, y),
        headline.clone(),
        Color32::PLACEHOLDER,
    );
    y += headline.size().y;
    if let Some(body) = body {
        y += 2.0;
        painter.galley(Pos2::new(text_left, y), body.clone(), Color32::PLACEHOLDER);
        y += body.size().y;
    }
    painter.galley(Pos2::new(text_left, y + 4.0), source, Color32::PLACEHOLDER);

    if dismissing {
        let surface = control.shrink(1.0);
        let down = dismiss.is_pointer_button_down_on();
        if dismiss.hovered() || down {
            painter.rect_filled(surface, 7, p.pressed);
        }
        if dismiss.has_focus() {
            painter.rect_stroke(
                surface,
                7,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        icons::paint(
            &painter,
            control.shrink(if down { 8.5 } else { 8.0 }),
            Icon::Close,
            if dismiss.hovered() { p.fg } else { p.secondary },
        );
    } else {
        galley_at(
            &painter,
            Pos2::new(rect.right() - 12.0 - time.size().x, middle),
            time,
        );
    }
    dismiss
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Dismiss notification");
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn wrapped(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
    rows: usize,
) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width.max(24.0));
    job.wrap.max_rows = rows;
    painter.layout_job(job)
}

/// The start of `text`, cut on a character boundary.
fn clip(text: &str, max_chars: usize) -> &str {
    text.char_indices()
        .nth(max_chars)
        .map_or(text, |(end, _)| &text[..end])
}

fn relative_time(age: Duration) -> String {
    match age.as_secs() {
        0..60 => "now".into(),
        seconds @ 60..3600 => format!("{}m", seconds / 60),
        seconds @ 3600..86_400 => format!("{}h", seconds / 3600),
        seconds => format!("{}d", seconds / 86_400),
    }
}

/// Room an unread pill takes for `count`.
pub fn pill_width(count: usize) -> f32 {
    match count {
        0..10 => 18.0,
        10..100 => 24.0,
        _ => 31.0,
    }
}

/// An unread count in the attention colour. Workspace rows, folders and the
/// popover share it; it is tinted, like other chips, to stay quiet in chrome.
pub fn pill(painter: &Painter, at: Pos2, anchor: Align2, count: usize, p: Palette) -> Rect {
    let rect = anchor.anchor_size(at, vec2(pill_width(count), 18.0));
    painter.rect_filled(
        rect,
        9,
        theme::tint(p.attention, if p.dark { 0.22 } else { 0.2 }),
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        if count > 99 {
            "99+".to_owned()
        } else {
            count.to_string()
        },
        theme::semibold(10.5),
        if p.dark {
            p.attention
        } else {
            theme::mix(p.attention, Color32::BLACK, 0.38)
        },
    );
    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Modifiers, PointerButton};

    fn context() -> (egui::Context, Palette) {
        let ctx = egui::Context::default();
        crate::theme::fonts(&ctx);
        let config = crate::config::Config::default();
        theme::apply(&ctx, &config);
        (ctx, Palette::for_config(&config))
    }

    fn items(count: u64) -> Vec<NotificationItem<'static>> {
        (1..=count)
            .rev()
            .map(|sequence| NotificationItem {
                sequence,
                pane: PaneId::new(sequence),
                generation: 7,
                identity: 1,
                workspace: "web-app",
                program: "sh".into(),
                title: "Ready for review",
                body: "A process is waiting for your input.",
                unread: true,
                age: Duration::ZERO,
            })
            .collect()
    }

    struct Pass {
        rect: Rect,
        actions: Vec<Action>,
        first_frame: bool,
    }

    fn frame(
        ctx: &egui::Context,
        p: Palette,
        items: &[NotificationItem<'_>],
        size: Vec2,
        events: Vec<Event>,
        first_frame: bool,
    ) -> Pass {
        let mut out = Pass {
            rect: Rect::NOTHING,
            actions: Vec::new(),
            first_frame,
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |_| {
                // As the application does while the popover is open.
                ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
                out.rect = show(
                    ctx,
                    p,
                    NotificationView {
                        items,
                        unavailable: false,
                        first_frame: &mut out.first_frame,
                    },
                    &mut out.actions,
                )
                .rect;
            },
        );
        output.textures_delta.clear();
        out
    }

    fn settle(ctx: &egui::Context, p: Palette, items: &[NotificationItem<'_>], size: Vec2) -> Pass {
        let mut first_frame = true;
        let mut last = None;
        for _ in 0..3 {
            let out = frame(ctx, p, items, size, vec![], first_frame);
            first_frame = out.first_frame;
            last = Some(out);
        }
        last.unwrap()
    }

    #[test]
    fn popover_grows_with_new_entries_and_stays_inside_a_narrow_window() {
        let (ctx, p) = context();
        let empty = settle(&ctx, p, &[], vec2(1180.0, 760.0));
        assert!(!empty.first_frame, "an empty popover still settles");
        let four = items(4);
        let populated = settle(&ctx, p, &four, vec2(1180.0, 760.0));
        assert!(
            !populated.first_frame,
            "the newest alert takes the keyboard"
        );
        assert!(
            populated.rect.height() > empty.rect.height() + 60.0,
            "{:?} -> {:?}",
            empty.rect,
            populated.rect
        );
        let many = items(40);
        for size in [vec2(640.0, 400.0), vec2(300.0, 260.0)] {
            let narrow = settle(&ctx, p, &many, size).rect;
            assert!(
                narrow.left() >= 0.0 && narrow.bottom() <= size.y && narrow.right() <= size.x,
                "{size:?}: {narrow:?}"
            );
        }
    }

    #[test]
    fn a_row_opens_its_captured_pane_and_its_dismiss_control_only_dismisses() {
        let (ctx, p) = context();
        let size = vec2(1180.0, 760.0);
        let two = items(2);
        let popover = settle(&ctx, p, &two, size).rect;
        let click = |pos: Pos2| {
            frame(&ctx, p, &two, size, vec![Event::PointerMoved(pos)], false);
            let mut actions = Vec::new();
            for pressed in [true, false] {
                let event = Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                };
                actions.extend(frame(&ctx, p, &two, size, vec![event], false).actions);
            }
            actions
        };
        // The newest alert is the first row, below the header.
        let first = Pos2::new(popover.center().x, popover.top() + HEADER + 30.0);
        assert!(matches!(
            click(first)[..],
            [Action::OpenNotification(pane, 7)] if pane == PaneId::new(2)
        ));
        let dismiss = Pos2::new(popover.right() - 26.0, popover.top() + HEADER + 24.0);
        assert!(matches!(
            click(dismiss)[..],
            [Action::DismissNotification(2)]
        ));
    }

    #[test]
    fn arrow_keys_walk_the_alerts_and_delete_moves_on_to_the_neighbour() {
        let (ctx, p) = context();
        let size = vec2(1180.0, 760.0);
        let three = items(3);
        assert!(!settle(&ctx, p, &three, size).first_frame);
        let press = |key: Key| {
            let event = Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            };
            frame(&ctx, p, &three, size, vec![event], false).actions
        };
        // Nothing is highlighted until an arrow is pressed.
        assert!(press(Key::Enter).is_empty());
        assert!(press(Key::ArrowDown).is_empty());
        assert!(press(Key::ArrowDown).is_empty());
        assert!(matches!(
            press(Key::Enter)[..],
            [Action::OpenNotification(pane, 7)] if pane == PaneId::new(2)
        ));
        assert!(matches!(
            press(Key::Delete)[..],
            [Action::DismissNotification(2)]
        ));
        assert!(matches!(
            press(Key::Enter)[..],
            [Action::OpenNotification(pane, 7)] if pane == PaneId::new(1)
        ));
        // The highlight stops at the ends of the list.
        assert!(press(Key::ArrowDown).is_empty());
        assert!(matches!(
            press(Key::Enter)[..],
            [Action::OpenNotification(pane, 7)] if pane == PaneId::new(1)
        ));
    }

    #[test]
    fn alert_text_is_clipped_on_character_boundaries_and_ages_read_plainly() {
        assert_eq!(clip("日本語の端末", 3), "日本語");
        assert_eq!(clip("short", 240), "short");
        for (seconds, text) in [(0, "now"), (59, "now"), (60, "1m"), (3599, "59m")] {
            assert_eq!(relative_time(Duration::from_secs(seconds)), text);
        }
        assert_eq!(relative_time(Duration::from_secs(7200)), "2h");
        assert_eq!(relative_time(Duration::from_secs(200_000)), "2d");
        assert!(pill_width(7) < pill_width(42) && pill_width(42) < pill_width(1000));
    }
}
