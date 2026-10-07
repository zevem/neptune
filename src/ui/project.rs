//! The project tab: the workspace's project as a conversation with its
//! lead. Under the project's name what needs the person is pinned while it
//! does, and only then. A strip of one row has a pill for each agent the
//! lead started, pressed as a tab is, and lists them all over the chat when
//! asked, so the chat keeps its height; then the chat and the field a
//! message is written in. Everything else the project has is
//! one press away on its details: what its agents share, what wakes its
//! lead, and what the lead and its agents run with. Without a project the
//! tab is the field that starts one.
mod creation;
mod details;
mod settings;
#[cfg(test)]
mod tests;

use super::chat::{self, Chat, Editor};
use super::explorer::centered_note;
use super::helpers::{
    LinkedPullRequest, PullRequestChips, SheetPlacement, elided, focus_ring, galley_at, menu_item,
    menu_layout, menu_separator, path_label, place, sheet, sheet_header,
};
use super::{Action, agents::elapsed_label};
use crate::{
    icons::{self, Icon},
    projects::{
        settings::{LeadSettings, Settings},
        transcript::Attachment,
    },
    theme::{self, Palette},
};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Frame, Id, Layout, Margin, Pos2, Rect, Sense, Ui,
    UiBuilder, Vec2, WidgetInfo, WidgetType,
    scroll_area::{ScrollBarVisibility, ScrollSource},
    vec2,
};
use neptune_model::{AgentKind, PaneId, ProjectId, WorkspaceId};
use std::{collections::BTreeMap, time::Duration};

const HEADER: f32 = 30.0;
/// The strip of the project's agents: one row, however many they are.
const ROW: f32 = 26.0;
/// An agent's pill in the strip, and the widest one gets.
const PILL: f32 = 22.0;
const PILL_MAX: f32 = 130.0;
/// The room between two pills.
const PILL_GAP: f32 = 4.0;
/// The least of its name a pill shows after its number, with the room
/// between them: a letter or two says nothing.
const PILL_NAME: f32 = 20.0;
/// How far a clipped edge of the strip fades.
const FADE: f32 = 18.0;
/// An agent in the list of all of them: its name and state, then its CLI,
/// its branch and its pull requests.
const MEMBER: f32 = 42.0;
/// The line that heads that list.
const HEADING: f32 = 24.0;
/// The share of the tab's height the list may take over the chat.
const ROSTER: f32 = 0.6;
/// The widest the tab's body is drawn in a sheet, in a window too narrow
/// for the panel.
const SHEET: f32 = 560.0;
/// The line under the composer.
const HINT: f32 = 18.0;
/// The room each of the two controls at the composer's trailing edge takes.
const CONTROLS: f32 = 28.0;
/// The row that leads the details back to the chat: the control that
/// chooses among their parts, and the room under it.
const BACK: f32 = 34.0;
/// A file of the project's context: its name, then who wrote it and when.
const FILE: f32 = 42.0;
/// The share of the room above the composer that what is pinned over the
/// chat may take before it scrolls by itself.
const PINNED: f32 = 0.4;
/// The room the chat keeps under what is pinned: about four lines.
const TRANSCRIPT: f32 = 68.0;
/// A sheet lower than this says what needs the person in one line a row.
const LOW: f32 = 260.0;
/// What the instructions may hold, as the store counts it.
const MAX_INSTRUCTIONS: usize = crate::projects::context::MAX_INSTRUCTIONS;

/// What a watch may tell the lead to do, and how often one may run.
const MAX_INSTRUCTION: usize = crate::projects::subscription::MAX_INSTRUCTION;
const MIN_MINUTES: u32 = crate::projects::schedule::MIN_MINUTES;
const MAX_MINUTES: u32 = crate::projects::schedule::MAX_MINUTES;

pub fn instructions_id() -> Id {
    Id::new("project-instructions")
}
fn watch_title_id() -> Id {
    Id::new("project-watch-title")
}
/// The fields of the tab beside the chat's: none keeps the keyboard when
/// the tab leaves, and Escape in one returns it to the terminal.
pub fn field_ids() -> [Id; 8] {
    [
        instructions_id(),
        watch_title_id(),
        Id::new("project-watch-every"),
        Id::new("project-watch-url"),
        Id::new("project-watch-instruction"),
        model_id(Setting::Lead),
        model_id(Setting::Claude),
        model_id(Setting::Codex),
    ]
}
/// Whose model and effort a group of the settings holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Setting {
    Lead,
    /// The Claude Code agents the lead starts,
    Claude,
    /// and the Codex ones.
    Codex,
}
impl Setting {
    /// The group of the agents a lead starts with `kind`.
    pub fn agents(kind: AgentKind) -> Self {
        match kind {
            AgentKind::Codex => Self::Codex,
            _ => Self::Claude,
        }
    }
    /// The CLI its choices are those of, in a project `lead` leads.
    pub fn kind(self, lead: AgentKind) -> AgentKind {
        match self {
            Self::Lead => lead,
            Self::Claude => AgentKind::Claude,
            Self::Codex => AgentKind::Codex,
        }
    }
}
/// The field a model that is in no list is typed in.
fn model_id(setting: Setting) -> Id {
    Id::new(("project-setting-model", setting))
}

/// What the body of the tab shows: the chat, or one part of the details.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Segment {
    #[default]
    Chat,
    /// What the project's agents share.
    Context,
    /// What wakes its lead beside its agents.
    Watches,
    /// What its lead and the agents it starts run with.
    Settings,
}

/// What sets off the watch being added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WatchKind {
    #[default]
    Schedule,
    PullRequest,
}
/// The watch the person is adding.
pub struct WatchForm {
    pub open: bool,
    pub title: String,
    pub kind: WatchKind,
    /// The minutes between its runs, where they are one of those offered.
    /// None is a number of the person's own, typed in `every`.
    pub interval: Option<u32>,
    pub every: String,
    pub url: String,
    pub instruction: String,
    /// Its first field takes the keyboard on its next frame.
    pub focus: bool,
}
impl Default for WatchForm {
    fn default() -> Self {
        Self {
            open: false,
            title: String::new(),
            kind: WatchKind::default(),
            interval: Some(60),
            every: String::new(),
            url: String::new(),
            instruction: String::new(),
            focus: false,
        }
    }
}
/// What sets a watch off, as the person asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchAsk {
    Every(u32),
    PullRequest(String),
}
/// A model that is in no list, while it is typed in place of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Custom {
    /// The creation form's is under no project, a project's settings under
    /// its own.
    pub owner: Option<ProjectId>,
    pub setting: Setting,
    pub text: String,
    /// Its field takes the keyboard on its next frame.
    pub focus: bool,
}

