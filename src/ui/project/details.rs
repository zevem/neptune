//! The project's details: everything about it that is not the
//! conversation, behind one control of its header. A row leads back to the
//! chat and chooses among three parts: what the project's agents share,
//! what wakes its lead by itself, and what the lead and its agents run with.
use super::settings::{self, choice_row};
use super::{
    Action, BACK, Event, FILE, File, MAX_INSTRUCTION, MAX_INSTRUCTIONS, MAX_MINUTES, MIN_MINUTES,
    Project, ROW, Segment, State, Tone, Watch, WatchAsk, WatchForm, WatchKind, field_ids,
    instructions_id, quiet, row_action,
};
use crate::{
    icons::{self, Icon},
    theme::{self, Palette},
    ui::{
        agents::elapsed_label,
        chat::{self, Editor},
        helpers::{
            caption, elided, focus_ring, galley_at, menu_item, menu_layout, menu_separator, place,
            section_label, segmented, select,
        },
    },
};
use eframe::egui::{
    self, Align, Align2, CursorIcon, Frame, Id, Layout, Margin, Pos2, Rect, Sense, Ui, UiBuilder,
    Vec2, WidgetInfo, WidgetType, vec2,
};

/// How often a watch may run without a number being typed, in minutes.
const INTERVALS: [(u32, &str); 6] = [
    (15, "Every 15 minutes"),
    (30, "Every 30 minutes"),
    (60, "Every hour"),
    (240, "Every 4 hours"),
    (1440, "Every day"),
    (10080, "Every week"),
];

/// The details in `rect`, under the project's header.
pub(super) fn show(
    ui: &mut Ui,
    p: Palette,
    rect: Rect,
    project: &Project,
    composing: bool,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let row = Rect::from_min_size(rect.min, vec2(rect.width(), 28.0));
    let back = place(
        ui,
        Rect::from_min_size(row.min, Vec2::splat(28.0)),
        Layout::left_to_right(Align::Center),
        "project-back",
        |ui| icons::button(ui, Icon::ChevronLeft, "Back to chat").clicked(),
    );
    let segments = Rect::from_min_max(Pos2::new(row.left() + 32.0, row.top()), row.max);
    place(
        ui,
        segments,
        Layout::top_down(Align::Min),
        "project-segments",
        |ui| {
            segmented(
                ui,
                p,
                "project-segment",
                &mut state.segment,
                &[
                    (Segment::Context, "Context"),
                    (Segment::Watches, "Watches"),
                    (Segment::Settings, "Settings"),
                ],
                segments.width(),
            )
        },
    );
    let segment = state.segment;
    state.details = segment;
    let mut top = rect.top() + BACK;
    // What needs the person is the chat's to show: here it is one line
    // that leads back there.
    let mut leave = back;
    if !project.needs.is_empty() {
        let line = Rect::from_min_size(Pos2::new(rect.left(), top), vec2(rect.width(), ROW));
        let words = format!("{} needs you", project.needs.len());
        let response = ui
            .interact(line, Id::new("project-details-needs"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, true, format!("{words}, back to chat"))
        });
        let surface = line.shrink2(vec2(0.0, 1.0));
        if response.is_pointer_button_down_on() {
            ui.painter().rect_filled(surface, 7, p.pressed);
        } else if response.hovered() {
            ui.painter().rect_filled(surface, 7, p.hover);
        }
        if response.has_focus() {
            focus_ring(ui.painter(), line.shrink(2.0), 7, p);
        }
        ui.painter().circle_filled(
            Pos2::new(line.left() + 13.0, line.center().y),
            4.0,
            p.attention,
        );
        ui.painter().text(
            Pos2::new(line.left() + 26.0, line.center().y),
            Align2::LEFT_CENTER,
            &words,
            theme::medium(11.5),
            p.attention,
        );
        leave |= response.clicked();
        top += ROW;
    }
    if project.locked {
        // Said once for the part in view: nothing in it takes a change.
        ui.painter().text(
            Pos2::new(rect.left() + 4.0, top + 9.0),
            Align2::LEFT_CENTER,
            "Read-only",
            theme::regular(11.5),
            p.muted,
        );
        top += 20.0;
    }
    let body = Rect::from_min_max(Pos2::new(rect.left(), top), rect.max);
    match segment {
        Segment::Context => {
            state.removing = None;
            context(ui, p, body, project, composing, state, actions);
        }
        Segment::Settings => {
            state.removing = None;
            state.deleting = None;
            settings::page(ui, p, body, project, composing, state, actions);
        }
        _ => {
            state.deleting = None;
            watches(ui, p, body, project, composing, state, actions);
        }
    }
    if leave {
        state.segment = Segment::Chat;
    }
}

