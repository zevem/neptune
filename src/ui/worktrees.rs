//! An agent in a git worktree of its own: the sheet that asks only for a
//! branch, and the one that says what removing a worktree takes with it.
use super::helpers::{
    ButtonKind, SheetPlacement, button, compact_path, elided, galley_at, padded, place,
    section_label, segmented, sheet, sheet_header, text_field,
};
use super::{Action, UiState};
use crate::{
    icons::{self, Icon},
    runtime::worktrees::{Condition, Existing, Repository, branch_name},
    theme::{self, Palette},
};
use eframe::egui::{
    self, Align, CursorIcon, Id, Layout, Pos2, Rect, Sense, Ui, Vec2, WidgetInfo, WidgetType, vec2,
};
use neptune_model::{AgentKind, PaneId, Worktree};

/// What a tab shows of the worktree its terminal is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub branch: String,
    /// The branch it was merged into, once nothing is left uncommitted.
    pub merged: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Event {
    /// Ask for a branch, for a tab beside this terminal.
    New(PaneId),
    Create {
        branch: String,
        agent: AgentKind,
    },
    /// Start an agent in a new tab of a worktree made earlier.
    Open(Worktree, AgentKind),
    /// Ask before removing the worktree of this terminal.
    RemoveOf(PaneId),
    /// Ask before removing a worktree that may have no terminal.
    Remove(Worktree),
    ConfirmRemove {
        discard: bool,
    },
}

/// The repository of the terminal the sheet was opened from.
#[derive(Default)]
pub enum Probe {
    #[default]
    Looking,
    Found(Repository),
    Failed(String),
}

/// The worktree a sheet asks about, and what was found in it.
pub struct Removal {
    pub worktree: Worktree,
    pub condition: Option<Result<Condition, String>>,
}

pub struct State {
    /// The branch as typed.
    pub branch: String,
    pub agent: AgentKind,
    pub repository: Probe,
    /// git is making the worktree.
    pub creating: bool,
    /// Why the last attempt made nothing.
    pub error: Option<String>,
    pub removal: Option<Removal>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            branch: String::new(),
            agent: AgentKind::Claude,
            repository: Probe::default(),
            creating: false,
            error: None,
            removal: None,
        }
    }
}

pub fn agent_name(kind: AgentKind) -> &'static str {
    super::agents::kind_name(kind)
}

fn note(ui: &mut Ui, text: impl Into<String>, ink: egui::Color32) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::regular(12.0))
                .color(ink),
        )
        .wrap()
        .selectable(false),
    );
}

/// What Create would do with the name as typed, or why it cannot.
fn outcome(state: &State) -> (String, bool) {
    if let Some(error) = &state.error {
        return (error.clone(), true);
    }
    let repository = match &state.repository {
        Probe::Looking => return ("Looking for the repository…".into(), false),
        Probe::Failed(error) => return (error.clone(), true),
        Probe::Found(repository) => repository,
    };
    let project = repository
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| compact_path(&repository.root));
    let from = if repository.base == "HEAD" {
        "the current commit".to_owned()
    } else {
        repository.base.clone()
    };
    if state.branch.trim().is_empty() {
        return (
            format!(
                "A new branch from {from}, checked out in a folder of its own beside {project}. \
                 The agent starts there and the tab takes the branch's name."
            ),
            false,
        );
    }
    match branch_name(&state.branch) {
        Err(problem) => (problem.to_owned(), true),
        Ok(name) => {
            if repository
                .worktrees
                .iter()
                .any(|existing| existing.worktree.branch == name)
            {
                (
                    format!("Starts another agent in the worktree {name} already has."),
                    false,
                )
            } else if repository.branches.contains(&name) {
                (
                    format!("Checks out the existing branch {name} in a folder of its own."),
                    false,
                )
            } else {
                (format!("Creates the branch {name} from {from}."), false)
            }
        }
    }
}

