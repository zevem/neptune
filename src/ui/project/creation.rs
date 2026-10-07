//! The form that starts a project: one field for what the person wants
//! done, which Enter or the control inside it starts. The line under it
//! names the lead, and unfolds the choice of its CLI, model and effort for
//! the few who want another. A project that worked here before is offered
//! again under the form.
use super::settings::{effort_control, model_control};
use super::{Action, Catalog, Event, Setting, State, Tone, lead_words, quiet, row_action};
use crate::{
    icons::{self, Icon},
    projects::settings::LeadSettings,
    theme::{self, Palette},
    ui::{
        agents::kind_name,
        chat::{self, Editor},
        helpers::{elided, focus_ring, galley_at, place, section_label, segmented},
    },
};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Frame, Id, Layout, Margin, Pos2, Rect, Sense, Ui,
    UiBuilder, Vec2, WidgetInfo, WidgetType, vec2,
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
const NONE_INSTALLED: &str = "Neptune found neither Claude Code nor Codex. Install one and check \
                              that it runs in a terminal. Neptune looks again within a minute.";
/// The names of the options: as wide as the longest of them.
const LABELS: f32 = 56.0;

/// One option of the lead: its name, then its control in the room beside.
fn option<R>(ui: &mut Ui, p: Palette, label: &str, control: impl FnOnce(&mut Ui, f32) -> R) -> R {
    let (_, row) = ui.allocate_space(vec2(ui.available_width(), theme::metrics::CONTROL_HEIGHT));
    ui.painter().text(
        Pos2::new(row.left() + 4.0, row.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::regular(12.0),
        p.secondary,
    );
    let room = Rect::from_min_max(Pos2::new(row.left() + LABELS + 4.0, row.top()), row.max);
    let inner = place(
        ui,
        room,
        Layout::left_to_right(Align::Center),
        ("project-option", label),
        |ui| control(ui, room.width()),
    );
    ui.add_space(6.0);
    inner
}

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
                ui.add_space(2.0);
            }
            // Where it would work: the focused terminal's directory, which
            // it follows until the project is started.
            let (_, line) = ui.allocate_space(vec2(width, 18.0));
            icons::paint(
                ui.painter(),
                Rect::from_center_size(
                    Pos2::new(line.left() + 10.0, line.center().y),
                    Vec2::splat(12.0),
                ),
                Icon::Folder,
                p.muted,
            );
            let folder = galley_at(
                ui.painter(),
                Pos2::new(line.left() + 22.0, line.center().y),
                elided(
                    ui.painter(),
                    directory,
                    theme::regular(11.5),
                    p.muted,
                    width - 26.0,
                ),
            );
            ui.interact(folder, Id::new("project-create-folder"), Sense::hover())
                .on_hover_text(
                    "The project works in this folder. It follows the focused terminal until \
                     you start.",
                );
            ui.add_space(10.0);

            let height = chat::editor_height(ui, &state.goal, width - 28.0, (3, 8));
            let (_, field) = ui.allocate_space(vec2(width, height));
            let mut start = false;
            ui.add_enabled_ui(!starting, |ui| {
                let (_, entered) = chat::editor(
                    ui,
                    p,
                    field,
                    Editor {
                        id: chat::goal_id(),
                        text: &mut state.goal,
                        hint: "What do you want to get done?",
                        label: "Project goal",
                        focus: &mut state.focus,
                        composing,
                        trailing: if starting { 0.0 } else { 28.0 },
                        newline: false,
                    },
                );
                start |= entered;
            });
            let ready = !starting && lead.is_some() && !state.goal.trim().is_empty();
            // While its folder is made there is nothing to press.
            if !starting {
                let control = Rect::from_min_size(
                    Pos2::new(field.right() - 29.0, field.bottom() - 28.0),
                    Vec2::splat(24.0),
                );
                // The composer's control, so that the two fields are one habit.
                start |= chat::send_button_named(
                    ui,
                    p,
                    control,
                    (false, ready),
                    "Start project",
                    "Start · Enter. Shift+Enter starts a new line.",
                )
                .clicked();
            }

            // The line that names the lead. A press unfolds what else can
            // be chosen for it.
            ui.add_space(4.0);
            let (_, row) = ui.allocate_space(vec2(width, 22.0));
            let mut right = row.right() - 4.0;
            if starting || width >= 240.0 {
                let said = ui.painter().text(
                    Pos2::new(right, row.center().y),
                    Align2::RIGHT_CENTER,
                    if starting {
                        "Starting…"
                    } else {
                        "Enter starts"
                    },
                    theme::regular(11.0),
                    p.muted,
                );
                right = said.left() - 10.0;
            }
            let left = row.left() + 4.0;
            let choices = lead.and_then(|lead| catalog.of(Setting::Lead, lead));
            match (leads, lead) {
                (_, Some(kind)) => {
                    let words = lead_words(
                        kind,
                        state.lead_settings.model.as_deref(),
                        state.lead_settings.effort.as_deref(),
                        choices,
                    );
                    let font = theme::regular(11.5);
                    let room = (right - left - 16.0).max(0.0);
                    let chip = elided(ui.painter(), &words, font.clone(), p.secondary, room);
                    let rect = Rect::from_min_size(
                        Pos2::new(left, row.center().y - chip.size().y * 0.5),
                        chip.size() + vec2(16.0, 0.0),
                    );
                    let response = ui.interact(
                        rect.expand2(vec2(4.0, 3.0)),
                        Id::new("project-lead-options"),
                        if starting {
                            Sense::hover()
                        } else {
                            Sense::click()
                        },
                    );
                    if response.clicked() {
                        state.lead_options = !state.lead_options;
                    }
                    let open = state.lead_options;
                    response.widget_info(|| {
                        WidgetInfo::labeled(WidgetType::CollapsingHeader, !starting, "Lead options")
                    });
                    ui.ctx().accesskit_node_builder(response.id, |node| {
                        node.set_expanded(open);
                    });
                    if response.has_focus() {
                        focus_ring(ui.painter(), rect.expand2(vec2(4.0, 2.0)), 5, p);
                    }
                    let lit = !starting && (response.hovered() || response.has_focus());
                    let chip = if lit {
                        elided(ui.painter(), &words, font, p.fg, room)
                    } else {
                        chip
                    };
                    icons::paint(
                        ui.painter(),
                        Rect::from_center_size(
                            Pos2::new(rect.right() - 6.0, row.center().y + 0.5),
                            Vec2::splat(10.0),
                        ),
                        if open {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        if lit { p.fg } else { p.muted },
                    );
                    ui.painter().galley(rect.min, chip, Color32::PLACEHOLDER);
                    if !starting {
                        response
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .on_hover_text("Choose the lead's CLI, model and effort");
                    }
                }
                (found, None) => {
                    galley_at(
                        ui.painter(),
                        Pos2::new(left, row.center().y),
                        elided(
                            ui.painter(),
                            if found.is_none() {
                                "Looking for Claude Code and Codex…"
                            } else {
                                "No lead installed"
                            },
                            theme::regular(11.5),
                            p.secondary,
                            right - left,
                        ),
                    );
                }
            }

            if let (true, Some(picked)) = (state.lead_options, lead.as_mut()) {
                ui.add_space(8.0);
                ui.add_enabled_ui(!starting, |ui| {
                    if let Some(leads) = leads.filter(|leads| leads.len() > 1) {
                        let options: Vec<(AgentKind, &str)> =
                            leads.iter().map(|kind| (*kind, kind_name(*kind))).collect();
                        let changed = option(ui, p, "Lead", |ui, room| {
                            segmented(ui, p, "project-lead", picked, &options, room)
                        });
                        if changed {
                            state.lead = Some(*picked);
                            // What one CLI takes is not what the other does.
                            state.lead_settings = LeadSettings::default();
                            state.custom = None;
                        }
                    }
                    // What the lead runs with is its CLI's own choice
                    // unless it is asked for here; the project's settings
                    // change it later.
                    if let Some(choices) = catalog.of(Setting::Lead, *picked) {
                        let model = option(ui, p, "Model", |ui, room| {
                            model_control(
                                ui,
                                p,
                                room,
                                (None, Setting::Lead),
                                ("Lead model", "Default", "What your CLI is set to"),
                                state.lead_settings.model.as_deref(),
                                choices,
                                composing,
                                &mut state.custom,
                            )
                        });
                        if let Some(model) = model {
                            state.lead_settings.model = model;
                        }
                        let effort = option(ui, p, "Effort", |ui, room| {
                            effort_control(
                                ui,
                                p,
                                room,
                                (None, Setting::Lead),
                                ("Lead effort", "Default", "What your CLI is set to"),
                                state.lead_settings.effort.as_deref(),
                                choices,
                            )
                        });
                        if let Some(effort) = effort {
                            state.lead_settings.effort = effort;
                        }
                    }
                });
            }

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
