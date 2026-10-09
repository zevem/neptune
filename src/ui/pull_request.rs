//! The pull request tab: one pull request read where the work is, without
//! the browser. Its header says which it is and where it stands; under it
//! "Summary" has what can be done with it, its reviewers, description,
//! checks and comments, "Timeline" what happened to it in order, and "Code"
//! the files it changes. Without a pull request it lists those the
//! terminals of the workspace in view have linked. It draws what the
//! application read on its worker and reports what was asked of it.
use super::changes::{self, DiffBody, File};
use super::explorer::centered_note;
use super::helpers::{
    LinkedPullRequest, elided, focus_ring, galley_at, menu_item, menu_layout, menu_separator,
    padded, place, segmented,
};
use super::{Action, chat, markup};
use crate::{
    icons::{self, Icon},
    platform::links::WebLink,
    runtime::{
        pull_request::{
            Act, Allowed, Check, Choices, Commit, Decision, Detail, Emoji, Entry, Failure, Kind,
            LineComment, MAX_COMMENT, Merge, Method, Outcome, Reaction, Reply, Verdict,
        },
        pull_requests::{Checks, Lookup, State as Standing},
    },
    theme::{self, Palette},
};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Frame, Id, Layout, Margin, Pos2, Rect, Response,
    Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, text::LayoutJob, vec2,
};
use neptune_model::PullRequest;

/// The row that names the pull request, above its title.
pub const HEADER: f32 = 30.0;
/// A check, and a line of the timeline.
pub const ROW: f32 = 26.0;
/// A linked pull request in the list.
pub const LINKED_ROW: f32 = 46.0;
/// The strip of the pull requests open in the tab.
pub const OPEN: f32 = 28.0;
/// The comments shown before the older ones are asked for.
const RECENT: usize = 10;
/// The most drafts kept, one for each pull request written to.
const DRAFTS: usize = 8;

/// The most pull requests open in the tab at once.
pub const MAX_OPEN: usize = 8;

/// The fields of the tab, which give the keyboard back when it leaves view.
pub fn field_ids() -> [Id; 4] {
    [composer_id(), reply_id(), title_id(), description_id()]
}
pub fn reply_id() -> Id {
    Id::new("pull-request-reply")
}
pub fn title_id() -> Id {
    Id::new("pull-request-title-field")
}
pub fn description_id() -> Id {
    Id::new("pull-request-description-field")
}
pub fn composer_id() -> Id {
    Id::new("pull-request-composer")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Segment {
    #[default]
    Summary,
    Timeline,
    Code,
}

/// What is being written to the pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Composer {
    Comment,
    Review,
    /// A comment on the line chosen in a diff, kept for the review.
    Line,
}

/// What of the pull request is being rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editing {
    Title,
    Description,
}

/// A line of a changed file that a comment is being written on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: String,
    pub line: u32,
    pub removed: bool,
}

/// What waits for the person to say it once more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirm {
    Merge(Method),
    AutoMerge(Method),
    Revert,
    Close,
}

pub struct State {
    pub segment: Segment,
    /// Which parts of the summary are unfolded.
    pub description: bool,
    pub checks: bool,
    pub resolved: bool,
    /// Comments older than the recent ones are shown.
    pub older: bool,
    pub newest_first: bool,
    /// The path of the file whose diff is shown.
    pub selected: Option<String>,
    /// The diff's share of the room under the list's heading.
    pub diff_share: f32,
    /// How the person chose to merge; the repository's first way otherwise.
    pub method: Option<Method>,
    pub confirm: Option<Confirm>,
    pub composer: Option<Composer>,
    /// What a review concludes.
    pub verdict: Verdict,
    /// What is being written, by the address of the pull request it is for.
    drafts: Vec<(String, String)>,
    /// The commit whose files the code part shows, in place of all of them.
    pub scope: Option<String>,
    pub editing: Option<Editing>,
    /// The title or description as it is being rewritten.
    pub edit: String,
    /// The review conversation being answered, and the answer.
    pub replying: Option<String>,
    pub reply: String,
    /// The line of a diff a comment is being written on.
    pub line: Option<Target>,
    /// What is being written about that line.
    pub note: String,
    /// Comments on lines that wait for the review, by pull request.
    pending: Vec<(String, LineComment)>,
    /// The labels and people of the repository were asked for.
    pub asked: bool,
    /// The field should take the keyboard; cleared once it has.
    pub focus: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            segment: Segment::default(),
            description: true,
            checks: false,
            resolved: false,
            older: false,
            newest_first: true,
            selected: None,
            diff_share: 0.6,
            method: None,
            confirm: None,
            composer: None,
            verdict: Verdict::Commented,
            drafts: Vec::new(),
            scope: None,
            editing: None,
            edit: String::new(),
            replying: None,
            reply: String::new(),
            line: None,
            note: String::new(),
            pending: Vec::new(),
            asked: false,
            focus: false,
        }
    }
}

impl State {
    /// Another pull request is shown: it starts on its summary, with nothing
    /// of the last one chosen or asked. What was written stays with its own.
    pub fn opened(&mut self) {
        *self = Self {
            diff_share: self.diff_share,
            newest_first: self.newest_first,
            method: self.method,
            drafts: std::mem::take(&mut self.drafts),
            pending: std::mem::take(&mut self.pending),
            ..Self::default()
        };
    }

    /// The comments on lines that wait for the review of this pull request.
    pub fn pending(&self, url: &str) -> impl Iterator<Item = &LineComment> {
        self.pending
            .iter()
            .filter(move |(of, _)| of == url)
            .map(|(_, comment)| comment)
    }

    pub fn add_pending(&mut self, url: &str, comment: LineComment) {
        self.pending.push((url.to_owned(), comment));
    }

    /// Takes the `place`th waiting comment of this pull request back.
    pub fn discard_pending(&mut self, url: &str, place: usize) {
        let found = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, (of, _))| of == url)
            .map(|(index, _)| index)
            .nth(place);
        if let Some(index) = found {
            self.pending.remove(index);
        }
    }

    /// Something asked of the pull request was done: what was written for
    /// it is put away.
    pub fn done(&mut self, url: &str, act: &Act) {
        match act {
            Act::Comment(_) => self.sent(url),
            Act::Review(..) => {
                self.pending.retain(|(of, _)| of != url);
                self.sent(url);
            }
            Act::Reply { .. } => {
                self.replying = None;
                self.reply.clear();
            }
            Act::Title(_) | Act::Description(_) => {
                self.editing = None;
                self.edit.clear();
            }
            _ => {}
        }
    }

    pub fn draft(&self, url: &str) -> &str {
        self.drafts
            .iter()
            .find(|(of, _)| of == url)
            .map_or("", |(_, text)| text)
    }

    pub fn draft_mut(&mut self, url: &str) -> &mut String {
        let place = match self.drafts.iter().position(|(of, _)| of == url) {
            Some(place) => place,
            None => {
                // Room is made by one that says nothing, then by the oldest.
                if self.drafts.len() >= DRAFTS {
                    let empty = self
                        .drafts
                        .iter()
                        .position(|(_, text)| text.trim().is_empty());
                    self.drafts.remove(empty.unwrap_or(0));
                }
                self.drafts.push((url.to_owned(), String::new()));
                self.drafts.len() - 1
            }
        };
        &mut self.drafts[place].1
    }

    /// What was written was posted: its field is put away empty.
    pub fn sent(&mut self, url: &str) {
        self.drafts.retain(|(of, _)| of != url);
        self.composer = None;
        self.verdict = Verdict::Commented;
        self.focus = false;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Show this pull request in the tab.
    Open(PullRequest),
    /// Leave it for the list of those linked.
    Back,
    /// Take it out of the tab.
    Close(PullRequest),
    /// Read what its repository offers: labels and people to ask.
    Choices,
    /// Show the files of one commit, or of all of them.
    Scope(Option<String>),
    Refresh,
    Act(Act),
    DismissProblem,
    /// Show how this file differs.
    Select(String),
    CloseDiff,
    Copy(String),
}

/// Why the last thing asked of the pull request was not done.
pub struct Problem<'a> {
    pub title: &'a str,
    pub said: &'a str,
}

pub enum FileList<'a> {
    Reading,
    Failed(Failure),
    Listed {
        files: &'a [File],
        /// Files beyond those read.
        more: usize,
        diff: Option<(&'a File, &'a DiffBody)>,
    },
}

pub struct Shown<'a> {
    pub link: &'a PullRequest,
    pub detail: &'a Detail,
    pub files: FileList<'a>,
    /// The labels and people of its repository, once they were asked for.
    pub choices: Option<&'a Choices>,
    /// It is being read again because the person asked.
    pub refreshing: bool,
    /// What is being done to it; nothing else is asked meanwhile.
    pub acting: Option<&'a Act>,
    pub problem: Option<Problem<'a>>,
}

pub enum Body<'a> {
    /// No pull request is open: those the workspace in view has linked.
    Linked(&'a [LinkedPullRequest]),
    /// One was opened and has not been read yet.
    Reading(&'a PullRequest),
    Failed(&'a PullRequest, Failure),
    Shown(Shown<'a>),
}

pub struct View<'a> {
    pub body: Body<'a>,
    /// The pull requests open in the tab, each a pill above what is shown.
    pub opened: &'a [PullRequest],
    /// Seconds since 1970, for how long ago things happened.
    pub now: i64,
    /// An input method is composing in the comment field.
    pub composing: bool,
    /// How much of the panel is shown while a toggle slides it.
    pub reveal: f32,
    pub window: Rect,
}

/// "just now", "5 min ago", "3 h ago", "2 d ago", "4 mo ago", "2 y ago".
pub fn ago(seconds: i64) -> String {
    match seconds.max(0) {
        0..=59 => "just now".into(),
        seconds @ 60..=3599 => format!("{} min ago", seconds / 60),
        seconds @ 3600..=86_399 => format!("{} h ago", seconds / 3600),
        seconds @ 86_400..=5_183_999 => format!("{} d ago", seconds / 86_400),
        seconds @ 5_184_000..=31_535_999 => format!("{} mo ago", seconds / 2_592_000),
        seconds => format!("{} y ago", seconds / 31_536_000),
    }
}

/// How long a check ran: "45s", "1m 30s", "1h 5m".
fn duration(seconds: u32) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

fn count(number: usize, one: &str, many: &str) -> String {
    if number == 1 {
        format!("1 {one}")
    } else {
        format!("{number} {many}")
    }
}

/// A pull request's state as its number shows it on a tab: a word, an icon
/// and a colour, so that it never rests on colour alone.
fn standing(state: Option<Standing>, p: Palette) -> (&'static str, Icon, Color32) {
    match state {
        Some(Standing::Open) => ("Open", Icon::PullRequest, p.accent),
        Some(Standing::Draft) => ("Draft", Icon::PullRequest, p.secondary),
        Some(Standing::Merged) => ("Merged", Icon::Merged, p.merged),
        Some(Standing::Closed) => ("Closed", Icon::PullRequest, p.muted),
        None => ("", Icon::PullRequest, p.secondary),
    }
}

/// Where the pull request's host is called by name.
fn on_host(link: &PullRequest) -> String {
    match link.location().0 {
        host if host == "github.com" || host.ends_with(".github.com") => "Open on GitHub".into(),
        host => format!("Open on {host}"),
    }
}

/// The checks in words: "All checks passed", "2 of 9 failing".
pub fn checks_summary(detail: &Detail) -> String {
    let total = detail.checks.len() + detail.more_checks;
    let of = |outcome| {
        detail
            .checks
            .iter()
            .filter(|check| check.outcome == outcome)
            .count()
    };
    let (failed, running, passed) = (
        of(Outcome::Failed),
        of(Outcome::Running),
        of(Outcome::Passed),
    );
    if total == 0 {
        "No checks reported".into()
    } else if failed > 0 {
        format!("{failed} of {total} failing")
    } else if running > 0 {
        format!("{running} of {total} running")
    } else if passed + of(Outcome::Skipped) == total {
        "All checks passed".into()
    } else {
        format!("{passed} of {total} passing")
    }
}

/// How many checks fail or still run, of all of them.
fn checks_stand(detail: &Detail, outcome: Outcome, word: &str) -> String {
    let total = detail.checks.len() + detail.more_checks;
    let some = detail
        .checks
        .iter()
        .filter(|check| check.outcome == outcome)
        .count();
    format!("{some} of {} {word}", count(total, "check", "checks"))
}

/// What kind of news the line about where it stands is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Merged,
    Quiet,
    Good,
    Running,
    Bad,
}

/// Where the pull request stands, in one line: what would keep it from
/// being merged first, then that nothing does.
pub fn stands(detail: &Detail, now: i64) -> (Tone, String) {
    let when = |at: Option<i64>| at.map_or(String::new(), |at| format!(" · {}", ago(now - at)));
    match detail.state {
        Standing::Merged if detail.merged_by.is_empty() => {
            (Tone::Merged, format!("Merged{}", when(detail.ended)))
        }
        Standing::Merged => (
            Tone::Merged,
            format!("Merged by {}{}", detail.merged_by, when(detail.ended)),
        ),
        Standing::Closed => (
            Tone::Quiet,
            format!("Closed without merging{}", when(detail.ended)),
        ),
        Standing::Draft => (Tone::Quiet, "Draft, not ready for review".into()),
        Standing::Open => {
            if detail.merge == Merge::Conflicts {
                (Tone::Bad, format!("Conflicts with {}", detail.base))
            } else if detail.decision == Some(Decision::ChangesRequested) {
                (Tone::Bad, "Changes requested".into())
            } else if detail.checks() == Checks::Failing {
                (Tone::Bad, checks_stand(detail, Outcome::Failed, "failing"))
            } else if detail.checks() == Checks::Pending {
                (
                    Tone::Running,
                    checks_stand(detail, Outcome::Running, "running"),
                )
            } else if detail.decision == Some(Decision::ReviewRequired) {
                (Tone::Quiet, "Review required".into())
            } else if detail.merge == Merge::Behind {
                (Tone::Quiet, format!("Out of date with {}", detail.base))
            } else if detail.merge == Merge::Blocked {
                (Tone::Quiet, "Held by a rule of the repository".into())
            } else if let Some(way) = detail.auto_merge {
                (
                    Tone::Running,
                    format!("Merges by itself when it may · {}", way.label()),
                )
            } else if detail.merge == Merge::Unknown {
                (Tone::Quiet, "Open".into())
            } else {
                (Tone::Good, "Ready to merge".into())
            }
        }
    }
}

fn verdict_words(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Approved => "approved",
        Verdict::ChangesRequested => "requested changes",
        Verdict::Commented => "reviewed",
        Verdict::Dismissed => "review dismissed",
        Verdict::Awaited => "review requested",
    }
}

fn verdict_ink(verdict: Verdict, p: Palette) -> Color32 {
    match verdict {
        Verdict::Approved => p.green,
        Verdict::ChangesRequested => p.red,
        _ => p.secondary,
    }
}

