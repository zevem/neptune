//! The file explorer: a tab of the panel at the window's trailing edge showing
//! the focused terminal's folder as a tree, with a search and a preview. It
//! draws what the application read on its workers and reports what was asked
//! of it.
use super::Action;
use super::helpers::{
    animate, bare_text_edit, compact_path, elided, field_frame, galley_at, menu_item, menu_layout,
    menu_separator, place,
};
use crate::{
    icons::{self, Icon},
    platform::files::REVEAL_LABEL,
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Align2, CursorIcon, Id, Key, Layout, Pos2, Rect, Sense, Stroke, Ui, UiBuilder,
    Vec2, WidgetInfo, WidgetType, vec2,
};
use std::path::{Path, PathBuf};

mod selection;
use selection::Selection;

/// Folders a search leaves out until the field says otherwise.
pub const DEFAULT_EXCLUDE: &str =
    "**/node_modules/**, **/.git/**, **/dist/**, **/target/**, **/build/**";
const ROW: f32 = 26.0;
const INDENT: f32 = 14.0;

pub fn query_id() -> Id {
    Id::new("explorer-query")
}
pub fn exclude_id() -> Id {
    Id::new("explorer-exclude")
}
pub fn edit_id() -> Id {
    Id::new("explorer-edit")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    NewFile,
    NewFolder,
    Rename,
}

/// A name being typed in the tree, for a new item or an existing one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub kind: EditKind,
    /// The folder that holds the item.
    pub parent: PathBuf,
    /// The item being renamed.
    pub target: Option<PathBuf>,
    pub text: String,
    /// The field takes the keyboard on its next frame.
    pub focus: bool,
    pub error: Option<String>,
}

/// A row to bring into view once the tree has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScrollTarget {
    Path(PathBuf),
    Edit,
}

pub struct State {
    pub query: String,
    pub exclude: String,
    /// The exclusion field is shown without a search.
    pub filters: bool,
    pub edit: Option<Edit>,
    /// The highlighted item, and whether it is a folder.
    pub selected: Option<(PathBuf, bool)>,
    pub scroll_to: Option<ScrollTarget>,
    /// The target was outside the rows laid out last frame.
    scroll_jump: bool,
    /// The item awaiting confirmation in the delete sheet.
    pub delete: Option<PathBuf>,
    /// The preview's share of the panel's height below the fields.
    pub preview_share: f32,
    pub preview_location: Option<(u32, u32)>,
    pub preview_jump: bool,
    pub markdown_preview: bool,
    pub markdown_anchor: Option<String>,
    pub source_selection: Selection,
}

impl Default for State {
    fn default() -> Self {
        Self {
            query: String::new(),
            exclude: DEFAULT_EXCLUDE.into(),
            filters: false,
            edit: None,
            selected: None,
            scroll_to: None,
            scroll_jump: false,
            delete: None,
            preview_share: 0.45,
            preview_location: None,
            preview_jump: false,
            markdown_preview: true,
            markdown_anchor: None,
            source_selection: Selection::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Expand(PathBuf, bool),
    /// Preview a file.
    Select(PathBuf),
    FollowLink {
        path: PathBuf,
        anchor: Option<String>,
    },
    /// Leave the search and show this item, a folder if `true`, in the tree.
    ShowInTree(PathBuf, bool),
    ClosePreview,
    /// Name a new item in `parent`, or beside the selection without one.
    BeginCreate {
        parent: Option<PathBuf>,
        folder: bool,
    },
    BeginRename(PathBuf),
    /// Use the typed name. Enter is explicit; clicking away is not.
    Commit {
        explicit: bool,
    },
    CancelEdit,
    /// The typed name changed, so its problem no longer describes it.
    Retyped,
    /// Ask before deleting.
    Delete(PathBuf),
    ConfirmDelete,
    Reveal(PathBuf),
    Open(PathBuf),
    OpenLink(crate::platform::links::WebLink),
    CopyPath(PathBuf),
    CopyRelativePath(PathBuf),
    Refresh,
    CollapseAll,
    SearchChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Folder {
        expanded: bool,
    },
    File {
        picture: bool,
    },
    /// The name field of the edit in progress.
    Edit {
        folder: bool,
    },
    /// A line about the folder it is in: loading, empty or unreadable.
    Note {
        error: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    /// The item's name, or the text of a note.
    pub name: String,
    pub depth: usize,
    /// A symbolic link.
    pub link: bool,
    pub kind: RowKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    pub name: String,
    /// The folder that holds it, from the root.
    pub folder: String,
    pub dir: bool,
}

pub struct SearchView<'a> {
    pub hits: &'a [Hit],
    pub running: bool,
    /// The search stopped at its limit.
    pub truncated: bool,
}

pub enum PreviewBody<'a> {
    Loading,
    Text {
        lines: &'a [String],
        /// Columns of the longest line.
        widest: usize,
        truncated: bool,
        markdown: Option<&'a super::markup::Document>,
    },
    Image {
        texture: &'a egui::TextureHandle,
        pixels: [u32; 2],
    },
    Binary,
    Failed(&'a str),
}

pub struct PreviewView<'a> {
    pub path: &'a Path,
    pub name: &'a str,
    pub revision: u64,
    pub size: Option<u64>,
    pub body: PreviewBody<'a>,
}

pub struct View<'a> {
    /// The folder shown, or `None` with `notice` saying why there is none.
    pub root: Option<&'a Path>,
    pub notice: &'a str,
    pub rows: &'a [Row],
    /// The results in place of the tree while a search is typed.
    pub search: Option<SearchView<'a>>,
    pub preview: Option<PreviewView<'a>>,
    /// How much of the panel is shown while a toggle slides it.
    pub reveal: f32,
    pub window: Rect,
}

pub fn file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// An icon button that stays pressed while what it shows is open.
fn latching(ui: &mut Ui, p: Palette, icon: Icon, label: &str, on: bool) -> egui::Response {
    let surface = ui.painter().add(egui::Shape::Noop);
    let response = icons::button(ui, icon, label);
    if on {
        ui.painter().set(
            surface,
            egui::Shape::rect_filled(response.rect.shrink(1.0), 7, p.pressed),
        );
    }
    response
}

/// A single-line field keeps the keyboard when Enter is pressed in it, and
/// the key goes no further.
fn keep_focus_on_enter(ui: &Ui, response: &egui::Response) {
    if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
        ui.input_mut(|input| input.consume_key(input.modifiers, Key::Enter));
        response.request_focus();
    }
}

pub(super) fn centered_note(ui: &Ui, rect: Rect, p: Palette, text: &str) {
    let galley = ui.painter().layout(
        text.to_owned(),
        theme::regular(12.0),
        p.muted,
        (rect.width() - 24.0).max(40.0),
    );
    let pos = Pos2::new(
        rect.center().x - galley.size().x * 0.5,
        (rect.top() + 28.0).min(rect.center().y - galley.size().y * 0.5),
    );
    ui.painter()
        .with_clip_rect(rect)
        .galley(pos, galley, p.muted);
}

