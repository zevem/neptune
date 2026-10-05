//! Terminal panes: rounded content surfaces arranged by the workspace layout.
use super::helpers::{self, animate, capsule, elided, galley_at, menu_item, shortcut};
use super::{Action, PaneRender};
use crate::platform::file_drag::FileDrag;
use crate::{
    config::Config,
    icons::{self, Icon},
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Align2, CursorIcon, Id, LayerId, Layout, Order, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, vec2,
};
use neptune_model::{AgentKind, Axis, Destination, Edge, PaneId};
use std::collections::BTreeMap;
use terminal_core::{Mode as TermMode, SessionMetadata, SessionStatus, ViewportSnapshot};

pub struct PanePresentation {
    /// The CLI agent the terminal is running, which takes dropped files.
    pub agent: Option<AgentKind>,
    /// Pull requests the agent linked to the terminal, oldest first.
    pub pull_requests: Vec<helpers::LinkedPullRequest>,
    /// The agents this terminal's agent started, oldest first.
    pub spawned: Vec<super::agents::Spawned>,
    /// The git worktree made for the terminal's agent, which names its tab.
    pub worktree: Option<super::worktrees::Tab>,
    pub unread: usize,
    pub metadata: SessionMetadata,
    /// The terminal's content; absent for a tab that is not in view.
    pub snapshot: Option<ViewportSnapshot>,
    /// The shell has been requested but has not started yet.
    pub starting: bool,
    /// SSH destination when the terminal runs on another machine.
    pub remote: Option<String>,
}

impl PanePresentation {
    /// Where the terminal is: its host when remote, otherwise its directory.
    pub fn location(&self) -> String {
        match &self.remote {
            Some(destination) => destination.clone(),
            None => helpers::compact_path(&self.metadata.cwd),
        }
    }
}

/// Per-frame inputs shared by every pane of the visible layout.
pub struct Stage<'a> {
    pub presentations: &'a BTreeMap<PaneId, PanePresentation>,
    pub active: PaneId,
    /// The layout holds more than one terminal, so each place carries its
    /// tabs and focus cues.
    pub multiple: bool,
    pub zoomed: bool,
    /// No overlay is open, so the focused terminal may own the keyboard.
    pub keyboard: bool,
    /// The terminal widget that owned the keyboard on the previous frame.
    pub previous_terminal: Option<egui::Id>,
    pub config: &'a Config,
    pub p: Palette,
    /// Highlighted in the focused pane while search is open.
    pub search: &'a str,
    /// The terminal being carried by its header, as of the last frame.
    pub drag: Option<PaneId>,
}

/// Where part of the layout is drawn, and where it rests once the chrome stops
/// moving. Terminals are measured at rest, so a sliding sidebar resizes each
/// shell once instead of on every frame.
#[derive(Clone, Copy)]
pub struct Placement {
    pub drawn: Rect,
    pub settled: Rect,
}

#[derive(Default)]
pub struct StageOutput {
    /// Terminal grid of the focused pane, for input and pointer routing.
    pub active_body: Option<Rect>,
    /// Widget identity of the focused terminal, which holds keyboard focus.
    pub active_terminal: Option<egui::Id>,
    /// The terminal whose header is being dragged this frame.
    pub dragging: Option<PaneId>,
    /// Where a carried terminal would land, and the area it would take.
    pub drop: Option<(Destination, Rect)>,
    /// The terminals in view, and whether each has a shell to take files
    /// dragged in from another application.
    pub cards: Vec<(PaneId, Rect, bool)>,
    /// Readings of a picture path the pointer rests on in a local terminal.
    pub image_paths: Option<(PaneId, Vec<crate::terminal_view::ImagePath>)>,
}

/// Where a terminal dropped at `pointer` lands on the place it is over:
/// against the nearest edge, or among the place's tabs when dropped near its
/// centre. A terminal already there (`own`) can only leave for an edge. The
/// rectangle is the area the terminal would take.
fn drop_destination(
    card: Rect,
    pointer: Pos2,
    pane: PaneId,
    own: bool,
) -> Option<(Destination, Rect)> {
    let x = (pointer.x - card.left()) / card.width().max(1.0);
    let y = (pointer.y - card.top()) / card.height().max(1.0);
    let (distance, edge) = [
        (x, Edge::Left),
        (1.0 - x, Edge::Right),
        (y, Edge::Top),
        (1.0 - y, Edge::Bottom),
    ]
    .into_iter()
    .min_by(|a, b| a.0.total_cmp(&b.0))
    .unwrap_or((0.0, Edge::Right));
    if distance > 0.3 {
        let tab = Destination::Tab {
            pane,
            index: usize::MAX,
        };
        return (!own).then_some((tab, card));
    }
    let half = metrics::GUTTER * 0.5;
    let centre = card.center();
    let area = match edge {
        Edge::Left => Rect::from_min_max(card.min, Pos2::new(centre.x - half, card.bottom())),
        Edge::Right => Rect::from_min_max(Pos2::new(centre.x + half, card.top()), card.max),
        Edge::Top => Rect::from_min_max(card.min, Pos2::new(card.right(), centre.y - half)),
        Edge::Bottom => Rect::from_min_max(Pos2::new(card.left(), centre.y + half), card.max),
    };
    Some((Destination::Beside { pane, edge }, area))
}

/// The program or shell a pane is showing. Prompt-style titles such as
/// `user@host:~/dir` fall back to the shell name; the path is shown separately.
pub fn pane_label(metadata: &SessionMetadata) -> String {
    let custom = !metadata.title.is_empty()
        && !metadata.title.contains('@')
        && !metadata.title.contains(":/")
        && !metadata.title.contains(":~");
    if custom {
        metadata.title.clone()
    } else {
        std::path::Path::new(&metadata.shell)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

fn status_text(status: &SessionStatus) -> Option<(String, bool)> {
    match status {
        SessionStatus::Running => None,
        SessionStatus::Exited {
            signal: Some(signal),
            ..
        } => Some((format!("Stopped by {signal}"), true)),
        SessionStatus::Exited { code: 0, .. } => Some(("Process exited".into(), false)),
        SessionStatus::Exited { code, .. } => Some((format!("Exited with code {code}"), true)),
        SessionStatus::Error(_) => Some(("Could not start shell".into(), true)),
    }
}

struct Status<'a> {
    message: &'a str,
    /// Draws the indicator in the problem colour.
    problem: bool,
    /// Longer explanation shown on hover.
    detail: Option<&'a str>,
    action: &'a str,
    action_hint: &'a str,
}

/// A floating status line with one action, centred near the pane's bottom edge.
fn status_capsule(
    ui: &mut Ui,
    card: Rect,
    p: Palette,
    salt: (&str, PaneId),
    status: Status,
) -> bool {
    let Status {
        message,
        problem,
        detail,
        action,
        action_hint,
    } = status;
    let font = theme::medium(12.0);
    let painter = ui.painter().clone();
    let action_galley = painter.layout_no_wrap(action.to_owned(), font.clone(), p.accent);
    let action_width = action_galley.size().x + 20.0;
    let max_text = (card.width() - 24.0 - 34.0 - action_width - 12.0).max(0.0);
    let text = elided(&painter, message, font, p.fg, max_text);
    let width = 30.0 + text.size().x + 12.0 + action_width + 5.0;
    let rect = Rect::from_center_size(
        Pos2::new(card.center().x, card.bottom() - 34.0),
        vec2(width.min(card.width() - 16.0), 34.0),
    );
    if rect.width() < 60.0 || card.height() < 60.0 {
        return false;
    }
    capsule(&painter, rect, p);
    painter.circle_filled(
        Pos2::new(rect.left() + 17.0, rect.center().y),
        3.5,
        if problem { p.red } else { p.muted },
    );
    let text_rect = galley_at(
        &painter,
        Pos2::new(rect.left() + 30.0, rect.center().y),
        text,
    );
    if let Some(detail) = detail {
        ui.interact(text_rect, ui.id().with((salt, "detail")), Sense::hover())
            .on_hover_text(detail);
    }
    let button = Rect::from_min_max(
        Pos2::new(rect.right() - action_width - 5.0, rect.top() + 5.0),
        rect.max - vec2(5.0, 5.0),
    );
    let response = ui.interact(button, ui.id().with((salt, "action")), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, action));
    painter.rect_filled(
        button,
        12,
        theme::tint(
            p.accent,
            if response.is_pointer_button_down_on() {
                0.34
            } else if response.hovered() {
                0.26
            } else {
                0.16
            },
        ),
    );
    if response.has_focus() {
        painter.rect_stroke(button, 12, Stroke::new(1.5, p.accent), StrokeKind::Inside);
    }
    painter.galley(
        button.center() - action_galley.size() * 0.5,
        action_galley,
        p.accent,
    );
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if action_hint.is_empty() {
        response.clicked()
    } else {
        response.on_hover_text(action_hint).clicked()
    }
}