/// A stable number for a name, which chooses its identity colour.
fn seed(name: &str) -> u64 {
    name.bytes().fold(7u64, |seed, byte| {
        seed.wrapping_mul(31).wrapping_add(byte as u64)
    }) % 8
        + 1
}

/// A person, as a disc in their identity colour with their initial. No
/// picture is loaded for anyone.
fn person(painter: &egui::Painter, centre: Pos2, p: Palette, name: &str) {
    let ink = theme::identity_color(seed(name), p.dark);
    painter.circle_filled(centre, 8.0, theme::tint(ink, 0.22));
    let initial = name
        .chars()
        .find(|character| character.is_alphanumeric())
        .map_or('?', |character| character.to_ascii_uppercase());
    painter.text(
        centre + vec2(0.0, 0.5),
        Align2::CENTER_CENTER,
        initial,
        theme::medium(9.5),
        ink,
    );
}

/// A shape for each outcome of a check, as well as its colour.
fn outcome_mark(painter: &egui::Painter, centre: Pos2, p: Palette, outcome: Outcome) {
    let glyph = Rect::from_center_size(centre, Vec2::splat(11.0));
    match outcome {
        Outcome::Passed => icons::paint(painter, glyph, Icon::Check, p.green),
        Outcome::Failed => icons::paint(painter, glyph, Icon::Close, p.red),
        Outcome::Cancelled => icons::paint(painter, glyph, Icon::Close, p.muted),
        Outcome::Skipped => icons::paint(painter, glyph, Icon::Minus, p.muted),
        Outcome::Running => {
            painter.circle_stroke(centre, 3.5, Stroke::new(1.5, p.yellow));
        }
    }
}

fn outcome_words(check: &Check) -> String {
    match (check.outcome, check.seconds) {
        (Outcome::Passed, Some(seconds)) => duration(seconds),
        (Outcome::Passed, None) => "Passed".into(),
        (Outcome::Failed, _) => "Failed".into(),
        (Outcome::Running, _) => "Running".into(),
        (Outcome::Cancelled, _) => "Cancelled".into(),
        (Outcome::Skipped, _) => "Skipped".into(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Fill {
    Plain,
    Accent,
    Red,
    /// Drawn plain, and takes no press.
    Off,
}

fn button_width(ui: &Ui, label: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(label.to_owned(), theme::medium(11.5), Color32::PLACEHOLDER)
        .size()
        .x
        + 18.0
}

/// A compact text button. `name` is its own among the buttons of the tab,
/// so that it keeps its press while what is above it changes.
fn button(ui: &mut Ui, p: Palette, rect: Rect, label: &str, name: &str, fill: Fill) -> Response {
    let id = Id::new(("pull-request-button", name));
    button_with(ui, p, rect, label, name, id, fill)
}

/// The same for one of several buttons that share a name, such as the one
/// under each comment: `id` tells them apart.
fn button_with(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    label: &str,
    name: &str,
    id: Id,
    fill: Fill,
) -> Response {
    let live = fill != Fill::Off && ui.is_enabled();
    let response = ui.interact(rect, id, if live { Sense::click() } else { Sense::hover() });
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, live, name));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let (surface, ink) = match fill {
            Fill::Accent => (p.accent, p.on_accent),
            Fill::Red => (p.red, p.on_red()),
            Fill::Plain => (p.control, p.fg),
            Fill::Off => (p.control, p.muted),
        };
        painter.rect_filled(rect, 6, surface);
        let solid = matches!(fill, Fill::Accent | Fill::Red);
        if live && response.is_pointer_button_down_on() {
            let pressed = if solid {
                Color32::from_black_alpha(46)
            } else {
                p.pressed
            };
            painter.rect_filled(rect, 6, pressed);
        } else if live && response.hovered() {
            let hover = if solid {
                Color32::from_white_alpha(26)
            } else {
                p.hover
            };
            painter.rect_filled(rect, 6, hover);
        }
        if response.has_focus() {
            focus_ring(painter, rect, 6, p);
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            theme::medium(11.5),
            ink,
        );
    }
    if live {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Words that are pressed, such as the order of the comments.
fn text_button(ui: &mut Ui, p: Palette, right_center: Pos2, label: &str, name: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::regular(11.5), p.secondary);
    let rect = Rect::from_min_max(
        Pos2::new(
            right_center.x - galley.size().x - 12.0,
            right_center.y - 10.0,
        ),
        Pos2::new(right_center.x, right_center.y + 10.0),
    );
    let response = ui
        .interact(rect, Id::new(("pull-request-words", name)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
    if response.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, 6, p.pressed);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 6, p.hover);
    }
    if response.has_focus() {
        focus_ring(ui.painter(), rect, 6, p);
    }
    galley_at(
        ui.painter(),
        Pos2::new(rect.left() + 6.0, rect.center().y),
        galley,
    );
    response
}

/// Says "Copied" for a moment where something was copied by a press.
fn copied(ui: &Ui, id: Id) -> bool {
    let now = ui.input(|input| input.time);
    let since = ui
        .data(|data| data.get_temp::<f64>(id))
        .map(|at| now - at)
        .filter(|since| *since < 1.4);
    if let Some(since) = since {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(1.4 - since));
    }
    since.is_some()
}

fn copy(ui: &Ui, id: Id, text: &str, events: &mut Vec<Event>) {
    let now = ui.input(|input| input.time);
    ui.data_mut(|data| data.insert_temp(id, now));
    events.push(Event::Copy(text.to_owned()));
}

fn open(link: &str, actions: &mut Vec<Action>) {
    if let Some(link) = WebLink::new(link) {
        actions.push(Action::OpenLink(link));
    }
}

/// A heading that unfolds what is under it. Returns whether it was pressed.
fn disclosure(ui: &mut Ui, p: Palette, title: &str, open: bool, trailing: &str) -> Response {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 28.0));
    let response = ui
        .interact(rect, Id::new(("pull-request-fold", title)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::selected(WidgetType::CollapsingHeader, true, open, title));
    let surface = rect.shrink2(vec2(0.0, 1.0));
    if response.is_pointer_button_down_on() {
        ui.painter().rect_filled(surface, 7, p.pressed);
    } else if response.hovered() {
        ui.painter().rect_filled(surface, 7, p.hover);
    }
    if response.has_focus() {
        focus_ring(ui.painter(), surface.shrink(2.0), 7, p);
    }
    let painter = ui.painter();
    icons::paint(
        painter,
        Rect::from_center_size(
            Pos2::new(rect.left() + 12.0, rect.center().y),
            Vec2::splat(10.0),
        ),
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        p.secondary,
    );
    let named = galley_at(
        painter,
        Pos2::new(rect.left() + 24.0, rect.center().y),
        painter.layout_no_wrap(title.to_owned(), theme::medium(12.0), p.fg),
    );
    if !trailing.is_empty() {
        let words = elided(
            painter,
            trailing,
            theme::regular(11.5),
            p.muted,
            rect.right() - 8.0 - named.right() - 10.0,
        );
        galley_at(
            painter,
            Pos2::new(rect.right() - 8.0 - words.size().x, rect.center().y + 0.5),
            words,
        );
    }
    response
}

/// A paragraph of plain words, wrapped to the room.
fn words(ui: &mut Ui, text: &str, size: f32, ink: Color32, inset: f32) {
    let galley = ui.painter().layout(
        text.to_owned(),
        theme::regular(size),
        ink,
        (ui.available_width() - inset * 2.0).max(40.0),
    );
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), galley.size().y));
    ui.painter()
        .galley(Pos2::new(rect.left() + inset, rect.top()), galley, ink);
}

/// What someone wrote, with its Markdown drawn. Named, so that its links
/// keep their names while what is above it comes and goes.
fn written(
    ui: &mut Ui,
    p: Palette,
    name: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
    actions: &mut Vec<Action>,
) {
    ui.push_id(name, |ui| {
        markup::show_in(ui, p, text.trim(), markup::Lines::Kept, p.fg, actions);
    });
}

/// The row that names the pull request: where it lives and its number,
/// which open it in the browser, and what is offered for it at the trailing
/// edge. Returns the row.
#[allow(clippy::too_many_arguments)]
fn name_row(
    ui: &mut Ui,
    inner: Rect,
    p: Palette,
    link: &PullRequest,
    state: Option<Standing>,
    trailing: impl FnOnce(&mut Ui),
    events: &mut Vec<Event>,
    actions: &mut Vec<Action>,
) -> Rect {
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER));
    let middle = header.center().y;
    let buttons = place(
        ui,
        header,
        Layout::right_to_left(Align::Center),
        "pull-request-actions",
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            trailing(ui);
            ui.min_rect().left()
        },
    );
    // The way back to the pull requests linked here leads the row.
    let pressed = place(
        ui,
        Rect::from_min_size(header.min, vec2(28.0, HEADER)),
        Layout::left_to_right(Align::Center),
        "pull-request-back",
        |ui| icons::button(ui, Icon::ChevronLeft, "Back to pull requests").clicked(),
    );
    if pressed {
        events.push(Event::Back);
    }
    let left = inner.left() + 28.0;
    let (_, icon, ink) = standing(state, p);
    let (_, owner, repository) = link.location();
    let number =
        ui.painter()
            .layout_no_wrap(format!("#{}", link.number()), theme::medium(12.5), ink);
    let room = buttons - 8.0 - (left + 26.0) - number.size().x - 6.0;
    let place_name = elided(
        ui.painter(),
        &format!("{owner}/{repository}"),
        theme::medium(12.0),
        p.secondary,
        room.max(0.0),
    );
    let named = Rect::from_min_max(
        Pos2::new(left + 4.0, header.top() + 4.0),
        Pos2::new(
            left + 26.0 + place_name.size().x + 6.0 + number.size().x + 6.0,
            header.bottom() - 4.0,
        ),
    );
    let response = ui
        .interact(named, Id::new("pull-request-name"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let label = on_host(link);
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Link,
            true,
            format!("{label}: pull request {}", link.label()),
        )
    });
    if response.is_pointer_button_down_on() {
        ui.painter().rect_filled(named, 7, p.pressed);
    } else if response.hovered() {
        ui.painter().rect_filled(named, 7, p.hover);
    }
    if response.has_focus() {
        focus_ring(ui.painter(), named.shrink(2.0), 7, p);
    }
    let painter = ui.painter();
    icons::paint(
        painter,
        Rect::from_center_size(Pos2::new(left + 14.0, middle), Vec2::splat(14.0)),
        icon,
        ink,
    );
    let placed = galley_at(painter, Pos2::new(left + 26.0, middle), place_name);
    galley_at(painter, Pos2::new(placed.right() + 6.0, middle), number);
    if response.clicked() {
        open(link.url(), actions);
    }
    response.on_hover_text(label);
    header
}

/// The pull requests the terminals of the workspace in view have linked.
fn linked(
    ui: &mut Ui,
    inner: Rect,
    p: Palette,
    links: &[LinkedPullRequest],
    events: &mut Vec<Event>,
    actions: &mut Vec<Action>,
) {
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER));
    ui.painter().text(
        Pos2::new(inner.left() + 6.0, header.center().y),
        Align2::LEFT_CENTER,
        "Pull requests",
        theme::medium(12.5),
        p.fg,
    );
    let below = Rect::from_min_max(Pos2::new(inner.left(), header.bottom()), inner.max);
    if links.is_empty() {
        centered_note(
            ui,
            below,
            p,
            "No pull request is open.\nClick a pull request's number on a terminal's tab to read it here. Agents link the pull requests they create or work on.",
        );
        return;
    }
    let mut list = ui.new_child(
        UiBuilder::new()
            .id_salt("pull-request-linked")
            .max_rect(below),
    );
    list.set_clip_rect(below.intersect(list.clip_rect()));
    egui::ScrollArea::vertical()
        .id_salt("pull-request-linked-rows")
        .auto_shrink([false, false])
        .show(&mut list, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for linked in links {
                let (_, rect) = ui.allocate_space(vec2(ui.available_width(), LINKED_ROW));
                let response = ui
                    .interact(
                        rect,
                        Id::new(("pull-request-linked", linked.link.url())),
                        Sense::click(),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand);
                let stands = match linked.lookup {
                    Lookup::Known(status) => status.describe(),
                    Lookup::Unavailable => "Status unavailable".into(),
                    Lookup::Checking => "Checking…".into(),
                };
                response.widget_info(|| {
                    WidgetInfo::labeled(
                        WidgetType::Button,
                        true,
                        format!("Pull request {}, {stands}", linked.link.label()),
                    )
                });
                let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                let surface = rect.shrink2(vec2(0.0, 1.0));
                if response.is_pointer_button_down_on() {
                    painter.rect_filled(surface, 7, p.pressed);
                } else if response.hovered() {
                    painter.rect_filled(surface, 7, p.hover);
                }
                if response.has_focus() {
                    focus_ring(&painter, surface.shrink(2.0), 7, p);
                }
                let (_, icon, ink) = standing(linked.lookup.status().map(|status| status.state), p);
                let (first, second) = (rect.top() + 15.0, rect.top() + 31.0);
                icons::paint(
                    &painter,
                    Rect::from_center_size(Pos2::new(rect.left() + 13.0, first), Vec2::splat(14.0)),
                    icon,
                    ink,
                );
                let left = rect.left() + 26.0;
                let room = rect.right() - 8.0 - left;
                galley_at(
                    &painter,
                    Pos2::new(left, first),
                    elided(
                        &painter,
                        &linked.link.label(),
                        theme::medium(12.5),
                        p.fg,
                        room,
                    ),
                );
                galley_at(
                    &painter,
                    Pos2::new(left, second),
                    elided(&painter, &stands, theme::regular(11.5), p.secondary, room),
                );
                if response.clicked() {
                    // The same press that opens a link in the browser from
                    // a terminal does here.
                    let outside = ui.input(|input| input.modifiers.ctrl || input.modifiers.mac_cmd);
                    if outside {
                        open(linked.link.url(), actions);
                    } else {
                        events.push(Event::Open(linked.link.clone()));
                    }
                }
            }
        });
}

/// A pull request that could not be read: what happened, what to do about
/// it, and the two ways on.
fn failed(
    ui: &mut Ui,
    body: Rect,
    p: Palette,
    link: &PullRequest,
    failure: Failure,
    events: &mut Vec<Event>,
    actions: &mut Vec<Action>,
) {
    let (title, said) = failure.explain(link.location().0);
    let width = (body.width() - 24.0).max(40.0);
    let heading = ui
        .painter()
        .layout(title.to_owned(), theme::medium(13.0), p.fg, width);
    let reason = ui
        .painter()
        .layout(said, theme::regular(12.0), p.muted, width);
    let again = button_width(ui, "Try again");
    let label = on_host(link);
    let browse = button_width(ui, &label);
    let mut top = body.top() + 28.0;
    let centred = |width: f32| body.center().x - width * 0.5;
    let painter = ui.painter().with_clip_rect(body);
    painter.galley(
        Pos2::new(centred(heading.size().x), top),
        heading.clone(),
        p.fg,
    );
    top += heading.size().y + 6.0;
    painter.galley(
        Pos2::new(centred(reason.size().x), top),
        reason.clone(),
        p.muted,
    );
    top += reason.size().y + 14.0;
    let left = centred(again + 6.0 + browse);
    let first = Rect::from_min_size(Pos2::new(left, top), vec2(again, 24.0));
    if button(ui, p, first, "Try again", "Try again", Fill::Accent).clicked() {
        events.push(Event::Refresh);
    }
    let second = Rect::from_min_size(Pos2::new(first.right() + 6.0, top), vec2(browse, 24.0));
    if button(ui, p, second, &label, &label, Fill::Plain).clicked() {
        open(link.url(), actions);
    }
}