fn item_menu(
    ui: &mut Ui,
    p: Palette,
    path: &Path,
    dir: bool,
    found: bool,
    events: &mut Vec<Event>,
) {
    menu_layout(ui, 236.0);
    let mut chosen = None;
    let path = || path.to_path_buf();
    if dir {
        if menu_item(ui, p, Icon::FilePlus, "New file…", "", false) {
            chosen = Some(Event::BeginCreate {
                parent: Some(path()),
                folder: false,
            });
        }
        if menu_item(ui, p, Icon::FolderPlus, "New folder…", "", false) {
            chosen = Some(Event::BeginCreate {
                parent: Some(path()),
                folder: true,
            });
        }
    } else if menu_item(
        ui,
        p,
        Icon::ArrowUpRight,
        "Open with default app",
        "",
        false,
    ) {
        chosen = Some(Event::Open(path()));
    }
    menu_separator(ui, p);
    if menu_item(ui, p, Icon::Folder, REVEAL_LABEL, "", false) {
        chosen = Some(Event::Reveal(path()));
    }
    if found && menu_item(ui, p, Icon::Files, "Show in tree", "", false) {
        chosen = Some(Event::ShowInTree(path(), dir));
    }
    menu_separator(ui, p);
    if menu_item(ui, p, Icon::Copy, "Copy path", "", false) {
        chosen = Some(Event::CopyPath(path()));
    }
    if menu_item(ui, p, Icon::Copy, "Copy relative path", "", false) {
        chosen = Some(Event::CopyRelativePath(path()));
    }
    menu_separator(ui, p);
    if menu_item(ui, p, Icon::Pencil, "Rename…", "", false) {
        chosen = Some(Event::BeginRename(path()));
    }
    if menu_item(ui, p, Icon::Trash, "Delete…", "", true) {
        chosen = Some(Event::Delete(path()));
    }
    if let Some(event) = chosen {
        events.push(event);
        ui.close();
    }
}

fn root_menu(ui: &mut Ui, p: Palette, root: &Path, events: &mut Vec<Event>) {
    menu_layout(ui, 236.0);
    let mut chosen = None;
    let create = |folder| Event::BeginCreate {
        parent: Some(root.to_path_buf()),
        folder,
    };
    if menu_item(ui, p, Icon::FilePlus, "New file…", "", false) {
        chosen = Some(create(false));
    }
    if menu_item(ui, p, Icon::FolderPlus, "New folder…", "", false) {
        chosen = Some(create(true));
    }
    menu_separator(ui, p);
    if menu_item(ui, p, Icon::Folder, REVEAL_LABEL, "", false) {
        chosen = Some(Event::Reveal(root.to_path_buf()));
    }
    if menu_item(ui, p, Icon::Copy, "Copy path", "", false) {
        chosen = Some(Event::CopyPath(root.to_path_buf()));
    }
    menu_separator(ui, p);
    if menu_item(ui, p, Icon::ChevronRight, "Collapse all folders", "", false) {
        chosen = Some(Event::CollapseAll);
    }
    if menu_item(ui, p, Icon::Refresh, "Refresh", "", false) {
        chosen = Some(Event::Refresh);
    }
    if let Some(event) = chosen {
        events.push(event);
        ui.close();
    }
}

/// The surface of a row that can be clicked: selected, under the pointer or
/// under its own menu.
pub(super) fn row_surface(
    ui: &Ui,
    rect: Rect,
    p: Palette,
    response: &egui::Response,
    selected: bool,
) {
    let menu = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(response));
    let fill = if selected {
        theme::tint(p.accent, 0.18)
    } else if response.is_pointer_button_down_on() {
        p.pressed
    } else if response.hovered() || menu {
        p.hover
    } else {
        return;
    };
    ui.painter()
        .rect_filled(rect.shrink2(vec2(0.0, 1.0)), 6, fill);
}

fn item_icon(folder: bool, picture: bool) -> Icon {
    if folder {
        Icon::Folder
    } else if picture {
        Icon::Image
    } else {
        Icon::File
    }
}

fn tree_row(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    row: &Row,
    state: &mut State,
    events: &mut Vec<Event>,
) {
    let left = rect.left() + 6.0 + row.depth as f32 * INDENT;
    let middle = rect.center().y;
    let glyph = |ui: &Ui, icon: Icon, color| {
        icons::paint(
            ui.painter(),
            Rect::from_center_size(Pos2::new(left + 20.0, middle), Vec2::splat(14.0)),
            icon,
            color,
        );
    };
    let (folder, picture, expanded) = match row.kind {
        RowKind::Note { error } => {
            let painter = ui.painter();
            galley_at(
                painter,
                Pos2::new(left + 13.0, middle),
                elided(
                    painter,
                    &row.name,
                    theme::regular(12.0),
                    if error { p.red } else { p.muted },
                    rect.right() - left - 19.0,
                ),
            );
            return;
        }
        RowKind::Edit { folder } => {
            let Some(edit) = &mut state.edit else {
                return;
            };
            glyph(ui, item_icon(folder, false), p.secondary);
            let field = Rect::from_min_max(
                Pos2::new(left + 29.0, rect.top() + 1.0),
                Pos2::new(rect.right() - 4.0, rect.bottom() - 1.0),
            );
            field_frame(ui, p, field, edit_id(), edit.error.is_none());
            let inner = field.shrink2(vec2(7.0, 0.0));
            let (hint, label) = match edit.kind {
                EditKind::NewFile => ("File name", "New file name"),
                EditKind::NewFolder => ("Folder name", "New folder name"),
                EditKind::Rename => ("Name", "New name"),
            };
            let response = place(
                ui,
                inner,
                Layout::left_to_right(Align::Center),
                "explorer-edit-field",
                |ui| bare_text_edit(ui, edit_id(), &mut edit.text, hint, 12.5, inner.width()),
            );
            response.widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, label));
            if edit.focus {
                edit.focus = false;
                response.request_focus();
                // Typing replaces the name and keeps the extension.
                let name = edit.text.chars().count();
                let stem = match edit.text.rfind('.') {
                    Some(dot) if dot > 0 && !folder => edit.text[..dot].chars().count(),
                    _ => name,
                };
                let mut editor =
                    egui::text_edit::TextEditState::load(ui.ctx(), edit_id()).unwrap_or_default();
                editor
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(stem),
                    )));
                editor.store(ui.ctx(), edit_id());
            }
            if response.changed() && edit.error.take().is_some() {
                events.push(Event::Retyped);
            }
            if response.lost_focus() {
                let explicit = ui.input(|input| input.key_pressed(Key::Enter));
                if explicit {
                    // The shell must not also receive the Enter that names a file.
                    ui.input_mut(|input| input.consume_key(input.modifiers, Key::Enter));
                }
                // Clicking away uses the name. Focus taken by a shortcut, such
                // as the command palette's, leaves a half-typed name unused.
                let clicked =
                    ui.input(|input| input.pointer.any_pressed() || input.pointer.any_released());
                events.push(if explicit || clicked {
                    Event::Commit { explicit }
                } else {
                    Event::CancelEdit
                });
            } else if !ui.memory(|memory| memory.has_focus(edit_id())) {
                // Another surface took the keyboard without a frame in between.
                // The window losing focus is not that: the name stays.
                events.push(Event::CancelEdit);
            }
            return;
        }
        RowKind::Folder { expanded } => (true, false, expanded),
        RowKind::File { picture } => (false, picture, false),
    };
    let response = ui.interact(rect, Id::new(("explorer-row", &row.path)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &row.name));
    let selected = state
        .selected
        .as_ref()
        .is_some_and(|(path, _)| *path == row.path);
    row_surface(ui, rect, p, &response, selected);
    if folder {
        icons::paint(
            ui.painter(),
            Rect::from_center_size(Pos2::new(left + 5.0, middle), Vec2::splat(9.0)),
            if expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            },
            p.muted,
        );
    }
    glyph(ui, item_icon(folder, picture), p.secondary);
    let right = rect.right() - if row.link { 22.0 } else { 8.0 };
    let painter = ui.painter();
    let name = elided(
        painter,
        &row.name,
        theme::regular(12.5),
        p.fg,
        right - left - 31.0,
    );
    let cut = name.elided;
    galley_at(painter, Pos2::new(left + 31.0, middle), name);
    if row.link {
        icons::paint(
            painter,
            Rect::from_center_size(Pos2::new(rect.right() - 12.0, middle), Vec2::splat(10.0)),
            Icon::ArrowUpRight,
            p.muted,
        );
    }
    if response.clicked() {
        events.push(if folder {
            Event::Expand(row.path.clone(), !expanded)
        } else {
            Event::Select(row.path.clone())
        });
    }
    response.context_menu(|ui| item_menu(ui, p, &row.path, folder, false, events));
    if cut || row.link {
        response.on_hover_text(if row.link {
            format!("{} (link)", row.name)
        } else {
            row.name.clone()
        });
    }
}

