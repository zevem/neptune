//! Window chrome: the full-height sidebar, the toolbar and window controls.
use super::helpers::{
    self, animate, edit_shortcut, elided, galley_at, keycaps, menu_item, menu_layout,
    menu_separator, menu_submenu, shortcut,
};
use super::{Action, UiState, WorkspaceView};
use crate::{
    icons::{self, Icon},
    platform::window::{self, WindowOperation},
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Key, Layout, Modifiers, PointerButton, Pos2, Rect,
    Sense, Shadow, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType,
    emath::GuiRounding as _, style::ScrollAnimation, vec2,
};
use neptune_model::{
    Destination, PaneId, SidebarItem, WorkspaceGroup, WorkspaceGroupId, WorkspaceId,
};

/// What the chrome needs to know about the frame it surrounds.
pub struct ChromeView<'a> {
    pub workspaces: &'a [WorkspaceView],
    pub groups: &'a [WorkspaceGroup],
    pub sidebar_order: &'a [SidebarItem],
    pub active: Option<WorkspaceId>,
    pub pane: Option<PaneId>,
    /// The focused terminal's label and directory.
    pub subtitle: &'a str,
    /// The focused terminal's linked pull requests, while it has no tab to
    /// carry them.
    pub pull_requests: &'a [helpers::LinkedPullRequest],
    /// And the files its agent attached.
    pub attached: &'a [super::attached::File],
    /// The agents the focused terminal's agent started, likewise.
    pub spawned: &'a [super::agents::Spawned],
    /// The worktree of a terminal alone in view, which has no tab to name.
    pub worktree: Option<(neptune_model::PaneId, &'a super::worktrees::Tab)>,
    pub ports: &'a [crate::runtime::ports::Port],
    pub pane_generation: u64,
    pub zoomed: bool,
    /// The whole window. The window controls keep their place in it.
    pub window: Rect,
    /// How much of the sidebar is shown: 0 hidden, 1 shown, and between the
    /// two while a toggle slides it.
    pub sidebar: f32,
    /// The sidebar is shown once it has settled.
    pub sidebar_open: bool,
    /// The sidebar's full width.
    pub sidebar_width: f32,
    /// The window is wide enough to show a sidebar at all.
    pub sidebar_available: bool,
    /// A terminal of the active workspace is being carried by its header.
    pub pane_drag: Option<PaneId>,
    /// The trailing panel is shown, or `None` in a window too narrow for it.
    pub panel: Option<bool>,
    /// An agent waits for input where the panel does not show it.
    pub panel_attention: bool,
}

const SIDEBAR_WIDTH: std::ops::RangeInclusive<f32> = 170.0..=360.0;
const DEFAULT_SIDEBAR_WIDTH: f32 = 216.0;
const ROW_HEIGHT: f32 = 40.0;
/// The distance between the tops of neighbouring workspace rows.
const ROW_STEP: f32 = ROW_HEIGHT + 2.0;
const GROUP_HEIGHT: f32 = 28.0;
/// The space around a folder: above its workspaces and before the next item.
const GROUP_GAP: f32 = 4.0;
/// The workspace list starts this far below the toolbar strip.
const LIST_TOP: f32 = 4.0;
/// Time constant of a displaced row easing into place; it lands within 160 ms.
const ROW_SETTLE: f32 = 0.035;
/// Holding a dragged row this close to the list's edge scrolls the list.
const SCROLL_REACH: f32 = 28.0;

/// A workspace row lifted out of the sidebar list to be dropped elsewhere in it.
#[derive(Default)]
pub struct WorkspaceDrag {
    /// The row drawn above the others: held by the pointer, or settling into
    /// its place after release.
    lifted: Option<WorkspaceId>,
    /// Where the pointer holds the lifted row, measured from the row's top.
    /// `None` once the row is released or the drag is cancelled.
    grip: Option<f32>,
    /// Row tops relative to the list while a row is lifted; empty at rest.
    tops: Vec<(WorkspaceId, f32)>,
}

/// Top-level workspaces and folders share a drag list. A folder moves with its
/// visible children; measured geometry stays under stable entry identities.
#[derive(Default)]
pub struct SidebarDrag {
    lifted: Option<SidebarItem>,
    grip: Option<f32>,
    positions: Vec<(SidebarItem, f32, f32)>,
}

fn sidebar_drop_index(top: f32, heights: &[f32], origin: usize) -> usize {
    let mut slot = 0.0;
    let mut target = 0;
    for (_, height) in heights
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != origin)
    {
        if top < slot + height * 0.5 {
            break;
        }
        slot += height;
        target += 1;
    }
    target
}

/// The position a row would take if dropped with its top at `top`.
fn drop_index(top: f32, count: usize) -> usize {
    ((top / ROW_STEP).round().max(0.0) as usize).min(count.saturating_sub(1))
}

/// Where the row at `index` rests while the row from `origin` is held over
/// `target`: the rows between them close the gap it left and open one for it.
fn displaced(index: usize, origin: usize, target: usize) -> usize {
    if origin < index && index <= target {
        index - 1
    } else if target <= index && index < origin {
        index + 1
    } else {
        index
    }
}

/// Seconds a toggled sidebar takes to slide in or out.
const SIDEBAR_SLIDE: f64 = 0.16;

/// A sidebar toggle in motion. It starts from however much of the sidebar was
/// showing, so a toggle reversed midway turns around without a jump.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarSlide {
    from: f32,
    started: f64,
}

impl SidebarSlide {
    /// Begins the slide that follows a toggle. `shown` is the state being left.
    pub fn toggled(previous: Option<Self>, shown: bool, now: f64) -> Self {
        Self {
            from: previous
                .and_then(|slide| slide.reveal(shown, now))
                .unwrap_or(if shown { 1.0 } else { 0.0 }),
            started: now,
        }
    }

    /// How much of the sidebar is shown at `now`, or `None` once it has settled.
    pub fn reveal(self, shown: bool, now: f64) -> Option<f32> {
        let progress = ((now - self.started) / SIDEBAR_SLIDE) as f32;
        (progress < 1.0).then(|| {
            egui::lerp(
                self.from..=if shown { 1.0 } else { 0.0 },
                egui::emath::easing::cubic_out(progress.max(0.0)),
            )
        })
    }
}

/// Centre of the sidebar toggle from the window's leading edge: beside the
/// window controls while the sidebar is hidden, at its trailing edge while shown.
fn toggle_offset(view: &ChromeView) -> f32 {
    egui::lerp(94.0..=view.sidebar_width - 22.0, view.sidebar)
}

fn drag_region(ui: &mut Ui, rect: Rect, name: &str) {
    let drag = ui.interact(rect, ui.id().with(name), Sense::click_and_drag());
    if drag.drag_started() {
        window::send(ui.ctx(), WindowOperation::StartDrag);
    }
    if drag.double_clicked() {
        toggle_maximized(ui.ctx());
    }
}

fn toggle_maximized(ctx: &egui::Context) {
    let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
    window::send(ctx, WindowOperation::SetMaximized(!maximized));
}

/// Close, minimize and maximize as three quiet lights. Their glyphs appear when
/// the pointer approaches, and they fade to neutral in an inactive window.
pub fn window_controls(ui: &mut Ui, left_center: Pos2, p: Palette, actions: &mut Vec<Action>) {
    let group = Rect::from_min_max(
        left_center + vec2(-6.0, -12.0),
        left_center + vec2(58.0, 12.0),
    );
    let hovered = ui.rect_contains_pointer(group);
    let active = ui.input(|input| input.focused);
    let reveal = animate(ui.ctx(), ui.id().with("window-controls"), hovered, 0.12);
    for (index, (label, rgb)) in [
        ("Close window", 0xff5f57),
        ("Minimize", 0xfebc2e),
        ("Maximize", 0x28c840),
    ]
    .into_iter()
    .enumerate()
    {
        let center = left_center + vec2(6.0 + index as f32 * 20.0, 0.0);
        let response = ui.interact(
            Rect::from_center_size(center, Vec2::splat(20.0)),
            ui.id().with(("window-control", label)),
            Sense::click(),
        );
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
        let color = theme::color(rgb);
        let painter = ui.painter();
        let fill = if active || hovered {
            color
        } else {
            theme::mix(p.chrome, p.muted, 0.55)
        };
        painter.circle_filled(
            center,
            6.0,
            if response.is_pointer_button_down_on() {
                theme::mix(fill, Color32::BLACK, 0.2)
            } else {
                fill
            },
        );
        painter.circle_stroke(center, 6.0, Stroke::new(0.5, Color32::from_black_alpha(48)));
        if response.has_focus() {
            painter.circle_stroke(center, 8.5, Stroke::new(1.5, theme::tint(p.accent, 0.8)));
        }
        if reveal > 0.0 {
            let ink = Stroke::new(
                1.2,
                theme::tint(theme::mix(color, Color32::BLACK, 0.62), reveal),
            );
            match index {
                0 => {
                    for flip in [1.0, -1.0] {
                        painter.line_segment(
                            [
                                center + vec2(-2.4, -2.4 * flip),
                                center + vec2(2.4, 2.4 * flip),
                            ],
                            ink,
                        );
                    }
                }
                1 => {
                    painter.line_segment([center - vec2(3.0, 0.0), center + vec2(3.0, 0.0)], ink);
                }
                _ => {
                    painter.line_segment([center - vec2(3.0, 0.0), center + vec2(3.0, 0.0)], ink);
                    painter.line_segment([center - vec2(0.0, 3.0), center + vec2(0.0, 3.0)], ink);
                }
            }
        }
        if response.clicked() {
            match index {
                0 => actions.push(Action::WindowClose),
                1 => window::send(ui.ctx(), WindowOperation::Minimize),
                _ => toggle_maximized(ui.ctx()),
            }
        }
        response.on_hover_text(label);
    }
}

fn icon_at(
    ui: &mut Ui,
    center: Pos2,
    icon: Icon,
    label: &str,
    hint: &str,
    salt: &str,
) -> egui::Response {
    ui.scope_builder(
        UiBuilder::new()
            .id_salt(salt)
            .max_rect(Rect::from_center_size(center, Vec2::splat(28.0))),
        |ui| icons::button_with_hint(ui, icon, label, hint),
    )
    .inner
}

