//! The agents tab: every CLI agent running in a terminal of any workspace,
//! grouped by what it is doing. Agents waiting for a person come first.
use super::Action;
use super::helpers::{elided, galley_at, menu_item, menu_layout};
use crate::{
    agent_activity::{Activity, Attention},
    icons::Icon,
    theme::{self, Palette},
};
use eframe::egui::{
    self, Align2, CursorIcon, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, WidgetInfo, WidgetType,
    vec2,
};
use neptune_model::{AgentKind, PaneId};
use std::time::Duration;

pub const ROW: f32 = 46.0;
/// The heading above each group of rows.
pub const HEADING: f32 = 28.0;

pub struct Row {
    /// The terminal the agent runs in, as it was when the row was made.
    pub pane: PaneId,
    pub generation: u64,
    pub kind: AgentKind,
    pub activity: Activity,
    /// How long the agent has been doing it.
    pub elapsed: Duration,
    /// What the agent calls its conversation, or its own name without one.
    pub title: String,
    pub workspace: String,
    pub folder: String,
    /// The agent's terminal is the focused one.
    pub focused: bool,
    /// Its terminal has a tab that can be put away again.
    pub can_background: bool,
}

/// An agent that the agent of a terminal started, for that terminal's tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawned {
    /// The terminal the started agent runs in.
    pub pane: PaneId,
    /// What the agent calls its conversation, or its own name without one.
    pub title: String,
    /// `None` until its CLI has opened.
    pub activity: Option<Activity>,
    /// `Some` until its CLI has taken its task: whether it stands at a
    /// question of its own, which a person answers in its terminal.
    pub starting: Option<bool>,
    /// Its terminal has a tab that can be put away again.
    pub can_background: bool,
}
impl Spawned {
    pub fn state(&self) -> &'static str {
        match self.starting {
            Some(true) => "Needs your answer",
            Some(false) => "Starting",
            None => self.activity.map_or("Starting", activity_name),
        }
    }
    /// It waits for a person.
    pub fn waits(&self) -> bool {
        self.starting == Some(true)
            || (self.starting.is_none() && matches!(self.activity, Some(Activity::NeedsInput(_))))
    }
}

pub struct View<'a> {
    pub rows: &'a [Row],
    pub window: Rect,
    /// How much of the panel is shown while a toggle slides it.
    pub reveal: f32,
}

pub fn kind_name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude Code",
        AgentKind::Codex => "Codex",
        AgentKind::Opencode => "OpenCode",
        AgentKind::Gemini => "Gemini CLI",
        AgentKind::Pi => "pi",
        AgentKind::Omp => "Oh My Pi",
    }
}

/// The state in words, as the row and its accessible name say it.
pub fn activity_name(activity: Activity) -> &'static str {
    match activity {
        Activity::Working => "Working",
        Activity::Idle => "Idle",
        Activity::NeedsInput(Attention::Permission) => "Needs permission",
        Activity::NeedsInput(Attention::Question) => "Asked a question",
        Activity::NeedsInput(Attention::Plan) => "Plan needs approval",
        Activity::NeedsInput(Attention::Input) => "Needs input",
    }
}

/// A short age: "now", then whole minutes, hours and days.
pub fn elapsed_label(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        0..=59 => "now".into(),
        seconds @ 60..=3599 => format!("{}m", seconds / 60),
        seconds @ 3600..=86_399 => format!("{}h", seconds / 3600),
        seconds => format!("{}d", seconds / 86_400),
    }
}

/// What a row about an agent offers on a secondary click: its terminal, and
/// for one opened as a tab the way back out of view. The target is the
/// terminal as it was when the row was made.
pub fn row_menu(
    response: &egui::Response,
    p: Palette,
    (pane, generation): (PaneId, u64),
    can_background: bool,
    actions: &mut Vec<Action>,
) {
    response.context_menu(|ui| {
        menu_layout(ui, 200.0);
        if menu_item(ui, p, Icon::Terminal, "Open terminal", "", false) {
            actions.push(Action::OpenAgent(pane, generation));
            ui.close();
        }
        if can_background && menu_item(ui, p, Icon::Minus, BACKGROUND, "", false) {
            actions.push(Action::Background(pane));
            ui.close();
        }
    });
}

/// What sends a terminal's tab out of view, wherever it is offered.
pub const BACKGROUND: &str = "Send to background";

/// The order of the groups, and each one's heading.
fn group(activity: Activity) -> (u8, &'static str) {
    match activity {
        Activity::NeedsInput(_) => (0, "Needs input"),
        Activity::Working => (1, "Working"),
        Activity::Idle => (2, "Idle"),
    }
}

/// The mark that leads a row: filled while the agent wants a person, a ring
/// while it works on its own, a faint ring at rest.
fn mark(painter: &egui::Painter, centre: Pos2, p: Palette, activity: Activity) {
    match activity {
        Activity::NeedsInput(_) => {
            painter.circle_filled(centre, 4.0, p.attention);
        }
        Activity::Working => {
            painter.circle_filled(centre, 4.0, theme::tint(p.green, 0.28));
            painter.circle_filled(centre, 2.2, p.green);
        }
        Activity::Idle => {
            painter.circle_stroke(centre, 3.5, Stroke::new(1.5, p.muted));
        }
    }
}

