//! The conversation with a project's lead: its entries as they are read, and
//! the field a message is written in. What the person said is in a filled
//! bubble at the trailing edge, what the lead said is plain text, and what
//! Neptune reports is in an outlined card. Enter sends and Shift+Enter breaks
//! the line; the field asks for the keyboard only when told to, so an input
//! method's composition is never interrupted.
use super::helpers::{elided, field_frame, focus_ring, galley_at, place};
use super::{Action, markup};
use crate::{
    icons::{self, Icon},
    projects::transcript::{Attachment, Ended, Entry, Record, Source},
    theme::{self, Palette},
};
use eframe::egui::{
    self, Align, Color32, CursorIcon, Frame, Id, Key, KeyboardShortcut, Layout, Margin, Modifiers,
    Pos2, Rect, Response, Sense, Stroke, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType,
    text::LayoutJob, vec2,
};
use neptune_model::{PaneId, ProjectId};
use std::collections::VecDeque;

/// Entries shown at first, and added by "Load earlier".
pub const PAGE: usize = 60;
/// How much of a report a folded card reads for the two lines it shows.
const PREVIEW: usize = 800;
const ROW: f32 = 17.0;
/// The share of the chat's width a message of the person's takes at most.
const BUBBLE: f32 = 0.86;

pub fn composer_id() -> Id {
    Id::new("project-composer")
}
pub fn goal_id() -> Id {
    Id::new("project-goal")
}

/// A multi-line field. `rows` is how many lines it shows before it scrolls.
pub struct Editor<'a> {
    pub id: Id,
    pub text: &'a mut String,
    pub hint: &'a str,
    /// Its accessible name.
    pub label: &'a str,
    /// Take the keyboard this frame; cleared once it has.
    pub focus: &'a mut bool,
    /// An input method is composing: Enter belongs to it.
    pub composing: bool,
    /// Room kept clear at the trailing edge for a control drawn over it.
    pub trailing: f32,
    /// Enter breaks the line, as in any text: the field is saved with a
    /// button, not sent.
    pub newline: bool,
}

/// The height a field of `width` takes for `text`, between `rows` lines.
pub fn editor_height(ui: &Ui, text: &str, width: f32, rows: (usize, usize)) -> f32 {
    let galley = ui.painter().layout(
        text.to_owned(),
        theme::regular(13.0),
        Color32::PLACEHOLDER,
        (width - 20.0).max(40.0),
    );
    galley
        .size()
        .y
        .clamp(rows.0 as f32 * ROW, rows.1 as f32 * ROW)
        + 14.0
}

/// A fresh press of Enter without a modifier, taken from this frame's input
/// so the field does not see it.
pub fn take_enter(ui: &Ui) -> bool {
    ui.input_mut(|input| {
        let before = input.events.len();
        input.events.retain(|event| {
            !matches!(
                event,
                egui::Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.shift && !modifiers.alt && !modifiers.ctrl && !modifiers.mac_cmd
            )
        });
        input.events.len() != before
    })
}

/// Draws the field in `rect`. Returns its response and whether Enter asked
/// for its text to be used.
pub fn editor(ui: &mut Ui, p: Palette, rect: Rect, editor: Editor) -> (Response, bool) {
    let Editor {
        id,
        text,
        hint,
        label,
        focus,
        composing,
        trailing,
        newline,
    } = editor;
    let composing = composing
        || ui.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Ime(_)))
        });
    // Before the field runs: it would otherwise do nothing with the key, and
    // the key must not reach anything behind it.
    let entered =
        !newline && ui.memory(|memory| memory.has_focus(id)) && !composing && take_enter(ui);
    let breaks = if newline {
        Modifiers::NONE
    } else {
        Modifiers::SHIFT
    };
    field_frame(ui, p, rect, id, true);
    let inner = Rect::from_min_max(
        rect.min + vec2(10.0, 7.0),
        rect.max - vec2(10.0 + trailing, 7.0),
    );
    let response = place(
        ui,
        inner,
        Layout::top_down(Align::Min),
        ("project-editor", id),
        |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("project-editor-scroll", id))
                .max_height(inner.height())
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .id(id)
                            .hint_text(hint)
                            .font(theme::regular(13.0))
                            .frame(Frame::NONE)
                            .background_color(Color32::TRANSPARENT)
                            .margin(Margin::ZERO)
                            .desired_width(inner.width())
                            .desired_rows(1)
                            .return_key(KeyboardShortcut::new(breaks, Key::Enter)),
                    )
                })
                .inner
        },
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, ui.is_enabled(), label));
    if *focus && ui.is_enabled() && !ui.is_sizing_pass() {
        response.request_focus();
        *focus = false;
    }
    (response, entered && !text.trim().is_empty())
}

/// A compact text button for a row of a list. `name` is its accessible name
/// and must be its own among the buttons in view.
pub fn row_button(ui: &mut Ui, p: Palette, rect: Rect, label: &str, name: &str) -> Response {
    row_button_of(ui, p, rect, (label, name), None)
}
/// The same in the accent, for the one action a row recommends. One that is
/// not `enabled` is drawn as a plain button that takes no press.
pub fn row_button_primary(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    label: &str,
    name: &str,
    enabled: bool,
) -> Response {
    row_button_of(ui, p, rect, (label, name), Some(enabled))
}
/// `primary` is whether an accent button can be pressed; a plain one has none.
fn row_button_of(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    (label, name): (&str, &str),
    primary: Option<bool>,
) -> Response {
    let live = primary != Some(false);
    let accent = primary == Some(true);
    // Named for what it does, not for where it is laid out: rows above it
    // come and go while it is pressed.
    let response = ui.interact(
        rect,
        Id::new(("project-row-button", name)),
        if live { Sense::click() } else { Sense::hover() },
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, live && ui.is_enabled(), name));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 6, if accent { p.accent } else { p.control });
        // The overlays of a filled button, as `controls::button` has them.
        let (pressed, hover) = if accent {
            (Color32::from_black_alpha(46), Color32::from_white_alpha(26))
        } else {
            (p.pressed, p.hover)
        };
        if live && response.is_pointer_button_down_on() {
            painter.rect_filled(rect, 6, pressed);
        } else if live && response.hovered() {
            painter.rect_filled(rect, 6, hover);
        }
        if response.has_focus() {
            focus_ring(painter, rect, 6, p);
        }
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            theme::medium(11.5),
            if accent {
                p.on_accent
            } else if live {
                p.fg
            } else {
                p.muted
            },
        );
    }
    if live {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}
/// The width `row_button` needs for `label`.
pub fn row_button_width(ui: &Ui, label: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(label.to_owned(), theme::medium(11.5), Color32::PLACEHOLDER)
        .size()
        .x
        + 16.0
}

pub struct Chat<'a> {
    pub project: ProjectId,
    pub records: &'a VecDeque<Record>,
    /// How many of the newest entries are laid out.
    pub shown: usize,
    /// The project's agents that still have a terminal, with its generation.
    pub members: &'a [(PaneId, u64)],
    /// Those at rest whose pull request has failing checks, or review
    /// comments nobody resolved: the newest row of each offers to have it
    /// told so.
    pub follow: &'a [(PaneId, bool, bool)],
    /// The lead's turn has begun and nothing of it has come yet.
    pub thinking: bool,
    /// What the lead is doing that Neptune has not carried out yet.
    pub pending: &'a [String],
    /// The block the lead is writing.
    pub streaming: Option<&'a str>,
    /// A line under the last entry, such as a retry under way.
    pub status: Option<&'a str>,
    /// The project's folder holds entries older than `records`.
    pub more: bool,
    /// The watches the lead proposed that wait for the person's yes or no.
    pub proposed: &'a [u64],
    /// The watches of pull requests an agent at rest owns, with whether its
    /// checks fail and whether it has comments nobody resolved: the newest
    /// row of each offers what that agent's own row does.
    pub pulls: &'a [(u64, PaneId, bool, bool)],
}
/// How many of `records` are shown as entries of the chat.
pub fn readable(records: &VecDeque<Record>) -> usize {
    records.iter().filter(|record| read(&record.entry)).count()
}

fn read(entry: &Entry) -> bool {
    matches!(
        entry,
        Entry::User { .. }
            | Entry::Lead { .. }
            | Entry::Tool { .. }
            | Entry::Event { .. }
            | Entry::Proposal { .. }
            | Entry::Notice { .. }
    )
}

/// The room above a run of words, which says whose they are to assistive
/// technology alone: who speaks is seen from how the words are set.
fn said(ui: &mut Ui, who: &str, salt: impl std::hash::Hash + std::fmt::Debug) {
    let (_, gap) = ui.allocate_space(vec2(ui.available_width(), 10.0));
    ui.interact(gap, Id::new(("project-said", salt)), Sense::hover())
        .widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, who));
}

/// What the person said: a bubble at the trailing edge, no wider than its
/// words need.
fn bubble(ui: &mut Ui, p: Palette, text: &str) {
    let room = (ui.available_width() * BUBBLE - 20.0).max(40.0);
    let width = markup::width(ui, text, markup::Lines::Kept, room);
    let mut rect = ui.available_rect_before_wrap();
    rect.min.x = rect.max.x - (width + 20.0);
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            Frame::new()
                .fill(p.control)
                .corner_radius(theme::metrics::ROW_RADIUS)
                .inner_margin(Margin::symmetric(10, 7))
                .show(ui, |ui| {
                    ui.set_width(width);
                    markup::show(ui, p, text, markup::Lines::Kept);
                });
        },
    );
}

/// A file of a message: its name on one line, as tall as this,
pub const CHIP: f32 = 22.0;
/// with this much room to the next.
const CHIP_GAP: f32 = 4.0;
/// The narrowest a chip is before one fewer stands in a row, and the widest.
const CHIP_WIDTH: (f32, f32) = (120.0, 180.0);

/// How the chips of `count` files are set in `width`: how many stand in a
/// row, how wide each is, and how tall they are together.
pub fn chip_grid(width: f32, count: usize) -> (usize, f32, f32) {
    if count == 0 {
        return (1, 0.0, 0.0);
    }
    let columns = (((width + CHIP_GAP) / (CHIP_WIDTH.0 + CHIP_GAP)) as usize).clamp(1, count);
    let each = ((width - CHIP_GAP * (columns - 1) as f32) / columns as f32).min(CHIP_WIDTH.1);
    let rows = count.div_ceil(columns) as f32;
    (
        columns,
        each.max(0.0),
        rows * CHIP + (rows - 1.0) * CHIP_GAP,
    )
}

/// What was pressed on the chips of a message's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    /// A file, to be opened.
    Open(usize),
    /// The control that takes a file off a message not yet sent.
    Remove(usize),
}

