//! The tab drawn headlessly: frames are rendered and pressed where their
//! words are, and what was painted is read back.
use super::*;
use crate::projects::transcript::{Entry, Origin, Source, Transcript};
use crate::runtime::pull_requests::{self, Checks, Lookup, Status};
use eframe::egui::{Event as Input, Key, Modifiers, PointerButton, RawInput};

/// What the settings offer in these tests: two models and three
/// efforts a CLI, with Codex's Ultra for agents only.
fn catalog() -> &'static Catalog {
    static CATALOG: std::sync::LazyLock<Catalog> = std::sync::LazyLock::new(|| {
        let choice = |value: &'static str, label: &str| Choice {
            value,
            label: label.into(),
        };
        let of = |kind, ultra: bool| Choices {
            kind,
            models: match kind {
                AgentKind::Claude => vec![choice("opus", "Opus"), choice("sonnet", "Sonnet")],
                _ => vec![choice("gpt-6-sol", "gpt-6-sol")],
            },
            efforts: [choice("low", "Low"), choice("high", "High")]
                .into_iter()
                .chain(ultra.then(|| choice("ultra", "Ultra")))
                .collect(),
        };
        Catalog {
            lead: vec![of(AgentKind::Claude, false), of(AgentKind::Codex, false)],
            agents: vec![of(AgentKind::Claude, false), of(AgentKind::Codex, true)],
        }
    });
    &CATALOG
}
/// A project nothing was chosen for.
fn settings() -> &'static Settings {
    static SETTINGS: std::sync::LazyLock<Settings> = std::sync::LazyLock::new(Settings::default);
    &SETTINGS
}

const WINDOW: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(900.0, 860.0));

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::platform::fonts::bundled_definitions());
    theme::apply(&ctx, &crate::config::Config::default());
    ctx
}
fn panel(width: f32) -> Rect {
    Rect::from_min_max(Pos2::new(WINDOW.right() - width, 78.0), WINDOW.max)
}
/// Every text that can be read, with where it is: what a scrolled list
/// clips away is not counted.
fn texts(output: &egui::FullOutput) -> Vec<(String, Rect)> {
    fn collect(shape: &egui::Shape, clip: Rect, texts: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                let rect = text.visual_bounding_rect();
                if !text.galley.text().is_empty() && clip.contains(rect.center()) {
                    texts.push((text.galley.text().to_owned(), rect));
                }
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, clip, texts)),
            _ => {}
        }
    }
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, clipped.clip_rect, &mut texts);
    }
    texts
}
fn frame(
    ctx: &egui::Context,
    body: Body,
    state: &mut State,
    width: f32,
    events: Vec<Input>,
) -> (Vec<Action>, Vec<(String, Rect)>) {
    let (actions, output) = frame_in(ctx, body, state, panel(width), events);
    (actions, texts(&output))
}
/// A frame of the tab in `rect`, with all that was painted.
fn frame_in(
    ctx: &egui::Context,
    body: Body,
    state: &mut State,
    rect: Rect,
    events: Vec<Input>,
) -> (Vec<Action>, egui::FullOutput) {
    let mut actions = Vec::new();
    let view = View {
        body,
        composing: false,
        window: WINDOW,
        reveal: 1.0,
    };
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(WINDOW),
            events,
            ..Default::default()
        },
        |ui| {
            let p = Palette::for_config(&crate::config::Config::default());
            show(ui, rect, p, &view, state, &mut actions);
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
fn key(key: Key) -> Vec<Input> {
    [true, false]
        .into_iter()
        .map(|pressed| Input::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        })
        .collect()
}
fn has(texts: &[(String, Rect)], wanted: &str) -> bool {
    texts.iter().any(|(text, _)| text.contains(wanted))
}
fn at(texts: &[(String, Rect)], wanted: &str) -> Pos2 {
    texts
        .iter()
        .find(|(text, _)| text == wanted)
        .unwrap_or_else(|| panic!("{wanted} in {texts:?}"))
        .1
        .center()
}
/// Where the nth of the texts that read `wanted` is, counted from zero.
fn nth(texts: &[(String, Rect)], wanted: &str, n: usize) -> Pos2 {
    texts
        .iter()
        .filter(|(text, _)| text == wanted)
        .nth(n)
        .unwrap_or_else(|| panic!("{wanted} #{n} in {texts:?}"))
        .1
        .center()
}
/// The last of them: a row of an open list, which is painted over the tab.
fn last(texts: &[(String, Rect)], wanted: &str) -> Pos2 {
    texts
        .iter()
        .rfind(|(text, _)| text == wanted)
        .unwrap_or_else(|| panic!("{wanted} in {texts:?}"))
        .1
        .center()
}
fn count(texts: &[(String, Rect)], wanted: &str) -> usize {
    texts.iter().filter(|(text, _)| text == wanted).count()
}
fn inside(texts: &[(String, Rect)], bounds: Rect) {
    for (text, rect) in texts {
        assert!(
            rect.left() >= bounds.left() - 1.0 && rect.right() <= bounds.right() + 1.0,
            "{text} at {rect:?} leaves {bounds:?}"
        );
    }
}
fn events(actions: Vec<Action>) -> Vec<Event> {
    actions
        .into_iter()
        .filter_map(|action| match action {
            Action::Project(event) => Some(event),
            _ => None,
        })
        .collect()
}
/// The header's menu in a panel of `width`, and the control beside it that
/// shows the details.
fn menu_button(width: f32) -> Pos2 {
    let bounds = panel(width);
    Pos2::new(bounds.right() - 8.0 - 14.0, bounds.top() + HEADER * 0.5)
}
fn details_button(width: f32) -> Pos2 {
    menu_button(width) - vec2(28.0, 0.0)
}
/// The control inside the goal's field, over the line that reads `line`.
fn start_control(texts: &[(String, Rect)], line: &str, width: f32) -> Pos2 {
    Pos2::new(
        panel(width).right() - 8.0 - 4.0 - 17.0,
        nth(texts, line, 0).y - 31.0,
    )
}
/// The fill painted last under `pos`.
fn fill_at(output: &egui::FullOutput, pos: Pos2) -> egui::Color32 {
    fn collect(shape: &egui::Shape, pos: Pos2, fills: &mut Vec<egui::Color32>) {
        match shape {
            egui::Shape::Rect(rect)
                if rect.rect.contains(pos) && rect.fill != egui::Color32::TRANSPARENT =>
            {
                fills.push(rect.fill)
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, pos, fills)),
            _ => {}
        }
    }
    let mut fills = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, pos, &mut fills);
    }
    *fills.last().expect("a fill")
}

const BOTH: &[AgentKind] = &[AgentKind::Claude, AgentKind::Codex];
fn empty<'a>(leads: Option<&'a [AgentKind]>, starting: bool, reopen: Option<&'a str>) -> Body<'a> {
    Body::Empty {
        workspace: WorkspaceId::new(3),
        directory: "~/code/shop",
        starting,
        reopen,
        leads,
        catalog: catalog(),
    }
}

#[test]
fn the_lead_is_named_under_the_goal_and_picked_among_the_clis_installed_here() {
    // A form with a goal typed into it, and what Enter and its control do.
    fn asked(
        leads: Option<&[AgentKind]>,
        state: &mut State,
        width: f32,
        line: &str,
    ) -> (Vec<AgentKind>, Vec<(String, Rect)>) {
        let ctx = context();
        state.focus = true;
        frame(&ctx, empty(leads, false, None), state, width, Vec::new());
        frame(
            &ctx,
            empty(leads, false, None),
            state,
            width,
            vec![Input::Text("Split checkout".into())],
        );
        let (mut actions, texts) = frame(
            &ctx,
            empty(leads, false, None),
            state,
            width,
            key(Key::Enter),
        );
        for events in click(start_control(&texts, line, width)) {
            actions.extend(frame(&ctx, empty(leads, false, None), state, width, events).0);
        }
        let leads: Vec<AgentKind> = actions
            .iter()
            .filter_map(|action| match action {
                Action::Project(Event::Create { lead, .. }) => Some(*lead),
                _ => None,
            })
            .collect();
        (leads, texts)
    }
    // While they are looked for, and where none is installed, nothing
    // is started: a project whose lead cannot run helps nobody. What was
    // typed stays in the field.
    let mut state = State::default();
    let looking = "Looking for Claude Code and Codex…";
    let (started, texts) = asked(None, &mut state, 300.0, looking);
    assert!(started.is_empty() && has(&texts, "A lead plans the work"));
    assert_eq!(state.goal, "Split checkout");
    let (started, texts) = asked(Some(&[]), &mut State::default(), 300.0, "No lead installed");
    assert!(started.is_empty());
    assert!(has(&texts, "found neither Claude Code nor Codex"));
    assert!(!has(&texts, "A lead plans the work"));
    // One installed: it leads, and its name is all the line says.
    let (started, texts) = asked(
        Some(&[AgentKind::Codex]),
        &mut State::default(),
        300.0,
        "Codex",
    );
    assert_eq!(started, [AgentKind::Codex; 2]);
    assert!(!has(&texts, "Claude Code"), "{texts:?}");
    // Both: the first is named, and the other is behind that line, at
    // the narrowest the panel gets as well.
    for width in [300.0, 220.0] {
        let mut state = State::default();
        let (started, texts) = asked(Some(BOTH), &mut state, width, "Claude Code");
        assert_eq!(started, [AgentKind::Claude; 2]);
        assert_eq!(count(&texts, "Codex"), 0, "folded until asked for");
        let ctx = context();
        let both = || empty(Some(BOTH), false, None);
        for events in click(at(&texts, "Claude Code")) {
            frame(&ctx, both(), &mut state, width, events);
        }
        assert!(state.lead_options);
        let (_, texts) = frame(&ctx, both(), &mut state, width, Vec::new());
        inside(&texts, panel(width));
        for wanted in ["Lead", "Model", "Effort", "Codex"] {
            assert!(count(&texts, wanted) == 1, "{wanted} in {texts:?}");
        }
        for events in click(at(&texts, "Codex")) {
            frame(&ctx, both(), &mut state, width, events);
        }
        assert_eq!(state.lead, Some(AgentKind::Codex));
        state.goal.clear();
        state.lead_options = false;
        let (started, _) = asked(Some(BOTH), &mut state, width, "Codex");
        assert_eq!(started, [AgentKind::Codex; 2]);
        // A pick whose CLI is gone does not stand.
        state.goal.clear();
        let (started, _) = asked(Some(&[AgentKind::Claude]), &mut state, width, "Claude Code");
        assert_eq!(started, [AgentKind::Claude; 2]);
    }
}