fn hit_row(ui: &mut Ui, rect: Rect, p: Palette, hit: &Hit, state: &State, events: &mut Vec<Event>) {
    let response = ui.interact(rect, Id::new(("explorer-hit", &hit.path)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &hit.name));
    let selected = state
        .selected
        .as_ref()
        .is_some_and(|(path, _)| *path == hit.path);
    row_surface(ui, rect, p, &response, selected);
    let middle = rect.center().y;
    let painter = ui.painter();
    icons::paint(
        painter,
        Rect::from_center_size(Pos2::new(rect.left() + 15.0, middle), Vec2::splat(14.0)),
        item_icon(hit.dir, false),
        p.secondary,
    );
    let left = rect.left() + 28.0;
    let budget = rect.right() - 8.0 - left;
    let name = elided(painter, &hit.name, theme::regular(12.5), p.fg, budget);
    let used = name.size().x;
    galley_at(painter, Pos2::new(left, middle), name);
    if !hit.folder.is_empty() && budget - used > 36.0 {
        galley_at(
            painter,
            Pos2::new(left + used + 8.0, middle + 0.5),
            elided(
                painter,
                &hit.folder,
                theme::regular(11.5),
                p.muted,
                budget - used - 8.0,
            ),
        );
    }
    if response.clicked() {
        events.push(if hit.dir {
            Event::ShowInTree(hit.path.clone(), true)
        } else {
            Event::Select(hit.path.clone())
        });
    }
    response.context_menu(|ui| item_menu(ui, p, &hit.path, hit.dir, true, events));
    response.on_hover_text(if hit.folder.is_empty() {
        hit.name.clone()
    } else {
        format!("{}/{}", hit.folder, hit.name)
    });
}

/// The tree, or the search results in its place.
fn list(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &View,
    root: &Path,
    state: &mut State,
    events: &mut Vec<Event>,
) {
    // Registered first so the rows over it take their own clicks and menus.
    let background = ui.interact(rect, ui.id().with("explorer-background"), Sense::click());
    if background.clicked() {
        state.selected = None;
    }
    background.context_menu(|ui| root_menu(ui, p, root, events));
    let mut ui = ui.new_child(UiBuilder::new().id_salt("explorer-list").max_rect(rect));
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let ui = &mut ui;
    if let Some(search) = &view.search {
        let status = if search.running && search.hits.is_empty() {
            "Searching…".to_owned()
        } else if search.hits.is_empty() {
            "No files match".to_owned()
        } else {
            format!(
                "{}{} result{}{}",
                search.hits.len(),
                if search.truncated { "+" } else { "" },
                if search.hits.len() == 1 { "" } else { "s" },
                if search.running { "…" } else { "" },
            )
        };
        let (_, line) = ui.allocate_space(vec2(ui.available_width(), 20.0));
        ui.painter().text(
            Pos2::new(line.left() + 6.0, line.center().y),
            Align2::LEFT_CENTER,
            status,
            theme::regular(11.0),
            p.muted,
        );
        egui::ScrollArea::vertical()
            .id_salt("explorer-hits")
            .auto_shrink([false, false])
            .show_rows(ui, ROW, search.hits.len(), |ui, range| {
                for hit in &search.hits[range] {
                    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW));
                    hit_row(ui, rect, p, hit, state, events);
                }
            });
        return;
    }
    let target = state.scroll_to.as_ref().and_then(|target| {
        view.rows.iter().position(|row| match target {
            ScrollTarget::Path(path) => {
                row.path == *path && !matches!(row.kind, RowKind::Note { .. })
            }
            ScrollTarget::Edit => matches!(row.kind, RowKind::Edit { .. }),
        })
    });
    let mut area = egui::ScrollArea::vertical()
        .id_salt("explorer-rows")
        .auto_shrink([false, false]);
    let jump = std::mem::take(&mut state.scroll_jump);
    if jump && let Some(index) = target {
        area = area.vertical_scroll_offset((index as f32 * ROW - rect.height() * 0.4).max(0.0));
        state.scroll_to = None;
    }
    area.show_rows(ui, ROW, view.rows.len(), |ui, range| {
        if let Some(index) = target.filter(|_| !jump) {
            if range.contains(&index) {
                state.scroll_to = None;
            } else {
                // Its row is not laid out; go to where it will be.
                state.scroll_jump = true;
                ui.ctx().request_repaint();
            }
        }
        for index in range {
            let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW));
            if target == Some(index) && !jump {
                ui.scroll_to_rect(rect, None);
            }
            tree_row(ui, rect, p, &view.rows[index], state, events);
        }
    });
}