pub fn toolbar(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &ChromeView,
    state: &mut UiState,
    actions: &mut Vec<Action>,
) {
    drag_region(ui, rect, "title-drag");
    let middle = rect.center().y;
    // The title starts clear of the leading controls wherever they overlap the
    // toolbar: always while the sidebar is hidden, and in passing as it slides.
    let leading = if view.sidebar_available {
        toggle_offset(view) + 24.0
    } else {
        78.0
    };
    let left = (rect.left() + 8.0).max(view.window.left() + leading);

    // The command field stays centred on the content; narrow windows fall back
    // to an icon so the title keeps its room. A sliding sidebar decides this
    // from the width it is heading for, so the field does not swap midway.
    let field_width = (rect.width() * 0.32).clamp(210.0, 320.0);
    let settled_width = view.window.width()
        - if view.sidebar_open {
            view.sidebar_width
        } else {
            0.0
        };
    let show_field = settled_width >= 760.0;
    let field = Rect::from_center_size(rect.center(), vec2(field_width, 28.0));

    let mut right = rect.right() - 8.0;
    // "New workspace" moves here while the sidebar is away. It fades in from
    // the trailing edge so its neighbours make room gradually.
    let cluster = ui
        .scope_builder(
            UiBuilder::new()
                .id_salt("toolbar-actions")
                .max_rect(Rect::from_min_max(
                    Pos2::new(left, rect.top() + 8.0),
                    Pos2::new(
                        right
                            + if view.sidebar < 1.0 {
                                30.0 * view.sidebar
                            } else {
                                0.0
                            },
                        rect.bottom() - 8.0,
                    ),
                ))
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if view.sidebar < 1.0 {
                    ui.set_opacity(1.0 - view.sidebar);
                    let create =
                        icons::button_with_hint(ui, Icon::Plus, "New workspace", &shortcut("N"));
                    ui.set_opacity(1.0);
                    if create.clicked() {
                        actions.push(Action::New);
                    }
                }
                if let Some(open) = view.panel
                    && super::panel::toggle(ui, p, open, view.panel_attention, &shortcut("O"))
                {
                    actions.push(Action::Panel(super::panel::Event::Toggle));
                }
                if notification_bell(
                    ui,
                    p,
                    view.workspaces.iter().any(|w| w.unread > 0),
                    state.overlay == super::OverlayState::Notifications,
                ) {
                    actions.push(Action::Notifications);
                }
                if let Some(pane) = view.pane {
                    if icons::button_with_hint(
                        ui,
                        Icon::SplitHorizontal,
                        "Split below",
                        &shortcut("E"),
                    )
                    .clicked()
                    {
                        actions.push(Action::Split(pane, neptune_model::Axis::Horizontal));
                    }
                    if icons::button_with_hint(
                        ui,
                        Icon::SplitVertical,
                        "Split right",
                        &shortcut("D"),
                    )
                    .clicked()
                    {
                        actions.push(Action::Split(pane, neptune_model::Axis::Vertical));
                    }
                    if icons::button_with_hint(ui, Icon::Terminal, "New tab", &shortcut("T"))
                        .clicked()
                    {
                        actions.push(Action::NewTab(pane));
                    }
                    if icons::button_with_hint(ui, Icon::Search, "Find in terminal", &shortcut("F"))
                        .clicked()
                    {
                        actions.push(Action::Find);
                    }
                }
                if !show_field
                    && icons::button_with_hint(ui, Icon::Command, "Command palette", &shortcut("P"))
                        .clicked()
                {
                    actions.push(Action::Palette);
                }
                ui.min_rect().left()
            },
        )
        .inner;
    right = cluster - 6.0;

    if view.zoomed {
        let chip = Rect::from_min_max(
            Pos2::new(right - 84.0, middle - 11.0),
            Pos2::new(right, middle + 11.0),
        );
        if chip.left() > left + 40.0 {
            let response = ui.interact(chip, ui.id().with("toolbar-zoom"), Sense::click());
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Exit zoom"));
            let painter = ui.painter();
            painter.rect_filled(
                chip,
                11,
                theme::tint(p.accent, if response.hovered() { 0.3 } else { 0.2 }),
            );
            icons::paint(
                painter,
                Rect::from_center_size(Pos2::new(chip.left() + 15.0, middle), Vec2::splat(12.0)),
                Icon::Minimize,
                p.accent,
            );
            painter.text(
                Pos2::new(chip.left() + 26.0, middle),
                Align2::LEFT_CENTER,
                "Zoomed",
                theme::medium(11.5),
                p.accent,
            );
            if response
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(format!("Show all terminals   {}", shortcut("Enter")))
                .clicked()
            {
                actions.push(Action::Zoom);
            }
            right = chip.left() - 8.0;
        }
    }

    if state.search_open && view.pane.is_some() {
        // Search takes the command field's place. In a narrow window it uses
        // the title's room instead, so its controls stay reachable.
        let field = if show_field {
            let width = (rect.width() * 0.42).clamp(300.0, 440.0);
            let centre = rect.center().x.min(right - 8.0 - width * 0.5);
            Rect::from_center_size(Pos2::new(centre, middle), vec2(width, 30.0))
        } else {
            Rect::from_min_max(
                Pos2::new(left + 6.0, middle - 15.0),
                Pos2::new(right - 4.0, middle + 15.0),
            )
        };
        if field.width() >= 150.0 {
            super::search::show(ui, field, p, state, actions);
        }
        if !show_field {
            return;
        }
        right = right.min(field.left() - 14.0);
    } else if show_field {
        let response = ui.interact(field, ui.id().with("toolbar-commands"), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Command palette"));
        let painter = ui.painter();
        painter.rect_filled(
            field,
            metrics::CONTROL_RADIUS,
            if response.is_pointer_button_down_on() {
                p.pressed
            } else if response.hovered() {
                p.hover
            } else {
                p.control
            },
        );
        if response.has_focus() {
            painter.rect_stroke(
                field,
                metrics::CONTROL_RADIUS,
                Stroke::new(1.5, p.accent),
                egui::StrokeKind::Inside,
            );
        }
        icons::paint(
            painter,
            Rect::from_center_size(Pos2::new(field.left() + 16.0, middle), Vec2::splat(13.0)),
            Icon::Search,
            p.muted,
        );
        let caps = keycaps(
            painter,
            Pos2::new(field.right() - 6.0, middle),
            &shortcut("P"),
            p.muted,
        );
        galley_at(
            painter,
            Pos2::new(field.left() + 30.0, middle),
            elided(
                painter,
                "Search commands",
                theme::regular(12.5),
                p.muted,
                field.width() - 44.0 - caps,
            ),
        );
        if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            actions.push(Action::Palette);
        }
        right = right.min(field.left() - 14.0);
    }

    let Some(workspace) = view.workspaces.iter().find(|w| Some(w.id) == view.active) else {
        return;
    };
    let chips = helpers::PullRequestChips::layout(
        ui,
        ui.id().with("toolbar-pull-request"),
        view.pull_requests,
        (left + 96.0, right - 2.0, middle),
        p,
    );
    let spawned = helpers::SpawnedChip::layout(
        ui,
        ui.id().with("toolbar-spawned"),
        view.spawned,
        (left + 96.0, chips.left, middle),
        p,
    );
    let merged = super::worktrees::MergedChip::layout(
        ui,
        ui.id().with("toolbar-merged"),
        match view.worktree {
            Some((pane, tab)) => (pane, Some(tab)),
            None => (neptune_model::PaneId::new(0), None),
        },
        (left + 96.0, spawned.left, middle),
        p,
    );
    let attached = view.pane.map(|pane| {
        super::attached::AttachedChip::layout(
            ui,
            ui.id().with("toolbar-attached"),
            (pane, view.attached),
            (left + 96.0, merged.left, middle),
            p,
        )
    });
    let leading = attached.as_ref().map_or(merged.left, |chip| chip.left);
    let ports = super::ports::Chips::layout(
        ui,
        ui.id().with("toolbar-ports"),
        view.ports,
        view.pane.map(|pane| (pane, view.pane_generation)),
        (left + 42.0, leading, middle),
        p,
    );
    if ports.left < right - 2.0 {
        right = ports.left - 6.0;
    }
    chips.paint(ui.painter(), p, actions);
    spawned.paint(ui.painter(), p, actions);
    merged.paint(ui.painter(), p, actions);
    if let Some(attached) = attached {
        attached.paint(ui.painter(), p, actions);
    }
    ports.paint(ui.painter(), p, actions);
    let budget = right - left - 6.0;
    if budget < 36.0 {
        return;
    }
    let painter = ui.painter().with_clip_rect(Rect::from_min_max(
        Pos2::new(left, rect.top()),
        Pos2::new(right, rect.bottom()),
    ));
    let name = elided(&painter, &workspace.name, theme::medium(13.0), p.fg, budget);
    let name_width = name.size().x;
    galley_at(&painter, Pos2::new(left + 6.0, middle), name);
    let remaining = budget - name_width - 12.0;
    if remaining > 48.0 && !view.subtitle.is_empty() {
        galley_at(
            &painter,
            Pos2::new(left + 6.0 + name_width + 10.0, middle + 0.5),
            elided(
                &painter,
                view.subtitle,
                theme::regular(12.0),
                p.secondary,
                remaining,
            ),
        );
    }
}

/// The bell stays pressed while its popover is open, and carries a dot in the
/// attention colour while any terminal has an unread alert.
fn notification_bell(ui: &mut Ui, p: Palette, unread: bool, open: bool) -> bool {
    let surface = ui.painter().add(egui::Shape::Noop);
    let response = icons::button(ui, Icon::Bell, "Notifications");
    if open {
        ui.painter().set(
            surface,
            egui::Shape::rect_filled(response.rect.shrink(1.0), 7, p.pressed),
        );
    }
    let dot = animate(ui.ctx(), response.id.with("unread"), unread, 0.16);
    if dot > 0.0 {
        // Cut out of the glyph, on whatever the button is showing beneath.
        let beneath = if open || response.is_pointer_button_down_on() {
            p.pressed
        } else if response.hovered() {
            p.hover
        } else {
            Color32::TRANSPARENT
        };
        let centre = response.rect.center() + vec2(5.0, -5.0);
        let painter = ui.painter();
        painter.circle_filled(centre, 5.0 * dot, p.chrome);
        painter.circle_filled(centre, 5.0 * dot, beneath);
        painter.circle_filled(centre, 3.5 * dot, p.attention);
    }
    response.clicked()
}

fn workspace_menu(
    ui: &mut Ui,
    p: Palette,
    workspace: &WorkspaceView,
    view: &ChromeView,
    actions: &mut Vec<Action>,
) {
    menu_layout(ui, 210.0);
    let siblings: Vec<usize> = view
        .workspaces
        .iter()
        .enumerate()
        .filter(|(_, row)| row.group == workspace.group)
        .map(|(index, _)| index)
        .collect();
    let position = siblings
        .iter()
        .position(|index| view.workspaces[*index].id == workspace.id)
        .unwrap_or(0);
    if menu_item(ui, p, Icon::Pencil, "Rename…", "", false) {
        actions.push(Action::Rename(workspace.id));
        ui.close();
    }
    if position > 0 && menu_item(ui, p, Icon::ArrowUp, "Move up", "", false) {
        actions.push(Action::MoveWorkspace(workspace.id, siblings[position - 1]));
        ui.close();
    }
    if position + 1 < siblings.len() && menu_item(ui, p, Icon::ArrowDown, "Move down", "", false) {
        actions.push(Action::MoveWorkspace(workspace.id, siblings[position + 1]));
        ui.close();
    }
    if !view.groups.is_empty() {
        menu_submenu(ui, p, Icon::Folder, "Move to group", |ui| {
            menu_layout(ui, 210.0);
            if workspace.group.is_some() && menu_item(ui, p, Icon::Grid, "Ungrouped", "", false) {
                actions.push(Action::MoveToGroup(workspace.id, None));
                ui.close();
            }
            for group in view.groups {
                if Some(group.id()) != workspace.group {
                    ui.push_id(group.id(), |ui| {
                        if menu_item(ui, p, Icon::Folder, group.name(), "", false) {
                            actions.push(Action::MoveToGroup(workspace.id, Some(group.id())));
                            ui.close();
                        }
                    });
                }
            }
        });
    }
    if workspace.remote.is_some() {
        if menu_item(ui, p, Icon::Globe, "Disconnect from SSH", "", false) {
            actions.push(Action::Disconnect(workspace.id));
            ui.close();
        }
    } else if menu_item(ui, p, Icon::Globe, "Connect over SSH…", "", false) {
        actions.push(Action::Ssh(Some(workspace.id)));
        ui.close();
    }
    menu_separator(ui, p);
    if menu_item(ui, p, Icon::Close, "Close workspace", "", true) {
        actions.push(Action::CloseWorkspace(workspace.id));
        ui.close();
    }
}

