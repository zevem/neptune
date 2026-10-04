//! The panel at the window's trailing edge. It holds the file explorer and
//! the running agents as tabs, and owns what they share: the toolbar control,
//! the slide, the width and the edge that resizes it.
use super::helpers::{animate, elided, galley_at};
use super::{Action, agents, chrome::SidebarSlide, explorer};
use crate::{
    icons::{self, Icon},
    theme::{self, Palette},
};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Ui, UiBuilder, WidgetInfo, WidgetType, vec2};
use std::ops::RangeInclusive;

pub const DEFAULT_WIDTH: f32 = 300.0;
pub const WIDTH: RangeInclusive<f32> = 220.0..=560.0;
/// The terminals keep at least this much of the window beside the panel.
pub const MIN_STAGE: f32 = 240.0;
/// The strip of tabs above the panel's content.
pub const TABS: f32 = 34.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Files,
    Agents,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Agents => "Agents",
        }
    }
}

pub struct State {
    pub open: bool,
    /// A toggle still sliding into place.
    pub slide: Option<SidebarSlide>,
    pub width: f32,
    pub tab: Tab,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            slide: None,
            width: DEFAULT_WIDTH,
            tab: Tab::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Show or hide the panel.
    Toggle,
    /// Bring a tab into view, opening the panel for it.
    Show(Tab),
}

/// A dot in the attention colour over a control, cut out of what is beneath.
fn attention_dot(ui: &Ui, p: Palette, response: &egui::Response, on: bool, pressed: bool) {
    let dot = animate(ui.ctx(), response.id.with("attention"), on, 0.16);
    if dot <= 0.0 {
        return;
    }
    let beneath = if pressed || response.is_pointer_button_down_on() {
        p.pressed
    } else if response.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    let centre = response.rect.center() + vec2(7.0, -6.0);
    let painter = ui.painter();
    painter.circle_filled(centre, 5.0 * dot, p.chrome);
    painter.circle_filled(centre, 5.0 * dot, beneath);
    painter.circle_filled(centre, 3.5 * dot, p.attention);
}

/// The toolbar's control for the panel. It stays pressed while the panel is
/// open and carries a dot while an agent out of view waits for input.
pub fn toggle(ui: &mut Ui, p: Palette, open: bool, waiting: bool, hint: &str) -> bool {
    let surface = ui.painter().add(egui::Shape::Noop);
    let response = icons::button_with_hint(ui, Icon::PanelRight, "Toggle right panel", hint);
    if open {
        ui.painter().set(
            surface,
            egui::Shape::rect_filled(response.rect.shrink(1.0), 7, p.pressed),
        );
    }
    attention_dot(ui, p, &response, waiting, open);
    response.clicked()
}

pub struct View<'a> {
    pub files: &'a explorer::View<'a>,
    pub agents: &'a agents::View<'a>,
    /// Agents waiting for input, whichever tab is in view.
    pub waiting: usize,
    /// The window, which clips the panel while it slides.
    pub window: Rect,
    /// How far the panel has slid in, 0 to 1.
    pub reveal: f32,
}

/// One tab of the strip. Returns whether it was chosen.
fn tab(ui: &mut Ui, rect: Rect, p: Palette, tab: Tab, selected: bool, waiting: usize) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(("panel-tab", tab.label())),
        Sense::click(),
    );
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, tab.label())
    });
    let fill = if selected {
        0.08
    } else if response.hovered() {
        0.045
    } else {
        0.0
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if fill > 0.0 {
        painter.rect_filled(rect, 7, theme::tint(p.fg, fill));
    }
    // Agents waiting for input are counted on their tab.
    let badge = (waiting > 0)
        .then(|| painter.layout_no_wrap(waiting.to_string(), theme::medium(10.5), p.attention));
    let badge_width = badge.as_ref().map_or(0.0, |badge| badge.size().x + 12.0);
    let title = elided(
        &painter,
        tab.label(),
        theme::medium(12.0),
        if selected { p.fg } else { p.secondary },
        (rect.width() - 16.0 - badge_width).max(0.0),
    );
    let width = title.size().x + badge_width;
    let named = galley_at(
        &painter,
        Pos2::new(rect.center().x - width * 0.5, rect.center().y),
        title,
    );
    if let Some(badge) = badge {
        let pill = Rect::from_min_max(
            Pos2::new(named.right() + 5.0, rect.center().y - 7.5),
            Pos2::new(
                named.right() + 5.0 + badge.size().x + 8.0,
                rect.center().y + 7.5,
            ),
        );
        painter.rect_filled(pill, 7.5, theme::tint(p.attention, 0.18));
        painter.galley(pill.center() - badge.size() * 0.5, badge, p.attention);
    }
    response.clicked()
}