fn pane_menu(
    ui: &mut Ui,
    p: Palette,
    id: PaneId,
    // Zoomed, on this machine, and in a worktree made for its agent.
    (zoomed, local, worktree): (bool, bool, bool),
    actions: &mut Vec<Action>,
) {
    helpers::menu_layout(ui, 240.0);
    let mut chosen: Vec<Action> = Vec::new();
    if menu_item(ui, p, Icon::Copy, "Copy", &shortcut("C"), false) {
        chosen.push(Action::Copy(id));
    }
    if menu_item(ui, p, Icon::Clipboard, "Paste", &shortcut("V"), false) {
        chosen.push(Action::Paste(id));
    }
    helpers::menu_separator(ui, p);
    let hints_shortcut = if cfg!(target_os = "macos") {
        "⌘⇧H".to_owned()
    } else {
        shortcut("H")
    };
    if menu_item(ui, p, Icon::Copy, "Copy with hints", &hints_shortcut, false) {
        chosen.push(Action::CopyHints(id));
    }
    if menu_item(ui, p, Icon::Search, "Find…", &shortcut("F"), false) {
        chosen.extend([Action::Focus(id), Action::Find]);
    }
    helpers::menu_separator(ui, p);
    if menu_item(ui, p, Icon::Plus, "New tab", &shortcut("T"), false) {
        chosen.push(Action::NewTab(id));
    }
    if local
        && menu_item(
            ui,
            p,
            Icon::Branch,
            "New agent in worktree…",
            &shortcut("G"),
            false,
        )
    {
        chosen.push(Action::Worktree(super::worktrees::Event::New(id)));
    }
    if menu_item(
        ui,
        p,
        Icon::SplitVertical,
        "Split right",
        &shortcut("D"),
        false,
    ) {
        chosen.extend([Action::Focus(id), Action::Split(id, Axis::Vertical)]);
    }
    if menu_item(
        ui,
        p,
        Icon::SplitHorizontal,
        "Split below",
        &shortcut("E"),
        false,
    ) {
        chosen.extend([Action::Focus(id), Action::Split(id, Axis::Horizontal)]);
    }
    if menu_item(
        ui,
        p,
        if zoomed {
            Icon::Minimize
        } else {
            Icon::Maximize
        },
        if zoomed {
            "Show all terminals"
        } else {
            "Zoom terminal"
        },
        &shortcut("Enter"),
        false,
    ) {
        chosen.extend([Action::Focus(id), Action::Zoom]);
    }
    helpers::menu_separator(ui, p);
    if menu_item(ui, p, Icon::Eraser, "Clear scrollback", "", false) {
        chosen.push(Action::Clear(id));
    }
    if menu_item(ui, p, Icon::Refresh, "Restart terminal", "", false) {
        chosen.push(Action::Restart(id));
    }
    helpers::menu_separator(ui, p);
    if worktree && menu_item(ui, p, Icon::Trash, "Remove worktree…", "", true) {
        chosen.push(Action::Worktree(super::worktrees::Event::RemoveOf(id)));
    }
    if menu_item(ui, p, Icon::Close, "Close terminal", &shortcut("W"), true) {
        chosen.push(Action::ClosePane(id));
    }
    if !chosen.is_empty() {
        actions.extend(chosen);
        ui.close();
    }
}

/// One tab of a place: the terminal's program and, where there is room, its
/// folder. The tab is also the handle that carries the terminal elsewhere.
/// Reports whether it is being carried.
fn tab(
    ui: &mut Ui,
    id: PaneId,
    rect: Rect,
    // In view, the only tab of its place, and how far its controls are revealed.
    (shown, alone, reveal): (bool, bool, f32),
    stage: &Stage,
    actions: &mut Vec<Action>,
) -> bool {
    let p = stage.p;
    let Some(presentation) = stage.presentations.get(&id) else {
        return false;
    };
    let metadata = &presentation.metadata;
    let response = ui
        .interact(
            rect,
            ui.id().with(("pane-title", id)),
            Sense::click_and_drag(),
        )
        .on_hover_cursor(CursorIcon::Grab);
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            true,
            shown,
            format!("Terminal tab {}", id.get()),
        )
    });
    let closable = rect.width() >= 60.0;
    let close = Rect::from_center_size(
        Pos2::new(rect.right() - 12.0, rect.center().y),
        Vec2::splat(18.0),
    );
    let close_response = closable.then(|| {
        let response = ui.interact(close, ui.id().with(("tab-close", id)), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Close terminal"));
        response
    });
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    // Linked pull requests sit beside the close control, the newest nearest
    // to it, for as long as the tab keeps room for its title.
    let chips = helpers::PullRequestChips::layout(
        ui,
        ui.id().with(("tab-pull-request", id)),
        if closable {
            &presentation.pull_requests
        } else {
            &[]
        },
        (rect.left() + 72.0, close.left() - 2.0, rect.center().y),
        p,
    );
    // The agents this one started are listed before them.
    let spawned = helpers::SpawnedChip::layout(
        ui,
        ui.id().with(("tab-spawned", id)),
        if closable { &presentation.spawned } else { &[] },
        (rect.left() + 72.0, chips.left, rect.center().y),
        p,
    );
    // A merged worktree offers its cleanup before either.
    let merged = super::worktrees::MergedChip::layout(
        ui,
        ui.id().with(("tab-merged", id)),
        (id, presentation.worktree.as_ref().filter(|_| closable)),
        (rect.left() + 72.0, spawned.left, rect.center().y),
        p,
    );
    let hovered = response.hovered()
        || close_response
            .as_ref()
            .is_some_and(|response| response.hovered())
        || chips.hovered()
        || spawned.hovered()
        || merged.hovered();

    // A place with one terminal reads as a plain title, as it always has.
    let fill = if shown && !alone {
        0.08
    } else if hovered && !alone {
        0.045
    } else {
        0.0
    };
    if fill > 0.0 {
        painter.rect_filled(rect, 7, theme::tint(p.fg, fill));
    }
    let mut left = rect.left() + 9.0;
    if !shown && presentation.unread > 0 {
        painter.circle_filled(Pos2::new(left + 3.0, rect.center().y), 3.0, p.attention);
        left += 11.0;
    }
    let close_alpha = if hovered {
        1.0
    } else if shown {
        reveal
    } else {
        0.0
    };
    let right = if !chips.is_empty() || !spawned.is_empty() || !merged.is_empty() {
        merged.left - 5.0
    } else if closable && close_alpha > 0.0 {
        close.left() - 3.0
    } else {
        rect.right() - 7.0
    };
    // A worktree's tab is named by its branch, and says what runs there
    // where another tab names its folder.
    let branch = presentation.worktree.as_ref().map(|tab| &tab.branch);
    if branch.is_some() && right - left > 40.0 {
        icons::paint(
            &painter,
            Rect::from_center_size(Pos2::new(left + 6.0, rect.center().y), Vec2::splat(12.0)),
            Icon::Branch,
            if shown { p.secondary } else { p.muted },
        );
        left += 17.0;
    }
    let room = (right - left).max(0.0);
    let title = elided(
        &painter,
        branch.map_or(&pane_label(metadata), |branch| branch),
        theme::medium(12.0),
        if shown && id == stage.active {
            p.fg
        } else {
            p.secondary
        },
        room,
    );
    let title_width = title.size().x;
    galley_at(&painter, Pos2::new(left, rect.center().y + 1.0), title);
    let remaining = room - title_width - 9.0;
    if remaining > 36.0 {
        let folder = match &presentation.remote {
            _ if branch.is_some() => pane_label(metadata),
            Some(destination) => destination.clone(),
            None => metadata
                .cwd
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| helpers::compact_path(&metadata.cwd)),
        };
        galley_at(
            &painter,
            Pos2::new(left + title_width + 9.0, rect.center().y + 1.0),
            elided(&painter, &folder, theme::regular(11.5), p.muted, remaining),
        );
    }
    chips.paint(&painter, p, actions);
    spawned.paint(&painter, p, actions);
    merged.paint(&painter, p, actions);
    if let Some(close_response) = close_response {
        if close_alpha > 0.0 {
            if close_response.hovered() {
                painter.rect_filled(close, 5, theme::tint(p.fg, 0.1));
            }
            icons::paint(
                &painter,
                close.shrink(4.5),
                Icon::Close,
                theme::tint(
                    if close_response.hovered() {
                        p.fg
                    } else {
                        p.secondary
                    },
                    close_alpha,
                ),
            );
        }
        if close_response
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("Close terminal   {}", shortcut("W")))
            .clicked()
        {
            actions.push(Action::ClosePane(id));
        }
    }

    if response.clicked() || response.drag_started() {
        actions.push(Action::Focus(id));
    }
    if response.double_clicked() {
        actions.push(Action::Zoom);
    }
    if response.middle_clicked() {
        actions.push(Action::ClosePane(id));
    }
    let carried = response.dragged();
    response.context_menu(|ui| {
        pane_menu(
            ui,
            p,
            id,
            (
                stage.zoomed,
                presentation.remote.is_none(),
                presentation.worktree.is_some(),
            ),
            actions,
        )
    });
    response.on_hover_text(format!(
        "{}{}\n{}",
        presentation
            .worktree
            .as_ref()
            .map(|worktree| format!("{}\n", worktree.branch))
            .unwrap_or_default(),
        if metadata.title.is_empty() {
            pane_label(metadata)
        } else {
            metadata.title.clone()
        },
        match &presentation.remote {
            Some(destination) => destination.clone(),
            None => metadata.cwd.display().to_string(),
        }
    ));
    carried
}