/// A quiet label over a list, with how many it holds. Returns its row, for
/// a control at its trailing edge.
fn counted(ui: &mut Ui, p: Palette, label: &str, count: usize) -> Rect {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 24.0));
    let painter = ui.painter();
    let named = painter.text(
        Pos2::new(rect.left() + 4.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::medium(11.5),
        p.secondary,
    );
    painter.text(
        Pos2::new(named.right() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        count.to_string(),
        theme::regular(11.5),
        p.muted,
    );
    rect
}

/// The question a row asks before something of it is deleted for good.
/// Returns whether it was answered, and with a yes.
fn asked(ui: &mut Ui, p: Palette, question: &str, (delete, keep): (&str, &str)) -> Option<bool> {
    let mut answer = None;
    Frame::new()
        .fill(p.control)
        .corner_radius(theme::metrics::ROW_RADIUS)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(question)
                        .font(theme::regular(12.0))
                        .color(p.fg),
                )
                .wrap()
                .selectable(false),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if row_action(ui, p, "Delete", delete, Tone::Plain) {
                    answer = Some(true);
                }
                if row_action(ui, p, "Cancel", keep, Tone::Plain) {
                    answer = Some(false);
                }
            });
        });
    ui.add_space(4.0);
    answer
}

/// One file of the context: a press opens it, and its menu shows it in the
/// file manager or deletes it. Deleting asks once more, in the row itself.
fn file(
    ui: &mut Ui,
    p: Palette,
    project: &Project,
    file: &File,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let deleting = state
        .deleting
        .as_ref()
        .is_some_and(|(of, name)| *of == project.id && *name == file.name);
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), FILE));
    let menu = Rect::from_center_size(
        Pos2::new(rect.right() - 14.0, rect.center().y),
        Vec2::splat(28.0),
    );
    let row = Rect::from_min_max(rect.min, Pos2::new(menu.left() - 2.0, rect.bottom()));
    let response = ui
        .interact(
            row,
            Id::new(("project-file", project.id, &file.name)),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| {
        WidgetInfo::labeled(WidgetType::Button, true, format!("Open {}", file.name))
    });
    let detail = [
        file.author.as_str(),
        &elapsed_label(file.age),
        file.size.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    if ui.is_rect_visible(rect) {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        if response.is_pointer_button_down_on() {
            painter.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 7, p.pressed);
        } else if response.hovered() {
            painter.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 7, p.hover);
        }
        if response.has_focus() {
            focus_ring(&painter, row.shrink(1.0), 7, p);
        }
        let width = row.width() - 10.0;
        galley_at(
            &painter,
            Pos2::new(rect.left() + 8.0, rect.top() + 13.0),
            elided(&painter, &file.name, theme::medium(12.0), p.fg, width),
        );
        galley_at(
            &painter,
            Pos2::new(rect.left() + 8.0, rect.top() + 29.0),
            elided(&painter, &detail, theme::regular(11.5), p.muted, width),
        );
    }
    if response.clicked() {
        actions.push(Action::Project(Event::OpenFile(
            project.id,
            file.name.clone(),
        )));
    }
    response.on_hover_text(if file.removable {
        format!("Open {} with its default application", file.name)
    } else {
        // The one file nobody else writes: its list of the others.
        format!("Open {}. Written by Neptune", file.name)
    });
    place(
        ui,
        menu,
        Layout::left_to_right(Align::Center),
        ("project-file-menu", &file.name),
        |ui| {
            let more = icons::button(ui, Icon::Ellipsis, &format!("Actions for {}", file.name));
            egui::Popup::menu(&more).show(|ui| {
                menu_layout(ui, 190.0);
                if menu_item(ui, p, Icon::File, "Open", "", false) {
                    actions.push(Action::Project(Event::OpenFile(
                        project.id,
                        file.name.clone(),
                    )));
                    ui.close();
                }
                if menu_item(ui, p, Icon::Folder, "Reveal in file manager", "", false) {
                    actions.push(Action::Project(Event::RevealFile(
                        project.id,
                        file.name.clone(),
                    )));
                    ui.close();
                }
                // Nothing of a read-only project is deleted, and Neptune
                // would only write its index again.
                if file.removable && !project.locked {
                    menu_separator(ui, p);
                    if menu_item(ui, p, Icon::Trash, "Delete…", "", true) {
                        state.deleting = Some((project.id, file.name.clone()));
                        ui.close();
                    }
                }
            });
        },
    );
    if deleting {
        let question = format!("Delete {}? Agents will no longer be handed it.", file.name);
        let names = (
            format!("Delete {}", file.name),
            format!("Keep {}", file.name),
        );
        match asked(ui, p, &question, (&names.0, &names.1)) {
            Some(true) => {
                actions.push(Action::Project(Event::DeleteFile(
                    project.id,
                    file.name.clone(),
                )));
                state.deleting = None;
            }
            Some(false) => state.deleting = None,
            None => {}
        }
    }
}