fn preview(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &PreviewView,
    state: &mut State,
    events: &mut Vec<Event>,
) {
    ui.painter().rect_filled(rect, metrics::PANE_RADIUS, p.bg);
    let header = Rect::from_min_size(rect.min, vec2(rect.width(), 32.0));
    let middle = header.center().y;
    let picture = matches!(view.body, PreviewBody::Image { .. });
    let markdown = match &view.body {
        PreviewBody::Text { markdown, .. } => *markdown,
        _ => None,
    };
    let left = place(
        ui,
        header.shrink2(vec2(3.0, 0.0)),
        Layout::right_to_left(Align::Center),
        "explorer-preview-actions",
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if icons::button(ui, Icon::Close, "Close preview").clicked() {
                events.push(Event::ClosePreview);
            }
            if icons::button(ui, Icon::ArrowUpRight, "Open with default app").clicked() {
                events.push(Event::Open(view.path.to_path_buf()));
            }
            if markdown.is_some() {
                let response = latching(
                    ui,
                    p,
                    Icon::Code,
                    "Toggle Markdown source / preview",
                    !state.markdown_preview,
                );
                if response.clicked() {
                    state.markdown_preview = !state.markdown_preview;
                    state.source_selection.reset();
                    ui.ctx().with_plugin(
                        |labels: &mut egui::text_selection::LabelSelectionState| {
                            labels.clear_selection()
                        },
                    );
                }
            }
            ui.min_rect().left()
        },
    );
    let painter = ui.painter().with_clip_rect(rect);
    icons::paint(
        &painter,
        Rect::from_center_size(Pos2::new(rect.left() + 16.0, middle), Vec2::splat(14.0)),
        item_icon(false, picture),
        p.secondary,
    );
    let budget = left - 6.0 - (rect.left() + 29.0);
    let name = elided(&painter, view.name, theme::medium(12.0), p.fg, budget);
    let used = name.size().x;
    galley_at(&painter, Pos2::new(rect.left() + 29.0, middle), name);
    let mut details = Vec::new();
    if let PreviewBody::Image { pixels, .. } = view.body {
        details.push(format!("{} × {}", pixels[0], pixels[1]));
    }
    details.extend(view.size.map(file_size));
    if !details.is_empty() && budget - used > 40.0 {
        galley_at(
            &painter,
            Pos2::new(rect.left() + 29.0 + used + 8.0, middle + 0.5),
            elided(
                &painter,
                &details.join(" · "),
                theme::regular(11.0),
                p.muted,
                budget - used - 8.0,
            ),
        );
    }
    let body = Rect::from_min_max(
        Pos2::new(rect.left() + 2.0, header.bottom()),
        Pos2::new(rect.right() - 2.0, rect.bottom() - 4.0),
    );
    if body.height() < 8.0 {
        return;
    }
    if let Some(document) = markdown.filter(|_| state.markdown_preview) {
        let mut child = ui.new_child(
            UiBuilder::new()
                .id_salt(("explorer-markdown", view.path))
                .max_rect(body.shrink2(vec2(10.0, 4.0))),
        );
        child.set_clip_rect(body.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical()
            .id_salt(("explorer-markdown-scroll", view.path))
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Never,
                ..Default::default()
            })
            .show(&mut child, |ui| {
                let mut actions = Vec::new();
                super::markup::show_document(
                    ui,
                    p,
                    document,
                    view.path,
                    state.markdown_anchor.as_deref(),
                    &mut actions,
                );
                state.markdown_anchor = None;
                for action in actions {
                    match action {
                        Action::Explorer(event) => events.push(event),
                        Action::OpenLink(link) => events.push(Event::OpenLink(link)),
                        _ => {}
                    }
                }
                if matches!(
                    view.body,
                    PreviewBody::Text {
                        truncated: true,
                        ..
                    }
                ) {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("The source preview is truncated")
                            .color(p.muted)
                            .size(11.0),
                    );
                }
            });
        if document.blocks.is_empty() {
            centered_note(ui, body, p, "Empty file");
        }
        return;
    }
    if ui.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pos| !body.contains(pos))
    }) && !egui::Popup::is_any_open(ui.ctx())
    {
        state.source_selection.reset();
    }
    match &view.body {
        PreviewBody::Loading => {}
        PreviewBody::Failed(message) => centered_note(ui, body, p, message),
        PreviewBody::Binary => centered_note(ui, body, p, "No preview for this kind of file"),
        PreviewBody::Text { lines: [], .. } => {
            centered_note(ui, body, p, "Empty file");
        }
        PreviewBody::Text {
            lines,
            widest,
            truncated,
            ..
        } => {
            if state
                .preview_location
                .is_some_and(|(line, _)| line as usize > lines.len())
            {
                let message = if *truncated {
                    "This line is beyond the preview. Choose an external editor in Preferences to open it."
                } else {
                    "This line is beyond the end of the file."
                };
                centered_note(ui, body, p, message);
                return;
            }
            let font = egui::FontId::monospace(11.5);
            let column = painter
                .layout_no_wrap("0".into(), font.clone(), p.fg)
                .size()
                .x;
            let height = (font.size * 1.5).round();
            let gutter = lines.len().to_string().len() as f32 * column + 20.0;
            let width = gutter + *widest as f32 * column + 12.0;
            let mut ui = ui.new_child(
                UiBuilder::new()
                    .id_salt("explorer-preview-text")
                    .max_rect(body),
            );
            ui.set_clip_rect(body.intersect(ui.clip_rect()));
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let location = state.preview_location;
            let mut scroll = egui::ScrollArea::both();
            if state.preview_jump
                && let Some((line, col)) = location
            {
                scroll = scroll
                    .vertical_scroll_offset(
                        (line.saturating_sub(1) as usize).min(lines.len().saturating_sub(1)) as f32
                            * height,
                    )
                    .horizontal_scroll_offset(
                        (col.saturating_sub(1) as f32 * column - body.width() / 2.0).max(0.0),
                    );
            }
            let scrolled = scroll
                .id_salt(("explorer-preview-lines", view.path))
                .auto_shrink([false, false])
                .scroll_source(egui::scroll_area::ScrollSource {
                    drag: egui::scroll_area::DragScroll::Never,
                    ..Default::default()
                })
                .show_viewport(&mut ui, |ui, viewport| {
                    let count = lines.len() + usize::from(*truncated);
                    let (_, content) = ui.allocate_space(vec2(
                        width.max(ui.available_width()),
                        count as f32 * height,
                    ));
                    let response = ui
                        .interact(
                            content.intersect(ui.clip_rect()),
                            ui.id().with(("source-selection", view.path)),
                            Sense::click_and_drag() - Sense::FOCUSABLE,
                        )
                        .on_hover_cursor(CursorIcon::Text);
                    response.widget_info(|| {
                        WidgetInfo::labeled(WidgetType::Label, true, "File source")
                    });
                    state.source_selection.sync(view.path, view.revision);
                    state.source_selection.update(
                        ui,
                        &response,
                        lines,
                        content.min + vec2(gutter, 0.0),
                        height,
                        &font,
                    );
                    state.source_selection.menu(&response, lines, p);
                    let range = (viewport.top() / height).floor().max(0.0) as usize
                        ..((viewport.bottom() / height).ceil() as usize).min(count);
                    for index in range {
                        let line = Rect::from_min_size(
                            content.min + vec2(0.0, index as f32 * height),
                            vec2(content.width(), height),
                        );
                        let painter = ui.painter();
                        if let Some((target, col)) = location
                            && index + 1 == target as usize
                        {
                            painter.rect_filled(line, 0, p.accent.gamma_multiply(0.15));
                            let x = line.left()
                                + gutter
                                + (col.saturating_sub(1) as usize).min(lines[index].chars().count())
                                    as f32
                                    * column;
                            painter.line_segment(
                                [egui::pos2(x, line.top()), egui::pos2(x, line.bottom())],
                                egui::Stroke::new(1.5, p.accent),
                            );
                        }
                        let Some(text) = lines.get(index) else {
                            painter.text(
                                Pos2::new(line.left() + gutter, line.center().y),
                                Align2::LEFT_CENTER,
                                "The rest of the file is not shown",
                                theme::regular(11.0),
                                p.muted,
                            );
                            continue;
                        };
                        painter.text(
                            Pos2::new(line.left() + gutter - 10.0, line.center().y),
                            Align2::RIGHT_CENTER,
                            index + 1,
                            font.clone(),
                            p.muted,
                        );
                        let galley = painter.layout_no_wrap(text.clone(), font.clone(), p.fg);
                        let origin = Pos2::new(line.left() + gutter, line.top());
                        state
                            .source_selection
                            .paint(ui, index, origin, &galley, height, p);
                        painter.galley(
                            origin + vec2(0.0, (height - galley.size().y) * 0.5),
                            galley,
                            p.fg,
                        );
                    }
                });
            if state.preview_jump && !ui.is_sizing_pass() && !ui.ctx().will_discard() {
                let desired = location.map_or(0.0, |(line, _)| {
                    (line.saturating_sub(1) as usize).min(lines.len().saturating_sub(1)) as f32
                        * height
                });
                let expected =
                    desired.min((scrolled.content_size.y - scrolled.inner_rect.height()).max(0.0));
                if (scrolled.state.offset.y - expected).abs() < 1.0 {
                    state.preview_jump = false;
                } else {
                    ui.ctx().request_repaint();
                }
            }
        }
        PreviewBody::Image { texture, pixels } => {
            let room = body.shrink(8.0);
            let natural = vec2(pixels[0].max(1) as f32, pixels[1].max(1) as f32);
            // A small picture is enlarged a little; a large one fits the room.
            let enlarge = if natural.max_elem() < 64.0 { 4.0 } else { 1.0 };
            let scale = (room.width() / natural.x)
                .min(room.height() / natural.y)
                .min(enlarge);
            if scale > 0.0 {
                egui::Image::from_texture(egui::load::SizedTexture::new(
                    texture.id(),
                    natural * scale,
                ))
                .corner_radius(4)
                .paint_at(ui, Rect::from_center_size(room.center(), natural * scale));
            }
        }
    }
}