/// The tabs of one place and the controls that act on the terminal in view.
fn pane_header(
    ui: &mut Ui,
    (tabs, shown): (&[PaneId], PaneId),
    header: Rect,
    reveal: f32,
    stage: &Stage,
    actions: &mut Vec<Action>,
    output: &mut StageOutput,
) {
    let show_new = header.width() >= 72.0;
    let show_all = header.width() >= 260.0;
    let controls_width = if show_all {
        118.0
    } else if show_new {
        34.0
    } else {
        0.0
    };
    // The tabs stand clear of the focus ring that follows the place's edge.
    let left = (header.left() + 6.0).min(header.right());
    let strip = Rect::from_min_max(
        Pos2::new(left, header.top() + 6.0),
        Pos2::new(
            (header.right() - controls_width - 6.0).max(left),
            header.bottom() - 2.0,
        ),
    );
    let width = (strip.width() / tabs.len() as f32).min(220.0);
    let slot = |index: usize| {
        Rect::from_min_size(
            Pos2::new(strip.left() + width * index as f32, strip.top()),
            vec2((width - 2.0).max(0.0), strip.height()),
        )
    };
    for (index, id) in tabs.iter().enumerate() {
        let state = (*id == shown, tabs.len() == 1, reveal);
        if tab(ui, *id, slot(index), state, stage, actions) {
            output.dragging = Some(*id);
        }
    }
    // A carried terminal dropped on the tabs lands between them.
    if let Some(dragged) = stage.drag
        && tabs != [dragged]
        && let Some(pointer) = ui.ctx().pointer_interact_pos()
        && header.contains(pointer)
    {
        let others: Vec<_> = (0..tabs.len())
            .filter(|index| tabs[*index] != dragged)
            .map(slot)
            .collect();
        let index = others
            .iter()
            .filter(|slot| slot.center().x < pointer.x)
            .count();
        let x = match others.get(index) {
            Some(slot) => slot.left() - 1.0,
            None => others
                .last()
                .map_or(strip.left(), |slot| slot.right() + 1.0),
        };
        output.drop = Some((
            Destination::Tab { pane: shown, index },
            Rect::from_min_max(
                Pos2::new(x - 1.5, strip.top() + 2.0),
                Pos2::new(x + 1.5, strip.bottom() - 2.0),
            ),
        ));
    }
    if !show_new {
        return;
    }
    ui.scope_builder(
        UiBuilder::new()
            .id_salt(("pane-controls", shown))
            .max_rect(Rect::from_min_max(
                Pos2::new(header.right() - controls_width, strip.top() - 1.0),
                Pos2::new(header.right() - 4.0, strip.bottom() + 1.0),
            ))
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.set_clip_rect(header.intersect(ui.clip_rect()));
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.set_opacity(reveal);
            if show_all {
                if icons::button_with_hint(ui, Icon::SplitHorizontal, "Split below", &shortcut("E"))
                    .clicked()
                {
                    actions.extend([Action::Focus(shown), Action::Split(shown, Axis::Horizontal)]);
                }
                if icons::button_with_hint(ui, Icon::SplitVertical, "Split right", &shortcut("D"))
                    .clicked()
                {
                    actions.extend([Action::Focus(shown), Action::Split(shown, Axis::Vertical)]);
                }
                if icons::button_with_hint(ui, Icon::Maximize, "Zoom terminal", &shortcut("Enter"))
                    .clicked()
                {
                    actions.extend([Action::Focus(shown), Action::Zoom]);
                }
            }
            if icons::button_with_hint(ui, Icon::Plus, "New tab", &shortcut("T")).clicked() {
                actions.push(Action::NewTab(shown));
            }
        },
    );
}