#[test]
fn a_workspace_without_a_project_starts_one_from_a_goal_typed_into_one_field() {
    let ctx = context();
    let mut state = State::default();
    let workspace = WorkspaceId::new(3);
    let form = |starting| empty(Some(&[AgentKind::Claude]), starting, None);
    let (actions, texts) = frame(&ctx, form(false), &mut state, 300.0, Vec::new());
    assert!(actions.is_empty());
    let position = |wanted: &str| {
        texts
            .iter()
            .find(|(text, _)| text.contains(wanted))
            .unwrap_or_else(|| panic!("{wanted} in {texts:?}"))
            .1
            .top()
    };
    // The field comes before anything is explained.
    let order = [
        "New project",
        "~/code/shop",
        "What do you want to get done?",
        "Claude Code",
        "A lead plans the work",
        "The chat and notes stay on this computer.",
    ];
    for pair in order.windows(2) {
        assert!(position(pair[0]) <= position(pair[1]), "{pair:?}");
    }
    assert!(has(&texts, "Enter starts"));
    // Without a goal there is nothing to start.
    let start = start_control(&texts, "Enter starts", 300.0);
    for events in click(start) {
        assert!(
            frame(&ctx, form(false), &mut state, 300.0, events)
                .0
                .is_empty()
        );
    }
    // The goal is typed where it is asked for; Enter starts the project.
    state.focus = true;
    frame(&ctx, form(false), &mut state, 300.0, Vec::new());
    assert!(!state.focus);
    frame(
        &ctx,
        form(false),
        &mut state,
        300.0,
        vec![Input::Text("  Split checkout ".into())],
    );
    assert_eq!(state.goal, "  Split checkout ");
    let (actions, _) = frame(&ctx, form(false), &mut state, 300.0, key(Key::Enter));
    assert_eq!(
        events(actions),
        [Event::Create {
            workspace,
            goal: "Split checkout".into(),
            lead: AgentKind::Claude,
            model: None,
            effort: None,
        }]
    );
    // The goal stays until the project exists, and the control in the
    // field starts it as well.
    assert_eq!(state.goal, "  Split checkout ");
    let mut pressed = Vec::new();
    for events in click(start) {
        pressed.extend(frame(&ctx, form(false), &mut state, 300.0, events).0);
    }
    assert!(matches!(
        pressed.as_slice(),
        [Action::Project(Event::Create { .. })]
    ));
    // While its folder is made it cannot be started twice.
    let (_, texts) = frame(&ctx, form(true), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Starting…") && !has(&texts, "Enter starts"));
    let mut pressed = Vec::new();
    for events in click(start).into_iter().chain([key(Key::Enter)]) {
        pressed.extend(frame(&ctx, form(true), &mut state, 300.0, events).0);
    }
    assert!(pressed.is_empty());

    // Where a project cannot be, the tab says why, and nothing waits
    // to take the keyboard later.
    state.focus = true;
    let (_, texts) = frame(&ctx, Body::Remote, &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Projects run on this computer."));
    assert!(!state.focus);
    let (_, texts) = frame(&ctx, Body::Nothing, &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Open a workspace to start a project."));
    // The narrowest panel holds the whole form.
    let (_, texts) = frame(&ctx, form(false), &mut state, 220.0, Vec::new());
    inside(&texts, panel(220.0));
    assert!(has(&texts, "New project") && has(&texts, "Claude Code"));
}

fn members() -> Vec<Member> {
    vec![
        Member {
            pane: PaneId::new(12),
            generation: 1,
            kind: AgentKind::Claude,
            title: "auth-refactor".into(),
            state: "Working".into(),
            detail: "Working".into(),
            waits: false,
            works: true,
            ready: false,
            elapsed: Some(Duration::from_secs(14 * 60)),
            branch: Some("feat/auth".into()),
            pull_requests: vec![LinkedPullRequest {
                link: neptune_model::PullRequest::parse(
                    "https://github.com/zevem/neptune/pull/214",
                )
                .unwrap(),
                lookup: Lookup::Known(Status {
                    state: pull_requests::State::Open,
                    checks: Checks::Failing,
                    unresolved: 2,
                }),
            }],
            can_background: true,
        },
        Member {
            pane: PaneId::new(13),
            generation: 4,
            kind: AgentKind::Codex,
            title: "api-tests".into(),
            state: "Needs you".into(),
            detail: "Needs permission".into(),
            waits: true,
            works: false,
            ready: false,
            elapsed: Some(Duration::from_secs(30)),
            branch: None,
            pull_requests: Vec::new(),
            can_background: false,
        },
    ]
}
fn needs() -> Vec<Need> {
    vec![
        Need {
            title: "13 Codex · api-tests".into(),
            detail: "Needs permission. Only you can answer, in its terminal.".into(),
            remedy: Remedy::Open(PaneId::new(13), 4),
        },
        Need {
            title: "The lead needs you".into(),
            detail: "Claude Code isn't signed in. Run `claude` in a terminal and sign in, \
                     then try again."
                .into(),
            remedy: Remedy::Retry,
        },
    ]
}
fn chat() -> Transcript {
    let mut chat = Transcript::default();
    for entry in [
        Entry::User {
            text: "Split checkout into auth and API tests".into(),
            attachments: Vec::new(),
        },
        Entry::Turn {
            id: "turn-1".into(),
            origin: Origin::User,
        },
        Entry::Lead {
            text: "Two agents: `auth-refactor` and `api-tests`.".into(),
        },
        Entry::Tool {
            name: "spawn_agent".into(),
            summary: "Started agent 12 · Claude Code · auth-refactor".into(),
            ok: true,
        },
        Entry::Event {
            source: Source::Agent,
            agent: Some(12),
            what: "“auth-refactor” finished its turn".into(),
            text: "All tests pass.".into(),
        },
    ] {
        chat.push(entry, 0);
    }
    chat
}
const ID: ProjectId = ProjectId::new(2);
const NOTHING: Context = Context {
    files: &[],
    instructions: "",
    read: true,
};
fn running<'a>(
    chat: &'a Transcript,
    members: &'a [Member],
    needs: &'a [Need],
    open: &'a [(PaneId, u64)],
    busy: bool,
    queued: usize,
) -> Body<'a> {
    Body::Project(Box::new(Project {
        id: ID,
        name: "shop",
        directory: "~/code/shop",
        directory_path: std::path::Path::new("/home/me/code/shop"),
        lead: AgentKind::Claude,
        lead_model: None,
        settings: settings(),
        pending: false,
        catalog: catalog(),
        leads: Some(BOTH),
        paused: false,
        needs,
        members,
        chat: Chat {
            project: ID,
            proposed: &[],
            pulls: &[],
            records: chat.records(),
            shown: chat::PAGE,
            members: open,
            follow: &[],
            thinking: false,
            pending: &[],
            streaming: None,
            status: None,
            more: false,
        },
        busy,
        elapsed: None,
        usage: None,
        queued,
        locked: false,
        context: NOTHING,
        watches: &[],
    }))
}
/// The same with something of it changed.
fn changed<'a>(body: Body<'a>, change: impl FnOnce(&mut Project<'a>)) -> Body<'a> {
    let Body::Project(mut project) = body else {
        unreachable!()
    };
    change(&mut project);
    Body::Project(project)
}

#[test]
fn a_project_is_its_chat_under_a_strip_of_its_agents_with_what_needs_the_person_pinned() {
    let ctx = context();
    let mut state = State::default();
    let (chat, members, needs) = (chat(), members(), needs());
    let open = [(PaneId::new(12), 1), (PaneId::new(13), 4)];
    let body = || running(&chat, &members, &needs, &open, false, 0);
    let (actions, texts) = frame(&ctx, body(), &mut state, 300.0, Vec::new());
    assert!(actions.is_empty());
    let position = |texts: &[(String, Rect)], wanted: &str| {
        texts
            .iter()
            .find(|(text, _)| text.contains(wanted))
            .unwrap_or_else(|| panic!("{wanted} in {texts:?}"))
            .1
            .top()
    };
    // Top to bottom: the project, what needs the person, the strip of its
    // agents, the chat, the composer and who leads.
    let order = [
        "shop",
        "13 Codex · api-tests",
        "Only you can answer",
        "The lead needs you",
        "isn't signed in",
        "Split checkout",
        "Started agent 12 · Claude Code · auth-refactor",
        "finished its turn",
        "Message the lead…",
    ];
    for pair in order.windows(2) {
        assert!(
            position(&texts, pair[0]) <= position(&texts, pair[1]),
            "{pair:?}"
        );
    }
    assert!(at(&texts, "Claude Code").y > at(&texts, "Message the lead…").y);
    // Where it works stands beside what it is called.
    assert!(at(&texts, "~/code/shop").x > at(&texts, "shop").x);
    assert!((at(&texts, "~/code/shop").y - at(&texts, "shop").y).abs() < 2.0);
    // The strip is one row between what waits and the chat: a pill for
    // each agent, the one that waits first, and the control that counts
    // them at its end.
    let (waits, works, all) = (at(&texts, "13"), at(&texts, "12"), at(&texts, "2"));
    assert!((waits.y - works.y).abs() < 1.0 && (works.y - all.y).abs() < 1.0);
    assert!(waits.x < works.x && works.x < all.x);
    assert!(position(&texts, "isn't signed in") < waits.y);
    assert!(waits.y < position(&texts, "Split checkout"));
    assert!(has(&texts, "api-tests") && has(&texts, "auth-refactor"));
    // Nothing else is in view: no heading over what waits or over the
    // agents, no line of counts, no parts to choose among, and the list of
    // all the agents is away until it is asked for.
    for absent in [
        "Needs you",
        "Agents",
        "Context",
        "Watches",
        "Settings",
        "Paused",
        "12 auth-refactor",
    ] {
        assert_eq!(count(&texts, absent), 0, "{absent} in {texts:?}");
    }
    assert!(!has(&texts, "Queued") && !has(&texts, "Enter sends"));

    let press = |state: &mut State, pos: Pos2| {
        let mut actions = Vec::new();
        for events in click(pos) {
            actions.extend(frame(&ctx, body(), state, 300.0, events).0);
        }
        actions
    };
    // A waiting agent's terminal is one click away from its row here; a
    // lead that could not start is tried again.
    let opened = press(&mut state, at(&texts, "Open"));
    assert!(matches!(opened.as_slice(), [Action::OpenAgent(pane, 4)] if *pane == PaneId::new(13)));
    assert_eq!(
        events(press(&mut state, at(&texts, "Try again"))),
        [Event::Retry(ID)]
    );
    // A pill is pressed as a tab is: it shows its agent's terminal.
    let opened = press(&mut state, works);
    assert!(matches!(opened.as_slice(), [Action::OpenAgent(pane, 1)] if *pane == PaneId::new(12)));
    // The control at the strip's end lists them all under it, over a chat
    // that keeps its place: how they stand in words, then a row for each.
    let list = |state: &mut State| {
        assert!(press(state, all).is_empty());
        frame(&ctx, body(), state, 300.0, Vec::new()).1
    };
    let listed = list(&mut state);
    let below = |wanted: &str| at(&listed, wanted).y;
    assert!(works.y < below("1 needs you · 1 working"));
    assert!(below("1 needs you · 1 working") < below("13 api-tests"));
    assert!(below("13 api-tests") < below("12 auth-refactor"));
    assert!(has(&listed, "Working") && has(&listed, "14m"));
    for kept in [
        "13 Codex · api-tests",
        "Split checkout",
        "Message the lead…",
    ] {
        assert_eq!(position(&listed, kept), position(&texts, kept), "{kept}");
    }
    assert_eq!(at(&listed, "13"), waits);
    // A row of it shows the agent's terminal and puts the list away.
    let opened = press(&mut state, at(&listed, "12 auth-refactor"));
    assert!(matches!(opened.as_slice(), [Action::OpenAgent(pane, 1)] if *pane == PaneId::new(12)));
    let shown = |state: &mut State| {
        let (_, texts) = frame(&ctx, body(), state, 300.0, Vec::new());
        has(&texts, "12 auth-refactor")
    };
    assert!(!shown(&mut state));
    // So do Escape, a press outside it and a second press on its control.
    assert!(has(&list(&mut state), "12 auth-refactor"));
    frame(&ctx, body(), &mut state, 300.0, key(Key::Escape));
    assert!(!shown(&mut state));
    assert!(has(&list(&mut state), "12 auth-refactor"));
    press(&mut state, at(&texts, "shop"));
    assert!(!shown(&mut state));
    assert!(has(&list(&mut state), "12 auth-refactor"));
    assert!(!has(&list(&mut state), "12 auth-refactor"));
    let (_, away) = frame(&ctx, body(), &mut state, 300.0, Vec::new());
    assert!(has(&away, "13 Codex · api-tests"), "a need is not put away");

    // The narrowest panel keeps every row inside itself, the list as well.
    state.show_agents = true;
    frame(&ctx, body(), &mut state, 220.0, Vec::new());
    let (_, narrow) = frame(&ctx, body(), &mut state, 220.0, Vec::new());
    assert!(!state.show_agents);
    inside(&narrow, panel(220.0));
    for wanted in ["Open", "Try again", "Message the lead…", "13 api"] {
        assert!(has(&narrow, wanted), "{wanted} in {narrow:?}");
    }
    frame(&ctx, body(), &mut state, 220.0, key(Key::Escape));
    // A project nothing of which waits or works is its name, its chat
    // and its composer.
    let quiet = running(&chat, &[], &[], &[], false, 0);
    let (_, texts) = frame(&ctx, quiet, &mut state, 300.0, Vec::new());
    assert!(!has(&texts, "working") && !has(&texts, "idle"));
    let top = at(&texts, "shop").y;
    assert!(position(&texts, "Split checkout") - top < HEADER + 24.0);
}

#[test]
fn pausing_is_in_the_menu_and_a_paused_project_says_so_with_the_way_back() {
    let ctx = context();
    let mut state = State::default();
    let (chat, members) = (chat(), members());
    let guard = [Need {
        title: "Paused after 20 turns without you".into(),
        detail: "Neptune paused this project so that it does not run on by itself.".into(),
        remedy: Remedy::Resume,
    }];
    let body = |paused: bool, needs: &'static [Need]| {
        changed(running(&chat, &members, needs, &[], false, 0), |project| {
            project.paused = paused
        })
    };
    let press = |state: &mut State, paused: bool, pos: Pos2| {
        let mut actions = Vec::new();
        for input in click(pos) {
            actions.extend(frame(&ctx, body(paused, &[]), state, 300.0, input).0);
        }
        events(actions)
    };
    let menu = |state: &mut State, paused: bool| {
        assert!(press(state, paused, menu_button(300.0)).is_empty());
        frame(&ctx, body(paused, &[]), state, 300.0, Vec::new()).1
    };
    // The header has no control of its own for it.
    let (_, texts) = frame(&ctx, body(false, &[]), &mut state, 300.0, Vec::new());
    assert_eq!(count(&texts, "Paused"), 0);
    let texts = menu(&mut state, false);
    assert_eq!(
        press(&mut state, false, at(&texts, "Pause project")),
        [Event::SetPaused(ID, true)]
    );
    // Paused by the person: a plain row says so over the chat, and resumes.
    let (_, texts) = frame(&ctx, body(true, &[]), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Nothing starts by itself."));
    assert!(at(&texts, "Paused").y < at(&texts, "13").y);
    assert_eq!(
        press(&mut state, true, at(&texts, "Resume")),
        [Event::SetPaused(ID, false)]
    );
    let texts = menu(&mut state, true);
    assert_eq!(
        press(&mut state, true, at(&texts, "Resume project")),
        [Event::SetPaused(ID, false)]
    );
    // One Neptune made is among what needs the person, with its reason:
    // it is not said twice.
    let guard: &'static [Need] = Box::leak(Box::new(guard));
    let (_, texts) = frame(&ctx, body(true, guard), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Paused after 20 turns without you"));
    assert_eq!(count(&texts, "Paused"), 0);
    assert_eq!(count(&texts, "Resume"), 1);
}

#[test]
fn the_list_of_agents_says_each_ones_cli_branch_pull_request_and_one_of_six_states() {
    let ctx = context();
    let mut state = State::default();
    let chat = chat();
    let mut members = members();
    members.push(Member {
        pane: PaneId::new(14),
        generation: 2,
        kind: AgentKind::Claude,
        title: "docs".into(),
        state: "Ready for review".into(),
        detail: "Idle".into(),
        waits: false,
        works: false,
        ready: true,
        elapsed: Some(Duration::from_secs(120)),
        branch: None,
        pull_requests: Vec::new(),
        can_background: false,
    });
    // How they stand is said in words, those that wait first, and counts
    // only agents: what else needs the person is said above the strip.
    assert_eq!(
        summary(&members),
        "1 needs you · 1 working · 1 ready for review"
    );
    // One agent is named.
    assert_eq!(summary(&members[..1]), "12 auth-refactor · Working · 14m");
    let rested = |state: &str, pane| Member {
        pane: PaneId::new(pane),
        generation: 1,
        kind: AgentKind::Claude,
        title: "worker".into(),
        state: state.into(),
        detail: state.into(),
        waits: false,
        works: state == "Starting",
        ready: false,
        elapsed: None,
        branch: None,
        pull_requests: Vec::new(),
        can_background: false,
    };
    assert_eq!(
        summary(&[
            rested("Idle", 1),
            rested("Ended", 2),
            rested("Starting", 3),
            rested("Idle", 4)
        ]),
        "1 starting · 2 idle · 1 ended"
    );
    let body = || running(&chat, &members, &[], &[], false, 0);
    // The list of them all, asked for by name, in a panel of `width`.
    let listed = |state: &mut State, width: f32| {
        state.show_agents = true;
        frame(&ctx, body(), state, width, Vec::new());
        frame(&ctx, body(), state, width, Vec::new()).1
    };
    let texts = listed(&mut state, 300.0);
    // Those words head it.
    assert!(has(&texts, "1 needs you · 1 working · 1 ready for review"));
    for wanted in [
        "12 auth-refactor",
        "Working",
        "Claude Code · feat/auth",
        "214",
        "13 api-tests",
        "Needs you",
        "Codex",
        "14 docs",
        "Ready for review",
    ] {
        assert!(count(&texts, wanted) >= 1, "{wanted} in {texts:?}");
    }
    // Its state stands at the row's trailing edge, over its age.
    let row = |wanted: &str| texts.iter().find(|(text, _)| text == wanted).unwrap().1;
    assert!(row("Working").left() > row("12 auth-refactor").right());
    assert!(row("Claude Code · feat/auth").top() > row("12 auth-refactor").bottom() - 2.0);
    assert!(row("214").left() > row("Claude Code · feat/auth").right());
    let press = |state: &mut State, pos: Pos2| {
        let mut actions = Vec::new();
        for events in click(pos) {
            actions.extend(frame(&ctx, body(), state, 300.0, events).0);
        }
        actions
    };
    // The rows are in the order of the pills: who waits, who works, the rest.
    assert!(row("13 api-tests").top() < row("12 auth-refactor").top());
    assert!(row("12 auth-refactor").top() < row("14 docs").top());
    // The pull request opens its page; the rest of the row, the terminal.
    let opened = press(&mut state, row("214").center());
    assert!(
        matches!(opened.as_slice(), [Action::OpenLink(link)] if link.as_str().ends_with("/pull/214")),
        "the chip did not open its pull request"
    );
    listed(&mut state, 300.0);
    let opened = press(&mut state, row("Claude Code · feat/auth").center());
    assert!(matches!(opened.as_slice(), [Action::OpenAgent(pane, 1)] if *pane == PaneId::new(12)));
    // A secondary click on a pill offers the terminal and, for an agent
    // whose terminal was opened as a tab, the way back out of view.
    let (_, strip) = frame(&ctx, body(), &mut state, 300.0, Vec::new());
    assert!(
        !has(&strip, "12 auth-refactor"),
        "a pressed row leaves the list open"
    );
    let menu = |state: &mut State, pos: Pos2| {
        let button = |pressed| Input::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![Input::PointerMoved(pos)],
            vec![button(true)],
            vec![button(false)],
        ] {
            frame(&ctx, body(), state, 300.0, events);
        }
        frame(&ctx, body(), state, 300.0, Vec::new()).1
    };
    let offered = menu(&mut state, at(&strip, "13"));
    assert!(has(&offered, "Open terminal") && !has(&offered, "Send to background"));
    frame(&ctx, body(), &mut state, 300.0, key(Key::Escape));
    let offered = menu(&mut state, at(&strip, "12"));
    let sent = press(&mut state, at(&offered, "Send to background"));
    assert!(matches!(sent.as_slice(), [Action::Background(pane)] if *pane == PaneId::new(12)));
    // The narrowest panel keeps both lines of every row inside itself,
    // and gives up the branch before the name.
    let narrow = listed(&mut state, 220.0);
    inside(&narrow, panel(220.0));
    assert!(has(&narrow, "12 auth") && has(&narrow, "214") && has(&narrow, "Needs you"));
}

#[test]
fn the_line_under_the_composer_names_the_lead_and_says_what_its_turn_is_doing() {
    let ctx = context();
    let mut state = State::default();
    let (chat, members) = (chat(), members());
    let chosen = Settings {
        lead: LeadSettings {
            model: Some("opus".into()),
            effort: Some("high".into()),
        },
        ..Settings::default()
    };
    // While the lead's turn runs, the line counts it at its trailing edge.
    let timed = |queued| {
        changed(
            running(&chat, &members, &[], &[], true, queued),
            |project| project.elapsed = Some(Duration::from_secs(72)),
        )
    };
    let (_, texts) = frame(&ctx, timed(0), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Lead at work · 1 min 12 s"));
    assert!(at(&texts, "Claude Code").x < at(&texts, "Lead at work · 1 min 12 s").x);
    let (_, texts) = frame(&ctx, timed(2), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Queued · 2 · 1 min 12 s"));
    let (_, texts) = frame(&ctx, timed(2), &mut state, 220.0, Vec::new());
    inside(&texts, panel(220.0));
    assert!(has(&texts, "Queued · 2 · 1 min 12 s"));
    // The lead's CLI, with the model and effort chosen for it. Where the
    // model is left to the CLI, what it named when it last started.
    let led = |model: Option<&'static str>, settings: &'static Settings| {
        changed(running(&chat, &members, &[], &[], false, 0), |project| {
            project.lead_model = model;
            project.settings = settings;
        })
    };
    let chosen: &'static Settings = Box::leak(Box::new(chosen));
    let (_, texts) = frame(
        &ctx,
        led(Some("claude-opus-5-5"), chosen),
        &mut state,
        300.0,
        Vec::new(),
    );
    assert!(has(&texts, "Claude Code · Opus · High"));
    let reported = led(Some("claude-opus-5-5"), settings());
    let (_, texts) = frame(&ctx, reported, &mut state, 300.0, Vec::new());
    let chip = at(&texts, "Claude Code · claude-opus-5-5");
    // A press on it shows what the lead runs with.
    for input in click(chip) {
        let body = led(Some("claude-opus-5-5"), settings());
        assert!(frame(&ctx, body, &mut state, 300.0, input).0.is_empty());
    }
    assert_eq!(
        (state.segment, state.details),
        (Segment::Settings, Segment::Settings)
    );
    state.show(Segment::Chat);

    // What the lead's CLI put on its turns is under the menu, as an
    // estimate, and is nothing to press.
    let costed = || {
        changed(running(&chat, &members, &[], &[], false, 0), |project| {
            project.usage = Some("$0.42")
        })
    };
    for input in click(menu_button(300.0)) {
        frame(&ctx, costed(), &mut state, 300.0, input);
    }
    let (_, texts) = frame(&ctx, costed(), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Usage estimate") && has(&texts, "$0.42"));
    let mut actions = Vec::new();
    for input in click(at(&texts, "Usage estimate")) {
        actions.extend(frame(&ctx, costed(), &mut state, 300.0, input).0);
    }
    assert!(actions.is_empty());
}

#[test]
fn a_window_too_narrow_for_the_panel_shows_the_project_in_a_sheet() {
    let (chat, members, needs) = (chat(), members(), needs());
    let open = [(PaneId::new(12), 1)];
    // The sheet in a window of `size`, after `events`.
    let sheet =
        |ctx: &egui::Context, state: &mut State, size: Vec2, body: Body, events: Vec<Input>| {
            let screen = Rect::from_min_size(Pos2::ZERO, size);
            let mut closed = false;
            let mut actions = Vec::new();
            let view = View {
                body,
                composing: false,
                window: screen,
                reveal: 1.0,
            };
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let p = Palette::for_config(&crate::config::Config::default());
                    closed |= show_sheet(ui.ctx(), p, &view, state, &mut actions);
                },
            );
            output.textures_delta.clear();
            assert!(!closed);
            (actions, self::texts(&output))
        };
    let settled = |ctx: &egui::Context, state: &mut State, size: Vec2, needs: &[Need]| {
        for _ in 0..3 {
            sheet(
                ctx,
                state,
                size,
                running(&chat, &members, needs, &open, false, 0),
                Vec::new(),
            );
        }
        sheet(
            ctx,
            state,
            size,
            running(&chat, &members, needs, &open, false, 0),
            Vec::new(),
        )
    };
    // The smallest window at one and a half times the size: 640 by 400
    // pixels are 427 by 267 points, where the panel has no room.
    let small = vec2(427.0, 267.0);
    let screen = Rect::from_min_size(Pos2::ZERO, small);
    let ctx = context();
    let mut state = State {
        focus: true,
        ..State::default()
    };
    let (actions, texts) = settled(&ctx, &mut state, small, &needs);
    assert!(actions.is_empty());
    // The same parts as the tab, inside the window. The sheet's title row
    // is the project's header: no title stands over its name. What needs
    // the person leads, one line a row here, and the composer is at the
    // bottom with who leads under it.
    for wanted in ["shop", "13 Codex · api-tests", "Message the lead…"] {
        assert!(has(&texts, wanted), "{wanted} in {texts:?}");
    }
    assert!(has(&texts, "Needs permission") && !has(&texts, "Only you can answer"));
    assert_eq!(count(&texts, "Project"), 0);
    inside(&texts, screen);
    let edge = |texts: &[(String, Rect)], wanted: &str| {
        texts
            .iter()
            .find(|(text, _)| text.contains(wanted))
            .unwrap_or_else(|| panic!("{wanted} in {texts:?}"))
            .1
    };
    assert!(edge(&texts, "Claude Code").bottom() <= screen.bottom() - 8.0);
    assert!(edge(&texts, "Message the lead…").bottom() < edge(&texts, "Claude Code").bottom());
    // The composer took the keyboard that was asked for it.
    assert!(!state.focus);
    assert!(ctx.memory(|memory| memory.has_focus(chat::composer_id())));

    // The strip of its agents is the same one row there, and the chat
    // keeps about four lines over the composer whatever is pinned.
    let (_, texts) = settled(&ctx, &mut state, small, &[]);
    assert!((exact(&texts, "13").center().y - exact(&texts, "12").center().y).abs() < 1.0);
    assert!(!has(&texts, "12 auth-refactor"));
    for wanted in ["All tests pass.", "Open terminal"] {
        assert!(has(&texts, wanted), "{wanted} in {texts:?}");
    }
    let room = edge(&texts, "Message the lead…").top() - exact(&texts, "13").bottom();
    assert!(room >= TRANSCRIPT, "{room}");
    // The list of them all lies over the chat there as well, inside the
    // window and over a composer that keeps its place. Escape puts it away.
    for events in click(exact(&texts, "2").center()) {
        sheet(
            &ctx,
            &mut state,
            small,
            running(&chat, &members, &[], &open, false, 0),
            events,
        );
    }
    let (_, listed) = settled(&ctx, &mut state, small, &[]);
    assert!(has(&listed, "1 needs you · 1 working") && has(&listed, "13 api-tests"));
    assert_eq!(
        edge(&listed, "Message the lead…"),
        edge(&texts, "Message the lead…")
    );
    assert!(edge(&listed, "13 api-tests").bottom() < screen.bottom());
    inside(&listed, screen);
    sheet(
        &ctx,
        &mut state,
        small,
        running(&chat, &members, &[], &open, false, 0),
        key(Key::Escape),
    );
    let (_, texts) = settled(&ctx, &mut state, small, &[]);
    assert!(!has(&texts, "13 api-tests"));
    assert!(edge(&texts, "Claude Code").bottom() <= screen.bottom() - 8.0);

    // A taller window has the same title row, and room to say all of what
    // needs the person.
    let ctx = context();
    let mut state = State::default();
    let tall = vec2(427.0, 480.0);
    let (_, texts) = settled(&ctx, &mut state, tall, &needs);
    assert_eq!((count(&texts, "Project"), count(&texts, "shop")), (0, 1));
    assert!(has(&texts, "Only you can answer"));
    let (_, texts) = settled(&ctx, &mut state, tall, &[]);
    assert!(count(&texts, "13") == 1 && !has(&texts, "12 auth-refactor"));
    // Its details are a press away there as well.
    let title = edge(&texts, "shop").center().y;
    let details = Pos2::new(tall.x * 0.5 + (tall.x - 24.0) * 0.5 - 44.0 - 42.0, title);
    for events in click(details) {
        sheet(
            &ctx,
            &mut state,
            tall,
            running(&chat, &members, &[], &open, false, 0),
            events,
        );
    }
    assert_eq!(state.segment, Segment::Context);
    let (_, texts) = settled(&ctx, &mut state, tall, &[]);
    assert!(has(&texts, "Instructions") && !has(&texts, "Message the lead…"));
    inside(&texts, Rect::from_min_size(Pos2::ZERO, tall));

    // Without a project the sheet is named for the form, which has no room
    // to explain what a lead is.
    let mut state = State::default();
    let form = || empty(Some(BOTH), false, None);
    for _ in 0..3 {
        sheet(&ctx, &mut state, small, form(), Vec::new());
    }
    let (_, texts) = sheet(&ctx, &mut state, small, form(), Vec::new());
    assert_eq!(count(&texts, "New project"), 1);
    assert!(has(&texts, "What do you want to get done?") && has(&texts, "Claude Code"));
    assert!(!has(&texts, "A lead plans the work"));
    inside(&texts, screen);
}

/// An agent called `worker-<pane>` that waits, works or rests.
fn worker(pane: u64, waits: bool, works: bool) -> Member {
    let state = if waits {
        "Needs you"
    } else if works {
        "Working"
    } else {
        "Idle"
    };
    Member {
        pane: PaneId::new(pane),
        generation: 1,
        kind: AgentKind::Claude,
        title: format!("worker-{pane}"),
        state: state.into(),
        detail: state.into(),
        waits,
        works,
        ready: false,
        elapsed: Some(Duration::from_secs(60)),
        branch: None,
        pull_requests: Vec::new(),
        can_background: false,
    }
}
/// Where the one text that reads `wanted` is.
fn exact(texts: &[(String, Rect)], wanted: &str) -> Rect {
    assert_eq!(count(texts, wanted), 1, "{wanted} in {texts:?}");
    texts.iter().find(|(text, _)| text == wanted).unwrap().1
}

#[test]
fn the_strip_is_one_row_however_many_agents_and_their_list_lies_over_the_chat() {
    let chat = chat();
    // A panel as low as a 640 by 480 window leaves it.
    let rect = Rect::from_min_size(Pos2::new(600.0, 78.0), vec2(300.0, 330.0));
    let draw = |ctx: &egui::Context, state: &mut State, members: &[Member], events| {
        let body = running(&chat, members, &[], &[], false, 0);
        let (_, output) = frame_in(ctx, body, state, rect, events);
        texts(&output)
    };
    // Where the chat begins and the composer stands, for each count.
    let mut places = Vec::new();
    for many in [1, 4, 12] {
        let ctx = context();
        let mut state = State::default();
        let members: Vec<Member> = (20..20 + many)
            .map(|pane| worker(pane, false, false))
            .collect();
        for _ in 0..3 {
            draw(&ctx, &mut state, &members, Vec::new());
        }
        let texts = draw(&ctx, &mut state, &members, Vec::new());
        // Every pill in view and the count of them stand on the one row
        // under the project's name.
        let row = exact(&texts, "20").center().y;
        assert!(
            (row - (rect.top() + HEADER + ROW * 0.5)).abs() < 1.5,
            "{row}"
        );
        let numbered: Vec<&(String, Rect)> = texts
            .iter()
            .filter(|(text, _)| text.parse::<u64>().is_ok())
            .collect();
        assert!(numbered.len() > many.min(4) as usize, "{numbered:?}");
        for (text, at) in numbered {
            assert!((at.center().y - row).abs() < 1.0, "{text} at {at:?}");
        }
        assert!(exact(&texts, &many.to_string()).left() > exact(&texts, "20").right());
        inside(&texts, rect);
        let place = |texts: &[(String, Rect)]| {
            (
                exact(texts, "Split checkout into auth and API tests"),
                exact(texts, "Message the lead…"),
            )
        };
        places.push(place(&texts));
        // Their list opens over the chat and moves nothing of it: no
        // taller than its share, with the rest of it a scroll away.
        state.show_agents = true;
        draw(&ctx, &mut state, &members, Vec::new());
        let listed = draw(&ctx, &mut state, &members, Vec::new());
        assert!(has(&listed, "20 worker-20"), "{listed:?}");
        assert_eq!(place(&listed), place(&texts));
        let limit = rect.top() + HEADER + ROW + (rect.height() - 8.0) * ROSTER + 16.0;
        for (text, at) in &listed {
            assert!(
                !text.contains(" worker-") || at.bottom() <= limit,
                "{text} at {at:?}"
            );
        }
        assert!(!has(&listed, "31 worker-31"), "{listed:?}");
        draw(&ctx, &mut state, &members, key(Key::Escape));
        let away = draw(&ctx, &mut state, &members, Vec::new());
        assert!(!has(&away, "20 worker-20"));

        if many < 12 {
            continue;
        }
        // Twelve pills do not fit a row: each is its mark and its number,
        // and the rest are a turn of the wheel away, on the same row.
        assert!(!has(&texts, "worker-20"));
        assert_eq!((count(&texts, "20"), count(&texts, "31")), (1, 0));
        draw(
            &ctx,
            &mut state,
            &members,
            vec![Input::PointerMoved(exact(&texts, "21").center())],
        );
        draw(
            &ctx,
            &mut state,
            &members,
            vec![Input::MouseWheel {
                phase: egui::TouchPhase::Move,
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -600.0),
                modifiers: Modifiers::NONE,
            }],
        );
        for _ in 0..30 {
            draw(&ctx, &mut state, &members, Vec::new());
        }
        let scrolled = draw(&ctx, &mut state, &members, Vec::new());
        assert_eq!((count(&scrolled, "20"), count(&scrolled, "31")), (0, 1));
        assert!((exact(&scrolled, "31").center().y - row).abs() < 1.0);
        assert_eq!(place(&scrolled), place(&texts));
    }
    // A strip that goes on past its edge fades there, and only there.
    let fades = |many: u64, wheel: f32| {
        let ctx = context();
        let mut state = State::default();
        let members: Vec<Member> = (20..20 + many)
            .map(|pane| worker(pane, false, false))
            .collect();
        let mut frame = |events| {
            let body = running(&chat, &members, &[], &[], false, 0);
            frame_in(&ctx, body, &mut state, rect, events).1
        };
        frame(vec![Input::PointerMoved(Pos2::new(
            rect.left() + 40.0,
            rect.top() + HEADER + ROW * 0.5,
        ))]);
        frame(vec![Input::MouseWheel {
            phase: egui::TouchPhase::Move,
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, wheel),
            modifiers: Modifiers::NONE,
        }]);
        for _ in 0..30 {
            frame(Vec::new());
        }
        let output = frame(Vec::new());
        let mut edges: Vec<f32> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Mesh(mesh) => Some(mesh.calc_bounds().left()),
                _ => None,
            })
            .collect();
        edges.sort_by(f32::total_cmp);
        edges
    };
    assert!(fades(4, 0.0).is_empty());
    let leading = rect.left() + 6.0;
    // At its start the trailing edge, in its middle both, at its end the
    // leading one.
    assert!(matches!(fades(12, 0.0).as_slice(), [edge] if *edge > leading + 100.0));
    assert!(matches!(fades(12, -100.0).as_slice(), [first, second]
        if *first == leading && *second > leading + 100.0));
    assert!(matches!(fades(12, -600.0).as_slice(), [edge] if *edge == leading));
    // One agent or twelve, the chat begins and the composer stands where
    // they did.
    assert!(
        places.windows(2).all(|pair| pair[0] == pair[1]),
        "{places:?}"
    );
}