/// The files of a message as chips in `rect`: what kind each is and its
/// name, cut where it does not fit. They fill from the leading edge, or
/// stand against the trailing one under the person's own words. A file of a
/// message being written has a control that takes it off. `salt` names the
/// message among those in view.
pub fn chips(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    files: &[Attachment],
    (trailing, removable): (bool, bool),
) -> Option<Chip> {
    let (columns, each, _) = chip_grid(rect.width(), files.len());
    let mut pressed = None;
    for (index, file) in files.iter().enumerate() {
        let (row, column) = (index / columns, index % columns);
        let step = each + CHIP_GAP;
        let left = if trailing {
            let in_row = (files.len() - row * columns).min(columns);
            rect.right() - step * (in_row - column) as f32 + CHIP_GAP
        } else {
            rect.left() + step * column as f32
        };
        let chip = Rect::from_min_size(
            Pos2::new(left, rect.top() + (CHIP + CHIP_GAP) * row as f32),
            vec2(each, CHIP),
        );
        let name = file.name();
        let body = ui.interact(chip, Id::new(("project-file", salt, index)), Sense::click());
        body.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, true, format!("Open attached {name}"))
        });
        let cross = Rect::from_center_size(
            Pos2::new(chip.right() - 11.0, chip.center().y),
            Vec2::splat(16.0),
        );
        // Over the chip, so that a press on it is its own.
        let remove = removable.then(|| {
            let remove = ui.interact(
                cross,
                Id::new(("project-file-remove", salt, index)),
                Sense::click(),
            );
            remove.widget_info(|| {
                WidgetInfo::labeled(WidgetType::Button, true, format!("Remove {name}"))
            });
            remove
        });
        if ui.is_rect_visible(chip) {
            let painter = ui.painter();
            let over = remove.as_ref().is_some_and(Response::hovered);
            if body.is_pointer_button_down_on() {
                painter.rect_filled(chip, 6, p.pressed);
            } else if body.hovered() && !over {
                painter.rect_filled(chip, 6, p.hover);
            }
            painter.rect_stroke(
                chip,
                6,
                Stroke::new(1.0, p.border),
                egui::StrokeKind::Inside,
            );
            if body.has_focus() {
                focus_ring(painter, chip, 6, p);
            }
            icons::paint(
                painter,
                Rect::from_center_size(
                    Pos2::new(chip.left() + 12.0, chip.center().y),
                    Vec2::splat(12.0),
                ),
                if file.picture() {
                    Icon::Image
                } else {
                    Icon::File
                },
                p.muted,
            );
            let room = each - 22.0 - if removable { 22.0 } else { 8.0 };
            galley_at(
                painter,
                Pos2::new(chip.left() + 22.0, chip.center().y),
                elided(painter, name, theme::regular(12.0), p.fg, room),
            );
            if let Some(remove) = &remove {
                if remove.is_pointer_button_down_on() {
                    painter.rect_filled(cross, 5, p.pressed);
                } else if remove.hovered() {
                    painter.rect_filled(cross, 5, p.hover);
                }
                if remove.has_focus() {
                    focus_ring(painter, cross, 5, p);
                }
                icons::paint(
                    painter,
                    Rect::from_center_size(cross.center(), Vec2::splat(10.0)),
                    Icon::Close,
                    if over { p.fg } else { p.muted },
                );
            }
        }
        if let Some(remove) = remove
            && remove
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("Remove")
                .clicked()
        {
            pressed = Some(Chip::Remove(index));
        } else if body
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("{} · {}", file.path, file.size()))
            .clicked()
        {
            pressed = Some(Chip::Open(index));
        }
    }
    pressed
}

/// The control beside the one that sends: it asks for files to go with the
/// message.
pub fn attach_button(ui: &mut Ui, p: Palette, rect: Rect) -> Response {
    let response = ui.interact(rect, ui.id().with("project-attach"), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Attach files"));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.is_pointer_button_down_on() {
            painter.rect_filled(rect, 7, p.pressed);
        } else if response.hovered() {
            painter.rect_filled(rect, 7, p.hover);
        }
        if response.has_focus() {
            focus_ring(painter, rect, 7, p);
        }
        icons::paint(
            painter,
            Rect::from_center_size(rect.center(), Vec2::splat(14.0)),
            Icon::Paperclip,
            if response.hovered() || response.has_focus() {
                p.fg
            } else {
                p.muted
            },
        );
    }
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Attach files. Pictures can also be pasted or dropped here.")
}

/// The control that copies a run of the lead's words, at the trailing edge
/// of `tail`, the run's last line; on a row of its own where that line
/// leaves it no room. `seq` is the run's first entry, which names it.
fn copy(
    ui: &mut Ui,
    p: Palette,
    seq: u64,
    tail: Rect,
    text: impl FnOnce() -> String,
    actions: &mut Vec<Action>,
) {
    let right = ui.max_rect().right();
    let centre = if tail.is_positive() && tail.right() + 6.0 <= right - 18.0 {
        Pos2::new(right - 9.0, tail.center().y)
    } else {
        let (_, row) = ui.allocate_space(vec2(ui.available_width(), 18.0));
        Pos2::new(row.right() - 9.0, row.center().y)
    };
    let rect = Rect::from_center_size(centre, Vec2::splat(18.0));
    let response = ui.interact(rect, Id::new(("project-copy", seq)), Sense::click());
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!("Copy the lead's reply, entry {seq}"),
        )
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.is_pointer_button_down_on() {
            painter.rect_filled(rect, 5, p.pressed);
        } else if response.hovered() {
            painter.rect_filled(rect, 5, p.hover);
        }
        if response.has_focus() {
            focus_ring(painter, rect, 5, p);
        }
        icons::paint(
            painter,
            Rect::from_center_size(rect.center(), Vec2::splat(12.0)),
            Icon::Copy,
            if response.hovered() || response.has_focus() {
                p.fg
            } else {
                p.muted
            },
        );
    }
    if response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Copy reply")
        .clicked()
    {
        actions.push(Action::Project(super::project::Event::Copy(text())));
    }
}

/// A line led by a small icon: something Neptune did, or says. Returns
/// where its last line is.
fn line(ui: &mut Ui, icon: Option<(Icon, Color32)>, text: &str, size: f32, ink: Color32) -> Rect {
    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if let Some((icon, color)) = icon {
            let (_, mark) = ui.allocate_space(vec2(14.0, ROW));
            icons::paint(
                ui.painter(),
                Rect::from_center_size(mark.center(), Vec2::splat(12.0)),
                icon,
                color,
            );
        }
        markup::wrapped(
            ui,
            LayoutJob::simple(text.to_owned(), theme::regular(size), ink, 0.0),
        )
    })
    .inner
}

/// The first words of a report, without the marks that set them: what a
/// folded card shows.
fn preview(text: &str) -> String {
    let text = text.trim();
    let start = crate::projects::transcript::clip(text, PREVIEW);
    // A word or a mark the cut went through says nothing.
    let whole = match start.rfind(char::is_whitespace) {
        Some(end) if start.len() < text.len() => &start[..end],
        _ => start,
    };
    markup::plain(whole)
}

/// `words` in at most two lines of `width`, ended with "…" after a whole
/// word where they do not fit, and whether they did not.
fn two_lines(
    ui: &Ui,
    words: &str,
    width: f32,
    ink: Color32,
) -> (std::sync::Arc<egui::Galley>, bool) {
    let lay = |words: String| {
        let mut job = LayoutJob::simple(words, theme::regular(12.0), ink, width);
        job.wrap.max_rows = 2;
        ui.painter().layout_job(job)
    };
    let galley = lay(words.to_owned());
    if !galley.elided {
        return (galley, false);
    }
    // What the two lines hold, less the mark that ends them and the word
    // they stop in.
    let held: usize = galley.rows.iter().map(|row| row.row.glyphs.len()).sum();
    let end = words
        .char_indices()
        .nth(held.saturating_sub(2))
        .map_or(words.len(), |(end, _)| end);
    let kept = &words[..end];
    let kept = match kept.rfind(char::is_whitespace) {
        Some(end) if end > kept.len() / 2 => &kept[..end],
        _ => kept,
    };
    let kept = kept.trim_end_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation());
    (lay(format!("{kept}…")), true)
}

/// What a card carried.
enum Carried<'a> {
    /// What the person is asked to allow: every word, as written.
    Whole(&'a str),
    /// A report. The card is an event, not a message: it shows two lines
    /// of the report's words, and all of it once it is unfolded, `marked`
    /// up as a reply is or as written. `fold` names its fold and `seq` is
    /// the entry it belongs to.
    Report {
        text: &'a str,
        marked: bool,
        fold: Id,
        seq: u64,
    },
}

/// Something that happened, set apart from what was said: a headline, two
/// lines of what it carried, and what `more` puts under them. It is
/// outlined, not filled: that tells what Neptune reports from what the
/// person said. The headline unfolds the report, which is written as a
/// reply is, so its lists, tables and code are laid out as the lead's are.
fn card(
    ui: &mut Ui,
    p: Palette,
    headline: &str,
    (text, fold, seq): (&str, Id, u64),
    actions: &mut Vec<Action>,
    more: impl FnOnce(&mut Ui, &mut Vec<Action>),
) {
    let carried = Carried::Report {
        text,
        marked: true,
        fold,
        seq,
    };
    card_of(ui, p, (headline, None), carried, actions, more);
}
/// `second` is a line of its own under the headline.
fn card_of(
    ui: &mut Ui,
    p: Palette,
    (headline, second): (&str, Option<&str>),
    carried: Carried,
    actions: &mut Vec<Action>,
    more: impl FnOnce(&mut Ui, &mut Vec<Action>),
) {
    ui.add_space(8.0);
    Frame::new()
        .stroke(p.hairline())
        .corner_radius(theme::metrics::ROW_RADIUS)
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // A report that two lines hold has nothing to unfold.
            let folded = match &carried {
                Carried::Report {
                    text, fold, seq, ..
                } if !text.trim().is_empty() => {
                    let text = text.trim();
                    let (lines, cut) =
                        two_lines(ui, &preview(text), ui.available_width(), p.secondary);
                    let folds = cut || text.len() > PREVIEW || text.contains('\n');
                    Some((lines, folds.then_some((*fold, *seq))))
                }
                _ => None,
            };
            let fold = folded.as_ref().and_then(|(_, fold)| *fold);
            let mut unfolded =
                fold.is_some_and(|(fold, _)| ui.data(|data| data.get_temp(fold)) == Some(true));
            // Its lines stand close together, as one thing that is said;
            // an unfolded report keeps the room a reply's blocks have.
            let apart = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 3.0);
            let row = ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                // The mark the rows of what Neptune did are led by.
                let mark = fold.map(|_| ui.allocate_space(vec2(14.0, ROW)).1);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(headline)
                            .font(theme::medium(12.0))
                            .color(p.fg),
                    )
                    .wrap()
                    .selectable(fold.is_none()),
                );
                mark
            });
            if let (Some((fold, seq)), Some(mark)) = (fold, row.inner) {
                // The whole line is pressed, not the mark alone.
                let line = row.response.rect.with_max_x(ui.max_rect().right());
                let response = ui.interact(line, fold, Sense::click());
                if response.clicked() {
                    unfolded = !unfolded;
                    ui.data_mut(|data| data.insert_temp(fold, unfolded));
                }
                let (name, tip) = if unfolded {
                    (format!("Show less, entry {seq}"), "Show less")
                } else {
                    (format!("Show more, entry {seq}"), "Show all of it")
                };
                response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
                if response.has_focus() {
                    focus_ring(ui.painter(), line.expand2(vec2(3.0, 1.0)), 5, p);
                }
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(mark.center(), Vec2::splat(12.0)),
                    if unfolded {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    if response.hovered() || response.has_focus() {
                        p.fg
                    } else {
                        p.muted
                    },
                );
                response
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(tip);
            }
            if let Some(second) = second {
                ui.add_space(1.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(second)
                            .font(theme::regular(12.0))
                            .color(p.fg),
                    )
                    .wrap(),
                );
            }
            let written = |ui: &mut Ui, text: &str| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(text)
                            .font(theme::regular(12.0))
                            .color(p.secondary),
                    )
                    .wrap(),
                );
            };
            match (carried, folded) {
                (
                    Carried::Report {
                        text, marked, fold, ..
                    },
                    Some(_),
                ) if unfolded => {
                    ui.spacing_mut().item_spacing.y = apart;
                    ui.add_space(3.0);
                    if marked {
                        // Named, so that its links keep theirs while
                        // entries come and go above it.
                        ui.push_id(fold.with("report"), |ui| {
                            let (lines, ink) = (markup::Lines::Kept, p.secondary);
                            markup::show_in(ui, p, text.trim(), lines, ink, actions);
                        });
                    } else {
                        written(ui, text.trim());
                    }
                }
                (_, Some((lines, _))) => {
                    ui.add(egui::Label::new(lines));
                }
                (Carried::Whole(text), _) if !text.trim().is_empty() => {
                    ui.add_space(3.0);
                    written(ui, text.trim());
                }
                _ => {}
            }
            ui.spacing_mut().item_spacing.y = apart;
            more(ui, actions);
        });
}

