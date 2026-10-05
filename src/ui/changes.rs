//! The changes tab: the branch of the focused terminal's folder, the files
//! that differ from its last commit or from the main branch, and how the
//! chosen one differs. It draws what the application read from git on its
//! worker and reports what was asked of it.
use super::Action;
use super::explorer::{centered_note, divider, row_surface};
use super::helpers::{compact_path, elided, galley_at, place, segmented};
use crate::{
    icons::{self, Icon},
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Layout, Pos2, Rect, Sense, Ui, UiBuilder, Vec2,
    WidgetInfo, WidgetType, vec2,
};
use std::path::Path;

pub const ROW: f32 = 26.0;
/// The branch and its refresh control, above the choice of what to compare.
pub const HEADER: f32 = 30.0;
/// The choice of what to compare and the count under it.
pub const SCOPE: f32 = 58.0;

/// What the folder is compared with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scope {
    /// The last commit: work that is not committed yet.
    #[default]
    WorkingTree,
    /// Where the branch left the main one: everything it would merge,
    /// committed or not.
    Branch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Modified,
    Added,
    Deleted,
    Renamed,
    /// A new file git has not been told about.
    Untracked,
    Conflicted,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Self::Modified => "Modified",
            Self::Added => "Added",
            Self::Deleted => "Deleted",
            Self::Renamed => "Renamed",
            Self::Untracked => "Untracked",
            Self::Conflicted => "Conflicted",
        }
    }

    /// The letter that leads a row, and its colour.
    fn mark(self, p: Palette) -> (&'static str, Color32) {
        match self {
            Self::Modified => ("M", p.yellow),
            Self::Added => ("A", p.green),
            Self::Deleted => ("D", p.red),
            Self::Renamed => ("R", p.accent),
            Self::Untracked => ("U", p.green),
            Self::Conflicted => ("!", p.red),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// From the top of the repository, with `/` between folders.
    pub path: String,
    /// The path a renamed file had.
    pub from: Option<String>,
    pub status: Status,
    /// Lines added and removed, for a file that is text git tracks.
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

impl File {
    /// The file's name and the folder that holds it.
    fn split(&self) -> (&str, &str) {
        match self.path.rsplit_once('/') {
            Some((folder, name)) => (name, folder),
            None => (&self.path, ""),
        }
    }

    fn label(&self) -> String {
        let mut label = format!("{} {}", self.status.name(), self.path);
        if let (Some(added), Some(removed)) = (self.added, self.removed) {
            label.push_str(&format!(", {added} added, {removed} removed"));
        }
        label
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// Where a run of changed lines sits in the file.
    Hunk,
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// The line's number before and after the change.
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffBody {
    Loading,
    Lines {
        lines: Vec<Line>,
        /// Columns of the longest line.
        widest: usize,
        truncated: bool,
    },
    Binary,
    /// The file changed without a line of it changing, as a rename does.
    Same,
    Note(&'static str),
    Failed(String),
}

pub struct State {
    pub scope: Scope,
    /// The path of the file whose diff is shown.
    pub selected: Option<String>,
    /// The diff's share of the panel's height below the list's heading.
    pub diff_share: f32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            scope: Scope::default(),
            selected: None,
            diff_share: 0.6,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Scope(Scope),
    /// Show how this file differs.
    Select(String),
    CloseDiff,
    Refresh,
}

pub struct Repository<'a> {
    pub root: &'a Path,
    /// The branch, or the short name of a commit checked out without one.
    pub branch: &'a str,
    pub detached: bool,
    pub ahead: u32,
    pub behind: u32,
    /// The branch the changes are counted from, in the branch comparison.
    pub base: Option<&'a str>,
    /// Why there is no list, where a list was expected.
    pub problem: Option<&'a str>,
    pub files: &'a [File],
    /// Files beyond the most a list shows.
    pub more: usize,
    pub diff: Option<(&'a File, &'a DiffBody)>,
}

pub struct View<'a> {
    /// The repository of the focused terminal's folder, or `None` with
    /// `notice` saying why there is none.
    pub repository: Option<Repository<'a>>,
    pub notice: &'a str,
    /// How much of the panel is shown while a toggle slides it.
    pub reveal: f32,
    pub window: Rect,
}

fn file_row(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    file: &File,
    selected: bool,
    events: &mut Vec<Event>,
) {
    let response = ui
        .interact(rect, ui.id().with(("changed", &file.path)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, file.label())
    });
    row_surface(ui, rect, p, &response, selected);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let middle = rect.center().y;
    let (letter, colour) = file.status.mark(p);
    painter.text(
        Pos2::new(rect.left() + 13.0, middle),
        Align2::CENTER_CENTER,
        letter,
        theme::medium(11.0),
        colour,
    );
    // The counts take the trailing edge: removed last, added before it.
    let mut right = rect.right() - 8.0;
    for (count, sign, colour) in [(file.removed, '−', p.red), (file.added, '+', p.green)] {
        let Some(count) = count.filter(|count| *count > 0) else {
            continue;
        };
        let galley = painter.layout_no_wrap(format!("{sign}{count}"), theme::regular(11.0), colour);
        right -= galley.size().x;
        galley_at(&painter, Pos2::new(right, middle), galley);
        right -= 6.0;
    }
    let left = rect.left() + 26.0;
    let (name, folder) = file.split();
    let name = elided(
        &painter,
        name,
        theme::regular(12.5),
        if file.status == Status::Deleted {
            p.secondary
        } else {
            p.fg
        },
        right - left,
    );
    let named = galley_at(&painter, Pos2::new(left, middle), name);
    if !folder.is_empty() && right - named.right() > 30.0 {
        galley_at(
            &painter,
            Pos2::new(named.right() + 7.0, middle + 0.5),
            elided(
                &painter,
                folder,
                theme::regular(11.0),
                p.muted,
                right - named.right() - 7.0,
            ),
        );
    }
    if response.clicked() {
        events.push(if selected {
            Event::CloseDiff
        } else {
            Event::Select(file.path.clone())
        });
    }
    response.on_hover_text(match &file.from {
        Some(from) => format!("{} · {} from {from}", file.path, file.status.name()),
        None => format!("{} · {}", file.path, file.status.name()),
    });
}

fn diff(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    (file, body): (&File, &DiffBody),
    events: &mut Vec<Event>,
) {
    ui.painter().rect_filled(rect, metrics::PANE_RADIUS, p.bg);
    let header = Rect::from_min_size(rect.min, vec2(rect.width(), 32.0));
    let middle = header.center().y;
    let left = place(
        ui,
        header.shrink2(vec2(3.0, 0.0)),
        Layout::right_to_left(Align::Center),
        "changes-diff-actions",
        |ui| {
            if icons::button(ui, Icon::Close, "Close diff").clicked() {
                events.push(Event::CloseDiff);
            }
            ui.min_rect().left()
        },
    );
    let painter = ui.painter().with_clip_rect(rect);
    let (letter, colour) = file.status.mark(p);
    painter.text(
        Pos2::new(rect.left() + 16.0, middle),
        Align2::CENTER_CENTER,
        letter,
        theme::medium(11.0),
        colour,
    );
    let (name, folder) = file.split();
    let budget = left - 6.0 - (rect.left() + 29.0);
    let name = elided(&painter, name, theme::medium(12.0), p.fg, budget);
    let used = name.size().x;
    galley_at(&painter, Pos2::new(rect.left() + 29.0, middle), name);
    if !folder.is_empty() && budget - used > 40.0 {
        galley_at(
            &painter,
            Pos2::new(rect.left() + 29.0 + used + 8.0, middle + 0.5),
            elided(
                &painter,
                folder,
                theme::regular(11.0),
                p.muted,
                budget - used - 8.0,
            ),
        );
    }
    let area = Rect::from_min_max(
        Pos2::new(rect.left() + 2.0, header.bottom()),
        Pos2::new(rect.right() - 2.0, rect.bottom() - 4.0),
    );
    if area.height() < 8.0 {
        return;
    }
    let (lines, widest, truncated) = match body {
        DiffBody::Loading => return,
        DiffBody::Failed(message) => return centered_note(ui, area, p, message),
        DiffBody::Note(note) => return centered_note(ui, area, p, note),
        DiffBody::Binary => return centered_note(ui, area, p, "No diff for this kind of file"),
        DiffBody::Same => {
            let note = if file.from.is_some() {
                "Renamed without changes"
            } else {
                "No lines changed"
            };
            return centered_note(ui, area, p, note);
        }
        DiffBody::Lines {
            lines,
            widest,
            truncated,
        } => (lines, *widest, *truncated),
    };
    let font = egui::FontId::monospace(11.5);
    let column = painter
        .layout_no_wrap("0".into(), font.clone(), p.fg)
        .size()
        .x;
    let height = (font.size * 1.5).round();
    let last = lines
        .iter()
        .filter_map(|line| line.new.max(line.old))
        .max()
        .unwrap_or(0);
    // The line's number, then its sign, then the line.
    let numbers = last.to_string().len() as f32 * column + 14.0;
    let gutter = numbers + column + 8.0;
    let width = gutter + widest as f32 * column + 12.0;
    let mut ui = ui.new_child(UiBuilder::new().id_salt("changes-diff-text").max_rect(area));
    ui.set_clip_rect(area.intersect(ui.clip_rect()));
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    egui::ScrollArea::both()
        // Each file keeps its own place.
        .id_salt(("changes-diff-lines", &file.path))
        .auto_shrink([false, false])
        .show_rows(
            &mut ui,
            height,
            lines.len() + usize::from(truncated),
            |ui, range| {
                ui.set_min_width(width);
                for index in range {
                    let (_, row) = ui.allocate_space(vec2(width.max(ui.available_width()), height));
                    let painter = ui.painter();
                    let text = |x: f32, align: Align2, text: &str, colour: Color32| {
                        painter.text(
                            Pos2::new(row.left() + x, row.center().y),
                            align,
                            text,
                            font.clone(),
                            colour,
                        );
                    };
                    let Some(line) = lines.get(index) else {
                        painter.text(
                            Pos2::new(row.left() + gutter, row.center().y),
                            Align2::LEFT_CENTER,
                            "The rest of the diff is not shown",
                            theme::regular(11.0),
                            p.muted,
                        );
                        continue;
                    };
                    let (fill, sign, ink) = match line.kind {
                        LineKind::Hunk => (Some(theme::tint(p.fg, 0.05)), "", p.muted),
                        LineKind::Context => (None, "", p.fg),
                        LineKind::Added => (Some(theme::tint(p.green, 0.14)), "+", p.green),
                        LineKind::Removed => (Some(theme::tint(p.red, 0.14)), "−", p.red),
                    };
                    if let Some(fill) = fill {
                        painter.rect_filled(row, 0, fill);
                    }
                    if line.kind == LineKind::Hunk {
                        text(numbers, Align2::LEFT_CENTER, &line.text, ink);
                        continue;
                    }
                    if let Some(number) = line.new.or(line.old) {
                        text(
                            numbers - 8.0,
                            Align2::RIGHT_CENTER,
                            &number.to_string(),
                            p.muted,
                        );
                    }
                    text(numbers, Align2::LEFT_CENTER, sign, ink);
                    text(gutter, Align2::LEFT_CENTER, &line.text, p.fg);
                }
            },
        );
}

/// How many files changed, in words, and what they are counted from.
fn counted(repository: &Repository) -> String {
    let files = repository.files.len() + repository.more;
    let since = repository
        .base
        .map(|base| format!(" since {base}"))
        .unwrap_or_default();
    match files {
        0 => format!("No changes{since}"),
        1 => format!("1 file changed{since}"),
        files => format!("{files} files changed{since}"),
    }
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
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER));
    let middle = header.center().y;
    let buttons = place(
        ui,
        header,
        Layout::right_to_left(Align::Center),
        "changes-actions",
        |ui| {
            if view.repository.is_none() {
                ui.disable();
            }
            if icons::button(ui, Icon::Refresh, "Refresh changes").clicked() {
                events.push(Event::Refresh);
            }
            ui.min_rect().left()
        },
    );
    let below = Rect::from_min_max(Pos2::new(inner.left(), header.bottom()), inner.max);
    let Some(repository) = &view.repository else {
        galley_at(
            ui.painter(),
            Pos2::new(inner.left() + 6.0, middle),
            elided(
                ui.painter(),
                "Changes",
                theme::medium(12.5),
                p.fg,
                buttons - inner.left() - 12.0,
            ),
        );
        centered_note(ui, below, p, view.notice);
        return;
    };
    icons::paint(
        ui.painter(),
        Rect::from_center_size(Pos2::new(inner.left() + 13.0, middle), Vec2::splat(14.0)),
        Icon::Branch,
        p.secondary,
    );
    // Commits to push and to pull trail the branch's name.
    let mut tracking = Vec::new();
    for (count, icon) in [
        (repository.ahead, Icon::ArrowUp),
        (repository.behind, Icon::ArrowDown),
    ] {
        if count > 0 {
            let galley =
                ui.painter()
                    .layout_no_wrap(count.to_string(), theme::regular(11.0), p.muted);
            tracking.push((icon, galley));
        }
    }
    let tracking_width: f32 = tracking
        .iter()
        .map(|(_, galley)| galley.size().x + 17.0)
        .sum();
    let left = inner.left() + 26.0;
    let named = galley_at(
        ui.painter(),
        Pos2::new(left, middle),
        elided(
            ui.painter(),
            repository.branch,
            theme::medium(12.5),
            p.fg,
            buttons - left - 8.0 - tracking_width,
        ),
    );
    let mut at = named.right() + 8.0;
    for (icon, galley) in tracking {
        icons::paint(
            ui.painter(),
            Rect::from_center_size(Pos2::new(at + 5.0, middle), Vec2::splat(10.0)),
            icon,
            p.muted,
        );
        at = galley_at(ui.painter(), Pos2::new(at + 11.0, middle + 0.5), galley).right() + 6.0;
    }
    let state_of_branch = match (repository.detached, repository.ahead, repository.behind) {
        (true, ..) => "No branch is checked out".to_owned(),
        (_, 0, 0) => "Branch".to_owned(),
        (_, ahead, behind) => format!("Branch, {ahead} to push and {behind} to pull"),
    };
    ui.interact(
        named.expand2(vec2(4.0, 6.0)),
        ui.id().with("changes-branch"),
        Sense::hover(),
    )
    .on_hover_text(format!(
        "{state_of_branch} · {}",
        compact_path(repository.root)
    ));

    let scope = Rect::from_min_size(
        Pos2::new(inner.left(), header.bottom() + 2.0),
        vec2(inner.width(), 28.0),
    );
    let mut chosen = state.scope;
    let changed = place(
        ui,
        scope,
        Layout::top_down(Align::Min),
        "changes-scope",
        |ui| {
            segmented(
                ui,
                p,
                "changes-scope",
                &mut chosen,
                &[
                    (Scope::WorkingTree, "Working tree"),
                    (Scope::Branch, "Branch"),
                ],
                scope.width(),
            )
        },
    );
    if changed {
        events.push(Event::Scope(chosen));
    }
    let count = Rect::from_min_max(
        Pos2::new(inner.left(), scope.bottom()),
        Pos2::new(inner.right(), header.bottom() + SCOPE),
    );
    let body = Rect::from_min_max(count.left_bottom(), inner.max);
    if let Some(problem) = repository.problem {
        centered_note(ui, body, p, problem);
        return;
    }
    // Lines added and removed across the files, at the count's trailing edge.
    let mut right = count.right() - 8.0;
    let total = |count: fn(&File) -> Option<u32>| -> u32 {
        repository.files.iter().filter_map(count).sum()
    };
    for (lines, sign, colour) in [
        (total(|file| file.removed), '−', p.red),
        (total(|file| file.added), '+', p.green),
    ] {
        if lines > 0 {
            let galley =
                ui.painter()
                    .layout_no_wrap(format!("{sign}{lines}"), theme::regular(11.0), colour);
            right -= galley.size().x;
            galley_at(
                ui.painter(),
                Pos2::new(right, count.center().y + 1.0),
                galley,
            );
            right -= 6.0;
        }
    }
    galley_at(
        ui.painter(),
        Pos2::new(count.left() + 6.0, count.center().y + 1.0),
        elided(
            ui.painter(),
            &counted(repository),
            theme::regular(11.5),
            p.secondary,
            right - count.left() - 6.0,
        ),
    );
    if repository.files.is_empty() {
        let note = match (state.scope, repository.base) {
            (Scope::WorkingTree, _) => "Nothing has changed since the last commit.".to_owned(),
            (Scope::Branch, Some(base)) => format!("This branch has nothing that {base} lacks."),
            (Scope::Branch, None) => String::new(),
        };
        centered_note(ui, body, p, &note);
        return;
    }
    let total = repository.files.len() + usize::from(repository.more > 0);
    // The diff takes the bottom of the panel; the divider above resizes it.
    // A list shorter than its share keeps only its rows, and the diff has
    // the room that leaves.
    let diff_height = repository.diff.map_or(0.0, |_| {
        (body.height() * state.diff_share)
            .clamp(96.0, (body.height() - 80.0).max(96.0))
            .max(body.height() - total as f32 * ROW - 6.0)
            .min(body.height())
            .round()
    });
    let list = Rect::from_min_max(
        body.min,
        Pos2::new(body.right(), body.bottom() - diff_height),
    );
    let mut rows = ui.new_child(UiBuilder::new().id_salt("changes-list").max_rect(list));
    rows.set_clip_rect(list.intersect(rows.clip_rect()));
    rows.spacing_mut().item_spacing = Vec2::ZERO;
    egui::ScrollArea::vertical()
        .id_salt("changes-rows")
        .auto_shrink([false, false])
        .show_rows(&mut rows, ROW, total, |ui, range| {
            for index in range {
                let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW));
                let Some(file) = repository.files.get(index) else {
                    ui.painter().text(
                        Pos2::new(rect.left() + 26.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        format!("{} more files are not listed", repository.more),
                        theme::regular(11.5),
                        p.muted,
                    );
                    continue;
                };
                let selected = state.selected.as_deref() == Some(file.path.as_str());
                file_row(ui, rect, p, file, selected, events);
            }
        });
    if let Some(shown) = repository.diff {
        let surface = Rect::from_min_max(Pos2::new(body.left(), list.bottom() + 6.0), body.max);
        diff(ui, surface, p, shown, events);
        let handle = Rect::from_min_max(
            Pos2::new(body.left(), list.bottom()),
            Pos2::new(body.right(), list.bottom() + 6.0),
        );
        let resize = divider(ui, p, handle, "changes-diff-resize", "Resize diff");
        if resize.dragged()
            && let Some(pointer) = resize.interact_pointer_pos()
            && body.height() > 0.0
        {
            state.diff_share = ((body.bottom() - pointer.y) / body.height()).clamp(0.2, 0.85);
        }
        if resize.double_clicked() {
            state.diff_share = State::default().diff_share;
        }
    }
}