pub struct State {
    /// What the creation form holds.
    pub goal: String,
    /// The CLI picked to lead the next project, where more than one can.
    pub lead: Option<AgentKind>,
    /// The message being written to each project's lead.
    pub drafts: BTreeMap<ProjectId, String>,
    /// The files that go with it, in the order they were attached.
    pub attached: BTreeMap<ProjectId, Vec<Attachment>>,
    /// Where the chat of the project in view was drawn, while its field
    /// takes files: files released there are attached. Taken every frame.
    pub chat_area: Option<(ProjectId, Rect)>,
    /// Files from another application are held over the chat.
    pub dropping: bool,
    /// The field in view takes the keyboard on its next frame.
    pub focus: bool,
    /// The list of all the agents opens over the chat on its next frame.
    pub show_agents: bool,
    /// How many of the newest entries of the chat are laid out.
    pub shown: usize,
    pub segment: Segment,
    /// The part of the details last looked at: they open on it again.
    pub details: Segment,
    /// Each project's instructions as they are being written, while they
    /// differ from what its folder holds.
    pub instructions: BTreeMap<ProjectId, String>,
    /// The file about to be deleted, with the project whose context holds
    /// it: the question is never carried over to another project's file.
    pub deleting: Option<(ProjectId, String)>,
    pub watch: WatchForm,
    /// The watch about to be deleted, with its project.
    pub removing: Option<(ProjectId, u64)>,
    /// The model and effort picked for the lead of the next project.
    pub lead_settings: LeadSettings,
    /// The creation form shows them, and the choice of its lead.
    pub lead_options: bool,
    /// The one model being typed, until it is applied or put away.
    pub custom: Option<Custom>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            goal: String::new(),
            lead: None,
            drafts: BTreeMap::new(),
            attached: BTreeMap::new(),
            chat_area: None,
            dropping: false,
            focus: false,
            show_agents: false,
            shown: chat::PAGE,
            segment: Segment::Chat,
            details: Segment::Context,
            instructions: BTreeMap::new(),
            deleting: None,
            watch: WatchForm::default(),
            removing: None,
            lead_settings: LeadSettings::default(),
            lead_options: false,
            custom: None,
        }
    }
}
impl State {
    /// Shows `segment`: the chat, or the details on that part.
    pub fn show(&mut self, segment: Segment) {
        self.segment = segment;
        if segment != Segment::Chat {
            self.details = segment;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Show the tab with the keyboard in its field: the creation form's, or
    /// the composer's where the workspace has a project.
    Compose,
    /// Show the tab on one of its parts: the chat, or a part of its details.
    Show(Segment),
    /// Show the chat with the list of all the project's agents over it.
    ShowAgents,
    /// Show the watches with the form that adds one, ready to be written in.
    WriteWatch,
    Create {
        workspace: WorkspaceId,
        goal: String,
        lead: AgentKind,
        /// The model and effort its lead runs with; none is its CLI's own.
        model: Option<String>,
        effort: Option<String>,
    },
    /// What the project's lead runs with from its next turn; none is its
    /// CLI's own choice.
    SetLead {
        project: ProjectId,
        model: Option<String>,
        effort: Option<String>,
    },
    /// What the agents the lead starts with `kind` run with. A value that
    /// is set wins over what the lead asks for; none leaves it to the lead.
    /// `ultracode` is Claude Code's alone.
    SetAgentDefaults {
        project: ProjectId,
        kind: AgentKind,
        model: Option<String>,
        effort: Option<String>,
        ultracode: Option<bool>,
    },
    /// Show the folder the project works in, in the file manager.
    RevealDirectory(ProjectId),
    Send {
        project: ProjectId,
        text: String,
        attachments: Vec<Attachment>,
    },
    /// Ask for files to go with the message being written.
    Attach(ProjectId),
    /// Interrupt the lead's turn.
    Stop(ProjectId),
    SetPaused(ProjectId, bool),
    /// Start the lead again after it could not start or ran into a limit.
    Retry(ProjectId),
    /// Open the sheet that renames it.
    Rename(ProjectId),
    SetName(ProjectId, String),
    /// Ask before removing it.
    Remove(ProjectId),
    ConfirmRemove(ProjectId),
    /// Begin a new chat with a lead that starts afresh.
    NewChat(ProjectId),
    /// Show the folder Neptune keeps the project's chat and context in.
    Reveal(ProjectId),
    /// Show the folder inside it that holds what its agents share.
    RevealContext(ProjectId),
    /// "Load earlier" was pressed where older entries are in its folder.
    Earlier(ProjectId),
    /// Take up the project that worked in this workspace's directory.
    Reopen(WorkspaceId),
    /// Open a file of the project's context with its default application.
    OpenFile(ProjectId, String),
    /// Show it in the file manager.
    RevealFile(ProjectId, String),
    /// Delete it; the person has confirmed.
    DeleteFile(ProjectId, String),
    /// Keep what the person wrote as the project's instructions.
    SaveInstructions {
        project: ProjectId,
        text: String,
    },
    /// Add a watch the person wrote: it runs without being allowed again.
    AddWatch {
        project: ProjectId,
        title: String,
        trigger: WatchAsk,
        instruction: String,
    },
    /// Allow a schedule the lead proposed, or decline it.
    AllowWatch(ProjectId, u64, bool),
    PauseWatch(ProjectId, u64, bool),
    /// Fire it now, whatever its schedule says.
    RunWatch(ProjectId, u64),
    /// Delete it; the person has confirmed.
    DeleteWatch(ProjectId, u64),
    /// Bring an agent's terminal forward and show what changed where it
    /// works, if it is still that terminal.
    Changes(PaneId, u64),
    /// Have an agent told what its pull request needs, in fixed words.
    FollowUp {
        project: ProjectId,
        pane: PaneId,
        generation: u64,
        ask: FollowUp,
    },
    /// Put words of the chat on the clipboard.
    Copy(String),
}
/// What the person has an agent told about its pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowUp {
    /// Its checks fail.
    Checks,
    /// It has review comments nobody resolved.
    Comments,
}

/// What a row of "Needs you" offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remedy {
    /// The agent's terminal, where its prompt is answered.
    Open(PaneId, u64),
    Retry,
    Resume,
    /// Nothing here: the person acts elsewhere.
    None,
}
pub struct Need {
    pub title: String,
    pub detail: String,
    pub remedy: Remedy,
}
pub struct Member {
    pub pane: PaneId,
    pub generation: u64,
    pub kind: AgentKind,
    /// What the lead called it.
    pub title: String,
    /// What it is doing: Starting, Working, Needs you, Ready for review,
    /// Idle or Ended.
    pub state: String,
    /// The same more exactly, such as what it waits for.
    pub detail: String,
    pub waits: bool,
    pub works: bool,
    /// Its last turn ended with a report nobody has opened its terminal
    /// since.
    pub ready: bool,
    pub elapsed: Option<Duration>,
    /// The branch of its worktree, where it works in one.
    pub branch: Option<String>,
    /// The pull requests it linked, with what is known of each.
    pub pull_requests: Vec<LinkedPullRequest>,
    /// Its terminal has a tab that can be put away again.
    pub can_background: bool,
}
/// A file of a project's context folder.
pub struct File {
    /// Its path from the folder.
    pub name: String,
    /// Who last wrote it, as far as it says.
    pub author: String,
    /// How long ago it last changed.
    pub age: Duration,
    pub size: String,
    /// False for the one Neptune writes again by itself.
    pub removable: bool,
}
/// One watch of a project.
pub struct Watch {
    pub id: u64,
    pub title: String,
    /// What sets it off, and when it runs next or what became of its last
    /// run, in words.
    pub trigger: String,
    pub detail: String,
    pub paused: bool,
    /// The lead proposed it and the person has not allowed it.
    pub proposed: bool,
    /// What it would tell the lead to do, while it is proposed: the person
    /// allows every word of it.
    pub instruction: String,
}
/// What a project's agents share.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub files: &'a [File],
    /// The instructions as the folder holds them.
    pub instructions: &'a str,
    /// The folder was read at least once.
    pub read: bool,
}
/// One value a setting can take, and the words a list shows for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: &'static str,
    pub label: String,
}
/// What one CLI can be asked for, beside leaving the choice open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choices {
    pub kind: AgentKind,
    /// Its models that have a name here. Any other is typed, and
    /// `model_name` says whether a CLI takes what was typed.
    pub models: Vec<Choice>,
    pub efforts: Vec<Choice>,
}
/// Every choice the settings offer. The same for every project.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    /// For a lead, by its CLI.
    pub lead: Vec<Choices>,
    /// For the agents a lead starts, by their CLI.
    pub agents: Vec<Choices>,
}
impl Catalog {
    /// What `setting` offers in a project `lead` leads.
    pub fn of(&self, setting: Setting, lead: AgentKind) -> Option<&Choices> {
        let list = match setting {
            Setting::Lead => &self.lead,
            Setting::Claude | Setting::Codex => &self.agents,
        };
        let kind = setting.kind(lead);
        list.iter().find(|choices| choices.kind == kind)
    }
}
pub struct Project<'a> {
    pub id: ProjectId,
    pub name: &'a str,
    /// Where it works, as the person knows it, and in full.
    pub directory: &'a str,
    pub directory_path: &'a std::path::Path,
    /// The CLI that leads it, which stays the same.
    pub lead: AgentKind,
    /// The model its lead runs on, as its CLI named it when it last
    /// started. None before that.
    pub lead_model: Option<&'a str>,
    /// What the person chose for its lead and for the agents it starts.
    pub settings: &'a Settings,
    /// Its lead was started with something else than is chosen now: it
    /// takes the choice with its next turn.
    pub pending: bool,
    pub catalog: &'a Catalog,
    /// The CLIs installed here that can lead. None while they are looked
    /// for.
    pub leads: Option<&'a [AgentKind]>,
    pub paused: bool,
    pub needs: &'a [Need],
    pub members: &'a [Member],
    pub chat: Chat<'a>,
    /// A turn is running: the composer's control stops it.
    pub busy: bool,
    /// How long the lead's turn has run.
    pub elapsed: Option<Duration>,
    /// What the turns of the chat in memory cost, as the lead's CLI put it.
    pub usage: Option<&'a str>,
    /// The user's messages that have not reached the lead.
    pub queued: usize,
    /// Nothing of it is written and its lead stays off.
    pub locked: bool,
    pub context: Context<'a>,
    pub watches: &'a [Watch],
}
pub enum Body<'a> {
    /// No workspace is open.
    Nothing,
    /// The workspace is connected over SSH.
    Remote,
    /// A lead cannot reach Neptune on this system.
    Unsupported,
    Empty {
        workspace: WorkspaceId,
        /// Where the project would work, as the person knows it.
        directory: &'a str,
        /// Its folder is being made.
        starting: bool,
        /// The project that worked here before, by name.
        reopen: Option<&'a str>,
        /// The CLIs installed here that can lead, in the order they are
        /// offered. None while they are looked for.
        leads: Option<&'a [AgentKind]>,
        catalog: &'a Catalog,
    },
    Project(Box<Project<'a>>),
}
pub struct View<'a> {
    pub body: Body<'a>,
    /// An input method is composing.
    pub composing: bool,
    pub window: Rect,
    /// How much of the panel is shown while a toggle slides it.
    pub reveal: f32,
}

