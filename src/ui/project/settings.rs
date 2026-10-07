//! What a project's lead and the agents it starts run with. Every setting
//! is a field that names its value in words and opens the list of the
//! others; leaving a choice open is the first of them, never a blank. A
//! change is the project's at once.
use super::{
    Action, Choice, Choices, Custom, Event, Project, Setting, State, Tone, model_id, row_action,
};
use crate::{
    icons::{self, Icon},
    projects::settings::model_name,
    theme::{self, Palette},
    ui::{
        agents::kind_name,
        chat::{self, Editor},
        helpers::{
            caption, elided, galley_at, group, menu_layout, menu_separator, section_label, select,
        },
    },
};
use eframe::egui::{
    self, Align, Id, Layout, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
    WidgetInfo, WidgetType, vec2,
};
use neptune_model::{AgentKind, ProjectId};

/// What a list was told.
pub(super) enum Pick {
    /// One of its values, or the choice left open.
    Value(Option<String>),
    /// A value of the person's own, to be typed.
    Custom,
}

/// A row of a list of choices: the one in force carries a check, and the
/// others nothing in its place. The hovered row takes the accent, as the
/// rows of a menu do.
pub(super) fn choice_row(ui: &mut Ui, p: Palette, label: &str, checked: bool) -> Response {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 28.0));
    let response = ui.interact(rect, ui.id().with(("menu-item", label)), Sense::click());
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::RadioButton, ui.is_enabled(), checked, label)
    });
    let painter = ui.painter();
    let ink = if response.hovered() || response.has_focus() {
        painter.rect_filled(rect, 6, p.accent);
        p.on_accent
    } else {
        p.fg
    };
    if checked {
        icons::paint(
            painter,
            Rect::from_center_size(
                Pos2::new(rect.left() + 15.0, rect.center().y),
                Vec2::splat(14.0),
            ),
            Icon::Check,
            ink,
        );
    }
    galley_at(
        painter,
        Pos2::new(rect.left() + 32.0, rect.center().y),
        elided(
            painter,
            label,
            theme::regular(13.0),
            ink,
            rect.width() - 42.0,
        ),
    );
    response
}

/// One setting as a field that opens the list of its choices, under the
/// words for leaving it open. `name` is its accessible name. `custom`
/// offers a value of the person's own after the list, and says whether
/// that is what is in force. Returns what the list was told, if anything.
pub(super) fn choice_select(
    ui: &mut Ui,
    p: Palette,
    (id, name, width): (Id, &str, f32),
    (open, tip): (&str, &str),
    current: Option<&str>,
    choices: &[Choice],
    custom: Option<bool>,
) -> Option<Pick> {
    let shown = match current {
        None => open.to_owned(),
        Some(value) => choices
            .iter()
            .find(|choice| choice.value == value)
            .map_or_else(|| value.to_owned(), |choice| choice.label.clone()),
    };
    let response = select(ui, p, id, &shown, name, width);
    let mut picked = None;
    egui::Popup::menu(&response).show(|ui| {
        menu_layout(ui, width.max(180.0));
        let first = choice_row(ui, p, open, current.is_none());
        if first.clicked() {
            picked = Some(Pick::Value(None));
            ui.close();
        }
        if !tip.is_empty() {
            first.on_hover_text(tip);
        }
        for choice in choices {
            if choice_row(ui, p, &choice.label, current == Some(choice.value)).clicked() {
                picked = Some(Pick::Value(Some(choice.value.to_owned())));
                ui.close();
            }
        }
        if let Some(typed) = custom {
            menu_separator(ui, p);
            if choice_row(ui, p, "Custom…", typed).clicked() {
                picked = Some(Pick::Custom);
                ui.close();
            }
        }
    });
    // Choosing what is in force already changes nothing.
    picked.filter(|picked| match picked {
        Pick::Value(value) => value.as_deref() != current,
        Pick::Custom => true,
    })
}