pub(super) fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "·".into())
}

/// How a workspace row is drawn this frame.
struct RowState {
    rect: Rect,
    /// The row's position in the list and the list's length.
    position: (usize, usize),
    selected: bool,
    /// Elevation above the list: 0 at rest, 1 while dragged or settling.
    lift: f32,
    /// A row is held by the pointer, so rows give no hover feedback.
    dragging: bool,
    /// A terminal carried from the stage, which this row may receive.
    pane_drag: Option<PaneId>,
    /// The carried terminal's session can run in this workspace.
    accepts: bool,
}

fn workspace_row(
    ui: &mut Ui,
    p: Palette,
    workspace: &WorkspaceView,
    view: &ChromeView,
    state: RowState,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let RowState {
        rect: row,
        position,
        selected,
        lift,
        dragging,
        pane_drag,
        accepts,
    } = state;
    let response = ui.interact(
        row,
        ui.id().with(("workspace", workspace.id.get())),
        Sense::click_and_drag(),
    );
    let more_id = ui.id().with(("workspace-more", workspace.id.get()));
    let menu_open =
        egui::Popup::is_id_open(ui.ctx(), more_id.with("popup")) || response.context_menu_opened();
    let hovered = lift > 0.0 || (!dragging && (ui.rect_contains_pointer(row) || menu_open));
    let painter = if lift > 0.0 {
        // A lifted row floats: its shadow may fall outside the list.
        ui.painter()
            .with_clip_rect(ui.clip_rect().expand2(vec2(8.0, 12.0)))
    } else {
        ui.painter().clone()
    };
    if lift > 0.0 {
        let shadow = p.popup_shadow();
        painter.add(
            Shadow {
                color: shadow.color.gamma_multiply(lift),
                ..shadow
            }
            .as_shape(row, metrics::ROW_RADIUS),
        );
        painter.rect_filled(row, metrics::ROW_RADIUS, p.elevated.gamma_multiply(lift));
        painter.rect_stroke(
            row,
            metrics::ROW_RADIUS,
            Stroke::new(1.0, p.border.gamma_multiply(lift)),
            StrokeKind::Inside,
        );
    }
    if selected {
        painter.rect_filled(row, metrics::ROW_RADIUS, p.pressed);
    } else if hovered {
        painter.rect_filled(row, metrics::ROW_RADIUS, p.hover);
    }
    if response.has_focus() {
        painter.rect_stroke(
            row,
            metrics::ROW_RADIUS,
            Stroke::new(1.5, p.accent),
            egui::StrokeKind::Inside,
        );
    }
    // A carried terminal can be dropped on any workspace but its own, as
    // long as that workspace is on the same machine.
    let receiving = pane_drag.filter(|_| !selected && accepts && ui.rect_contains_pointer(row));
    let receive = animate(
        ui.ctx(),
        ui.id().with(("workspace-drop", workspace.id.get())),
        receiving.is_some(),
        0.12,
    );
    if receive > 0.0 {
        painter.rect_filled(
            row,
            metrics::ROW_RADIUS,
            theme::tint(p.accent, 0.2 * receive),
        );
        painter.rect_stroke(
            row,
            metrics::ROW_RADIUS,
            Stroke::new(1.5, theme::tint(p.accent, 0.9 * receive)),
            egui::StrokeKind::Inside,
        );
    }
    if let Some(pane) = receiving
        && ui.input(|input| input.pointer.primary_released())
    {
        actions.push(Action::MovePane(pane, Destination::Workspace(workspace.id)));
    }

    let text_left = row.left() + 9.0;
    // Every terminal here has stopped: a red dot leads the name.
    let name_left = if workspace.running {
        text_left
    } else {
        painter.circle_filled(Pos2::new(text_left + 3.0, row.center().y - 7.5), 3.0, p.red);
        text_left + 11.0
    };
    let show_shortcut = position.0 < 9
        && ui.input(|input| {
            input.focused
                && if cfg!(target_os = "macos") {
                    input.modifiers.mac_cmd
                } else {
                    input.modifiers.ctrl
                }
        });
    let hint_width = if show_shortcut {
        let digit = position.0 + 1;
        let hint = if cfg!(target_os = "macos") {
            format!("⌘{digit}")
        } else {
            format!("Ctrl⇧{digit}")
        };
        keycaps(
            &painter,
            Pos2::new(row.right() - 7.0, row.top() + 13.0),
            &hint,
            p.secondary,
        )
    } else {
        0.0
    };
    // An unread count takes the trailing edge; wider counts need more room.
    let trailing = if workspace.unread > 0 {
        super::notifications::pill_width(workspace.unread) + 14.0
    } else {
        30.0
    };
    let text_width = row.right() - text_left - trailing;
    galley_at(
        &painter,
        Pos2::new(name_left, row.center().y - 7.5),
        elided(
            &painter,
            &workspace.name,
            theme::medium(13.0),
            if selected || hovered || workspace.unread > 0 {
                p.fg
            } else {
                theme::mix(p.secondary, p.fg, 0.45)
            },
            row.right() - name_left - (hint_width + 14.0).max(trailing),
        ),
    );
    let detail = Pos2::new(text_left, row.center().y + 8.5);
    if let Some(alert) = &workspace.alert {
        // What the workspace is asking for, until it is read.
        icons::paint(
            &painter,
            Rect::from_center_size(detail + vec2(5.5, 0.0), Vec2::splat(11.0)),
            Icon::Bell,
            p.attention,
        );
        galley_at(
            &painter,
            detail + vec2(15.0, 0.0),
            elided(
                &painter,
                alert,
                theme::regular(11.0),
                p.secondary,
                text_width - 15.0,
            ),
        );
    } else if let Some(destination) = &workspace.remote {
        // A remote workspace shows its host where a local one shows its folder.
        icons::paint(
            &painter,
            Rect::from_center_size(detail + vec2(5.5, 0.0), Vec2::splat(11.0)),
            Icon::Globe,
            p.muted,
        );
        galley_at(
            &painter,
            detail + vec2(15.0, 0.0),
            elided(
                &painter,
                destination,
                theme::regular(11.0),
                p.muted,
                text_width - 15.0,
            ),
        );
    } else {
        // The branch trails the folder and may take most of the line: the
        // name above usually says which folder this is.
        let branch = workspace.branch.as_ref().map(|branch| {
            let dot = if branch.dirty { 9.0 } else { 0.0 };
            let name = elided(
                &painter,
                &branch.name,
                theme::regular(11.0),
                p.muted,
                (text_width * 0.6 - 14.0 - dot).max(0.0),
            );
            (name, branch.dirty, dot)
        });
        let branch_width = branch
            .as_ref()
            .map_or(0.0, |(name, _, dot)| 8.0 + 14.0 + name.size().x + dot);
        let path = painter.text(
            detail,
            Align2::LEFT_CENTER,
            helpers::path_label(
                &workspace.cwd,
                ((text_width - branch_width) / 5.9).max(4.0) as usize,
            ),
            theme::regular(11.0),
            p.muted,
        );
        if let Some((name, dirty, _)) = branch {
            let left = path.right() + 8.0;
            icons::paint(
                &painter,
                Rect::from_center_size(Pos2::new(left + 5.5, detail.y), Vec2::splat(11.0)),
                Icon::Branch,
                p.muted,
            );
            let named = galley_at(&painter, Pos2::new(left + 14.0, detail.y), name);
            // Work that is not committed yet marks the branch.
            if dirty {
                painter.circle_filled(Pos2::new(named.right() + 6.0, detail.y), 2.5, p.yellow);
            }
        }
    }

    response.widget_info(|| {
        let label = match workspace.unread {
            0 => workspace.name.clone(),
            1 => format!("{}, 1 unread notification", workspace.name),
            unread => format!("{}, {unread} unread notifications", workspace.name),
        };
        WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, label)
    });
    if response.clicked() {
        actions.push(Action::SelectWorkspace(workspace.id));
    }
    if response.double_clicked() {
        actions.push(Action::Rename(workspace.id));
    }
    response.context_menu(|ui| workspace_menu(ui, p, workspace, view, actions));

    let more = Rect::from_center_size(
        Pos2::new(
            row.right() - 17.0,
            if show_shortcut {
                row.bottom() - 12.0
            } else {
                row.center().y
            },
        ),
        Vec2::splat(24.0),
    );
    let more_response = ui.interact(more, more_id, Sense::click());
    more_response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!("Actions for {}", workspace.name),
        )
    });
    // While a row or a terminal is carried, rows are destinations, not controls.
    if (hovered && !dragging && pane_drag.is_none()) || more_response.has_focus() {
        if more_response.hovered() || menu_open {
            painter.rect_filled(more, 6, p.pressed);
        }
        icons::paint(
            &painter,
            more.shrink(5.0),
            Icon::Ellipsis,
            if more_response.hovered() {
                p.fg
            } else {
                p.secondary
            },
        );
    } else if workspace.unread > 0 {
        super::notifications::pill(
            &painter,
            Pos2::new(row.right() - 8.0, more.center().y),
            Align2::RIGHT_CENTER,
            workspace.unread,
            p,
        );
    } else if workspace.panes > 1 {
        painter.text(
            more.center(),
            Align2::CENTER_CENTER,
            workspace.panes.to_string(),
            theme::medium(11.0),
            p.muted,
        );
    }
    egui::Popup::menu(&more_response).show(|ui| workspace_menu(ui, p, workspace, view, actions));
    response
}