/// One worktree made earlier: its branch and where it stands. Clicking it
/// opens it; the trailing control removes it.
fn existing_row(
    ui: &mut Ui,
    p: Palette,
    existing: &Existing,
    first: bool,
    agent: AgentKind,
    actions: &mut Vec<Action>,
) {
    let worktree = &existing.worktree;
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 40.0));
    let id = ui.id().with(("worktree-row", &worktree.path));
    let remove = Rect::from_center_size(
        Pos2::new(rect.right() - 22.0, rect.center().y),
        Vec2::splat(26.0),
    );
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            match &existing.merged {
                Some(into) => format!("Open worktree {}, merged into {into}", worktree.branch),
                None => format!("Open worktree {}", worktree.branch),
            },
        )
    });
    let remove_response = ui.interact(remove, id.with("remove"), Sense::click());
    remove_response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!("Remove worktree {}", worktree.branch),
        )
    });
    let painter = ui.painter();
    if !first {
        painter.line_segment(
            [
                Pos2::new(rect.left() + 14.0, rect.top()),
                Pos2::new(rect.right(), rect.top()),
            ],
            p.hairline(),
        );
    }
    if response.hovered() && !remove_response.hovered() {
        painter.rect_filled(rect.shrink(2.0), 8, p.hover);
    }
    if response.has_focus() {
        super::helpers::focus_ring(painter, rect.shrink(2.0), 8, p);
    }
    icons::paint(
        painter,
        Rect::from_center_size(
            Pos2::new(rect.left() + 20.0, rect.center().y),
            Vec2::splat(14.0),
        ),
        Icon::Branch,
        p.secondary,
    );
    // Only a finished one says so; the rest are known by their branch.
    let status = existing
        .merged
        .as_ref()
        .map(|into| format!("Merged into {into}"))
        .unwrap_or_default();
    let room = remove.left() - rect.left() - 44.0;
    let status = elided(
        painter,
        &status,
        theme::regular(11.5),
        p.accent,
        room * 0.55,
    );
    let name = elided(
        painter,
        &worktree.branch,
        theme::regular(13.0),
        p.fg,
        room - status.size().x - 10.0,
    );
    let name_width = name.size().x;
    galley_at(
        painter,
        Pos2::new(rect.left() + 36.0, rect.center().y),
        name,
    );
    galley_at(
        painter,
        Pos2::new(rect.left() + 46.0 + name_width, rect.center().y + 0.5),
        status,
    );
    if remove_response.hovered() {
        painter.rect_filled(remove.shrink(1.0), 7, theme::tint(p.fg, 0.1));
    }
    if remove_response.has_focus() {
        super::helpers::focus_ring(painter, remove.shrink(1.0), 7, p);
    }
    icons::paint(
        painter,
        remove.shrink(6.5),
        Icon::Trash,
        if remove_response.hovered() {
            p.fg
        } else {
            p.muted
        },
    );
    if remove_response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Remove worktree…")
        .clicked()
    {
        actions.push(Action::Worktree(Event::Remove(worktree.clone())));
    } else if response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(format!(
            "Start {} in {}",
            agent_name(agent),
            worktree.branch
        ))
        .clicked()
    {
        actions.push(Action::Worktree(Event::Open(worktree.clone(), agent)));
    }
}