/// What the "more" control offers.
fn more_menu(
    ui: &mut Ui,
    p: Palette,
    shown: &Shown,
    state: &mut State,
    events: &mut Vec<Event>,
    actions: &mut Vec<Action>,
) {
    menu_layout(ui, 236.0);
    let detail = shown.detail;
    let allowed = &detail.allowed;
    let idle = shown.acting.is_none();
    let mut led = false;
    if idle && allowed.update && detail.in_review() {
        led = true;
        if detail.state == Standing::Draft {
            if menu_item(ui, p, Icon::Check, "Ready for review", "", false) {
                events.push(Event::Act(Act::Ready));
                ui.close();
            }
        } else if menu_item(ui, p, Icon::Pencil, "Convert to draft", "", false) {
            events.push(Event::Act(Act::Draft));
            ui.close();
        }
    }
    if idle && allowed.edit {
        led = true;
        if menu_item(ui, p, Icon::Pencil, "Edit title…", "", false) {
            state.editing = Some(Editing::Title);
            state.edit = detail.title.clone();
            state.focus = true;
            ui.close();
        }
        if menu_item(ui, p, Icon::Pencil, "Edit description…", "", false) {
            state.editing = Some(Editing::Description);
            state.edit = detail.body.clone();
            state.segment = Segment::Summary;
            state.focus = true;
            ui.close();
        }
    }
    // What the host does by itself, and what keeps the branch current.
    if idle && detail.state == Standing::Open {
        if let Some(way) = detail.auto_merge.filter(|_| allowed.merge) {
            led = true;
            let label = format!("Disable auto-merge ({})", way.label().to_lowercase());
            if menu_item(ui, p, Icon::Close, &label, "", false) {
                events.push(Event::Act(Act::AutoMerge(None)));
                ui.close();
            }
        } else if allowed.auto_merge
            && let Some(way) = method(detail, state)
        {
            led = true;
            if menu_item(ui, p, Icon::Merged, "Enable auto-merge…", "", false) {
                state.confirm = Some(Confirm::AutoMerge(way));
                state.segment = Segment::Summary;
                ui.close();
            }
        }
        if allowed.update_branch && detail.merge == Merge::Behind {
            led = true;
            if menu_item(ui, p, Icon::Refresh, "Update branch", "", false) {
                events.push(Event::Act(Act::UpdateBranch));
                ui.close();
            }
        }
    }
    if idle && allowed.merge && detail.state == Standing::Merged {
        led = true;
        if menu_item(ui, p, Icon::Swap, "Revert changes…", "", false) {
            state.confirm = Some(Confirm::Revert);
            state.segment = Segment::Summary;
            ui.close();
        }
    }
    // Every way the repository merges, the one in use ticked.
    if idle && allowed.merge && detail.state == Standing::Open && allowed.methods.len() > 1 {
        if led {
            menu_separator(ui, p);
        }
        led = true;
        let chosen = method(detail, state);
        for way in &allowed.methods {
            let icon = if Some(*way) == chosen {
                Icon::Check
            } else {
                Icon::Merged
            };
            if menu_item(ui, p, icon, way.label(), "", false) {
                state.method = Some(*way);
                ui.close();
            }
        }
    }
    if led {
        menu_separator(ui, p);
    }
    if menu_item(ui, p, Icon::ArrowUpRight, &on_host(shown.link), "", false) {
        open(shown.link.url(), actions);
        ui.close();
    }
    for (label, text) in [
        ("Copy link", shown.link.url().to_owned()),
        ("Copy pull request number", shown.link.number().to_string()),
        ("Copy branch name", branch(&detail.head).to_owned()),
        (
            "Copy checkout command",
            format!("gh pr checkout {}", shown.link.url()),
        ),
    ] {
        if menu_item(ui, p, Icon::Copy, label, "", false) {
            events.push(Event::Copy(text));
            ui.close();
        }
    }
    if idle && allowed.update && detail.in_review() {
        menu_separator(ui, p);
        if menu_item(ui, p, Icon::Close, "Close pull request…", "", true) {
            // Asked once more where it stands, on its summary.
            state.confirm = Some(Confirm::Close);
            state.segment = Segment::Summary;
            ui.close();
        }
    } else if idle && allowed.update && detail.state == Standing::Closed {
        menu_separator(ui, p);
        if menu_item(ui, p, Icon::Refresh, "Reopen pull request", "", false) {
            events.push(Event::Act(Act::Reopen));
            ui.close();
        }
    }
}

/// A branch without the owner a fork's is named with.
fn branch(head: &str) -> &str {
    head.rsplit_once(':').map_or(head, |(_, branch)| branch)
}

/// How it would be merged: as the person chose, where the repository still
/// lets it, or the repository's first way.
fn method(detail: &Detail, state: &State) -> Option<Method> {
    let methods = &detail.allowed.methods;
    state
        .method
        .filter(|chosen| methods.contains(chosen))
        .or(methods.first().copied())
}

/// The pull request's title, two lines at most, and under it its state, who
/// opened it and when it last changed. Returns where they end.
fn heading(ui: &mut Ui, inner: Rect, top: f32, p: Palette, detail: &Detail, now: i64) -> f32 {
    let mut job = LayoutJob::simple(
        detail.title.clone(),
        theme::medium(14.0),
        p.fg,
        (inner.width() - 12.0).max(40.0),
    );
    job.wrap.max_rows = 2;
    let title = ui.painter().layout_job(job);
    let titled = Rect::from_min_size(Pos2::new(inner.left() + 6.0, top + 2.0), title.size());
    ui.painter().galley(titled.min, title.clone(), p.fg);
    let response = ui.interact(titled, Id::new("pull-request-title"), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &detail.title));
    if title.elided {
        response.on_hover_text(&detail.title);
    }
    let line = Rect::from_min_size(
        Pos2::new(inner.left() + 6.0, titled.bottom() + 6.0),
        vec2(inner.width() - 12.0, 18.0),
    );
    let (word, icon, ink) = standing(Some(detail.state), p);
    let painter = ui.painter();
    let label = painter.layout_no_wrap(word.to_owned(), theme::medium(11.0), ink);
    let pill = Rect::from_min_size(line.min, vec2(label.size().x + 28.0, 18.0));
    painter.rect_filled(pill, 9, theme::tint(ink, 0.14));
    icons::paint(
        painter,
        Rect::from_center_size(
            Pos2::new(pill.left() + 11.0, pill.center().y),
            Vec2::splat(11.0),
        ),
        icon,
        ink,
    );
    galley_at(
        painter,
        Pos2::new(pill.left() + 20.0, pill.center().y + 0.5),
        label,
    );
    let by = format!("{} · updated {}", detail.author, ago(now - detail.updated));
    galley_at(
        painter,
        Pos2::new(pill.right() + 8.0, line.center().y + 0.5),
        elided(
            painter,
            &by,
            theme::regular(11.5),
            p.secondary,
            line.right() - pill.right() - 8.0,
        ),
    );
    line.bottom()
}

/// A button of the card: its label and fill, what it asks to be done and
/// the question it raises or puts away.
type Offered = (String, Fill, Option<Event>, Option<Option<Confirm>>);

/// Where the pull request stands and the one thing to do about it, or the
/// question that thing asks first.
fn standing_card(
    ui: &mut Ui,
    p: Palette,
    shown: &Shown,
    now: i64,
    state: &mut State,
    events: &mut Vec<Event>,
) {
    let detail = shown.detail;
    let allowed = &detail.allowed;
    let number = shown.link.number();
    let way = method(detail, state);
    // What the trailing buttons are, the confirming one last.
    let mut buttons: Vec<Offered> = Vec::new();
    let (tone, said) = match (shown.acting, state.confirm) {
        (Some(act), _) => (
            Tone::Quiet,
            match act {
                Act::Merge(_) => "Merging…",
                Act::Ready => "Marking ready for review…",
                Act::Draft => "Converting to draft…",
                Act::Close => "Closing…",
                Act::Reopen => "Reopening…",
                Act::Comment(_) => "Posting the comment…",
                Act::Review(..) => "Submitting the review…",
                Act::AutoMerge(Some(_)) => "Turning on auto-merge…",
                Act::AutoMerge(None) => "Turning off auto-merge…",
                Act::UpdateBranch => "Updating the branch…",
                Act::Revert => "Opening a pull request that reverts this…",
                Act::Reply { .. } => "Posting the reply…",
                _ => "Saving…",
            }
            .to_owned(),
        ),
        (None, Some(Confirm::Merge(way))) => {
            buttons.push(("Cancel".into(), Fill::Plain, None, Some(None)));
            buttons.push((
                "Merge".into(),
                Fill::Accent,
                Some(Event::Act(Act::Merge(way))),
                None,
            ));
            (
                Tone::Quiet,
                format!("{} #{number} into {}?", way.label(), detail.base),
            )
        }
        (None, Some(Confirm::AutoMerge(way))) => {
            buttons.push(("Cancel".into(), Fill::Plain, None, Some(None)));
            buttons.push((
                "Enable".into(),
                Fill::Accent,
                Some(Event::Act(Act::AutoMerge(Some(way)))),
                None,
            ));
            (
                Tone::Quiet,
                format!(
                    "Have the host {} #{number} as soon as it may?",
                    way.label().to_lowercase()
                ),
            )
        }
        (None, Some(Confirm::Revert)) => {
            buttons.push(("Cancel".into(), Fill::Plain, None, Some(None)));
            buttons.push((
                "Create revert".into(),
                Fill::Accent,
                Some(Event::Act(Act::Revert)),
                None,
            ));
            (
                Tone::Quiet,
                format!("Open a pull request that reverts #{number}?"),
            )
        }
        (None, Some(Confirm::Close)) => {
            buttons.push(("Cancel".into(), Fill::Plain, None, Some(None)));
            buttons.push((
                "Close".into(),
                Fill::Red,
                Some(Event::Act(Act::Close)),
                None,
            ));
            (Tone::Quiet, format!("Close #{number} without merging it?"))
        }
        (None, None) => {
            match detail.state {
                Standing::Draft if allowed.update => buttons.push((
                    "Ready for review".into(),
                    Fill::Accent,
                    Some(Event::Act(Act::Ready)),
                    None,
                )),
                Standing::Closed if allowed.update => buttons.push((
                    "Reopen".into(),
                    Fill::Plain,
                    Some(Event::Act(Act::Reopen)),
                    None,
                )),
                Standing::Open
                    if detail.merge == Merge::Behind && allowed.update_branch && !allowed.merge =>
                {
                    buttons.push((
                        "Update branch".into(),
                        Fill::Plain,
                        Some(Event::Act(Act::UpdateBranch)),
                        None,
                    ));
                }
                Standing::Open if allowed.merge && detail.merge != Merge::Conflicts => {
                    if detail.merge == Merge::Behind && allowed.update_branch {
                        buttons.push((
                            "Update branch".into(),
                            Fill::Plain,
                            Some(Event::Act(Act::UpdateBranch)),
                            None,
                        ));
                    }
                    if let Some(way) = way {
                        buttons.push((
                            way.label().into(),
                            Fill::Accent,
                            None,
                            Some(Some(Confirm::Merge(way))),
                        ));
                    }
                }
                _ => {}
            }
            stands(detail, now)
        }
    };
    // The buttons share the line with the words while both fit whole.
    let across: f32 = buttons
        .iter()
        .map(|(label, ..)| button_width(ui, label) + 6.0)
        .sum();
    let needed = ui
        .painter()
        .layout_no_wrap(said.clone(), theme::medium(12.0), p.fg)
        .size()
        .x;
    let beside = !buttons.is_empty() && 28.0 + needed + 10.0 + across + 4.0 <= ui.available_width();
    let height = if buttons.is_empty() || beside {
        34.0
    } else {
        66.0
    };
    let (_, card) = ui.allocate_space(vec2(ui.available_width(), height));
    ui.painter()
        .rect_filled(card, theme::metrics::ROW_RADIUS, p.control);
    let middle = card.top() + 17.0;
    let ink = match tone {
        Tone::Merged => p.merged,
        Tone::Quiet => p.secondary,
        Tone::Good => p.green,
        Tone::Running => p.yellow,
        Tone::Bad => p.red,
    };
    let painter = ui.painter();
    let mark = Pos2::new(card.left() + 15.0, middle);
    let glyph = Rect::from_center_size(mark, Vec2::splat(12.0));
    match tone {
        Tone::Merged => icons::paint(painter, glyph, Icon::Merged, ink),
        Tone::Good => icons::paint(painter, glyph, Icon::Check, ink),
        Tone::Bad => icons::paint(painter, glyph, Icon::Warning, ink),
        Tone::Running => {
            painter.circle_stroke(mark, 3.5, Stroke::new(1.5, ink));
        }
        Tone::Quiet => {
            painter.circle_stroke(mark, 3.5, Stroke::new(1.5, p.muted));
        }
    }
    let line = elided(
        painter,
        &said,
        theme::medium(12.0),
        p.fg,
        card.right() - 10.0 - (card.left() + 28.0) - if beside { across } else { 0.0 },
    );
    let cut = line.elided;
    let lined = galley_at(painter, Pos2::new(card.left() + 28.0, middle + 0.5), line);
    let hover = ui.interact(lined, Id::new("pull-request-stands"), Sense::hover());
    hover.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &said));
    if cut {
        hover.on_hover_text(&said);
    }
    let (mut right, top) = if beside {
        (card.right() - 5.0, card.top() + 5.0)
    } else {
        (card.right() - 8.0, card.top() + 34.0)
    };
    for (label, fill, event, confirm) in buttons.into_iter().rev() {
        let width = button_width(ui, &label);
        let place = Rect::from_min_size(Pos2::new(right - width, top), vec2(width, 24.0));
        right = place.left() - 6.0;
        if button(ui, p, place, &label, &label, fill).clicked() {
            if let Some(confirm) = confirm {
                state.confirm = confirm;
            }
            events.extend(event);
        }
    }
}