/// How the project's agents stand, in one line: the one agent by name, or
/// how many are in each state.
pub fn summary(members: &[Member]) -> String {
    if let [one] = members {
        let mut parts = vec![format!("{} {}", one.pane, one.title), one.state.clone()];
        if let Some(elapsed) = one.elapsed {
            parts.push(elapsed_label(elapsed));
        }
        return parts.join(" · ");
    }
    tally(members)
}
/// How many of them are in each state, those that wait first.
fn tally<'a>(members: impl IntoIterator<Item = &'a Member>) -> String {
    let mut counts = [0usize; 6];
    for member in members {
        let slot = if member.waits {
            0
        } else if member.state == "Starting" {
            2
        } else if member.works {
            1
        } else if member.ready {
            3
        } else if member.state == "Ended" {
            5
        } else {
            4
        };
        counts[slot] += 1;
    }
    let words = [
        "needs you",
        "working",
        "starting",
        "ready for review",
        "idle",
        "ended",
    ];
    counts
        .into_iter()
        .zip(words)
        .filter(|(count, _)| *count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The words a list shows for `value`: its name there, or the value itself
/// where no list has it.
fn choice_label(choices: Option<&Choices>, value: &str, models: bool) -> String {
    choices
        .map(|choices| {
            if models {
                &choices.models
            } else {
                &choices.efforts
            }
        })
        .and_then(|list| list.iter().find(|choice| choice.value == value))
        .map_or_else(|| value.to_owned(), |choice| choice.label.clone())
}
/// "Claude Code · Opus · High": a lead's CLI, then what it runs with. A
/// part that is left to the CLI is left out.
fn lead_words(
    kind: AgentKind,
    model: Option<&str>,
    effort: Option<&str>,
    choices: Option<&Choices>,
) -> String {
    let mut parts = vec![super::agents::kind_name(kind).to_owned()];
    parts.extend(model.map(|model| choice_label(choices, model, true)));
    parts.extend(effort.map(|effort| choice_label(choices, effort, false)));
    parts.join(" · ")
}

/// Where the tab is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fit {
    /// In the panel.
    Tab,
    /// In the sheet of a window too narrow for the panel: its title row is
    /// the project's header.
    Sheet,
    /// In a sheet too low for whole rows about what needs the person: each
    /// is one line.
    Low,
}

/// How much a small button of a row asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Plain,
    /// The one recommended action of its row.
    Accent,
    /// Text alone, for taking something back.
    Quiet,
}
/// A small button of a row at the cursor. `name` is its accessible name and
/// must be its own among the buttons in view.
fn row_action(ui: &mut Ui, p: Palette, label: &str, name: &str, tone: Tone) -> bool {
    let width = chat::row_button_width(ui, label);
    let (_, rect) = ui.allocate_space(vec2(width, 22.0));
    match tone {
        Tone::Plain => return chat::row_button(ui, p, rect, label, name).clicked(),
        Tone::Accent => {
            let enabled = ui.is_enabled();
            return chat::row_button_primary(ui, p, rect, label, name, enabled).clicked();
        }
        Tone::Quiet => {}
    }
    let response = ui.interact(rect, Id::new(("project-row-button", name)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), name));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.is_pointer_button_down_on() {
            painter.rect_filled(rect, 6, p.pressed);
        } else if response.hovered() {
            painter.rect_filled(rect, 6, p.hover);
        }
        if response.has_focus() {
            focus_ring(painter, rect, 6, p);
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            theme::medium(11.5),
            p.secondary,
        );
    }
    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// A muted line of words, wrapped.
fn quiet(ui: &mut Ui, p: Palette, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::regular(11.5))
                .color(p.muted),
        )
        .wrap()
        .selectable(false),
    );
}