/// Asks for a branch and nothing else: where the worktree goes and what it
/// starts from are decided, and said beneath the field.
pub fn new_agent(
    ctx: &egui::Context,
    p: Palette,
    state: &mut UiState,
    enter: bool,
    accepts_focus: impl Fn(&Ui) -> bool,
    actions: &mut Vec<Action>,
) {
    const TITLE: &str = "New agent in worktree";
    let mut create = enter;
    let mut cancel = false;
    let output = sheet(ctx, p, TITLE, 440.0, SheetPlacement::Center, |ui| {
        sheet_header(ui, p, TITLE, None);
        padded(ui, 20.0, |ui| {
            let width = ui.available_width();
            let before = state.worktree.branch.clone();
            let field = text_field(
                ui,
                p,
                Id::new("worktree-branch"),
                &mut state.worktree.branch,
                "Branch name, such as fix-login",
                "Branch name",
                width,
            );
            if state.overlay_focus && accepts_focus(ui) {
                field.request_focus();
                state.overlay_focus = false;
            }
            if state.worktree.branch != before {
                state.worktree.error = None;
            }
            ui.add_space(10.0);
            segmented(
                ui,
                p,
                "worktree-agent",
                &mut state.worktree.agent,
                &[
                    (AgentKind::Claude, agent_name(AgentKind::Claude)),
                    (AgentKind::Codex, agent_name(AgentKind::Codex)),
                ],
                width,
            );
            ui.add_space(12.0);
            let (text, problem) = outcome(&state.worktree);
            note(ui, text, if problem { p.red } else { p.secondary });
            if let Probe::Found(repository) = &state.worktree.repository
                && !repository.worktrees.is_empty()
            {
                ui.add_space(14.0);
                section_label(ui, p, "Worktrees");
                let agent = state.worktree.agent;
                let rows = repository.worktrees.len();
                egui::ScrollArea::vertical()
                    .max_height(40.0 * 4.5)
                    .min_scrolled_height(40.0 * rows.min(4) as f32)
                    .show(ui, |ui| {
                        super::helpers::group(ui, p, |ui, _| {
                            for (index, existing) in repository.worktrees.iter().enumerate() {
                                existing_row(ui, p, existing, index == 0, agent, actions);
                            }
                        });
                    });
            }
        });
        ui.add_space(4.0);
        let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 62.0));
        place(
            ui,
            rect.shrink2(vec2(20.0, 0.0)),
            Layout::right_to_left(Align::Center),
            "worktree-actions",
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let label = if state.worktree.creating {
                    "Creating…"
                } else {
                    "Create"
                };
                create |= button(ui, p, label, ButtonKind::Primary).clicked();
                cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
            },
        );
    });
    if cancel || output.backdrop_clicked {
        actions.push(Action::CloseOverlay);
    } else if create && !state.worktree.creating {
        match (
            &state.worktree.repository,
            branch_name(&state.worktree.branch),
        ) {
            (Probe::Found(_), Ok(branch)) => actions.push(Action::Worktree(Event::Create {
                branch,
                agent: state.worktree.agent,
            })),
            // Enter leaves a single-line field; give it back for a correction.
            _ => state.overlay_focus = true,
        }
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// What is said before a worktree is removed, the confirming verb, and
/// whether uncommitted work goes with it. `None` while nothing may be removed.
pub fn removal_copy(
    worktree: &Worktree,
    condition: Option<&Result<Condition, String>>,
) -> (String, Option<(&'static str, bool)>) {
    let branch = &worktree.branch;
    let condition = match condition {
        None => return ("Checking what it holds…".into(), None),
        Some(Err(error)) => return (format!("Neptune could not check it: {error}."), None),
        Some(Ok(condition)) => condition,
    };
    let closes = "Terminals open in it close.";
    if condition.uncommitted > 0 {
        let files = plural(
            condition.uncommitted,
            "file has changes that were",
            "files have changes that were",
        );
        let kept = if condition.merged.is_none() && condition.unmerged > 0 {
            format!(" The branch {branch} keeps what was committed.")
        } else {
            String::new()
        };
        return (
            format!(
                "{files} never committed. They are deleted with the folder and cannot be \
                 recovered. {closes}{kept}"
            ),
            Some(("Discard and remove", true)),
        );
    }
    let message = match (&condition.merged, condition.unmerged) {
        (Some(into), _) => format!(
            "{branch} was merged into {into}. Its folder and its local branch are removed. {closes}"
        ),
        (None, 0) => format!(
            "{branch} has no commits that {} lacks. Its folder and its local branch are removed. {closes}",
            condition.base
        ),
        (None, commits) => format!(
            "{} not in {} yet. The folder is removed and the branch {branch} keeps them. {closes}",
            plural(commits, "commit is", "commits are"),
            condition.base
        ),
    };
    (message, Some(("Remove", false)))
}

pub fn remove(ctx: &egui::Context, p: Palette, removal: &Removal, actions: &mut Vec<Action>) {
    let title = format!(
        "Remove worktree {}?",
        super::helpers::ellipsize(&removal.worktree.branch, 40)
    );
    let (message, confirming) = removal_copy(&removal.worktree, removal.condition.as_ref());
    let mut confirm = false;
    let mut cancel = false;
    let output = sheet(
        ctx,
        p,
        "Remove worktree",
        400.0,
        SheetPlacement::Center,
        |ui| {
            ui.add_space(22.0);
            padded(ui, 22.0, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(title)
                            .font(theme::semibold(15.0))
                            .color(p.fg),
                    )
                    .wrap()
                    .selectable(false),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(message)
                            .font(theme::regular(13.0))
                            .color(p.secondary),
                    )
                    .wrap()
                    .selectable(false),
                );
                ui.add_space(8.0);
                note(ui, compact_path(&removal.worktree.path), p.muted);
            });
            ui.add_space(6.0);
            let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 62.0));
            place(
                ui,
                rect.shrink2(vec2(20.0, 0.0)),
                Layout::right_to_left(Align::Center),
                "remove-worktree-actions",
                |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if let Some((verb, _)) = confirming {
                        confirm = button(ui, p, verb, ButtonKind::Destructive).clicked();
                    }
                    cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
                },
            );
        },
    );
    if let (true, Some((_, discard))) = (confirm, confirming) {
        actions.push(Action::Worktree(Event::ConfirmRemove { discard }));
    } else if cancel || output.backdrop_clicked {
        actions.push(Action::CloseOverlay);
    }
}