/// Draws one place of the layout: its tabs and the terminal in view.
fn draw_pane(
    ui: &mut Ui,
    (tabs, id): (&[PaneId], PaneId),
    place: Placement,
    pane: &mut PaneRender,
    stage: &Stage,
    actions: &mut Vec<Action>,
    output: &mut StageOutput,
) {
    let p = stage.p;
    let card = place.drawn;
    let Some(presentation) = stage.presentations.get(&id) else {
        ui.painter().rect_filled(card, metrics::PANE_RADIUS, p.bg);
        return;
    };
    let metadata = &presentation.metadata;
    let remote = presentation.remote.is_some();
    let selected = id == stage.active;
    let header_height = if stage.multiple {
        metrics::PANE_HEADER
    } else {
        0.0
    };
    let inset = |card: Rect| {
        let min = card.min + vec2(12.0, if stage.multiple { header_height } else { 10.0 });
        Rect::from_min_max(min, (card.max - vec2(12.0, 10.0)).max(min))
    };
    let body = inset(card);

    // Measure and prepare first, so the surface matches a background the
    // running program may have set.
    if let Some(geometry) = pane.cache.geometry(ui, inset(place.settled), stage.config) {
        actions.push(Action::Resize(id, geometry));
    }
    if let Some(snapshot) = &presentation.snapshot {
        pane.cache.prepare(ui, snapshot, stage.config, p);
    }
    let surface = pane.cache.background();
    ui.painter()
        .rect_filled(card, metrics::PANE_RADIUS, surface);

    let focus = animate(ui.ctx(), ui.id().with(("pane-focus", id)), selected, 0.14);
    if stage.multiple {
        let hovered = ui.rect_contains_pointer(card);
        let reveal = animate(
            ui.ctx(),
            ui.id().with(("pane-controls-reveal", id)),
            hovered || selected,
            0.12,
        );
        pane_header(
            ui,
            (tabs, id),
            Rect::from_min_size(card.min, vec2(card.width(), header_height)),
            reveal,
            stage,
            actions,
            output,
        );
    }

    ui.scope_builder(UiBuilder::new().id_salt(id).max_rect(body), |ui| {
        let painted = pane.cache.paint(
            ui,
            body,
            stage.config,
            p,
            selected,
            if selected { stage.search } else { "" },
            &pane.preedit,
        );
        let response = painted.response;
        response.widget_info(|| {
            WidgetInfo::labeled(
                WidgetType::Other,
                true,
                format!("Terminal pane {}", id.get()),
            )
        });
        if let Some(interaction) = painted.interaction {
            actions.push(Action::Selection(id, interaction));
        }
        if let Some(link) = painted.open_link {
            actions.push(Action::OpenTerminalLink(id, link));
        }
        // A remote terminal names files on another machine.
        if !painted.image_paths.is_empty() && !remote {
            output.image_paths = Some((id, painted.image_paths));
        }
        // A focused widget also reports Enter and Space as clicks; only the
        // pointer changes which pane is focused.
        if (response.clicked() && response.interact_pointer_pos().is_some())
            || response.drag_started()
        {
            actions.push(Action::Focus(id));
        }
        if selected {
            output.active_terminal = Some(response.id);
            if stage.keyboard {
                // The terminal owns Tab, arrows and Escape. It takes focus
                // when nothing has it, or straight from the terminal that had
                // it before a split, close or workspace switch. A focused
                // field such as search keeps the keyboard.
                let focused = ui.memory(|memory| memory.focused());
                if focused != Some(response.id)
                    && (focused.is_none() || focused == stage.previous_terminal)
                {
                    response.request_focus();
                    // The key filter below only takes effect from the frame
                    // after focus is gained.
                    ui.ctx().request_repaint();
                }
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        response.id,
                        egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                        },
                    );
                });
            }
        } else if response.has_focus() {
            response.surrender_focus();
            ui.ctx().request_repaint();
        }
        if !pane.cache.mode.intersects(TermMode::MOUSE_MODE) || ui.input(|i| i.modifiers.shift) {
            response.context_menu(|ui| {
                pane_menu(
                    ui,
                    p,
                    id,
                    (
                        stage.zoomed,
                        presentation.remote.is_none(),
                        presentation.worktree.is_some(),
                    ),
                    actions,
                )
            });
        }
    });

    let painter = ui.painter().clone();
    if stage.multiple {
        // Unfocused panes recede slightly; the focused one carries the accent.
        if focus < 1.0 {
            painter.rect_filled(
                card,
                metrics::PANE_RADIUS,
                theme::tint(surface, 0.24 * (1.0 - focus)),
            );
        }
    }
    painter.rect_stroke(
        card,
        metrics::PANE_RADIUS,
        Stroke::new(1.0, p.separator),
        StrokeKind::Inside,
    );
    // An unread alert rings the pane in the attention colour. It takes the
    // focus ring's place rather than doubling it; a single pane shows it too.
    let attention = animate(
        ui.ctx(),
        ui.id().with(("pane-attention", id)),
        presentation.unread > 0,
        0.16,
    );
    if stage.multiple && focus > 0.0 {
        painter.rect_stroke(
            card,
            metrics::PANE_RADIUS,
            Stroke::new(1.5, theme::tint(p.accent, 0.85 * focus * (1.0 - attention))),
            StrokeKind::Inside,
        );
    }
    if attention > 0.0 {
        // A crisp edge over a glow that fades into the terminal's margin.
        for (inset, width, alpha) in [
            (4.5, 1.0, 0.06),
            (3.5, 1.0, 0.11),
            (2.5, 1.0, 0.18),
            (1.5, 1.0, 0.28),
            (0.0, 1.5, 0.95),
        ] {
            painter.rect_stroke(
                card.shrink(inset),
                metrics::PANE_RADIUS.saturating_sub(inset as u8),
                Stroke::new(width, theme::tint(p.attention, alpha * attention)),
                StrokeKind::Inside,
            );
        }
    }
    // A carried terminal recedes where it was; any place can receive it.
    let lifted = animate(
        ui.ctx(),
        ui.id().with(("pane-lifted", id)),
        stage.drag == Some(id),
        0.12,
    );
    if lifted > 0.0 {
        painter.rect_filled(
            card,
            metrics::PANE_RADIUS,
            theme::tint(p.chrome, 0.6 * lifted),
        );
    }
    if let Some(dragged) = stage.drag
        && tabs != [dragged]
        && let Some(pointer) = ui.ctx().pointer_interact_pos()
        && card.contains(pointer)
        && pointer.y >= card.top() + header_height
    {
        output.drop = drop_destination(card, pointer, id, tabs.contains(&dragged));
    }

    output.cards.push((
        id,
        card,
        !presentation.starting && matches!(metadata.status, SessionStatus::Running),
    ));

    if presentation.starting {
        let center = card.center();
        egui::Spinner::new().size(14.0).color(p.muted).paint_at(
            ui,
            Rect::from_center_size(center - vec2(52.0, 0.0), Vec2::splat(14.0)),
        );
        painter.text(
            center - vec2(38.0, 0.0),
            Align2::LEFT_CENTER,
            if remote {
                "Connecting…"
            } else {
                "Starting shell…"
            },
            theme::regular(12.5),
            p.muted,
        );
    } else if let Some((mut message, problem)) = status_text(&metadata.status) {
        let detail = match &metadata.status {
            SessionStatus::Error(error) => Some(error.as_str()),
            _ => None,
        };
        if remote && detail.is_some() {
            message = "Could not start the SSH client".into();
        }
        if status_capsule(
            ui,
            card,
            p,
            ("pane-status", id),
            Status {
                message: &message,
                problem,
                detail,
                action: if remote { "Reconnect" } else { "Restart" },
                action_hint: if remote {
                    "Connect again   Enter"
                } else {
                    "Start a new shell   Enter"
                },
            },
        ) {
            actions.extend([Action::Focus(id), Action::Restart(id)]);
        }
    } else if let Some(error) = pane.cache.resize_error.clone()
        && status_capsule(
            ui,
            card,
            p,
            ("resize-error", id),
            Status {
                message: "Resize unavailable",
                problem: true,
                detail: Some(&error),
                action: "Retry",
                action_hint: "",
            },
        )
    {
        pane.cache.retry_resize();
        ui.ctx().request_repaint();
    }

    if pane.cache.display_offset > 0 && card.width() > 150.0 && card.height() > 80.0 {
        let chip = Rect::from_min_size(card.right_bottom() - vec2(140.0, 42.0), vec2(128.0, 28.0));
        let response = ui.interact(chip, ui.id().with(("scroll-bottom", id)), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Back to bottom"));
        capsule(&painter, chip, p);
        let ink = if response.hovered() {
            p.fg
        } else {
            p.secondary
        };
        icons::paint(
            &painter,
            Rect::from_center_size(
                Pos2::new(chip.left() + 17.0, chip.center().y),
                Vec2::splat(12.0),
            ),
            Icon::ArrowDown,
            ink,
        );
        painter.text(
            Pos2::new(chip.left() + 29.0, chip.center().y),
            Align2::LEFT_CENTER,
            "Back to bottom",
            theme::medium(11.5),
            ink,
        );
        if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            actions.push(Action::ScrollBottom(id));
        }
    }
    if selected {
        output.active_body = Some(body);
    }
}