/// A divider that is dragged to resize what it separates.
pub(super) fn divider(
    ui: &mut Ui,
    p: Palette,
    handle: Rect,
    name: &str,
    label: &str,
) -> egui::Response {
    let vertical = handle.height() > handle.width();
    let cursor = if vertical {
        CursorIcon::ResizeHorizontal
    } else {
        CursorIcon::ResizeVertical
    };
    let response = ui
        .interact(handle, ui.id().with(name), Sense::click_and_drag())
        .on_hover_and_drag_cursor(cursor);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::ResizeHandle, true, label));
    let grip = animate(
        ui.ctx(),
        response.id.with("grip"),
        response.hovered() || response.dragged(),
        0.12,
    );
    if grip > 0.0 {
        let centre = handle.center();
        let ends = if vertical {
            [
                Pos2::new(centre.x, handle.top() + 8.0),
                Pos2::new(centre.x, handle.bottom() - 8.0),
            ]
        } else {
            [
                Pos2::new(handle.left() + 8.0, centre.y),
                Pos2::new(handle.right() - 8.0, centre.y),
            ]
        };
        ui.painter().line_segment(
            ends,
            Stroke::new(
                2.0,
                theme::tint(
                    if response.dragged() {
                        p.accent
                    } else {
                        p.muted
                    },
                    grip * 0.8,
                ),
            ),
        );
    }
    response
}

/// The explorer in `rect`, below the panel's tabs.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &View,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let mut events = Vec::new();
    let mut child = ui.new_child(UiBuilder::new().id_salt("file-explorer").max_rect(rect));
    child.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(view.window));
    child.multiply_opacity(view.reveal);
    contents(&mut child, rect, p, view, state, &mut events);
    // An edit ends before anything else it shared a frame with begins.
    events.sort_by_key(|event| !matches!(event, Event::Commit { .. } | Event::CancelEdit));
    actions.extend(events.into_iter().map(Action::Explorer));
}

