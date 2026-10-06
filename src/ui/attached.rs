//! The files an agent attached to its terminal: a count on the terminal's
//! tab that lists them, each with its picture where it is one, to be opened,
//! shown in the file manager or taken off the list, and its pictures to be
//! looked through together.
use super::helpers::{elided, galley_at, menu_item, menu_layout, menu_separator};
use super::{Action, helpers::path_label};
use crate::{
    icons::{self, Icon},
    platform::files::REVEAL_LABEL,
    theme::{self, Palette},
};
use eframe::egui::{
    self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType, vec2,
};
use neptune_model::PaneId;
use std::path::PathBuf;

/// What is known of an attached file once it has been looked at.
#[derive(Clone, Default)]
pub enum Kind {
    /// Not read yet.
    #[default]
    Unread,
    /// A picture, with a small copy of it.
    Picture(egui::TextureHandle),
    /// A file of another kind.
    Other,
    /// Moved or deleted since it was attached.
    Missing,
}

/// One attached file, as its row shows it.
#[derive(Clone)]
pub struct File {
    pub path: PathBuf,
    /// What the agent called it, or its name.
    pub label: String,
    /// Its name under a title, and the folder it is in.
    pub detail: String,
    pub kind: Kind,
}

impl File {
    pub fn new(file: &neptune_model::Attachment, kind: Kind) -> Self {
        let name = file.name();
        // A long folder keeps its end, which tells one from another.
        let folder = |room| {
            file.path()
                .parent()
                .map(|folder| path_label(folder, room))
                .unwrap_or_default()
        };
        let (label, detail) = match file.title() {
            Some(title) => {
                let room = 40_usize.saturating_sub(name.chars().count()).max(12);
                (title.to_owned(), format!("{name}  ·  {}", folder(room)))
            }
            None => (name, folder(44)),
        };
        Self {
            path: file.path().into(),
            label,
            detail,
            kind,
        }
    }
    fn missing(&self) -> bool {
        matches!(self.kind, Kind::Missing)
    }
}

const ROW: f32 = 48.0;
/// The side of the well a file's picture is drawn in.
pub const THUMB: f32 = 36.0;
/// Rows in view before the list scrolls.
const ROWS_IN_VIEW: f32 = 6.5;
const WIDTH: f32 = 340.0;

