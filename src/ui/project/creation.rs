//! The form that starts a project: the composer, for what the person wants
//! done, which Enter or the control inside it starts. Its row names the
//! lead's CLI and model and its effort, each a press away from the others;
//! the tray under it names the folder the project works in. A project that
//! worked here before is offered again under the form.
use super::composer::{self, Bar, Who};
use super::{Action, Catalog, Event, Setting, State, Tone, quiet, row_action};
use crate::{
    icons::{self, Icon},
    projects::settings::LeadSettings,
    theme::{self, Palette},
    ui::{
        chat::{self, Editor},
        helpers::{elided, galley_at, place, section_label, spinner},
    },
};
use eframe::egui::{
    self, Align, Align2, CornerRadius, Frame, Id, Layout, Margin, Pos2, Rect, Sense, StrokeKind,
    Ui, UiBuilder, Vec2, vec2,
};
use neptune_model::{AgentKind, WorkspaceId};

/// What the form is drawn from.
pub(super) struct Form<'a> {
    pub workspace: WorkspaceId,
    /// Where the project would work, as the person knows it.
    pub directory: &'a str,
    /// Its folder is being made.
    pub starting: bool,
    /// The project that worked here before, by name.
    pub reopen: Option<&'a str>,
    /// The CLIs installed here that can lead. None while they are looked for.
    pub leads: Option<&'a [AgentKind]>,
    pub catalog: &'a Catalog,
    /// In the sheet of a narrow window, whose title row names the form and
    /// which has no room to explain what a lead is.
    pub sheet: bool,
}

const EXPLAINER: &str = "A lead plans the work, starts agents in their own terminals and reports \
                         back here. The chat and notes stay on this computer.";
/// The tray under the composer.
const TRAY: f32 = 26.0;
const NONE_INSTALLED: &str = "Neptune found neither Claude Code nor Codex. Install one and check \
                              that it runs in a terminal. Neptune looks again within a minute.";

pub(super) fn show(
    ui: &mut Ui,
    p: Palette,
    inner: Rect,
    form: Form,
    composing: bool,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let Form {
        workspace,
        directory,
        starting,
        reopen,
        leads,
        catalog,
        sheet,
    } = form;
    let mut column = ui.new_child(
        UiBuilder::new()
            .id_salt("project-create")
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    column.set_clip_rect(inner.expand2(vec2(4.0, 0.0)).intersect(ui.clip_rect()));
    column.spacing_mut().item_spacing = Vec2::ZERO;
    // The settings of a project that is gone are not this form's.
    if state
        .custom
        .as_ref()
        .is_some_and(|typed| typed.owner.is_some())
    {
        state.custom = None;
    }
    // The pick stands while its CLI is installed; otherwise the first that is.
    let mut lead = leads.and_then(|leads| {
        state
            .lead
            .filter(|lead| leads.contains(lead))
            .or(leads.first().copied())
    });
    // Taller than a short panel once its options are open: it scrolls, and
    // the field that starts the project is never pushed out of reach.
    egui::ScrollArea::vertical()
        .id_salt("project-create-scroll")
        .auto_shrink([false, false])
        .show(&mut column, |ui| {
            let width = ui.available_width() - 4.0;
            ui.set_width(width);
            if sheet {
                ui.add_space(2.0);
            } else {
                ui.add_space(14.0);
                let (_, title) = ui.allocate_space(vec2(width, 18.0));
                ui.painter().text(
                    Pos2::new(title.left() + 4.0, title.center().y),
                    Align2::LEFT_CENTER,
                    "New project",
                    theme::medium(13.0),
                    p.fg,
                );
                ui.add_space(10.0);
            }
            let height = composer::height(ui, &state.goal, width, (3, 8));
            let (_, card) = ui.allocate_space(vec2(width, height));
            let (_, mut start) = composer::surface(
                ui,
                p,
                card,
                !starting,
                Editor {
                    id: chat::goal_id(),
                    text: &mut state.goal,
                    hint: "What do you want to get done?",
                    label: "Project goal",
                    focus: &mut state.focus,
                    composing,
                    trailing: 0.0,
                    newline: false,
                },
            );
            let ready = !starting && lead.is_some() && !state.goal.trim().is_empty();
            let (send, _) = composer::controls(card);
            if starting {
                // While its folder is made there is nothing to press.
                spinner(ui, send.shrink(7.0), p.secondary);
            } else {
                // The composer's control, so that the two fields are one habit.
                start |= chat::send_button_named(
                    ui,
                    p,
                    send,
                    (false, ready),
                    "Start project",
                    "Start · Enter. Shift+Enter starts a new line.",
                )
                .clicked();
            }
            // Who leads and what it runs with, each a press away. The CLI
            // is picked beside the models, where more than one can lead.
            let who = match (leads, lead) {
                (Some(leads), Some(kind)) => Who::Pick(kind, leads),
                (None, _) => Who::Missing("Looking for Claude Code and Codex…"),
                (Some(_), None) => Who::Missing("No lead installed"),
            };
            let told = ui
                .add_enabled_ui(!starting, |ui| {
                    composer::bar(
                        ui,
                        p,
                        card,
                        Bar {
                            who,
                            owner: None,
                            choices: lead.and_then(|lead| catalog.of(Setting::Lead, lead)),
                            model: state.lead_settings.model.as_deref(),
                            effort: state.lead_settings.effort.as_deref(),
                            reported: None,
                            rounds: 1,
                            above: false,
                            composing,
                        },
                        &mut state.custom,
                    )
                })
                .inner;
            if let Some(picked) = told.lead {
                lead = Some(picked);
                state.lead = Some(picked);
                // What one CLI takes is not what the other does.
                state.lead_settings = LeadSettings::default();
                state.custom = None;
            }
            // What the lead runs with is its CLI's own choice unless it is
            // asked for here; the project's settings change it later.
            if let Some(model) = told.model {
                state.lead_settings.model = model;
            }
            if let Some(effort) = told.effort {
                state.lead_settings.effort = effort;
            }
            tray(ui, p, card, directory, starting);

            if leads.is_some_and(<[AgentKind]>::is_empty) {
                ui.add_space(12.0);
                padded(ui, |ui| quiet(ui, p, NONE_INSTALLED));
            } else if reopen.is_none() && !sheet {
                // Someone with an earlier project knows what a lead is.
                ui.add_space(12.0);
                padded(ui, |ui| quiet(ui, p, EXPLAINER));
            }
            if let (true, true, Some(lead)) = (start, ready, lead) {
                actions.push(Action::Project(Event::Create {
                    workspace,
                    goal: state.goal.trim().to_owned(),
                    lead,
                    model: state.lead_settings.model.clone(),
                    effort: state.lead_settings.effort.clone(),
                }));
            }
            // A project whose workspace was closed kept its folder.
            if let Some(name) = reopen.filter(|_| !starting) {
                ui.add_space(10.0);
                section_label(ui, p, "Earlier in this folder");
                Frame::new()
                    .fill(p.control)
                    .corner_radius(theme::metrics::ROW_RADIUS)
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        ui.set_width(width);
                        let (_, card) = ui.allocate_space(vec2(width, 34.0));
                        let button = chat::row_button_width(ui, "Reopen");
                        let at = Rect::from_min_size(
                            Pos2::new(card.right() - button, card.center().y - 11.0),
                            vec2(button, 22.0),
                        );
                        let pressed = place(
                            ui,
                            at,
                            Layout::left_to_right(Align::Center),
                            "project-reopen",
                            |ui| {
                                let name = format!("Reopen project {name}");
                                row_action(ui, p, "Reopen", &name, Tone::Plain)
                            },
                        );
                        if pressed {
                            actions.push(Action::Project(Event::Reopen(workspace)));
                        }
                        let room = at.left() - 10.0 - card.left();
                        galley_at(
                            ui.painter(),
                            Pos2::new(card.left(), card.top() + 9.0),
                            elided(ui.painter(), name, theme::medium(12.5), p.fg, room),
                        );
                        galley_at(
                            ui.painter(),
                            Pos2::new(card.left(), card.top() + 25.0),
                            elided(
                                ui.painter(),
                                "Its chat and notes are still here",
                                theme::regular(11.5),
                                p.muted,
                                room,
                            ),
                        );
                    });
            }
            ui.add_space(8.0);
        });
}