#[test]
fn pills_lead_with_who_waits_then_who_works_each_by_number() {
    let members = [
        worker(9, false, false),
        worker(4, false, true),
        worker(7, true, false),
        worker(3, false, false),
        worker(5, false, true),
        worker(2, true, false),
    ];
    let order: Vec<u64> = ordered(&members)
        .iter()
        .map(|member| member.pane.get())
        .collect();
    assert_eq!(order, [2, 7, 4, 5, 3, 9]);
    let ctx = context();
    let mut state = State::default();
    let chat = chat();
    let body = || running(&chat, &members, &[], &[], false, 0);
    frame(&ctx, body(), &mut state, 420.0, Vec::new());
    let (_, texts) = frame(&ctx, body(), &mut state, 420.0, Vec::new());
    let lefts: Vec<f32> = order
        .iter()
        .map(|pane| exact(&texts, &pane.to_string()).left())
        .collect();
    assert!(lefts.windows(2).all(|pair| pair[0] < pair[1]), "{lefts:?}");
    // The control that counts them stays at the trailing edge.
    assert!(exact(&texts, "6").left() > lefts[5]);
}

#[test]
fn pills_are_as_wide_as_their_words_then_narrowed_alike_down_to_a_number() {
    let sizes = [(38.0, 120.0), (38.0, 60.0), (38.0, 120.0), (31.0, 31.0)];
    let gaps = PILL_GAP * 3.0;
    // With room for all of them, each is as wide as its words.
    assert_eq!(pill_widths(&sizes, 400.0), [120.0, 60.0, 120.0, 31.0]);
    // Without, the widest give way first and the row is filled.
    let narrowed = pill_widths(&sizes, 291.0 + gaps);
    assert!((narrowed[0] - 100.0).abs() < 0.5 && narrowed[0] == narrowed[2]);
    assert_eq!((narrowed[1], narrowed[3]), (60.0, 31.0));
    assert!(narrowed.iter().sum::<f32>() + gaps <= 291.0 + gaps);
    // A name of a letter or two is left out for the number alone, and no
    // pill is narrower than its mark and number: fewer fit, and scroll.
    assert_eq!(pill_widths(&sizes, 180.0), [38.0, 38.0, 38.0, 31.0]);
    assert_eq!(pill_widths(&sizes, 60.0), [38.0, 38.0, 38.0, 31.0]);
    assert!(pill_widths(&[], 100.0).is_empty());

    // The narrowest panel still shows each agent's mark and number, and
    // the control that lists them, inside itself.
    let ctx = context();
    let mut state = State::default();
    let chat = chat();
    let members = [
        worker(12, false, true),
        worker(13, true, false),
        worker(14, false, false),
    ];
    let body = || running(&chat, &members, &[], &[], false, 0);
    frame(&ctx, body(), &mut state, 220.0, Vec::new());
    let (_, texts) = frame(&ctx, body(), &mut state, 220.0, Vec::new());
    for wanted in ["13", "12", "14", "3"] {
        assert_eq!(count(&texts, wanted), 1, "{wanted} in {texts:?}");
    }
    inside(&texts, panel(220.0));
}