/// What the host said of something that was not done.
fn problem(ui: &mut Ui, p: Palette, problem: &Problem, events: &mut Vec<Event>) {
    ui.add_space(6.0);
    let width = ui.available_width();
    let said = ui.painter().layout(
        problem.said.to_owned(),
        theme::regular(11.5),
        p.secondary,
        (width - 28.0 - 10.0).max(40.0),
    );
    let (_, card) = ui.allocate_space(vec2(width, 30.0 + said.size().y + 8.0));
    ui.painter()
        .rect_filled(card, theme::metrics::ROW_RADIUS, theme::tint(p.red, 0.10));
    let painter = ui.painter();
    icons::paint(
        painter,
        Rect::from_center_size(
            Pos2::new(card.left() + 15.0, card.top() + 16.0),
            Vec2::splat(12.0),
        ),
        Icon::Warning,
        p.red,
    );
    galley_at(
        painter,
        Pos2::new(card.left() + 28.0, card.top() + 16.5),
        elided(
            painter,
            problem.title,
            theme::medium(12.0),
            p.fg,
            card.width() - 28.0 - 32.0,
        ),
    );
    painter.galley(
        Pos2::new(card.left() + 28.0, card.top() + 30.0),
        said,
        p.secondary,
    );
    let dismissed = place(
        ui,
        Rect::from_min_size(
            Pos2::new(card.right() - 30.0, card.top() + 2.0),
            Vec2::splat(28.0),
        ),
        Layout::left_to_right(Align::Center),
        "pull-request-problem",
        |ui| icons::button(ui, Icon::Close, "Dismiss").clicked(),
    );
    if dismissed {
        events.push(Event::DismissProblem);
    }
}

/// The branch it merges into and the one it comes from, which a press
/// copies, with how much it changes at the trailing edge.
fn branches(ui: &mut Ui, p: Palette, detail: &Detail, events: &mut Vec<Event>) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 30.0));
    let middle = rect.center().y + 1.0;
    let font = egui::FontId::monospace(11.0);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    // The counts take the trailing edge: removed last, added before it.
    let mut right = rect.right() - 6.0;
    for (lines, sign, colour) in [(detail.removed, '−', p.red), (detail.added, '+', p.green)] {
        if lines > 0 {
            let galley =
                painter.layout_no_wrap(format!("{sign}{lines}"), theme::regular(11.0), colour);
            right -= galley.size().x;
            galley_at(&painter, Pos2::new(right, middle), galley);
            right -= 6.0;
        }
    }
    let left = rect.left() + 6.0;
    let room = (right - 4.0 - left).max(0.0);
    let arrow = painter.layout_no_wrap(" ← ".into(), font.clone(), p.muted);
    let base = elided(
        &painter,
        &detail.base,
        font.clone(),
        p.secondary,
        room * 0.4,
    );
    let id = Id::new("pull-request-branch");
    let said = copied(ui, id);
    let head = elided(
        &painter,
        if said { "Copied" } else { &detail.head },
        font,
        p.fg,
        (room - base.size().x - arrow.size().x - 12.0).max(0.0),
    );
    let based = galley_at(&painter, Pos2::new(left, middle), base);
    let arrowed = galley_at(&painter, Pos2::new(based.right(), middle), arrow);
    let chip = Rect::from_min_size(
        Pos2::new(arrowed.right(), middle - 10.0),
        vec2(head.size().x + 12.0, 20.0),
    );
    let response = ui
        .interact(chip, id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!("Copy branch name {}", detail.head),
        )
    });
    painter.rect_filled(
        chip,
        6,
        if response.is_pointer_button_down_on() {
            p.pressed
        } else if response.hovered() {
            p.hover
        } else {
            p.control
        },
    );
    if response.has_focus() {
        focus_ring(&painter, chip, 6, p);
    }
    galley_at(&painter, Pos2::new(chip.left() + 6.0, middle), head);
    if response.clicked() {
        copy(ui, id, branch(&detail.head), events);
    }
    response.on_hover_text(format!(
        "Copy branch name · merges into {} from {}",
        detail.base, detail.head
    ));
}

/// One of the people or labels of a row.
struct Chip {
    text: String,
    /// A person leads with their disc.
    person: bool,
    verdict: Option<Verdict>,
    hint: String,
}

/// A labelled row of chips that wraps to as many lines as they need. With
/// `add`, a control named so ends the row; it is returned for the list it
/// opens.
fn chips(
    ui: &mut Ui,
    p: Palette,
    label: &str,
    items: &[Chip],
    add: Option<&str>,
) -> Option<Response> {
    const LABEL: f32 = 70.0;
    const LINE: f32 = 24.0;
    let width = ui.available_width();
    let painter = ui.painter().clone();
    let sized: Vec<(f32, std::sync::Arc<egui::Galley>)> = items
        .iter()
        .map(|item| {
            let room = width - LABEL - 6.0 - 40.0;
            let text = elided(
                &painter,
                &item.text,
                theme::regular(12.0),
                p.fg,
                room.max(20.0),
            );
            let lead = if item.person { 22.0 } else { 9.0 };
            let trail = if item.verdict.is_some() { 20.0 } else { 9.0 };
            (lead + text.size().x + trail, text)
        })
        .collect();
    // Where each one goes, a line at a time.
    let mut places = Vec::new();
    let (mut x, mut line) = (LABEL, 0usize);
    for (chip, _) in &sized {
        if x > LABEL && x + chip > width - 6.0 {
            x = LABEL;
            line += 1;
        }
        places.push((x, line));
        x += chip + 4.0;
    }
    // A row without anything says so before the control that adds to it.
    if items.is_empty() {
        x += 34.0;
    }
    if add.is_some() && x > LABEL && x + 22.0 > width - 6.0 {
        x = LABEL;
        line += 1;
    }
    let adder = (x, line);
    let (_, rect) = ui.allocate_space(vec2(width, (line + 1) as f32 * LINE + 2.0));
    if items.is_empty() {
        painter.text(
            Pos2::new(rect.left() + LABEL + 2.0, rect.top() + LINE * 0.5 + 1.0),
            Align2::LEFT_CENTER,
            "None",
            theme::regular(12.0),
            p.muted,
        );
    }
    painter.text(
        Pos2::new(rect.left() + 6.0, rect.top() + LINE * 0.5 + 1.0),
        Align2::LEFT_CENTER,
        label,
        theme::regular(11.5),
        p.muted,
    );
    for ((item, (chip, text)), (x, line)) in items.iter().zip(sized).zip(places) {
        let place = Rect::from_min_size(
            Pos2::new(rect.left() + x, rect.top() + line as f32 * LINE + 2.0),
            vec2(chip, 20.0),
        );
        let middle = place.center().y;
        let mut left = place.left() + 9.0;
        if item.person {
            person(
                &painter,
                Pos2::new(place.left() + 9.0, middle),
                p,
                &item.text,
            );
            left = place.left() + 22.0;
        } else {
            painter.rect_filled(place, 10, p.control);
        }
        let named = galley_at(&painter, Pos2::new(left, middle + 0.5), text);
        if let Some(verdict) = item.verdict {
            let centre = Pos2::new(named.right() + 10.0, middle + 0.5);
            let glyph = Rect::from_center_size(centre, Vec2::splat(10.0));
            match verdict {
                Verdict::Approved => icons::paint(&painter, glyph, Icon::Check, p.green),
                Verdict::ChangesRequested => icons::paint(&painter, glyph, Icon::Close, p.red),
                Verdict::Commented => icons::paint(&painter, glyph, Icon::Comment, p.secondary),
                Verdict::Dismissed => icons::paint(&painter, glyph, Icon::Minus, p.muted),
                Verdict::Awaited => {
                    painter.circle_stroke(centre, 3.0, Stroke::new(1.5, p.muted));
                }
            }
        }
        let response = ui.interact(
            place,
            Id::new(("pull-request-chip", label, &item.text)),
            Sense::hover(),
        );
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &item.hint));
        response.on_hover_text(&item.hint);
    }
    let name = add?;
    let place = Rect::from_min_size(
        Pos2::new(
            rect.left() + adder.0,
            rect.top() + adder.1 as f32 * LINE + 2.0,
        ),
        vec2(22.0, 20.0),
    );
    let response = ui
        .interact(place, Id::new(("pull-request-add", label)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    if response.hovered() || open {
        painter.rect_filled(place, 10, p.hover);
    }
    if response.has_focus() {
        focus_ring(&painter, place, 10, p);
    }
    icons::paint(
        &painter,
        Rect::from_center_size(place.center(), Vec2::splat(11.0)),
        Icon::Plus,
        if response.hovered() || open {
            p.fg
        } else {
            p.muted
        },
    );
    if !open {
        return Some(response.on_hover_text(name));
    }
    Some(response)
}

fn check_row(ui: &mut Ui, p: Palette, check: &Check, actions: &mut Vec<Action>) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW));
    let page = WebLink::new(&check.url);
    let response = ui.interact(
        rect,
        Id::new(("pull-request-check", &check.workflow, &check.name)),
        if page.is_some() {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let state = match check.outcome {
        Outcome::Passed => "Passed",
        Outcome::Failed => "Failed",
        Outcome::Running => "Running",
        Outcome::Cancelled => "Cancelled",
        Outcome::Skipped => "Skipped",
    };
    response.widget_info(|| {
        WidgetInfo::labeled(
            if page.is_some() {
                WidgetType::Link
            } else {
                WidgetType::Label
            },
            true,
            format!("Check {}, {state}", check.name),
        )
    });
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let surface = rect.shrink2(vec2(0.0, 1.0));
    if page.is_some() && response.is_pointer_button_down_on() {
        painter.rect_filled(surface, 6, p.pressed);
    } else if page.is_some() && response.hovered() {
        painter.rect_filled(surface, 6, p.hover);
    }
    if response.has_focus() {
        focus_ring(&painter, surface.shrink(2.0), 6, p);
    }
    let middle = rect.center().y;
    outcome_mark(
        &painter,
        Pos2::new(rect.left() + 13.0, middle),
        p,
        check.outcome,
    );
    let trailing = painter.layout_no_wrap(
        outcome_words(check),
        theme::regular(11.0),
        if check.outcome == Outcome::Failed {
            p.red
        } else {
            p.muted
        },
    );
    let right = rect.right() - 8.0 - trailing.size().x;
    galley_at(&painter, Pos2::new(right, middle + 0.5), trailing);
    let left = rect.left() + 26.0;
    let named = galley_at(
        &painter,
        Pos2::new(left, middle),
        elided(
            &painter,
            &check.name,
            theme::regular(12.5),
            p.fg,
            right - 8.0 - left,
        ),
    );
    if !check.workflow.is_empty() && right - 8.0 - named.right() > 36.0 {
        galley_at(
            &painter,
            Pos2::new(named.right() + 7.0, middle + 0.5),
            elided(
                &painter,
                &check.workflow,
                theme::regular(11.0),
                p.muted,
                right - 8.0 - named.right() - 7.0,
            ),
        );
    }
    if let Some(page) = page {
        if response.clicked() {
            actions.push(Action::OpenLink(page));
        }
        response
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(format!("{} · {state} · Open its page", check.name));
    } else {
        response.on_hover_text(format!("{} · {state}", check.name));
    }
}

/// Who said something, what kind of thing it was and how long ago, with the
/// way to it on the host at the trailing edge.
fn said_by(
    ui: &mut Ui,
    p: Palette,
    (author, verb, ink): (&str, &str, Color32),
    at: i64,
    now: i64,
    url: &str,
    actions: &mut Vec<Action>,
) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 22.0));
    let middle = rect.center().y;
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    person(&painter, Pos2::new(rect.left() + 8.0, middle), p, author);
    let mut right = rect.right();
    if let Some(page) = WebLink::new(url) {
        let place = Rect::from_min_size(
            Pos2::new(rect.right() - 22.0, rect.top()),
            Vec2::splat(22.0),
        );
        let response = ui
            .interact(place, Id::new(("pull-request-said", url)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Link, true, format!("Open what {author} wrote"))
        });
        if response.hovered() {
            painter.rect_filled(place.shrink(1.0), 6, p.hover);
        }
        if response.has_focus() {
            focus_ring(&painter, place.shrink(3.0), 6, p);
        }
        icons::paint(
            &painter,
            Rect::from_center_size(place.center(), Vec2::splat(11.0)),
            Icon::ArrowUpRight,
            if response.hovered() { p.fg } else { p.muted },
        );
        if response.clicked() {
            actions.push(Action::OpenLink(page));
        }
        response.on_hover_text("Open in the browser");
        right = place.left() - 2.0;
    }
    let age = painter.layout_no_wrap(ago(now - at), theme::regular(11.0), p.muted);
    let age_left = right - age.size().x;
    galley_at(&painter, Pos2::new(age_left, middle + 0.5), age);
    let left = rect.left() + 22.0;
    let name = elided(
        &painter,
        author,
        theme::medium(12.0),
        p.fg,
        age_left - 8.0 - left,
    );
    let named = galley_at(&painter, Pos2::new(left, middle), name);
    if age_left - 8.0 - named.right() > 30.0 {
        galley_at(
            &painter,
            Pos2::new(named.right() + 5.0, middle + 0.5),
            elided(
                &painter,
                verb,
                theme::regular(11.5),
                ink,
                age_left - 8.0 - named.right() - 5.0,
            ),
        );
    }
}

/// The file and line a review conversation is about, and how it stands.
fn about(ui: &mut Ui, p: Palette, path: &str, line: Option<u32>, tags: &[&str]) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 18.0));
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let mut right = rect.right();
    for tag in tags.iter().rev() {
        let galley = painter.layout_no_wrap((*tag).to_owned(), theme::regular(10.5), p.muted);
        right -= galley.size().x;
        galley_at(&painter, Pos2::new(right, rect.center().y), galley);
        right -= 8.0;
    }
    let place = match line {
        Some(line) => format!("{path}:{line}"),
        None => path.to_owned(),
    };
    // A path is cut at its start: its end is what names the file.
    let mut job = LayoutJob::simple_singleline(place, egui::FontId::monospace(10.5), p.secondary);
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - rect.left()).max(0.0));
    galley_at(
        &painter,
        Pos2::new(rect.left(), rect.center().y),
        painter.layout_job(job),
    );
}

/// What the controls of a comment need: what the person may do, whether
/// something is under way, and where what they ask for goes.
struct Talk<'a> {
    allowed: &'a Allowed,
    /// Nothing is being done to the pull request: its controls take a press.
    idle: bool,
    composing: bool,
    now: i64,
    state: &'a mut State,
    events: &'a mut Vec<Event>,
    actions: &'a mut Vec<Action>,
}

/// A review conversation as its controls address it.
#[derive(Clone, Copy)]
struct Thread<'a> {
    id: &'a str,
    resolved: bool,
    can_reply: bool,
    can_resolve: bool,
}