/// The project's name, where it works, and its two controls in `rect`: the
/// details and the menu.
fn header(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    project: &Project,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let menu = Rect::from_center_size(
        Pos2::new(rect.right() - 14.0, rect.center().y),
        Vec2::splat(28.0),
    );
    let details = menu.translate(vec2(-28.0, 0.0));
    // It stays pressed while the details are in view, and puts them away.
    let shown = state.segment != Segment::Chat;
    let toggled = place(
        ui,
        details,
        Layout::left_to_right(Align::Center),
        "project-details",
        |ui| {
            let under = ui.painter().add(egui::Shape::Noop);
            let response = icons::button(ui, Icon::Settings, "Project details");
            if shown {
                ui.painter().set(
                    under,
                    egui::Shape::rect_filled(response.rect.shrink(1.0), 7, p.pressed),
                );
            }
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_toggled(shown.into());
            });
            response.clicked()
        },
    );
    if toggled {
        let next = if shown { Segment::Chat } else { state.details };
        state.show(next);
    }
    place(
        ui,
        menu,
        Layout::left_to_right(Align::Center),
        "project-actions",
        |ui| {
            let more = icons::button(ui, Icon::Ellipsis, "Project menu");
            egui::Popup::menu(&more).show(|ui| {
                menu_layout(ui, 200.0);
                // A project that is not written to has no chat to begin
                // and no lead to hold back.
                ui.add_enabled_ui(!project.locked, |ui| {
                    if menu_item(ui, p, Icon::Plus, "New chat", "", false) {
                        actions.push(Action::Project(Event::NewChat(project.id)));
                        ui.close();
                    }
                    let (icon, label) = if project.paused {
                        (Icon::Play, "Resume project")
                    } else {
                        (Icon::Pause, "Pause project")
                    };
                    if menu_item(ui, p, icon, label, "", false) {
                        actions.push(Action::Project(Event::SetPaused(
                            project.id,
                            !project.paused,
                        )));
                        ui.close();
                    }
                });
                menu_separator(ui, p);
                if menu_item(ui, p, Icon::Pencil, "Rename…", "", false) {
                    actions.push(Action::Project(Event::Rename(project.id)));
                    ui.close();
                }
                if menu_item(ui, p, Icon::Folder, "Reveal saved files", "", false) {
                    actions.push(Action::Project(Event::Reveal(project.id)));
                    ui.close();
                }
                // What the lead's CLI put on its turns, not a bill: a
                // subscription is not charged by the turn.
                if let Some(usage) = project.usage {
                    menu_separator(ui, p);
                    ui.add_enabled_ui(false, |ui| {
                        menu_item(ui, p, Icon::Clipboard, "Usage estimate", usage, false);
                    });
                }
                menu_separator(ui, p);
                if menu_item(ui, p, Icon::Trash, "Remove project…", "", true) {
                    actions.push(Action::Project(Event::Remove(project.id)));
                    ui.close();
                }
            });
        },
    );
    let room = details.left() - 4.0;
    let title = elided(
        ui.painter(),
        project.name,
        theme::medium(12.5),
        p.fg,
        room - rect.left() - 6.0,
    );
    let after = rect.left() + 6.0 + title.size().x + 8.0;
    galley_at(
        ui.painter(),
        Pos2::new(rect.left() + 6.0, rect.center().y),
        title,
    );
    // Where it works, beside what it is called, as far as there is room:
    // its last directory is what is kept of it.
    if room - after >= 48.0 {
        let font = theme::regular(11.5);
        let whole =
            ui.painter()
                .layout_no_wrap(project.directory.to_owned(), font.clone(), p.muted);
        let words = if whole.size().x <= room - after {
            project.directory.to_owned()
        } else {
            let letters = project.directory.chars().count().max(1) as f32;
            let fit = ((room - after) / (whole.size().x / letters)).floor() as usize;
            path_label(project.directory_path, fit.max(4))
        };
        let shown = galley_at(
            ui.painter(),
            Pos2::new(after, rect.center().y),
            elided(ui.painter(), &words, font, p.muted, room - after),
        );
        ui.interact(shown, ui.id().with("project-folder"), Sense::hover())
            .on_hover_text(project.directory_path.display().to_string());
    }
}

/// What a row about something that waits offers, with its accessible name.
fn remedy(project: ProjectId, need: &Need) -> Option<(&'static str, String, Action)> {
    match need.remedy {
        Remedy::Open(pane, generation) => Some((
            "Open",
            format!("Open terminal of agent {pane}"),
            Action::OpenAgent(pane, generation),
        )),
        Remedy::Retry => Some((
            "Try again",
            "Start the lead again".to_owned(),
            Action::Project(Event::Retry(project)),
        )),
        Remedy::Resume => Some((
            "Resume",
            "Resume project".to_owned(),
            Action::Project(Event::SetPaused(project, false)),
        )),
        Remedy::None => None,
    }
}
/// A row pinned over the chat: a title with its one action at the trailing
/// edge, and what is true of it underneath. `mark` leads a row about
/// something that waits; the attention colour is the mark's alone. `low`
/// says both in one line, with the whole text a rest of the pointer away.
fn pinned_row(
    ui: &mut Ui,
    p: Palette,
    (title, detail): (&str, &str),
    mark: bool,
    low: bool,
    action: Option<(&str, &str, Tone)>,
) -> bool {
    let mut pressed = false;
    let fill = if mark {
        theme::tint(p.attention, 0.10)
    } else {
        p.control
    };
    Frame::new()
        .fill(fill)
        .corner_radius(theme::metrics::ROW_RADIUS)
        .inner_margin(Margin::symmetric(10, if low { 2 } else { 8 }))
        .show(ui, |ui| {
            let width = ui.available_width();
            ui.set_width(width);
            let (_, line) = ui.allocate_space(vec2(width, 22.0));
            let mut right = line.right();
            if let Some((label, name, tone)) = action {
                let button = chat::row_button_width(ui, label);
                right -= button;
                let at = Rect::from_min_size(Pos2::new(right, line.top()), vec2(button, 22.0));
                pressed = place(
                    ui,
                    at,
                    Layout::left_to_right(Align::Center),
                    ("project-pinned-action", name),
                    |ui| row_action(ui, p, label, name, tone),
                );
                right -= 8.0;
            }
            let indent = if mark { 16.0 } else { 0.0 };
            if mark {
                ui.painter().circle_filled(
                    Pos2::new(line.left() + 4.0, line.center().y),
                    4.0,
                    p.attention,
                );
            }
            let left = line.left() + indent;
            let named = galley_at(
                ui.painter(),
                Pos2::new(left, line.center().y),
                elided(ui.painter(), title, theme::medium(12.0), p.fg, right - left),
            );
            if low {
                // As much of it as one line holds: its first sentence.
                let first = detail.split_inclusive(". ").next().unwrap_or(detail).trim();
                let from = named.right() + 8.0;
                if right - from > 40.0 {
                    galley_at(
                        ui.painter(),
                        Pos2::new(from, line.center().y),
                        elided(
                            ui.painter(),
                            first,
                            theme::regular(11.5),
                            p.secondary,
                            right - from,
                        ),
                    );
                }
            }
            // What is painted is read out as well: all of it, where one
            // line shows only its beginning.
            let words = Rect::from_min_max(line.min, Pos2::new(right, line.bottom()));
            let said = ui.interact(
                words,
                Id::new(("project-pinned-words", title)),
                Sense::hover(),
            );
            said.widget_info(|| {
                let text = if low {
                    format!("{title}. {detail}")
                } else {
                    title.to_owned()
                };
                WidgetInfo::labeled(WidgetType::Label, true, text)
            });
            if low {
                said.on_hover_text(format!("{title}\n{detail}"));
                return;
            }
            ui.horizontal_top(|ui| {
                ui.add_space(indent);
                ui.vertical(|ui| {
                    ui.set_width(width - indent);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(detail)
                                .font(theme::regular(11.5))
                                .color(p.secondary),
                        )
                        .wrap(),
                    );
                });
            });
        });
    ui.add_space(4.0);
    pressed
}

/// An agent where it is read out, after the word that says which of its
/// two controls it is: its number, its CLI, its state and what the lead
/// called it.
fn agent_words(member: &Member) -> String {
    format!(
        "{}, {}, {}: {}",
        member.pane,
        super::agents::kind_name(member.kind),
        member.state,
        member.title
    )
}
/// The mark that leads an agent, on its pill and in the list.
fn agent_mark(painter: &egui::Painter, centre: Pos2, p: Palette, member: &Member) {
    if member.ready {
        // Something to look at, which is not a wait.
        painter.circle_filled(centre, 4.0, p.accent);
    } else {
        chat::mark(painter, centre, p, member.waits, member.works);
    }
}
/// The order agents are shown in: those that wait for a person, then those
/// at work, then the rest, each by number.
fn ordered(members: &[Member]) -> Vec<&Member> {
    let mut order: Vec<&Member> = members.iter().collect();
    order.sort_by_key(|member| {
        let group = if member.waits {
            0
        } else if member.works {
            1
        } else {
            2
        };
        (group, member.pane)
    });
    order
}