/// The workspace rows. Dragging one lifts it out of the list: it follows the
/// pointer while its neighbours ease aside, and takes the gap on release.
fn workspace_rows(
    ui: &mut Ui,
    p: Palette,
    view: &ChromeView,
    rows: &[&WorkspaceView],
    viewport: Rect,
    drag: &mut WorkspaceDrag,
    actions: &mut Vec<Action>,
) {
    let count = rows.len();
    let (list, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            (count as f32 * ROW_STEP - (ROW_STEP - ROW_HEIGHT)).max(0.0),
        ),
        Sense::hover(),
    );
    let lifted = drag
        .lifted
        .and_then(|id| rows.iter().position(|w| w.id == id));
    if lifted.is_none() {
        // Nothing is lifted, or its workspace has closed.
        *drag = WorkspaceDrag::default();
    }
    // A carried terminal stays on the machine of the workspace it came from.
    let machine = view
        .workspaces
        .iter()
        .find(|workspace| Some(workspace.id) == view.active)
        .map(|workspace| &workspace.remote);
    let shown = |id: WorkspaceId, tops: &[(WorkspaceId, f32)]| {
        tops.iter().find(|(row, _)| *row == id).map(|(_, top)| *top)
    };

    // The held row follows the pointer, kept inside the list and its viewport.
    let mut held = lifted.zip(drag.grip).map(|(origin, grip)| {
        let pointer = ui.input(|input| input.pointer.latest_pos());
        let low = (viewport.top() - list.top()).max(0.0);
        let high = ((count - 1) as f32 * ROW_STEP)
            .min(viewport.bottom() - list.top() - ROW_HEIGHT)
            .max(low);
        let top = pointer
            .map(|pointer| pointer.y - grip - list.top())
            .or_else(|| shown(rows[origin].id, &drag.tops))
            .unwrap_or(origin as f32 * ROW_STEP)
            .clamp(low, high);
        if let Some(pointer) = pointer {
            let over = if list.top() < viewport.top() - 0.5 {
                (viewport.top() + SCROLL_REACH - pointer.y).max(0.0)
            } else {
                0.0
            } - if list.bottom() > viewport.bottom() + 0.5 {
                (pointer.y - viewport.bottom() + SCROLL_REACH).max(0.0)
            } else {
                0.0
            };
            if over != 0.0 {
                let speed = over.clamp(-2.0 * SCROLL_REACH, 2.0 * SCROLL_REACH) * 12.0;
                ui.scroll_with_delta_animation(
                    vec2(0.0, speed * ui.input(|input| input.stable_dt).min(0.1)),
                    ScrollAnimation::none(),
                );
                ui.ctx().request_repaint();
            }
        }
        (origin, top)
    });
    // Escape puts the row back; the shell does not receive that key.
    if held.is_some() && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
        drag.grip = None;
        held = None;
        ui.ctx().request_repaint();
    }
    let target = held.map(|(origin, top)| (origin, drop_index(top, count)));

    let decay = (-ui.input(|input| input.stable_dt).min(0.1) / ROW_SETTLE).exp();
    let mut tops = Vec::new();
    let mut moving = false;
    // The lifted row is drawn last, above its neighbours.
    for index in (0..count)
        .filter(|index| Some(*index) != lifted)
        .chain(lifted)
    {
        let workspace = rows[index];
        let global_index = view
            .workspaces
            .iter()
            .position(|w| w.id == workspace.id)
            .unwrap_or(index);
        let top = match held {
            Some((origin, top)) if origin == index => top,
            _ => {
                let rest = target.map_or(index, |(origin, target)| displaced(index, origin, target))
                    as f32
                    * ROW_STEP;
                // A row not yet tracked starts from its own place.
                let from = shown(workspace.id, &drag.tops).unwrap_or(index as f32 * ROW_STEP);
                let top = rest + (from - rest) * decay;
                if (top - rest).abs() < 0.5 {
                    rest
                } else {
                    moving = true;
                    top
                }
            }
        };
        if lifted.is_some() {
            tops.push((workspace.id, top));
        }
        let response = workspace_row(
            ui,
            p,
            workspace,
            view,
            RowState {
                rect: Rect::from_min_size(
                    Pos2::new(
                        list.left(),
                        if lifted.is_some() {
                            // Rows in motion stay on whole pixels, so their edges
                            // stay crisp at fractional display scales.
                            (list.top() + top).round_to_pixels(ui.pixels_per_point())
                        } else {
                            list.top() + top
                        },
                    ),
                    vec2(list.width(), ROW_HEIGHT),
                ),
                position: (global_index, view.workspaces.len()),
                selected: Some(workspace.id) == view.active,
                lift: animate(
                    ui.ctx(),
                    ui.id().with(("workspace-lift", workspace.id.get())),
                    Some(index) == lifted,
                    0.12,
                ),
                dragging: held.is_some(),
                pane_drag: view.pane_drag,
                accepts: Some(&workspace.remote) == machine,
            },
            actions,
        );
        if let Some((origin, target)) = target.filter(|(origin, _)| *origin == index) {
            if response.drag_stopped() {
                if origin != target {
                    let global_target = view
                        .workspaces
                        .iter()
                        .position(|w| w.id == rows[target].id)
                        .unwrap_or(target);
                    actions.push(Action::MoveWorkspace(workspace.id, global_target));
                }
                drag.grip = None;
                ui.ctx().request_repaint();
            } else if response.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else {
                // The pointer was released while the row was not shown.
                drag.grip = None;
                ui.ctx().request_repaint();
            }
        } else if response.drag_started_by(PointerButton::Primary)
            && let Some(press) = ui.input(|input| input.pointer.press_origin())
        {
            drag.lifted = Some(workspace.id);
            drag.grip = Some(press.y - response.rect.top());
            ui.ctx().request_repaint();
        }
    }
    if moving {
        ui.ctx().request_repaint();
    }
    if held.is_none() && drag.grip.is_none() && !moving {
        // Every row is back in place.
        *drag = WorkspaceDrag::default();
    } else {
        drag.tops = tops;
    }
}

fn creation_menu(
    ui: &mut Ui,
    p: Palette,
    group: Option<WorkspaceGroupId>,
    actions: &mut Vec<Action>,
) {
    menu_layout(ui, 230.0);
    if menu_item(ui, p, Icon::Plus, "New workspace", "", false) {
        actions.push(group.map_or(Action::New, Action::NewInGroup));
        ui.close();
    }
    if menu_item(ui, p, Icon::Globe, "New SSH workspace", "", false) {
        actions.push(group.map_or(Action::Ssh(None), Action::SshInGroup));
        ui.close();
    }
    if group.is_none() && menu_item(ui, p, Icon::Folder, "New workspace group", "", false) {
        actions.push(Action::NewGroup);
        ui.close();
    }
}

fn group_row(
    ui: &mut Ui,
    p: Palette,
    view: &ChromeView,
    group: &WorkspaceGroup,
    openness: f32,
    dragging: bool,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), GROUP_HEIGHT), Sense::hover());
    let plus = Rect::from_center_size(
        Pos2::new(row.right() - 17.0, row.center().y),
        Vec2::splat(28.0),
    );
    let response = ui.interact(
        row,
        ui.id().with(("workspace-group", group.id())),
        Sense::click_and_drag(),
    );
    let create = ui.interact(
        plus,
        ui.id().with(("group-create", group.id())),
        Sense::click(),
    );
    let hovered = !dragging && (ui.rect_contains_pointer(row) || response.context_menu_opened());
    let selected = view
        .workspaces
        .iter()
        .any(|w| w.group == Some(group.id()) && Some(w.id) == view.active);
    // Open folders show each workspace's own count; a closed one sums them.
    let unread: usize = view
        .workspaces
        .iter()
        .filter(|w| group.collapsed() && w.group == Some(group.id()))
        .map(|w| w.unread)
        .sum();
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(row, metrics::ROW_RADIUS, p.hover);
    }
    if response.has_focus() {
        painter.rect_stroke(
            response.rect,
            metrics::ROW_RADIUS,
            Stroke::new(1.5, p.accent),
            StrokeKind::Inside,
        );
    }
    let center = Pos2::new(row.left() + 12.0, row.center().y);
    let rotation = egui::emath::Rot2::from_angle(openness * std::f32::consts::FRAC_PI_2);
    painter.add(egui::Shape::line(
        vec![
            center + rotation * vec2(-2.0, -4.0),
            center + rotation * vec2(2.0, 0.0),
            center + rotation * vec2(-2.0, 4.0),
        ],
        Stroke::new(1.5, p.muted),
    ));
    icons::paint(
        painter,
        Rect::from_center_size(
            Pos2::new(row.left() + 30.0, row.center().y),
            Vec2::splat(16.0),
        ),
        Icon::Folder,
        if selected { p.accent } else { p.secondary },
    );
    galley_at(
        painter,
        Pos2::new(row.left() + 44.0, row.center().y),
        elided(
            painter,
            group.name(),
            theme::medium(13.0),
            if selected || hovered {
                p.fg
            } else {
                p.secondary
            },
            plus.left() - row.left() - 48.0,
        ),
    );
    let reveal = animate(
        ui.ctx(),
        create.id.with("reveal"),
        !dragging && (hovered || create.has_focus()),
        0.12,
    );
    if reveal > 0.0 {
        // The target spans the row's height; its highlight sits inside the row.
        let highlight = plus.shrink(3.0);
        if create.hovered() || create.is_pointer_button_down_on() {
            painter.rect_filled(highlight, 6, p.pressed);
        }
        if create.has_focus() {
            painter.rect_stroke(highlight, 6, Stroke::new(1.5, p.accent), StrokeKind::Inside);
        }
        icons::paint(
            painter,
            Rect::from_center_size(
                plus.center(),
                Vec2::splat(
                    16.0 * (0.25 + 0.75 * reveal)
                        * if create.is_pointer_button_down_on() {
                            0.96
                        } else {
                            1.0
                        },
                ),
            ),
            Icon::Plus,
            theme::tint(if create.hovered() { p.fg } else { p.secondary }, reveal),
        );
    } else {
        let count = view
            .workspaces
            .iter()
            .filter(|w| w.group == Some(group.id()))
            .count();
        if unread > 0 {
            super::notifications::pill(
                painter,
                Pos2::new(row.right() - 8.0, row.center().y),
                Align2::RIGHT_CENTER,
                unread,
                p,
            );
        } else if count > 0 {
            painter.text(
                plus.center(),
                Align2::CENTER_CENTER,
                count.to_string(),
                theme::medium(11.0),
                p.muted,
            );
        }
    }
    response.widget_info(|| {
        let label = match unread {
            0 => group.name().to_owned(),
            1 => format!("{}, 1 unread notification", group.name()),
            unread => format!("{}, {unread} unread notifications", group.name()),
        };
        WidgetInfo::selected(
            WidgetType::CollapsingHeader,
            true,
            !group.collapsed(),
            label,
        )
    });
    create.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!("New workspace in {}", group.name()),
        )
    });
    if response.double_clicked() {
        actions.push(Action::RenameGroup(group.id()));
    } else if response.clicked() {
        actions.push(Action::SetGroupCollapsed(group.id(), !group.collapsed()));
    }
    if create
        .on_hover_text(format!("New workspace in {}", group.name()))
        .clicked()
    {
        actions.push(Action::NewInGroup(group.id()));
    }
    response
        .clone()
        .on_hover_cursor(CursorIcon::PointingHand)
        .context_menu(|ui| {
            creation_menu(ui, p, Some(group.id()), actions);
            menu_separator(ui, p);
            if menu_item(ui, p, Icon::Pencil, "Rename group…", "", false) {
                actions.push(Action::RenameGroup(group.id()));
                ui.close();
            }
            if menu_item(ui, p, Icon::Folder, "Default directory…", "", false) {
                actions.push(Action::GroupDefaultDirectory(group.id()));
                ui.close();
            }
            if menu_item(ui, p, Icon::Close, "Remove group", "", false) {
                actions.push(Action::RemoveGroup(group.id()));
                ui.close();
            }
        });
    response
}