/// "Merged" on the tab of a worktree whose branch is in its base: the offer
/// to clean up. Laid out before the tab's other chips and painted like them.
pub struct MergedChip {
    chip: Option<(Rect, std::sync::Arc<egui::Galley>, egui::Response)>,
    pane: PaneId,
    /// Leading edge of the chip; the trailing edge given when there is none.
    pub left: f32,
}
impl MergedChip {
    pub fn layout(
        ui: &Ui,
        id: Id,
        (pane, tab): (PaneId, Option<&Tab>),
        (limit, right, middle): (f32, f32, f32),
        p: Palette,
    ) -> Self {
        let chip = tab
            .filter(|tab| tab.merged.is_some())
            .map(|_| {
                ui.painter()
                    .layout_no_wrap("Merged".into(), theme::medium(11.5), p.accent)
            })
            .filter(|label| right - (label.size().x + 28.0) >= limit)
            .map(|label| {
                let rect = Rect::from_min_max(
                    Pos2::new(right - label.size().x - 28.0, middle - 9.0),
                    Pos2::new(right, middle + 9.0),
                );
                let response = ui.interact(rect, id, Sense::click());
                response.widget_info(|| {
                    WidgetInfo::labeled(WidgetType::Button, true, "Remove merged worktree")
                });
                (rect, label, response)
            });
        Self {
            left: chip.as_ref().map_or(right, |chip| chip.0.left() - 1.0),
            chip,
            pane,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.chip.is_none()
    }
    pub fn hovered(&self) -> bool {
        self.chip.as_ref().is_some_and(|chip| chip.2.hovered())
    }
    pub fn paint(self, painter: &egui::Painter, p: Palette, actions: &mut Vec<Action>) {
        let Some((rect, label, response)) = self.chip else {
            return;
        };
        painter.rect_filled(
            rect,
            5,
            theme::tint(p.accent, if response.hovered() { 0.22 } else { 0.12 }),
        );
        icons::paint(
            painter,
            Rect::from_center_size(
                Pos2::new(rect.left() + 11.5, rect.center().y),
                Vec2::splat(12.0),
            ),
            Icon::Check,
            p.accent,
        );
        galley_at(
            painter,
            Pos2::new(rect.left() + 21.0, rect.center().y + 1.0),
            label,
        );
        if response
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("The branch is merged. Remove its worktree…")
            .clicked()
        {
            actions.push(Action::Worktree(Event::RemoveOf(self.pane)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> Worktree {
        Worktree {
            repository: "/code/project".into(),
            path: "/code/project.worktrees/fix-login".into(),
            branch: "fix-login".into(),
            start: "a".repeat(40),
        }
    }

    #[test]
    fn removal_names_what_goes_and_asks_twice_only_for_uncommitted_work() {
        let copy = |condition: Condition| removal_copy(&worktree(), Some(&Ok(condition)));
        assert_eq!(removal_copy(&worktree(), None).1, None);
        assert_eq!(
            removal_copy(&worktree(), Some(&Err("no git".into()))).1,
            None
        );
        let (message, verb) = copy(Condition {
            merged: Some("main".into()),
            base: "main".into(),
            ..Default::default()
        });
        assert!(message.starts_with("fix-login was merged into main."));
        assert_eq!(verb, Some(("Remove", false)));
        let (message, verb) = copy(Condition {
            base: "main".into(),
            unmerged: 3,
            ..Default::default()
        });
        assert!(message.starts_with("3 commits are not in main yet."));
        assert!(message.contains("the branch fix-login keeps them"));
        assert_eq!(verb, Some(("Remove", false)));
        let (message, verb) = copy(Condition {
            base: "main".into(),
            unmerged: 1,
            uncommitted: 1,
            ..Default::default()
        });
        assert!(message.starts_with("1 file has changes that were never committed."));
        assert!(message.ends_with("The branch fix-login keeps what was committed."));
        assert_eq!(verb, Some(("Discard and remove", true)));
    }

    #[test]
    fn the_sheet_says_what_create_will_do_with_the_name_as_typed() {
        let mut state = State::default();
        assert_eq!(
            outcome(&state),
            ("Looking for the repository…".into(), false)
        );
        state.repository = Probe::Found(Repository {
            root: "/code/project".into(),
            base: "main".into(),
            branches: vec!["main".into(), "fix-login".into(), "old".into()],
            worktrees: vec![Existing {
                worktree: worktree(),
                merged: None,
            }],
        });
        assert!(outcome(&state).0.starts_with("A new branch from main"));
        state.branch = "add  search".into();
        assert_eq!(
            outcome(&state),
            ("Creates the branch add-search from main.".into(), false)
        );
        state.branch = "old".into();
        assert!(
            outcome(&state)
                .0
                .starts_with("Checks out the existing branch old")
        );
        state.branch = "fix-login".into();
        assert_eq!(
            outcome(&state),
            (
                "Starts another agent in the worktree fix-login already has.".into(),
                false
            )
        );
        state.branch = "a..b".into();
        assert!(outcome(&state).1);
        state.error = Some("git said no".into());
        assert_eq!(outcome(&state), ("git said no".into(), true));
        state.repository = Probe::Failed("This terminal is not in a git repository".into());
        state.error = None;
        assert!(outcome(&state).1);
    }
}