/// An agent's row in the list of all of them. Its pill in the strip is
/// "Agent …" where names are read out, so the row is "Listed agent …":
/// one name never stands for two controls.
fn member(ui: &mut Ui, p: Palette, rect: Rect, member: &Member, actions: &mut Vec<Action>) {
    let response = ui
        .interact(
            rect,
            egui::Id::new(("project-agent", member.pane)),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    let kind = super::agents::kind_name(member.kind);
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!("Listed agent {}", agent_words(member)),
        )
    });
    let (first, second) = (rect.top() + 13.0, rect.top() + 29.0);
    let age = member.elapsed.map(elapsed_label).unwrap_or_default();
    let age = ui
        .painter()
        .layout_no_wrap(age, theme::regular(11.0), p.muted);
    let age_left = rect.right() - 8.0 - age.size().x;
    let left = rect.left() + 26.0;
    // Its pull requests open their page; they are claimed after the row,
    // which lies under them.
    let chips = PullRequestChips::layout(
        ui,
        egui::Id::new(("project-agent-pull-request", member.pane)),
        &member.pull_requests,
        (left + 64.0, age_left - 4.0, second),
        p,
    );
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if response.is_pointer_button_down_on() {
        painter.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 7, p.pressed);
    } else if response.hovered() || chips.hovered() {
        painter.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 7, p.hover);
    }
    if response.has_focus() {
        focus_ring(&painter, rect.shrink(1.0), 7, p);
    }
    agent_mark(&painter, Pos2::new(rect.left() + 13.0, first), p, member);
    painter.galley(
        Pos2::new(age_left, second - age.size().y * 0.5),
        age,
        p.muted,
    );
    let state = painter.layout_no_wrap(
        member.state.clone(),
        theme::regular(11.5),
        if member.waits {
            p.attention
        } else if member.ready {
            p.fg
        } else {
            p.muted
        },
    );
    // The state gives way to the name in a narrow panel, never the reverse.
    let room = (rect.right() - 8.0 - left).max(0.0);
    let state_width = if state.size().x + 60.0 <= room {
        state.size().x + 8.0
    } else {
        0.0
    };
    let name = elided(
        &painter,
        &format!("{} {}", member.pane, member.title),
        theme::medium(12.0),
        p.fg,
        room - state_width,
    );
    galley_at(&painter, Pos2::new(left, first), name);
    if state_width > 0.0 {
        let at = Pos2::new(rect.right() - 8.0 - state.size().x, first);
        galley_at(&painter, at, state);
    }
    // Which CLI it is and the branch it works on, as far as there is room
    // before its pull requests.
    let about = match &member.branch {
        Some(branch) => format!("{kind} · {branch}"),
        None => kind.to_owned(),
    };
    let about = elided(
        &painter,
        &about,
        theme::regular(11.0),
        p.muted,
        (chips.left - 6.0 - left).max(0.0),
    );
    galley_at(&painter, Pos2::new(left, second), about);
    chips.paint(&painter, p, actions);
    if response.clicked() {
        actions.push(Action::OpenAgent(member.pane, member.generation));
    }
    // No menu of its own here: one popup shows at a time, and a menu would
    // take the place of the list its row is in. The agent's pill has it.
    response.on_hover_text(match &member.branch {
        Some(branch) => format!("{kind} · {} · {branch}", member.detail),
        None => format!("{kind} · {}", member.detail),
    });
}

/// How wide each pill is where `room` holds them all: as wide as its words
/// up to the widest a pill gets, then all narrowed alike while they do not
/// fit, down to a mark and a number. Narrower than that they scroll.
/// `sizes` are each pill's least width and the one its words ask for.
fn pill_widths(sizes: &[(f32, f32)], room: f32) -> Vec<f32> {
    let gaps = PILL_GAP * sizes.len().saturating_sub(1) as f32;
    let capped = |cap: f32| -> Vec<f32> {
        sizes
            .iter()
            .map(|&(least, asked)| {
                let width = asked.min(cap).max(least);
                if width - least < PILL_NAME {
                    least
                } else {
                    width
                }
            })
            .collect()
    };
    let fits = |widths: &[f32]| widths.iter().sum::<f32>() + gaps <= room;
    let whole = capped(PILL_MAX);
    if fits(&whole) {
        return whole;
    }
    let (mut low, mut high) = (0.0, PILL_MAX);
    for _ in 0..12 {
        let cap = (low + high) * 0.5;
        if fits(&capped(cap)) {
            low = cap;
        } else {
            high = cap;
        }
    }
    capped(low)
}

/// What a pill has no room to say, a rest of the pointer away: the agent's
/// number and name, its CLI and branch, and what it is doing for how long.
fn pill_card(ui: &mut Ui, p: Palette, member: &Member) {
    ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
    let line = |ui: &mut Ui, text: String, font, color| {
        ui.label(egui::RichText::new(text).font(font).color(color));
    };
    line(
        ui,
        format!("{} {}", member.pane, member.title),
        theme::medium(12.0),
        p.fg,
    );
    let kind = super::agents::kind_name(member.kind);
    line(
        ui,
        match &member.branch {
            Some(branch) => format!("{kind} · {branch}"),
            None => kind.to_owned(),
        },
        theme::regular(11.5),
        p.muted,
    );
    // A wait is said exactly: what it waits for.
    let mut doing = if member.waits {
        member.detail.clone()
    } else {
        member.state.clone()
    };
    if let Some(elapsed) = member.elapsed {
        doing = format!("{doing} · {}", elapsed_label(elapsed));
    }
    let color = if member.waits {
        p.attention
    } else {
        p.secondary
    };
    line(ui, doing, theme::regular(11.5), color);
}

/// One agent of the strip in `rect`: its mark, its number and as much of
/// its name as there is room for. It is pressed as a tab is, and one whose
/// terminal is a tab is filled as the tab in view is.
fn pill(ui: &mut Ui, p: Palette, rect: Rect, member: &Member, actions: &mut Vec<Action>) {
    let response = ui
        .interact(
            rect,
            Id::new(("project-agent-pill", member.pane)),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!("Agent {}", agent_words(member)),
        )
    });
    // The keyboard reaches a pill that is scrolled away.
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let open = member.can_background;
        let down = response.is_pointer_button_down_on();
        let hovered = response.hovered();
        // An agent that waits is seen at a glance, in the tint of the rows
        // about what needs the person.
        let fill = if member.waits {
            let strength = if down {
                0.22
            } else if hovered {
                0.16
            } else {
                0.10
            };
            Some(theme::tint(p.attention, strength))
        } else if down {
            Some(p.pressed)
        } else if hovered {
            Some(theme::tint(p.fg, if open { 0.11 } else { 0.045 }))
        } else {
            open.then(|| theme::tint(p.fg, 0.08))
        };
        if let Some(fill) = fill {
            painter.rect_filled(rect, 7, fill);
        }
        if response.has_focus() {
            focus_ring(painter, rect.shrink(2.0), 5, p);
        }
        let centre = rect.center().y;
        agent_mark(painter, Pos2::new(rect.left() + 10.0, centre), p, member);
        let ink = if open || hovered || member.waits || response.has_focus() {
            p.fg
        } else {
            p.secondary
        };
        let font = theme::medium(11.5);
        let number = painter.layout_no_wrap(member.pane.to_string(), font.clone(), ink);
        let named = galley_at(painter, Pos2::new(rect.left() + 18.0, centre), number);
        let room = rect.right() - 6.0 - named.right() - 4.0;
        if room >= PILL_NAME - 4.0 {
            galley_at(
                painter,
                Pos2::new(named.right() + 4.0, centre),
                elided(painter, &member.title, font, ink, room),
            );
        }
    }
    if response.clicked() {
        actions.push(Action::OpenAgent(member.pane, member.generation));
    }
    super::agents::row_menu(
        &response,
        p,
        (member.pane, member.generation),
        member.can_background,
        actions,
    );
    response.on_hover_ui(|ui| pill_card(ui, p, member));
}