#[test]
fn a_pill_is_opened_from_the_keyboard_and_says_the_rest_under_the_pointer() {
    let ctx = context();
    let mut state = State::default();
    let (chat, members) = (chat(), members());
    let body = || running(&chat, &members, &[], &[], false, 0);
    let (_, texts) = frame(&ctx, body(), &mut state, 300.0, Vec::new());
    // Enter and Space press the pill that holds the keyboard.
    for pressed in [Key::Enter, Key::Space] {
        ctx.memory_mut(|memory| {
            memory.request_focus(Id::new(("project-agent-pill", PaneId::new(12))))
        });
        frame(&ctx, body(), &mut state, 300.0, Vec::new());
        let mut opened = Vec::new();
        for event in key(pressed) {
            opened.extend(frame(&ctx, body(), &mut state, 300.0, vec![event]).0);
        }
        assert!(
            matches!(opened.as_slice(), [Action::OpenAgent(pane, 1)] if *pane == PaneId::new(12)),
            "{pressed:?}"
        );
    }
    // A rest of the pointer says what the pill has no room for: the CLI
    // and branch, and what the agent is doing for how long. A wait is said
    // exactly.
    let said = |state: &mut State, pos: Pos2| {
        frame(&ctx, body(), state, 300.0, vec![Input::PointerMoved(pos)]);
        let mut texts = Vec::new();
        for _ in 0..60 {
            texts = frame(&ctx, body(), state, 300.0, Vec::new()).1;
        }
        texts
    };
    let card = said(&mut state, at(&texts, "12"));
    for wanted in [
        "12 auth-refactor",
        "Claude Code · feat/auth",
        "Working · 14m",
    ] {
        assert!(has(&card, wanted), "{wanted} in {card:?}");
    }
    let card = said(&mut state, at(&texts, "13"));
    for wanted in ["13 api-tests", "Codex", "Needs permission · now"] {
        assert!(has(&card, wanted), "{wanted} in {card:?}");
    }
    // The control that lists them says how they stand.
    let card = said(&mut state, at(&texts, "2"));
    assert!(has(&card, "1 needs you · 1 working"), "{card:?}");
}

#[test]
fn an_action_that_cannot_be_taken_is_not_shown_in_the_accent() {
    let p = Palette::for_config(&crate::config::Config::default());
    // Grey, as a control that is not the recommended one.
    let plain = |fill: egui::Color32| {
        fill != p.accent && (i32::from(fill.b()) - i32::from(fill.r())).abs() < 24
    };
    let ctx = context();
    let (chat, members) = (chat(), members());
    #[allow(clippy::type_complexity)]
    let check = |pos: &dyn Fn(&[(String, Rect)]) -> Pos2,
                 state: &mut State,
                 body: &dyn Fn() -> Body<'static>,
                 ready: bool,
                 what: &str| {
        for _ in 0..2 {
            frame_in(&ctx, body(), state, panel(300.0), Vec::new());
        }
        let (_, output) = frame_in(&ctx, body(), state, panel(300.0), Vec::new());
        let fill = fill_at(&output, pos(&texts(&output)));
        if ready {
            assert_eq!(fill, p.accent, "{what}");
        } else {
            assert!(plain(fill), "{what} is {fill:?}");
        }
    };
    // The control that starts a project, without a goal and with one.
    let form = || empty(Some(&[AgentKind::Claude]), false, None);
    let start = |texts: &[(String, Rect)]| start_control(texts, "Enter starts", 300.0);
    let mut state = State::default();
    check(&start, &mut state, &form, false, "Start");
    state.goal = "Split checkout".into();
    check(&start, &mut state, &form, true, "Start");
    // Save, with the instructions changed and with more than can be kept.
    let chat: &'static Transcript = Box::leak(Box::new(chat));
    let members: &'static [Member] = Box::leak(members.into_boxed_slice());
    let project = || running(chat, members, &[], &[], false, 0);
    let named = |label: &'static str| move |texts: &[(String, Rect)]| at(texts, label);
    let mut state = State::default();
    state.show(Segment::Context);
    state.instructions.insert(ID, "Small commits.".into());
    check(&named("Save"), &mut state, &project, true, "Save");
    state.instructions.insert(ID, "x".repeat(MAX_INSTRUCTIONS));
    check(&named("Save"), &mut state, &project, false, "Save");
    // Add, with the form empty and filled in.
    let mut state = State::default();
    state.show(Segment::Watches);
    state.watch.open = true;
    check(&named("Add"), &mut state, &project, false, "Add");
    state.watch.title = "Nightly".into();
    state.watch.instruction = "Look at CI.".into();
    check(&named("Add"), &mut state, &project, true, "Add");
}