/// The reactions to something, each pressed to give or take one's own, the
/// control that adds another and, for a review conversation, the controls
/// that answer and resolve it.
fn responses(
    ui: &mut Ui,
    p: Palette,
    subject: &str,
    given: &[Reaction],
    thread: Option<Thread>,
    talk: &mut Talk,
) {
    let react = talk.allowed.react && !subject.is_empty();
    let answers = thread.is_some_and(|thread| thread.can_reply || thread.can_resolve);
    if given.is_empty() && !react && !answers {
        return;
    }
    ui.add_space(6.0);
    let (_, row) = ui.allocate_space(vec2(ui.available_width(), 22.0));
    let fill = if talk.idle { Fill::Plain } else { Fill::Off };
    let mut right = row.right();
    if let Some(thread) = thread {
        if thread.can_resolve {
            let label = if thread.resolved { "Reopen" } else { "Resolve" };
            let width = button_width(ui, label);
            let place = Rect::from_min_size(Pos2::new(right - width, row.top()), vec2(width, 22.0));
            right = place.left() - 4.0;
            let id = Id::new(("pull-request-resolve", thread.id));
            if button_with(ui, p, place, label, label, id, fill).clicked() {
                talk.events.push(Event::Act(Act::Resolve {
                    thread: thread.id.to_owned(),
                    resolved: !thread.resolved,
                }));
            }
        }
        if thread.can_reply {
            let width = button_width(ui, "Reply");
            let place = Rect::from_min_size(Pos2::new(right - width, row.top()), vec2(width, 22.0));
            right = place.left() - 4.0;
            let id = Id::new(("pull-request-answer", thread.id));
            if button_with(ui, p, place, "Reply", "Reply", id, fill).clicked() {
                if talk.state.replying.as_deref() != Some(thread.id) {
                    talk.state.reply.clear();
                }
                talk.state.replying = Some(thread.id.to_owned());
                talk.state.focus = true;
            }
        }
    }
    let live = react && talk.idle;
    let mut left = row.left();
    for reaction in given {
        let text = format!("{} {}", reaction.emoji.word(), reaction.count);
        let ink = if reaction.mine { p.accent } else { p.secondary };
        let galley = ui
            .painter()
            .layout_no_wrap(text.clone(), theme::regular(11.0), ink);
        let pill = Rect::from_min_size(
            Pos2::new(left, row.top() + 1.0),
            vec2(galley.size().x + 14.0, 20.0),
        );
        if pill.right() > right - 4.0 {
            break;
        }
        left = pill.right() + 4.0;
        let response = ui.interact(
            pill,
            Id::new(("pull-request-reaction", subject, reaction.emoji.word())),
            if live { Sense::click() } else { Sense::hover() },
        );
        response
            .widget_info(|| WidgetInfo::selected(WidgetType::Button, live, reaction.mine, &text));
        let painter = ui.painter();
        painter.rect_filled(
            pill,
            10,
            if reaction.mine {
                theme::tint(p.accent, 0.18)
            } else {
                p.pressed
            },
        );
        if live && response.hovered() {
            painter.rect_filled(pill, 10, p.hover);
        }
        if response.has_focus() {
            focus_ring(painter, pill, 10, p);
        }
        galley_at(
            painter,
            Pos2::new(pill.left() + 7.0, pill.center().y + 0.5),
            galley,
        );
        if live && response.clicked() {
            talk.events.push(Event::Act(Act::React {
                subject: subject.to_owned(),
                emoji: reaction.emoji,
                on: !reaction.mine,
            }));
        }
        if live {
            response
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(if reaction.mine {
                    "Take your reaction back"
                } else {
                    "React the same"
                });
        }
    }
    if react && left + 26.0 <= right {
        let place = Rect::from_min_size(Pos2::new(left, row.top() + 1.0), vec2(22.0, 20.0));
        let response = ui
            .interact(
                place,
                Id::new(("pull-request-react", subject)),
                if talk.idle {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        response
            .widget_info(|| WidgetInfo::labeled(WidgetType::Button, talk.idle, "Add a reaction"));
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
        let painter = ui.painter();
        if response.hovered() || open {
            painter.rect_filled(place, 10, p.hover);
        }
        if response.has_focus() {
            focus_ring(painter, place, 10, p);
        }
        icons::paint(
            painter,
            Rect::from_center_size(place.center(), Vec2::splat(11.0)),
            Icon::Plus,
            if response.hovered() || open {
                p.fg
            } else {
                p.muted
            },
        );
        egui::Popup::menu(&response).show(|ui| {
            menu_layout(ui, 170.0);
            for emoji in Emoji::ALL {
                let mine = given
                    .iter()
                    .any(|reaction| reaction.emoji == emoji && reaction.mine);
                let icon = if mine { Icon::Check } else { Icon::Plus };
                if menu_item(ui, p, icon, emoji.word(), "", false) {
                    talk.events.push(Event::Act(Act::React {
                        subject: subject.to_owned(),
                        emoji,
                        on: !mine,
                    }));
                    ui.close();
                }
            }
        });
        if !open {
            response.on_hover_text("Add a reaction");
        }
    }
}

/// A field under something for what is written about it, with the control
/// that puts it away and the one that sends it. Returns whether it was
/// sent and whether it was put away.
fn written_under(
    ui: &mut Ui,
    p: Palette,
    (id, hint, send_label): (Id, &str, &str),
    rows: (usize, usize),
    text: &mut String,
    talk: (&mut bool, bool, bool),
) -> (bool, bool) {
    let (focus, composing, idle) = talk;
    ui.add_space(8.0);
    let height = chat::editor_height(ui, text, ui.available_width(), rows);
    let (_, field) = ui.allocate_space(vec2(ui.available_width(), height));
    let long = text.chars().count() > MAX_COMMENT;
    let ready = idle && !long && !text.trim().is_empty();
    // The platform's command key with Enter sends, as in any comment field.
    let focused = ui.memory(|memory| memory.has_focus(id));
    let mut send = focused
        && !composing
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
    ui.add_enabled_ui(idle, |ui| {
        chat::editor(
            ui,
            p,
            field,
            chat::Editor {
                id,
                text,
                hint,
                label: hint,
                focus,
                composing,
                trailing: 0.0,
                newline: true,
            },
        );
    });
    ui.add_space(6.0);
    let (_, bar) = ui.allocate_space(vec2(ui.available_width(), 24.0));
    let width = button_width(ui, send_label);
    let place = Rect::from_min_size(Pos2::new(bar.right() - width, bar.top()), vec2(width, 24.0));
    let fill = if ready { Fill::Accent } else { Fill::Off };
    send |= button_with(ui, p, place, send_label, send_label, id.with("send"), fill).clicked();
    let width = button_width(ui, "Cancel");
    let place = Rect::from_min_size(
        Pos2::new(place.left() - 6.0 - width, bar.top()),
        vec2(width, 24.0),
    );
    let cancelled = button_with(
        ui,
        p,
        place,
        "Cancel",
        "Cancel",
        id.with("cancel"),
        Fill::Plain,
    )
    .clicked();
    if cancelled {
        ui.memory_mut(|memory| memory.surrender_focus(id));
    }
    (send && ready, cancelled)
}

/// One thing said on the pull request, as a card.
fn entry_card(ui: &mut Ui, p: Palette, entry: &Entry, talk: &mut Talk) {
    Frame::new()
        .fill(p.control)
        .corner_radius(theme::metrics::ROW_RADIUS)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(ui.available_width());
            entry_body(ui, p, entry, talk);
        });
}

fn entry_body(ui: &mut Ui, p: Palette, entry: &Entry, talk: &mut Talk) {
    let (verb, ink) = match &entry.kind {
        Kind::Comment | Kind::Thread { .. } => ("commented", p.secondary),
        Kind::Review(verdict) => (verdict_words(*verdict), verdict_ink(*verdict, p)),
    };
    said_by(
        ui,
        p,
        (&entry.author, verb, ink),
        entry.at,
        talk.now,
        &entry.url,
        talk.actions,
    );
    let mut thread = None;
    if let Kind::Thread {
        id,
        can_reply,
        can_resolve,
        path,
        line,
        resolved,
        outdated,
        ..
    } = &entry.kind
    {
        ui.add_space(3.0);
        let mut tags = Vec::new();
        if *outdated {
            tags.push("Outdated");
        }
        if *resolved {
            tags.push("Resolved");
        }
        about(ui, p, path, *line, &tags);
        thread = Some(Thread {
            id,
            resolved: *resolved,
            can_reply: *can_reply,
            can_resolve: *can_resolve,
        });
    }
    if !entry.body.trim().is_empty() {
        ui.add_space(5.0);
        written(
            ui,
            p,
            ("pull-request-entry", &entry.url, entry.at),
            &entry.body,
            talk.actions,
        );
    }
    if let Kind::Thread { replies, .. } = &entry.kind {
        for (index, reply) in replies.iter().enumerate() {
            reply_under(ui, p, (&entry.url, index), reply, talk);
        }
    }
    responses(ui, p, &entry.id, &entry.reactions, thread, talk);
    if let Some(thread) = thread
        && talk.state.replying.as_deref() == Some(thread.id)
    {
        let mut focus = std::mem::take(&mut talk.state.focus);
        let (send, cancelled) = written_under(
            ui,
            p,
            (reply_id(), "Reply", "Reply"),
            (2, 6),
            &mut talk.state.reply,
            (&mut focus, talk.composing, talk.idle),
        );
        talk.state.focus = focus;
        if send {
            talk.events.push(Event::Act(Act::Reply {
                thread: thread.id.to_owned(),
                body: talk.state.reply.trim().to_owned(),
            }));
        }
        if cancelled {
            talk.state.replying = None;
        }
    }
}

/// A reply, set in under what it answers behind a bar.
fn reply_under(ui: &mut Ui, p: Palette, name: (&str, usize), reply: &Reply, talk: &mut Talk) {
    ui.add_space(8.0);
    let top = ui.cursor().top();
    let left = ui.cursor().left();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(ui.available_width());
            said_by(
                ui,
                p,
                (&reply.author, "replied", p.secondary),
                reply.at,
                talk.now,
                "",
                talk.actions,
            );
            if !reply.body.trim().is_empty() {
                ui.add_space(4.0);
                written(
                    ui,
                    p,
                    ("pull-request-reply", name),
                    &reply.body,
                    talk.actions,
                );
            }
        });
    });
    let bottom = ui.cursor().top();
    ui.painter().line_segment(
        [Pos2::new(left + 1.0, top), Pos2::new(left + 1.0, bottom)],
        Stroke::new(2.0, p.border),
    );
}

/// The order of what is listed, as words that turn it around.
fn order(ui: &mut Ui, p: Palette, right_center: Pos2, state: &mut State) {
    let label = if state.newest_first {
        "Newest first"
    } else {
        "Oldest first"
    };
    if text_button(ui, p, right_center, label, "Change the order")
        .on_hover_text("Show the oldest first, or the newest")
        .clicked()
    {
        state.newest_first = !state.newest_first;
    }
}

/// The list a row's "+" opens: every label or person of the repository,
/// those the pull request has ticked. A press gives or takes one.
fn picker(
    response: &Response,
    p: Palette,
    names: Option<&[String]>,
    chosen: &[&str],
    act: impl Fn(String, bool) -> Act,
    talk: &mut Talk,
) {
    egui::Popup::menu(response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            menu_layout(ui, 220.0);
            let Some(names) = names else {
                // Read once, when the list is first opened.
                if !talk.state.asked {
                    talk.state.asked = true;
                    talk.events.push(Event::Choices);
                }
                ui.add_space(4.0);
                words(ui, "Reading…", 12.0, p.muted, 10.0);
                ui.add_space(4.0);
                return;
            };
            if names.is_empty() {
                ui.add_space(4.0);
                words(ui, "Nothing to choose from.", 12.0, p.muted, 10.0);
                ui.add_space(4.0);
            }
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for name in names {
                        let on = chosen.contains(&name.as_str());
                        let icon = if on { Icon::Check } else { Icon::Plus };
                        if menu_item(ui, p, icon, name, "", false) && talk.idle {
                            talk.events.push(Event::Act(act(name.clone(), !on)));
                            ui.close();
                        }
                    }
                });
        });
}