/// What the project's agents share: the instructions only the person
/// writes, in a field of their own, and the files of the context folder.
fn context(
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
            .id_salt(("project-context", project.id))
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    column.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(ui.clip_rect()));
    column.spacing_mut().item_spacing = Vec2::ZERO;
    let kept = project.context.instructions;
    // A file that is gone takes the question about it along.
    if let Some((of, name)) = &state.deleting
        && (*of != project.id || !project.context.files.iter().any(|file| file.name == *name))
    {
        state.deleting = None;
    }
    egui::ScrollArea::vertical()
        .id_salt("project-context-scroll")
        .auto_shrink([false, false])
        .show(&mut column, |ui| {
            let width = ui.available_width() - 4.0;
            ui.set_width(width);
            section_label(ui, p, "Instructions");
            // What is being written stays the person's until it is saved
            // or taken back, whatever the folder holds meanwhile.
            let mut text = state
                .instructions
                .get(&project.id)
                .cloned()
                .unwrap_or_else(|| kept.to_owned());
            let height = chat::editor_height(ui, &text, width, (3, 10));
            let (_, field) = ui.allocate_space(vec2(width, height));
            let mut focus = false;
            ui.add_enabled_ui(!project.locked && project.context.read, |ui| {
                chat::editor(
                    ui,
                    p,
                    field,
                    Editor {
                        id: instructions_id(),
                        text: &mut text,
                        hint: "How should agents work here? For example: small commits, run \
                               the tests before you report, never push.",
                        label: "Project instructions",
                        focus: &mut focus,
                        composing,
                        trailing: 0.0,
                        newline: true,
                    },
                );
            });
            let changed = text.trim() != kept.trim();
            if changed {
                state.instructions.insert(project.id, text.clone());
            } else {
                state.instructions.remove(&project.id);
            }
            if changed {
                // Saving and taking back are offered only while there is
                // something to save or take back.
                let over = text.trim().len() >= MAX_INSTRUCTIONS;
                ui.add_space(4.0);
                let (_, row) = ui.allocate_space(vec2(width, theme::metrics::CONTROL_HEIGHT));
                let buttons = place(
                    ui,
                    row,
                    Layout::right_to_left(Align::Center),
                    "project-instructions-actions",
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add_enabled_ui(!over, |ui| {
                            if row_action(ui, p, "Save", "Save instructions", Tone::Accent) {
                                actions.push(Action::Project(Event::SaveInstructions {
                                    project: project.id,
                                    text: text.trim().to_owned(),
                                }));
                            }
                        });
                        if row_action(ui, p, "Revert", "Revert instructions", Tone::Quiet) {
                            state.instructions.remove(&project.id);
                        }
                        ui.min_rect().left()
                    },
                );
                let (said, ink) = if over {
                    ("Too long to save", p.red)
                } else {
                    ("Not saved", p.secondary)
                };
                galley_at(
                    ui.painter(),
                    Pos2::new(row.left() + 4.0, row.center().y),
                    elided(
                        ui.painter(),
                        said,
                        theme::regular(11.5),
                        ink,
                        buttons - row.left() - 12.0,
                    ),
                );
            } else {
                caption(ui, p, "Every agent is handed this first.");
            }
            ui.add_space(10.0);
            counted(ui, p, "Files", project.context.files.len());
            if project.context.files.is_empty() {
                ui.horizontal_top(|ui| {
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.set_width(width - 8.0);
                        quiet(
                            ui,
                            p,
                            if project.context.read {
                                "Nothing yet. The lead records decisions and status here, and \
                                 agents add notes."
                            } else {
                                "Reading…"
                            },
                        );
                    });
                });
            }
            for item in project.context.files {
                file(ui, p, project, item, state, actions);
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                if row_action(
                    ui,
                    p,
                    "Reveal context folder",
                    "Reveal context folder",
                    Tone::Plain,
                ) {
                    actions.push(Action::Project(Event::RevealContext(project.id)));
                }
            });
            ui.add_space(8.0);
        });
}