#[test]
fn a_message_is_written_sent_with_enter_and_a_running_turn_can_be_stopped() {
    let ctx = context();
    let mut state = State::default();
    let (chat, members) = (chat(), members());
    let body = |busy, queued| running(&chat, &members, &[], &[], busy, queued);
    // The composer takes the keyboard only when asked to.
    frame(&ctx, body(false, 0), &mut state, 300.0, Vec::new());
    frame(
        &ctx,
        body(false, 0),
        &mut state,
        300.0,
        vec![Input::Text("lost".into())],
    );
    assert!(state.drafts.get(&ID).is_none_or(String::is_empty));
    state.focus = true;
    frame(&ctx, body(false, 0), &mut state, 300.0, Vec::new());
    frame(
        &ctx,
        body(false, 0),
        &mut state,
        300.0,
        vec![Input::Text(" go ahead ".into())],
    );
    assert_eq!(state.drafts[&ID], " go ahead ");
    let (actions, _) = frame(&ctx, body(false, 0), &mut state, 300.0, key(Key::Enter));
    assert_eq!(
        events(actions),
        [Event::Send {
            project: ID,
            text: "go ahead".into(),
            attachments: Vec::new(),
        }]
    );
    assert_eq!(state.drafts[&ID], "", "what was sent leaves the field");
    // While a turn runs the control stops it, and says what waits.
    let (_, texts) = frame(&ctx, body(true, 2), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Queued · 2"));
    let bounds = panel(300.0);
    let control = Pos2::new(
        bounds.right() - 8.0 - 17.0,
        bounds.bottom() - 8.0 - HINT - 16.0,
    );
    let mut actions = Vec::new();
    for events in click(control) {
        actions.extend(frame(&ctx, body(true, 2), &mut state, 300.0, events).0);
    }
    assert_eq!(events(actions), [Event::Stop(ID)]);
    // A message written meanwhile is sent, to wait its turn.
    state.focus = true;
    frame(&ctx, body(true, 2), &mut state, 300.0, Vec::new());
    frame(
        &ctx,
        body(true, 2),
        &mut state,
        300.0,
        vec![Input::Text("and tests".into())],
    );
    let mut actions = Vec::new();
    for events in click(control) {
        actions.extend(frame(&ctx, body(true, 2), &mut state, 300.0, events).0);
    }
    assert_eq!(
        events(actions),
        [Event::Send {
            project: ID,
            text: "and tests".into(),
            attachments: Vec::new(),
        }]
    );
    assert!(state.focus, "the field takes the keyboard back");
    // Idle with nothing written, the control does nothing.
    let mut actions = Vec::new();
    for events in click(control) {
        actions.extend(frame(&ctx, body(false, 0), &mut state, 300.0, events).0);
    }
    assert!(actions.is_empty());
}

#[test]
fn a_project_that_worked_here_before_is_offered_again_under_the_form() {
    let ctx = context();
    let mut state = State::default();
    let form = |reopen| empty(Some(&[AgentKind::Claude]), false, reopen);
    let (_, texts) = frame(&ctx, form(None), &mut state, 300.0, Vec::new());
    assert!(!has(&texts, "Reopen") && !has(&texts, "Earlier in this folder"));
    assert!(has(&texts, "A lead plans the work"));
    let (_, texts) = frame(&ctx, form(Some("shop")), &mut state, 300.0, Vec::new());
    for wanted in [
        "Earlier in this folder",
        "shop",
        "Its chat and notes are still here",
    ] {
        assert!(count(&texts, wanted) == 1, "{wanted} in {texts:?}");
    }
    // Someone with an earlier project knows what a lead is.
    assert!(!has(&texts, "A lead plans the work"));
    // The form to start another stays above the offer.
    assert!(at(&texts, "What do you want to get done?").y < at(&texts, "Reopen").y);
    let mut actions = Vec::new();
    for input in click(at(&texts, "Reopen")) {
        actions.extend(frame(&ctx, form(Some("shop")), &mut state, 300.0, input).0);
    }
    assert_eq!(events(actions), [Event::Reopen(WorkspaceId::new(3))]);
    // A long name is cut, and the narrowest panel holds all of it.
    let long = "a very long project name that no narrow panel could ever show whole";
    let (_, narrow) = frame(&ctx, form(Some(long)), &mut state, 220.0, Vec::new());
    let named = narrow.iter().find(|(text, _)| text == long).unwrap().1;
    let reopen = narrow.iter().find(|(text, _)| text == "Reopen").unwrap().1;
    assert!(named.right() < reopen.left(), "{narrow:?}");
    inside(&narrow, panel(220.0));
    // While a project is being made, nothing else is offered.
    let starting = empty(Some(&[AgentKind::Claude]), true, Some("shop"));
    let (_, texts) = frame(&ctx, starting, &mut state, 300.0, Vec::new());
    assert!(!has(&texts, "Reopen"));
}

#[test]
fn the_menu_holds_what_is_rarely_done_and_a_read_only_project_takes_nothing() {
    let ctx = context();
    let mut state = State::default();
    let chat = chat();
    let body = |locked, more| {
        changed(running(&chat, &[], &[], &[], false, 0), |project| {
            project.locked = locked;
            project.chat.more = more;
        })
    };
    let press = |state: &mut State, locked: bool, pos: Pos2| {
        let mut actions = Vec::new();
        for input in click(pos) {
            actions.extend(frame(&ctx, body(locked, false), state, 300.0, input).0);
        }
        events(actions)
    };
    let open = |state: &mut State, locked: bool| {
        assert!(press(state, locked, menu_button(300.0)).is_empty());
        frame(&ctx, body(locked, false), state, 300.0, Vec::new()).1
    };
    let texts = open(&mut state, false);
    let order = [
        "New chat",
        "Pause project",
        "Rename…",
        "Reveal saved files",
        "Remove project…",
    ];
    for pair in order.windows(2) {
        assert!(at(&texts, pair[0]).y < at(&texts, pair[1]).y, "{pair:?}");
    }
    // Without a cost on any turn the menu says nothing of usage.
    assert!(!has(&texts, "Usage estimate"));
    frame(
        &ctx,
        body(false, false),
        &mut state,
        300.0,
        key(Key::Escape),
    );
    for (label, event) in [
        ("New chat", Event::NewChat(ID)),
        ("Rename…", Event::Rename(ID)),
        ("Reveal saved files", Event::Reveal(ID)),
        ("Remove project…", Event::Remove(ID)),
    ] {
        let texts = open(&mut state, false);
        assert_eq!(press(&mut state, false, at(&texts, label)), [event]);
    }
    // Older entries in its folder are asked for where the chat begins.
    let (_, texts) = frame(&ctx, body(false, false), &mut state, 300.0, Vec::new());
    assert!(!has(&texts, "Load earlier"));
    let (_, texts) = frame(&ctx, body(false, true), &mut state, 300.0, Vec::new());
    let earlier = at(&texts, "Load earlier");
    let mut actions = Vec::new();
    for input in click(earlier) {
        actions.extend(frame(&ctx, body(false, true), &mut state, 300.0, input).0);
    }
    assert_eq!(events(actions), [Event::Earlier(ID)]);
    assert_eq!(state.shown, chat::PAGE * 2);

    // A project nothing is written of takes no message, begins no new
    // chat, is not paused and does not hold on to a request for the
    // keyboard.
    state.focus = true;
    let (_, texts) = frame(&ctx, body(true, false), &mut state, 300.0, Vec::new());
    assert!(!state.focus);
    assert!(has(&texts, "This project is read-only"));
    assert!(has(&texts, "Nothing here is changed"));
    state.drafts.insert(ID, "left from before".into());
    let (actions, _) = frame(&ctx, body(true, false), &mut state, 300.0, key(Key::Enter));
    assert!(actions.is_empty());
    assert_eq!(state.drafts[&ID], "left from before");
    for label in ["New chat", "Pause project"] {
        let texts = open(&mut state, true);
        assert!(press(&mut state, true, at(&texts, label)).is_empty());
        frame(&ctx, body(true, false), &mut state, 300.0, key(Key::Escape));
    }
    let texts = open(&mut state, true);
    assert_eq!(
        press(&mut state, true, at(&texts, "Reveal saved files")),
        [Event::Reveal(ID)]
    );
}

fn kept_files() -> Vec<File> {
    vec![
        File {
            name: "notes/auth-tokens.md".into(),
            author: "agent 12 \"auth-refactor\"".into(),
            age: Duration::from_secs(3 * 3600),
            size: "2.1 KB".into(),
            removable: true,
        },
        File {
            name: "DECISIONS.md".into(),
            author: "lead".into(),
            age: Duration::from_secs(4 * 60),
            size: "812 B".into(),
            removable: true,
        },
        File {
            name: "INDEX.md".into(),
            author: "Neptune".into(),
            age: Duration::from_secs(10),
            size: "300 B".into(),
            removable: false,
        },
    ]
}
/// The control at the trailing edge of the row whose name reads `name`.
fn row_menu(texts: &[(String, Rect)], name: &str) -> Pos2 {
    Pos2::new(
        panel(300.0).right() - 8.0 - 4.0 - 14.0,
        at(texts, name).y + 8.0,
    )
}
const HINT_TEXT: &str = "How should agents work here? For example: small commits, run the \
                         tests before you report, never push.";

#[test]
fn the_details_are_one_press_away_and_their_context_is_what_the_agents_share() {
    let ctx = context();
    let mut state = State::default();
    let (chat, members, needs, files) = (chat(), members(), needs(), kept_files());
    let body = |instructions: &'static str, locked: bool, read: bool| {
        changed(running(&chat, &members, &needs, &[], false, 0), |project| {
            project.locked = locked;
            project.context = Context {
                files: if read { &files } else { &[] },
                instructions,
                read,
            };
        })
    };
    let draw = |state: &mut State, kept: &'static str, width: f32, input: Vec<Input>| {
        frame(&ctx, body(kept, false, true), state, width, input)
    };
    let press = |state: &mut State, kept: &'static str, pos: Pos2| {
        let mut actions = Vec::new();
        for input in click(pos) {
            actions.extend(draw(state, kept, 300.0, input).0);
        }
        events(actions)
    };

    // The chat is what the tab opens with. One control of the header
    // shows everything else, on the part last looked at.
    let (_, texts) = draw(&mut state, "", 300.0, Vec::new());
    assert!(has(&texts, "Message the lead…") && !has(&texts, "Instructions"));
    assert!(press(&mut state, "", details_button(300.0)).is_empty());
    assert_eq!(state.segment, Segment::Context);
    state.focus = true;
    // A few frames let the control's thumb come to rest.
    for _ in 0..3 {
        draw(&mut state, "", 300.0, Vec::new());
    }
    let (actions, texts) = draw(&mut state, "", 300.0, Vec::new());
    assert!(actions.is_empty());
    assert!(!state.focus, "no field here asked for the keyboard");
    assert!(!has(&texts, "Message the lead…") && !has(&texts, "12 auth-refactor"));
    assert!(at(&texts, "Context").x < at(&texts, "Watches").x);
    assert!(at(&texts, "Watches").x < at(&texts, "Settings").x);
    let position = |wanted: &str| {
        texts
            .iter()
            .find(|(text, _)| text.contains(wanted))
            .unwrap_or_else(|| panic!("{wanted} in {texts:?}"))
            .1
            .top()
    };
    // What needs the person is counted in one line here, not said again.
    let order = [
        "shop",
        "Context",
        "2 needs you",
        "Instructions",
        "How should agents work here?",
        "Every agent is handed this first.",
        "Files",
        "notes/auth-tokens.md",
        "agent 12 \"auth-refactor\" · 3h · 2.1 KB",
        "DECISIONS.md",
        "lead · 4m · 812 B",
        "INDEX.md",
        "Neptune · now · 300 B",
        "Reveal context folder",
    ];
    for pair in order.windows(2) {
        assert!(position(pair[0]) <= position(pair[1]), "{pair:?}");
    }
    assert!(!has(&texts, "Only you can answer") && !has(&texts, "Try again"));
    // Saving is offered only while there is something to save.
    for absent in ["Save", "Revert", "Not saved", "Saved"] {
        assert_eq!(count(&texts, absent), 0, "{absent}");
    }
    // That line, the chevron and the header's control each lead back to
    // the chat; the details open again where they were left.
    assert!(press(&mut state, "", at(&texts, "2 needs you")).is_empty());
    assert_eq!(state.segment, Segment::Chat);
    press(&mut state, "", details_button(300.0));
    let (_, texts) = draw(&mut state, "", 300.0, Vec::new());
    press(&mut state, "", at(&texts, "Watches"));
    assert_eq!(state.segment, Segment::Watches);
    let bounds = panel(300.0);
    let back = Pos2::new(bounds.left() + 6.0 + 14.0, bounds.top() + HEADER + 14.0);
    press(&mut state, "", back);
    assert_eq!(state.segment, Segment::Chat);
    press(&mut state, "", details_button(300.0));
    assert_eq!(state.segment, Segment::Watches);
    press(&mut state, "", details_button(300.0));
    assert_eq!(state.segment, Segment::Chat);
    state.show(Segment::Context);
    for _ in 0..3 {
        draw(&mut state, "", 300.0, Vec::new());
    }

    // The instructions are written in their own field. Enter breaks
    // the line; nothing is kept until the person says so.
    let (_, texts) = draw(&mut state, "", 300.0, Vec::new());
    assert!(press(&mut state, "", at(&texts, HINT_TEXT)).is_empty());
    draw(
        &mut state,
        "",
        300.0,
        vec![Input::Text("Small commits.".into())],
    );
    let (actions, _) = draw(&mut state, "", 300.0, key(Key::Enter));
    assert!(actions.is_empty(), "Enter saves nothing");
    draw(
        &mut state,
        "",
        300.0,
        vec![Input::Text("Run the tests. ".into())],
    );
    assert_eq!(state.instructions[&ID], "Small commits.\nRun the tests. ");
    let (_, texts) = draw(&mut state, "", 300.0, Vec::new());
    assert!(has(&texts, "Not saved") && !has(&texts, "Every agent is handed this first."));
    assert_eq!(
        press(&mut state, "", at(&texts, "Save")),
        [Event::SaveInstructions {
            project: ID,
            text: "Small commits.\nRun the tests.".into(),
        }]
    );
    // What was written stays the person's until the folder holds it.
    assert!(state.instructions.contains_key(&ID));
    let saved = "Small commits.\nRun the tests.";
    let (_, texts) = draw(&mut state, saved, 300.0, Vec::new());
    assert!(!state.instructions.contains_key(&ID));
    assert!(has(&texts, "Small commits.") && has(&texts, "Every agent is handed this first."));
    assert_eq!(count(&texts, "Save") + count(&texts, "Not saved"), 0);
    // A change can be taken back to what the folder holds.
    let field = texts
        .iter()
        .find(|(text, _)| text.starts_with("Small commits."))
        .unwrap()
        .1
        .center();
    assert!(press(&mut state, saved, field).is_empty());
    draw(&mut state, saved, 300.0, vec![Input::Text("x".into())]);
    assert!(state.instructions.contains_key(&ID));
    let (_, texts) = draw(&mut state, saved, 300.0, Vec::new());
    assert!(press(&mut state, saved, at(&texts, "Revert")).is_empty());
    assert!(!state.instructions.contains_key(&ID));
    // More than the file may hold is not offered for saving.
    state.instructions.insert(ID, "x".repeat(MAX_INSTRUCTIONS));
    let (_, texts) = draw(&mut state, saved, 300.0, Vec::new());
    assert!(has(&texts, "Too long to save"));
    assert!(press(&mut state, saved, at(&texts, "Save")).is_empty());
    state.instructions.clear();
    // The keyboard is given up for the terminal with the tab.
    ctx.memory_mut(|memory| memory.surrender_focus(instructions_id()));

    // A press on a file opens it; its menu shows it or deletes it,
    // and deleting asks once more in the row.
    let (_, texts) = draw(&mut state, saved, 300.0, Vec::new());
    assert_eq!(
        press(&mut state, saved, at(&texts, "DECISIONS.md")),
        [Event::OpenFile(ID, "DECISIONS.md".into())]
    );
    let open = |state: &mut State, name: &str| {
        let (_, texts) = draw(state, saved, 300.0, Vec::new());
        assert!(press(state, saved, row_menu(&texts, name)).is_empty());
        draw(state, saved, 300.0, Vec::new()).1
    };
    let menu = open(&mut state, "notes/auth-tokens.md");
    for wanted in ["Open", "Reveal in file manager", "Delete…"] {
        assert!(has(&menu, wanted), "{wanted} in {menu:?}");
    }
    assert_eq!(
        press(&mut state, saved, at(&menu, "Reveal in file manager")),
        [Event::RevealFile(ID, "notes/auth-tokens.md".into())]
    );
    let menu = open(&mut state, "notes/auth-tokens.md");
    assert!(press(&mut state, saved, at(&menu, "Delete…")).is_empty());
    assert_eq!(
        state.deleting,
        Some((ID, "notes/auth-tokens.md".to_owned()))
    );
    let (_, asking) = draw(&mut state, saved, 300.0, Vec::new());
    assert!(has(&asking, "Delete notes/auth-tokens.md?"));
    assert!(press(&mut state, saved, at(&asking, "Cancel")).is_empty());
    assert_eq!(state.deleting, None);
    let menu = open(&mut state, "notes/auth-tokens.md");
    press(&mut state, saved, at(&menu, "Delete…"));
    let (_, asking) = draw(&mut state, saved, 300.0, Vec::new());
    assert_eq!(
        press(&mut state, saved, at(&asking, "Delete")),
        [Event::DeleteFile(ID, "notes/auth-tokens.md".into())]
    );
    assert_eq!(state.deleting, None);
    // The index is Neptune's to write again, so it is not deleted.
    let menu = open(&mut state, "INDEX.md");
    assert!(has(&menu, "Reveal in file manager") && !has(&menu, "Delete…"));
    draw(&mut state, saved, 300.0, key(Key::Escape));
    // The folder all of them are in is shown by the file manager.
    let (_, texts) = draw(&mut state, saved, 300.0, Vec::new());
    assert_eq!(
        press(&mut state, saved, at(&texts, "Reveal context folder")),
        [Event::RevealContext(ID)]
    );

    // The narrowest panel keeps all of it inside itself.
    let (_, narrow) = draw(&mut state, saved, 220.0, Vec::new());
    inside(&narrow, panel(220.0));
    for wanted in ["Context", "Watches", "Settings", "Files", "DECISIONS.md"] {
        assert!(has(&narrow, wanted), "{wanted} in {narrow:?}");
    }

    // Before the folder was read the list says so and the field waits;
    // a read-only project's instructions are shown and not changed.
    let (_, texts) = frame(&ctx, body("", false, false), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Reading…"));
    let (_, texts) = frame(&ctx, body(saved, true, true), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Read-only") && has(&texts, "Small commits."));
    let field = texts
        .iter()
        .find(|(text, _)| text.starts_with("Small commits."))
        .unwrap()
        .1
        .center();
    for input in click(field) {
        frame(&ctx, body(saved, true, true), &mut state, 300.0, input);
    }
    let typed = vec![Input::Text("mine".into())];
    frame(&ctx, body(saved, true, true), &mut state, 300.0, typed);
    assert!(state.instructions.is_empty());
    // A question about one project's file is not asked of another's
    // file of the same name, and is not left open back in the chat.
    state.deleting = Some((ProjectId::new(9), "DECISIONS.md".into()));
    let (_, texts) = draw(&mut state, saved, 300.0, Vec::new());
    assert!(!has(&texts, "Delete DECISIONS.md?") && state.deleting.is_none());
    state.deleting = Some((ID, "DECISIONS.md".into()));
    let (_, texts) = draw(&mut state, saved, 300.0, Vec::new());
    assert!(has(&texts, "Delete DECISIONS.md?"));
    state.show(Segment::Chat);
    let (_, texts) = draw(&mut state, saved, 300.0, Vec::new());
    assert!(has(&texts, "Message the lead…") && state.deleting.is_none());
}

fn kept_watches() -> Vec<Watch> {
    vec![
        Watch {
            id: 1,
            title: "Nightly check".into(),
            trigger: "Every 1 h".into(),
            detail: "Next in 14 min · Last ran 5 min ago".into(),
            paused: false,
            proposed: false,
            instruction: String::new(),
        },
        Watch {
            id: 2,
            title: "PR #83".into(),
            trigger: "Pull request zevem/neptune#83".into(),
            detail: "Paused".into(),
            paused: true,
            proposed: false,
            instruction: String::new(),
        },
        Watch {
            id: 3,
            title: "Hourly".into(),
            trigger: "Every 15 min".into(),
            detail: "Proposed by the lead · waits for you".into(),
            paused: false,
            proposed: true,
            instruction: "Look at the nightly build and say what broke.".into(),
        },
    ]
}

#[test]
fn the_watches_are_held_back_run_and_deleted_from_their_menus_and_the_person_adds_one() {
    let ctx = context();
    let mut state = State::default();
    state.show(Segment::Watches);
    let (chat, members, needs, kept) = (chat(), members(), needs(), kept_watches());
    // The form is tried in a tab with little above it, so that all of
    // it is in view without scrolling.
    let short = std::cell::Cell::new(false);
    let body = |locked: bool| {
        let needs = if short.get() { &[][..] } else { &needs[..] };
        changed(running(&chat, &members, needs, &[], false, 0), |project| {
            project.locked = locked;
            project.watches = if short.get() { &kept[..1] } else { &kept };
        })
    };
    let draw = |state: &mut State, width: f32, input: Vec<Input>| {
        frame(&ctx, body(false), state, width, input)
    };
    let press = |state: &mut State, pos: Pos2| {
        let mut actions = Vec::new();
        for input in click(pos) {
            actions.extend(draw(state, 300.0, input).0);
        }
        events(actions)
    };
    let top = |texts: &[(String, Rect)], wanted: &str| {
        texts
            .iter()
            .find(|(text, _)| text.contains(wanted))
            .unwrap_or_else(|| panic!("{wanted} in {texts:?}"))
            .1
            .top()
    };
    state.focus = true;
    for _ in 0..3 {
        draw(&mut state, 300.0, Vec::new());
    }
    let (actions, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(actions.is_empty() && !state.focus);
    assert!(!has(&texts, "Message the lead…") && !has(&texts, "12 auth-refactor"));
    // Each watch with what is true of it; one the lead proposed is
    // answered under every word it would tell the lead.
    let order = [
        "shop",
        "2 needs you",
        "Add watch",
        "Nightly check",
        "Every 1 h · Next in 14 min · Last ran 5 min ago",
        "PR #83",
        "Pull request zevem/neptune#83 · Paused",
        "Hourly",
        "Every 15 min · proposed by the lead",
        "Look at the nightly build and say what broke.",
        "Allow",
        "Watches run only while Neptune is open.",
    ];
    for pair in order.windows(2) {
        assert!(top(&texts, pair[0]) <= top(&texts, pair[1]), "{pair:?}");
    }
    assert!(at(&texts, "Add watch").x > nth(&texts, "Watches", 1).x);
    // What a row says is read to its end, and no row carries buttons: what
    // can be done with a watch is behind its menu. What is always heard
    // is said once under the list, not as a row.
    assert!(has(&texts, "Last ran 5 min ago") && !has(&texts, "Agent updates"));
    assert_eq!(
        ["Pause", "Resume", "Run now", "Delete", "Allow", "Decline"]
            .map(|label| count(&texts, label)),
        [0, 0, 0, 0, 1, 1]
    );
    let project = ID;
    let menu = |state: &mut State, name: &str| {
        let (_, texts) = draw(state, 300.0, Vec::new());
        assert!(press(state, row_menu(&texts, name)).is_empty());
        draw(state, 300.0, Vec::new()).1
    };
    let open = menu(&mut state, "Nightly check");
    for wanted in ["Pause", "Run now", "Delete…"] {
        assert_eq!(count(&open, wanted), 1, "{wanted} in {open:?}");
    }
    assert_eq!(
        press(&mut state, at(&open, "Pause")),
        [Event::PauseWatch(project, 1, true)]
    );
    let open = menu(&mut state, "PR #83");
    assert_eq!(
        press(&mut state, at(&open, "Resume")),
        [Event::PauseWatch(project, 2, false)]
    );
    let open = menu(&mut state, "Nightly check");
    assert_eq!(
        press(&mut state, at(&open, "Run now")),
        [Event::RunWatch(project, 1)]
    );
    // A proposed watch is answered, not held back or run.
    assert_eq!(
        press(&mut state, at(&texts, "Allow")),
        [Event::AllowWatch(project, 3, true)]
    );
    assert_eq!(
        press(&mut state, at(&texts, "Decline")),
        [Event::AllowWatch(project, 3, false)]
    );
    // Deleting asks once more, in the row.
    let open = menu(&mut state, "Nightly check");
    assert!(press(&mut state, at(&open, "Delete…")).is_empty());
    assert_eq!(state.removing, Some((project, 1)));
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(has(&texts, "Delete this watch?"));
    assert!(top(&texts, "Delete this watch?") < top(&texts, "PR #83"));
    assert!(press(&mut state, at(&texts, "Cancel")).is_empty());
    assert_eq!(state.removing, None);
    let open = menu(&mut state, "Nightly check");
    press(&mut state, at(&open, "Delete…"));
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert_eq!(
        press(&mut state, at(&texts, "Delete")),
        [Event::DeleteWatch(project, 1)]
    );
    assert_eq!(state.removing, None);

    // The form: a name, how it is set off, and what to do. It opens
    // over the list, where it is in view.
    short.set(true);
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(press(&mut state, at(&texts, "Add watch")).is_empty());
    assert!(state.watch.open);
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(!state.watch.focus, "its first field took the keyboard");
    for wanted in [
        "Name, such as Nightly check",
        "On a schedule",
        "Pull request",
        "Every hour",
        "What should the lead do each time?",
        "Add",
        "Cancel",
    ] {
        assert!(has(&texts, wanted), "{wanted} in {texts:?}");
    }
    assert!(!has(&texts, "Add watch"));
    assert!(top(&texts, "What should the lead do each time?") < top(&texts, "Every 1 h"));
    draw(&mut state, 300.0, vec![Input::Text("Nightly".into())]);
    assert_eq!(state.watch.title, "Nightly");
    // Enter in a one-line field sends nothing and breaks no line.
    let (actions, _) = draw(&mut state, 300.0, key(Key::Enter));
    assert!(actions.is_empty());
    assert_eq!(state.watch.title, "Nightly");
    // Without something to do there is nothing to add.
    assert!(press(&mut state, at(&texts, "Add")).is_empty());
    assert!(state.watch.open);
    press(&mut state, at(&texts, "What should the lead do each time?"));
    draw(
        &mut state,
        300.0,
        vec![Input::Text(" Check the build. ".into())],
    );
    // How often is chosen in words.
    press(&mut state, at(&texts, "Every hour"));
    let (_, list) = draw(&mut state, 300.0, Vec::new());
    for wanted in [
        "Every 15 minutes",
        "Every 30 minutes",
        "Every 4 hours",
        "Every day",
        "Every week",
        "Custom…",
    ] {
        assert_eq!(count(&list, wanted), 1, "{wanted} in {list:?}");
    }
    assert!(press(&mut state, at(&list, "Every day")).is_empty());
    assert_eq!(state.watch.interval, Some(1440));
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert_eq!(
        press(&mut state, at(&texts, "Add")),
        [Event::AddWatch {
            project,
            title: "Nightly".into(),
            trigger: WatchAsk::Every(1440),
            instruction: "Check the build.".into(),
        }]
    );
    assert!(!state.watch.open && state.watch.title.is_empty());
    // A number of the person's own is typed once it is asked for.
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    press(&mut state, at(&texts, "Add watch"));
    draw(&mut state, 300.0, Vec::new());
    draw(&mut state, 300.0, vec![Input::Text("Often".into())]);
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(!has(&texts, "Minutes between runs"));
    press(&mut state, at(&texts, "Every hour"));
    let (_, list) = draw(&mut state, 300.0, Vec::new());
    press(&mut state, at(&list, "Custom…"));
    assert_eq!(state.watch.interval, None);
    draw(&mut state, 300.0, Vec::new());
    draw(&mut state, 300.0, vec![Input::Text("5".into())]);
    assert_eq!(state.watch.every, "5", "the field took the keyboard");
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(has(&texts, "15 minutes or more"));
    draw(&mut state, 300.0, vec![Input::Text("0".into())]);
    assert_eq!(state.watch.every, "50");
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(!has(&texts, "15 minutes or more"));
    press(&mut state, at(&texts, "What should the lead do each time?"));
    draw(&mut state, 300.0, vec![Input::Text("Look.".into())]);
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert_eq!(
        press(&mut state, at(&texts, "Add")),
        [Event::AddWatch {
            project,
            title: "Often".into(),
            trigger: WatchAsk::Every(50),
            instruction: "Look.".into(),
        }]
    );

    // A pull request is followed by its address; what to do is optional.
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    press(&mut state, at(&texts, "Add watch"));
    draw(&mut state, 300.0, Vec::new());
    draw(&mut state, 300.0, vec![Input::Text("Review".into())]);
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    press(&mut state, at(&texts, "Pull request"));
    assert_eq!(state.watch.kind, WatchKind::PullRequest);
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(has(&texts, "when it changes? Optional.") && !has(&texts, "Every hour"));
    press(
        &mut state,
        at(&texts, "https://github.com/owner/repo/pull/12"),
    );
    let url = "https://github.com/zevem/neptune/pull/83";
    // What is not the address of a pull request is said to be none, and
    // the form stays with what was typed.
    draw(
        &mut state,
        300.0,
        vec![Input::Text(url[..url.len() - 2].into())],
    );
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(has(&texts, "Not a pull request address"), "{texts:?}");
    assert!(press(&mut state, at(&texts, "Add")).is_empty());
    assert!(state.watch.open && state.watch.title == "Review");
    // The field is taken up again at the end of what it holds.
    let typed = texts
        .iter()
        .find(|(text, _)| text == &url[..url.len() - 2])
        .unwrap()
        .1;
    press(&mut state, Pos2::new(typed.right() - 0.5, typed.center().y));
    draw(&mut state, 300.0, vec![Input::Text("83".into())]);
    assert_eq!(state.watch.url, url);
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(!has(&texts, "Not a pull request address"));
    assert_eq!(
        press(&mut state, at(&texts, "Add")),
        [Event::AddWatch {
            project,
            title: "Review".into(),
            trigger: WatchAsk::PullRequest(url.into()),
            instruction: String::new(),
        }]
    );
    // Cancel puts the form away with what was typed.
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    press(&mut state, at(&texts, "Add watch"));
    draw(&mut state, 300.0, Vec::new());
    draw(&mut state, 300.0, vec![Input::Text("Never".into())]);
    let (_, texts) = draw(&mut state, 300.0, Vec::new());
    assert!(press(&mut state, at(&texts, "Cancel")).is_empty());
    assert!(!state.watch.open && state.watch.title.is_empty());

    // In the narrowest panel everything stays inside it, with the form
    // open as well.
    state.watch.open = true;
    for _ in 0..2 {
        draw(&mut state, 220.0, Vec::new());
    }
    let (_, texts) = draw(&mut state, 220.0, Vec::new());
    inside(&texts, panel(220.0));
    for wanted in ["Context", "Watches", "Settings", "On a schedule"] {
        assert!(has(&texts, wanted), "{wanted} in {texts:?}");
    }
    // Nothing of a read-only project is added, held back, run or deleted.
    short.set(false);
    state.watch = WatchForm::default();
    let (_, texts) = frame(&ctx, body(true), &mut state, 300.0, Vec::new());
    assert!(!has(&texts, "Add watch") && has(&texts, "Nightly check"));
    // Its lead is off: the list says so, and does not say when a watch
    // would run.
    assert!(has(
        &texts,
        "Watches are off while this project is read-only."
    ));
    assert!(has(&texts, "Read-only"));
    assert!(!has(&texts, "Watches run only while Neptune is open."));
    for input in click(row_menu(&texts, "Nightly check")) {
        frame(&ctx, body(true), &mut state, 300.0, input);
    }
    let (_, texts) = frame(&ctx, body(true), &mut state, 300.0, Vec::new());
    assert!(!has(&texts, "Run now"));
    for input in click(at(&texts, "Allow")) {
        assert!(
            frame(&ctx, body(true), &mut state, 300.0, input)
                .0
                .is_empty()
        );
    }
    // A project without one says what a watch is.
    let none = running(&chat, &members, &[], &[], false, 0);
    let (_, texts) = frame(&ctx, none, &mut state, 300.0, Vec::new());
    assert!(has(&texts, "No watches yet.") && has(&texts, "Add watch"));
}

#[test]
fn the_settings_choose_what_the_lead_and_its_agents_run_with() {
    let ctx = context();
    let mut state = State::default();
    state.show(Segment::Settings);
    let chat = chat();
    let chosen: &'static Settings = Box::leak(Box::new(Settings {
        lead: LeadSettings {
            model: Some("opus".into()),
            effort: None,
        },
        ..Settings::default()
    }));
    let body = |locked: bool| {
        changed(running(&chat, &[], &[], &[], false, 0), |project| {
            project.settings = chosen;
            project.locked = locked;
            project.lead_model = Some("claude-opus-5-5");
        })
    };
    // Room for all of it, so that nothing asked for is scrolled away.
    let bounds = Rect::from_min_max(Pos2::new(WINDOW.right() - 320.0, 0.0), WINDOW.max);
    let show = |state: &mut State, input: Vec<Input>| {
        let (actions, output) = frame_in(&ctx, body(false), state, bounds, input);
        (events(actions), texts(&output))
    };
    let press = |state: &mut State, pos: Pos2| {
        let mut all = Vec::new();
        for input in click(pos) {
            all.extend(show(state, input).0);
        }
        all
    };
    for _ in 0..3 {
        show(&mut state, Vec::new());
    }
    let (none, texts) = show(&mut state, Vec::new());
    assert!(none.is_empty());
    let top = |wanted: &str| {
        texts
            .iter()
            .find(|(text, _)| text == wanted)
            .unwrap_or_else(|| panic!("{wanted} in {texts:?}"))
            .1
            .top()
    };
    // The lead's CLI, which stays, and what is chosen for it; a card for
    // the agents of each CLI; and the project itself.
    let order = [
        "Lead",
        "Runs on",
        "Opus",
        "Default",
        "Used from the lead's next turn.",
        "Claude Code agents",
        "Ultracode",
        "Codex agents",
        "Project",
        "Name",
        "Rename…",
        "Folder",
        "Reveal",
    ];
    for pair in order.windows(2) {
        assert!(top(pair[0]) <= top(pair[1]), "{pair:?}");
    }
    assert!(has(&texts, "“Lead decides” leaves the choice to the lead"));
    // Every setting names its value in words: none is blank. Only
    // Claude Code agents have ultracode, so theirs are three and Codex's two.
    assert_eq!(count(&texts, "Lead decides"), 5);
    assert_eq!(count(&texts, "Ultracode"), 1);
    assert_eq!(count(&texts, "Model"), 3);
    assert!(!has(&texts, "far more tokens") && !has(&texts, "isn't installed"));

    // A list names leaving the choice open first, then what its CLI
    // takes; picking one says so for the whole card.
    assert!(press(&mut state, at(&texts, "Default")).is_empty());
    let (_, open) = show(&mut state, Vec::new());
    for wanted in ["Low", "High"] {
        assert_eq!(count(&open, wanted), 1, "{wanted} in {open:?}");
    }
    assert_eq!(count(&open, "Default"), 2, "the field and the list's first");
    assert_eq!(count(&open, "Ultra"), 0, "a lead has no ultra");
    assert_eq!(count(&open, "Custom…"), 0, "an effort is one of the list");
    assert_eq!(
        press(&mut state, at(&open, "High")),
        [Event::SetLead {
            project: ID,
            model: Some("opus".into()),
            effort: Some("high".into()),
        }]
    );
    // The model that is chosen can be left to the CLI again.
    let (_, texts) = show(&mut state, Vec::new());
    press(&mut state, at(&texts, "Opus"));
    let (_, open) = show(&mut state, Vec::new());
    assert!(has(&open, "Sonnet") && has(&open, "Custom…") && !has(&open, "gpt-6-sol"));
    assert_eq!(
        press(&mut state, last(&open, "Default")),
        [Event::SetLead {
            project: ID,
            model: None,
            effort: None,
        }]
    );
    // Choosing what is in force changes nothing.
    let (_, texts) = show(&mut state, Vec::new());
    press(&mut state, at(&texts, "Opus"));
    let (_, open) = show(&mut state, Vec::new());
    assert!(press(&mut state, last(&open, "Opus")).is_empty());
    // Codex agents can be given its Ultra as their effort,
    let (_, texts) = show(&mut state, Vec::new());
    press(&mut state, nth(&texts, "Lead decides", 4));
    let (_, open) = show(&mut state, Vec::new());
    assert_eq!(
        press(&mut state, at(&open, "Ultra")),
        [Event::SetAgentDefaults {
            project: ID,
            kind: AgentKind::Codex,
            model: None,
            effort: Some("ultra".into()),
            ultracode: None,
        }]
    );
    // and Claude Code agents ultracode, on or off.
    let (_, texts) = show(&mut state, Vec::new());
    press(&mut state, nth(&texts, "Lead decides", 2));
    let (_, open) = show(&mut state, Vec::new());
    assert!(has(&open, "On"));
    assert_eq!(
        press(&mut state, at(&open, "Off")),
        [Event::SetAgentDefaults {
            project: ID,
            kind: AgentKind::Claude,
            model: None,
            effort: None,
            ultracode: Some(false),
        }]
    );

    // A model no list names is typed in the list's place. What a CLI
    // would not take as a name is said and not used; a name is used with
    // Enter.
    let custom = |state: &mut State| {
        let (_, texts) = show(state, Vec::new());
        press(state, nth(&texts, "Lead decides", 0));
        let (_, open) = show(state, Vec::new());
        assert!(press(state, at(&open, "Custom…")).is_empty());
        show(state, Vec::new());
    };
    custom(&mut state);
    let typed = state.custom.clone().expect("the field");
    assert_eq!(
        (typed.owner, typed.setting, typed.text.as_str()),
        (Some(ID), Setting::Claude, "")
    );
    assert!(!typed.focus, "it took the keyboard");
    let (_, texts) = show(&mut state, Vec::new());
    assert!(has(&texts, "Model id"));
    assert_eq!(count(&texts, "Lead decides"), 4, "in place of its list");
    show(&mut state, vec![Input::Text("--model x".into())]);
    let (none, _) = show(&mut state, key(Key::Enter));
    assert!(none.is_empty());
    let (_, texts) = show(&mut state, Vec::new());
    assert!(has(&texts, "That is not a model name this CLI takes."));
    assert_eq!(
        state.custom.as_ref().map(|typed| typed.text.as_str()),
        Some("--model x"),
        "what was typed stays"
    );
    state.custom.as_mut().unwrap().text = " claude-opus-5-5 ".into();
    assert_eq!(
        show(&mut state, key(Key::Enter)).0,
        [Event::SetAgentDefaults {
            project: ID,
            kind: AgentKind::Claude,
            model: Some("claude-opus-5-5".into()),
            effort: None,
            ultracode: None,
        }]
    );
    assert!(state.custom.is_none());
    // A field the keyboard leaves gives the list back, with nothing used.
    custom(&mut state);
    show(&mut state, vec![Input::Text("half".into())]);
    ctx.memory_mut(|memory| memory.surrender_focus(field_ids()[6]));
    let (none, _) = show(&mut state, Vec::new());
    assert!(none.is_empty() && state.custom.is_none());
    let (_, texts) = show(&mut state, Vec::new());
    assert_eq!(count(&texts, "Lead decides"), 5);

    // The project is renamed from here as well, and the folder it works
    // in is shown by the file manager.
    assert_eq!(
        press(&mut state, at(&texts, "Rename…")),
        [Event::Rename(ID)]
    );
    assert_eq!(
        press(&mut state, at(&texts, "Reveal")),
        [Event::RevealDirectory(ID)]
    );
    // The narrowest panel holds every row.
    let narrow = Rect::from_min_max(Pos2::new(WINDOW.right() - 220.0, 0.0), WINDOW.max);
    for _ in 0..2 {
        frame_in(&ctx, body(false), &mut state, narrow, Vec::new());
    }
    let (_, output) = frame_in(&ctx, body(false), &mut state, narrow, Vec::new());
    let texts = self::texts(&output);
    inside(&texts, narrow);
    assert_eq!(count(&texts, "Lead decides"), 5);

    // What costs far more is said while it is chosen, a CLI that is not
    // here is named, and a choice the lead has not taken up yet is said
    // to wait for its next turn.
    let heavy: &'static Settings = Box::leak(Box::new(Settings {
        claude: crate::projects::settings::AgentSettings {
            ultracode: Some(true),
            ..Default::default()
        },
        codex: crate::projects::settings::AgentSettings {
            effort: Some("ultra".into()),
            ..Default::default()
        },
        ..Settings::default()
    }));
    let other = changed(running(&chat, &[], &[], &[], false, 0), |project| {
        project.settings = heavy;
        project.leads = Some(&[AgentKind::Codex]);
        project.pending = true;
    });
    let (_, output) = frame_in(&ctx, other, &mut state, bounds, Vec::new());
    let texts = self::texts(&output);
    for wanted in [
        "Ultracode uses far more tokens.",
        "Ultra uses far more tokens.",
        "Changed. The lead takes this up with its next turn.",
        "On",
        "Ultra",
    ] {
        assert_eq!(count(&texts, wanted), 1, "{wanted} in {texts:?}");
    }
    assert_eq!(count(&texts, "Claude Code isn't installed here."), 2);
    assert!(!has(&texts, "Codex isn't installed here."));

    // A read-only project's settings are shown and take nothing; its
    // folder can still be looked at.
    let locked = |state: &mut State, pos: Pos2| {
        let mut all = Vec::new();
        for input in click(pos) {
            all.extend(frame_in(&ctx, body(true), state, bounds, input).0);
        }
        events(all)
    };
    let (_, output) = frame_in(&ctx, body(true), &mut state, bounds, Vec::new());
    let texts = self::texts(&output);
    assert!(has(&texts, "Read-only"));
    assert!(locked(&mut state, at(&texts, "Opus")).is_empty());
    let (_, output) = frame_in(&ctx, body(true), &mut state, bounds, Vec::new());
    assert!(!has(&self::texts(&output), "Sonnet"));
    assert_eq!(
        locked(&mut state, at(&texts, "Reveal")),
        [Event::RevealDirectory(ID)]
    );
}