/// The two sides of a split and the gutter between them. Each side keeps a
/// minimum for every place it holds along the split, `places` of them before
/// and after the gutter; with too little room, the places share the shortfall.
fn divide(rect: Rect, vertical: bool, ratio: f32, places: (usize, usize)) -> (Rect, Rect, Rect) {
    let length = if vertical {
        rect.width()
    } else {
        rect.height()
    };
    let min = if vertical { 200.0 } else { 130.0 };
    let (before, after) = (places.0 as f32, places.1 as f32);
    let min = (min / length).min(0.9 / (before + after));
    let cut = length * ratio.clamp(min * before, 1.0 - min * after);
    let half = metrics::GUTTER * 0.5;
    if vertical {
        (
            Rect::from_min_max(rect.min, Pos2::new(rect.left() + cut - half, rect.bottom())),
            Rect::from_min_max(Pos2::new(rect.left() + cut + half, rect.top()), rect.max),
            Rect::from_min_max(
                Pos2::new(rect.left() + cut - half, rect.top()),
                Pos2::new(rect.left() + cut + half, rect.bottom()),
            ),
        )
    } else {
        (
            Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + cut - half)),
            Rect::from_min_max(Pos2::new(rect.left(), rect.top() + cut + half), rect.max),
            Rect::from_min_max(
                Pos2::new(rect.left(), rect.top() + cut - half),
                Pos2::new(rect.right(), rect.top() + cut + half),
            ),
        )
    }
}

pub fn draw_node(
    ui: &mut Ui,
    node: &neptune_model::Layout,
    place: Placement,
    panes: &mut BTreeMap<PaneId, PaneRender>,
    stage: &Stage,
    actions: &mut Vec<Action>,
    output: &mut StageOutput,
) {
    match node {
        neptune_model::Layout::Tabs { panes: tabs, shown } => {
            if let Some(pane) = panes.get_mut(shown) {
                draw_pane(ui, (tabs, *shown), place, pane, stage, actions, output);
            }
        }
        neptune_model::Layout::Split {
            id: split,
            axis,
            ratio,
            first,
            second,
        } => {
            let vertical = *axis == Axis::Vertical;
            let rect = place.drawn;
            let length = if vertical {
                rect.width()
            } else {
                rect.height()
            };
            let places = (first.places(*axis), second.places(*axis));
            let (a, b, gap) = divide(rect, vertical, *ratio, places);
            let (settled_a, settled_b, _) = divide(place.settled, vertical, *ratio, places);
            // A slightly wider grab area than the visible gutter.
            let grab = if vertical {
                gap.expand2(vec2(2.0, 0.0))
            } else {
                gap.expand2(vec2(0.0, 2.0))
            };
            // Click sensing is what reports the double-click that evens a line.
            let response = ui
                .interact(
                    grab,
                    ui.id().with(("divider", split.get())),
                    Sense::click_and_drag(),
                )
                .on_hover_and_drag_cursor(if vertical {
                    CursorIcon::ResizeHorizontal
                } else {
                    CursorIcon::ResizeVertical
                });
            if response.dragged()
                && let Some(pos) = response.interact_pointer_pos()
            {
                let ratio = if vertical {
                    (pos.x - rect.left()) / length
                } else {
                    (pos.y - rect.top()) / length
                };
                actions.push(Action::Ratio(*split, ratio.clamp(0.1, 0.9)));
            }
            if response.double_clicked() || response.triple_clicked() {
                actions.push(Action::EvenSplit(*split));
            }
            let grip = animate(
                ui.ctx(),
                response.id.with("grip"),
                response.hovered() || response.dragged(),
                0.12,
            );
            if grip > 0.0 {
                let size = if vertical {
                    vec2(3.0, 40.0f32.min(gap.height() - 16.0))
                } else {
                    vec2(40.0f32.min(gap.width() - 16.0), 3.0)
                };
                ui.painter().rect_filled(
                    Rect::from_center_size(gap.center(), size.max(Vec2::ZERO)),
                    2,
                    theme::tint(
                        if response.dragged() {
                            stage.p.accent
                        } else {
                            stage.p.muted
                        },
                        grip,
                    ),
                );
            }
            for (node, drawn, settled) in [(first, a, settled_a), (second, b, settled_b)] {
                let place = Placement { drawn, settled };
                draw_node(ui, node, place, panes, stage, actions, output);
            }
        }
    }
}

/// Eases a rectangle toward `target`, repainting only until it arrives.
fn glide(ctx: &egui::Context, from: Rect, target: Rect) -> Rect {
    let step = 1.0 - (-ctx.input(|input| input.stable_dt).min(0.1) / 0.04).exp();
    let next = Rect::from_min_max(
        from.min + (target.min - from.min) * step,
        from.max + (target.max - from.max) * step,
    );
    if (next.min - target.min).length() < 0.5 && (next.max - target.max).length() < 0.5 {
        return target;
    }
    ctx.request_repaint();
    next
}

/// Feedback for a terminal carried by its tab: the area it would take on the
/// place under the pointer, and a chip that follows the pointer over the
/// whole window. Releasing over a place moves the terminal there.
pub fn pane_drag(ui: &mut Ui, stage: &Stage, output: &StageOutput, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let p = stage.p;
    let target = stage.drag.and(output.drop);
    if let Some(pane) = stage.drag
        && let Some((destination, _)) = target
        && ctx.input(|input| input.pointer.primary_released())
    {
        actions.push(Action::MovePane(pane, destination));
    }

    // The preview glides between drop areas and fades where it was released.
    let preview = Id::new("pane-drop-preview");
    let opacity = animate(&ctx, preview, target.is_some(), 0.12);
    let shown = ctx.data(|data| data.get_temp::<Rect>(preview));
    let area = match (target, shown) {
        (Some((_, area)), Some(shown)) if opacity > 0.0 => Some(glide(&ctx, shown, area)),
        (Some((_, area)), _) => Some(area),
        (None, shown) => shown.filter(|_| opacity > 0.0),
    };
    ctx.data_mut(|data| match area {
        Some(area) => {
            data.insert_temp(preview, area);
        }
        None => data.remove::<Rect>(preview),
    });
    if let Some(area) = area {
        let painter = ui.painter();
        painter.rect_filled(
            area,
            metrics::PANE_RADIUS,
            theme::tint(p.accent, 0.2 * opacity),
        );
        painter.rect_stroke(
            area,
            metrics::PANE_RADIUS,
            Stroke::new(1.5, theme::tint(p.accent, 0.9 * opacity)),
            StrokeKind::Inside,
        );
    }

    let carried = stage.drag.filter(|pane| output.dragging == Some(*pane));
    let reveal = animate(&ctx, Id::new("pane-drag-chip"), carried.is_some(), 0.12);
    let Some(pane) = carried else {
        return;
    };
    ctx.set_cursor_icon(CursorIcon::Grabbing);
    let (Some(pointer), Some(presentation)) =
        (ctx.pointer_latest_pos(), stage.presentations.get(&pane))
    else {
        return;
    };
    let mut painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("pane-drag-chip")));
    painter.set_opacity(reveal);
    let label = elided(
        &painter,
        &pane_label(&presentation.metadata),
        theme::medium(12.0),
        p.fg,
        180.0,
    );
    let size = vec2(label.size().x + 42.0, 28.0);
    let screen = ctx.content_rect().shrink(4.0);
    let chip = Rect::from_min_size(
        Pos2::new(
            (pointer.x + 14.0).min(screen.right() - size.x),
            (pointer.y + 16.0).min(screen.bottom() - size.y),
        ),
        size,
    );
    capsule(&painter, chip, p);
    icons::paint(
        &painter,
        Rect::from_center_size(
            Pos2::new(chip.left() + 16.0, chip.center().y),
            Vec2::splat(13.0),
        ),
        Icon::Terminal,
        p.secondary,
    );
    galley_at(
        &painter,
        Pos2::new(chip.left() + 29.0, chip.center().y),
        label,
    );
}