fn summary(ui: &mut Ui, p: Palette, shown: &Shown, talk: &mut Talk) {
    let detail = shown.detail;
    standing_card(ui, p, shown, talk.now, talk.state, talk.events);
    if let Some(said) = &shown.problem {
        problem(ui, p, said, talk.events);
    }
    ui.add_space(2.0);
    branches(ui, p, detail, talk.events);

    let reviewers: Vec<Chip> = detail
        .reviewers
        .iter()
        .map(|reviewer| Chip {
            text: reviewer.name.clone(),
            person: true,
            verdict: Some(reviewer.verdict),
            hint: format!(
                "{} · {}",
                reviewer.name,
                match reviewer.verdict {
                    Verdict::Approved => "Approved",
                    Verdict::ChangesRequested => "Requested changes",
                    Verdict::Commented => "Commented",
                    Verdict::Dismissed => "Review dismissed",
                    Verdict::Awaited => "Asked to review",
                }
            ),
        })
        .collect();
    let named = |names: &[String], person: bool| -> Vec<Chip> {
        names
            .iter()
            .map(|name| Chip {
                text: name.clone(),
                person,
                verdict: None,
                hint: name.clone(),
            })
            .collect()
    };
    let choices = shown.choices;
    let ask = talk.allowed.request && detail.in_review();
    if !reviewers.is_empty() || ask {
        let add = ask.then_some("Request a review");
        if let Some(response) = chips(ui, p, "Reviewers", &reviewers, add) {
            let awaited: Vec<&str> = detail
                .reviewers
                .iter()
                .filter(|reviewer| reviewer.verdict == Verdict::Awaited)
                .map(|reviewer| reviewer.name.as_str())
                .collect();
            picker(
                &response,
                p,
                choices.map(|choices| choices.people.as_slice()),
                &awaited,
                |name, on| Act::Request { name, on },
                talk,
            );
        }
    }
    if !detail.assignees.is_empty() {
        chips(ui, p, "Assignees", &named(&detail.assignees, true), None);
    }
    if !detail.labels.is_empty() || talk.allowed.label {
        let add = talk.allowed.label.then_some("Change labels");
        if let Some(response) = chips(ui, p, "Labels", &named(&detail.labels, false), add) {
            let has: Vec<&str> = detail.labels.iter().map(String::as_str).collect();
            picker(
                &response,
                p,
                choices.map(|choices| choices.labels.as_slice()),
                &has,
                |name, on| Act::Label { name, on },
                talk,
            );
        }
    }
    ui.add_space(6.0);

    let editing = talk.state.editing == Some(Editing::Description);
    let heading = disclosure(ui, p, "Description", talk.state.description || editing, "");
    let mut edit = false;
    if talk.allowed.edit && !editing {
        let at = Pos2::new(heading.rect.right() - 2.0, heading.rect.center().y);
        edit = text_button(ui, p, at, "Edit", "Edit description").clicked();
    }
    if edit {
        talk.state.editing = Some(Editing::Description);
        talk.state.edit = detail.body.clone();
        talk.state.focus = true;
    } else if heading.clicked() && !editing {
        talk.state.description = !talk.state.description;
    }
    if editing {
        let mut focus = std::mem::take(&mut talk.state.focus);
        let (save, cancelled) = written_under(
            ui,
            p,
            (description_id(), "Describe this pull request", "Save"),
            (6, 16),
            &mut talk.state.edit,
            (&mut focus, talk.composing, talk.idle),
        );
        talk.state.focus = focus;
        if save {
            let body = talk.state.edit.trim().to_owned();
            talk.events.push(Event::Act(Act::Description(body)));
        }
        if cancelled {
            talk.state.editing = None;
        }
        ui.add_space(8.0);
    } else if talk.state.description {
        ui.add_space(2.0);
        if detail.body.trim().is_empty() {
            words(ui, "No description provided.", 12.0, p.muted, 8.0);
        } else {
            padded(ui, 8.0, |ui| {
                written(
                    ui,
                    p,
                    "pull-request-description",
                    &detail.body,
                    talk.actions,
                );
            });
        }
        padded(ui, 8.0, |ui| {
            responses(ui, p, &detail.id, &detail.reactions, None, talk);
        });
        ui.add_space(8.0);
    }

    if disclosure(ui, p, "Checks", talk.state.checks, &checks_summary(detail)).clicked() {
        talk.state.checks = !talk.state.checks;
    }
    if talk.state.checks {
        if detail.checks.is_empty() {
            words(ui, "No checks reported.", 12.0, p.muted, 8.0);
        }
        for check in &detail.checks {
            check_row(ui, p, check, talk.actions);
        }
        if detail.more_checks > 0 {
            words(
                ui,
                &format!(
                    "{} are not listed.",
                    count(detail.more_checks, "more check", "more checks")
                ),
                11.5,
                p.muted,
                8.0,
            );
        }
        ui.add_space(8.0);
    }

    // What was settled is put away; the rest is the conversation.
    let settled = |entry: &&Entry| matches!(entry.kind, Kind::Thread { resolved: true, .. });
    let open: Vec<&Entry> = detail
        .entries
        .iter()
        .filter(|entry| !settled(entry))
        .collect();
    let resolved: Vec<&Entry> = detail.entries.iter().filter(settled).collect();
    let (_, row) = ui.allocate_space(vec2(ui.available_width(), 28.0));
    let label = ui.painter().text(
        Pos2::new(row.left() + 6.0, row.center().y),
        Align2::LEFT_CENTER,
        "Comments",
        theme::medium(12.0),
        p.fg,
    );
    ui.painter().text(
        Pos2::new(label.right() + 6.0, row.center().y + 0.5),
        Align2::LEFT_CENTER,
        open.len().to_string(),
        theme::regular(11.5),
        p.muted,
    );
    if open.len() > 1 {
        order(
            ui,
            p,
            Pos2::new(row.right() - 2.0, row.center().y),
            talk.state,
        );
    }
    if open.is_empty() {
        words(ui, "No comments yet.", 12.0, p.muted, 8.0);
    }
    // The recent ones, in the order asked for.
    let hidden = if talk.state.older {
        0
    } else {
        open.len().saturating_sub(RECENT)
    };
    let mut listed: Vec<&Entry> = open[hidden..].to_vec();
    if talk.state.newest_first {
        listed.reverse();
    }
    for entry in listed {
        entry_card(ui, p, entry, talk);
        ui.add_space(6.0);
    }
    if open.len() > RECENT {
        let label = if talk.state.older {
            format!("Show only the {RECENT} most recent")
        } else {
            format!("Show {}", count(hidden, "older comment", "older comments"))
        };
        let (_, row) = ui.allocate_space(vec2(ui.available_width(), 26.0));
        let width = button_width(ui, &label);
        let place = Rect::from_center_size(row.center(), vec2(width, 22.0));
        if button(ui, p, place, &label, "Older comments", Fill::Plain).clicked() {
            talk.state.older = !talk.state.older;
        }
    }
    if detail.earlier > 0 {
        words(
            ui,
            &format!(
                "{} on the host are not shown here.",
                count(detail.earlier, "earlier comment", "earlier comments")
            ),
            11.5,
            p.muted,
            8.0,
        );
        ui.add_space(4.0);
    }
    if !resolved.is_empty() {
        let title = count(
            resolved.len(),
            "resolved conversation",
            "resolved conversations",
        );
        if disclosure(ui, p, &title, talk.state.resolved, "").clicked() {
            talk.state.resolved = !talk.state.resolved;
        }
        if talk.state.resolved {
            for entry in resolved {
                entry_card(ui, p, entry, talk);
                ui.add_space(6.0);
            }
        }
    }
}

/// Something that happened to the pull request.
enum Moment<'a> {
    Opened,
    Commit(&'a Commit),
    Said(&'a Entry),
    Ended,
}

fn timeline(ui: &mut Ui, p: Palette, shown: &Shown, talk: &mut Talk) {
    let detail = shown.detail;
    let now = talk.now;
    let mut moments: Vec<(i64, Moment)> = vec![(detail.opened, Moment::Opened)];
    moments.extend(
        detail
            .history
            .iter()
            .map(|commit| (commit.at, Moment::Commit(commit))),
    );
    moments.extend(
        detail
            .entries
            .iter()
            .map(|entry| (entry.at, Moment::Said(entry))),
    );
    if let Some(ended) = detail.ended.filter(|_| !detail.in_review()) {
        moments.push((ended, Moment::Ended));
    }
    // Stable, so that what happened in one second keeps the order read.
    moments.sort_by_key(|(at, _)| *at);
    if talk.state.newest_first {
        moments.reverse();
    }

    let (_, row) = ui.allocate_space(vec2(ui.available_width(), 26.0));
    let counted = format!(
        "{} · {}",
        count(detail.entries.len(), "comment", "comments"),
        count(detail.commits as usize, "commit", "commits")
    );
    ui.painter().text(
        Pos2::new(row.left() + 6.0, row.center().y),
        Align2::LEFT_CENTER,
        counted,
        theme::regular(11.5),
        p.secondary,
    );
    order(
        ui,
        p,
        Pos2::new(row.right() - 2.0, row.center().y),
        talk.state,
    );

    const RAIL: f32 = 13.0;
    const INSET: f32 = 30.0;
    let last = moments.len().saturating_sub(1);
    for (index, (at, moment)) in moments.iter().enumerate() {
        let top = ui.cursor().top();
        let left = ui.cursor().left();
        let line = |ui: &mut Ui, words: String, ink: Color32, trailing: String| -> Rect {
            let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW));
            let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
            let age = painter.layout_no_wrap(trailing, theme::regular(11.0), p.muted);
            let right = rect.right() - 6.0 - age.size().x;
            galley_at(&painter, Pos2::new(right, rect.center().y + 0.5), age);
            galley_at(
                &painter,
                Pos2::new(rect.left(), rect.center().y),
                elided(
                    &painter,
                    &words,
                    theme::regular(12.0),
                    ink,
                    right - 8.0 - rect.left(),
                ),
            );
            rect
        };
        let age = ago(now - at);
        let mut mark = (Icon::PullRequest, p.secondary);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(INSET);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.set_width(ui.available_width());
                match moment {
                    Moment::Opened => {
                        mark = (Icon::PullRequest, p.accent);
                        line(
                            ui,
                            format!("{} opened this pull request", detail.author),
                            p.fg,
                            age,
                        );
                    }
                    Moment::Commit(commit) => {
                        mark = (Icon::Branch, p.muted);
                        let headline = if commit.headline.is_empty() {
                            "Untitled commit".to_owned()
                        } else {
                            commit.headline.clone()
                        };
                        let rect = line(
                            ui,
                            headline.clone(),
                            p.secondary,
                            format!("{} · {age}", commit.short),
                        );
                        // A commit leads to the files it changes.
                        let response = ui
                            .interact(
                                rect,
                                Id::new(("pull-request-commit", &commit.oid)),
                                Sense::click(),
                            )
                            .on_hover_cursor(CursorIcon::PointingHand);
                        response.widget_info(|| {
                            WidgetInfo::labeled(
                                WidgetType::Button,
                                true,
                                format!("Show the files of commit {}", commit.short),
                            )
                        });
                        if response.hovered() {
                            ui.painter()
                                .rect_filled(rect.shrink2(vec2(-4.0, 1.0)), 6, p.hover);
                        }
                        if response.clicked() {
                            talk.events.push(Event::Scope(Some(commit.oid.clone())));
                        }
                        response.on_hover_text(format!("{headline}\nShow the files it changes"));
                    }
                    Moment::Ended => {
                        let (word, icon, ink) = standing(Some(detail.state), p);
                        mark = (icon, ink);
                        let who = match (detail.state, detail.merged_by.as_str()) {
                            (Standing::Merged, by) if !by.is_empty() => {
                                format!("{by} merged this pull request")
                            }
                            _ => format!("Pull request {}", word.to_lowercase()),
                        };
                        line(ui, who, p.fg, age);
                    }
                    Moment::Said(entry) => {
                        mark = match entry.kind {
                            Kind::Review(Verdict::Approved) => (Icon::Check, p.green),
                            Kind::Review(Verdict::ChangesRequested) => (Icon::Close, p.red),
                            _ => (Icon::Comment, p.secondary),
                        };
                        ui.add_space(2.0);
                        entry_body(ui, p, entry, talk);
                        ui.add_space(8.0);
                    }
                }
            });
        });
        let bottom = ui.cursor().top();
        // The rail runs through every moment but stops at the ends.
        let centre = Pos2::new(left + RAIL, top + ROW * 0.5);
        let painter = ui.painter();
        let rail = Stroke::new(1.0, p.separator);
        if index > 0 {
            painter.line_segment([Pos2::new(centre.x, top), centre - vec2(0.0, 10.0)], rail);
        }
        if index < last {
            painter.line_segment(
                [centre + vec2(0.0, 10.0), Pos2::new(centre.x, bottom)],
                rail,
            );
        }
        painter.circle_filled(centre, 9.5, p.control);
        icons::paint(
            painter,
            Rect::from_center_size(centre, Vec2::splat(11.0)),
            mark.0,
            mark.1,
        );
    }
    let unread = (detail.commits as usize).saturating_sub(detail.history.len());
    if unread + detail.earlier > 0 {
        ui.add_space(4.0);
        words(
            ui,
            "Its earlier history is on the host and is not shown here.",
            11.5,
            p.muted,
            8.0,
        );
    }
}

fn code(
    ui: &mut Ui,
    body: Rect,
    p: Palette,
    shown: &Shown,
    state: &mut State,
    events: &mut Vec<Event>,
) {
    let detail = shown.detail;
    let line = Rect::from_min_size(body.min, vec2(body.width(), 24.0));
    let below = Rect::from_min_max(line.left_bottom(), body.max);
    // Which commits are shown is chosen at the trailing edge, whatever
    // else the line says.
    let scoped = state
        .scope
        .as_ref()
        .and_then(|oid| detail.history.iter().find(|commit| commit.oid == *oid));
    let label = scoped.map_or("All commits", |commit| commit.short.as_str());
    let scope = text_button(
        ui,
        p,
        Pos2::new(line.right() - 2.0, line.center().y),
        label,
        "Choose the commits shown",
    );
    egui::Popup::menu(&scope).show(|ui| {
        menu_layout(ui, 250.0);
        let all = if scoped.is_none() {
            Icon::Check
        } else {
            Icon::Files
        };
        if menu_item(ui, p, all, "All commits", "", false) {
            events.push(Event::Scope(None));
            ui.close();
        }
        if !detail.history.is_empty() {
            menu_separator(ui, p);
        }
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                for commit in detail.history.iter().rev() {
                    let chosen = scoped.is_some_and(|scoped| scoped.oid == commit.oid);
                    let icon = if chosen { Icon::Check } else { Icon::Branch };
                    let chosen = ui
                        .push_id(&commit.oid, |ui| {
                            menu_item(ui, p, icon, &commit.headline, &commit.short, false)
                        })
                        .inner;
                    if chosen {
                        events.push(Event::Scope(Some(commit.oid.clone())));
                        ui.close();
                    }
                }
            });
    });
    let mut right = scope.rect.left() - 4.0;

    let (files, more, diff) = match &shown.files {
        FileList::Reading => return centered_note(ui, below, p, "Reading the changed files…"),
        FileList::Failed(failure) => {
            let (title, said) = failure.explain(shown.link.location().0);
            return centered_note(ui, below, p, &format!("{title}\n{said}"));
        }
        FileList::Listed { files, more, diff } => (*files, *more, *diff),
    };
    // The file whose diff is shown is marked as viewed from here.
    if let Some((file, _)) = diff.filter(|_| scoped.is_none() && detail.in_review()) {
        let viewed = detail.viewed.contains(&file.path);
        let label = if viewed { "Viewed ✓" } else { "Viewed" };
        let response = text_button(
            ui,
            p,
            Pos2::new(right, line.center().y),
            label,
            "Mark this file as viewed",
        );
        right = response.rect.left() - 4.0;
        if response
            .on_hover_text(if viewed {
                "You marked this file as viewed. Press to take that back."
            } else {
                "Mark this file as viewed on the host"
            })
            .clicked()
            && shown.acting.is_none()
        {
            events.push(Event::Act(Act::Viewed {
                path: file.path.clone(),
                on: !viewed,
            }));
        }
    }
    let total = files.len() + more;
    let seen = files
        .iter()
        .filter(|file| detail.viewed.contains(&file.path))
        .count();
    let mut counted = match total {
        0 => "No file changes".to_owned(),
        1 => "1 file changed".to_owned(),
        total => format!("{total} files changed"),
    };
    if seen > 0 && scoped.is_none() {
        counted.push_str(&format!(" · {seen} viewed"));
    }
    galley_at(
        ui.painter(),
        Pos2::new(line.left() + 6.0, line.center().y),
        elided(
            ui.painter(),
            &counted,
            theme::regular(11.5),
            p.secondary,
            (right - line.left() - 10.0).max(0.0),
        ),
    );
    if files.is_empty() {
        let note = if scoped.is_some() {
            "This commit has no file changes."
        } else {
            "This pull request has no file changes."
        };
        return centered_note(ui, below, p, note);
    }
    // Lines of the diff in view that something is said of: a conversation
    // on the host, or a comment that waits for the review.
    let url = shown.link.url();
    let mut marked: Vec<(u32, bool)> = Vec::new();
    if let Some((file, _)) = diff {
        marked.extend(
            state
                .pending(url)
                .filter(|comment| comment.path == file.path)
                .map(|comment| (comment.line, comment.removed)),
        );
        marked.extend(detail.entries.iter().filter_map(|entry| match &entry.kind {
            Kind::Thread {
                path,
                line: Some(line),
                resolved: false,
                ..
            } if *path == file.path => Some((*line, false)),
            _ => None,
        }));
    }
    // A comment is on the whole change: one commit's lines take none.
    let pick = std::cell::Cell::new(None);
    let mut chosen = Vec::new();
    changes::listing(
        ui,
        below,
        p,
        changes::Listed {
            files,
            more,
            diff,
            lines: changes::Lines {
                marked: &marked,
                pick: scoped.is_none().then_some(&pick),
            },
        },
        (state.selected.as_deref(), &mut state.diff_share),
        &mut chosen,
    );
    events.extend(chosen.into_iter().filter_map(|event| match event {
        changes::Event::Select(path) => Some(Event::Select(path)),
        changes::Event::CloseDiff => Some(Event::CloseDiff),
        _ => None,
    }));
    if let (Some((line, removed)), Some((file, _))) = (pick.get(), diff) {
        state.line = Some(Target {
            path: file.path.clone(),
            line,
            removed,
        });
        state.composer = Some(Composer::Line);
        state.focus = true;
    }
}