/// A turn the lead finished with more than its reply. What it said and what
/// Neptune did for it on the way are its work, folded behind one row; the
/// reply is read without them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Work {
    /// The entry the turn began with, which names it, and the one that
    /// ended it.
    turn: u64,
    end: u64,
    /// The lead's last words of the turn: its reply.
    reply: u64,
    /// How long the turn took, in seconds.
    took: u64,
}
impl Work {
    /// `record` is folded away with the work. What Neptune refused stays in
    /// view, as does everything that is not the lead's own.
    fn folds(&self, record: &Record) -> bool {
        (self.turn..self.end).contains(&record.seq)
            && match &record.entry {
                Entry::Lead { .. } => record.seq != self.reply,
                Entry::Tool { ok, .. } => *ok,
                _ => false,
            }
    }
}

/// The turns of `records` that have work to fold, oldest first. A turn that
/// still runs, one that was stopped or failed, and one whose beginning is
/// not held have no reply to set apart: they are read in full.
fn works(records: &VecDeque<Record>) -> Vec<Work> {
    let mut works = Vec::new();
    // The turn the lead is in: where it began, its last words so far, and
    // how many entries of it would fold or be its reply.
    let mut open = None::<(&Record, Option<u64>, usize)>;
    for record in records {
        match (&record.entry, &mut open) {
            (Entry::Turn { .. }, _) => open = Some((record, None, 0)),
            (Entry::Lead { .. }, Some((_, reply, parts))) => {
                *reply = Some(record.seq);
                *parts += 1;
            }
            (Entry::Tool { ok: true, .. }, Some((_, _, parts))) => *parts += 1,
            (Entry::End { turn, outcome, .. }, _) => {
                if let Some((began, Some(reply), parts)) = open.take()
                    && matches!(&began.entry, Entry::Turn { id, .. } if id == turn)
                    && *outcome == Ended::Completed
                    && parts > 1
                {
                    works.push(Work {
                        turn: began.seq,
                        end: record.seq,
                        reply,
                        took: record.at.saturating_sub(began.at),
                    });
                }
            }
            _ => {}
        }
    }
    works
}

/// How long a turn took, in the two largest units it has: "8s", "1m 12s",
/// "1h 4m".
fn lasted(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// The row that stands for a turn's work: how long the turn took and a
/// chevron, over a hairline. The whole row is pressed, and unfolds the work
/// in place under it. `fold` names its fold, as a report's does. Returns
/// whether it is unfolded.
fn worked(ui: &mut Ui, p: Palette, fold: Id, work: Work) -> bool {
    let mut unfolded = ui.data(|data| data.get_temp(fold)) == Some(true);
    let (_, row) = ui.allocate_space(vec2(ui.available_width(), ROW));
    let response = ui.interact(row, fold, Sense::click());
    if response.clicked() {
        unfolded = !unfolded;
        ui.data_mut(|data| data.insert_temp(fold, unfolded));
    }
    let (name, tip) = if unfolded {
        (format!("Hide work, turn {}", work.turn), "Hide the work")
    } else {
        (format!("Show work, turn {}", work.turn), "Show the work")
    };
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
    if ui.is_rect_visible(row) {
        let painter = ui.painter();
        if response.has_focus() {
            focus_ring(painter, row.expand2(vec2(3.0, 1.0)), 5, p);
        }
        let ink = if response.hovered() || response.has_focus() {
            p.fg
        } else {
            p.muted
        };
        let words = format!("Worked for {}", lasted(work.took));
        let words = painter.layout_no_wrap(words, theme::regular(11.5), ink);
        let words = galley_at(painter, row.left_center(), words);
        icons::paint(
            painter,
            Rect::from_center_size(
                Pos2::new(words.right() + 10.0, row.center().y),
                Vec2::splat(12.0),
            ),
            if unfolded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            },
            ink,
        );
    }
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tip);
    let (_, rule) = ui.allocate_space(vec2(ui.available_width(), 1.0));
    ui.painter()
        .line_segment([rule.left_center(), rule.right_center()], p.hairline());
    unfolded
}