/// Feedback for files dragged in from another application: the terminal that
/// would take them is tinted and says what a drop does. Files go to the
/// terminal they are held over, or to the focused one when they are over the
/// chrome or the platform does not say where they are. A drop pastes their
/// paths there.
pub fn file_drop(
    ui: &mut Ui,
    stage: &Stage,
    output: &StageOutput,
    drag: FileDrag,
    actions: &mut Vec<Action>,
) {
    let ctx = ui.ctx().clone();
    let p = stage.p;
    // A sheet or the palette owns the window; files dropped on it go nowhere.
    let held_over = drag
        .pointer
        .and_then(|pointer| output.cards.iter().find(|card| card.1.contains(pointer)));
    let target = held_over
        .or_else(|| output.cards.iter().find(|card| card.0 == stage.active))
        .filter(|(_, _, running)| *running && stage.keyboard)
        .map(|(pane, card, _)| (*pane, *card));
    if !drag.dropped.is_empty()
        && let Some((pane, _)) = target
    {
        actions.push(Action::DropFiles(pane, drag.dropped));
    }

    // The highlight glides between terminals and fades once the files leave.
    let preview = Id::new("file-drop-preview");
    let target = target.filter(|_| drag.hovering);
    let opacity = animate(&ctx, preview, target.is_some(), 0.12);
    let shown = ctx.data(|data| data.get_temp::<(PaneId, Rect)>(preview));
    let shown = match (target, shown) {
        (Some((pane, area)), Some((_, shown))) if opacity > 0.0 => {
            Some((pane, glide(&ctx, shown, area)))
        }
        (Some(target), _) => Some(target),
        (None, shown) => shown.filter(|_| opacity > 0.0),
    };
    ctx.data_mut(|data| match shown {
        Some(shown) => {
            data.insert_temp(preview, shown);
        }
        None => data.remove::<(PaneId, Rect)>(preview),
    });
    let Some((pane, area)) = shown else {
        return;
    };
    let mut painter = ui.painter().clone();
    painter.set_opacity(opacity);
    painter.rect_filled(area, metrics::PANE_RADIUS, theme::tint(p.accent, 0.16));
    painter.rect_stroke(
        area,
        metrics::PANE_RADIUS,
        Stroke::new(1.5, theme::tint(p.accent, 0.9)),
        StrokeKind::Inside,
    );
    let text = match stage.presentations.get(&pane).and_then(|pane| pane.agent) {
        Some(AgentKind::Claude) => "Drop to attach to Claude",
        Some(AgentKind::Codex) => "Drop to attach to Codex",
        Some(AgentKind::Opencode) => "Drop to attach to OpenCode",
        Some(AgentKind::Gemini) => "Drop to attach to Gemini",
        Some(AgentKind::Pi) => "Drop to attach to pi",
        Some(AgentKind::Omp) => "Drop to attach to Oh My Pi",
        None => "Drop to insert path",
    };
    let label = elided(&painter, text, theme::medium(12.0), p.fg, 220.0);
    let chip = Rect::from_center_size(area.center(), vec2(label.size().x + 44.0, 30.0));
    if !area.shrink(8.0).contains_rect(chip) {
        return;
    }
    capsule(&painter, chip, p);
    icons::paint(
        &painter,
        Rect::from_center_size(
            Pos2::new(chip.left() + 18.0, chip.center().y),
            Vec2::splat(14.0),
        ),
        Icon::Image,
        p.accent,
    );
    galley_at(
        &painter,
        Pos2::new(chip.left() + 31.0, chip.center().y),
        label,
    );
}