#[test]
fn a_new_projects_lead_can_be_given_a_model_and_an_effort_before_it_starts() {
    let ctx = context();
    let mut state = State {
        goal: "Split checkout".into(),
        ..State::default()
    };
    let workspace = WorkspaceId::new(3);
    let form = || empty(Some(BOTH), false, None);
    let show = |state: &mut State, input: Vec<Input>| {
        let (actions, texts) = frame(&ctx, form(), state, 300.0, input);
        (events(actions), texts)
    };
    let press = |state: &mut State, pos: Pos2| {
        let mut all = Vec::new();
        for input in click(pos) {
            all.extend(show(state, input).0);
        }
        all
    };
    // Folded until asked for: the form is the goal and who leads.
    let (_, texts) = show(&mut state, Vec::new());
    assert!(!has(&texts, "Model") && !has(&texts, "Default"));
    assert!(press(&mut state, at(&texts, "Claude Code")).is_empty());
    let (_, texts) = show(&mut state, Vec::new());
    assert_eq!(count(&texts, "Default"), 2);
    press(&mut state, nth(&texts, "Default", 0));
    let (_, open) = show(&mut state, Vec::new());
    assert!(has(&open, "Opus") && has(&open, "Sonnet") && !has(&open, "gpt-6-sol"));
    assert!(press(&mut state, at(&open, "Sonnet")).is_empty());
    assert_eq!(state.lead_settings.model.as_deref(), Some("sonnet"));
    let (_, texts) = show(&mut state, Vec::new());
    press(&mut state, at(&texts, "Default"));
    let (_, open) = show(&mut state, Vec::new());
    press(&mut state, at(&open, "Low"));
    // The line under the field says what was picked,
    let (_, texts) = show(&mut state, Vec::new());
    let line = "Claude Code · Sonnet · Low";
    assert!(has(&texts, line));
    // and the project starts with it.
    assert_eq!(
        press(&mut state, start_control(&texts, line, 300.0)),
        [Event::Create {
            workspace,
            goal: "Split checkout".into(),
            lead: AgentKind::Claude,
            model: Some("sonnet".into()),
            effort: Some("low".into()),
        }]
    );
    // A model no list names is typed here as it is in the settings.
    press(&mut state, at(&texts, "Sonnet"));
    let (_, open) = show(&mut state, Vec::new());
    press(&mut state, at(&open, "Custom…"));
    show(&mut state, Vec::new());
    show(&mut state, vec![Input::Text("claude-opus-5-5".into())]);
    assert!(show(&mut state, key(Key::Enter)).0.is_empty());
    assert_eq!(
        state.lead_settings.model.as_deref(),
        Some("claude-opus-5-5")
    );
    assert!(state.custom.is_none());
    // Another CLI takes other models: what was picked does not carry
    // over to it.
    let (_, texts) = show(&mut state, Vec::new());
    press(&mut state, at(&texts, "Codex"));
    assert_eq!(state.lead, Some(AgentKind::Codex));
    assert_eq!(state.lead_settings, LeadSettings::default());
    let (_, texts) = show(&mut state, Vec::new());
    assert_eq!(count(&texts, "Default"), 2);
    assert_eq!(
        press(&mut state, start_control(&texts, "Codex", 300.0)),
        [Event::Create {
            workspace,
            goal: "Split checkout".into(),
            lead: AgentKind::Codex,
            model: None,
            effort: None,
        }]
    );

    // A panel too short for the form with its options open scrolls it:
    // the field that starts the project stays in view at the top, and the
    // rest is a turn of the wheel away.
    let short = Rect::from_min_size(Pos2::new(600.0, 78.0), vec2(300.0, 150.0));
    let draw = |state: &mut State, input: Vec<Input>| {
        let (_, output) = frame_in(&ctx, form(), state, short, input);
        self::texts(&output)
    };
    for _ in 0..3 {
        draw(&mut state, Vec::new());
    }
    let texts = draw(&mut state, Vec::new());
    assert!(has(&texts, "Split checkout") && !has(&texts, "Effort"));
    for (text, rect) in &texts {
        assert!(short.contains(rect.center()), "{text} at {rect:?}");
    }
    draw(&mut state, vec![Input::PointerMoved(short.center())]);
    draw(
        &mut state,
        vec![Input::MouseWheel {
            phase: egui::TouchPhase::Move,
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -400.0),
            modifiers: Modifiers::NONE,
        }],
    );
    for _ in 0..30 {
        draw(&mut state, Vec::new());
    }
    assert!(has(&draw(&mut state, Vec::new()), "Effort"));
}