/// Says at a clipped edge of the strip that it goes on: what is under it
/// fades into `surface`.
fn fade(painter: &egui::Painter, rect: Rect, surface: Color32, leading: bool) {
    let (left, right) = if leading {
        (surface, Color32::TRANSPARENT)
    } else {
        (Color32::TRANSPARENT, surface)
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// The list of all the agents, as the popover over the chat holds it: how
/// they stand in words, then a row for each in `order`, scrolling where
/// they are more than `tall` holds. A press on a row puts the list away.
fn roster(
    ui: &mut Ui,
    p: Palette,
    order: &[&Member],
    (width, tall): (f32, f32),
    actions: &mut Vec<Action>,
) {
    let margin = Frame::popup(ui.style()).total_margin().sum().x;
    menu_layout(ui, width - margin);
    let (_, head) = ui.allocate_space(vec2(ui.available_width(), HEADING));
    let words = tally(order.iter().copied());
    galley_at(
        ui.painter(),
        Pos2::new(head.left() + 9.0, head.center().y),
        elided(
            ui.painter(),
            &words,
            theme::medium(11.5),
            p.secondary,
            head.width() - 18.0,
        ),
    );
    let before = actions.len();
    egui::ScrollArea::vertical()
        .id_salt("project-roster")
        .max_height((tall - HEADING).max(MEMBER))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for item in order {
                let (_, rect) = ui.allocate_space(vec2(ui.available_width(), MEMBER));
                if ui.is_rect_visible(rect) {
                    member(ui, p, rect, item, actions);
                }
            }
        });
    if actions.len() > before {
        ui.close();
    }
}

/// The strip of the project's agents in `rect`: a pill for each, pressed as
/// a tab is, and at its trailing edge the control that lists them all in a
/// popover over the chat. It is one row however many they are: pills that
/// do not fit are narrowed, then scrolled, and the list never takes room
/// from the chat. `surface` is what the tab is drawn on and `tall` the most
/// the list may take.
fn agent_strip(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    members: &[Member],
    (surface, tall): (Color32, f32),
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let order = ordered(members);
    let font = theme::medium(11.5);
    let wide = |ui: &Ui, text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };
    // The control that lists them keeps its place while the pills scroll.
    let count = members.len().to_string();
    let control = Rect::from_min_max(
        Pos2::new(
            rect.right() - (26.0 + wide(ui, &count) + 16.0),
            rect.center().y - PILL * 0.5,
        ),
        Pos2::new(rect.right(), rect.center().y + PILL * 0.5),
    );
    let pills = Rect::from_min_max(rect.min, Pos2::new(control.left() - 6.0, rect.bottom()));
    let sizes: Vec<(f32, f32)> = order
        .iter()
        .map(|member| {
            let least = 18.0 + wide(ui, &member.pane.to_string()) + 6.0;
            let asked = least + 4.0 + wide(ui, &member.title);
            (least, asked.min(PILL_MAX).max(least))
        })
        .collect();
    let widths = pill_widths(&sizes, pills.width());
    let total = widths.iter().sum::<f32>() + PILL_GAP * widths.len().saturating_sub(1) as f32;
    let scrolled = place(
        ui,
        pills,
        Layout::left_to_right(Align::Center),
        "project-agent-pills",
        |ui| {
            // No bar: it would take the row's height. A wheel, a trackpad
            // or a drag moves it, and its clipped edge fades. A wheel that
            // turns only up and down moves it as well. The fade is drawn
            // here, into the surface the tab is on, and not the toolkit's.
            ui.style_mut().always_scroll_the_only_direction = true;
            ui.spacing_mut().scroll.fade.strength = 0.0;
            egui::ScrollArea::horizontal()
                .id_salt("project-agent-pills-scroll")
                .auto_shrink([false, false])
                .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
                .scroll_source(ScrollSource::MOUSE_WHEEL | ScrollSource::DRAG)
                .show(ui, |ui| {
                    let (_, row) = ui.allocate_space(vec2(total, pills.height()));
                    let mut left = row.left();
                    for (member, width) in order.iter().zip(&widths) {
                        let at = Rect::from_min_size(
                            Pos2::new(left, row.center().y - PILL * 0.5),
                            vec2(*width, PILL),
                        );
                        pill(ui, p, at, member, actions);
                        left += width + PILL_GAP;
                    }
                })
        },
    );
    // As much as is clipped at an edge, up to the fade's width: nothing
    // appears at once as the strip begins to move.
    let before = scrolled.state.offset.x;
    let after = scrolled.content_size.x - scrolled.inner_rect.width() - before;
    for (hidden, leading) in [(before, true), (after, false)] {
        if hidden > 0.5 {
            let left = if leading {
                pills.left()
            } else {
                pills.right() - FADE
            };
            let edge = Rect::from_min_max(
                Pos2::new(left, pills.top()),
                Pos2::new(left + FADE, pills.bottom()),
            );
            let surface = surface.gamma_multiply((hidden / FADE).min(1.0));
            fade(ui.painter(), edge, surface, leading);
        }
    }

    let words = summary(members);
    let response = ui
        .interact(control, Id::new("project-agents"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("Agents, {words}")));
    let listing = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_expanded(listing);
    });
    if ui.is_rect_visible(control) {
        let painter = ui.painter();
        if response.is_pointer_button_down_on() {
            painter.rect_filled(control, 7, p.pressed);
        } else if listing {
            painter.rect_filled(control, 7, theme::tint(p.fg, 0.08));
        } else if response.hovered() {
            painter.rect_filled(control, 7, theme::tint(p.fg, 0.045));
        }
        if response.has_focus() {
            focus_ring(painter, control.shrink(2.0), 5, p);
        }
        let ink = if listing || response.hovered() || response.has_focus() {
            p.fg
        } else {
            p.secondary
        };
        let centre = control.center().y;
        icons::paint(
            painter,
            Rect::from_center_size(Pos2::new(control.left() + 13.0, centre), Vec2::splat(13.0)),
            Icon::Agents,
            ink,
        );
        galley_at(
            painter,
            Pos2::new(control.left() + 23.0, centre),
            painter.layout_no_wrap(count, font, ink),
        );
        icons::paint(
            painter,
            Rect::from_center_size(Pos2::new(control.right() - 10.0, centre), Vec2::splat(9.0)),
            Icon::ChevronDown,
            ink,
        );
    }
    // Asked for by name, the list opens as if its control were pressed.
    let command = if std::mem::take(&mut state.show_agents) {
        Some(egui::SetOpenCommand::Bool(true))
    } else {
        response.clicked().then_some(egui::SetOpenCommand::Toggle)
    };
    // Under the whole strip and as wide: it lies over the chat and moves
    // nothing of it.
    let under = ui
        .ctx()
        .layer_transform_to_global(ui.layer_id())
        .map_or(rect, |to_global| to_global * rect);
    egui::Popup::menu(&response)
        .open_memory(command)
        .anchor(egui::PopupAnchor::ParentRect(under))
        .align(egui::RectAlign::BOTTOM_START)
        .gap(3.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| roster(ui, p, &order, (rect.width(), tall), actions));
    if !listing {
        response.on_hover_text(words);
    }
}

/// How long a turn has run, as the line under the composer says it.
fn running_words(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds} s")
    } else {
        format!("{} min {} s", seconds / 60, seconds % 60)
    }
}