/// The model of one group of settings: its list, or in the list's place
/// the field a model no list names is typed in. Enter uses what was typed
/// if a CLI takes it as a name; a field the keyboard leaves gives the
/// list back. Returns the model that was chosen, if it is another.
#[allow(clippy::too_many_arguments)]
pub(super) fn model_control(
    ui: &mut Ui,
    p: Palette,
    width: f32,
    (owner, setting): (Option<ProjectId>, Setting),
    (name, open, tip): (&str, &str, &str),
    current: Option<&str>,
    choices: &Choices,
    composing: bool,
    custom: &mut Option<Custom>,
) -> Option<Option<String>> {
    if let Some(typed) = custom
        .as_mut()
        .filter(|typed| typed.owner == owner && typed.setting == setting)
    {
        let height = chat::editor_height(ui, "", width, (1, 1));
        let (_, rect) = ui.allocate_space(vec2(width, height));
        let (response, entered) = chat::editor(
            ui,
            p,
            rect,
            Editor {
                id: model_id(setting),
                text: &mut typed.text,
                hint: "Model id",
                label: &format!("{name} id"),
                focus: &mut typed.focus,
                composing,
                trailing: 0.0,
                newline: false,
            },
        );
        // One line: a break that was pasted does not stay.
        if typed.text.contains(['\n', '\r']) {
            typed.text = typed.text.replace(['\n', '\r'], " ");
        }
        let named = model_name(&typed.text).map(str::to_owned);
        if !typed.text.trim().is_empty() && named.is_none() {
            ui.painter().rect_stroke(
                rect,
                theme::metrics::CONTROL_RADIUS,
                Stroke::new(1.0, p.red),
                StrokeKind::Inside,
            );
        }
        if entered && let Some(model) = named {
            *custom = None;
            return Some(Some(model)).filter(|model| model.as_deref() != current);
        }
        if response.lost_focus() {
            *custom = None;
        }
        return None;
    }
    let listed =
        current.is_none_or(|value| choices.models.iter().any(|choice| choice.value == value));
    let id = Id::new(("project-setting", owner, setting, "model"));
    match choice_select(
        ui,
        p,
        (id, name, width),
        (open, tip),
        current,
        &choices.models,
        Some(!listed),
    )? {
        Pick::Value(model) => Some(model),
        Pick::Custom => {
            // What is in force and in no list is what the field starts with.
            *custom = Some(Custom {
                owner,
                setting,
                text: current.filter(|_| !listed).unwrap_or_default().to_owned(),
                focus: true,
            });
            None
        }
    }
}

/// The effort of one group of settings, from its CLI's list.
pub(super) fn effort_control(
    ui: &mut Ui,
    p: Palette,
    width: f32,
    (owner, setting): (Option<ProjectId>, Setting),
    (name, open, tip): (&str, &str, &str),
    current: Option<&str>,
    choices: &Choices,
) -> Option<Option<String>> {
    let id = Id::new(("project-setting", owner, setting, "effort"));
    match choice_select(
        ui,
        p,
        (id, name, width),
        (open, tip),
        current,
        &choices.efforts,
        None,
    )? {
        Pick::Value(effort) => Some(effort),
        Pick::Custom => None,
    }
}

/// Whether what is being typed for `setting` is no name a CLI takes.
fn refused(custom: &Option<Custom>, owner: Option<ProjectId>, setting: Setting) -> bool {
    custom.as_ref().is_some_and(|typed| {
        typed.owner == owner
            && typed.setting == setting
            && !typed.text.trim().is_empty()
            && model_name(&typed.text).is_none()
    })
}
const REFUSED: &str = "That is not a model name this CLI takes.";
/// What the lead's own CLI is set to is what "Default" leaves in force.
const DEFAULT: (&str, &str) = ("Default", "What your CLI is set to");
const OPEN: (&str, &str) = ("Lead decides", "");

/// A value of a row that is said and not chosen, as far as there is room.
fn value(ui: &mut Ui, p: Palette, text: &str) -> Response {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::regular(13.0))
                .color(p.secondary),
        )
        .truncate()
        .selectable(false),
    )
}