/// The tray under the surface in `card`: the folder the project would work
/// in, which follows the focused terminal until it is started, and while
/// that folder is made, that it is starting.
fn tray(ui: &mut Ui, p: Palette, card: Rect, directory: &str, starting: bool) {
    ui.allocate_space(vec2(card.width(), TRAY));
    let tray = Rect::from_min_max(
        Pos2::new(card.left() + 12.0, card.bottom()),
        Pos2::new(card.right() - 12.0, card.bottom() + TRAY),
    );
    let radius = CornerRadius {
        nw: 0,
        ne: 0,
        sw: 8,
        se: 8,
    };
    let painter = ui.painter();
    painter.rect_filled(tray, radius, p.control.gamma_multiply(0.5));
    // Its sides and foot: the surface's edge is its top.
    painter
        .with_clip_rect(
            tray.with_min_y(tray.top() + 0.5)
                .intersect(painter.clip_rect()),
        )
        .rect_stroke(
            tray.with_min_y(tray.top() - 8.0),
            8,
            p.hairline(),
            StrokeKind::Inside,
        );
    let centre = tray.center().y;
    let mut right = tray.right() - 10.0;
    if starting {
        let said = painter.text(
            Pos2::new(right, centre),
            Align2::RIGHT_CENTER,
            "Starting…",
            theme::regular(11.5),
            p.secondary,
        );
        right = said.left() - 10.0;
    }
    // Under the mark of the lead, its words under the lead's name.
    let mark = card.left() + composer::MARK_CENTRE;
    icons::paint(
        painter,
        Rect::from_center_size(Pos2::new(mark, centre), Vec2::splat(12.0)),
        Icon::Folder,
        p.muted,
    );
    let words = mark + 13.0;
    let folder = galley_at(
        painter,
        Pos2::new(words, centre),
        elided(
            painter,
            directory,
            theme::regular(11.5),
            p.muted,
            (right - words).max(0.0),
        ),
    );
    ui.interact(folder, Id::new("project-create-folder"), Sense::hover())
        .on_hover_text(
            "The project works in this folder. It follows the focused terminal until you start.",
        );
}

/// Words set in from the field's edge as the form's other lines are.
fn padded(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width() - 8.0;
    ui.horizontal_top(|ui| {
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.set_width(width);
            add(ui);
        });
    });
}