/// How tall the field for a comment is in a tab of `width`.
fn composer_height(ui: &Ui, text: &str, width: f32, waiting: usize) -> f32 {
    34.0 + waiting as f32 * 22.0 + chat::editor_height(ui, text, width, (3, 8)) + 6.0 + 26.0 + 6.0
}

/// The field for a comment or a review, at the bottom of the tab.
fn composer(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    shown: &Shown,
    view: &View,
    state: &mut State,
    events: &mut Vec<Event>,
) {
    let Some(mut kind) = state.composer else {
        return;
    };
    let judge = shown.detail.allowed.judge;
    let url = shown.link.url();
    ui.painter()
        .line_segment([rect.left_top(), rect.right_top()], p.hairline());
    let top = Rect::from_min_size(
        Pos2::new(rect.left(), rect.top() + 6.0),
        vec2(rect.width(), 28.0),
    );
    let closed = place(
        ui,
        Rect::from_min_size(Pos2::new(top.right() - 28.0, top.top()), Vec2::splat(28.0)),
        Layout::left_to_right(Align::Center),
        "pull-request-composer-close",
        |ui| icons::button(ui, Icon::Close, "Put the comment away").clicked(),
    );
    let waiting = state.pending(url).count();
    if let (Composer::Line, Some(target)) = (kind, &state.line) {
        // The line it is about names the field.
        let name = target.path.rsplit('/').next().unwrap_or(&target.path);
        galley_at(
            ui.painter(),
            Pos2::new(top.left() + 6.0, top.center().y),
            elided(
                ui.painter(),
                &format!("Comment on {name}:{}", target.line),
                theme::medium(12.0),
                p.fg,
                top.width() - 40.0,
            ),
        );
    } else {
        if kind == Composer::Line {
            kind = Composer::Comment;
        }
        let review = if waiting > 0 {
            format!("Review ({waiting})")
        } else {
            "Review".to_owned()
        };
        let segments = Rect::from_min_size(top.min, vec2((top.width() - 34.0).min(200.0), 28.0));
        place(
            ui,
            segments,
            Layout::top_down(Align::Min),
            "pull-request-composer-kind",
            |ui| {
                segmented(
                    ui,
                    p,
                    "pull-request-composer",
                    &mut kind,
                    &[(Composer::Comment, "Comment"), (Composer::Review, &review)],
                    segments.width(),
                )
            },
        );
    }
    state.composer = Some(kind);
    let idle = shown.acting.is_none();
    // Only someone else's pull request is approved or sent back.
    let verdict = if kind == Composer::Review && judge {
        state.verdict
    } else {
        Verdict::Commented
    };
    // What waits for the review is listed over its field, each with the
    // control that takes it back.
    let mut field_top = top.bottom() + 6.0;
    if kind == Composer::Review {
        let mut discard = None;
        for (index, comment) in state.pending(url).enumerate() {
            let row =
                Rect::from_min_size(Pos2::new(rect.left(), field_top), vec2(rect.width(), 22.0));
            field_top = row.bottom();
            let name = comment.path.rsplit('/').next().unwrap_or(&comment.path);
            let painter = ui.painter().with_clip_rect(row);
            let place_of = painter.layout_no_wrap(
                format!("{name}:{}", comment.line),
                egui::FontId::monospace(10.5),
                p.secondary,
            );
            let named = galley_at(
                &painter,
                Pos2::new(row.left() + 6.0, row.center().y),
                place_of,
            );
            galley_at(
                &painter,
                Pos2::new(named.right() + 8.0, row.center().y),
                elided(
                    &painter,
                    comment.body.lines().next().unwrap_or_default(),
                    theme::regular(11.5),
                    p.fg,
                    row.right() - 26.0 - named.right() - 8.0,
                ),
            );
            let cross =
                Rect::from_min_size(Pos2::new(row.right() - 22.0, row.top()), Vec2::splat(22.0));
            let response = ui
                .interact(
                    cross,
                    Id::new(("pull-request-pending", index)),
                    Sense::click(),
                )
                .on_hover_cursor(CursorIcon::PointingHand);
            response.widget_info(|| {
                WidgetInfo::labeled(WidgetType::Button, true, "Take this comment back")
            });
            if response.hovered() {
                ui.painter().rect_filled(cross.shrink(1.0), 6, p.hover);
            }
            icons::paint(
                ui.painter(),
                Rect::from_center_size(cross.center(), Vec2::splat(10.0)),
                Icon::Close,
                if response.hovered() { p.fg } else { p.muted },
            );
            if response.on_hover_text("Take this comment back").clicked() {
                discard = Some(index);
            }
        }
        if let Some(index) = discard {
            state.discard_pending(url, index);
        }
    }
    let on_line = kind == Composer::Line;
    let text_of = |state: &State| -> String {
        if on_line {
            state.note.clone()
        } else {
            state.draft(url).to_owned()
        }
    };
    let written = text_of(state);
    let long = written.chars().count() > MAX_COMMENT;
    // An approval and a review of single lines need no words of their own.
    let said = !written.trim().is_empty();
    let ready = idle
        && !long
        && (said || (kind == Composer::Review && (verdict == Verdict::Approved || waiting > 0)));
    // The platform's command key with Enter sends, as in any comment field.
    let focused = ui.memory(|memory| memory.has_focus(composer_id()));
    let mut send = focused
        && !view.composing
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
    let field = Rect::from_min_max(
        Pos2::new(rect.left(), field_top),
        Pos2::new(rect.right(), rect.bottom() - 32.0 - 6.0),
    );
    let mut focus = std::mem::take(&mut state.focus);
    ui.add_enabled_ui(idle, |ui| {
        chat::editor(
            ui,
            p,
            field,
            chat::Editor {
                id: composer_id(),
                text: if on_line {
                    &mut state.note
                } else {
                    state.draft_mut(url)
                },
                hint: match kind {
                    Composer::Review => "Summarize your review",
                    Composer::Line => "Say something about this line",
                    Composer::Comment => "Leave a comment",
                },
                label: match kind {
                    Composer::Review => "Review",
                    Composer::Line => "Comment on a line",
                    Composer::Comment => "Comment",
                },
                focus: &mut focus,
                composing: view.composing,
                trailing: 0.0,
                newline: true,
            },
        );
    });
    state.focus = focus;
    let bar = Rect::from_min_max(
        Pos2::new(rect.left(), field.bottom() + 6.0),
        Pos2::new(rect.right(), rect.bottom() - 6.0),
    );
    let label = match (shown.acting, kind) {
        (Some(Act::Comment(_)), _) => "Posting…",
        (Some(Act::Review(..)), _) => "Submitting…",
        (_, Composer::Comment) => "Comment",
        (_, Composer::Review) => "Submit review",
        (_, Composer::Line) => "Add to review",
    };
    let width = button_width(ui, label);
    let submit = Rect::from_min_size(Pos2::new(bar.right() - width, bar.top()), vec2(width, 26.0));
    let fill = if ready { Fill::Accent } else { Fill::Off };
    let pressed = button(ui, p, submit, label, "Send what was written", fill);
    send |= pressed.clicked();
    if long {
        pressed.on_hover_text(
            "This is too long to send from here. Shorten it, or write it on the host.",
        );
    } else if on_line {
        pressed.on_hover_text("It is sent when you submit the review");
    } else if ready {
        pressed.on_hover_text(format!("{label} · {}", super::helpers::shortcut("Enter")));
    }
    if kind == Composer::Review && judge {
        let name = match state.verdict {
            Verdict::Approved => "Approve",
            Verdict::ChangesRequested => "Request changes",
            _ => "Comment",
        };
        let width = button_width(ui, name) + 12.0;
        let place = Rect::from_min_size(
            bar.min,
            vec2(width.min(submit.left() - 6.0 - bar.left()), 26.0),
        );
        let response = button(ui, p, place, "", "What the review concludes", Fill::Plain);
        let painter = ui.painter().with_clip_rect(place);
        galley_at(
            &painter,
            Pos2::new(place.left() + 9.0, place.center().y),
            elided(
                &painter,
                name,
                theme::medium(11.5),
                p.fg,
                place.width() - 26.0,
            ),
        );
        icons::paint(
            &painter,
            Rect::from_center_size(
                Pos2::new(place.right() - 10.0, place.center().y + 0.5),
                Vec2::splat(10.0),
            ),
            Icon::ChevronDown,
            p.secondary,
        );
        egui::Popup::menu(&response).show(|ui| {
            menu_layout(ui, 190.0);
            for (verdict, icon, label) in [
                (Verdict::Commented, Icon::Comment, "Comment"),
                (Verdict::Approved, Icon::Check, "Approve"),
                (Verdict::ChangesRequested, Icon::Close, "Request changes"),
            ] {
                if menu_item(ui, p, icon, label, "", false) {
                    state.verdict = verdict;
                    ui.close();
                }
            }
        });
    }
    if send && ready {
        let body = written.trim().to_owned();
        match (kind, state.line.take()) {
            (Composer::Line, Some(target)) => {
                state.add_pending(
                    url,
                    LineComment {
                        path: target.path,
                        line: target.line,
                        removed: target.removed,
                        body,
                    },
                );
                state.note.clear();
                // What was added is seen where it will be sent from.
                state.composer = Some(Composer::Review);
            }
            (Composer::Review, _) => {
                let lines = state.pending(url).cloned().collect();
                events.push(Event::Act(Act::Review(verdict, body, lines)));
            }
            _ => events.push(Event::Act(Act::Comment(body))),
        }
    }
    if closed {
        state.composer = None;
        state.line = None;
        ui.memory_mut(|memory| memory.surrender_focus(composer_id()));
    }
}

