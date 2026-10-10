//! The panel at the window's trailing edge. It holds the file explorer, the
//! running agents, git's changes, the workspace's project and a pull request
//! as tabs, and owns what they share: the toolbar control, the slide, the width and the edge
//! that resizes it.
use super::helpers::{animate, elided, galley_at};
use super::{Action, agents, changes, chrome::SidebarSlide, explorer, project, pull_request};
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
/// A tab's icon, where it stands for its name.
const ICON: f32 = 14.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Files,
    Agents,
    Changes,
    Project,
    PullRequest,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Agents => "Agents",
            Self::Changes => "Changes",
            Self::Project => "Project",
            Self::PullRequest => "Pull request",
        }
    }

    /// The icon that stands for it where the strip has no room for names:
    /// the one its command has in the palette.
    fn icon(self) -> Icon {
        match self {
            Self::Files => Icon::Files,
            Self::Agents => Icon::Terminal,
            Self::Changes => Icon::Branch,
            Self::Project => Icon::Agents,
            Self::PullRequest => Icon::PullRequest,
        }
    }

    /// Its name where the strip has no room for it in full.
    fn short(self) -> &'static str {
        match self {
            Self::PullRequest => "PR",
            tab => tab.label(),
        }
    }
}

pub struct State {
    pub open: bool,
    /// A toggle still sliding into place.
    pub slide: Option<SidebarSlide>,
    pub width: f32,
    pub tab: Tab,
    /// The window had room for the panel when it was last drawn. Without
    /// it the project is shown in a sheet.
    pub available: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            slide: None,
            width: DEFAULT_WIDTH,
            tab: Tab::default(),
            available: true,
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
    pub changes: &'a changes::View<'a>,
    pub project: &'a project::View<'a>,
    pub pull_request: &'a pull_request::View<'a>,
    /// Agents waiting for input, whichever tab is in view.
    pub waiting: usize,
    /// What the project of the workspace in view needs the person for.
    pub needs: usize,
    /// The window, which clips the panel while it slides.
    pub window: Rect,
    /// How far the panel has slid in, 0 to 1.
    pub reveal: f32,
}

/// What the tabs keep between frames.
pub struct Contents<'a> {
    pub files: &'a mut explorer::State,
    pub changes: &'a mut changes::State,
    pub project: &'a mut project::State,
    pub pull_request: &'a mut pull_request::State,
}

/// How the names of the tabs are set in the strip.
#[derive(Clone, Copy, PartialEq)]
struct Fit {
    font: f32,
    /// Clear space inside a tab, both sides together.
    padding: f32,
    gap: f32,
    /// A name too long for the strip is cut to its short form.
    short: bool,
    /// Each tab is its icon, named under the pointer.
    icons: bool,
}
impl Fit {
    const ROOMY: Self = Self {
        font: 12.0,
        padding: 16.0,
        gap: 4.0,
        short: false,
        icons: false,
    };
    /// For a panel short of room, where the names are set closer.
    const TIGHT: Self = Self {
        font: 11.5,
        padding: 8.0,
        gap: 2.0,
        short: false,
        icons: false,
    };
    /// For a panel near its narrowest, where five names share the strip.
    const SHORT: Self = Self {
        font: 11.5,
        padding: 6.0,
        gap: 2.0,
        short: true,
        icons: false,
    };
    /// For a strip that open pull requests leave no room for names in.
    const ICONS: Self = Self {
        font: 11.5,
        padding: 6.0,
        gap: 2.0,
        short: true,
        icons: true,
    };

    fn name(self, tab: Tab) -> &'static str {
        if self.short { tab.short() } else { tab.label() }
    }
}

/// One tab of the strip. Returns whether it was chosen.
fn tab(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    tab: Tab,
    selected: bool,
    waiting: usize,
    fit: Fit,
) -> bool {
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
    let ink = if selected { p.fg } else { p.secondary };
    let named = if fit.icons {
        let width = ICON + badge_width;
        let glyph = Rect::from_min_size(
            Pos2::new(rect.center().x - width * 0.5, rect.center().y - ICON * 0.5),
            vec2(ICON, ICON),
        );
        icons::paint(&painter, glyph, tab.icon(), ink);
        glyph
    } else {
        let title = elided(
            &painter,
            fit.name(tab),
            theme::medium(fit.font),
            ink,
            (rect.width() - fit.padding - badge_width).max(0.0),
        );
        let width = title.size().x + badge_width;
        galley_at(
            &painter,
            Pos2::new(rect.center().x - width * 0.5, rect.center().y),
            title,
        )
    };
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
    let chosen = response.clicked();
    if fit.icons {
        response.on_hover_text(tab.label());
    }
    chosen
}