fn project(
    ui: &mut Ui,
    p: Palette,
    inner: Rect,
    project: &Project,
    (composing, fit): (bool, Fit),
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    // A sheet's title row is the header.
    let mut top = inner.top();
    if fit == Fit::Tab {
        let row = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER));
        header(ui, p, row, project, state, actions);
        top += HEADER;
    }
    if state.segment != Segment::Chat {
        // No field there asks for the chat's keyboard; a request for it is
        // spent, and so is one for the list of agents.
        state.focus = false;
        state.show_agents = false;
        let body = Rect::from_min_max(Pos2::new(inner.left(), top), inner.max);
        details::show(ui, p, body, project, composing, state, actions);
        return;
    }
    state.deleting = None;
    state.removing = None;
    if state
        .custom
        .as_ref()
        .is_some_and(|custom| custom.owner.is_some())
    {
        state.custom = None;
    }

    // The composer keeps its place at the bottom and grows upwards.
    let draft = state.drafts.entry(project.id).or_default();
    let field_height = chat::editor_height(ui, draft, inner.width() - 2.0 * CONTROLS - 6.0, (1, 6))
        .min((inner.height() * 0.4).max(31.0));
    let hint = Rect::from_min_max(Pos2::new(inner.left(), inner.bottom() - HINT), inner.max);
    let field = Rect::from_min_max(
        Pos2::new(inner.left(), hint.top() - field_height),
        Pos2::new(inner.right(), hint.top()),
    );
    // The files that go with the message wait over the field: a few rows
    // of them, one in a low window, and the rest a scroll away.
    let files = state.attached.get(&project.id).map_or(0, Vec::len);
    let (_, _, tall) = chat::chip_grid(inner.width(), files);
    let rows = if fit == Fit::Low { 1.0 } else { 3.0 };
    let strip = Rect::from_min_max(
        Pos2::new(
            inner.left(),
            field.top() - 6.0 - tall.min(rows * (chat::CHIP + 4.0) - 4.0),
        ),
        Pos2::new(inner.right(), field.top() - 6.0),
    );
    let above = if files == 0 { field.top() } else { strip.top() };
    let middle = Rect::from_min_max(
        Pos2::new(inner.left(), top),
        Pos2::new(inner.right(), above - 6.0),
    );

    let mut column = ui.new_child(
        UiBuilder::new()
            .id_salt(("project-body", project.id))
            .max_rect(middle)
            .layout(Layout::top_down(Align::Min)),
    );
    column.set_clip_rect(middle.intersect(ui.clip_rect()));
    column.spacing_mut().item_spacing = Vec2::ZERO;
    // What needs the person stays above the chat, then the strip of the
    // agents, which is one row and never scrolls away. What waits takes no
    // more than its share and scrolls by itself: the chat keeps a few lines
    // however much it is. The list of all the agents lies over the chat
    // and takes nothing from it.
    // The person's own pause is said plainly; one Neptune made is among
    // what needs them, with its reason.
    let banner = project.paused
        && !project
            .needs
            .iter()
            .any(|need| need.remedy == Remedy::Resume);
    let waiting = !project.needs.is_empty() || banner;
    if project.members.is_empty() {
        // No strip, and so no list to open later.
        state.show_agents = false;
    }
    if waiting || !project.members.is_empty() {
        let share = (middle.height() * PINNED)
            .min(middle.height() - TRANSCRIPT)
            .max(ROW + MEMBER)
            .min(middle.height());
        let line = if project.members.is_empty() { 0.0 } else { ROW };
        let low = fit == Fit::Low;
        column.scope(|ui| {
            // Its bar shows without the pointer over it: a row that is cut
            // off is seen to be reachable.
            let scroll = &mut ui.spacing_mut().scroll;
            scroll.dormant_handle_opacity = scroll.active_handle_opacity;
            if waiting {
                egui::ScrollArea::vertical()
                    .id_salt("project-pinned")
                    .max_height((share - line).max(ROW))
                    .min_scrolled_height(ROW)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for item in project.needs {
                            let action = remedy(project.id, item);
                            let offered = action
                                .as_ref()
                                .map(|(label, name, _)| (*label, name.as_str(), Tone::Accent));
                            if pinned_row(ui, p, (&item.title, &item.detail), true, low, offered)
                                && let Some((_, _, action)) = action
                            {
                                actions.push(action);
                            }
                        }
                        if banner
                            && pinned_row(
                                ui,
                                p,
                                (
                                    "Paused",
                                    "Nothing starts by itself. Your messages still reach the \
                                     lead.",
                                ),
                                false,
                                low,
                                Some(("Resume", "Resume project", Tone::Plain)),
                            )
                        {
                            actions.push(Action::Project(Event::SetPaused(project.id, false)));
                        }
                    });
            }
        });
        if !project.members.is_empty() {
            let (_, row) = column.allocate_space(vec2(column.available_width(), ROW));
            // A sheet is drawn on another surface than the panel.
            let surface = if fit == Fit::Tab {
                p.chrome
            } else {
                p.elevated
            };
            agent_strip(
                &mut column,
                p,
                row,
                project.members,
                (surface, inner.height() * ROSTER),
                state,
                actions,
            );
        }
        let (_, rule) = column.allocate_space(vec2(column.available_width(), 1.0));
        column
            .painter()
            .line_segment([rule.left_center(), rule.right_center()], p.hairline());
    }
    let mut earlier = false;
    egui::ScrollArea::vertical()
        .id_salt("project-chat")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(&mut column, |ui| {
            let width = ui.available_width() - 12.0;
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    earlier = chat::transcript(ui, p, &project.chat, actions);
                });
            });
        });
    if earlier {
        state.shown += chat::PAGE;
        if project.chat.more {
            actions.push(Action::Project(Event::Earlier(project.id)));
        }
    }

    if project.locked {
        // A field that takes nothing does not take the keyboard later.
        state.focus = false;
    } else {
        let area = Rect::from_min_max(middle.min, field.max);
        state.chat_area = Some((project.id, area));
        if state.dropping {
            drop_target(ui, p, area);
        }
    }
    if files > 0 {
        let attached = state.attached.entry(project.id).or_default();
        let pressed = place(
            ui,
            strip,
            Layout::top_down(Align::Min),
            ("project-attached", project.id),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("project-attached-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let (_, rect) = ui.allocate_space(vec2(strip.width(), tall));
                        chat::chips(ui, p, rect, project.id, attached, (false, !project.locked))
                    })
                    .inner
            },
        );
        match pressed {
            Some(chat::Chip::Remove(file)) => {
                attached.remove(file);
            }
            Some(chat::Chip::Open(file)) => actions.push(Action::Explorer(
                super::explorer::Event::Open(attached[file].path.clone().into()),
            )),
            None => {}
        }
    }
    let files = state.attached.get(&project.id).map_or(0, Vec::len);
    let draft = state.drafts.entry(project.id).or_default();
    // Files alone are a message: Enter in the field sends them, which the
    // field itself would not ask for without words.
    let bare = files > 0
        && !project.locked
        && draft.trim().is_empty()
        && !composing
        && ui.memory(|memory| memory.has_focus(chat::composer_id()))
        && chat::take_enter(ui);
    let mut entered = false;
    ui.add_enabled_ui(!project.locked, |ui| {
        entered = bare
            | chat::editor(
                ui,
                p,
                field,
                Editor {
                    id: chat::composer_id(),
                    text: draft,
                    hint: if project.locked {
                        "This project is read-only"
                    } else {
                        "Message the lead…"
                    },
                    label: "Message the lead",
                    focus: &mut state.focus,
                    composing,
                    trailing: 2.0 * CONTROLS,
                    newline: false,
                },
            )
            .1;
    });
    let ready = !project.locked && (!draft.trim().is_empty() || files > 0);
    // A project that takes nothing has no control to send with.
    if !project.locked {
        let control = Rect::from_min_size(
            Pos2::new(field.right() - 29.0, field.bottom() - 28.0),
            Vec2::splat(24.0),
        );
        // While a turn runs the control stops it; a message typed
        // meanwhile is sent with Enter and waits its turn.
        let stop = project.busy && !ready;
        if chat::attach_button(ui, p, control.translate(vec2(-CONTROLS, 0.0))).clicked() {
            actions.push(Action::Project(Event::Attach(project.id)));
        }
        let pressed = chat::send_button(ui, p, control, stop, ready).clicked();
        if pressed && stop {
            actions.push(Action::Project(Event::Stop(project.id)));
        } else if (pressed || entered) && ready {
            actions.push(Action::Project(Event::Send {
                project: project.id,
                text: std::mem::take(draft).trim().to_owned(),
                attachments: state.attached.remove(&project.id).unwrap_or_default(),
            }));
            // The pointer took the keyboard to the control; the next
            // message is written where the last one was.
            state.focus |= pressed;
        }
    }
    footer(ui, p, hint, project, state);
}