/// The control that opens the field for a comment, over the trailing
/// corner of what is read. It counts the comments that wait for a review.
fn compose_button(ui: &mut Ui, body: Rect, p: Palette, waiting: usize, state: &mut State) {
    let place = Rect::from_min_size(
        Pos2::new(body.right() - 40.0, body.bottom() - 40.0),
        Vec2::splat(32.0),
    );
    let response = ui
        .interact(place, Id::new("pull-request-compose"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Comment or review"));
    let painter = ui.painter();
    painter.add(p.popup_shadow().as_shape(place, 16));
    painter.rect_filled(place, 16, p.elevated);
    if response.is_pointer_button_down_on() {
        painter.rect_filled(place, 16, p.pressed);
    } else if response.hovered() {
        painter.rect_filled(place, 16, p.hover);
    }
    painter.rect_stroke(place, 16, Stroke::new(1.0, p.border), StrokeKind::Inside);
    if response.has_focus() {
        focus_ring(painter, place, 16, p);
    }
    icons::paint(
        painter,
        Rect::from_center_size(place.center(), Vec2::splat(15.0)),
        Icon::Comment,
        p.fg,
    );
    if waiting > 0 {
        let centre = place.right_top() + vec2(-4.0, 4.0);
        painter.circle_filled(centre, 8.0, p.accent);
        painter.text(
            centre + vec2(0.0, 0.5),
            Align2::CENTER_CENTER,
            waiting.min(99).to_string(),
            theme::medium(9.5),
            p.on_accent,
        );
    }
    if response.clicked() {
        state.composer = Some(if waiting > 0 {
            Composer::Review
        } else {
            Composer::Comment
        });
        state.focus = true;
    }
    response.on_hover_text(match waiting {
        0 => "Comment or review".to_owned(),
        waiting => format!(
            "Comment or review · {} for the review",
            count(waiting, "comment waits", "comments wait")
        ),
    });
}

/// The title as a field, with the controls that save it and put it away.
/// Returns where it ends.
fn title_field(
    ui: &mut Ui,
    inner: Rect,
    top: f32,
    p: Palette,
    shown: &Shown,
    state: &mut State,
    events: &mut Vec<Event>,
) -> f32 {
    let idle = shown.acting.is_none();
    let field = Rect::from_min_size(
        Pos2::new(inner.left(), top + 2.0),
        vec2(inner.width(), theme::metrics::CONTROL_HEIGHT),
    );
    let ready = idle && !state.edit.trim().is_empty() && state.edit.chars().count() <= 256;
    let focused = ui.memory(|memory| memory.has_focus(title_id()));
    let mut save =
        focused && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let response = place(
        ui,
        field,
        Layout::top_down(Align::Min),
        "pull-request-title-edit",
        |ui| {
            ui.add_enabled_ui(idle, |ui| {
                super::helpers::text_field(
                    ui,
                    p,
                    title_id(),
                    &mut state.edit,
                    "Title",
                    "Pull request title",
                    field.width(),
                )
            })
            .inner
        },
    );
    if std::mem::take(&mut state.focus) {
        response.request_focus();
    }
    let bar = Rect::from_min_size(
        Pos2::new(inner.left(), field.bottom() + 6.0),
        vec2(inner.width(), 24.0),
    );
    let width = button_width(ui, "Save");
    let place_of =
        Rect::from_min_size(Pos2::new(bar.right() - width, bar.top()), vec2(width, 24.0));
    let fill = if ready { Fill::Accent } else { Fill::Off };
    save |= button(ui, p, place_of, "Save", "Save the title", fill).clicked();
    let width = button_width(ui, "Cancel");
    let place_of = Rect::from_min_size(
        Pos2::new(place_of.left() - 6.0 - width, bar.top()),
        vec2(width, 24.0),
    );
    if button(ui, p, place_of, "Cancel", "Keep the title", Fill::Plain).clicked() {
        state.editing = None;
        ui.memory_mut(|memory| memory.surrender_focus(title_id()));
    }
    if save && ready {
        events.push(Event::Act(Act::Title(state.edit.trim().to_owned())));
    }
    bar.bottom() - 4.0
}

#[allow(clippy::too_many_arguments)]
fn pull_request(
    ui: &mut Ui,
    inner: Rect,
    p: Palette,
    shown: &Shown,
    view: &View,
    state: &mut State,
    events: &mut Vec<Event>,
    actions: &mut Vec<Action>,
) {
    let detail = shown.detail;
    let mut menu_events = Vec::new();
    let mut menu_actions = Vec::new();
    let header = name_row(
        ui,
        inner,
        p,
        shown.link,
        Some(detail.state),
        |ui| {
            let more = icons::button(ui, Icon::Ellipsis, "More pull request actions");
            egui::Popup::menu(&more).show(|ui| {
                more_menu(ui, p, shown, state, &mut menu_events, &mut menu_actions);
            });
            ui.add_enabled_ui(!shown.refreshing, |ui| {
                let label = if shown.refreshing {
                    "Refreshing pull request"
                } else {
                    "Refresh pull request"
                };
                if icons::button(ui, Icon::Refresh, label).clicked() {
                    menu_events.push(Event::Refresh);
                }
            });
        },
        events,
        actions,
    );
    events.append(&mut menu_events);
    actions.append(&mut menu_actions);
    let bottom = if state.editing == Some(Editing::Title) {
        title_field(ui, inner, header.bottom(), p, shown, state, events)
    } else {
        heading(ui, inner, header.bottom(), p, detail, view.now)
    };
    let segments = Rect::from_min_size(
        Pos2::new(inner.left(), bottom + 10.0),
        vec2(inner.width(), 28.0),
    );
    place(
        ui,
        segments,
        Layout::top_down(Align::Min),
        "pull-request-segments",
        |ui| {
            segmented(
                ui,
                p,
                "pull-request-segment",
                &mut state.segment,
                &[
                    (Segment::Summary, "Summary"),
                    (Segment::Timeline, "Timeline"),
                    (Segment::Code, "Code"),
                ],
                segments.width(),
            )
        },
    );
    let mut body = Rect::from_min_max(Pos2::new(inner.left(), segments.bottom() + 6.0), inner.max);
    if body.height() < 8.0 {
        return;
    }
    let url = shown.link.url();
    let waiting = state.pending(url).count();
    if state.composer.is_some() {
        let text = if state.composer == Some(Composer::Line) {
            state.note.clone()
        } else {
            state.draft(url).to_owned()
        };
        let listed = if state.composer == Some(Composer::Review) {
            waiting
        } else {
            0
        };
        let height =
            composer_height(ui, &text, body.width(), listed).min((body.height() - 60.0).max(120.0));
        let field = Rect::from_min_max(Pos2::new(body.left(), body.bottom() - height), body.max);
        body.max.y = field.top() - 6.0;
        composer(ui, field, p, shown, view, state, events);
    }
    if state.segment == Segment::Code {
        code(ui, body, p, shown, state, events);
        return;
    }
    let mut content = ui.new_child(
        UiBuilder::new()
            .id_salt(("pull-request-body", state.segment as u8))
            .max_rect(body),
    );
    content.set_clip_rect(body.intersect(content.clip_rect()));
    {
        let segment = state.segment;
        let mut talk = Talk {
            allowed: &detail.allowed,
            idle: shown.acting.is_none(),
            composing: view.composing,
            now: view.now,
            state,
            events,
            actions,
        };
        egui::ScrollArea::vertical()
            // Each part keeps its own place.
            .id_salt(("pull-request-scroll", segment as u8))
            .auto_shrink([false, false])
            .show(&mut content, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                match segment {
                    Segment::Summary => summary(ui, p, shown, &mut talk),
                    _ => timeline(ui, p, shown, &mut talk),
                }
                // Room for the control that floats over the corner.
                ui.add_space(44.0);
            });
    }
    if state.composer.is_none() {
        compose_button(ui, body, p, waiting, state);
    }
}

/// The pull requests open in the tab, a pill each: pressed, it is shown;
/// its cross, or a middle press, takes it out of the tab.
fn open_strip(ui: &mut Ui, strip: Rect, p: Palette, view: &View, events: &mut Vec<Event>) {
    let current = match &view.body {
        Body::Linked(_) => None,
        Body::Reading(link) | Body::Failed(link, _) => Some(*link),
        Body::Shown(shown) => Some(shown.link),
    };
    let mut row = ui.new_child(
        UiBuilder::new()
            .id_salt("pull-request-open")
            .max_rect(strip)
            .layout(Layout::left_to_right(Align::Center)),
    );
    row.set_clip_rect(strip.intersect(row.clip_rect()));
    egui::ScrollArea::horizontal()
        .id_salt("pull-request-open-pills")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .auto_shrink([false, false])
        .show(&mut row, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for link in view.opened {
                let selected = current.is_some_and(|current| current.same(link));
                let text = format!("#{}", link.number());
                let galley = ui.painter().layout_no_wrap(
                    text,
                    theme::medium(11.5),
                    if selected { p.fg } else { p.secondary },
                );
                let (_, pill) = ui.allocate_space(vec2(galley.size().x + 34.0, 22.0));
                let cross = Rect::from_min_size(
                    Pos2::new(pill.right() - 20.0, pill.top() + 2.0),
                    Vec2::splat(18.0),
                );
                let response = ui
                    .interact(
                        pill,
                        Id::new(("pull-request-pill", link.url())),
                        Sense::click(),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand);
                response.widget_info(|| {
                    WidgetInfo::selected(
                        WidgetType::SelectableLabel,
                        true,
                        selected,
                        format!("Pull request {}", link.label()),
                    )
                });
                let close = ui
                    .interact(
                        cross,
                        Id::new(("pull-request-pill-close", link.url())),
                        Sense::click(),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand);
                close.widget_info(|| {
                    WidgetInfo::labeled(
                        WidgetType::Button,
                        true,
                        format!("Close pull request {} in this tab", link.label()),
                    )
                });
                let painter = ui.painter();
                let fill = if selected {
                    Some(theme::tint(p.fg, 0.08))
                } else if response.hovered() || close.hovered() {
                    Some(theme::tint(p.fg, 0.045))
                } else {
                    None
                };
                if let Some(fill) = fill {
                    painter.rect_filled(pill, 7, fill);
                }
                if response.has_focus() {
                    focus_ring(painter, pill, 7, p);
                }
                galley_at(
                    painter,
                    Pos2::new(pill.left() + 9.0, pill.center().y + 0.5),
                    galley,
                );
                if close.hovered() {
                    painter.rect_filled(cross, 5, p.hover);
                }
                icons::paint(
                    painter,
                    Rect::from_center_size(cross.center(), Vec2::splat(9.0)),
                    Icon::Close,
                    if close.hovered() { p.fg } else { p.muted },
                );
                if close.clicked() || response.middle_clicked() {
                    events.push(Event::Close(link.clone()));
                } else if response.clicked() && !selected {
                    events.push(Event::Open(link.clone()));
                }
                close.on_hover_text("Close in this tab");
                response.on_hover_text(link.label());
            }
        });
}

/// The tab in `rect`, below the panel's tabs.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &View,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let mut events = Vec::new();
    let mut child = ui.new_child(UiBuilder::new().id_salt("pull-request").max_rect(rect));
    child.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(view.window));
    child.multiply_opacity(view.reveal);
    let ui = &mut child;
    let mut inner = Rect::from_min_max(
        Pos2::new(rect.left() + 6.0, rect.top()),
        Pos2::new(rect.right() - 8.0, rect.bottom() - 8.0),
    );
    if !view.opened.is_empty() {
        let strip = Rect::from_min_size(inner.min, vec2(inner.width(), OPEN));
        open_strip(ui, strip, p, view, &mut events);
        inner.min.y = strip.bottom();
    }
    match &view.body {
        Body::Linked(links) => linked(ui, inner, p, links, &mut events, actions),
        Body::Reading(link) => {
            let header = name_row(ui, inner, p, link, None, |_| {}, &mut events, actions);
            let below = Rect::from_min_max(Pos2::new(inner.left(), header.bottom()), inner.max);
            centered_note(ui, below, p, "Reading the pull request…");
        }
        Body::Failed(link, failure) => {
            let header = name_row(ui, inner, p, link, None, |_| {}, &mut events, actions);
            let below = Rect::from_min_max(Pos2::new(inner.left(), header.bottom()), inner.max);
            failed(ui, below, p, link, *failure, &mut events, actions);
        }
        Body::Shown(shown) => {
            pull_request(ui, inner, p, shown, view, state, &mut events, actions);
        }
    }
    actions.extend(events.into_iter().map(Action::PullRequest));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::pull_request::Allowed;

    fn detail() -> Detail {
        Detail {
            id: "PR_1".into(),
            auto_merge: None,
            viewed: Vec::new(),
            reactions: Vec::new(),
            title: "A tab for pull requests".into(),
            state: Standing::Open,
            author: "ada".into(),
            body: String::new(),
            base: "main".into(),
            head: "fork:feat/tab".into(),
            opened: 1000,
            updated: 1000,
            ended: None,
            merged_by: String::new(),
            commits: 1,
            history: Vec::new(),
            head_commit: String::new(),
            added: 0,
            removed: 0,
            files: 0,
            merge: Merge::Ready,
            decision: None,
            reviewers: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            checks: Vec::new(),
            more_checks: 0,
            entries: Vec::new(),
            earlier: 0,
            allowed: Allowed::default(),
        }
    }

    fn check(outcome: Outcome) -> Check {
        Check {
            name: "Test".into(),
            workflow: "CI".into(),
            outcome,
            seconds: None,
            url: String::new(),
        }
    }

    #[test]
    fn ages_and_durations_are_said_in_the_largest_unit_that_fits() {
        let said: Vec<String> = [
            -5,
            59,
            60,
            3599,
            3600,
            86_400,
            59 * 86_400,
            60 * 86_400,
            400 * 86_400,
        ]
        .into_iter()
        .map(ago)
        .collect();
        assert_eq!(
            said,
            [
                "just now",
                "just now",
                "1 min ago",
                "59 min ago",
                "1 h ago",
                "1 d ago",
                "59 d ago",
                "2 mo ago",
                "1 y ago"
            ]
        );
        assert_eq!(
            [duration(45), duration(90), duration(3900)],
            ["45s", "1m 30s", "1h 5m"]
        );
    }

    #[test]
    fn what_keeps_a_pull_request_from_merging_is_said_before_that_nothing_does() {
        let mut detail = detail();
        assert_eq!(stands(&detail, 2000), (Tone::Good, "Ready to merge".into()));
        assert_eq!(checks_summary(&detail), "No checks reported");

        detail.checks = vec![check(Outcome::Passed), check(Outcome::Skipped)];
        assert_eq!(checks_summary(&detail), "All checks passed");
        assert_eq!(stands(&detail, 2000).0, Tone::Good);
        detail.checks.push(check(Outcome::Running));
        detail.more_checks = 2;
        assert_eq!(
            stands(&detail, 2000),
            (Tone::Running, "1 of 5 checks running".into())
        );
        detail.checks.push(check(Outcome::Failed));
        assert_eq!(
            stands(&detail, 2000),
            (Tone::Bad, "1 of 6 checks failing".into())
        );
        // A review that asks for changes and a conflict come before checks.
        detail.decision = Some(Decision::ChangesRequested);
        assert_eq!(stands(&detail, 2000).1, "Changes requested");
        detail.merge = Merge::Conflicts;
        assert_eq!(stands(&detail, 2000).1, "Conflicts with main");

        detail.state = Standing::Draft;
        assert_eq!(stands(&detail, 2000).1, "Draft, not ready for review");
        detail.state = Standing::Merged;
        detail.ended = Some(2000 - 7200);
        detail.merged_by = "grace".into();
        assert_eq!(
            stands(&detail, 2000),
            (Tone::Merged, "Merged by grace · 2 h ago".into())
        );
        detail.state = Standing::Closed;
        assert_eq!(stands(&detail, 2000).1, "Closed without merging · 2 h ago");
    }

    #[test]
    fn the_way_to_merge_is_the_persons_where_the_repository_still_allows_it() {
        let mut detail = detail();
        let mut state = State::default();
        assert_eq!(method(&detail, &state), None);
        detail.allowed.methods = vec![Method::Merge, Method::Squash];
        assert_eq!(method(&detail, &state), Some(Method::Merge));
        state.method = Some(Method::Squash);
        assert_eq!(method(&detail, &state), Some(Method::Squash));
        state.method = Some(Method::Rebase);
        assert_eq!(method(&detail, &state), Some(Method::Merge));
        assert_eq!(branch("fork:feat/tab"), "feat/tab");
        assert_eq!(branch("feat/tab"), "feat/tab");
    }

    #[test]
    fn a_draft_stays_with_its_pull_request_and_the_oldest_gives_way() {
        let mut state = State {
            segment: Segment::Code,
            selected: Some("src/a.rs".into()),
            confirm: Some(Confirm::Close),
            method: Some(Method::Squash),
            ..State::default()
        };
        *state.draft_mut("a") = "Thanks".into();
        state.composer = Some(Composer::Review);
        // Another pull request starts on its summary; the draft is kept.
        state.opened();
        assert_eq!(state.segment, Segment::Summary);
        assert!(state.selected.is_none() && state.confirm.is_none() && state.composer.is_none());
        assert_eq!(state.method, Some(Method::Squash));
        assert_eq!((state.draft("a"), state.draft("b")), ("Thanks", ""));
        for url in ["b", "c", "d", "e", "f", "g", "h", "i"] {
            state.draft_mut(url);
        }
        // An empty one made room; what was written is still there.
        assert_eq!(state.drafts.len(), DRAFTS);
        assert_eq!(state.draft("a"), "Thanks");
        state.sent("a");
        assert_eq!(state.draft("a"), "");
    }

    #[test]
    fn comments_on_lines_wait_with_their_pull_request_until_its_review_is_sent() {
        let comment = |line: u32| LineComment {
            path: "src/a.rs".into(),
            line,
            removed: false,
            body: "Why?".into(),
        };
        let mut state = State::default();
        state.add_pending("a", comment(1));
        state.add_pending("b", comment(2));
        state.add_pending("a", comment(3));
        // Another pull request shown keeps what waits for each.
        state.opened();
        let lines = |state: &State, url: &str| -> Vec<u32> {
            state.pending(url).map(|comment| comment.line).collect()
        };
        assert_eq!(
            (lines(&state, "a"), lines(&state, "b")),
            (vec![1, 3], vec![2])
        );
        state.discard_pending("a", 1);
        assert_eq!((lines(&state, "a"), lines(&state, "b")), (vec![1], vec![2]));
        // A comment alone sends nothing of the review.
        state.done("a", &Act::Comment("Thanks".into()));
        assert_eq!(lines(&state, "a"), [1]);
        state.done(
            "a",
            &Act::Review(Verdict::Commented, String::new(), Vec::new()),
        );
        assert_eq!((lines(&state, "a"), lines(&state, "b")), (vec![], vec![2]));
        // What was rewritten is put away once it is saved.
        state.editing = Some(Editing::Title);
        state.edit = "New".into();
        state.done("a", &Act::Title("New".into()));
        assert!(state.editing.is_none() && state.edit.is_empty());
    }

    #[test]
    fn a_pull_request_the_host_merges_by_itself_says_so() {
        let mut detail = detail();
        detail.auto_merge = Some(Method::Squash);
        assert_eq!(
            stands(&detail, 2000),
            (
                Tone::Running,
                "Merges by itself when it may · Squash and merge".into()
            )
        );
        // What holds it back is still said first.
        detail.merge = Merge::Conflicts;
        assert_eq!(stands(&detail, 2000).0, Tone::Bad);
    }
}