/// The entries, oldest first, in the width `ui` offers. Returns whether
/// earlier ones were asked for.
pub fn transcript(ui: &mut Ui, p: Palette, chat: &Chat, actions: &mut Vec<Action>) -> bool {
    let mut earlier = false;
    let total = chat
        .records
        .iter()
        .filter(|record| read(&record.entry))
        .count();
    let hidden = total.saturating_sub(chat.shown);
    if hidden > 0 || chat.more {
        ui.add_space(6.0);
        let width = row_button_width(ui, "Load earlier");
        let (_, rect) = ui.allocate_space(vec2(width, 22.0));
        earlier = row_button(ui, p, rect, "Load earlier", "Load earlier messages").clicked();
    }
    if total == 0 && chat.streaming.is_none() && chat.pending.is_empty() {
        ui.add_space(14.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new("Tell the lead what you want done.")
                    .font(theme::regular(12.0))
                    .color(p.muted),
            )
            .wrap()
            .selectable(false),
        );
    }
    // Whose words the entry above was, so each run is set apart only once.
    let mut last = None::<bool>;
    // The first entry of the run the lead is in, for the control that
    // copies it, and where the last line laid out for it is.
    let mut run = None::<(u64, usize)>;
    let mut tail = Rect::NOTHING;
    // A run its lead is still adding to is copied once it is whole.
    let writing = chat.streaming.is_some() || chat.thinking || !chat.pending.is_empty();
    let rows: Vec<&Record> = chat
        .records
        .iter()
        .filter(|record| read(&record.entry))
        .skip(hidden)
        .collect();
    // The newest row about each agent: what is offered for its pull
    // request is offered there, once.
    let newest = |agent: u64| {
        rows.iter()
            .rev()
            .find(|record| {
                matches!(
                    &record.entry,
                    Entry::Event { source: Source::Agent, agent: Some(about), .. } if *about == agent
                )
            })
            .map(|record| record.seq)
    };
    // The newest row about each watched pull request, likewise.
    let newest_pull = |watch: u64| {
        rows.iter()
            .rev()
            .find(|record| {
                matches!(
                    &record.entry,
                    Entry::Event { source: Source::Pr, agent: Some(about), .. } if *about == watch
                )
            })
            .map(|record| record.seq)
    };
    let works = works(chat.records);
    // The turn that folds which an entry of the lead's was written in.
    let work_of = |record: &Record| {
        let own = matches!(record.entry, Entry::Lead { .. } | Entry::Tool { .. });
        let at = works.partition_point(|work| work.end < record.seq);
        works
            .get(at)
            .copied()
            .filter(|work| own && work.turn < record.seq)
    };
    // The turn whose entries are being laid out, and whether its work is.
    let mut turn = None::<(u64, bool)>;
    for (index, record) in rows.iter().enumerate() {
        let work = work_of(record);
        if let Some(work) = work {
            if turn.map(|(turn, _)| turn) != Some(work.turn) {
                // Its row stands where the turn's first entry in view is,
                // before anything of it that stays in view. Work that is
                // all further up than what is laid out has no row here.
                let hides = rows[index..]
                    .iter()
                    .take_while(|next| next.seq < work.end)
                    .any(|next| work.folds(next));
                let unfolded = !hides || {
                    if last != Some(false) {
                        said(ui, "Lead said", work.turn);
                    } else {
                        ui.add_space(6.0);
                    }
                    last = Some(false);
                    let fold = Id::new(("project-work", chat.project, work.turn));
                    worked(ui, p, fold, work)
                };
                turn = Some((work.turn, unfolded));
            }
            if work.folds(record) && turn != Some((work.turn, true)) {
                continue;
            }
        }
        match &record.entry {
            Entry::User { text, attachments } => {
                if last != Some(true) {
                    said(ui, "You said", record.seq);
                } else {
                    ui.add_space(4.0);
                }
                last = Some(true);
                // Files alone are a message without a bubble.
                let words = !text.trim().is_empty();
                if words {
                    bubble(ui, p, text);
                }
                if !attachments.is_empty() {
                    if words {
                        ui.add_space(4.0);
                    }
                    let width = (ui.available_width() * BUBBLE).max(40.0);
                    let (_, _, height) = chip_grid(width, attachments.len());
                    let (_, mut row) = ui.allocate_space(vec2(ui.available_width(), height));
                    row.min.x = row.max.x - width;
                    // A file that has gone since still has its chip; opening
                    // it says that it is not there.
                    if let Some(Chip::Open(file)) =
                        chips(ui, p, row, record.seq, attachments, (true, false))
                    {
                        actions.push(Action::Explorer(super::explorer::Event::Open(
                            attachments[file].path.clone().into(),
                        )));
                    }
                }
            }
            Entry::Lead { text } => {
                if last != Some(false) {
                    said(ui, "Lead said", record.seq);
                    tail = Rect::NOTHING;
                } else {
                    ui.add_space(6.0);
                }
                // A turn that folds is copied by its reply, not as a run.
                if work.is_none() && run.is_none() {
                    run = Some((record.seq, index));
                }
                last = Some(false);
                // A `ui` of its own, so that its links keep their names.
                let shown = ui.push_id(("project-reply", record.seq), |ui| {
                    markup::show_in(ui, p, text, markup::Lines::Kept, p.fg, actions)
                });
                if let Some(line) = shown.inner {
                    tail = line;
                }
                // The reply is what the person asked for: it alone is
                // copied, whether the work before it is in view or not.
                if work.is_some_and(|work| work.reply == record.seq) {
                    let reply = || text.trim().to_owned();
                    copy(
                        ui,
                        p,
                        record.seq,
                        shown.inner.unwrap_or(Rect::NOTHING),
                        reply,
                        actions,
                    );
                }
            }
            Entry::Tool { summary, ok, .. } => {
                // What Neptune did sits with the lead's words that asked,
                // and recedes behind them unless it failed.
                tail = if *ok {
                    let icon = (Icon::ChevronRight, p.muted);
                    line(ui, Some(icon), summary, 11.5, p.muted)
                } else {
                    let icon = (Icon::Warning, p.red);
                    line(ui, Some(icon), summary, 11.5, p.secondary)
                };
            }
            Entry::Event {
                source,
                agent,
                what,
                text,
            } => {
                last = None;
                // What a watch says is numbered by the watch, not an agent.
                let about = agent.filter(|_| matches!(source, Source::Agent | Source::Worktree));
                let open = about.and_then(|agent| {
                    chat.members
                        .iter()
                        .find(|(pane, _)| pane.get() == agent)
                        .copied()
                });
                let headline = match about.filter(|_| *source == Source::Agent) {
                    Some(agent) => format!("Agent {agent} {what}"),
                    None => what.clone(),
                };
                let seq = record.seq;
                // What its pull request needs, on the newest row about it.
                let (checks, comments) = open
                    .filter(|(pane, _)| newest(pane.get()) == Some(seq))
                    .and_then(|(pane, _)| chat.follow.iter().find(|follow| follow.0 == pane))
                    .map_or((false, false), |follow| (follow.1, follow.2));
                // A pull request's own row offers the same for the agent
                // that owns it, where that agent's row has scrolled away.
                let pull = agent
                    .filter(|watch| *source == Source::Pr && newest_pull(*watch) == Some(seq))
                    .and_then(|watch| chat.pulls.iter().find(|pull| pull.0 == watch))
                    .and_then(|&(watch, pane, checks, comments)| {
                        let (_, generation) = chat.members.iter().find(|open| open.0 == pane)?;
                        Some((watch, pane, *generation, checks, comments))
                    });
                let fold = Id::new(("project-report", chat.project, seq));
                card(
                    ui,
                    p,
                    &headline,
                    (text, fold, seq),
                    actions,
                    |ui, actions| {
                        if open.is_none() && pull.is_none() {
                            return;
                        }
                        use super::project::{Event, FollowUp};
                        ui.add_space(6.0);
                        // They wrap in a narrow panel, never run out of it.
                        ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                        let button = |ui: &mut Ui, label: &str, name: String| {
                            let width = row_button_width(ui, label);
                            let (_, rect) = ui.allocate_space(vec2(width, 22.0));
                            row_button(ui, p, rect, label, &name).clicked()
                        };
                        // `of` tells the buttons of a pull request's row
                        // from those of its agent's row.
                        let follow_ups = |ui: &mut Ui,
                                          actions: &mut Vec<Action>,
                                          (pane, generation),
                                          (checks, comments),
                                          of: &str| {
                            for (offered, label, name, ask) in [
                                (
                                    checks,
                                    "Fix CI",
                                    format!("Ask agent {pane} to fix its failing checks{of}"),
                                    FollowUp::Checks,
                                ),
                                (
                                    comments,
                                    "Address comments",
                                    format!("Ask agent {pane} to address its review comments{of}"),
                                    FollowUp::Comments,
                                ),
                            ] {
                                if offered && button(ui, label, name) {
                                    actions.push(Action::Project(Event::FollowUp {
                                        project: chat.project,
                                        pane,
                                        generation,
                                        ask,
                                    }));
                                }
                            }
                        };
                        if let Some((watch, pane, generation, checks, comments)) = pull {
                            let of = format!(", watch {watch}");
                            follow_ups(ui, actions, (pane, generation), (checks, comments), &of);
                        }
                        let Some((pane, generation)) = open else {
                            return;
                        };
                        if button(
                            ui,
                            "Open terminal",
                            format!("Open terminal of agent {pane}, entry {seq}"),
                        ) {
                            actions.push(Action::OpenAgent(pane, generation));
                        }
                        if button(
                            ui,
                            "Changes",
                            format!("Show changes of agent {pane}, entry {seq}"),
                        ) {
                            actions.push(Action::Project(Event::Changes(pane, generation)));
                        }
                        follow_ups(ui, actions, (pane, generation), (checks, comments), "");
                    });
                    },
                );
            }
            Entry::Proposal { watch, what, text } => {
                last = None;
                // It runs once the person says so, here or among the watches.
                let waits = chat.proposed.contains(watch);
                // What the person is asked to allow is shown whole: a yes
                // covers every word of it.
                let carried = if waits {
                    Carried::Whole(text)
                } else {
                    Carried::Report {
                        text,
                        marked: false,
                        fold: Id::new(("project-report", chat.project, record.seq)),
                        seq: record.seq,
                    }
                };
                // Which watch and how often has a line of its own.
                let headline = what
                    .split_once(": ")
                    .map_or((what.as_str(), None), |(headline, which)| {
                        (headline, Some(which))
                    });
                card_of(ui, p, headline, carried, actions, |ui, actions| {
                    if !waits {
                        return;
                    }
                    use super::project::{Event, Segment};
                    ui.add_space(6.0);
                    // They wrap in a narrow panel, never run out of it.
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                        let place = |ui: &mut Ui, label: &str| {
                            let width = row_button_width(ui, label);
                            ui.allocate_space(vec2(width, 22.0)).1
                        };
                        let rect = place(ui, "Allow");
                        let name = format!("Allow watch {watch}");
                        if row_button_primary(ui, p, rect, "Allow", &name, true).clicked() {
                            actions.push(Action::Project(Event::AllowWatch(
                                chat.project,
                                *watch,
                                true,
                            )));
                        }
                        let rect = place(ui, "Decline");
                        let name = format!("Decline watch {watch}");
                        if row_button(ui, p, rect, "Decline", &name).clicked() {
                            actions.push(Action::Project(Event::AllowWatch(
                                chat.project,
                                *watch,
                                false,
                            )));
                        }
                        // Where it is listed with the others, to look at
                        // before the answer.
                        let rect = place(ui, "Watches");
                        let name = format!("Show the watches, watch {watch}");
                        if row_button(ui, p, rect, "Watches", &name).clicked() {
                            actions.push(Action::Project(Event::Show(Segment::Watches)));
                        }
                    });
                });
            }
            Entry::Notice { text } => {
                last = None;
                line(ui, None, text, 12.0, p.muted);
            }
            _ => {}
        }
        // The run ends where someone else speaks, where a turn that folds
        // begins, or with the entries.
        let runs = |record: &Record| {
            matches!(record.entry, Entry::Lead { .. } | Entry::Tool { .. })
                && work_of(record).is_none()
        };
        let goes_on = rows.get(index + 1).is_some_and(|next| runs(next));
        let ends = runs(record) && !goes_on;
        if let Some((seq, first)) = run.filter(|_| ends)
            && !(writing && index + 1 == rows.len())
        {
            // Everything it said in the run, with what Neptune did for it
            // left out.
            let words = || {
                rows[first..=index]
                    .iter()
                    .filter_map(|record| match &record.entry {
                        Entry::Lead { text } => Some(text.trim()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            copy(ui, p, seq, tail, words, actions);
        }
        if ends {
            run = None;
        }
    }
    if let Some(text) = chat.streaming {
        if last != Some(false) {
            said(ui, "Lead said", "streaming");
        } else {
            ui.add_space(6.0);
        }
        ui.push_id("project-streaming", |ui| {
            markup::show_in(ui, p, text, markup::Lines::Kept, p.fg, actions);
        });
    } else if chat.thinking {
        // Its turn has begun: the wait is the lead's, not Neptune's.
        line(ui, None, "Thinking…", 12.0, p.muted);
    }
    for pending in chat.pending {
        line(ui, None, pending, 12.0, p.muted);
    }
    if let Some(status) = chat.status {
        line(ui, None, status, 12.0, p.muted);
    }
    ui.add_space(8.0);
    earlier
}

/// The control at the composer's trailing edge: it sends, or stops the turn
/// that is running.
pub fn send_button(ui: &mut Ui, p: Palette, rect: Rect, stop: bool, enabled: bool) -> Response {
    send_button_named(
        ui,
        p,
        rect,
        (stop, enabled),
        "Send message",
        "Send · Enter. Shift+Enter starts a new line.",
    )
}
/// The same control in a field that does something else with Enter, such as
/// starting a project: `name` is its accessible name and `tip` what it shows
/// under the pointer while it does not stop a turn.
pub fn send_button_named(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    (stop, enabled): (bool, bool),
    name: &str,
    tip: &str,
) -> Response {
    let (name, tip) = if stop {
        ("Stop the lead", "Stop the lead")
    } else {
        (name, tip)
    };
    let response = ui.interact(
        rect,
        ui.id().with("project-send"),
        if enabled || stop {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled || stop, name));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let live = enabled || stop;
        let fill = if stop {
            p.control
        } else if enabled {
            p.accent
        } else {
            p.control
        };
        painter.rect_filled(rect, 7, fill);
        if live && response.is_pointer_button_down_on() {
            painter.rect_filled(rect, 7, p.pressed);
        } else if live && response.hovered() {
            painter.rect_filled(rect, 7, p.hover);
        }
        if response.has_focus() {
            focus_ring(painter, rect, 7, p);
        }
        let ink = if stop {
            p.fg
        } else if enabled {
            p.on_accent
        } else {
            p.muted
        };
        icons::paint(
            painter,
            Rect::from_center_size(rect.center(), Vec2::splat(14.0)),
            if stop { Icon::Stop } else { Icon::ArrowUp },
            ink,
        );
        if stop {
            painter.rect_stroke(
                rect,
                7,
                Stroke::new(1.0, p.border),
                egui::StrokeKind::Inside,
            );
        }
    }
    let response = if enabled || stop {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    };
    response.on_hover_text(tip)
}

/// The dot that leads a row of the roster.
pub fn mark(painter: &egui::Painter, centre: Pos2, p: Palette, waits: bool, works: bool) {
    if waits {
        painter.circle_filled(centre, 4.0, p.attention);
    } else if works {
        painter.circle_filled(centre, 4.0, theme::tint(p.green, 0.28));
        painter.circle_filled(centre, 2.2, p.green);
    } else {
        painter.circle_stroke(centre, 3.5, Stroke::new(1.5, p.muted));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::transcript::{Ended, Origin, Source, Transcript};
    use eframe::egui::{Event as Input, RawInput};

    const WINDOW: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(400.0, 600.0));

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        theme::apply(&ctx, &crate::config::Config::default());
        ctx
    }
    fn texts(output: &egui::FullOutput) -> Vec<String> {
        fn collect(shape: &egui::Shape, texts: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, texts)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    }
    /// Every filled or outlined rectangle of `shape`, as (bounds, fill, outline).
    fn surfaces(shape: &egui::Shape, found: &mut Vec<(Rect, Color32, Stroke)>) {
        match shape {
            egui::Shape::Rect(rect) => found.push((rect.rect, rect.fill, rect.stroke)),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| surfaces(shape, found)),
            _ => {}
        }
    }
    fn key(key: Key, modifiers: Modifiers) -> Vec<Input> {
        [true, false]
            .into_iter()
            .map(|pressed| Input::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            })
            .collect()
    }

    struct Field {
        ctx: egui::Context,
        text: String,
        focus: bool,
        composing: bool,
    }
    impl Field {
        fn frame(&mut self, events: Vec<Input>) -> (bool, bool) {
            let mut sent = false;
            let mut focused = false;
            let mut output = self.ctx.run_ui(
                RawInput {
                    screen_rect: Some(WINDOW),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let p = Palette::for_config(&crate::config::Config::default());
                    let height = editor_height(ui, &self.text, 300.0, (1, 6));
                    let rect = Rect::from_min_size(Pos2::new(20.0, 20.0), vec2(300.0, height));
                    let (response, entered) = editor(
                        ui,
                        p,
                        rect,
                        Editor {
                            id: composer_id(),
                            text: &mut self.text,
                            hint: "Message the lead…",
                            label: "Message the lead",
                            focus: &mut self.focus,
                            composing: self.composing,
                            trailing: 30.0,
                            newline: false,
                        },
                    );
                    sent = entered;
                    focused = response.has_focus();
                },
            );
            output.textures_delta.clear();
            (sent, focused)
        }
    }

    #[test]
    fn enter_sends_shift_enter_breaks_the_line_and_escape_leaves_the_field() {
        let mut field = Field {
            ctx: context(),
            text: String::new(),
            focus: false,
            composing: false,
        };
        // It does not take the keyboard by itself.
        assert_eq!(field.frame(Vec::new()), (false, false));
        assert_eq!(
            field.frame(key(Key::Enter, Modifiers::NONE)),
            (false, false)
        );
        // Asked to, it takes it once and the request is spent.
        field.focus = true;
        field.frame(Vec::new());
        assert!(!field.focus);
        assert_eq!(field.frame(Vec::new()), (false, true));
        // Nothing to send yet: Enter adds no line and sends nothing.
        assert_eq!(field.frame(key(Key::Enter, Modifiers::NONE)), (false, true));
        assert_eq!(field.text, "");
        field.frame(vec![Input::Text("hello".into())]);
        assert_eq!(field.text, "hello");
        // Shift+Enter is a line break, and the field keeps the keyboard.
        assert_eq!(
            field.frame(key(Key::Enter, Modifiers::SHIFT)),
            (false, true)
        );
        field.frame(vec![Input::Text("there".into())]);
        assert_eq!(field.text, "hello\nthere");
        // While an input method composes, Enter is its own.
        field.composing = true;
        assert_eq!(field.frame(key(Key::Enter, Modifiers::NONE)), (false, true));
        field.composing = false;
        assert!(
            !field
                .frame(vec![
                    Input::Ime(egui::ImeEvent::Preedit {
                        text: "に".into(),
                        active_range_chars: None
                    }),
                    key(Key::Enter, Modifiers::NONE).remove(0)
                ])
                .0
        );
        field.frame(vec![Input::Ime(egui::ImeEvent::Commit(String::new()))]);
        // Enter alone sends; the text is the caller's to clear, and the
        // field keeps the keyboard for the next message.
        let text = field.text.clone();
        assert_eq!(field.frame(key(Key::Enter, Modifiers::NONE)), (true, true));
        assert_eq!(field.text, text, "Enter added no line");
        // Escape gives the keyboard up, for the terminal to take.
        field.frame(key(Key::Escape, Modifiers::NONE));
        assert_eq!(field.frame(Vec::new()), (false, false));
        // A taller text takes more room, up to its limit.
        let ctx = context();
        let mut heights = Vec::new();
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(WINDOW),
                ..Default::default()
            },
            |ui| {
                for lines in [0, 1, 3, 40] {
                    heights.push(editor_height(ui, &"x\n".repeat(lines), 300.0, (1, 6)));
                }
            },
        );
        output.textures_delta.clear();
        assert!(heights[0] <= heights[1] && heights[1] < heights[2] && heights[2] < heights[3]);
        assert_eq!(heights[3], 6.0 * ROW + 14.0);
    }

    fn chat_frame(
        ctx: &egui::Context,
        records: &VecDeque<Record>,
        shown: usize,
        events: Vec<Input>,
    ) -> (Vec<Action>, bool, egui::FullOutput) {
        following(ctx, records, shown, (&[], &[]), events)
    }
    /// The watched pull requests an agent at rest owns, as the chat is given them.
    type Pulls<'a> = &'a [(u64, PaneId, bool, bool)];
    fn following(
        ctx: &egui::Context,
        records: &VecDeque<Record>,
        shown: usize,
        (follow, pulls): (&[(PaneId, bool, bool)], Pulls),
        events: Vec<Input>,
    ) -> (Vec<Action>, bool, egui::FullOutput) {
        let mut actions = Vec::new();
        let mut earlier = false;
        let members = [(PaneId::new(12), 3)];
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(WINDOW),
                events,
                ..Default::default()
            },
            |ui| {
                let p = Palette::for_config(&crate::config::Config::default());
                ui.set_width(360.0);
                earlier = transcript(
                    ui,
                    p,
                    &Chat {
                        project: neptune_model::ProjectId::new(1),
                        proposed: &[],
                        pulls,
                        records,
                        shown,
                        members: &members,
                        follow,
                        thinking: false,
                        pending: &["Starting an agent…".to_owned()],
                        streaming: Some("I am sta"),
                        status: Some("Retrying (2 of 10)…"),
                        more: false,
                    },
                    &mut actions,
                );
            },
        );
        output.textures_delta.clear();
        (actions, earlier, output)
    }

    #[test]
    fn a_chat_sets_apart_who_spoke_shows_what_neptune_did_and_opens_an_agents_terminal() {
        let ctx = context();
        let mut chat = Transcript::default();
        for entry in [
            Entry::User {
                text: "Split checkout".into(),
                attachments: Vec::new(),
            },
            Entry::Turn {
                id: "t1".into(),
                origin: Origin::User,
            },
            Entry::Lead {
                text: "Plan:\n1. auth\n2. tests".into(),
            },
            Entry::Tool {
                name: "spawn_agent".into(),
                summary: "Started agent 12 · Claude Code · auth".into(),
                ok: true,
            },
            Entry::Lead {
                text: "Started one agent.".into(),
            },
            Entry::End {
                turn: "t1".into(),
                outcome: Ended::Completed,
                cost: None,
            },
            Entry::Event {
                source: Source::Agent,
                agent: Some(12),
                what: "finished its turn".into(),
                text: "All tests pass.".into(),
            },
            Entry::Event {
                source: Source::Agent,
                agent: Some(40),
                what: "ended".into(),
                text: String::new(),
            },
            Entry::Delivered { through: 7 },
            Entry::Notice {
                text: "The lead was stopped.".into(),
            },
        ] {
            chat.push(entry, 0);
        }
        // The turn's work is unfolded here: how its rows are set is what is
        // looked at.
        let fold = Id::new(("project-work", neptune_model::ProjectId::new(1), 2u64));
        ctx.data_mut(|data| data.insert_temp(fold, true));
        let (_, earlier, output) = chat_frame(&ctx, chat.records(), PAGE, Vec::new());
        assert!(!earlier);
        let texts = texts(&output);
        let position = |wanted: &str| {
            texts
                .iter()
                .position(|text| text.contains(wanted))
                .unwrap_or_else(|| panic!("{wanted} in {texts:?}"))
        };
        // In the order it happened.
        let order = [
            "Split checkout",
            "Worked for 0s",
            "Plan:",
            "auth",
            "Started agent 12 · Claude Code · auth",
            "Started one agent.",
            "Agent 12 finished its turn",
            "All tests pass.",
            "Open terminal",
            "Agent 40 ended",
            "The lead was stopped.",
            "I am sta",
            "Starting an agent…",
            "Retrying (2 of 10)…",
        ];
        for pair in order.windows(2) {
            assert!(
                position(pair[0]) < position(pair[1]),
                "{pair:?} in {texts:?}"
            );
        }
        let count = |wanted: &str| texts.iter().filter(|text| *text == wanted).count();
        // Nobody is named: who spoke is seen from how the words are set.
        assert_eq!((count("You"), count("Lead")), (0, 0));
        let p = Palette::for_config(&crate::config::Config::default());
        let drawn = |wanted: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text().contains(wanted) => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{wanted} is not drawn"))
        };
        // The surface drawn around `wanted`, as (bounds, fill, outline).
        let around = |wanted: &str| {
            let centre = drawn(wanted).center();
            let mut found = Vec::new();
            for clipped in &output.shapes {
                surfaces(&clipped.shape, &mut found);
            }
            found
                .into_iter()
                .filter(|(rect, ..)| rect.contains(centre) && rect.height() > 24.0)
                .collect::<Vec<_>>()
        };
        let left = drawn("Plan:").left();
        // What the person said is a filled bubble at the trailing edge, no
        // wider than its words; the lead's words are plain and start where
        // the column does.
        let said = around("Split checkout");
        assert!(
            matches!(said.as_slice(), [(_, fill, _)] if *fill == p.control),
            "{said:?}"
        );
        let bubble = said[0].0;
        assert!((bubble.right() - (left + 360.0)).abs() < 2.0, "{bubble:?}");
        assert!(bubble.width() < 160.0 && bubble.left() > left + 180.0);
        assert!(around("Plan:").is_empty());
        // What Neptune reports is outlined, not filled.
        let card = around("Agent 12 finished its turn");
        assert!(
            matches!(
                card.as_slice(),
                [(rect, Color32::TRANSPARENT, stroke)]
                    if *stroke == p.hairline() && (rect.width() - 360.0).abs() < 1.0
            ),
            "{card:?}"
        );
        // What Neptune did for the lead recedes; what failed would not.
        let ink = |wanted: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == wanted => {
                        Some(text.galley.job.sections[0].format.clone())
                    }
                    _ => None,
                })
                .unwrap()
        };
        let did = ink("Started agent 12 · Claude Code · auth");
        assert_eq!((did.color, did.font_id), (p.muted, theme::regular(11.5)));
        // Only an agent that still has a terminal offers it.
        assert_eq!(count("Open terminal"), 1);
        // Turn boundaries and delivery marks are not for reading.
        assert!(!texts.iter().any(|text| text.contains("t1")));

        // The button reveals that terminal as it was listed.
        let button = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Open terminal" => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap();
        let mut actions = Vec::new();
        for events in [
            vec![Input::PointerMoved(button)],
            vec![Input::PointerButton {
                pos: button,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            vec![Input::PointerButton {
                pos: button,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        ] {
            actions.extend(chat_frame(&ctx, chat.records(), PAGE, events).0);
        }
        assert!(
            matches!(actions.as_slice(), [Action::OpenAgent(pane, 3)] if *pane == PaneId::new(12))
        );

        // A row about an agent that still has a terminal also shows what
        // changed where it works. What its pull request needs is offered on
        // the newest row about it, once, and only where it needs something.
        chat.push(
            Entry::Event {
                source: Source::Agent,
                agent: Some(12),
                what: "finished its turn".into(),
                text: "Opened the pull request.".into(),
            },
            0,
        );
        let count = |output: &egui::FullOutput, wanted: &str| {
            self::texts(output)
                .iter()
                .filter(|text| *text == wanted)
                .count()
        };
        let (_, _, plain) = chat_frame(&ctx, chat.records(), PAGE, Vec::new());
        assert_eq!(
            (
                count(&plain, "Open terminal"),
                count(&plain, "Changes"),
                count(&plain, "Fix CI"),
                count(&plain, "Address comments")
            ),
            (2, 2, 0, 0)
        );
        let follow = [(PaneId::new(12), true, true), (PaneId::new(40), true, true)];
        let (_, _, output) = following(&ctx, chat.records(), PAGE, (&follow, &[]), Vec::new());
        assert_eq!(
            (count(&output, "Fix CI"), count(&output, "Address comments")),
            (1, 1)
        );
        let centre = |output: &egui::FullOutput, wanted: &str| {
            output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == wanted => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .next_back()
                .unwrap_or_else(|| panic!("{wanted} is not drawn"))
        };
        let press = |pos: Pos2, follow: &[(PaneId, bool, bool)]| {
            let button = |pressed| Input::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut actions = Vec::new();
            for events in [
                vec![Input::PointerMoved(pos)],
                vec![button(true)],
                vec![button(false)],
            ] {
                actions.extend(following(&ctx, chat.records(), PAGE, (follow, &[]), events).0);
            }
            actions
                .into_iter()
                .filter_map(|action| match action {
                    Action::Project(event) => Some(event),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        use crate::ui::project::{Event, FollowUp};
        let (project, pane) = (neptune_model::ProjectId::new(1), PaneId::new(12));
        let asked = |ask| Event::FollowUp {
            project,
            pane,
            generation: 3,
            ask,
        };
        assert_eq!(
            press(centre(&output, "Fix CI").center(), &follow),
            [asked(FollowUp::Checks)]
        );
        assert_eq!(
            press(centre(&output, "Address comments").center(), &follow),
            [asked(FollowUp::Comments)]
        );
        assert_eq!(
            press(centre(&output, "Changes").center(), &follow),
            [Event::Changes(pane, 3)]
        );
        // Everything stays inside the width it was given, wrapped.
        for wanted in ["Open terminal", "Changes", "Fix CI", "Address comments"] {
            assert!(centre(&output, wanted).right() <= 360.0 + 8.0, "{wanted}");
        }
        // The control at the end of the lead's reply copies the reply,
        // without the work before it. Nothing is at the end of a block the
        // turn goes on after.
        let end = |wanted: &str| {
            let line = centre(&output, wanted);
            Pos2::new(left + 360.0 - 9.0, line.center().y)
        };
        assert_eq!(press(end("Plan:"), &[]), []);
        assert_eq!(
            press(end("Started one agent."), &[]),
            [Event::Copy("Started one agent.".into())]
        );

        // Only the newest entries are laid out until earlier ones are asked for.
        let (_, _, output) = chat_frame(&ctx, chat.records(), 2, Vec::new());
        let texts = self::texts(&output);
        assert!(texts.iter().any(|text| text == "Load earlier"));
        assert!(!texts.iter().any(|text| text.contains("Split checkout")));
        assert!(texts.iter().any(|text| text.contains("finished its turn")));
        // An empty chat says what it is for, and a lead whose turn has begun
        // is thinking until something of it comes.
        let ctx = context();
        let empty = VecDeque::new();
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(WINDOW),
                ..Default::default()
            },
            |ui| {
                let p = Palette::for_config(&crate::config::Config::default());
                transcript(
                    ui,
                    p,
                    &Chat {
                        project: neptune_model::ProjectId::new(1),
                        proposed: &[],
                        pulls: &[],
                        records: &empty,
                        shown: PAGE,
                        members: &[],
                        follow: &[],
                        thinking: true,
                        pending: &[],
                        streaming: None,
                        status: None,
                        more: false,
                    },
                    &mut Vec::new(),
                );
            },
        );
        output.textures_delta.clear();
        let texts = self::texts(&output);
        assert!(
            texts
                .iter()
                .any(|text| text == "Tell the lead what you want done.")
        );
        assert!(
            !texts.iter().any(|text| text == "Lead")
                && texts.iter().any(|text| text == "Thinking…")
        );
    }

    /// A chat of `entries`, each written at its second.
    fn written(entries: impl IntoIterator<Item = (u64, Entry)>) -> Transcript {
        let mut chat = Transcript::default();
        for (at, entry) in entries {
            chat.push(entry, at);
        }
        chat
    }
    fn begin(id: &str) -> Entry {
        Entry::Turn {
            id: id.into(),
            origin: Origin::User,
        }
    }
    fn end(id: &str, outcome: Ended) -> Entry {
        Entry::End {
            turn: id.into(),
            outcome,
            cost: None,
        }
    }
    fn lead(text: &str) -> Entry {
        Entry::Lead { text: text.into() }
    }
    fn tool(summary: &str, ok: bool) -> Entry {
        Entry::Tool {
            name: "spawn_agent".into(),
            summary: summary.into(),
            ok,
        }
    }

    #[test]
    fn a_finished_turn_folds_everything_of_the_lead_but_its_reply() {
        let done = Ended::Completed;
        let folded = |entries: Vec<(u64, Entry)>| works(written(entries).records());
        // A reply alone has nothing before it to fold.
        assert_eq!(
            folded(vec![
                (5, begin("a")),
                (6, lead("Done.")),
                (7, end("a", done))
            ]),
            []
        );
        // What the lead said and Neptune did on the way is the work; the
        // last thing it said is the reply. What Neptune refused, and what
        // others wrote meanwhile, is not the lead's work.
        let chat = written([
            (100, begin("a")),
            (101, lead("I'll start an agent.")),
            (103, tool("Started agent 8", true)),
            (
                104,
                tool("The lead's spawn_agent request was refused", false),
            ),
            (
                150,
                Entry::Notice {
                    text: "The lead summarised earlier conversation.".into(),
                },
            ),
            (172, lead("Two problems.")),
            (172, tool("Updated STATUS.md", true)),
            (172, end("a", done)),
            (180, begin("b")),
            (181, tool("Started agent 9", true)),
            (4000, lead("Started it.")),
            (4040, end("b", done)),
        ]);
        let found = works(chat.records());
        let first = Work {
            turn: 1,
            end: 8,
            reply: 6,
            took: 72,
        };
        let second = Work {
            turn: 9,
            end: 12,
            reply: 11,
            took: 3860,
        };
        assert_eq!(found, [first, second]);
        let hidden = |work: Work| {
            chat.records()
                .iter()
                .filter(|record| work.folds(record))
                .map(|record| record.seq)
                .collect::<Vec<_>>()
        };
        assert_eq!((hidden(first), hidden(second)), (vec![2, 3, 7], vec![10]));
        // A refusal beside the reply leaves nothing to fold.
        assert_eq!(
            folded(vec![
                (1, begin("a")),
                (2, tool("The lead's spawn_agent request was refused", false)),
                (3, lead("I could not.")),
                (4, end("a", done)),
            ]),
            []
        );
        // A turn without a word of the lead's has no reply to set apart, and
        // neither has one that was stopped, failed, or cut short by a
        // restart: all of each is read.
        for outcome in [Ended::Interrupted, Ended::Failed, Ended::Limit] {
            assert_eq!(
                folded(vec![
                    (1, begin("a")),
                    (2, lead("I'll start two.")),
                    (3, tool("Started agent 8", true)),
                    (4, end("a", outcome)),
                ]),
                []
            );
        }
        assert_eq!(
            folded(vec![
                (1, begin("a")),
                (2, tool("Started agent 8", true)),
                (3, tool("Started agent 9", true)),
                (4, end("a", done)),
            ]),
            []
        );
        // A turn that still runs is read as it comes, as is one that
        // another began after without an end,
        assert_eq!(
            folded(vec![
                (1, begin("a")),
                (2, lead("First,")),
                (3, tool("Started agent 8", true)),
                (4, lead("then")),
            ]),
            []
        );
        assert_eq!(
            folded(vec![
                (1, begin("a")),
                (2, lead("First,")),
                (3, tool("Started agent 8", true)),
                (4, begin("b")),
                (5, lead("Done.")),
                (6, end("b", done)),
                (7, end("a", done)),
            ]),
            []
        );
        // and one whose beginning is further back than the entries held.
        assert_eq!(
            folded(vec![
                (2, lead("First,")),
                (3, tool("Started agent 8", true)),
                (4, lead("Done.")),
                (5, end("a", done)),
            ]),
            []
        );
        // How long it took is said in its two largest units.
        let said: Vec<String> = [0, 8, 59, 60, 72, 292, 3599, 3600, 3840, 90_000]
            .into_iter()
            .map(lasted)
            .collect();
        assert_eq!(
            said,
            [
                "0s", "8s", "59s", "1m 0s", "1m 12s", "4m 52s", "59m 59s", "1h 0m", "1h 4m",
                "25h 0m"
            ]
        );
    }

    #[test]
    fn a_turns_work_is_one_row_that_unfolds_it_and_what_was_refused_stays_in_view() {
        const REPLY: &str = "The audit found two problems.";
        const REFUSED: &str = "The lead's spawn_agent request was refused";
        let p = Palette::for_config(&crate::config::Config::default());
        let project = neptune_model::ProjectId::new(1);
        let turn = |ended: bool| {
            let mut entries = vec![
                (
                    100,
                    Entry::User {
                        text: "Audit it".into(),
                        attachments: Vec::new(),
                    },
                ),
                (100, begin("t1")),
                (101, lead("I'll start an agent.")),
                (
                    103,
                    tool("Started agent 8 · Codex · Performance audit", true),
                ),
                (104, tool(REFUSED, false)),
                (
                    150,
                    Entry::Event {
                        source: Source::Agent,
                        agent: Some(12),
                        what: "finished its turn".into(),
                        text: "All tests pass.".into(),
                    },
                ),
                (160, lead("Halfway there.")),
                (170, tool("Updated STATUS.md", true)),
                (172, lead(REPLY)),
            ];
            if ended {
                entries.push((172, end("t1", Ended::Completed)));
            }
            written(entries)
        };
        let draw = |ctx: &egui::Context, chat: &Transcript, shown: usize, events| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(WINDOW),
                    events,
                    ..Default::default()
                },
                |ui| {
                    // As wide as the narrowest panel.
                    ui.set_width(300.0);
                    transcript(
                        ui,
                        p,
                        &Chat {
                            project,
                            proposed: &[],
                            pulls: &[],
                            records: chat.records(),
                            shown,
                            members: &[],
                            follow: &[],
                            thinking: false,
                            pending: &[],
                            streaming: None,
                            status: None,
                            more: false,
                        },
                        &mut actions,
                    );
                },
            );
            output.textures_delta.clear();
            (actions, output)
        };
        let names = |output: &egui::FullOutput| -> Vec<String> {
            let update = output.platform_output.accesskit_update.as_ref();
            update
                .expect("an accessibility tree")
                .nodes
                .iter()
                // A label is read by its value, a control by its name.
                .filter_map(|(_, node)| node.label().or(node.value()).map(str::to_owned))
                .collect()
        };
        let drawn = |output: &egui::FullOutput, wanted: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == wanted => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{wanted} is not drawn"))
        };
        // Which of what the turn wrote is in view, in the order it is.
        let all = [
            "Audit it",
            "Worked for 1m 12s",
            "I'll start an agent.",
            "Started agent 8 · Codex · Performance audit",
            REFUSED,
            "Agent 12 finished its turn",
            "Halfway there.",
            "Updated STATUS.md",
            REPLY,
        ];
        let read = |output: &egui::FullOutput| {
            let texts = texts(output);
            let mut found: Vec<(usize, &str)> = all
                .into_iter()
                .filter_map(|wanted| Some((texts.iter().position(|text| text == wanted)?, wanted)))
                .collect();
            found.sort();
            found.into_iter().map(|(_, text)| text).collect::<Vec<_>>()
        };
        let press = |ctx: &egui::Context, chat: &Transcript, pos: Pos2| {
            let button = |pressed| Input::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut actions = Vec::new();
            for events in [
                vec![Input::PointerMoved(pos)],
                vec![button(true)],
                vec![button(false)],
            ] {
                actions.extend(draw(ctx, chat, PAGE, events).0);
            }
            actions
                .into_iter()
                .filter_map(|action| match action {
                    Action::Project(event) => Some(event),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        use crate::ui::project::Event;
        let copies =
            |output: &egui::FullOutput| Pos2::new(300.0 - 9.0, drawn(output, REPLY).center().y);

        let ctx = context();
        ctx.enable_accesskit();
        // While the turn runs, all of it is read as it comes.
        let running = turn(false);
        let (_, output) = draw(&ctx, &running, PAGE, Vec::new());
        let mut whole = all.to_vec();
        whole.remove(1);
        assert_eq!(read(&output), whole);
        // Once it has ended its work is one row, folded: the reply is read
        // without it. What Neptune refused and what an agent reported
        // meanwhile stay where they were, under the row.
        let chat = turn(true);
        let (_, output) = draw(&ctx, &chat, PAGE, Vec::new());
        let folded = [
            "Audit it",
            "Worked for 1m 12s",
            REFUSED,
            "Agent 12 finished its turn",
            REPLY,
        ];
        assert_eq!(read(&output), folded);
        let named = names(&output);
        let count =
            |named: &[String], wanted: &str| named.iter().filter(|name| *name == wanted).count();
        assert_eq!(
            (
                count(&named, "Show work, turn 2"),
                count(&named, "Lead said"),
                count(&named, "Copy the lead's reply, entry 9"),
            ),
            (1, 2, 1),
            "{named:?}"
        );
        // The row is in the quiet ink and size of what Neptune did, with a
        // hairline under it.
        let row = drawn(&output, "Worked for 1m 12s");
        let ink = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Worked for 1m 12s" => {
                    Some(text.galley.job.sections[0].format.clone())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!((ink.color, ink.font_id), (p.muted, theme::regular(11.5)));
        let rules: Vec<[Pos2; 2]> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::LineSegment { points, stroke } if *stroke == p.hairline() => {
                    Some(*points)
                }
                _ => None,
            })
            .collect();
        assert!(
            matches!(
                rules.as_slice(),
                [[from, to]] if from.y > row.bottom()
                    && from.y < drawn(&output, REFUSED).top()
                    && (to.x - from.x - 300.0).abs() < 0.5
            ),
            "{rules:?}"
        );
        // The reply alone is copied.
        assert_eq!(
            press(&ctx, &chat, copies(&output)),
            [Event::Copy(REPLY.into())]
        );
        // The whole row is pressed, not its words alone: the work unfolds
        // in place, in the order it happened, and the reply stays last.
        assert_eq!(press(&ctx, &chat, Pos2::new(290.0, row.center().y)), []);
        let (_, output) = draw(&ctx, &chat, PAGE, Vec::new());
        assert_eq!(read(&output), all);
        let named = names(&output);
        assert_eq!(
            (
                count(&named, "Hide work, turn 2"),
                count(&named, "Show work, turn 2")
            ),
            (1, 0),
            "{named:?}"
        );
        assert_eq!(drawn(&output, "Worked for 1m 12s"), row);
        assert_eq!(
            press(&ctx, &chat, copies(&output)),
            [Event::Copy(REPLY.into())]
        );
        // Its words fold it again, and so do Enter and Space once it has
        // the keyboard.
        press(&ctx, &chat, row.center());
        assert_eq!(read(&draw(&ctx, &chat, PAGE, Vec::new()).1), folded);
        let fold = Id::new(("project-work", project, 2u64));
        ctx.memory_mut(|memory| memory.request_focus(fold));
        draw(&ctx, &chat, PAGE, Vec::new());
        draw(&ctx, &chat, PAGE, key(Key::Enter, Modifiers::NONE));
        assert_eq!(read(&draw(&ctx, &chat, PAGE, Vec::new()).1), all);
        draw(&ctx, &chat, PAGE, key(Key::Space, Modifiers::NONE));
        assert_eq!(read(&draw(&ctx, &chat, PAGE, Vec::new()).1), folded);

        // Where only the newest entries are laid out, a turn that begins
        // further up folds what of it is in view,
        let (_, output) = draw(&ctx, &chat, 3, Vec::new());
        assert_eq!(read(&output), ["Worked for 1m 12s", REPLY]);
        // and a reply whose work is all further up stands alone.
        let (_, output) = draw(&ctx, &chat, 1, Vec::new());
        assert_eq!(read(&output), [REPLY]);
        assert!(texts(&output).iter().any(|text| text == "Load earlier"));
    }

    #[test]
    fn a_proposed_watch_is_allowed_declined_or_looked_up_and_speakers_keep_their_names() {
        // Two lines in the width it is drawn in, the second a short one.
        const REPLY: &str =
            "I'd like to check the suite every night, after the last merge has landed.";
        let ctx = context();
        ctx.enable_accesskit();
        let mut chat = Transcript::default();
        for entry in [
            Entry::User {
                text: "Check it every night.\n\nAnd tell me what fails, with the names of the \
                       tests that failed and of whoever last touched them."
                    .into(),
                attachments: Vec::new(),
            },
            Entry::Lead { text: REPLY.into() },
            Entry::Proposal {
                watch: 3,
                what: "The lead proposes a watch: “Nightly check” · every day".into(),
                text: "Run the full suite.".into(),
            },
            Entry::Proposal {
                watch: 4,
                what: "The lead proposes a watch: “Hourly” · every hour".into(),
                text: "Look at the queue.".into(),
            },
        ] {
            chat.push(entry, 0);
        }
        let draw = |events| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(WINDOW),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let p = Palette::for_config(&crate::config::Config::default());
                    ui.set_width(300.0);
                    transcript(
                        ui,
                        p,
                        &Chat {
                            project: neptune_model::ProjectId::new(1),
                            // The second was answered already.
                            proposed: &[3],
                            pulls: &[],
                            records: chat.records(),
                            shown: PAGE,
                            members: &[],
                            follow: &[],
                            thinking: false,
                            pending: &[],
                            streaming: None,
                            status: None,
                            more: false,
                        },
                        &mut actions,
                    );
                },
            );
            output.textures_delta.clear();
            (actions, output)
        };
        let (_, output) = draw(Vec::new());
        let texts = texts(&output);
        let count = |wanted: &str| texts.iter().filter(|text| *text == wanted).count();
        // The headline, then which watch and how often on a line of its
        // own; only the one that still waits has anything to press.
        assert_eq!(
            (
                count("The lead proposes a watch"),
                count("“Nightly check” · every day"),
                count("“Hourly” · every hour"),
            ),
            (2, 1, 1),
            "{texts:?}"
        );
        assert_eq!(
            (
                count("Allow"),
                count("Decline"),
                count("Watches"),
                count("No")
            ),
            (1, 1, 1, 0)
        );
        // A long message wraps inside its bubble, which keeps clear of the
        // leading edge.
        let p = Palette::for_config(&crate::config::Config::default());
        let mut found = Vec::new();
        for clipped in &output.shapes {
            surfaces(&clipped.shape, &mut found);
        }
        let bubbles: Vec<Rect> = found
            .iter()
            .filter(|(rect, fill, _)| *fill == p.control && rect.height() > 40.0)
            .map(|(rect, ..)| *rect)
            .collect();
        assert!(
            matches!(
                bubbles.as_slice(),
                [bubble] if bubble.width() > 200.0
                    && bubble.width() <= 300.0 * BUBBLE + 0.5
                    && bubble.right() == 300.0
            ),
            "{bubbles:?}"
        );
        // Whose words a run is stays in its accessible name, and the run
        // the lead has ended is copied from its end.
        let names: Vec<String> = output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("an accessibility tree")
            .nodes
            .iter()
            // A label is read by its value, a control by its name.
            .filter_map(|(_, node)| node.label().or(node.value()).map(str::to_owned))
            .collect();
        for wanted in [
            "You said",
            "Lead said",
            "Copy the lead's reply, entry 2",
            "Allow watch 3",
            "Decline watch 3",
            "Show the watches, watch 3",
        ] {
            assert_eq!(
                names.iter().filter(|name| *name == wanted).count(),
                1,
                "{wanted} in {names:?}"
            );
        }
        let drawn = |wanted: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == wanted => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{wanted} is not drawn"))
        };
        let press = |pos: Pos2| {
            let button = |pressed| Input::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut actions = Vec::new();
            for events in [
                vec![Input::PointerMoved(pos)],
                vec![button(true)],
                vec![button(false)],
            ] {
                actions.extend(draw(events).0);
            }
            actions
                .into_iter()
                .filter_map(|action| match action {
                    Action::Project(event) => Some(event),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        use crate::ui::project::{Event, Segment};
        let project = neptune_model::ProjectId::new(1);
        let on = |wanted: &str| press(drawn(wanted).center());
        assert_eq!(on("Allow"), [Event::AllowWatch(project, 3, true)]);
        assert_eq!(on("Decline"), [Event::AllowWatch(project, 3, false)]);
        assert_eq!(on("Watches"), [Event::Show(Segment::Watches)]);
        // The copy control is beside the reply's last line, which leaves
        // it room, and not on a row of its own.
        let reply = drawn(REPLY);
        assert!(
            reply.height() > markup::LINE && reply.height() <= 2.0 * markup::LINE,
            "{reply:?}"
        );
        assert_eq!(
            press(Pos2::new(300.0 - 9.0, reply.bottom() - 8.0)),
            [Event::Copy(REPLY.into())]
        );
    }

    #[test]
    fn a_report_is_laid_out_as_a_reply_and_a_pull_requests_row_offers_what_its_agent_needs() {
        let ctx = context();
        let p = Palette::for_config(&crate::config::Config::default());
        let mut chat = Transcript::default();
        let pull = |watch, what: &str| Entry::Event {
            source: Source::Pr,
            agent: Some(watch),
            what: what.into(),
            text: "Open · checks failing".into(),
        };
        for entry in [
            Entry::Event {
                source: Source::Agent,
                agent: Some(12),
                what: "finished its turn".into(),
                text: "**Done.** Ran `cargo test`.\n\n- one\n- two".into(),
            },
            pull(
                4,
                "Pull request zevem/neptune#83 of agent 12: its checks are pending",
            ),
            pull(
                4,
                "Pull request zevem/neptune#83 of agent 12: its checks are failing",
            ),
            pull(9, "Pull request zevem/neptune#90: its checks are failing"),
        ] {
            chat.push(entry, 0);
        }
        let draw = |follow: &[(PaneId, bool, bool)], pulls: Pulls, events| {
            following(&ctx, chat.records(), PAGE, (follow, pulls), events)
        };
        let (_, _, output) = draw(&[], &[], Vec::new());
        // Of what an agent reported the card shows the words alone,
        // without the marks that set them, in the quieter ink of what is
        // quoted.
        let texts = texts(&output);
        let has = |wanted: &str| texts.iter().any(|text| text == wanted);
        assert!(has("Done. Ran cargo test. one two"), "{texts:?}");
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("**") || text.contains('`'))
        );
        let inks = |output: &egui::FullOutput, wanted: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == wanted => Some(
                        text.galley
                            .job
                            .sections
                            .iter()
                            .map(|section| section.format.color)
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                })
                .unwrap()
        };
        let report = inks(&output, "Done. Ran cargo test. one two");
        assert!(report.iter().all(|ink| *ink == p.secondary), "{report:?}");
        // Unfolded by its headline, the report is read as the lead's
        // replies are: its marks are styling and its list a list, in the
        // same ink.
        let headline = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Agent 12 finished its turn" => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap();
        let button = |pressed| Input::PointerButton {
            pos: headline,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        draw(&[], &[], vec![Input::PointerMoved(headline)]);
        draw(&[], &[], vec![button(true)]);
        draw(&[], &[], vec![button(false)]);
        let (_, _, output) = draw(&[], &[], Vec::new());
        let texts = self::texts(&output);
        let has = |wanted: &str| texts.iter().any(|text| text == wanted);
        assert!(
            has("Done. Ran cargo test.") && has("one") && has("two"),
            "{texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("**") || text.contains('`'))
        );
        let report = inks(&output, "Done. Ran cargo test.");
        assert!(report.iter().all(|ink| *ink == p.secondary), "{report:?}");
        // A row about a pull request says how it stands and offers nothing
        // by itself.
        let count = |output: &egui::FullOutput, wanted: &str| {
            self::texts(output)
                .iter()
                .filter(|text| *text == wanted)
                .count()
        };
        assert_eq!(count(&output, "Open · checks failing"), 3);
        assert_eq!(
            count(&output, "Fix CI") + count(&output, "Address comments"),
            0
        );
        // Where an agent at rest owns it, its newest row offers what that
        // agent's own row does, though that row is long gone. One whose
        // agent has no terminal offers nothing.
        let (resting, gone) = (PaneId::new(12), PaneId::new(77));
        let pulls = [(4, resting, true, true), (9, gone, true, false)];
        let (_, _, output) = draw(&[], &pulls, Vec::new());
        assert_eq!(
            (count(&output, "Fix CI"), count(&output, "Address comments")),
            (1, 1)
        );
        let centre = |output: &egui::FullOutput, wanted: &str| {
            output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == wanted => {
                        Some(text.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .next_back()
                .unwrap_or_else(|| panic!("{wanted} is not drawn"))
        };
        let press = |pos: Pos2, follow: &[(PaneId, bool, bool)]| {
            let button = |pressed| Input::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut actions = Vec::new();
            for events in [
                vec![Input::PointerMoved(pos)],
                vec![button(true)],
                vec![button(false)],
            ] {
                actions.extend(draw(follow, &pulls, events).0);
            }
            actions
                .into_iter()
                .filter_map(|action| match action {
                    Action::Project(event) => Some(event),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        use crate::ui::project::{Event, FollowUp};
        let asked = |ask| Event::FollowUp {
            project: neptune_model::ProjectId::new(1),
            pane: resting,
            generation: 3,
            ask,
        };
        // The same action as on the agent's row.
        assert_eq!(
            press(centre(&output, "Fix CI"), &[]),
            [asked(FollowUp::Checks)]
        );
        assert_eq!(
            press(centre(&output, "Address comments"), &[]),
            [asked(FollowUp::Comments)]
        );
        // Both rows in view offer it, each with a control of its own.
        let follow = [(resting, true, false)];
        let (_, _, output) = draw(&follow, &pulls, Vec::new());
        assert_eq!(count(&output, "Fix CI"), 2);
        assert_eq!(
            press(centre(&output, "Fix CI"), &follow),
            [asked(FollowUp::Checks)]
        );
    }

    #[test]
    fn a_long_report_is_two_lines_of_its_words_until_its_headline_unfolds_it() {
        const REPORT: &str = "## Tech stack\n\n**P2, measured:** discovery of the             packages took **four seconds** on a cold start, and the audit found three more             places where the same work is done twice before the first screen is drawn.\n\n            | Area | Stack |\n|---|---|\n| Mobile | Expo 57 |\n| API | Hono |\n\n            - one\n- two\n- three\n\n[Full report](https://reports.example/audit)";
        let ctx = context();
        ctx.enable_accesskit();
        let p = Palette::for_config(&crate::config::Config::default());
        let mut chat = Transcript::default();
        for (agent, text) in [(12, REPORT), (40, "**All** tests pass.")] {
            chat.push(
                Entry::Event {
                    source: Source::Agent,
                    agent: Some(agent),
                    what: "“Tech stack inspection” finished its turn".into(),
                    text: text.into(),
                },
                0,
            );
        }
        let draw = |events| chat_frame(&ctx, chat.records(), PAGE, events);
        // The outlined cards, from the top.
        let cards = |output: &egui::FullOutput| {
            let mut found = Vec::new();
            for clipped in &output.shapes {
                surfaces(&clipped.shape, &mut found);
            }
            found
                .into_iter()
                .filter(|(_, _, stroke)| *stroke == p.hairline())
                .map(|(rect, ..)| rect)
                .collect::<Vec<_>>()
        };
        let names = |output: &egui::FullOutput| -> Vec<String> {
            let update = output.platform_output.accesskit_update.as_ref();
            update
                .expect("an accessibility tree")
                .nodes
                .iter()
                .filter_map(|(_, node)| node.label().map(str::to_owned))
                .collect()
        };
        let rows = |output: &egui::FullOutput, start: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text().starts_with(start) => {
                        Some((text.galley.text().to_owned(), text.galley.rows.len()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{start} is not drawn"))
        };
        let centre = |output: &egui::FullOutput, wanted: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == wanted => {
                        Some(text.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{wanted} is not drawn"))
        };
        let press = |pos: Pos2| {
            let button = |pressed| Input::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut actions = Vec::new();
            for events in [
                vec![Input::PointerMoved(pos)],
                vec![button(true)],
                vec![button(false)],
            ] {
                actions.extend(draw(events).0);
            }
            actions
        };
        draw(Vec::new());
        let (_, _, output) = draw(Vec::new());
        // Folded, the card is an event: its headline, two lines of the
        // report's words ended after a whole word, and its actions.
        let (words, lines) = rows(&output, "Tech stack P2, measured:");
        assert_eq!(lines, 2, "{words}");
        assert!(words.ends_with('…') && !words.ends_with(" …"), "{words}");
        let whole = markup::plain(REPORT);
        let kept = words.trim_end_matches('…');
        assert!(
            whole.starts_with(kept) && whole[kept.len()..].starts_with([' ', ',', '.']),
            "{words}"
        );
        assert!(!words.contains(['*', '|', '#', '`', '[']), "{words}");
        let folded = cards(&output);
        // Its headline is two lines in this width.
        assert!(folded[0].height() < 112.0, "{folded:?}");
        assert!(folded[1].height() < 56.0, "{folded:?}");
        let texts = texts(&output);
        assert!(!texts.iter().any(|text| text == "Mobile" || text == "one"));
        // A report that two lines hold has nothing to unfold; its words
        // are shown all the same.
        assert!(texts.iter().any(|text| text == "All tests pass."));
        let named = names(&output);
        let folds: Vec<&String> = named
            .iter()
            .filter(|name| name.starts_with("Show more, entry "))
            .collect();
        assert_eq!(folds.len(), 1, "{named:?}");
        // The headline unfolds it in place: its table, its list and its
        // link, which opens.
        let headline = centre(
            &output,
            "Agent 12 “Tech stack inspection” finished its turn",
        );
        assert!(press(headline).is_empty());
        let (_, _, output) = draw(Vec::new());
        let texts = self::texts(&output);
        for wanted in ["Tech stack", "Mobile", "Expo 57", "three", "Full report"] {
            assert!(
                texts.iter().any(|text| text == wanted),
                "{wanted} in {texts:?}"
            );
        }
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("**") || text.contains('|'))
        );
        let unfolded = cards(&output);
        assert!(unfolded[0].height() > folded[0].height() + 100.0);
        assert!(unfolded[0].width() == folded[0].width());
        let named = names(&output);
        assert!(
            named
                .iter()
                .any(|name| name.starts_with("Show less, entry "))
        );
        assert!(matches!(
            press(centre(&output, "Full report")).as_slice(),
            [Action::OpenLink(link)] if link.as_str() == "https://reports.example/audit"
        ));
        // Its buttons stay under it, and the headline folds it again.
        assert!(matches!(
            press(centre(&output, "Open terminal")).as_slice(),
            [Action::OpenAgent(pane, 3)] if *pane == PaneId::new(12)
        ));
        press(headline);
        let (_, _, output) = draw(Vec::new());
        assert_eq!(cards(&output)[0], folded[0]);
        // Cut where its words are of several bytes, a preview is whole.
        assert!(
            preview(&"é ".repeat(PREVIEW))
                .chars()
                .all(|c| c == 'é' || c == ' ')
        );
    }
}