#[test]
fn a_watch_the_lead_proposes_is_allowed_or_declined_in_the_chat() {
    let ctx = context();
    let mut state = State::default();
    let mut chat = Transcript::default();
    chat.push(
        Entry::Proposal {
            watch: 3,
            what: "The lead proposes a watch: “Hourly” · every 15 min".into(),
            text: "Look at CI.".into(),
        },
        10,
    );
    chat.push(
        Entry::Event {
            source: Source::Subscription,
            agent: Some(1),
            what: "Watch “Nightly” fired · schedule · due 14:00 UTC".into(),
            text: "Check the build.".into(),
        },
        11,
    );
    let (members, proposed) = (members(), [3]);
    let open: Vec<(PaneId, u64)> = members.iter().map(|item| (item.pane, 1)).collect();
    let body = |waits: bool| {
        changed(running(&chat, &members, &[], &open, false, 0), |project| {
            project.chat.proposed = if waits { &proposed } else { &[] }
        })
    };
    let (_, texts) = frame(&ctx, body(true), &mut state, 300.0, Vec::new());
    for wanted in [
        "The lead proposes a watch",
        "“Hourly” · every 15 min",
        "Look at CI.",
        "Allow",
        "Decline",
        "Watch “Nightly” fired · schedule · due 14:00 UTC",
        "Check the build.",
    ] {
        assert!(has(&texts, wanted), "{wanted} in {texts:?}");
    }
    // What a watch said is numbered by the watch: it is no agent's row,
    // whatever agent has that number.
    assert!(!has(&texts, "Open terminal") && !has(&texts, "Agent 1 Watch"));
    for (label, allow) in [("Allow", true), ("Decline", false)] {
        let mut actions = Vec::new();
        for input in click(at(&texts, label)) {
            actions.extend(frame(&ctx, body(true), &mut state, 300.0, input).0);
        }
        assert_eq!(events(actions), [Event::AllowWatch(ID, 3, allow)]);
    }
    // Answered, the card stays as what was proposed, without a choice.
    let (_, texts) = frame(&ctx, body(false), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "The lead proposes a watch") && !has(&texts, "Allow"));
}