#[expect(
    clippy::too_many_arguments,
    reason = "folder presentation and row drag state stay separate"
)]
fn workspace_group(
    ui: &mut Ui,
    p: Palette,
    view: &ChromeView,
    group: &WorkspaceGroup,
    viewport: Rect,
    drag: &mut WorkspaceDrag,
    dragging: bool,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let mut collapse = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        ui.id().with("body"),
        !group.collapsed(),
    );
    collapse.set_open(!group.collapsed());
    let openness = collapse.openness(ui.ctx());
    let response = group_row(ui, p, view, group, openness, dragging, actions);
    if group.collapsed()
        && drag.lifted.is_some_and(|id| {
            view.workspaces
                .iter()
                .any(|w| w.id == id && w.group == Some(group.id()))
        })
    {
        *drag = WorkspaceDrag::default();
    }
    collapse.show_body_unindented(ui, |ui| {
        ui.set_opacity(openness);
        // A folder's children keep the row's full height and a quiet indent.
        ui.scope_builder(
            UiBuilder::new().max_rect(Rect::from_min_max(
                ui.cursor().min + vec2(14.0, 0.0),
                ui.max_rect().max,
            )),
            |ui| {
                let rows: Vec<_> = view
                    .workspaces
                    .iter()
                    .filter(|w| w.group == Some(group.id()))
                    .collect();
                let mut idle = WorkspaceDrag::default();
                let child_drag = if drag.lifted.is_none_or(|id| rows.iter().any(|w| w.id == id)) {
                    &mut *drag
                } else {
                    &mut idle
                };
                if rows.is_empty() {
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
                    ui.painter().text(
                        rect.left_center() + vec2(9.0, 0.0),
                        Align2::LEFT_CENTER,
                        "No workspaces",
                        theme::regular(11.5),
                        p.muted,
                    );
                } else {
                    workspace_rows(
                        ui,
                        p,
                        view,
                        &rows,
                        viewport.intersect(ui.clip_rect()),
                        child_drag,
                        actions,
                    );
                }
            },
        );
    });
    response
}

#[expect(
    clippy::too_many_arguments,
    reason = "shared top-level drag presentation"
)]
fn sidebar_item(
    ui: &mut Ui,
    p: Palette,
    view: &ChromeView,
    item: SidebarItem,
    viewport: Rect,
    workspace_drag: &mut WorkspaceDrag,
    lift: f32,
    dragging: bool,
    actions: &mut Vec<Action>,
) -> Option<egui::Response> {
    match item {
        SidebarItem::Group(id) => {
            let group = view.groups.iter().find(|g| g.id() == id)?;
            Some(workspace_group(
                ui,
                p,
                view,
                group,
                viewport,
                workspace_drag,
                dragging,
                actions,
            ))
        }
        SidebarItem::Workspace(id) => {
            let index = view
                .workspaces
                .iter()
                .position(|w| w.id == id && w.group.is_none())?;
            let workspace = &view.workspaces[index];
            let machine = view
                .workspaces
                .iter()
                .find(|w| Some(w.id) == view.active)
                .map(|w| &w.remote);
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
            Some(workspace_row(
                ui,
                p,
                workspace,
                view,
                RowState {
                    rect,
                    position: (index, view.workspaces.len()),
                    selected: Some(id) == view.active,
                    lift,
                    dragging,
                    pane_drag: view.pane_drag,
                    accepts: Some(&workspace.remote) == machine,
                },
                actions,
            ))
        }
    }
}

fn workspace_list(
    ui: &mut Ui,
    p: Palette,
    view: &ChromeView,
    viewport: Rect,
    workspace_drag: &mut WorkspaceDrag,
    drag: &mut SidebarDrag,
    actions: &mut Vec<Action>,
) {
    let items = view.sidebar_order;
    if workspace_drag.lifted.is_some_and(|id| {
        !view.workspaces.iter().any(|w| {
            w.id == id
                && w.group.is_some_and(|group| {
                    view.groups
                        .iter()
                        .any(|g| g.id() == group && !g.collapsed())
                })
        })
    }) {
        *workspace_drag = WorkspaceDrag::default();
    }
    let origin = drag
        .lifted
        .and_then(|id| items.iter().position(|item| *item == id));
    if origin.is_none() {
        drag.lifted = None;
        drag.grip = None;
    }
    ui.spacing_mut().item_spacing.y = GROUP_GAP;
    let spacing = GROUP_GAP;
    let trailing_gap = items.last().map_or(0.0, |item| match item {
        SidebarItem::Workspace(_) => ROW_STEP - ROW_HEIGHT,
        SidebarItem::Group(_) => spacing,
    });
    let heights: Vec<_> = items
        .iter()
        .map(|item| {
            drag.positions
                .iter()
                .find(|(id, _, _)| id == item)
                .map_or_else(
                    || match item {
                        SidebarItem::Workspace(_) => ROW_STEP,
                        SidebarItem::Group(id) => view
                            .groups
                            .iter()
                            .find(|g| g.id() == *id)
                            .map_or(GROUP_HEIGHT + spacing, |group| {
                                let count = view
                                    .workspaces
                                    .iter()
                                    .filter(|w| w.group == Some(*id))
                                    .count();
                                GROUP_HEIGHT
                                    + spacing
                                    + if group.collapsed() {
                                        0.0
                                    } else {
                                        spacing + (count as f32 * ROW_STEP - 2.0).max(28.0)
                                    }
                            }),
                    },
                    |(_, _, height)| *height,
                )
        })
        .collect();
    if origin.is_none() {
        let list_top = ui.cursor().min;
        let mut next = 0.0;
        let mut positions = Vec::with_capacity(items.len());
        for (index, item) in items.iter().copied().enumerate() {
            let rect = Rect::from_min_size(
                list_top + vec2(0.0, next),
                vec2(ui.available_width(), heights[index]),
            );
            let mut child = ui.new_child(
                UiBuilder::new()
                    .id_salt(("sidebar-item", item))
                    .max_rect(rect),
            );
            let lift = animate(ui.ctx(), child.id().with("group-lift"), false, 0.12);
            if let Some(response) = sidebar_item(
                &mut child,
                p,
                view,
                item,
                viewport,
                workspace_drag,
                lift,
                false,
                actions,
            ) {
                let height = match item {
                    SidebarItem::Workspace(_) => ROW_STEP,
                    SidebarItem::Group(_) => child.min_rect().height() + spacing,
                };
                positions.push((item, next, height));
                next += height;
                if response.drag_started_by(PointerButton::Primary)
                    && let Some(press) = ui.input(|input| input.pointer.press_origin())
                {
                    drag.lifted = Some(item);
                    drag.grip = Some(press.y - response.rect.top());
                    *workspace_drag = WorkspaceDrag::default();
                    ui.ctx().request_repaint();
                }
            }
        }
        if !positions.is_empty() {
            ui.allocate_exact_size(
                vec2(ui.available_width(), (next - trailing_gap).max(0.0)),
                Sense::hover(),
            );
        }
        drag.positions = positions;
        return;
    }
    let list = Rect::from_min_size(
        ui.cursor().min,
        vec2(ui.available_width(), heights.iter().sum::<f32>()),
    );
    let mut held = origin.zip(drag.grip).map(|(origin, grip)| {
        let pointer = ui.input(|input| input.pointer.latest_pos());
        let low = (viewport.top() - list.top()).max(0.0);
        let high = (list.height() - heights[origin])
            .min(
                viewport.bottom()
                    - list.top()
                    - match items[origin] {
                        SidebarItem::Workspace(_) => ROW_HEIGHT,
                        SidebarItem::Group(_) => GROUP_HEIGHT,
                    },
            )
            .max(low);
        let top = pointer
            .map_or(0.0, |pos| pos.y - grip - list.top())
            .clamp(low, high);
        if let Some(pointer) = pointer {
            let over = if list.top() < viewport.top() - 0.5 {
                (viewport.top() + SCROLL_REACH - pointer.y).max(0.0)
            } else {
                0.0
            } - if list.bottom() > viewport.bottom() + 0.5 {
                (pointer.y - viewport.bottom() + SCROLL_REACH).max(0.0)
            } else {
                0.0
            };
            if over != 0.0 {
                ui.scroll_with_delta_animation(
                    vec2(
                        0.0,
                        over.clamp(-2.0 * SCROLL_REACH, 2.0 * SCROLL_REACH)
                            * 12.0
                            * ui.input(|input| input.stable_dt).min(0.1),
                    ),
                    ScrollAnimation::none(),
                );
                ui.ctx().request_repaint();
            }
        }
        (origin, top)
    });
    if held.is_some() && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
        drag.grip = None;
        held = None;
        ui.ctx().stop_dragging();
    }
    let target = held.map(|(origin, top)| sidebar_drop_index(top, &heights, origin));
    let mut order: Vec<_> = (0..items.len()).collect();
    if let Some((origin, target)) = origin.zip(target) {
        order.remove(origin);
        order.insert(target, origin);
    }
    let mut rests = vec![0.0; heights.len()];
    let mut next = 0.0;
    for index in order {
        rests[index] = next;
        next += heights[index];
    }
    let decay = (-ui.input(|input| input.stable_dt).min(0.1) / ROW_SETTLE).exp();
    let mut positions = Vec::new();
    let mut moving = false;
    for index in (0..items.len())
        .filter(|index| Some(*index) != origin)
        .chain(origin)
    {
        let item = items[index];
        let rest = rests[index];
        let top = if let Some((_, top)) = held.filter(|(origin, _)| *origin == index) {
            top
        } else if origin.is_some() {
            let from = drag
                .positions
                .iter()
                .find(|(id, _, _)| *id == item)
                .map_or(rest, |(_, top, _)| *top);
            let top = rest + (from - rest) * decay;
            if (top - rest).abs() < 0.5 {
                rest
            } else {
                moving = true;
                top
            }
        } else {
            rest
        };
        let rect = Rect::from_min_size(
            Pos2::new(
                list.left(),
                (list.top() + top).round_to_pixels(ui.pixels_per_point()),
            ),
            vec2(
                list.width(),
                match item {
                    SidebarItem::Workspace(_) => ROW_HEIGHT,
                    SidebarItem::Group(_) => heights[index] - spacing,
                },
            ),
        );
        let mut child = ui.new_child(
            UiBuilder::new()
                .id_salt(("sidebar-item", item))
                .max_rect(rect),
        );
        let lift = animate(
            ui.ctx(),
            child.id().with("group-lift"),
            Some(index) == origin,
            0.12,
        );
        if lift > 0.0 && matches!(item, SidebarItem::Group(_)) {
            let painter = child
                .painter()
                .with_clip_rect(ui.clip_rect().expand2(vec2(8.0, 12.0)));
            let shadow = p.popup_shadow();
            painter.add(
                Shadow {
                    color: shadow.color.gamma_multiply(lift),
                    ..shadow
                }
                .as_shape(rect, metrics::ROW_RADIUS),
            );
            painter.rect_filled(rect, metrics::ROW_RADIUS, p.elevated.gamma_multiply(lift));
            painter.rect_stroke(
                rect,
                metrics::ROW_RADIUS,
                Stroke::new(1.0, p.border.gamma_multiply(lift)),
                StrokeKind::Inside,
            );
        }
        let mut idle = WorkspaceDrag::default();
        let response = sidebar_item(
            &mut child,
            p,
            view,
            item,
            viewport,
            if held.is_some() {
                &mut idle
            } else {
                &mut *workspace_drag
            },
            lift,
            held.is_some(),
            actions,
        );
        let height = match item {
            SidebarItem::Workspace(_) => ROW_STEP,
            SidebarItem::Group(_) => child.min_rect().height() + spacing,
        };
        positions.push((item, top, height));
        let Some(response) = response else {
            continue;
        };
        if let Some(target) = target.filter(|_| Some(index) == origin) {
            if response.drag_stopped() {
                if index != target {
                    actions.push(Action::MoveSidebarItem(item, target));
                }
                drag.grip = None;
                ui.ctx().request_repaint();
            } else if response.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else {
                drag.grip = None;
                ui.ctx().request_repaint();
            }
        } else if response.drag_started_by(PointerButton::Primary)
            && let Some(press) = ui.input(|input| input.pointer.press_origin())
        {
            drag.lifted = Some(item);
            drag.grip = Some(press.y - response.rect.top());
            *workspace_drag = WorkspaceDrag::default();
            ui.ctx().request_repaint();
        }
    }
    let height = positions.iter().map(|(_, _, height)| height).sum::<f32>();
    if !positions.is_empty() {
        ui.allocate_exact_size(
            vec2(list.width(), (height - trailing_gap).max(0.0)),
            Sense::hover(),
        );
    }
    if moving {
        ui.ctx().request_repaint();
    }
    if held.is_none() && drag.grip.is_none() && !moving {
        drag.lifted = None;
        positions.clear();
    }
    drag.positions = positions;
}