/// The changes in `rect`, below the panel's tabs.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &View,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let mut events = Vec::new();
    let mut child = ui.new_child(UiBuilder::new().id_salt("changes").max_rect(rect));
    child.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(view.window));
    child.multiply_opacity(view.reveal);
    contents(&mut child, rect, p, view, state, &mut events);
    actions.extend(events.into_iter().map(Action::Changes));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, status: Status, counts: Option<(u32, u32)>) -> File {
        File {
            path: path.into(),
            from: None,
            status,
            added: counts.map(|counts| counts.0),
            removed: counts.map(|counts| counts.1),
        }
    }

    #[test]
    fn a_file_is_named_apart_from_its_folder_and_described_in_words() {
        let nested = file("src/ui/changes.rs", Status::Modified, Some((12, 3)));
        assert_eq!(nested.split(), ("changes.rs", "src/ui"));
        assert_eq!(
            nested.label(),
            "Modified src/ui/changes.rs, 12 added, 3 removed"
        );
        let top = file("README.md", Status::Untracked, None);
        assert_eq!(top.split(), ("README.md", ""));
        assert_eq!(top.label(), "Untracked README.md");
    }

    #[test]
    fn the_count_says_what_the_files_are_counted_from() {
        let files = [
            file("a", Status::Added, None),
            file("b", Status::Deleted, None),
        ];
        let repository = |files, more, base| Repository {
            root: Path::new("/repo"),
            branch: "feat/x",
            detached: false,
            ahead: 0,
            behind: 0,
            base,
            problem: None,
            files,
            more,
            diff: None,
        };
        assert_eq!(counted(&repository(&[], 0, None)), "No changes");
        assert_eq!(counted(&repository(&files[..1], 0, None)), "1 file changed");
        assert_eq!(
            counted(&repository(&files, 3, Some("origin/main"))),
            "5 files changed since origin/main"
        );
    }

    #[test]
    fn every_status_has_its_own_name() {
        let all = [
            Status::Modified,
            Status::Added,
            Status::Deleted,
            Status::Renamed,
            Status::Untracked,
            Status::Conflicted,
        ];
        let names: std::collections::BTreeSet<_> = all.iter().map(|status| status.name()).collect();
        assert_eq!(names.len(), all.len());
    }
}