fn contents(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &View,
    state: &mut State,
    events: &mut Vec<Event>,
) {
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 6.0, rect.top()),
        Pos2::new(rect.right() - 8.0, rect.bottom() - 8.0),
    );
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), 30.0));
    let buttons = place(
        ui,
        header,
        Layout::right_to_left(Align::Center),
        "explorer-actions",
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if view.root.is_none() {
                ui.disable();
            }
            if icons::button(ui, Icon::Refresh, "Refresh").clicked() {
                events.push(Event::Refresh);
            }
            if icons::button(ui, Icon::FolderPlus, "New folder").clicked() {
                events.push(Event::BeginCreate {
                    parent: None,
                    folder: true,
                });
            }
            if icons::button(ui, Icon::FilePlus, "New file").clicked() {
                events.push(Event::BeginCreate {
                    parent: None,
                    folder: false,
                });
            }
            ui.min_rect().left()
        },
    );
    let title = view.root.map_or_else(
        || "Files".to_owned(),
        |root| {
            root.file_name().map_or_else(
                || root.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        },
    );
    let title = elided(
        ui.painter(),
        &title,
        theme::medium(12.5),
        p.fg,
        buttons - inner.left() - 12.0,
    );
    let named = galley_at(
        ui.painter(),
        Pos2::new(inner.left() + 6.0, header.center().y),
        title,
    );
    let Some(root) = view.root else {
        centered_note(
            ui,
            Rect::from_min_max(Pos2::new(inner.left(), header.bottom()), inner.max),
            p,
            view.notice,
        );
        return;
    };
    ui.interact(
        named.expand2(vec2(4.0, 6.0)),
        ui.id().with("explorer-root"),
        Sense::hover(),
    )
    .on_hover_text(compact_path(root));

    // Search, with the exclusions a search uses under it.
    let field = Rect::from_min_size(
        Pos2::new(inner.left(), header.bottom() + 2.0),
        vec2(inner.width(), metrics::CONTROL_HEIGHT),
    );
    field_frame(ui, p, field, query_id(), true);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(
            Pos2::new(field.left() + 15.0, field.center().y),
            Vec2::splat(13.0),
        ),
        Icon::Search,
        p.muted,
    );
    place(
        ui,
        Rect::from_min_max(
            Pos2::new(field.left() + 29.0, field.top()),
            Pos2::new(field.right() - 1.0, field.bottom()),
        ),
        Layout::right_to_left(Align::Center),
        "explorer-query-field",
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if latching(ui, p, Icon::Ellipsis, "Files to exclude", state.filters).clicked() {
                state.filters = !state.filters;
            }
            if !state.query.is_empty() && icons::button(ui, Icon::Close, "Clear search").clicked() {
                state.query.clear();
                events.push(Event::SearchChanged);
            }
            let input = ui.available_rect_before_wrap();
            let response = place(
                ui,
                input,
                Layout::left_to_right(Align::Center),
                "explorer-query-input",
                |ui| {
                    bare_text_edit(
                        ui,
                        query_id(),
                        &mut state.query,
                        "Search files",
                        12.5,
                        input.width(),
                    )
                },
            );
            response
                .widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, "Search files"));
            if response.changed() {
                events.push(Event::SearchChanged);
            }
            keep_focus_on_enter(ui, &response);
        },
    );
    let filtering = state.filters || view.search.is_some();
    if !filtering {
        // A hidden field must not keep the keyboard.
        ui.memory_mut(|memory| {
            if memory.has_focus(exclude_id()) {
                memory.surrender_focus(exclude_id());
            }
        });
    }
    const FILTERS: f32 = 54.0;
    let shown = animate(ui.ctx(), Id::new("explorer-filters"), filtering, 0.14);
    let filters = (shown * FILTERS).round();
    if shown > 0.0 {
        let block = Rect::from_min_size(
            Pos2::new(inner.left(), field.bottom() + filters - FILTERS),
            vec2(inner.width(), FILTERS),
        );
        let mut ui = ui.new_child(UiBuilder::new().id_salt("explorer-filters").max_rect(block));
        ui.set_clip_rect(
            Rect::from_min_max(
                Pos2::new(block.left() - 4.0, field.bottom()),
                Pos2::new(block.right() + 4.0, block.bottom() + 4.0),
            )
            .intersect(ui.clip_rect()),
        );
        ui.multiply_opacity(shown);
        ui.painter().text(
            Pos2::new(block.left() + 6.0, block.top() + 13.0),
            Align2::LEFT_CENTER,
            "files to exclude",
            theme::regular(11.0),
            p.muted,
        );
        let exclude = Rect::from_min_size(
            Pos2::new(block.left(), block.top() + 22.0),
            vec2(block.width(), metrics::CONTROL_HEIGHT),
        );
        field_frame(&ui, p, exclude, exclude_id(), true);
        let input = exclude.shrink2(vec2(10.0, 0.0));
        let response = place(
            &mut ui,
            input,
            Layout::left_to_right(Align::Center),
            "explorer-exclude-input",
            |ui| {
                bare_text_edit(
                    ui,
                    exclude_id(),
                    &mut state.exclude,
                    "e.g. **/node_modules/**, *.log",
                    12.0,
                    input.width(),
                )
            },
        );
        response
            .widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, "Files to exclude"));
        if response.changed() {
            events.push(Event::SearchChanged);
        }
        keep_focus_on_enter(&ui, &response);
    }

    let body = Rect::from_min_max(
        Pos2::new(inner.left(), field.bottom() + filters + 6.0),
        inner.max,
    );
    if body.height() < ROW {
        return;
    }
    // The preview takes its share from the bottom, leaving the tree in view.
    let previewing = animate(
        ui.ctx(),
        Id::new("explorer-preview"),
        view.preview.is_some(),
        0.16,
    );
    let total = body.height();
    let share = (state.preview_share * total)
        .clamp((total * 0.5).min(110.0), (total - 90.0).max(total * 0.5));
    let height = (share * previewing).round();
    const GAP: f32 = 6.0;
    let split = body.bottom() - height;
    let tree = Rect::from_min_max(
        body.min,
        Pos2::new(body.right(), split - if height > 0.0 { GAP } else { 0.0 }),
    );
    list(ui, tree, p, view, root, state, events);
    if height > 0.0 {
        let surface = Rect::from_min_max(Pos2::new(body.left(), split), body.max);
        match &view.preview {
            Some(preview) if previewing >= 1.0 => {
                self::preview(ui, surface, p, preview, state, events)
            }
            // Opening or closing: the surface alone, without content to reflow.
            _ => {
                ui.painter()
                    .rect_filled(surface, metrics::PANE_RADIUS, p.bg);
            }
        }
        if view.preview.is_some() && previewing >= 1.0 {
            let handle = Rect::from_min_max(
                Pos2::new(body.left(), split - GAP),
                Pos2::new(body.right(), split),
            );
            let resize = divider(ui, p, handle, "explorer-preview-resize", "Resize preview");
            if resize.dragged()
                && let Some(pointer) = resize.interact_pointer_pos()
            {
                state.preview_share = ((body.bottom() - pointer.y) / total).clamp(0.15, 0.85);
            }
            if resize.double_clicked() {
                state.preview_share = State::default().preview_share;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::panel::DEFAULT_WIDTH;
    use eframe::egui::{Event as Input, Modifiers, PointerButton};

    fn run(ctx: &egui::Context, events: Vec<Input>, view: &View, state: &mut State) -> Vec<Event> {
        let mut actions = Vec::new();
        let p = Palette::for_config(&crate::config::Config::default());
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(view.window),
                events,
                ..Default::default()
            },
            |ui| {
                let rect = Rect::from_min_max(
                    Pos2::new(view.window.right() - DEFAULT_WIDTH, 44.0),
                    view.window.max,
                );
                show(ui, rect, p, view, state, &mut actions);
            },
        );
        output.textures_delta.clear();
        actions
            .into_iter()
            .filter_map(|action| match action {
                Action::Explorer(event) => Some(event),
                _ => None,
            })
            .collect()
    }

    fn click(pos: Pos2, button: PointerButton) -> Vec<Vec<Input>> {
        let press = |pressed| Input::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        };
        vec![
            vec![Input::PointerMoved(pos)],
            vec![press(true)],
            vec![press(false)],
        ]
    }

    fn rows() -> Vec<Row> {
        vec![
            Row {
                path: "/p/src".into(),
                name: "src".into(),
                depth: 0,
                link: false,
                kind: RowKind::Folder { expanded: false },
            },
            Row {
                path: "/p/.env".into(),
                name: ".env".into(),
                depth: 0,
                link: false,
                kind: RowKind::File { picture: false },
            },
        ]
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        theme::apply(&ctx, &crate::config::Config::default());
        ctx
    }

    const WINDOW: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(900.0, 640.0));

    /// Where the tree's first row is with the default width and no filters.
    fn row(index: usize) -> Pos2 {
        Pos2::new(
            WINDOW.right() - DEFAULT_WIDTH + 80.0,
            44.0 + 30.0 + 2.0 + 30.0 + 6.0 + ROW * (index as f32 + 0.5),
        )
    }

    #[test]
    fn clicking_rows_opens_folders_and_previews_files() {
        let ctx = context();
        let rows = rows();
        let view = View {
            root: Some(Path::new("/p")),
            notice: "",
            rows: &rows,
            search: None,
            preview: None,
            reveal: 1.0,
            window: WINDOW,
        };
        let mut state = State::default();
        run(&ctx, vec![], &view, &mut state);
        let mut seen = Vec::new();
        for index in 0..2 {
            for events in click(row(index), PointerButton::Primary) {
                seen.extend(run(&ctx, events, &view, &mut state));
            }
        }
        assert_eq!(
            seen,
            [
                Event::Expand("/p/src".into(), true),
                Event::Select("/p/.env".into())
            ]
        );
    }

    #[test]
    fn a_rows_menu_offers_every_file_action_and_reports_the_chosen_one() {
        let ctx = context();
        let rows = rows();
        let view = View {
            root: Some(Path::new("/p")),
            notice: "",
            rows: &rows,
            search: None,
            preview: None,
            reveal: 1.0,
            window: WINDOW,
        };
        let mut state = State::default();
        run(&ctx, vec![], &view, &mut state);
        for events in click(row(1), PointerButton::Secondary) {
            assert!(run(&ctx, events, &view, &mut state).is_empty());
        }
        let mut labels = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(WINDOW),
                ..Default::default()
            },
            |ui| {
                let rect = Rect::from_min_max(Pos2::new(600.0, 44.0), WINDOW.max);
                let p = Palette::for_config(&crate::config::Config::default());
                show(ui, rect, p, &view, &mut state, &mut Vec::new());
            },
        );
        output.textures_delta.clear();
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape {
                labels.push(text.galley.text().to_owned());
            }
        }
        for label in [
            "Open with default app",
            REVEAL_LABEL,
            "Copy path",
            "Copy relative path",
            "Rename…",
            "Delete…",
        ] {
            assert!(
                labels.iter().any(|text| text == label),
                "{label}: {labels:?}"
            );
        }
        assert!(
            !labels.iter().any(|text| text == "New file…"),
            "a file holds nothing"
        );
    }

    #[test]
    fn enter_commits_a_typed_name_once_and_keeps_the_key_from_the_shell() {
        let ctx = context();
        let mut rows = rows();
        rows.insert(
            0,
            Row {
                path: "/p".into(),
                name: String::new(),
                depth: 0,
                link: false,
                kind: RowKind::Edit { folder: false },
            },
        );
        let view = View {
            root: Some(Path::new("/p")),
            notice: "",
            rows: &rows,
            search: None,
            preview: None,
            reveal: 1.0,
            window: WINDOW,
        };
        let mut state = State {
            edit: Some(Edit {
                kind: EditKind::NewFile,
                parent: "/p".into(),
                target: None,
                text: String::new(),
                focus: true,
                error: None,
            }),
            ..Default::default()
        };
        run(&ctx, vec![], &view, &mut state);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(edit_id()));
        assert!(
            run(
                &ctx,
                vec![Input::Text("notes.md".into())],
                &view,
                &mut state
            )
            .is_empty()
        );
        assert_eq!(state.edit.as_ref().unwrap().text, "notes.md");
        let enter = Input::Key {
            key: Key::Enter,
            physical_key: Some(Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let mut actions = Vec::new();
        let mut leaked = true;
        let p = Palette::for_config(&crate::config::Config::default());
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(WINDOW),
                events: vec![enter],
                ..Default::default()
            },
            |ui| {
                let rect = Rect::from_min_max(Pos2::new(600.0, 44.0), WINDOW.max);
                show(ui, rect, p, &view, &mut state, &mut actions);
                leaked = ui.input(|input| input.key_pressed(Key::Enter));
            },
        );
        output.textures_delta.clear();
        assert!(matches!(
            actions.as_slice(),
            [Action::Explorer(Event::Commit { explicit: true })]
        ));

        // Clicking away uses the name; losing the keyboard otherwise does not.
        state.edit.as_mut().unwrap().focus = true;
        run(&ctx, vec![], &view, &mut state);
        let mut seen = Vec::new();
        for events in click(Pos2::new(700.0, 500.0), PointerButton::Primary) {
            seen.extend(run(&ctx, events, &view, &mut state));
        }
        assert_eq!(seen, [Event::Commit { explicit: false }]);
        state.edit.as_mut().unwrap().focus = true;
        run(&ctx, vec![], &view, &mut state);
        ctx.memory_mut(|memory| memory.surrender_focus(edit_id()));
        assert_eq!(run(&ctx, vec![], &view, &mut state), [Event::CancelEdit]);
        assert!(
            !leaked,
            "Enter stayed in the frame's input for the terminal"
        );
    }

    #[test]
    fn sizes_read_in_the_nearest_unit() {
        assert_eq!(file_size(0), "0 B");
        assert_eq!(file_size(1023), "1023 B");
        assert_eq!(file_size(1536), "1.5 KB");
        assert_eq!(file_size(20 * 1024 * 1024), "20 MB");
        assert_eq!(file_size(u64::MAX), "16777216 TB");
    }
}