/// One watch: what it is and when it runs, with what the person can do
/// with it behind its menu. One the lead proposed is answered in the row,
/// under every word it would tell the lead.
fn watch(
    ui: &mut Ui,
    p: Palette,
    project: &Project,
    item: &Watch,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let (id, title) = (item.id, &item.title);
    let width = ui.available_width();
    let send = |actions: &mut Vec<Action>, event| actions.push(Action::Project(event));
    let words = |ui: &mut Ui, text: &str, font, ink, wrap: bool| {
        let label = egui::Label::new(egui::RichText::new(text).font(font).color(ink));
        let label = if wrap { label.wrap() } else { label.truncate() };
        ui.add(label.selectable(false));
    };
    if item.proposed {
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            let (_, mark) = ui.allocate_space(vec2(20.0, 18.0));
            ui.painter().circle_filled(
                Pos2::new(mark.left() + 8.0, mark.center().y),
                4.0,
                p.attention,
            );
            ui.vertical(|ui| {
                ui.set_width(width - 24.0);
                words(ui, title, theme::medium(12.0), p.fg, false);
                ui.add_space(1.0);
                quiet(ui, p, &format!("{} · proposed by the lead", item.trigger));
                if !item.instruction.is_empty() {
                    ui.add_space(3.0);
                    words(
                        ui,
                        &item.instruction,
                        theme::regular(11.5),
                        p.secondary,
                        true,
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if row_action(
                        ui,
                        p,
                        "Allow",
                        &format!("Allow watch {title}"),
                        Tone::Accent,
                    ) {
                        send(actions, Event::AllowWatch(project.id, id, true));
                    }
                    let name = format!("Decline watch {title}");
                    if row_action(ui, p, "Decline", &name, Tone::Plain) {
                        send(actions, Event::AllowWatch(project.id, id, false));
                    }
                });
            });
        });
        ui.add_space(8.0);
        return;
    }
    let detail = [item.trigger.as_str(), item.detail.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    ui.horizontal_top(|ui| {
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.set_width(width - 8.0 - 30.0);
            ui.add_space(5.0);
            // One that is held back steps back.
            let ink = if item.paused { p.secondary } else { p.fg };
            words(ui, title, theme::medium(12.0), ink, false);
            ui.add_space(1.0);
            // How a pull request stands is at the end of its line, so the
            // line wraps and cuts nothing off.
            quiet(ui, p, &detail);
            ui.add_space(6.0);
        });
        ui.add_space(2.0);
        ui.vertical(|ui| {
            ui.add_space(7.0);
            let more = icons::button(ui, Icon::Ellipsis, &format!("Actions for watch {title}"));
            egui::Popup::menu(&more).show(|ui| {
                menu_layout(ui, 170.0);
                let (icon, label) = if item.paused {
                    (Icon::Play, "Resume")
                } else {
                    (Icon::Pause, "Pause")
                };
                if menu_item(ui, p, icon, label, "", false) {
                    send(actions, Event::PauseWatch(project.id, id, !item.paused));
                    ui.close();
                }
                if menu_item(ui, p, Icon::Refresh, "Run now", "", false) {
                    send(actions, Event::RunWatch(project.id, id));
                    ui.close();
                }
                menu_separator(ui, p);
                if menu_item(ui, p, Icon::Trash, "Delete…", "", true) {
                    state.removing = Some((project.id, id));
                    ui.close();
                }
            });
        });
    });
    if state.removing == Some((project.id, id)) {
        let names = (
            format!("Delete watch {title} for good"),
            format!("Keep watch {title}"),
        );
        match asked(ui, p, "Delete this watch?", (&names.0, &names.1)) {
            Some(true) => {
                send(actions, Event::DeleteWatch(project.id, id));
                state.removing = None;
            }
            Some(false) => state.removing = None,
            None => {}
        }
    }
}