/// The window controls and the sidebar toggle, drawn over the sidebar and the
/// toolbar. The lights never move; the toggle travels with the sidebar's edge
/// between its place there and its place beside the lights.
pub fn leading_controls(ui: &mut Ui, p: Palette, view: &ChromeView, actions: &mut Vec<Action>) {
    let origin = view.window.min;
    let middle = origin.y + metrics::TOOLBAR_HEIGHT * 0.5;
    window_controls(ui, Pos2::new(origin.x + 16.0, middle), p, actions);
    if view.sidebar_available
        && icon_at(
            ui,
            Pos2::new(origin.x + toggle_offset(view).round(), middle),
            Icon::Sidebar,
            "Toggle sidebar",
            &shortcut("B"),
            "sidebar-toggle",
        )
        .clicked()
    {
        actions.push(Action::ToggleSidebar);
    }
}

/// The sidebar spans the full window height, like a native source list.
/// `state` holds its width while the edge is dragged, and a lifted workspace
/// row. While a toggle slides it, `rect` moves past the window's leading edge.
pub fn sidebar(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &ChromeView,
    state: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let drag = &mut state.sidebar_drag;
    let strip = Rect::from_min_size(rect.min, vec2(rect.width(), metrics::TOOLBAR_HEIGHT));
    drag_region(ui, strip, "sidebar-drag");
    // Register the backdrop first so workspace and folder menus take precedence.
    let background = ui.interact(
        Rect::from_min_max(
            Pos2::new(rect.left() + 2.0, strip.bottom()),
            Pos2::new(rect.right() - 4.0, rect.bottom()),
        ),
        ui.id().with("sidebar-context"),
        Sense::click(),
    );
    background.context_menu(|ui| creation_menu(ui, p, None, actions));

    let footer_top = rect.bottom() - 46.0;
    let viewport = Rect::from_min_max(
        Pos2::new(rect.left() + 8.0, strip.bottom() + LIST_TOP),
        Pos2::new(rect.right() - 8.0, footer_top - 4.0),
    );
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("workspace-list")
            .max_rect(viewport),
        |ui| {
            egui::ScrollArea::vertical()
                .id_salt("workspaces")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    workspace_list(
                        ui,
                        p,
                        view,
                        viewport,
                        &mut state.workspace_drag,
                        &mut state.item_drag,
                        actions,
                    );
                });
        },
    );

    // Footer: the primary creation action, with preferences beside it.
    let create = Rect::from_min_max(
        Pos2::new(rect.left() + 8.0, footer_top + 8.0),
        Pos2::new(rect.right() - 44.0, footer_top + 38.0),
    );
    let response = ui.interact(create, ui.id().with("workspace-create"), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "New workspace"));
    let painter = ui.painter();
    if response.is_pointer_button_down_on() {
        painter.rect_filled(create, metrics::ROW_RADIUS, p.pressed);
    } else if response.hovered() {
        painter.rect_filled(create, metrics::ROW_RADIUS, p.hover);
    }
    if response.has_focus() {
        painter.rect_stroke(
            create,
            metrics::ROW_RADIUS,
            Stroke::new(1.5, p.accent),
            egui::StrokeKind::Inside,
        );
    }
    let ink = if response.hovered() {
        p.fg
    } else {
        p.secondary
    };
    icons::paint(
        painter,
        Rect::from_center_size(
            Pos2::new(create.left() + 15.0, create.center().y),
            Vec2::splat(14.0),
        ),
        Icon::Plus,
        ink,
    );
    galley_at(
        painter,
        Pos2::new(create.left() + 31.0, create.center().y),
        elided(
            painter,
            "New workspace",
            theme::medium(12.5),
            ink,
            create.width() - 36.0,
        ),
    );
    if response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(format!("New workspace   {}", shortcut("N")))
        .clicked()
    {
        actions.push(Action::New);
    }
    if icon_at(
        ui,
        Pos2::new(rect.right() - 24.0, create.center().y),
        Icon::Settings,
        "Preferences",
        &edit_shortcut(","),
        "sidebar-preferences",
    )
    .clicked()
    {
        actions.push(Action::Settings);
    }

    // The trailing edge resizes the sidebar once it rests there; the width is
    // saved on release.
    if view.sidebar < 1.0 {
        return;
    }
    let handle = Rect::from_min_max(
        Pos2::new(rect.right() - 3.0, strip.bottom()),
        Pos2::new(rect.right() + 3.0, rect.bottom() - 12.0),
    );
    let resize = ui
        .interact(
            handle,
            ui.id().with("sidebar-resize"),
            Sense::click_and_drag(),
        )
        .on_hover_and_drag_cursor(CursorIcon::ResizeHorizontal);
    resize.widget_info(|| WidgetInfo::labeled(WidgetType::ResizeHandle, true, "Resize sidebar"));
    if resize.dragged()
        && let Some(pointer) = resize.interact_pointer_pos()
    {
        *drag = Some((pointer.x - rect.left()).clamp(*SIDEBAR_WIDTH.start(), *SIDEBAR_WIDTH.end()));
    }
    if resize.drag_stopped()
        && let Some(width) = drag.take()
    {
        actions.push(Action::SidebarWidth(width));
    }
    // A third quick click still means "reset", not a new gesture.
    if resize.double_clicked() || resize.triple_clicked() {
        *drag = None;
        actions.push(Action::SidebarWidth(DEFAULT_SIDEBAR_WIDTH));
    }
    let grip = animate(
        ui.ctx(),
        resize.id.with("grip"),
        resize.hovered() || resize.dragged(),
        0.12,
    );
    if grip > 0.0 {
        ui.painter().line_segment(
            [
                Pos2::new(rect.right(), strip.bottom() + 8.0),
                Pos2::new(rect.right(), rect.bottom() - 20.0),
            ],
            Stroke::new(
                2.0,
                theme::tint(
                    if resize.dragged() { p.accent } else { p.muted },
                    grip * 0.8,
                ),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use eframe::egui::Event;

    fn workspaces(ids: &[u64]) -> Vec<WorkspaceView> {
        ids.iter()
            .map(|id| WorkspaceView {
                unread: 0,
                alert: None,
                group: None,
                id: WorkspaceId::new(*id),
                name: format!("workspace {id}"),
                cwd: "/srv/app".into(),
                remote: None,
                branch: None,
                panes: 1,
                running: true,
            })
            .collect()
    }

    /// The vertical centre of the row at `index` in a list at rest.
    fn row(index: usize) -> Pos2 {
        Pos2::new(
            100.0,
            metrics::TOOLBAR_HEIGHT + LIST_TOP + index as f32 * ROW_STEP + ROW_HEIGHT * 0.5,
        )
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    /// A sidebar driven one frame at a time, applying moves as the app would.
    struct Fixture {
        ctx: egui::Context,
        order: Vec<u64>,
        groups: Vec<WorkspaceGroup>,
        membership: Vec<(u64, WorkspaceGroupId)>,
        sidebar_order: Vec<SidebarItem>,
        /// Window height; a short window makes the list scroll.
        height: f32,
        state: UiState,
        actions: Vec<Action>,
        /// Escape was still pending after the sidebar ran.
        escape_passed: bool,
    }

    impl Fixture {
        fn new(order: &[u64]) -> Self {
            Self::with_height(order, 600.0)
        }

        fn with_height(order: &[u64], height: f32) -> Self {
            let ctx = egui::Context::default();
            ctx.set_fonts(crate::platform::fonts::bundled_definitions());
            theme::apply(&ctx, &Config::default());
            let mut fixture = Self {
                ctx,
                order: order.into(),
                groups: Vec::new(),
                membership: Vec::new(),
                sidebar_order: order
                    .iter()
                    .map(|id| SidebarItem::Workspace(WorkspaceId::new(*id)))
                    .collect(),
                height,
                state: UiState::default(),
                actions: Vec::new(),
                escape_passed: false,
            };
            fixture.frame(vec![]);
            fixture
        }

        fn frame(&mut self, events: Vec<Event>) {
            let mut views = workspaces(&self.order);
            for workspace in &mut views {
                workspace.group = self
                    .membership
                    .iter()
                    .find(|(id, _)| *id == workspace.id.get())
                    .map(|(_, group)| *group);
            }
            let view = ChromeView {
                workspaces: &views,
                groups: &self.groups,
                sidebar_order: &self.sidebar_order,
                active: views.first().map(|workspace| workspace.id),
                pane: None,
                subtitle: "",
                pull_requests: &[],
                attached: &[],
                spawned: &[],
                worktree: None,
                ports: &[],
                pane_generation: 1,
                zoomed: false,
                window: Rect::from_min_size(Pos2::ZERO, vec2(900.0, self.height)),
                sidebar: 1.0,
                sidebar_open: true,
                sidebar_width: 216.0,
                sidebar_available: true,
                pane_drag: None,
                panel: None,
                panel_attention: false,
            };
            let p = Palette::new(Config::default().theme);
            let side = Rect::from_min_size(Pos2::ZERO, vec2(216.0, self.height));
            let mut actions = Vec::new();
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, self.height))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    sidebar(ui, side, p, &view, &mut self.state, &mut actions);
                    self.escape_passed = ui.input(|input| input.key_pressed(Key::Escape));
                },
            );
            output.textures_delta.clear();
            for action in &actions {
                let command = match action {
                    Action::SetGroupCollapsed(group, collapsed) => {
                        Some(neptune_model::Command::SetWorkspaceGroupCollapsed {
                            group: *group,
                            collapsed: *collapsed,
                        })
                    }
                    Action::MoveSidebarItem(item, index) => {
                        Some(neptune_model::Command::MoveSidebarItem {
                            item: *item,
                            index: *index,
                        })
                    }
                    Action::MoveWorkspace(workspace, index) => {
                        Some(neptune_model::Command::MoveWorkspace {
                            workspace: *workspace,
                            index: *index,
                        })
                    }
                    _ => None,
                };
                if let Some(command) = command {
                    let mut controller = neptune_model::Controller::new(self.model());
                    controller.dispatch(command).unwrap();
                    self.order = controller
                        .model()
                        .workspaces()
                        .iter()
                        .map(|w| w.id().get())
                        .collect();
                    self.groups = controller.model().groups().to_owned();
                    self.sidebar_order = controller.model().sidebar_order().to_owned();
                }
            }
            self.actions.extend(actions);
        }

        fn model(&self) -> neptune_model::Model {
            let specs = self
                .order
                .iter()
                .map(|id| neptune_model::WorkspaceSpec {
                    id: WorkspaceId::new(*id),
                    group: self
                        .membership
                        .iter()
                        .find(|(w, _)| w == id)
                        .map(|(_, g)| *g),
                    name: format!("workspace {id}"),
                    cwd: "/srv/app".into(),
                    remote: None,
                    panes: vec![neptune_model::PaneSpec {
                        id: PaneId::new(*id),
                        cwd: "/srv/app".into(),
                        remote_cwd: None,
                        agent: None,
                        pull_requests: Vec::new(),
                        attachments: Vec::new(),
                        spawned_by: None,
                        worktree: None,
                    }],
                    layout: neptune_model::Layout::pane(PaneId::new(*id)),
                    active: PaneId::new(*id),
                })
                .collect();
            neptune_model::Model::restore_ordered(
                specs,
                self.groups
                    .iter()
                    .map(|g| neptune_model::WorkspaceGroupSpec {
                        id: g.id(),
                        name: g.name().into(),
                        collapsed: g.collapsed(),
                        default_directory: g.default_directory().map(std::path::Path::to_path_buf),
                    })
                    .collect(),
                Some(self.sidebar_order.clone()),
                None,
                true,
                Default::default(),
            )
            .unwrap()
        }

        /// Presses a row and carries it to `to` over several frames.
        fn carry(&mut self, from: Pos2, to: Pos2) {
            self.frame(vec![Event::PointerMoved(from)]);
            self.frame(vec![button(from, true)]);
            for step in 1..=6 {
                self.frame(vec![Event::PointerMoved(from.lerp(to, step as f32 / 6.0))]);
            }
        }

        fn folders(&mut self, folders: &[(u64, bool)]) {
            self.groups = neptune_model::Model::restore_grouped(
                Vec::new(),
                folders
                    .iter()
                    .map(|(id, collapsed)| neptune_model::WorkspaceGroupSpec {
                        id: WorkspaceGroupId::new(*id),
                        name: format!("group {id}"),
                        collapsed: *collapsed,
                        default_directory: None,
                    })
                    .collect(),
                None,
                true,
                Default::default(),
            )
            .unwrap()
            .groups()
            .to_owned();
            self.sidebar_order = self
                .order
                .iter()
                .filter(|id| !self.membership.iter().any(|(w, _)| w == *id))
                .map(|id| SidebarItem::Workspace(WorkspaceId::new(*id)))
                .chain(self.groups.iter().map(|g| SidebarItem::Group(g.id())))
                .collect();
            self.order = self
                .model()
                .workspaces()
                .iter()
                .map(|w| w.id().get())
                .collect();
            for _ in 0..20 {
                self.frame(vec![]);
            }
        }

        fn group_moves(&self) -> Vec<(u64, usize)> {
            self.actions
                .iter()
                .filter_map(|a| match a {
                    Action::MoveSidebarItem(SidebarItem::Group(id), index) => {
                        Some((id.get(), *index))
                    }
                    _ => None,
                })
                .collect()
        }

        fn folder(&self, id: u64) -> Pos2 {
            let (_, top, _) = self
                .state
                .item_drag
                .positions
                .iter()
                .find(|(g, _, _)| *g == SidebarItem::Group(WorkspaceGroupId::new(id)))
                .unwrap();
            Pos2::new(
                100.0,
                metrics::TOOLBAR_HEIGHT + LIST_TOP + top + GROUP_HEIGHT * 0.5,
            )
        }

        fn top(&self, id: u64) -> Option<f32> {
            self.state
                .item_drag
                .positions
                .iter()
                .find(|(item, _, _)| *item == SidebarItem::Workspace(WorkspaceId::new(id)))
                .map(|(_, top, _)| *top)
        }

        fn moves(&self) -> Vec<(u64, usize)> {
            self.actions
                .iter()
                .filter_map(|action| match action {
                    Action::MoveWorkspace(id, index)
                    | Action::MoveSidebarItem(SidebarItem::Workspace(id), index) => {
                        Some((id.get(), *index))
                    }
                    _ => None,
                })
                .collect()
        }
    }

    #[test]
    fn folder_plus_creates_inside_its_group_without_toggling_or_selecting() {
        let mut sidebar = Fixture::new(&[1, 2]);
        let group = WorkspaceGroupId::new(7);
        sidebar.groups = neptune_model::Model::restore_grouped(
            Vec::new(),
            vec![neptune_model::WorkspaceGroupSpec {
                id: group,
                default_directory: None,
                name: "Projects".into(),
                collapsed: false,
            }],
            None,
            true,
            Default::default(),
        )
        .unwrap()
        .groups()
        .to_owned();
        sidebar.membership = vec![(2, group)];
        sidebar.sidebar_order = vec![
            SidebarItem::Workspace(WorkspaceId::new(1)),
            SidebarItem::Group(group),
        ];
        sidebar.frame(vec![]);
        let header_y = sidebar.folder(7).y;
        let plus = Pos2::new(216.0 - 8.0 - 17.0, header_y);
        sidebar.frame(vec![Event::PointerMoved(plus)]);
        sidebar.frame(vec![button(plus, true)]);
        sidebar.frame(vec![button(plus, false)]);
        assert!(matches!(sidebar.actions.as_slice(), [Action::NewInGroup(id)] if *id == group));
        sidebar.actions.clear();
        let header = Pos2::new(100.0, header_y);
        sidebar.frame(vec![Event::PointerMoved(header)]);
        sidebar.frame(vec![button(header, true)]);
        sidebar.frame(vec![button(header, false)]);
        assert!(
            matches!(sidebar.actions.as_slice(), [Action::SetGroupCollapsed(id, true)] if *id == group)
        );
        assert!(sidebar.groups[0].collapsed());
    }

    #[test]
    fn a_folder_can_be_dragged_above_ungrouped_workspaces() {
        let mut sidebar = Fixture::new(&[1, 2, 3]);
        sidebar.membership = vec![(3, WorkspaceGroupId::new(7))];
        sidebar.folders(&[(7, false)]);
        let from = sidebar.folder(7);
        let to = row(0);
        sidebar.carry(from, to);
        assert!(sidebar.group_moves().is_empty());
        sidebar.frame(vec![button(to, false)]);
        assert_eq!(sidebar.group_moves(), [(7, 0)]);
        assert_eq!(sidebar.order, [3, 1, 2]);
        assert_eq!(
            sidebar.sidebar_order[0],
            SidebarItem::Group(WorkspaceGroupId::new(7))
        );
    }

    #[test]
    fn mixed_folders_and_workspaces_move_in_both_directions_and_cancel() {
        let mut sidebar = Fixture::new(&[1, 2, 3, 4]);
        sidebar.membership = vec![(3, WorkspaceGroupId::new(7)), (4, WorkspaceGroupId::new(7))];
        sidebar.folders(&[(7, false), (8, true)]);
        let to = row(1);
        sidebar.carry(sidebar.folder(7), to);
        sidebar.frame(vec![button(to, false)]);
        assert_eq!(
            sidebar.sidebar_order,
            [
                SidebarItem::Workspace(WorkspaceId::new(1)),
                SidebarItem::Group(WorkspaceGroupId::new(7)),
                SidebarItem::Workspace(WorkspaceId::new(2)),
                SidebarItem::Group(WorkspaceGroupId::new(8))
            ]
        );
        assert_eq!(sidebar.order, [1, 3, 4, 2]);
        for _ in 0..30 {
            sidebar.frame(vec![]);
        }
        let to = sidebar.folder(8) + vec2(0.0, GROUP_HEIGHT);
        sidebar.carry(row(0), to);
        sidebar.frame(vec![button(to, false)]);
        assert_eq!(
            sidebar.sidebar_order.last(),
            Some(&SidebarItem::Workspace(WorkspaceId::new(1)))
        );
        for _ in 0..30 {
            sidebar.frame(vec![]);
        }
        let from = sidebar.folder(8);
        sidebar.carry(from, row(0));
        sidebar.frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        sidebar.frame(vec![button(row(0), false)]);
        assert!(!sidebar.escape_passed);
        assert_eq!(sidebar.group_moves(), [(7, 1)]);
        assert!(
            sidebar
                .groups
                .iter()
                .find(|g| g.id().get() == 8)
                .unwrap()
                .collapsed()
        );
    }

    #[test]
    fn grouped_workspace_drag_keeps_membership_and_mixed_sidebar_order() {
        let mut sidebar = Fixture::new(&[1, 2, 3]);
        sidebar.membership = vec![(2, WorkspaceGroupId::new(7)), (3, WorkspaceGroupId::new(7))];
        sidebar.folders(&[(7, false)]);
        sidebar.sidebar_order.swap(0, 1);
        sidebar.order = sidebar
            .model()
            .workspaces()
            .iter()
            .map(|w| w.id().get())
            .collect();
        sidebar.frame(vec![]);
        let before = sidebar.sidebar_order.clone();
        let from = sidebar.folder(7) + vec2(0.0, GROUP_HEIGHT * 0.5 + GROUP_GAP + ROW_HEIGHT * 0.5);
        let to = from + vec2(0.0, ROW_STEP);
        sidebar.carry(from, to);
        sidebar.frame(vec![button(to, false)]);
        assert_eq!(sidebar.moves(), [(2, 1)]);
        assert_eq!(sidebar.order, [3, 2, 1]);
        assert_eq!(sidebar.sidebar_order, before);
        assert!(sidebar.group_moves().is_empty());
    }

    #[test]
    fn removing_the_held_folder_cancels_its_drag() {
        let mut sidebar = Fixture::new(&[1, 2]);
        sidebar.folders(&[(7, true)]);
        sidebar.carry(sidebar.folder(7), row(0));
        sidebar.groups.clear();
        sidebar
            .sidebar_order
            .retain(|item| !matches!(item, SidebarItem::Group(_)));
        sidebar.frame(vec![button(row(0), false)]);
        assert!(sidebar.group_moves().is_empty());
        assert!(sidebar.state.item_drag.lifted.is_none());
    }

    #[test]
    fn expanded_and_collapsed_folders_move_as_blocks_once_on_release() {
        let mut sidebar = Fixture::new(&[1, 2, 3]);
        sidebar.membership = vec![
            (1, WorkspaceGroupId::new(7)),
            (2, WorkspaceGroupId::new(7)),
            (3, WorkspaceGroupId::new(8)),
        ];
        sidebar.folders(&[(7, false), (8, true), (9, false)]);
        let from = sidebar.folder(7);
        let to = sidebar.folder(9);
        sidebar.carry(from, to);
        assert!(sidebar.group_moves().is_empty());
        assert_eq!(
            sidebar.state.item_drag.lifted,
            Some(SidebarItem::Group(WorkspaceGroupId::new(7)))
        );
        sidebar.frame(vec![button(to, false)]);
        assert_eq!(sidebar.group_moves(), [(7, 2)]);
        assert_eq!(
            sidebar
                .groups
                .iter()
                .map(|g| g.id().get())
                .collect::<Vec<_>>(),
            [8, 9, 7]
        );
        assert_eq!(sidebar.order, [3, 1, 2]);
        assert!(sidebar.moves().is_empty());
        assert!(!sidebar.actions.iter().any(|a| matches!(
            a,
            Action::SetGroupCollapsed(..) | Action::SelectWorkspace(..) | Action::NewInGroup(..)
        )));
        for _ in 0..30 {
            sidebar.frame(vec![]);
        }
        assert!(sidebar.state.item_drag.lifted.is_none());
        let from = sidebar.folder(7);
        let to = sidebar.folder(8);
        sidebar.carry(from, to);
        sidebar.frame(vec![button(to, false)]);
        assert_eq!(sidebar.group_moves(), [(7, 2), (7, 0)]);
        assert!(sidebar.groups[1].collapsed());
    }

    #[test]
    fn cancelling_a_folder_drag_or_dropping_in_place_keeps_order() {
        let mut sidebar = Fixture::new(&[]);
        sidebar.folders(&[(7, true), (8, true), (9, true)]);
        let from = sidebar.folder(9);
        let to = sidebar.folder(7);
        sidebar.carry(from, to);
        sidebar.frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        assert!(!sidebar.escape_passed);
        assert!(sidebar.state.item_drag.grip.is_none());
        sidebar.frame(vec![
            Event::PointerMoved(to + vec2(0.0, 15.0)),
            button(to, false),
        ]);
        for _ in 0..30 {
            sidebar.frame(vec![]);
        }
        assert!(sidebar.group_moves().is_empty());
        let from = sidebar.folder(8);
        let to = from + vec2(0.0, 8.0);
        sidebar.carry(from, to);
        sidebar.frame(vec![button(to, false)]);
        assert!(sidebar.group_moves().is_empty());
        assert_eq!(
            sidebar
                .groups
                .iter()
                .map(|g| g.id().get())
                .collect::<Vec<_>>(),
            [7, 8, 9]
        );
    }

    #[test]
    fn holding_a_folder_at_the_edge_scrolls_to_hidden_groups() {
        let mut sidebar = Fixture::with_height(&[], 300.0);
        sidebar.folders(&[
            (1, true),
            (2, true),
            (3, true),
            (4, true),
            (5, true),
            (6, true),
            (7, true),
            (8, true),
        ]);
        let from = sidebar.folder(1);
        let edge = Pos2::new(100.0, 300.0 - 54.0);
        sidebar.carry(from, edge);
        for _ in 0..120 {
            sidebar.frame(vec![]);
        }
        sidebar.frame(vec![button(edge, false)]);
        assert_eq!(sidebar.group_moves(), [(1, 7)]);
    }

    #[test]
    fn rows_make_room_for_a_dragged_workspace() {
        assert_eq!(drop_index(-40.0, 4), 0);
        assert_eq!(drop_index(ROW_STEP * 0.49, 4), 0);
        assert_eq!(drop_index(ROW_STEP * 0.51, 4), 1);
        assert_eq!(drop_index(ROW_STEP * 9.0, 4), 3);
        assert_eq!(drop_index(0.0, 0), 0);
        let rests = |origin, target| -> Vec<usize> {
            (0..5).map(|i| displaced(i, origin, target)).collect()
        };
        // The held row's own entry is unused; its neighbours close the gap.
        assert_eq!(rests(1, 3), [0, 1, 1, 2, 4]);
        assert_eq!(rests(3, 0), [1, 2, 3, 3, 4]);
        assert_eq!(rests(2, 2), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn dragging_a_row_moves_its_workspace_once_and_the_list_settles() {
        let mut sidebar = Fixture::new(&[1, 2, 3]);
        let to = row(2) + vec2(0.0, 6.0);
        sidebar.carry(row(0), to);
        assert_eq!(
            sidebar.state.item_drag.lifted,
            Some(SidebarItem::Workspace(WorkspaceId::new(1)))
        );
        assert!(sidebar.moves().is_empty(), "nothing moves before release");
        // The held row tracks the pointer; its neighbours ease up to make room.
        assert_eq!(sidebar.top(1), Some(2.0 * ROW_STEP));
        for _ in 0..20 {
            sidebar.frame(vec![]);
        }
        assert_eq!(sidebar.top(2), Some(0.0));
        assert_eq!(sidebar.top(3), Some(ROW_STEP));

        sidebar.frame(vec![button(to, false)]);
        assert_eq!(sidebar.moves(), [(1, 2)]);
        assert_eq!(sidebar.order, [2, 3, 1]);
        for _ in 0..20 {
            sidebar.frame(vec![]);
        }
        let rest = &sidebar.state.item_drag;
        assert!(rest.lifted.is_none() && rest.grip.is_none());
        assert_eq!(sidebar.moves().len(), 1);
        assert!(
            !sidebar
                .actions
                .iter()
                .any(|action| matches!(action, Action::SelectWorkspace(_))),
            "a drag is not a click"
        );
    }

    #[test]
    fn a_click_still_selects_and_a_row_dropped_in_place_does_not_move() {
        let mut sidebar = Fixture::new(&[1, 2, 3]);
        sidebar.frame(vec![Event::PointerMoved(row(1))]);
        sidebar.frame(vec![button(row(1), true)]);
        sidebar.frame(vec![button(row(1), false)]);
        assert!(matches!(
            sidebar.actions[..],
            [Action::SelectWorkspace(id)] if id == WorkspaceId::new(2)
        ));

        // Lifted, but released before it passes the middle of a neighbour.
        let to = row(1) + vec2(0.0, 20.0);
        sidebar.carry(row(1), to);
        assert_eq!(
            sidebar.state.item_drag.lifted,
            Some(SidebarItem::Workspace(WorkspaceId::new(2)))
        );
        sidebar.frame(vec![button(to, false)]);
        assert!(sidebar.moves().is_empty());
        assert_eq!(sidebar.order, [1, 2, 3]);
    }

    #[test]
    fn holding_a_row_at_the_edge_scrolls_to_positions_out_of_view() {
        // Just over four of eight rows fit: the last position starts hidden.
        let mut sidebar = Fixture::with_height(&[1, 2, 3, 4, 5, 6, 7, 8], 300.0);
        let edge = Pos2::new(100.0, 300.0 - 54.0);
        sidebar.carry(row(0), edge);
        let reachable = sidebar.top(1).unwrap();
        assert!(reachable < 4.0 * ROW_STEP, "the row stays inside the view");
        for _ in 0..120 {
            sidebar.frame(vec![]);
        }
        assert_eq!(sidebar.top(1), Some(7.0 * ROW_STEP));
        sidebar.frame(vec![button(edge, false)]);
        assert_eq!(sidebar.moves(), [(1, 7)]);

        // And back up: the list scrolls the other way under a row held at the top.
        for _ in 0..20 {
            sidebar.frame(vec![]);
        }
        let top = Pos2::new(100.0, metrics::TOOLBAR_HEIGHT + LIST_TOP);
        sidebar.carry(edge - vec2(0.0, 20.0), top);
        for _ in 0..120 {
            sidebar.frame(vec![]);
        }
        sidebar.frame(vec![button(top, false)]);
        assert_eq!(sidebar.moves(), [(1, 7), (1, 0)]);
        assert_eq!(sidebar.order, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn escape_puts_a_dragged_row_back_and_is_not_passed_on() {
        let mut sidebar = Fixture::new(&[1, 2, 3]);
        let to = row(0) + vec2(0.0, 6.0);
        sidebar.carry(row(2), to);
        assert!(sidebar.state.item_drag.grip.is_some());
        sidebar.frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        assert!(!sidebar.escape_passed, "the shell must not see this Escape");
        assert!(sidebar.state.item_drag.grip.is_none());
        sidebar.frame(vec![button(to, false)]);
        assert!(sidebar.moves().is_empty());
        assert_eq!(sidebar.order, [1, 2, 3]);
    }

    #[test]
    fn a_toggled_sidebar_eases_to_rest_and_reverses_from_where_it_is() {
        let hide = SidebarSlide::toggled(None, true, 10.0);
        assert_eq!(hide.reveal(false, 10.0), Some(1.0));
        let midway = hide.reveal(false, 10.08).unwrap();
        assert!(
            midway > 0.0 && midway < 0.5,
            "ease-out covers most of the way early: {midway}"
        );
        assert_eq!(hide.reveal(false, 10.0 + SIDEBAR_SLIDE), None);

        // Toggled back midway: the slide continues from where the sidebar is.
        let show = SidebarSlide::toggled(Some(hide), false, 10.08);
        assert_eq!(show.reveal(true, 10.08), Some(midway));
        assert!(show.reveal(true, 10.12).unwrap() > midway);
        assert_eq!(show.reveal(true, 10.08 + SIDEBAR_SLIDE), None);

        // A finished slide leaves nothing to continue from.
        assert_eq!(
            SidebarSlide::toggled(Some(hide), false, 11.0).reveal(true, 11.0),
            Some(0.0)
        );
    }

    #[test]
    fn a_carried_terminal_drops_on_another_workspace_row_but_not_its_own() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let config = crate::config::Config::default();
        theme::apply(&ctx, &config);
        let workspaces: Vec<WorkspaceView> = [(1, None), (2, None), (3, Some("me@devbox"))]
            .into_iter()
            .map(|(id, remote): (u64, Option<&str>)| WorkspaceView {
                unread: 0,
                alert: None,
                group: None,
                id: WorkspaceId::new(id),
                name: format!("workspace {id}"),
                cwd: "/srv/app".into(),
                remote: remote.map(str::to_owned),
                branch: None,
                panes: 2,
                running: true,
            })
            .collect();
        let pane = PaneId::new(7);
        let frame = |drag: Option<PaneId>, events: Vec<egui::Event>| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    sidebar(
                        ui,
                        Rect::from_min_size(Pos2::ZERO, vec2(216.0, 600.0)),
                        Palette::for_config(&config),
                        &ChromeView {
                            workspaces: &workspaces,
                            groups: &[],
                            sidebar_order: &[
                                SidebarItem::Workspace(WorkspaceId::new(1)),
                                SidebarItem::Workspace(WorkspaceId::new(2)),
                                SidebarItem::Workspace(WorkspaceId::new(3)),
                            ],
                            active: Some(WorkspaceId::new(1)),
                            pane: Some(pane),
                            subtitle: "",
                            pull_requests: &[],
                            attached: &[],
                            spawned: &[],
                            worktree: None,
                            ports: &[],
                            pane_generation: 1,
                            zoomed: false,
                            window: Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0)),
                            sidebar: 1.0,
                            sidebar_open: true,
                            sidebar_width: 216.0,
                            sidebar_available: true,
                            pane_drag: drag,
                            panel: None,
                            panel_attention: false,
                        },
                        &mut UiState::default(),
                        &mut actions,
                    );
                },
            );
            output.textures_delta.clear();
            actions
        };
        // Rows sit one step apart below the toolbar strip.
        let own = Pos2::new(100.0, metrics::TOOLBAR_HEIGHT + LIST_TOP + ROW_HEIGHT * 0.5);
        let other = own + vec2(0.0, ROW_STEP);
        let on_a_host = other + vec2(0.0, ROW_STEP);
        let release = |pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        };
        let press = |pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        frame(None, vec![]);
        for (pos, drag, expected) in [
            (other, Some(pane), Some(WorkspaceId::new(2))),
            (own, Some(pane), None),
            // Its session is local and would not follow it to another machine.
            (on_a_host, Some(pane), None),
            // Without a carried terminal a release is an ordinary click.
            (other, None, None),
        ] {
            frame(drag, vec![egui::Event::PointerMoved(pos), press(pos)]);
            let moved: Vec<_> = frame(drag, vec![release(pos)])
                .into_iter()
                .filter_map(|action| match action {
                    Action::MovePane(pane, Destination::Workspace(workspace)) => {
                        Some((pane, workspace))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                moved,
                expected
                    .map(|workspace| (pane, workspace))
                    .into_iter()
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn workspace_tiles_use_a_visible_initial() {
        assert_eq!(initial("sandbox"), "S");
        assert_eq!(initial("  ~/日本語"), "日");
        assert_eq!(initial("ßeta"), "SS");
        assert_eq!(initial("---"), "·");
    }
}