#[cfg(test)]
mod location_tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        view: &PreviewView,
        state: &mut State,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0))),
                events,
                ..Default::default()
            },
            |ui| {
                preview(
                    ui,
                    Rect::from_min_size(Pos2::ZERO, vec2(300.0, 240.0)),
                    Palette::new(crate::config::Theme::Graphite),
                    view,
                    state,
                    &mut Vec::new(),
                )
            },
        );
        output.textures_delta.clear();
        output
    }

    fn painted(output: &egui::FullOutput, text: &str) -> Option<Rect> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(shape) if shape.galley.text() == text => {
                Some(shape.visual_bounding_rect())
            }
            _ => None,
        })
    }

    fn drag(ctx: &egui::Context, view: &PreviewView, state: &mut State, from: Pos2, to: Pos2) {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![egui::Event::PointerMoved(from)],
            vec![button(from, true)],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
        ] {
            frame(ctx, view, state, events);
        }
    }

    #[test]
    fn source_selection_copies_multiple_lines_and_survives_scrolling() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let lines: Vec<String> = (1..=100)
            .map(|line| format!("source line {line}"))
            .collect();
        let view = PreviewView {
            path: Path::new("/tmp/source.rs"),
            name: "source.rs",
            revision: 1,
            size: None,
            body: PreviewBody::Text {
                lines: &lines,
                widest: 15,
                truncated: false,
                markdown: None,
            },
        };
        let mut state = State::default();
        let output = frame(&ctx, &view, &mut state, Vec::new());
        let from = painted(&output, "source line 1").unwrap().left_center();
        let to = painted(&output, "source line 3").unwrap().right_center() + vec2(1.0, 0.0);
        drag(&ctx, &view, &mut state, from, to);
        let output = frame(&ctx, &view, &mut state, vec![egui::Event::Copy]);
        assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "source line 1\nsource line 2\nsource line 3")));
        let context = egui::Event::PointerButton {
            pos: from + vec2(30.0, 17.0),
            pressed: true,
            button: egui::PointerButton::Secondary,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &ctx,
            &view,
            &mut state,
            vec![egui::Event::PointerMoved(from + vec2(30.0, 17.0))],
        );
        frame(&ctx, &view, &mut state, vec![context.clone()]);
        let mut release = context;
        if let egui::Event::PointerButton { pressed, .. } = &mut release {
            *pressed = false;
        }
        frame(&ctx, &view, &mut state, vec![release]);
        let output = frame(&ctx, &view, &mut state, Vec::new());
        let copy = painted(&output, "Copy").unwrap().center();
        frame(
            &ctx,
            &view,
            &mut state,
            vec![egui::Event::PointerMoved(copy)],
        );
        let output = frame(
            &ctx,
            &view,
            &mut state,
            vec![
                egui::Event::PointerButton {
                    pos: copy,
                    pressed: true,
                    button: egui::PointerButton::Primary,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: copy,
                    pressed: false,
                    button: egui::PointerButton::Primary,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "source line 1\nsource line 2\nsource line 3")), "copying through the menu must preserve the selection");
        frame(
            &ctx,
            &view,
            &mut state,
            vec![egui::Event::MouseWheel {
                phase: egui::TouchPhase::Move,
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -600.0),
                modifiers: egui::Modifiers::NONE,
            }],
        );
        for _ in 0..12 {
            frame(&ctx, &view, &mut state, Vec::new());
        }
        let output = frame(&ctx, &view, &mut state, vec![egui::Event::Copy]);
        assert!(
            state
                .source_selection
                .has_selection(view.path, view.revision)
        );
        assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "source line 1\nsource line 2\nsource line 3")));
        assert!(
            painted(&output, "source line 1").is_none(),
            "offscreen lines stay virtualized"
        );
    }

    #[test]
    fn markdown_switch_shows_source_or_formatted_selectable_text() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let lines = vec!["# Heading".into(), "".into(), "Some **bold** text.".into()];
        let blocks =
            super::super::markup::blocks(&lines.join("\n"), super::super::markup::Lines::Joined);
        let document = super::super::markup::Document::new(blocks);
        let view = PreviewView {
            path: Path::new("/tmp/notes.md"),
            name: "notes.md",
            revision: 1,
            size: None,
            body: PreviewBody::Text {
                lines: &lines,
                widest: 19,
                truncated: false,
                markdown: Some(&document),
            },
        };
        let mut state = State::default();
        let output = frame(&ctx, &view, &mut state, Vec::new());
        let heading = painted(&output, "Heading").unwrap();
        assert!(painted(&output, "Some bold text.").is_some());
        drag(
            &ctx,
            &view,
            &mut state,
            heading.left_center(),
            heading.right_center() + vec2(1.0, 0.0),
        );
        let output = frame(&ctx, &view, &mut state, vec![egui::Event::Copy]);
        assert!(output.platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "Heading")
        ));
        // The third 28-point icon is left of Open and Close in the header.
        let source = Pos2::new(300.0 - 3.0 - 2.0 * 28.0 - 14.0, 16.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: source,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &ctx,
            &view,
            &mut state,
            vec![egui::Event::PointerMoved(source)],
        );
        frame(&ctx, &view, &mut state, vec![button(true)]);
        let output = frame(&ctx, &view, &mut state, vec![button(false)]);
        assert!(!state.markdown_preview);
        assert!(painted(&output, "# Heading").is_some());
        assert!(painted(&output, "Some **bold** text.").is_some());
        frame(&ctx, &view, &mut state, vec![button(true)]);
        let output = frame(&ctx, &view, &mut state, vec![button(false)]);
        assert!(state.markdown_preview);
        assert!(painted(&output, "Heading").is_some());
        assert!(painted(&output, "Some bold text.").is_some());
    }

    #[test]
    fn markdown_scroll_survives_a_file_revision_change() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let lines: Vec<String> = (0..80)
            .map(|index| format!("Paragraph {index}\n"))
            .collect();
        let document = super::super::markup::Document::new(super::super::markup::document_blocks(
            &lines.join("\n"),
        ));
        let mut view = PreviewView {
            path: Path::new("/tmp/scroll.md"),
            name: "scroll.md",
            revision: 1,
            size: None,
            body: PreviewBody::Text {
                lines: &lines,
                widest: 15,
                truncated: false,
                markdown: Some(&document),
            },
        };
        let mut state = State::default();
        frame(&ctx, &view, &mut state, Vec::new());
        frame(
            &ctx,
            &view,
            &mut state,
            vec![
                egui::Event::PointerMoved(Pos2::new(140.0, 180.0)),
                egui::Event::MouseWheel {
                    phase: egui::TouchPhase::Move,
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, -220.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        for _ in 0..12 {
            frame(&ctx, &view, &mut state, Vec::new());
        }
        let before = frame(&ctx, &view, &mut state, Vec::new());
        let paragraph = painted(&before, "Paragraph 10").unwrap();
        view.revision = 2;
        let after = frame(&ctx, &view, &mut state, Vec::new());
        let reloaded = painted(&after, "Paragraph 10").unwrap();
        assert!((reloaded.top() - paragraph.top()).abs() < 2.0);
        assert!(paragraph.top() < 160.0, "the preview actually scrolled");
    }

    #[test]
    fn a_file_location_scrolls_the_visible_preview_to_its_highlighted_line() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let lines: Vec<_> = (1..=100).map(|i| format!("source line {i}")).collect();
        let view = PreviewView {
            path: Path::new("/tmp/location.rs"),
            name: "location.rs",
            revision: 1,
            size: Some(1024),
            body: PreviewBody::Text {
                lines: &lines,
                widest: 20,
                truncated: false,
                markdown: None,
            },
        };
        let mut state = State {
            preview_location: Some((42, 3)),
            preview_jump: true,
            ..Default::default()
        };
        let mut painted = Vec::new();
        for _ in 0..4 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0))),
                    ..Default::default()
                },
                |ui| {
                    preview(
                        ui,
                        Rect::from_min_size(Pos2::ZERO, vec2(300.0, 240.0)),
                        Palette::new(crate::config::Theme::Graphite),
                        &view,
                        &mut state,
                        &mut Vec::new(),
                    );
                },
            );
            output.textures_delta.clear();
            painted = output
                .shapes
                .into_iter()
                .filter_map(|s| match s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        assert!(!state.preview_jump);
        assert!(
            painted.iter().any(|t| t == "source line 42"),
            "painted: {painted:?}"
        );
        assert!(!painted.iter().any(|t| t == "source line 1"));
    }
}