fn row(ui: &mut Ui, rect: Rect, p: Palette, row: &Row, actions: &mut Vec<Action>) {
    let response = ui
        .interact(rect, egui::Id::new(("agent-row", row.pane)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let state = activity_name(row.activity);
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!(
                "Agent {}, {state}, {}: {}",
                kind_name(row.kind),
                row.workspace,
                row.title
            ),
        )
    });
    let fill = if response.is_pointer_button_down_on() {
        Some(p.pressed)
    } else if response.hovered() {
        Some(p.hover)
    } else if row.focused {
        Some(theme::tint(p.fg, 0.06))
    } else {
        None
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if let Some(fill) = fill {
        painter.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 7, fill);
    }
    let first = rect.top() + 15.0;
    let second = rect.top() + 31.0;
    mark(
        &painter,
        Pos2::new(rect.left() + 13.0, first),
        p,
        row.activity,
    );
    let left = rect.left() + 26.0;
    let age = painter.layout_no_wrap(elapsed_label(row.elapsed), theme::regular(11.0), p.muted);
    let age_left = rect.right() - 8.0 - age.size().x;
    painter.galley(
        Pos2::new(age_left, first - age.size().y * 0.5),
        age,
        p.muted,
    );
    let title = elided(
        &painter,
        &row.title,
        theme::medium(12.5),
        p.fg,
        age_left - 8.0 - left,
    );
    galley_at(&painter, Pos2::new(left, first), title);
    // The state leads the second line, in words as well as in the mark.
    let waiting = matches!(row.activity, Activity::NeedsInput(_));
    let state = painter.layout_no_wrap(
        state.to_owned(),
        theme::medium(11.5),
        if waiting { p.attention } else { p.secondary },
    );
    let state_right = left + state.size().x;
    galley_at(&painter, Pos2::new(left, second), state);
    // An agent that has not named its conversation is titled by its own name.
    let place = if row.title == kind_name(row.kind) {
        format!("  ·  {}", row.workspace)
    } else {
        format!("  ·  {}  ·  {}", kind_name(row.kind), row.workspace)
    };
    let place = elided(
        &painter,
        &place,
        theme::regular(11.5),
        p.muted,
        rect.right() - 8.0 - state_right,
    );
    galley_at(&painter, Pos2::new(state_right, second), place);
    if response.clicked() {
        actions.push(Action::OpenAgent(row.pane, row.generation));
    }
    row_menu(
        &response,
        p,
        (row.pane, row.generation),
        row.can_background,
        actions,
    );
    response.on_hover_text(format!("{} in {}", kind_name(row.kind), row.folder));
}

/// The list in `rect`, below the panel's tabs.
pub fn show(ui: &mut Ui, rect: Rect, p: Palette, view: &View, actions: &mut Vec<Action>) {
    let mut child = ui.new_child(UiBuilder::new().id_salt("agents").max_rect(rect));
    child.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(view.window));
    child.multiply_opacity(view.reveal);
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 6.0, rect.top()),
        Pos2::new(rect.right() - 8.0, rect.bottom() - 8.0),
    );
    if view.rows.is_empty() {
        let galley = child.painter().layout(
            "No agents are running.\nStart a coding agent such as claude, codex or opencode in a terminal and it is listed here."
                .to_owned(),
            theme::regular(12.0),
            p.muted,
            (inner.width() - 24.0).max(40.0),
        );
        let pos = Pos2::new(
            inner.center().x - galley.size().x * 0.5,
            (inner.top() + 28.0).min(inner.center().y - galley.size().y * 0.5),
        );
        child
            .painter()
            .with_clip_rect(inner)
            .galley(pos, galley, p.muted);
        return;
    }
    let mut rows: Vec<&Row> = view.rows.iter().collect();
    // Stable, so agents of one group keep the order of their workspaces.
    rows.sort_by_key(|row| group(row.activity).0);
    let mut list = child.new_child(UiBuilder::new().id_salt("agents-list").max_rect(inner));
    egui::ScrollArea::vertical()
        .id_salt("agents-scroll")
        .auto_shrink([false, false])
        .show(&mut list, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut heading = None;
            for item in rows {
                let (order, name) = group(item.activity);
                if heading != Some(order) {
                    heading = Some(order);
                    let count = view
                        .rows
                        .iter()
                        .filter(|row| group(row.activity).0 == order)
                        .count();
                    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), HEADING));
                    let painter = ui.painter();
                    let label = painter.text(
                        Pos2::new(rect.left() + 8.0, rect.center().y + 3.0),
                        Align2::LEFT_CENTER,
                        name,
                        theme::medium(11.5),
                        p.secondary,
                    );
                    painter.text(
                        Pos2::new(label.right() + 6.0, rect.center().y + 3.0),
                        Align2::LEFT_CENTER,
                        count.to_string(),
                        theme::regular(11.5),
                        p.muted,
                    );
                }
                let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW));
                if ui.is_rect_visible(rect) {
                    row(ui, rect, p, item, actions);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_are_short_and_never_tick_in_seconds() {
        for (seconds, label) in [
            (0, "now"),
            (59, "now"),
            (60, "1m"),
            (3599, "59m"),
            (3600, "1h"),
            (86_399, "23h"),
            (86_400, "1d"),
            (400_000, "4d"),
        ] {
            assert_eq!(elapsed_label(Duration::from_secs(seconds)), label);
        }
    }

    #[test]
    fn every_state_is_named_in_words_and_waiting_agents_lead() {
        let states = [
            Activity::NeedsInput(Attention::Permission),
            Activity::NeedsInput(Attention::Question),
            Activity::NeedsInput(Attention::Plan),
            Activity::NeedsInput(Attention::Input),
            Activity::Working,
            Activity::Idle,
        ];
        let names: std::collections::BTreeSet<_> =
            states.iter().map(|state| activity_name(*state)).collect();
        assert_eq!(names.len(), states.len());
        let order: Vec<_> = states.iter().map(|state| group(*state).0).collect();
        assert_eq!(order, [0, 0, 0, 0, 1, 2]);
    }
}