/// The panel in `rect`, which slides past the window's trailing edge while a
/// toggle is under way.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &View,
    state: &mut State,
    files: &mut explorer::State,
    actions: &mut Vec<Action>,
) {
    let strip = Rect::from_min_max(
        Pos2::new(rect.left() + 6.0, rect.top()),
        Pos2::new(rect.right() - 8.0, rect.top() + TABS),
    );
    let mut child = ui.new_child(UiBuilder::new().id_salt("right-panel").max_rect(rect));
    child.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(view.window));
    child.multiply_opacity(view.reveal);
    let tabs = [Tab::Files, Tab::Agents];
    let gap = 4.0;
    let each = (strip.width() - gap * (tabs.len() - 1) as f32) / tabs.len() as f32;
    for (index, item) in tabs.into_iter().enumerate() {
        let left = strip.left() + index as f32 * (each + gap);
        let cell = Rect::from_min_max(
            Pos2::new(left, strip.top() + 3.0),
            Pos2::new(left + each, strip.bottom() - 3.0),
        );
        let waiting = if item == Tab::Agents { view.waiting } else { 0 };
        if tab(&mut child, cell, p, item, state.tab == item, waiting) && state.tab != item {
            actions.push(Action::Panel(Event::Show(item)));
        }
    }
    let body = Rect::from_min_max(Pos2::new(rect.left(), strip.bottom()), rect.max);
    match state.tab {
        Tab::Files => explorer::show(ui, body, p, view.files, files, actions),
        Tab::Agents => agents::show(ui, body, p, view.agents, actions),
    }
    // The leading edge resizes the panel once it rests there.
    if view.reveal >= 1.0 {
        let handle = Rect::from_min_max(
            Pos2::new(rect.left() - 3.0, rect.top() + 4.0),
            Pos2::new(rect.left() + 3.0, rect.bottom() - 12.0),
        );
        let resize = explorer::divider(&mut child, p, handle, "panel-resize", "Resize right panel");
        if resize.dragged()
            && let Some(pointer) = resize.interact_pointer_pos()
        {
            state.width = (view.window.right() - pointer.x).clamp(*WIDTH.start(), *WIDTH.end());
        }
        if resize.double_clicked() {
            state.width = DEFAULT_WIDTH;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_activity::{Activity, Attention};
    use eframe::egui::{Event as Input, PointerButton, RawInput};
    use neptune_model::{AgentKind, PaneId};
    use std::time::Duration;

    const WINDOW: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(900.0, 640.0));

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        theme::apply(&ctx, &crate::config::Config::default());
        ctx
    }

    fn panel(width: f32) -> Rect {
        Rect::from_min_max(Pos2::new(WINDOW.right() - width, 44.0), WINDOW.max)
    }

    fn rows() -> Vec<agents::Row> {
        [
            (1, Activity::Working),
            (2, Activity::NeedsInput(Attention::Question)),
            (3, Activity::Idle),
            (4, Activity::NeedsInput(Attention::Permission)),
        ]
        .into_iter()
        .map(|(pane, activity)| agents::Row {
            pane: PaneId::new(pane),
            generation: 1,
            kind: AgentKind::Claude,
            activity,
            elapsed: Duration::from_secs(90),
            title: format!("Task {pane}"),
            workspace: "neptune".into(),
            folder: "~/neptune".into(),
            focused: pane == 1,
        })
        .collect()
    }

    fn run(
        ctx: &egui::Context,
        events: Vec<Input>,
        rows: &[agents::Row],
        state: &mut State,
    ) -> (Vec<Action>, egui::FullOutput) {
        let mut actions = Vec::new();
        let mut files = explorer::State::default();
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(WINDOW),
                events,
                ..Default::default()
            },
            |ui| {
                let p = Palette::for_config(&crate::config::Config::default());
                let files_view = explorer::View {
                    root: None,
                    notice: "Open a workspace to browse its folder.",
                    rows: &[],
                    search: None,
                    preview: None,
                    reveal: 1.0,
                    window: WINDOW,
                };
                let view = View {
                    files: &files_view,
                    agents: &agents::View {
                        rows,
                        window: WINDOW,
                        reveal: 1.0,
                    },
                    waiting: rows
                        .iter()
                        .filter(|row| matches!(row.activity, Activity::NeedsInput(_)))
                        .count(),
                    window: WINDOW,
                    reveal: 1.0,
                };
                show(
                    ui,
                    panel(state.width),
                    p,
                    &view,
                    state,
                    &mut files,
                    &mut actions,
                );
            },
        );
        output.textures_delta.clear();
        (actions, output)
    }

    fn click(pos: Pos2) -> [Vec<Input>; 3] {
        let button = |pressed| Input::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        [
            vec![Input::PointerMoved(pos)],
            vec![button(true)],
            vec![button(false)],
        ]
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

    #[test]
    fn a_tab_is_chosen_by_clicking_it_and_the_chosen_one_is_not_chosen_again() {
        let ctx = context();
        let mut state = State {
            open: true,
            ..State::default()
        };
        let rows = rows();
        let strip = panel(state.width);
        let agents_tab = Pos2::new(strip.right() - 60.0, strip.top() + TABS * 0.5);
        let files_tab = Pos2::new(strip.left() + 60.0, strip.top() + TABS * 0.5);
        let mut actions = Vec::new();
        for events in click(agents_tab) {
            actions.extend(run(&ctx, events, &rows, &mut state).0);
        }
        assert!(matches!(
            actions.as_slice(),
            [Action::Panel(Event::Show(Tab::Agents))]
        ));
        // The files tab is the one in view: clicking it asks for nothing.
        let mut actions = Vec::new();
        for events in click(files_tab) {
            actions.extend(run(&ctx, events, &rows, &mut state).0);
        }
        assert!(actions.is_empty());
    }

    #[test]
    fn the_agents_tab_counts_agents_waiting_for_input_and_lists_every_state() {
        let ctx = context();
        let mut state = State {
            open: true,
            tab: Tab::Agents,
            ..State::default()
        };
        let rows = rows();
        let (_, output) = run(&ctx, Vec::new(), &rows, &mut state);
        let texts = texts(&output);
        for expected in [
            "Files",
            "Agents",
            "Needs input",
            "Asked a question",
            "Needs permission",
            "Working",
            "Idle",
            "Task 1",
            "Task 2",
            "Task 3",
        ] {
            assert!(
                texts.iter().any(|text| text == expected),
                "{expected} in {texts:?}"
            );
        }
        // Two wait: the tab's pill and their heading both say so, and the
        // pill stays while the other tab is in view.
        let count = |texts: &[String]| texts.iter().filter(|text| *text == "2").count();
        assert_eq!(count(&texts), 2, "{texts:?}");
        state.tab = Tab::Files;
        let (_, output) = run(&ctx, Vec::new(), &rows, &mut state);
        assert_eq!(count(&self::texts(&output)), 1);
        let (_, output) = run(&ctx, Vec::new(), &rows[..1], &mut state);
        assert_eq!(count(&self::texts(&output)), 0);
        state.tab = Tab::Agents;
        // Waiting agents come first, whatever order they were found in.
        let position = |wanted: &str| texts.iter().position(|text| text == wanted).unwrap();
        assert!(position("Task 2") < position("Task 1"));
        assert!(position("Task 1") < position("Task 3"));
        let (_, output) = run(&ctx, Vec::new(), &[], &mut state);
        assert!(
            texts_contain(&output, "No agents are running"),
            "an empty list says so"
        );
    }

    fn texts_contain(output: &egui::FullOutput, part: &str) -> bool {
        texts(output).iter().any(|text| text.contains(part))
    }

    #[test]
    fn a_row_reveals_the_terminal_its_agent_runs_in() {
        let ctx = context();
        let mut state = State {
            open: true,
            tab: Tab::Agents,
            ..State::default()
        };
        let rows = rows();
        let body = panel(state.width);
        // The first row sits under the tab strip and its section's heading.
        let first = Pos2::new(
            body.center().x,
            body.top() + TABS + agents::HEADING + agents::ROW * 0.5,
        );
        let mut actions = Vec::new();
        for events in click(first) {
            actions.extend(run(&ctx, events, &rows, &mut state).0);
        }
        assert!(
            matches!(actions.as_slice(), [Action::OpenAgent(pane, 1)] if *pane == PaneId::new(2)),
            "the waiting agent is listed first"
        );
    }

    #[test]
    fn the_edge_resizes_the_panel_within_its_limits() {
        let ctx = context();
        let mut state = State {
            open: true,
            ..State::default()
        };
        let rows = rows();
        let edge = Pos2::new(WINDOW.right() - state.width, 300.0);
        let drag = |to: Pos2| {
            vec![
                vec![Input::PointerMoved(edge)],
                vec![Input::PointerButton {
                    pos: edge,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
                vec![Input::PointerMoved(to)],
                vec![Input::PointerButton {
                    pos: to,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            ]
        };
        for events in drag(Pos2::new(0.0, 300.0)) {
            run(&ctx, events, &rows, &mut state);
        }
        assert_eq!(state.width, *WIDTH.end());
    }
}