/// A one-line field of the form that adds a watch.
fn form_field(
    ui: &mut Ui,
    p: Palette,
    (id, label, hint): (Id, &str, &str),
    text: &mut String,
    focus: &mut bool,
    composing: bool,
) {
    let width = ui.available_width();
    let height = chat::editor_height(ui, "", width, (1, 1));
    let (_, field) = ui.allocate_space(vec2(width, height));
    chat::editor(
        ui,
        p,
        field,
        Editor {
            id,
            text,
            hint,
            label,
            focus,
            composing,
            trailing: 0.0,
            newline: false,
        },
    );
    // One line: a break that was pasted or typed with Shift does not stay.
    if text.contains(['\n', '\r']) {
        *text = text.replace(['\n', '\r'], " ");
    }
    ui.add_space(6.0);
}

/// The form that adds a watch: the person writes it, so it needs no one's
/// leave to run.
fn watch_form(
    ui: &mut Ui,
    p: Palette,
    project: &Project,
    composing: bool,
    form: &mut WatchForm,
    actions: &mut Vec<Action>,
) {
    let [_, title, every, url, instruction, ..] = field_ids();
    let width = ui.available_width();
    let mut none = false;
    form_field(
        ui,
        p,
        (title, "Watch title", "Name, such as Nightly check"),
        &mut form.title,
        &mut form.focus,
        composing,
    );
    segmented(
        ui,
        p,
        "project-watch-kind",
        &mut form.kind,
        &[
            (WatchKind::Schedule, "On a schedule"),
            (WatchKind::PullRequest, "Pull request"),
        ],
        width,
    );
    ui.add_space(6.0);
    let schedule = form.kind == WatchKind::Schedule;
    if schedule {
        // How often, in words. A number of the person's own is typed under
        // the list once it is asked for.
        let shown = INTERVALS
            .iter()
            .find(|(minutes, _)| form.interval == Some(*minutes))
            .map_or("Custom…", |(_, label)| label);
        let list = select(
            ui,
            p,
            Id::new("project-watch-interval"),
            shown,
            "How often the watch runs",
            width,
        );
        let mut typed = false;
        egui::Popup::menu(&list).show(|ui| {
            menu_layout(ui, width.max(180.0));
            for (minutes, label) in INTERVALS {
                if choice_row(ui, p, label, form.interval == Some(minutes)).clicked() {
                    form.interval = Some(minutes);
                    ui.close();
                }
            }
            menu_separator(ui, p);
            if choice_row(ui, p, "Custom…", form.interval.is_none()).clicked() {
                form.interval = None;
                typed = true;
                ui.close();
            }
        });
        ui.add_space(6.0);
        if form.interval.is_none() {
            form_field(
                ui,
                p,
                (
                    every,
                    "Minutes between runs",
                    "Minutes between runs, 15 or more",
                ),
                &mut form.every,
                &mut typed,
                composing,
            );
        }
    } else {
        form_field(
            ui,
            p,
            (
                url,
                "Pull request address",
                "https://github.com/owner/repo/pull/12",
            ),
            &mut form.url,
            &mut none,
            composing,
        );
    }
    let height = chat::editor_height(ui, &form.instruction, width, (3, 8));
    let (_, field) = ui.allocate_space(vec2(width, height));
    chat::editor(
        ui,
        p,
        field,
        Editor {
            id: instruction,
            text: &mut form.instruction,
            hint: if schedule {
                "What should the lead do each time?"
            } else {
                "What should the lead do when it changes? Optional."
            },
            label: "What the watch does",
            focus: &mut none,
            composing,
            trailing: 0.0,
            newline: true,
        },
    );
    ui.add_space(4.0);
    let typed = form.every.trim().parse::<u32>().ok();
    let minutes = form.interval.or(typed);
    let trigger = if schedule {
        minutes
            .filter(|minutes| (MIN_MINUTES..=MAX_MINUTES).contains(minutes))
            .filter(|_| !form.instruction.trim().is_empty())
            .map(WatchAsk::Every)
    } else {
        Some(form.url.trim())
            .filter(|url| neptune_model::PullRequest::parse(url).is_some())
            .map(|url| WatchAsk::PullRequest(url.to_owned()))
    };
    let long = form.instruction.trim().len() > MAX_INSTRUCTION;
    let custom = schedule && form.interval.is_none() && !form.every.trim().is_empty();
    let said = if long {
        "Too long"
    } else if custom && typed.is_none_or(|minutes| minutes < MIN_MINUTES) {
        "15 minutes or more"
    } else if custom && typed.is_some_and(|minutes| minutes > MAX_MINUTES) {
        "A week at most"
    } else if !schedule && !form.url.trim().is_empty() && trigger.is_none() {
        "Not a pull request address"
    } else {
        ""
    };
    let ready = !form.title.trim().is_empty() && !long;
    let (_, row) = ui.allocate_space(vec2(width, theme::metrics::CONTROL_HEIGHT));
    let mut close = false;
    let buttons = place(
        ui,
        row,
        Layout::right_to_left(Align::Center),
        "project-watch-actions",
        |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_enabled_ui(ready && trigger.is_some(), |ui| {
                if row_action(ui, p, "Add", "Add the watch", Tone::Accent)
                    && let Some(trigger) = trigger.clone()
                {
                    actions.push(Action::Project(Event::AddWatch {
                        project: project.id,
                        title: form.title.trim().to_owned(),
                        trigger,
                        instruction: form.instruction.trim().to_owned(),
                    }));
                    close = true;
                }
            });
            close |= row_action(ui, p, "Cancel", "Cancel the watch", Tone::Quiet);
            ui.min_rect().left()
        },
    );
    galley_at(
        ui.painter(),
        Pos2::new(row.left() + 4.0, row.center().y),
        elided(
            ui.painter(),
            said,
            theme::regular(11.5),
            p.red,
            buttons - row.left() - 12.0,
        ),
    );
    if close {
        *form = WatchForm::default();
    }
}