/// How many files the terminal's agent attached, as a chip that lists them.
/// Laid out and painted like `SpawnedChip`, before which it sits.
pub struct AttachedChip<'a> {
    chip: Option<(Rect, std::sync::Arc<egui::Galley>, egui::Response)>,
    pane: PaneId,
    files: &'a [File],
    /// Leading edge of the chip; the trailing edge given when there is none.
    pub left: f32,
}
impl<'a> AttachedChip<'a> {
    pub fn layout(
        ui: &egui::Ui,
        id: egui::Id,
        (pane, files): (PaneId, &'a [File]),
        (limit, right, middle): (f32, f32, f32),
        p: Palette,
    ) -> Self {
        let mut chip = None;
        let mut left = right;
        if !files.is_empty() {
            let count =
                ui.painter()
                    .layout_no_wrap(files.len().to_string(), theme::medium(11.5), p.accent);
            let width = count.size().x + 41.0;
            if right - width >= limit {
                let rect = Rect::from_min_max(
                    egui::pos2(right - width, middle - 9.0),
                    egui::pos2(right, middle + 9.0),
                );
                left = rect.left() - 1.0;
                let response = ui.interact(rect, id, Sense::click());
                response.widget_info(|| {
                    WidgetInfo::labeled(WidgetType::Button, true, "Attached files")
                });
                chip = Some((rect, count, response));
            }
        }
        Self {
            chip,
            pane,
            files,
            left,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.chip.is_none()
    }
    pub fn hovered(&self) -> bool {
        self.chip.as_ref().is_some_and(|chip| chip.2.hovered())
    }
    pub fn paint(self, painter: &egui::Painter, p: Palette, actions: &mut Vec<Action>) {
        let Some((chip, count, response)) = self.chip else {
            return;
        };
        let listing =
            egui::Popup::is_id_open(&response.ctx, egui::Popup::default_response_id(&response));
        if response.hovered() || listing {
            painter.rect_filled(chip, 5, theme::tint(p.accent, 0.14));
        }
        icons::paint(
            painter,
            Rect::from_center_size(
                egui::pos2(chip.left() + 11.5, chip.center().y),
                Vec2::splat(13.0),
            ),
            Icon::Paperclip,
            p.accent,
        );
        galley_at(
            painter,
            egui::pos2(chip.left() + 22.0, chip.center().y + 1.0),
            count,
        );
        icons::paint(
            painter,
            Rect::from_center_size(
                egui::pos2(chip.right() - 10.0, chip.center().y + 0.5),
                Vec2::splat(10.0),
            ),
            Icon::ChevronDown,
            p.accent,
        );
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        // Taking a file off the list leaves the list open for the next one.
        egui::Popup::menu(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| list(ui, p, self.pane, self.files, actions));
        if !listing {
            response.on_hover_text(match self.files.len() {
                1 => "1 attached file".to_owned(),
                count => format!("{count} attached files"),
            });
        }
    }
}

/// The attached files, newest first, and the way to remove them all.
fn list(ui: &mut Ui, p: Palette, pane: PaneId, files: &[File], actions: &mut Vec<Action>) {
    menu_layout(ui, WIDTH);
    let mut rows = |ui: &mut Ui| {
        for file in files.iter().rev() {
            ui.push_id(&file.path, |ui| row(ui, p, pane, file, actions));
        }
    };
    if files.len() as f32 > ROWS_IN_VIEW {
        egui::ScrollArea::vertical()
            .id_salt("attached-files")
            .max_height(ROW * ROWS_IN_VIEW)
            .auto_shrink(false)
            .show(ui, rows);
    } else {
        rows(ui);
    }
    menu_separator(ui, p);
    let pictures = files
        .iter()
        .filter(|file| matches!(file.kind, Kind::Picture(_)))
        .count();
    // One picture is a click on its row away; several are looked through.
    if pictures > 1 && menu_item(ui, p, Icon::Image, "View all pictures", "", false) {
        actions.push(Action::ViewAttachedPictures(pane));
        ui.close();
    }
    let label = if files.len() == 1 {
        "Dismiss"
    } else {
        "Dismiss all"
    };
    // Nothing is deleted, so the row does not wear the destructive colour.
    if menu_item(ui, p, Icon::Close, label, "", false) {
        actions.push(Action::ClearAttachments(pane));
        ui.close();
    }
}

/// A small control at the trailing end of a row.
fn row_button(ui: &mut Ui, p: Palette, rect: Rect, icon: Icon, label: &str) -> bool {
    let response = ui
        .interact(rect, ui.id().with(label), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    if response.hovered() {
        ui.painter().rect_filled(rect, 6, theme::tint(p.fg, 0.12));
    }
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(14.0)),
        icon,
        if response.hovered() { p.fg } else { p.muted },
    );
    response.on_hover_text(label).clicked()
}

fn row(ui: &mut Ui, p: Palette, pane: PaneId, file: &File, actions: &mut Vec<Action>) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW));
    let response = ui.interact(rect, ui.id().with("open"), Sense::click());
    let picture = matches!(file.kind, Kind::Picture(_));
    let verb = if picture { "View" } else { "Open" };
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            !file.missing(),
            format!("{verb} attached file {}", file.label),
        )
    });
    // The row's own controls sit over it and take their clicks first.
    let button = |index: f32| {
        Rect::from_center_size(
            Pos2::new(rect.right() - 20.0 - index * 28.0, rect.center().y),
            Vec2::splat(26.0),
        )
    };
    let remove = row_button(ui, p, button(0.0), Icon::Close, "Dismiss");
    let reveal = !file.missing() && row_button(ui, p, button(1.0), Icon::Folder, REVEAL_LABEL);
    let over_button = ui.rect_contains_pointer(button(0.0).union(button(1.0)));
    let painter = ui.painter();
    if (response.hovered() || over_button || response.has_focus()) && ui.is_rect_visible(rect) {
        painter.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 7, theme::tint(p.fg, 0.07));
    }
    let well = Rect::from_center_size(
        Pos2::new(rect.left() + 8.0 + THUMB * 0.5, rect.center().y),
        Vec2::splat(THUMB),
    );
    painter.rect_filled(well, 6, p.control);
    match &file.kind {
        Kind::Picture(texture) => {
            // The whole picture, centred in its well.
            let size = texture.size_vec2();
            let fitted = size * (THUMB / size.x.max(size.y).max(1.0));
            painter.add(
                egui::epaint::RectShape::filled(
                    Rect::from_center_size(well.center(), fitted),
                    if fitted.min_elem() >= 12.0 { 5 } else { 2 },
                    Color32::WHITE,
                )
                .with_texture(
                    texture.id(),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                ),
            );
        }
        kind => icons::paint(
            painter,
            Rect::from_center_size(well.center(), Vec2::splat(16.0)),
            if matches!(kind, Kind::Missing) {
                Icon::Warning
            } else {
                Icon::File
            },
            p.muted,
        ),
    }
    painter.rect_stroke(
        well,
        6,
        Stroke::new(1.0, p.hairline().color),
        StrokeKind::Inside,
    );
    let text = well.right() + 10.0;
    let room = button(1.0).left() - 6.0 - text;
    let ink = if file.missing() { p.muted } else { p.fg };
    galley_at(
        painter,
        Pos2::new(text, rect.center().y - 8.0),
        elided(painter, &file.label, theme::regular(13.0), ink, room),
    );
    let detail = if file.missing() {
        "No longer there"
    } else {
        &file.detail
    };
    galley_at(
        painter,
        Pos2::new(text, rect.center().y + 9.0),
        elided(painter, detail, theme::regular(11.5), p.secondary, room),
    );
    if remove {
        actions.push(Action::RemoveAttachment(pane, file.path.clone()));
    } else if reveal {
        actions.push(Action::RevealAttachment(file.path.clone()));
        ui.close();
    } else if !file.missing()
        && response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    {
        actions.push(Action::OpenAttachment(pane, file.path.clone()));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_leads_with_the_title_and_keeps_the_name_beside_the_folder() {
        let folder = std::env::temp_dir().join("shots");
        let path = folder.join("after.png");
        let titled = neptune_model::Attachment::new(path.clone(), Some("After")).unwrap();
        let file = File::new(&titled, Kind::Other);
        assert_eq!(file.label, "After");
        assert_eq!(
            file.detail,
            format!("after.png  ·  {}", path_label(&folder, 31))
        );
        let plain = neptune_model::Attachment::new(path, None).unwrap();
        let file = File::new(&plain, Kind::Missing);
        assert_eq!((file.label.as_str(), file.missing()), ("after.png", true));
        assert_eq!(file.detail, path_label(&folder, 44));
    }
}