/// The panel in `rect`, which slides past the window's trailing edge while a
/// toggle is under way.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &View,
    state: &mut State,
    contents: Contents,
    actions: &mut Vec<Action>,
) {
    let strip = Rect::from_min_max(
        Pos2::new(rect.left() + 6.0, rect.top()),
        Pos2::new(rect.right() - 8.0, rect.top() + TABS),
    );
    let mut child = ui.new_child(UiBuilder::new().id_salt("right-panel").max_rect(rect));
    child.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(view.window));
    child.multiply_opacity(view.reveal);
    // A pull request that is open is a tab of its own, with its number;
    // with none open one tab stands for them.
    let mut tabs = vec![Tab::Files, Tab::Agents, Tab::Changes, Tab::Project];
    let opened = pull_request::tabs_width(&child, view.pull_request);
    if opened == 0.0 {
        tabs.push(Tab::PullRequest);
    }
    // The numbers take what the names leave them, set close, and scroll
    // past that; one of them always has room.
    let strip = {
        let names: f32 = tabs
            .iter()
            .map(|item| {
                child
                    .painter()
                    .layout_no_wrap(
                        item.label().to_owned(),
                        theme::medium(Fit::TIGHT.font),
                        p.fg,
                    )
                    .size()
                    .x
                    + Fit::TIGHT.padding
                    + Fit::TIGHT.gap
            })
            .sum();
        let numbers = opened
            .min((strip.width() - names - 2.0).max(72.0))
            .min((strip.width() * 0.5).floor());
        if numbers > 0.0 {
            let zone = Rect::from_min_max(
                Pos2::new(strip.right() - numbers, strip.top() + 3.0),
                Pos2::new(strip.right(), strip.bottom() - 3.0),
            );
            let in_view = state.tab == Tab::PullRequest;
            pull_request::tabs(&mut child, zone, p, view.pull_request, in_view, actions);
            Rect::from_min_max(strip.min, Pos2::new(zone.left() - 2.0, strip.bottom()))
        } else {
            strip
        }
    };
    let waiting = |item| match item {
        Tab::Agents => view.waiting,
        Tab::Project => view.needs,
        _ => 0,
    };
    // Tabs are of equal width while every name fits one. In a panel too
    // narrow for that, each takes what its name needs and a share of the
    // rest; in one too narrow for that as well, names are set closer, and
    // then the longest is cut to its short form.
    // Beside open pull requests, names that still do not fit give way to
    // icons rather than be cut.
    let last = if opened > 0.0 { Fit::ICONS } else { Fit::SHORT };
    let mut fits = [Fit::ROOMY, Fit::TIGHT, Fit::SHORT, last].into_iter();
    let mut fit = Fit::ROOMY;
    let (needed, room) = loop {
        let room = strip.width() - fit.gap * (tabs.len() - 1) as f32;
        let needed: Vec<f32> = tabs
            .iter()
            .map(|item| {
                let width = |text: String, size| {
                    child
                        .painter()
                        .layout_no_wrap(text, theme::medium(size), p.fg)
                        .size()
                        .x
                };
                let badge = match waiting(*item) {
                    0 => 0.0,
                    count => width(count.to_string(), 10.5) + 12.0,
                };
                if fit.icons {
                    ICON + badge + fit.padding * 2.0
                } else {
                    width(fit.name(*item).to_owned(), fit.font) + badge + fit.padding
                }
            })
            .collect();
        if needed.iter().sum::<f32>() <= room {
            break (needed, room);
        }
        match fits.find(|next| *next != fit) {
            Some(next) => fit = next,
            None => break (needed, room),
        }
    };
    let each = room / tabs.len() as f32;
    let total = needed.iter().sum::<f32>();
    let spare = (room - total) / tabs.len() as f32;
    let fitted = needed.iter().all(|needed| *needed <= each);
    let mut left = strip.left();
    for (item, needed) in tabs.into_iter().zip(needed) {
        let width = if fitted {
            each
        } else if spare >= 0.0 {
            needed + spare
        } else {
            // Past every allowance each name gives up its share.
            needed * room / total
        };
        let cell = Rect::from_min_max(
            Pos2::new(left, strip.top() + 3.0),
            Pos2::new(left + width, strip.bottom() - 3.0),
        );
        left += width + fit.gap;
        let chosen = tab(
            &mut child,
            cell,
            p,
            item,
            state.tab == item,
            waiting(item),
            fit,
        );
        if chosen && state.tab != item {
            actions.push(Action::Panel(Event::Show(item)));
        }
    }
    let body = Rect::from_min_max(Pos2::new(rect.left(), strip.bottom()), rect.max);
    match state.tab {
        Tab::Files => explorer::show(ui, body, p, view.files, contents.files, actions),
        Tab::Agents => agents::show(ui, body, p, view.agents, actions),
        Tab::Changes => changes::show(ui, body, p, view.changes, contents.changes, actions),
        Tab::Project => project::show(ui, body, p, view.project, contents.project, actions),
        Tab::PullRequest => pull_request::show(
            ui,
            body,
            p,
            view.pull_request,
            contents.pull_request,
            actions,
        ),
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
            can_background: false,
        })
        .collect()
    }

    fn run(
        ctx: &egui::Context,
        events: Vec<Input>,
        rows: &[agents::Row],
        state: &mut State,
    ) -> (Vec<Action>, egui::FullOutput) {
        run_with(ctx, events, rows, state, 0)
    }

    fn run_with(
        ctx: &egui::Context,
        events: Vec<Input>,
        rows: &[agents::Row],
        state: &mut State,
        needs: usize,
    ) -> (Vec<Action>, egui::FullOutput) {
        let mut actions = Vec::new();
        let mut drafted = project::State::default();
        let mut files = explorer::State::default();
        let mut changed = changes::State::default();
        let mut reviewed = pull_request::State::default();
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
                    changes: &changes::View {
                        repository: None,
                        notice: "This folder is not in a Git repository.",
                        reveal: 1.0,
                        window: WINDOW,
                    },
                    project: &project::View {
                        body: project::Body::Nothing,
                        composing: false,
                        window: WINDOW,
                        reveal: 1.0,
                    },
                    pull_request: &pull_request::View {
                        body: pull_request::Body::Linked(&[]),
                        opened: &[],
                        now: 0,
                        composing: false,
                        reveal: 1.0,
                        window: WINDOW,
                    },
                    needs,
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
                    Contents {
                        files: &mut files,
                        changes: &mut changed,
                        project: &mut drafted,
                        pull_request: &mut reviewed,
                    },
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
        // Five tabs share the strip, each as wide as its name needs.
        let places = |output: &egui::FullOutput| -> Vec<Pos2> {
            let mut places: Vec<(usize, Pos2)> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => {
                        let bounds = text.visual_bounding_rect();
                        ["Files", "Agents", "Changes", "Project", "Pull request"]
                            .iter()
                            .position(|name| *name == text.galley.text())
                            .filter(|_| bounds.bottom() <= strip.top() + TABS)
                            .map(|place| (place, bounds.center()))
                    }
                    _ => None,
                })
                .collect();
            places.sort_by_key(|(place, _)| *place);
            places.into_iter().map(|(_, centre)| centre).collect()
        };
        let named = places(&run(&ctx, Vec::new(), &rows, &mut state).1);
        let [
            files_tab,
            agents_tab,
            changes_tab,
            project_tab,
            pull_request_tab,
        ] = named[..]
        else {
            panic!("five tabs are named: {named:?}");
        };
        let mut actions = Vec::new();
        for events in click(agents_tab) {
            actions.extend(run(&ctx, events, &rows, &mut state).0);
        }
        assert!(matches!(
            actions.as_slice(),
            [Action::Panel(Event::Show(Tab::Agents))]
        ));
        let mut actions = Vec::new();
        for events in click(changes_tab) {
            actions.extend(run(&ctx, events, &rows, &mut state).0);
        }
        assert!(matches!(
            actions.as_slice(),
            [Action::Panel(Event::Show(Tab::Changes))]
        ));
        let mut actions = Vec::new();
        for events in click(project_tab) {
            actions.extend(run(&ctx, events, &rows, &mut state).0);
        }
        assert!(matches!(
            actions.as_slice(),
            [Action::Panel(Event::Show(Tab::Project))]
        ));
        let mut actions = Vec::new();
        for events in click(pull_request_tab) {
            actions.extend(run(&ctx, events, &rows, &mut state).0);
        }
        assert!(matches!(
            actions.as_slice(),
            [Action::Panel(Event::Show(Tab::PullRequest))]
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
            "Changes",
            "Project",
            "Pull request",
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

    #[test]
    fn the_narrowest_panel_names_every_tab_beside_a_count() {
        let ctx = context();
        let mut state = State {
            open: true,
            width: *WIDTH.start(),
            ..State::default()
        };
        let (_, output) = run(&ctx, Vec::new(), &rows(), &mut state);
        let texts = texts(&output);
        // The longest name is cut to its short form there; none is elided.
        for expected in ["Files", "Agents", "2", "Changes", "Project", "PR"] {
            assert!(
                texts.iter().any(|text| text == expected),
                "{expected} in {texts:?}"
            );
        }
        // With nobody waiting, and with the project's count beside the
        // agents', five names still share the narrowest strip.
        let (_, output) = run(&ctx, Vec::new(), &rows()[..1], &mut state);
        let texts = self::texts(&output);
        for expected in ["Files", "Agents", "Changes", "Project", "PR"] {
            assert!(
                texts.iter().any(|text| text == expected),
                "{expected} in {texts:?}"
            );
        }
        let (_, output) = run_with(&ctx, Vec::new(), &rows(), &mut state, 1);
        let texts = self::texts(&output);
        for expected in ["Files", "Agents", "2", "Changes", "Project", "1", "PR"] {
            assert!(
                texts.iter().any(|text| text == expected),
                "{expected} in {texts:?}"
            );
        }
        // Each tab keeps its own place: none is drawn over another.
        let labels: Vec<Rect> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if ["Files", "Agents", "Changes", "Project", "PR"]
                        .contains(&text.galley.text()) =>
                {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            // The explorer's own heading is below the strip.
            .filter(|label| label.bottom() <= panel(state.width).top() + TABS)
            .collect();
        assert_eq!(labels.len(), 5);
        for pair in labels.windows(2) {
            assert!(pair[0].right() < pair[1].left(), "{labels:?}");
        }
        assert!(labels[4].right() <= panel(state.width).right());
    }

    #[test]
    fn open_pull_requests_are_tabs_of_their_own_that_show_and_close_them() {
        let ctx = context();
        let mut state = State {
            open: true,
            tab: Tab::PullRequest,
            ..State::default()
        };
        let link = |number: u64| {
            neptune_model::PullRequest::parse(&format!(
                "https://github.com/zevem/neptune/pull/{number}"
            ))
            .unwrap()
        };
        let opened = [link(118), link(112)];
        let frame = |state: &mut State, events: Vec<Input>| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(WINDOW),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let p = Palette::for_config(&crate::config::Config::default());
                    let files = explorer::View {
                        root: None,
                        notice: "",
                        rows: &[],
                        search: None,
                        preview: None,
                        reveal: 1.0,
                        window: WINDOW,
                    };
                    let view = View {
                        files: &files,
                        agents: &agents::View {
                            rows: &[],
                            window: WINDOW,
                            reveal: 1.0,
                        },
                        changes: &changes::View {
                            repository: None,
                            notice: "",
                            reveal: 1.0,
                            window: WINDOW,
                        },
                        project: &project::View {
                            body: project::Body::Nothing,
                            composing: false,
                            window: WINDOW,
                            reveal: 1.0,
                        },
                        pull_request: &pull_request::View {
                            body: pull_request::Body::Reading(&opened[0]),
                            opened: &opened,
                            now: 0,
                            composing: false,
                            reveal: 1.0,
                            window: WINDOW,
                        },
                        needs: 0,
                        waiting: 0,
                        window: WINDOW,
                        reveal: 1.0,
                    };
                    show(
                        ui,
                        panel(state.width),
                        p,
                        &view,
                        state,
                        Contents {
                            files: &mut explorer::State::default(),
                            changes: &mut changes::State::default(),
                            project: &mut project::State::default(),
                            pull_request: &mut pull_request::State::default(),
                        },
                        &mut actions,
                    );
                },
            );
            output.textures_delta.clear();
            (actions, output)
        };
        // Each is named by its number, where one tab stood for them.
        let (_, output) = frame(&mut state, Vec::new());
        let named = texts(&output);
        for expected in ["Files", "Project", "#118", "#112"] {
            assert!(
                named.iter().any(|text| text == expected),
                "{expected} in {named:?}"
            );
        }
        assert!(
            !named
                .iter()
                .any(|text| text == "Pull request" || text == "PR")
        );
        let at = |wanted: &str| -> Rect {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == wanted => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap()
        };
        // The other one's number shows it; its cross closes it.
        let other = at("#112");
        let mut actions = Vec::new();
        for events in click(other.center()) {
            actions.extend(frame(&mut state, events).0);
        }
        assert!(matches!(
            actions.as_slice(),
            [Action::PullRequest(pull_request::Event::Open(link))] if link.number() == 112
        ));
        let mut actions = Vec::new();
        for events in click(Pos2::new(other.right() + 11.0, other.center().y)) {
            actions.extend(frame(&mut state, events).0);
        }
        assert!(matches!(
            actions.as_slice(),
            [Action::PullRequest(pull_request::Event::Close(link))] if link.number() == 112
        ));
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