/// The Settings part of the details: what the lead runs with, what the
/// agents it starts run with for each CLI, and the project itself.
pub(super) fn page(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    project: &Project,
    composing: bool,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let mut column = ui.new_child(
        UiBuilder::new()
            .id_salt(("project-settings", project.id))
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    column.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(ui.clip_rect()));
    column.spacing_mut().item_spacing = Vec2::ZERO;
    // The field of another project's settings is not this one's.
    if state
        .custom
        .as_ref()
        .is_some_and(|typed| typed.owner != Some(project.id))
    {
        state.custom = None;
    }
    let owner = Some(project.id);
    let missing = |kind: AgentKind| {
        project
            .leads
            .is_some_and(|leads| !leads.contains(&kind))
            .then(|| format!("{} isn't installed here.", kind_name(kind)))
    };
    egui::ScrollArea::vertical()
        .id_salt("project-settings-scroll")
        .auto_shrink([false, false])
        .show(&mut column, |ui| {
            let width = ui.available_width() - 4.0;
            ui.set_width(width);
            // What a row's control may take beside its name.
            let control = (width - 112.0).clamp(96.0, 180.0);
            ui.add_enabled_ui(!project.locked, |ui| {
                section_label(ui, p, "Lead");
                let kept = &project.settings.lead;
                let mut next = kept.clone();
                let refused_now = refused(&state.custom, owner, Setting::Lead);
                group(ui, p, |ui, rows| {
                    let runs = rows.row(ui, "Runs on", |ui| value(ui, p, kind_name(project.lead)));
                    if let Some(model) = project.lead_model {
                        runs.on_hover_text(format!("Last started on {model}"));
                    }
                    if let Some(choices) = project.catalog.of(Setting::Lead, project.lead) {
                        let model = rows.row(ui, "Model", |ui| {
                            model_control(
                                ui,
                                p,
                                control,
                                (owner, Setting::Lead),
                                ("Lead model", DEFAULT.0, DEFAULT.1),
                                kept.model.as_deref(),
                                choices,
                                composing,
                                &mut state.custom,
                            )
                        });
                        if let Some(model) = model {
                            next.model = model;
                        }
                        let effort = rows.row(ui, "Effort", |ui| {
                            effort_control(
                                ui,
                                p,
                                control,
                                (owner, Setting::Lead),
                                ("Lead effort", DEFAULT.0, DEFAULT.1),
                                kept.effort.as_deref(),
                                choices,
                            )
                        });
                        if let Some(effort) = effort {
                            next.effort = effort;
                        }
                    }
                    if refused_now {
                        rows.note(ui, REFUSED);
                    }
                    if let Some(missing) = missing(project.lead) {
                        rows.note(ui, &missing);
                    }
                    rows.note(
                        ui,
                        if project.pending {
                            "Changed. The lead takes this up with its next turn."
                        } else {
                            "Used from the lead's next turn."
                        },
                    );
                });
                if next != *kept {
                    actions.push(Action::Project(Event::SetLead {
                        project: project.id,
                        model: next.model,
                        effort: next.effort,
                    }));
                }
                for (setting, kept) in [
                    (Setting::Claude, &project.settings.claude),
                    (Setting::Codex, &project.settings.codex),
                ] {
                    let kind = setting.kind(project.lead);
                    let Some(choices) = project.catalog.of(setting, project.lead) else {
                        continue;
                    };
                    let title = format!("{} agents", kind_name(kind));
                    ui.add_space(8.0);
                    section_label(ui, p, &title);
                    let mut next = kept.clone();
                    let refused_now = refused(&state.custom, owner, setting);
                    group(ui, p, |ui, rows| {
                        let model = rows.row(ui, "Model", |ui| {
                            model_control(
                                ui,
                                p,
                                control,
                                (owner, setting),
                                (&format!("{title} model"), OPEN.0, OPEN.1),
                                kept.model.as_deref(),
                                choices,
                                composing,
                                &mut state.custom,
                            )
                        });
                        if let Some(model) = model {
                            next.model = model;
                        }
                        let effort = rows.row(ui, "Effort", |ui| {
                            effort_control(
                                ui,
                                p,
                                control,
                                (owner, setting),
                                (&format!("{title} effort"), OPEN.0, OPEN.1),
                                kept.effort.as_deref(),
                                choices,
                            )
                        });
                        if let Some(effort) = effort {
                            next.effort = effort;
                        }
                        // Claude Code's alone. A switch cannot say that the
                        // lead decides, so it is a list of three.
                        if setting == Setting::Claude {
                            let switch = [
                                Choice {
                                    value: "on",
                                    label: "On".into(),
                                },
                                Choice {
                                    value: "off",
                                    label: "Off".into(),
                                },
                            ];
                            let now = kept.ultracode.map(|on| if on { "on" } else { "off" });
                            let picked = rows.row(ui, "Ultracode", |ui| {
                                choice_select(
                                    ui,
                                    p,
                                    (
                                        Id::new(("project-setting", owner, setting, "ultracode")),
                                        &format!("{title} ultracode"),
                                        control,
                                    ),
                                    OPEN,
                                    now,
                                    &switch,
                                    None,
                                )
                            });
                            if let Some(Pick::Value(picked)) = picked {
                                next.ultracode = picked.map(|value| value == "on");
                            }
                        }
                        if refused_now {
                            rows.note(ui, REFUSED);
                        }
                        // Said only while it is what was chosen.
                        if kept.ultracode == Some(true) {
                            rows.note(ui, "Ultracode uses far more tokens.");
                        } else if kept.effort.as_deref() == Some("ultra") {
                            rows.note(ui, "Ultra uses far more tokens.");
                        }
                        if let Some(missing) = missing(kind) {
                            rows.note(ui, &missing);
                        }
                    });
                    if next != *kept {
                        actions.push(Action::Project(Event::SetAgentDefaults {
                            project: project.id,
                            kind,
                            model: next.model,
                            effort: next.effort,
                            ultracode: next.ultracode,
                        }));
                    }
                }
                caption(
                    ui,
                    p,
                    "Agents the lead starts from now on use these. “Lead decides” leaves the \
                     choice to the lead for each agent.",
                );
            });
            ui.add_space(8.0);
            section_label(ui, p, "Project");
            group(ui, p, |ui, rows| {
                rows.row(ui, "Name", |ui| {
                    if row_action(ui, p, "Rename…", "Rename project", Tone::Plain) {
                        actions.push(Action::Project(Event::Rename(project.id)));
                    }
                    ui.add_space(8.0);
                    value(ui, p, project.name);
                });
                rows.row(ui, "Folder", |ui| {
                    if row_action(ui, p, "Reveal", "Reveal the project's folder", Tone::Plain) {
                        actions.push(Action::Project(Event::RevealDirectory(project.id)));
                    }
                    ui.add_space(8.0);
                    value(ui, p, project.directory)
                        .on_hover_text(project.directory_path.display().to_string());
                });
            });
            ui.add_space(8.0);
        });
}