/// Says that files held over the chat are attached when they are let go, as
/// a terminal says it of the files held over it.
fn drop_target(ui: &Ui, p: Palette, area: Rect) {
    let painter = ui.painter().clone().with_layer_id(egui::LayerId::new(
        egui::Order::Foreground,
        Id::new("project-drop-target"),
    ));
    painter.rect_filled(
        area,
        theme::metrics::ROW_RADIUS,
        theme::tint(p.accent, 0.16),
    );
    painter.rect_stroke(
        area,
        theme::metrics::ROW_RADIUS,
        egui::Stroke::new(1.5, theme::tint(p.accent, 0.9)),
        egui::StrokeKind::Inside,
    );
    let label = elided(
        &painter,
        "Drop to attach for the lead",
        theme::medium(12.0),
        p.fg,
        area.width() - 24.0,
    );
    let chip = Rect::from_center_size(area.center(), label.size() + vec2(20.0, 12.0));
    painter.rect_filled(chip, theme::metrics::ROW_RADIUS, p.control);
    galley_at(
        &painter,
        Pos2::new(chip.left() + 10.0, chip.center().y),
        label,
    );
}

/// The line under the composer: which CLI leads and what it runs with,
/// which opens the settings, and at the trailing edge what its turn is
/// doing. How long the turn has taken sits where the count of waiting
/// messages does, so nothing of the chat moves as it is counted.
fn footer(ui: &mut Ui, p: Palette, hint: Rect, project: &Project, state: &mut State) {
    let centre = hint.center().y + 1.0;
    let running = project.elapsed.map(running_words);
    let trailing = match (project.queued, running) {
        (0, None) => None,
        (0, Some(running)) => Some(format!("Lead at work · {running}")),
        (queued, None) => Some(format!("Queued · {queued}")),
        (queued, Some(running)) => Some(format!("Queued · {queued} · {running}")),
    };
    // What the turn is doing outranks the lead's name where both do not fit.
    let mut right = hint.right() - 4.0;
    if let Some(trailing) = trailing {
        let said = ui
            .painter()
            .layout_no_wrap(trailing, theme::medium(11.0), p.secondary);
        right -= said.size().x;
        ui.painter().galley(
            Pos2::new(right, centre - said.size().y * 0.5),
            said,
            p.secondary,
        );
        right -= 10.0;
    }
    let left = hint.left() + 4.0;
    if project.locked {
        galley_at(
            ui.painter(),
            Pos2::new(left, centre),
            elided(
                ui.painter(),
                "Nothing here is changed",
                theme::regular(11.0),
                p.muted,
                right - left,
            ),
        );
        return;
    }
    // The model its CLI named when it last started stands in where none
    // was chosen.
    let words = lead_words(
        project.lead,
        project
            .settings
            .lead
            .model
            .as_deref()
            .or(project.lead_model),
        project.settings.lead.effort.as_deref(),
        project.catalog.of(Setting::Lead, project.lead),
    );
    let font = theme::regular(11.0);
    let chip = elided(ui.painter(), &words, font.clone(), p.muted, right - left);
    if chip.size().x < 20.0 {
        return;
    }
    let rect = Rect::from_min_size(Pos2::new(left, centre - chip.size().y * 0.5), chip.size());
    let response = ui
        .interact(
            rect.expand2(vec2(3.0, 2.0)),
            Id::new("project-lead-chip"),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Lead settings"));
    if response.has_focus() {
        focus_ring(ui.painter(), rect.expand2(vec2(3.0, 1.0)), 5, p);
    }
    let chip = if response.hovered() || response.has_focus() {
        elided(ui.painter(), &words, font, p.secondary, right - left)
    } else {
        chip
    };
    ui.painter().galley(rect.min, chip, Color32::PLACEHOLDER);
    if response.clicked() {
        state.show(Segment::Settings);
    }
    response.on_hover_text("What the lead and its agents run with");
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
    fitted(ui, rect, p, view, Fit::Tab, state, actions);
}
fn fitted(
    ui: &mut Ui,
    rect: Rect,
    p: Palette,
    view: &View,
    fit: Fit,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let mut child = ui.new_child(UiBuilder::new().id_salt("project").max_rect(rect));
    child.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(view.window));
    child.multiply_opacity(view.reveal);
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 6.0, rect.top()),
        Pos2::new(rect.right() - 8.0, rect.bottom() - 8.0),
    );
    // Where there is no field, a request for the keyboard is spent: it must
    // not wait for a field that comes into view later. Nor does one for the
    // list of agents wait for a project.
    if !matches!(view.body, Body::Empty { .. } | Body::Project(_)) {
        state.focus = false;
    }
    if !matches!(view.body, Body::Project(_)) {
        state.show_agents = false;
    }
    match &view.body {
        Body::Nothing => centered_note(&child, inner, p, "Open a workspace to start a project."),
        Body::Remote => centered_note(
            &child,
            inner,
            p,
            "Projects run on this computer.\nThis workspace is connected over SSH.",
        ),
        Body::Unsupported => centered_note(
            &child,
            inner,
            p,
            "Projects are not available on this system yet.",
        ),
        Body::Empty {
            workspace,
            directory,
            starting,
            reopen,
            leads,
            catalog,
        } => creation::show(
            &mut child,
            p,
            inner,
            creation::Form {
                workspace: *workspace,
                directory,
                starting: *starting,
                reopen: *reopen,
                leads: *leads,
                catalog,
                sheet: fit != Fit::Tab,
            },
            view.composing,
            state,
            actions,
        ),
        Body::Project(item) => project(
            &mut child,
            p,
            inner,
            item,
            (view.composing, fit),
            state,
            actions,
        ),
    }
}

/// The tab's body in a sheet, for a window too narrow for the panel: the
/// same parts, with the chat scrolling above a composer that keeps its
/// place. Its title row is the project's header, so no title stands over
/// the project's name. Returns whether the person closed it.
pub fn show_sheet(
    ctx: &egui::Context,
    p: Palette,
    view: &View,
    state: &mut State,
    actions: &mut Vec<Action>,
) -> bool {
    let screen = ctx.content_rect();
    // As tall as the window allows, so the composer is never below its edge.
    let height = (screen.height() - 52.0 - 24.0).clamp(120.0, 620.0);
    let fit = if height < LOW { Fit::Low } else { Fit::Sheet };
    let output = sheet(ctx, p, "Project", SHEET, SheetPlacement::Center, |ui| {
        let close = match &view.body {
            Body::Project(project) => {
                // Its controls come before the one that closes the sheet,
                // so Tab reaches them first.
                let (_, row) = ui.allocate_space(vec2(ui.available_width(), 52.0));
                let titled = Rect::from_min_max(
                    Pos2::new(row.left() + 14.0, row.top()),
                    Pos2::new(row.right() - 44.0, row.bottom()),
                );
                header(ui, p, titled, project, state, actions);
                place(
                    ui,
                    Rect::from_center_size(
                        Pos2::new(row.right() - 26.0, row.center().y),
                        Vec2::splat(28.0),
                    ),
                    Layout::left_to_right(Align::Center),
                    "project-sheet-close",
                    |ui| icons::button(ui, Icon::Close, "Close project").clicked(),
                )
            }
            Body::Empty { .. } => sheet_header(ui, p, "New project", Some("Close project")),
            _ => sheet_header(ui, p, "Project", Some("Close project")),
        };
        let (_, rect) = ui.allocate_space(vec2(ui.available_width(), height));
        let rect = Rect::from_min_max(
            Pos2::new(rect.left() + 8.0, rect.top()),
            Pos2::new(rect.right() - 6.0, rect.bottom() - 4.0),
        );
        fitted(ui, rect, p, view, fit, state, actions);
        close
    });
    output.inner || output.backdrop_clicked
}