#[test]
fn a_proposed_watch_shows_all_it_would_tell_the_lead_while_it_waits() {
    let ctx = context();
    let mut state = State::default();
    let mut chat = Transcript::default();
    // Longer than a card quotes of anything else.
    let text = format!("Look at CI. {}and then push to main.", "wait ".repeat(120));
    chat.push(
        Entry::Proposal {
            watch: 3,
            what: "The lead proposes a watch: “Hourly” · every 15 min".into(),
            text,
        },
        10,
    );
    let proposed = [3];
    let body = |waits: bool| {
        changed(running(&chat, &[], &[], &[], false, 0), |project| {
            project.chat.proposed = if waits { &proposed } else { &[] }
        })
    };
    // A yes covers every word: none is cut off while the choice is open.
    let (_, texts) = frame(&ctx, body(true), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "and then push to main."), "{texts:?}");
    let (_, texts) = frame(&ctx, body(false), &mut state, 300.0, Vec::new());
    assert!(has(&texts, "Look at CI.") && !has(&texts, "and then push to main."));
}

#[test]
fn files_wait_over_the_field_are_sent_with_or_without_words_and_stay_with_the_message() {
    use crate::projects::transcript::Attachment;
    use crate::ui::explorer;
    let ctx = context();
    let mut state = State::default();
    let file = |path: &str, bytes| Attachment {
        path: path.into(),
        bytes,
    };
    let shot = "Screenshot from 2026-10-06 at 15.27.03 of the whole checkout page.png";
    let files = vec![
        file(&format!("/home/me/Pictures/{shot}"), 245_760),
        file("/tmp/build.log", 900),
        file("/tmp/plan.pdf", 9),
    ];
    state.attached.insert(ID, files.clone());
    let (chat, members) = (chat(), members());
    let body = || running(&chat, &members, &[], &[], false, 0);
    let bounds = panel(300.0).shrink(8.0);
    let (_, texts) = frame(&ctx, body(), &mut state, 300.0, Vec::new());
    // Each has a chip over the field, and a long name is cut to its chip.
    let (columns, each, _) = chat::chip_grid(bounds.width(), files.len());
    assert_eq!(columns, 2);
    let chip = |texts: &[(String, Rect)], name: &str| {
        let (_, rect) = texts
            .iter()
            .find(|(text, _)| text == name)
            .unwrap_or_else(|| panic!("{name} in {texts:?}"));
        *rect
    };
    for name in [shot, "build.log", "plan.pdf"] {
        let name = chip(&texts, name);
        assert!(name.width() <= each - 40.0, "{name:?}");
        assert!(bounds.contains_rect(name), "{name:?}");
    }
    assert!(chip(&texts, "plan.pdf").top() > chip(&texts, shot).bottom());
    assert!(chip(&texts, "Message the lead…").top() > chip(&texts, "plan.pdf").bottom());
    // Files released over the chat are for this message.
    let (project, area) = state.chat_area.unwrap();
    assert!(project == ID && area.contains(chip(&texts, "build.log").center()));

    // A file is taken off with its own control, and opened by its chip.
    let name = chip(&texts, "build.log");
    let cross = Pos2::new(name.left() - 22.0 + each - 11.0, name.center().y);
    let mut actions = Vec::new();
    for events in click(cross) {
        actions.extend(frame(&ctx, body(), &mut state, 300.0, events).0);
    }
    assert!(actions.is_empty());
    assert_eq!(state.attached[&ID], [files[0].clone(), files[2].clone()]);
    let (_, texts) = frame(&ctx, body(), &mut state, 300.0, Vec::new());
    let mut actions = Vec::new();
    for events in click(chip(&texts, "plan.pdf").center()) {
        actions.extend(frame(&ctx, body(), &mut state, 300.0, events).0);
    }
    assert!(matches!(
        actions.as_slice(),
        [Action::Explorer(explorer::Event::Open(path))] if path.to_str() == Some("/tmp/plan.pdf")
    ));

    // The control beside the one that sends asks for more.
    let send = Pos2::new(
        bounds.right() - 17.0,
        panel(300.0).bottom() - 8.0 - HINT - 16.0,
    );
    let mut actions = Vec::new();
    for events in click(send - vec2(28.0, 0.0)) {
        actions.extend(frame(&ctx, body(), &mut state, 300.0, events).0);
    }
    assert_eq!(events(actions), [Event::Attach(ID)]);

    // Files alone are a message: Enter sends them, and so does the control.
    state.focus = true;
    frame(&ctx, body(), &mut state, 300.0, Vec::new());
    let (actions, _) = frame(&ctx, body(), &mut state, 300.0, key(Key::Enter));
    assert_eq!(
        events(actions),
        [Event::Send {
            project: ID,
            text: String::new(),
            attachments: vec![files[0].clone(), files[2].clone()],
        }]
    );
    assert!(!state.attached.contains_key(&ID), "what was sent leaves");
    let (actions, texts) = frame(&ctx, body(), &mut state, 300.0, key(Key::Enter));
    assert!(actions.is_empty() && !has(&texts, "plan.pdf"));
    state.attached.insert(ID, vec![files[1].clone()]);
    frame(
        &ctx,
        body(),
        &mut state,
        300.0,
        vec![Input::Text("why does it fail?".into())],
    );
    let mut actions = Vec::new();
    for events in click(send) {
        actions.extend(frame(&ctx, body(), &mut state, 300.0, events).0);
    }
    assert_eq!(
        events(actions),
        [Event::Send {
            project: ID,
            text: "why does it fail?".into(),
            attachments: vec![files[1].clone()],
        }]
    );

    // In the chat they stay with the message, under its words or alone,
    // and open the same way: also one whose file has gone since.
    let mut sent = Transcript::default();
    sent.push(
        Entry::User {
            text: "why does it fail?".into(),
            attachments: vec![files[1].clone()],
        },
        0,
    );
    sent.push(
        Entry::User {
            text: String::new(),
            attachments: vec![files[0].clone(), files[2].clone()],
        },
        0,
    );
    let body = || running(&sent, &members, &[], &[], false, 0);
    let (_, texts) = frame(&ctx, body(), &mut state, 300.0, Vec::new());
    for name in [shot, "build.log", "plan.pdf"] {
        assert!(bounds.contains_rect(chip(&texts, name)), "{name}");
    }
    assert!(chip(&texts, "build.log").top() > chip(&texts, "why does it fail?").bottom());
    let mut actions = Vec::new();
    for events in click(chip(&texts, shot).center()) {
        actions.extend(frame(&ctx, body(), &mut state, 300.0, events).0);
    }
    assert!(matches!(
        actions.as_slice(),
        [Action::Explorer(explorer::Event::Open(path))] if path.ends_with(shot)
    ));

    // A project that takes nothing takes no files either.
    state.chat_area = None;
    let locked = || changed(body(), |project| project.locked = true);
    let mut actions = Vec::new();
    for events in click(send - vec2(28.0, 0.0)) {
        actions.extend(frame(&ctx, locked(), &mut state, 300.0, events).0);
    }
    assert!(actions.is_empty() && state.chat_area.is_none());
}