/// A calm placeholder for a window without a workspace.
pub fn empty_state(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    restoring: bool,
    actions: &mut Vec<Action>,
) {
    ui.painter().rect_filled(rect, metrics::PANE_RADIUS, p.bg);
    ui.painter().rect_stroke(
        rect,
        metrics::PANE_RADIUS,
        Stroke::new(1.0, p.separator),
        StrokeKind::Inside,
    );
    let center = rect.center();
    if restoring {
        egui::Spinner::new().size(16.0).color(p.muted).paint_at(
            ui,
            Rect::from_center_size(center - vec2(0.0, 16.0), Vec2::splat(16.0)),
        );
        ui.painter().text(
            center + vec2(0.0, 12.0),
            Align2::CENTER_CENTER,
            "Restoring workspaces…",
            theme::regular(13.0),
            p.secondary,
        );
        return;
    }
    let tile = Rect::from_center_size(center - vec2(0.0, 58.0), Vec2::splat(52.0));
    ui.painter()
        .rect_filled(tile, 14, theme::tint(p.accent, 0.16));
    icons::paint(ui.painter(), tile.shrink(14.0), Icon::Terminal, p.accent);
    ui.painter().text(
        center - vec2(0.0, 8.0),
        Align2::CENTER_CENTER,
        "No open workspaces",
        theme::semibold(15.0),
        p.fg,
    );
    ui.painter().text(
        center + vec2(0.0, 14.0),
        Align2::CENTER_CENTER,
        "Start a shell in your home directory.",
        theme::regular(12.5),
        p.secondary,
    );
    let clicked = ui
        .scope_builder(
            UiBuilder::new()
                .id_salt("empty-state")
                .max_rect(Rect::from_center_size(
                    center + vec2(0.0, 56.0),
                    vec2(160.0, metrics::CONTROL_HEIGHT),
                ))
                .layout(Layout::top_down(Align::Center)),
            |ui| helpers::button(ui, p, "New workspace", helpers::ButtonKind::Primary).clicked(),
        )
        .inner;
    if clicked {
        actions.push(Action::New);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(title: &str) -> SessionMetadata {
        SessionMetadata {
            title: title.into(),
            shell: "/usr/bin/zsh".into(),
            cwd: "/tmp".into(),
            reported_cwd: None,
            process_id: None,
            status: SessionStatus::Running,
            bell_count: 0,
        }
    }

    #[test]
    fn minimum_sizes_count_every_place_on_each_side_of_a_split() {
        let rect = |width| Rect::from_min_size(Pos2::ZERO, vec2(width, 400.0));
        // One column beside four: an even line stays even however narrow.
        for width in [1500.0, 640.0, 300.0] {
            let (a, b, gap) = divide(rect(width), true, 0.2, (1, 4));
            let inner = width - gap.width();
            assert!((a.width() - inner / 5.0).abs() < 2.0, "{width}: {a:?}");
            assert!((b.width() - inner * 0.8).abs() < 2.0, "{width}: {b:?}");
        }
        // With room, a side keeps 200 points for each column it holds.
        let (a, b, _) = divide(rect(1500.0), true, 0.9, (1, 4));
        assert!((b.width() - 800.0).abs() < 4.0 && a.width() > 690.0);
        let (a, _, _) = divide(rect(1500.0), true, 0.1, (1, 4));
        assert!((a.width() - 200.0).abs() < 4.0);
        // Two places alone keep the limits they had.
        let (a, b, _) = divide(rect(1000.0), true, 0.1, (1, 1));
        assert!((a.width() - 200.0).abs() < 4.0 && b.width() > 790.0);
        let (a, b, _) = divide(rect(300.0), true, 0.1, (1, 1));
        assert!((a.width() - 135.0).abs() < 4.0 && (b.width() - 165.0).abs() < 4.0);
        let (a, _, _) = divide(rect(1000.0).with_max_y(600.0), false, 0.1, (1, 1));
        assert!((a.height() - 130.0).abs() < 4.0);
    }

    /// Two terminals side by side, drawn headlessly the way the app draws them.
    struct Bench {
        ctx: egui::Context,
        config: Config,
        panes: BTreeMap<PaneId, PaneRender>,
        presentations: BTreeMap<PaneId, PanePresentation>,
        layout: neptune_model::Layout,
        drag: Option<PaneId>,
        /// Files from another application, for the next frame only.
        files: FileDrag,
    }

    impl Bench {
        fn new() -> Self {
            let ctx = egui::Context::default();
            ctx.set_fonts(crate::platform::fonts::bundled_definitions());
            let config = Config::default();
            theme::apply(&ctx, &config);
            let ids = [PaneId::new(1), PaneId::new(2)];
            let mut bench = Self {
                ctx,
                config,
                panes: ids.map(|id| (id, PaneRender::new(id))).into(),
                presentations: ids
                    .map(|id| {
                        (
                            id,
                            PanePresentation {
                                agent: None,
                                pull_requests: Vec::new(),
                                spawned: Vec::new(),
                                worktree: None,
                                unread: 0,
                                metadata: metadata(""),
                                snapshot: Some(ViewportSnapshot::blank(80, 24)),
                                starting: false,
                                remote: None,
                            },
                        )
                    })
                    .into(),
                layout: neptune_model::Layout::Split {
                    id: neptune_model::SplitId::new(1),
                    axis: Axis::Vertical,
                    ratio: 0.5,
                    first: Box::new(neptune_model::Layout::pane(ids[0])),
                    second: Box::new(neptune_model::Layout::pane(ids[1])),
                },
                drag: None,
                files: FileDrag::default(),
            };
            bench.frame(vec![]);
            bench
        }

        /// One frame; the drag state is carried over as the app carries it.
        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<Action> {
            let mut actions = Vec::new();
            let mut output = StageOutput::default();
            let mut frame = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 400.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let stage = Stage {
                        presentations: &self.presentations,
                        active: PaneId::new(2),
                        multiple: true,
                        zoomed: false,
                        keyboard: true,
                        previous_terminal: None,
                        config: &self.config,
                        p: Palette::for_config(&self.config),
                        search: "",
                        drag: self.drag,
                    };
                    let rect = ui.max_rect();
                    draw_node(
                        ui,
                        &self.layout,
                        Placement {
                            drawn: rect,
                            settled: rect,
                        },
                        &mut self.panes,
                        &stage,
                        &mut actions,
                        &mut output,
                    );
                    pane_drag(ui, &stage, &output, &mut actions);
                    let files = std::mem::take(&mut self.files);
                    file_drop(ui, &stage, &output, files, &mut actions);
                },
            );
            frame.textures_delta.clear();
            self.drag = output.dragging;
            actions
        }

        fn button(&mut self, pos: Pos2, pressed: bool) -> Vec<Action> {
            self.frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }])
        }

        /// Picks up the first terminal by its header and carries it to `pos`.
        fn carry_to(&mut self, pos: Pos2) -> Vec<Action> {
            let handle = Pos2::new(60.0, 15.0);
            let mut actions = self.frame(vec![egui::Event::PointerMoved(handle)]);
            actions.extend(self.button(handle, true));
            for step in [0.5, 1.0] {
                actions.extend(self.frame(vec![egui::Event::PointerMoved(handle.lerp(pos, step))]));
            }
            actions
        }
    }

    fn moves(actions: &[Action]) -> Vec<(PaneId, Destination)> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::MovePane(pane, destination) => Some((*pane, *destination)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn dragging_a_header_onto_another_pane_moves_that_terminal_once_on_release() {
        let (first, second) = (PaneId::new(1), PaneId::new(2));
        for (pos, destination) in [
            (
                Pos2::new(780.0, 200.0),
                Destination::Beside {
                    pane: second,
                    edge: Edge::Right,
                },
            ),
            (
                Pos2::new(600.0, 200.0),
                Destination::Tab {
                    pane: second,
                    index: usize::MAX,
                },
            ),
            // On the other place's tabs: before its only tab.
            (
                Pos2::new(430.0, 15.0),
                Destination::Tab {
                    pane: second,
                    index: 0,
                },
            ),
        ] {
            let mut bench = Bench::new();
            let carried = bench.carry_to(pos);
            assert_eq!(bench.drag, Some(first));
            assert!(moves(&carried).is_empty(), "nothing moves before release");
            assert!(
                carried
                    .iter()
                    .any(|action| matches!(action, Action::Focus(pane) if *pane == first)),
                "the carried terminal takes focus"
            );
            assert_eq!(moves(&bench.button(pos, false)), [(first, destination)]);
            assert_eq!(bench.drag, None);
            assert!(moves(&bench.frame(vec![])).is_empty());
        }
    }

    #[test]
    fn tabs_out_of_view_take_focus_on_a_click_and_split_away_when_dragged_to_an_edge() {
        let (first, third) = (PaneId::new(1), PaneId::new(3));
        let mut bench = Bench::new();
        // The first place holds two tabs, with the first in view.
        let presentation = PanePresentation {
            agent: None,
            pull_requests: Vec::new(),
            spawned: Vec::new(),
            worktree: None,
            unread: 1,
            metadata: metadata(""),
            snapshot: None,
            starting: false,
            remote: None,
        };
        bench.presentations.insert(third, presentation);
        if let neptune_model::Layout::Split { first: place, .. } = &mut bench.layout {
            **place = neptune_model::Layout::Tabs {
                panes: vec![first, third],
                shown: first,
            };
        }
        bench.frame(vec![]);
        // Two tabs share the strip left of the first place's controls.
        let hidden = Pos2::new(200.0, 15.0);
        bench.frame(vec![egui::Event::PointerMoved(hidden)]);
        bench.button(hidden, true);
        let released = bench.button(hidden, false);
        assert!(moves(&released).is_empty());
        assert!(
            released
                .iter()
                .any(|action| matches!(action, Action::Focus(pane) if *pane == third))
        );

        // Carried by its tab to the bottom of its own place, the first
        // terminal leaves the tab it shared the place with.
        let below = Pos2::new(200.0, 380.0);
        bench.carry_to(below);
        assert_eq!(bench.drag, Some(first));
        assert_eq!(
            moves(&bench.button(below, false)),
            [(
                first,
                Destination::Beside {
                    pane: first,
                    edge: Edge::Bottom
                }
            )]
        );
        // Dropped past the other tab, it trades places with it.
        bench.carry_to(Pos2::new(240.0, 15.0));
        assert_eq!(
            moves(&bench.button(Pos2::new(240.0, 15.0), false)),
            [(
                first,
                Destination::Tab {
                    pane: first,
                    index: 1
                }
            )]
        );
        // Its own place has no centre to drop on.
        bench.carry_to(Pos2::new(200.0, 200.0));
        assert!(moves(&bench.button(Pos2::new(200.0, 200.0), false)).is_empty());
    }

    #[test]
    fn dropped_files_go_to_the_terminal_under_them_or_the_focused_one() {
        let mut bench = Bench::new();
        let files = vec![std::path::PathBuf::from("/tmp/shot.png")];
        let mut drop_at = |pointer: Option<Pos2>| {
            bench.files = FileDrag {
                hovering: false,
                pointer,
                dropped: files.clone(),
            };
            bench
                .frame(vec![])
                .into_iter()
                .filter_map(|action| match action {
                    Action::DropFiles(pane, paths) => Some((pane, paths)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        // The bench focuses the second terminal, on the right.
        assert_eq!(drop_at(None), [(PaneId::new(2), files.clone())]);
        assert_eq!(
            drop_at(Some(Pos2::new(100.0, 200.0))),
            [(PaneId::new(1), files.clone())]
        );
        assert_eq!(
            drop_at(Some(Pos2::new(700.0, 200.0))),
            [(PaneId::new(2), files.clone())]
        );
        // Between the terminals, the focused one takes them.
        assert_eq!(
            drop_at(Some(Pos2::new(400.0, 200.0))),
            [(PaneId::new(2), files.clone())]
        );

        // A terminal whose shell has exited takes no input.
        bench
            .presentations
            .get_mut(&PaneId::new(1))
            .unwrap()
            .metadata
            .status = SessionStatus::Exited {
            code: 0,
            signal: None,
        };
        let mut drop_at = |pointer: Option<Pos2>| {
            bench.files = FileDrag {
                hovering: false,
                pointer,
                dropped: files.clone(),
            };
            bench
                .frame(vec![])
                .into_iter()
                .any(|action| matches!(action, Action::DropFiles(..)))
        };
        assert!(!drop_at(Some(Pos2::new(100.0, 200.0))));
    }

    #[test]
    fn a_cancelled_or_returned_drag_moves_nothing() {
        // Released over the pane it came from.
        let mut bench = Bench::new();
        let home = Pos2::new(200.0, 200.0);
        bench.carry_to(home);
        assert_eq!(bench.drag, Some(PaneId::new(1)));
        assert!(moves(&bench.button(home, false)).is_empty());

        // Cancelled, as Escape does, while over a valid destination.
        let mut bench = Bench::new();
        let pos = Pos2::new(780.0, 200.0);
        bench.carry_to(pos);
        bench.ctx.stop_dragging();
        bench.drag = None;
        assert!(moves(&bench.frame(vec![])).is_empty());
        assert_eq!(bench.drag, None, "the held button does not resume the drag");
        assert!(moves(&bench.button(pos, false)).is_empty());

        // A press and release without movement is a click: focus, no drag.
        let mut bench = Bench::new();
        let handle = Pos2::new(60.0, 15.0);
        bench.frame(vec![egui::Event::PointerMoved(handle)]);
        bench.button(handle, true);
        let released = bench.button(handle, false);
        assert_eq!(bench.drag, None);
        assert!(moves(&released).is_empty());
        assert!(
            released
                .iter()
                .any(|action| matches!(action, Action::Focus(pane) if *pane == PaneId::new(1)))
        );
    }

    #[test]
    fn a_drop_lands_against_the_nearest_edge_or_joins_the_tabs_near_the_centre() {
        let card = Rect::from_min_size(Pos2::new(100.0, 50.0), vec2(400.0, 200.0));
        let pane = PaneId::new(3);
        let at = |x: f32, y: f32| drop_destination(card, Pos2::new(x, y), pane, false).unwrap();
        let beside = |edge| Destination::Beside { pane, edge };
        assert_eq!(at(120.0, 150.0).0, beside(Edge::Left));
        assert_eq!(at(480.0, 150.0).0, beside(Edge::Right));
        assert_eq!(at(300.0, 60.0).0, beside(Edge::Top));
        assert_eq!(at(300.0, 240.0).0, beside(Edge::Bottom));
        // Distance is relative to the pane, so a wide pane's corner still
        // resolves to the edge the pointer is proportionally closest to.
        assert_eq!(at(180.0, 60.0).0, beside(Edge::Top));
        let tab = Destination::Tab {
            pane,
            index: usize::MAX,
        };
        assert_eq!(at(300.0, 150.0), (tab, card));
        // A tab of this place can leave for an edge, not join where it is.
        let own = |x: f32| drop_destination(card, Pos2::new(x, 150.0), pane, true);
        assert_eq!(own(300.0), None);
        assert_eq!(own(480.0).map(|drop| drop.0), Some(beside(Edge::Right)));
        // Each edge takes its half of the pane, less the gutter between them.
        let (_, left) = at(120.0, 150.0);
        let (_, right) = at(480.0, 150.0);
        assert_eq!(left.right() + metrics::GUTTER, right.left());
        assert_eq!((left.min, right.max), (card.min, card.max));
        let (_, top) = at(300.0, 60.0);
        let (_, bottom) = at(300.0, 240.0);
        assert_eq!(top.bottom() + metrics::GUTTER, bottom.top());
        assert_eq!((top.min, bottom.max), (card.min, card.max));
    }

    #[test]
    fn a_sliding_stage_resizes_each_terminal_once_to_where_it_rests() {
        let ctx = egui::Context::default();
        theme::fonts(&ctx);
        let (left, right) = (PaneId::new(1), PaneId::new(2));
        let layout = neptune_model::Layout::Split {
            id: neptune_model::SplitId::new(1),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(neptune_model::Layout::pane(left)),
            second: Box::new(neptune_model::Layout::pane(right)),
        };
        let presentations: BTreeMap<_, _> = [left, right]
            .map(|id| {
                let presentation = PanePresentation {
                    agent: None,
                    pull_requests: Vec::new(),
                    spawned: Vec::new(),
                    worktree: None,
                    unread: 0,
                    metadata: metadata("zsh"),
                    snapshot: Some(ViewportSnapshot::blank(80, 24)),
                    starting: false,
                    remote: None,
                };
                (id, presentation)
            })
            .into();
        let mut panes: BTreeMap<_, _> = [left, right].map(|id| (id, PaneRender::new(id))).into();
        let config = Config::default();
        let settled = Rect::from_min_max(Pos2::new(216.0, 44.0), Pos2::new(1174.0, 754.0));
        let mut resizes = Vec::new();
        // The sidebar slides in: the stage's leading edge moves every frame.
        for edge in [6.0, 60.0, 140.0, 200.0, 216.0, 216.0] {
            let mut actions = Vec::new();
            let mut frame = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1180.0, 760.0))),
                    ..Default::default()
                },
                |ui| {
                    draw_node(
                        ui,
                        &layout,
                        Placement {
                            drawn: Rect::from_min_max(Pos2::new(edge, 44.0), settled.max),
                            settled,
                        },
                        &mut panes,
                        &Stage {
                            presentations: &presentations,
                            active: left,
                            multiple: true,
                            zoomed: false,
                            keyboard: true,
                            previous_terminal: None,
                            config: &config,
                            p: Palette::for_config(&config),
                            search: "",
                            drag: None,
                        },
                        &mut actions,
                        &mut StageOutput::default(),
                    );
                },
            );
            frame.textures_delta.clear();
            resizes.extend(actions.into_iter().filter_map(|action| match action {
                Action::Resize(id, geometry) => Some((id, geometry)),
                _ => None,
            }));
        }
        let (_, _, gap) = divide(settled, true, 0.5, (1, 1));
        assert_eq!(
            resizes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [left, right]
        );
        for (width, (_, geometry)) in [gap.left() - settled.left(), settled.right() - gap.right()]
            .into_iter()
            .zip(&resizes)
        {
            // Each body is inset 12 points from its card on both sides.
            assert_eq!(geometry.pixel_width, (width - 24.0) as u16);
        }
    }

    #[test]
    fn prompt_style_titles_fall_back_to_the_shell_name() {
        assert_eq!(pane_label(&metadata("")), "zsh");
        assert_eq!(pane_label(&metadata("me@host:~/code")), "zsh");
        assert_eq!(pane_label(&metadata("host:/srv")), "zsh");
        assert_eq!(pane_label(&metadata("nvim README.md")), "nvim README.md");
    }

    #[test]
    fn exit_states_are_described_without_alarm_for_success() {
        assert_eq!(status_text(&SessionStatus::Running), None);
        assert_eq!(
            status_text(&SessionStatus::Exited {
                code: 0,
                signal: None
            }),
            Some(("Process exited".into(), false))
        );
        assert_eq!(
            status_text(&SessionStatus::Exited {
                code: 2,
                signal: None
            }),
            Some(("Exited with code 2".into(), true))
        );
        assert_eq!(
            status_text(&SessionStatus::Error("missing".into())),
            Some(("Could not start shell".into(), true))
        );
    }
}