/// The project's watches: what wakes its lead by itself, beside its agents.
/// They are listed, held back, run and deleted here, and the person adds
/// one without asking the lead.
fn watches(
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
            .id_salt(("project-watches", project.id))
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    column.set_clip_rect(rect.expand2(vec2(4.0, 0.0)).intersect(ui.clip_rect()));
    column.spacing_mut().item_spacing = Vec2::ZERO;
    // A watch that is gone takes the question about it along.
    if let Some((of, id)) = state.removing
        && (of != project.id || !project.watches.iter().any(|watch| watch.id == id))
    {
        state.removing = None;
    }
    egui::ScrollArea::vertical()
        .id_salt("project-watches-scroll")
        .auto_shrink([false, false])
        .show(&mut column, |ui| {
            let width = ui.available_width() - 4.0;
            ui.set_width(width);
            let label = counted(ui, p, "Watches", project.watches.len());
            if project.locked {
                // Its lead is off, so nothing wakes it and nothing is added
                // to: what it holds is listed, and takes no press.
                state.watch = WatchForm::default();
                caption(ui, p, "Watches are off while this project is read-only.");
                ui.add_space(6.0);
                ui.add_enabled_ui(false, |ui| {
                    for item in project.watches {
                        watch(ui, p, project, item, state, actions);
                    }
                });
                ui.add_space(8.0);
                return;
            }
            if state.watch.open {
                // Under the label, where it is in view as it opens.
                ui.add_space(2.0);
                watch_form(ui, p, project, composing, &mut state.watch, actions);
                ui.add_space(8.0);
            } else {
                let add = place(
                    ui,
                    label,
                    Layout::right_to_left(Align::Center),
                    "project-watch-add",
                    |ui| row_action(ui, p, "Add watch", "Add watch", Tone::Plain),
                );
                if add {
                    state.watch.open = true;
                    state.watch.focus = true;
                }
            }
            if project.watches.is_empty() && !state.watch.open {
                ui.horizontal_top(|ui| {
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.set_width(width - 8.0);
                        quiet(
                            ui,
                            p,
                            "No watches yet. A watch wakes the lead on a schedule or when a \
                             pull request changes.",
                        );
                    });
                });
            }
            for item in project.watches {
                watch(ui, p, project, item, state, actions);
            }
            ui.add_space(4.0);
            caption(
                ui,
                p,
                "Watches run only while Neptune is open. The lead always hears when an agent \
                 finishes, needs you or ends.",
            );
            ui.add_space(8.0);
        });
}
