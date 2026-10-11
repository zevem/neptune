//! Transient dialogs edit only UI drafts and emit targeted commands.
use super::helpers::{
    ButtonKind, SheetPlacement, button, padded, place, sheet, sheet_header, text_field, toast,
};
use super::{Action, Close, CloseStatus, OverlayState, UiState};
use crate::theme::{self, Palette};
use eframe::egui::{self, Align, Id, Layout, Ui, vec2};

/// A sheet measures itself in a hidden first pass, where focus cannot be held.
fn accepts_focus(ui: &Ui) -> bool {
    ui.is_enabled() && !ui.is_sizing_pass()
}

/// Enter confirms a dialog only as a fresh press after the dialog has taken
/// the keyboard. The press that opened it, and a held key repeating from the
/// previous surface, must not confirm something the user has not seen.
fn confirmed_by_enter(ctx: &egui::Context, state: &UiState) -> bool {
    !state.overlay_focus
        && ctx.input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            })
        })
}

/// Trailing actions of a sheet: the confirming action sits at the far edge.
fn footer(ui: &mut Ui, salt: &str, add_buttons: impl FnOnce(&mut Ui)) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 62.0));
    place(
        ui,
        rect.shrink2(vec2(20.0, 0.0)),
        Layout::right_to_left(Align::Center),
        salt,
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            add_buttons(ui);
        },
    );
}

fn rename(
    ctx: &egui::Context,
    p: Palette,
    state: &mut UiState,
    target: OverlayState,
    actions: &mut Vec<Action>,
) {
    let (title, hint, label, verb) = match target {
        OverlayState::NewGroup => (
            "New workspace group",
            "Group name",
            "Workspace group name",
            "Create",
        ),
        OverlayState::RenameGroup(_) => (
            "Rename workspace group",
            "Group name",
            "Workspace group rename",
            "Save",
        ),
        OverlayState::RenameProject(_) => {
            ("Rename project", "Project name", "Project rename", "Save")
        }
        _ => (
            "Rename workspace",
            "Workspace name",
            "Workspace rename",
            "Save",
        ),
    };
    let mut save = confirmed_by_enter(ctx, state);
    let mut cancel = false;
    let output = sheet(ctx, p, title, 380.0, SheetPlacement::Center, |ui| {
        sheet_header(ui, p, title, None);
        padded(ui, 20.0, |ui| {
            let width = ui.available_width();
            let field = text_field(
                ui,
                p,
                Id::new(match target {
                    OverlayState::Rename(_) => "workspace-rename",
                    OverlayState::RenameProject(_) => "project-rename",
                    _ => "workspace-group-name",
                }),
                &mut state.rename_name,
                hint,
                label,
                width,
            );
            if state.overlay_focus && accepts_focus(ui) {
                field.request_focus();
                state.overlay_focus = false;
            }
        });
        ui.add_space(4.0);
        footer(ui, "rename-actions", |ui| {
            save |= button(ui, p, verb, ButtonKind::Primary).clicked();
            cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
        });
    });
    if cancel || output.backdrop_clicked {
        actions.push(Action::CloseOverlay);
    } else if save && !state.rename_name.trim().is_empty() {
        let name = state.rename_name.trim().to_owned();
        actions.push(match target {
            OverlayState::NewGroup => Action::CreateGroup(name),
            OverlayState::RenameGroup(group) => Action::SetGroupName(group, name),
            OverlayState::Rename(workspace) => Action::SetName(workspace, name),
            OverlayState::RenameProject(project) => {
                Action::Project(super::project::Event::SetName(project, name))
            }
            _ => return,
        });
        actions.push(Action::CloseOverlay);
    }
}

fn default_directory(
    ctx: &egui::Context,
    p: Palette,
    state: &mut UiState,
    group: neptune_model::WorkspaceGroupId,
    actions: &mut Vec<Action>,
) {
    let busy = state.directory_pending || state.directory_browsing;
    let mut save = confirmed_by_enter(ctx, state) && !busy;
    let mut browse = false;
    let mut reset = false;
    let mut cancel = false;
    // Reserve the window margin, header and footer so actions stay reachable
    // when a validation error makes the form taller.
    let body_height = (ctx.content_rect().height() - 24.0 - 52.0 - 62.0).max(60.0);
    let output = sheet(
        ctx,
        p,
        "Default directory",
        440.0,
        SheetPlacement::Center,
        |ui| {
            // Areas remember their previous size; allow the body to grow
            // when validation adds a message, within the current window.
            ui.set_max_height(ctx.content_rect().height() - 24.0);
            sheet_header(ui, p, "Default directory", None);
            egui::ScrollArea::vertical()
                .id_salt("group-directory-form")
                .max_height(body_height)
                .show(ui, |ui| {
                    padded(ui, 20.0, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&state.directory_group)
                                    .font(theme::medium(12.0))
                                    .color(p.secondary),
                            )
                            .truncate(),
                        );
                        ui.add_space(8.0);
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                let browse_width = (ui
                                    .painter()
                                    .layout_no_wrap("Browse…".into(), theme::medium(13.0), p.fg)
                                    .size()
                                    .x
                                    + 32.0)
                                    .max(72.0);
                                let width = ui.available_width()
                                    - browse_width
                                    - ui.spacing().item_spacing.x;
                                let field = text_field(
                                    ui,
                                    p,
                                    Id::new("group-default-directory"),
                                    &mut state.directory_path,
                                    "Path or ~/folder",
                                    "Workspace group default directory",
                                    width,
                                );
                                if state.overlay_focus && accepts_focus(ui) {
                                    field.request_focus();
                                    state.overlay_focus = false;
                                }
                                if field.changed() {
                                    state.directory_selected = None;
                                    state.directory_error = None;
                                }
                                browse = button(ui, p, "Browse…", ButtonKind::Secondary).clicked();
                            });
                        });
                        ui.add_space(8.0);
                        ui.add_enabled_ui(!busy, |ui| {
                            reset =
                                button(ui, p, "Use home directory", ButtonKind::Quiet).clicked();
                        });
                        if let Some(error) = &state.directory_error {
                            ui.add_space(8.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(error)
                                        .font(theme::regular(12.0))
                                        .color(p.red),
                                )
                                .wrap(),
                            );
                        }
                    });
                });
            footer(ui, "directory-actions", |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    save |= button(
                        ui,
                        p,
                        if state.directory_pending {
                            "Checking…"
                        } else {
                            "Save"
                        },
                        ButtonKind::Primary,
                    )
                    .clicked();
                });
                cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
            });
        },
    );
    if cancel || output.backdrop_clicked {
        actions.push(Action::CloseOverlay);
    } else if reset {
        actions.push(Action::SetGroupDefaultDirectory(group, None));
    } else if browse {
        actions.push(Action::BrowseGroupDirectory(group));
    } else if save {
        actions.push(Action::SetGroupDefaultDirectory(
            group,
            Some(state.directory_path.clone()),
        ));
    }
}

/// Connects a workspace over SSH, or creates a connected one when `workspace`
/// is `None`. It asks only for the host: signing in happens in the terminal,
/// through the system client, and a new workspace is named after its host.
fn ssh(
    ctx: &egui::Context,
    p: Palette,
    state: &mut UiState,
    workspace: Option<neptune_model::WorkspaceId>,
    group: Option<neptune_model::WorkspaceGroupId>,
    actions: &mut Vec<Action>,
) {
    let title = if workspace.is_some() {
        "Connect over SSH"
    } else {
        "New SSH workspace"
    };
    let mut connect = confirmed_by_enter(ctx, state);
    let mut cancel = false;
    let output = sheet(ctx, p, title, 420.0, SheetPlacement::Center, |ui| {
        sheet_header(ui, p, title, None);
        padded(ui, 20.0, |ui| {
            let width = ui.available_width();
            let host = text_field(
                ui,
                p,
                Id::new("ssh-host"),
                &mut state.ssh_host,
                "user@host",
                "SSH host",
                width,
            );
            if state.overlay_focus && accepts_focus(ui) {
                host.request_focus();
                state.overlay_focus = false;
            }
            ui.add_space(12.0);
            let problem = (!state.ssh_host.trim().is_empty())
                .then(|| neptune_model::Remote::parse(&state.ssh_host).err())
                .flatten();
            let (note, ink) = match problem {
                Some(error) => (error.to_string(), p.red),
                None if workspace.is_some() => (
                    "Every terminal in this workspace restarts on the host. Running processes will stop."
                        .to_owned(),
                    p.secondary,
                ),
                None => (
                    "Terminals open on the host using your SSH configuration and keys. Sign-in prompts appear in the terminal."
                        .to_owned(),
                    p.secondary,
                ),
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(note)
                        .font(theme::regular(12.0))
                        .color(ink),
                )
                .wrap()
                .selectable(false),
            );
        });
        ui.add_space(4.0);
        footer(ui, "ssh-actions", |ui| {
            connect |= button(ui, p, "Connect", ButtonKind::Primary).clicked();
            cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
        });
    });
    if cancel || output.backdrop_clicked {
        actions.push(Action::CloseOverlay);
    } else if connect {
        match neptune_model::Remote::parse(&state.ssh_host) {
            Ok(remote) => actions.push(match group {
                Some(group) => Action::ConnectInGroup(group, remote.destination().to_owned()),
                None => Action::Connect {
                    workspace,
                    destination: remote.destination().to_owned(),
                },
            }),
            // Enter leaves a single-line field. Return the keyboard to the
            // host so it can be corrected without reaching for the pointer.
            Err(_) => state.overlay_focus = true,
        }
    }
}

/// Title, consequence and confirming verb for each close target.
pub fn close_copy(close: Close) -> (&'static str, &'static str, &'static str) {
    match close {
        Close::Pane(_) => (
            "Close terminal?",
            "Any process running in this terminal will stop.",
            "Close",
        ),
        Close::Workspace(_) => (
            "Close workspace?",
            "Every terminal in this workspace will close, and running processes will stop.",
            "Close",
        ),
        Close::Connection(_) => (
            "Disconnect from SSH?",
            "Every terminal in this workspace restarts as a local shell. Processes running in them will stop.",
            "Disconnect",
        ),
        Close::App => (
            "Quit Neptune?",
            "Running processes in all terminals will stop. Workspaces reopen with fresh shells.",
            "Quit",
        ),
    }
}

fn confirm_close(
    ctx: &egui::Context,
    p: Palette,
    close: Close,
    status: CloseStatus,
    browser: bool,
    actions: &mut Vec<Action>,
) {
    let (title, consequence, verb) = if browser && matches!(close, Close::Pane(_)) {
        (
            "Close browser?",
            "This preview will close. Unsaved changes on the page will be lost.",
            "Close",
        )
    } else {
        close_copy(close)
    };
    let message = match status {
        CloseStatus::General => consequence.to_owned(),
        // Keep input ownership while checking, but do not flash a sheet or
        // backdrop for an idle terminal (or an app containing only idle shells).
        CloseStatus::Checking => return,
        CloseStatus::Unknown => {
            format!("Neptune could not check whether processes are running. {consequence}")
        }
        CloseStatus::Running { terminals, unknown } => {
            let detected = if matches!(close, Close::Pane(_)) {
                "A process is still running in this terminal.".to_owned()
            } else if terminals == 1 {
                "A process is still running in one terminal.".to_owned()
            } else {
                format!("Processes are still running in {terminals} terminals.")
            };
            let consequence = match close {
                Close::Pane(_) => "Closing this terminal will stop the process.",
                Close::Workspace(_) => {
                    "Closing this workspace will close its terminals and stop their processes."
                }
                Close::Connection(_) => {
                    "Disconnecting will stop these connections and restart the terminals as local shells."
                }
                Close::App => {
                    "Quitting will stop running processes. Workspaces reopen with fresh shells."
                }
            };
            let uncertainty = if unknown > 0 {
                " Other terminals could not be checked."
            } else {
                ""
            };
            format!("{detected}{uncertainty} {consequence}")
        }
    };
    let mut confirm = false;
    let mut cancel = false;
    let output = sheet(ctx, p, title, 360.0, SheetPlacement::Center, |ui| {
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
        });
        ui.add_space(6.0);
        footer(ui, "confirm-close-actions", |ui| {
            confirm = button(ui, p, verb, ButtonKind::Destructive).clicked();
            cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
        });
    });
    if confirm {
        actions.push(Action::Confirm(close));
    } else if cancel || output.backdrop_clicked {
        actions.push(Action::CancelClose);
    }
}

/// Deleting is not undone, so it names what goes and waits for a click.
fn confirm_delete(
    ctx: &egui::Context,
    p: Palette,
    path: &std::path::Path,
    actions: &mut Vec<Action>,
) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let title = format!("Delete “{}”?", super::helpers::ellipsize(&name, 48));
    let mut confirm = false;
    let mut cancel = false;
    let output = sheet(ctx, p, "Delete file", 380.0, SheetPlacement::Center, |ui| {
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
                    egui::RichText::new(
                        "It is deleted permanently, along with everything inside a folder. \
                         This cannot be undone.",
                    )
                    .font(theme::regular(13.0))
                    .color(p.secondary),
                )
                .wrap()
                .selectable(false),
            );
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(super::helpers::compact_path(path))
                        .font(theme::regular(11.5))
                        .color(p.muted),
                )
                .wrap()
                .selectable(false),
            );
        });
        ui.add_space(6.0);
        footer(ui, "confirm-delete-actions", |ui| {
            confirm = button(ui, p, "Delete", ButtonKind::Destructive).clicked();
            cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
        });
    });
    if confirm {
        actions.push(Action::Explorer(super::explorer::Event::ConfirmDelete));
    } else if cancel || output.backdrop_clicked {
        actions.push(Action::CloseOverlay);
    }
}

/// Removing a project ends its lead and discards its chat. Its agents are
/// the person's from then on, so nothing of theirs is stopped.
fn remove_project(
    ctx: &egui::Context,
    p: Palette,
    project: neptune_model::ProjectId,
    actions: &mut Vec<Action>,
) {
    let mut confirm = false;
    let mut cancel = false;
    let output = sheet(
        ctx,
        p,
        "Remove project",
        380.0,
        SheetPlacement::Center,
        |ui| {
            ui.add_space(22.0);
            padded(ui, 22.0, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("Remove this project?")
                            .font(theme::semibold(15.0))
                            .color(p.fg),
                    )
                    .wrap()
                    .selectable(false),
                );
                ui.add_space(6.0);
                for (lead, text) in [
                    (
                        "Deleted",
                        "The chat with its lead, the project's notes and its watches. Its \
                         lead stops.",
                    ),
                    (
                        "Stays",
                        "Its agents keep running: each terminal gets a tab in this workspace. \
                         Worktrees, branches and what Claude Code and Codex keep of their own \
                         conversations are not touched.",
                    ),
                ] {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(lead)
                                .font(theme::medium(12.0))
                                .color(p.fg),
                        )
                        .selectable(false),
                    );
                    ui.add_space(2.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .font(theme::regular(13.0))
                                .color(p.secondary),
                        )
                        .wrap()
                        .selectable(false),
                    );
                    ui.add_space(8.0);
                }
            });
            ui.add_space(6.0);
            footer(ui, "remove-project-actions", |ui| {
                confirm = button(ui, p, "Remove", ButtonKind::Destructive).clicked();
                cancel = button(ui, p, "Cancel", ButtonKind::Secondary).clicked();
            });
        },
    );
    if confirm {
        actions.push(Action::Project(super::project::Event::ConfirmRemove(
            project,
        )));
    } else if cancel || output.backdrop_clicked {
        actions.push(Action::CloseOverlay);
    }
}

pub fn show(ctx: &egui::Context, p: Palette, state: &mut UiState, actions: &mut Vec<Action>) {
    match state.overlay {
        OverlayState::DeleteFile => match &state.explorer.delete {
            Some(path) => confirm_delete(ctx, p, path, actions),
            // Nothing to confirm: the sheet has no reason to hold the window.
            None => actions.push(Action::CloseOverlay),
        },
        OverlayState::GroupDefaultDirectory(group) => {
            default_directory(ctx, p, state, group, actions)
        }
        target @ (OverlayState::Rename(_)
        | OverlayState::RenameGroup(_)
        | OverlayState::RenameProject(_)
        | OverlayState::NewGroup) => rename(ctx, p, state, target, actions),
        OverlayState::RemoveProject(project) => remove_project(ctx, p, project, actions),
        OverlayState::Ssh(workspace) => ssh(ctx, p, state, workspace, None, actions),
        OverlayState::SshInGroup(group) => ssh(ctx, p, state, None, Some(group), actions),
        OverlayState::ConfirmClose(close) => confirm_close(
            ctx,
            p,
            close,
            state.close_status,
            state.close_browser,
            actions,
        ),
        OverlayState::NewWorktree(_) => {
            let enter = confirmed_by_enter(ctx, state);
            super::worktrees::new_agent(ctx, p, state, enter, accepts_focus, actions)
        }
        OverlayState::RemoveWorktree => match &state.worktree.removal {
            Some(removal) => super::worktrees::remove(ctx, p, removal, actions),
            // Nothing to confirm: the sheet has no reason to hold the window.
            None => actions.push(Action::CloseOverlay),
        },
        _ => {}
    }
    if let Some(error) = &state.error {
        if toast(ctx, p, error) {
            actions.push(Action::DismissError);
        }
    } else if let Some((said, shown)) = &state.notice {
        // What was done is said for a while; what went wrong waits to be read.
        let left = UiState::NOTICE_SECONDS - (ctx.input(|i| i.time) - shown);
        if left <= 0.0
            || super::controls::notice(ctx, p, said, (crate::icons::Icon::Check, p.green))
        {
            state.notice = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
    }
    if let Some(level) = &state.level
        && !super::controls::level(ctx, p, level)
    {
        state.level = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;
    use neptune_model::{PaneId, WorkspaceId};

    #[test]
    fn close_checks_render_nothing_until_confirmation_is_needed() {
        for close in [
            Close::Pane(PaneId::new(1)),
            Close::Workspace(WorkspaceId::new(1)),
            Close::Connection(WorkspaceId::new(1)),
            Close::App,
        ] {
            for status in [
                CloseStatus::General,
                CloseStatus::Running {
                    terminals: 1,
                    unknown: 0,
                },
                CloseStatus::Unknown,
            ] {
                let ctx = egui::Context::default();
                ctx.set_fonts(crate::platform::fonts::bundled_definitions());
                let config = crate::config::Config::default();
                theme::apply(&ctx, &config);
                let p = Palette::for_config(&config);
                let mut state = UiState::default();
                let mut actions = Vec::new();
                let mut frame = |state: &mut UiState| {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                Pos2::ZERO,
                                vec2(640.0, 480.0),
                            )),
                            ..Default::default()
                        },
                        |ui| show(ui.ctx(), p, state, &mut actions),
                    );
                    output.textures_delta.clear();
                    output.shapes
                };
                let empty = frame(&mut state).len();
                state.overlay = OverlayState::ConfirmClose(close);
                state.close_status = CloseStatus::Checking;
                // Hold the asynchronous check pending across visible frames.
                // Neither the sheet nor its dimming backdrop may flash.
                for _ in 0..4 {
                    assert_eq!(frame(&mut state).len(), empty, "{close:?}");
                }
                state.close_status = status;
                for _ in 0..3 {
                    frame(&mut state);
                }
                assert!(frame(&mut state).iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == close_copy(close).0)
                }), "{close:?}: {status:?}");
                assert!(actions.is_empty());
            }
        }
    }

    #[test]
    fn removing_a_project_says_what_is_deleted_and_what_stays_in_a_narrow_window_too() {
        for size in [vec2(900.0, 640.0), vec2(360.0, 300.0)] {
            let ctx = egui::Context::default();
            ctx.set_fonts(crate::platform::fonts::bundled_definitions());
            let config = crate::config::Config::default();
            theme::apply(&ctx, &config);
            let p = Palette::for_config(&config);
            let project = neptune_model::ProjectId::new(4);
            let mut state = UiState {
                overlay: OverlayState::RemoveProject(project),
                ..UiState::default()
            };
            let mut actions = Vec::new();
            let screen = egui::Rect::from_min_size(Pos2::ZERO, size);
            let mut texts = Vec::new();
            for _ in 0..4 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| show(ui.ctx(), p, &mut state, &mut actions),
                );
                output.textures_delta.clear();
                texts = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some((
                            text.galley.text().to_owned(),
                            text.visual_bounding_rect(),
                            shape.clip_rect,
                        )),
                        _ => None,
                    })
                    .collect();
            }
            let all: String = texts
                .iter()
                .map(|(text, ..)| text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            for wanted in [
                "Remove this project?",
                "Deleted",
                "The chat with its lead, the project's notes and its watches.",
                "Stays",
                "each terminal gets a tab in this workspace",
                "Worktrees, branches and what Claude Code and Codex keep",
                "Remove",
                "Cancel",
            ] {
                assert!(all.contains(wanted), "{wanted} at {size:?}: {all}");
            }
            // Nothing is asked for before the person answers, and both
            // answers stay within the window.
            assert!(actions.is_empty());
            for (text, rect, clip) in &texts {
                if text == "Remove" || text == "Cancel" {
                    assert!(
                        screen.contains_rect(*rect) && clip.contains_rect(*rect),
                        "{text} at {rect:?} in {size:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn directory_validation_errors_keep_save_and_cancel_inside_their_clip_rect() {
        for size in [vec2(640.0, 400.0), vec2(480.0, 320.0), vec2(360.0, 240.0)] {
            let ctx = egui::Context::default();
            ctx.set_fonts(crate::platform::fonts::bundled_definitions());
            let config = crate::config::Config::default();
            theme::apply(&ctx, &config);
            let p = Palette::for_config(&config);
            let mut state = UiState {
                overlay: OverlayState::GroupDefaultDirectory(neptune_model::WorkspaceGroupId::new(
                    1,
                )),
                directory_group: "Project".into(),
                ..Default::default()
            };
            for frame in 0..8 {
                if frame == 4 {
                    state.directory_error =
                        Some("Cannot open this directory: access denied. ".repeat(5));
                }
                let mut actions = Vec::new();
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ui| show(ui.ctx(), p, &mut state, &mut actions),
                );
                output.textures_delta.clear();
                if frame >= 5 {
                    let mut buttons = 0;
                    for shape in &output.shapes {
                        if let egui::Shape::Text(text) = &shape.shape
                            && matches!(text.galley.text(), "Browse…" | "Save" | "Cancel")
                        {
                            buttons += 1;
                            assert!(
                                shape
                                    .clip_rect
                                    .contains_rect(text.galley.rect.translate(text.pos.to_vec2())),
                                "{} clipped at {size:?}",
                                text.galley.text()
                            );
                        }
                    }
                    assert_eq!(buttons, 3, "browse and footer actions visible at {size:?}");
                }
            }
        }
    }

    #[test]
    fn default_directory_owns_focus_and_only_a_fresh_enter_submits_its_captured_target() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let config = crate::config::Config::default();
        theme::apply(&ctx, &config);
        let p = Palette::for_config(&config);
        let group = neptune_model::WorkspaceGroupId::new(7);
        let mut state = UiState {
            overlay: OverlayState::GroupDefaultDirectory(group),
            overlay_focus: true,
            directory_path: "/project".into(),
            ..Default::default()
        };
        let frame = |state: &mut UiState, events| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(640.0, 400.0))),
                    events,
                    ..Default::default()
                },
                |ui| show(ui.ctx(), p, state, &mut actions),
            );
            output.textures_delta.clear();
            actions
        };
        let enter = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(frame(&mut state, vec![enter(true)]).is_empty());
        for _ in 0..3 {
            assert!(frame(&mut state, vec![]).is_empty());
        }
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(Id::new("group-default-directory"))
        );
        assert!(frame(&mut state, vec![enter(true)]).is_empty());
        assert!(frame(&mut state, vec![enter(false)]).is_empty());
        state.directory_pending = true;
        assert!(frame(&mut state, vec![enter(true)]).is_empty());
        assert!(frame(&mut state, vec![enter(false)]).is_empty());
        state.directory_pending = false;
        let actions = frame(&mut state, vec![enter(true)]);
        assert!(
            matches!(actions.as_slice(), [Action::SetGroupDefaultDirectory(target, Some(path))] if *target == group && path == "/project")
        );
        assert_eq!(
            state.overlay,
            OverlayState::GroupDefaultDirectory(group),
            "validation owns closing the sheet"
        );
        assert!(frame(&mut state, vec![enter(false)]).is_empty());
        ctx.memory_mut(|memory| memory.request_focus(Id::new("group-default-directory")));
        assert!(
            frame(
                &mut state,
                vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }]
            )
            .is_empty()
        );
        assert!(
            frame(
                &mut state,
                vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }]
            )
            .is_empty()
        );
        let actions = frame(&mut state, vec![enter(true)]);
        assert!(
            matches!(actions.as_slice(), [Action::BrowseGroupDirectory(target)] if *target == group),
            "Enter on Browse opens the picker without submitting the path"
        );
    }

    #[test]
    fn rename_gives_its_field_the_keyboard_once_visible() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let config = crate::config::Config::default();
        theme::apply(&ctx, &config);
        let p = Palette::for_config(&config);
        let mut state = UiState {
            overlay: OverlayState::Rename(WorkspaceId::new(1)),
            overlay_focus: true,
            ..UiState::default()
        };
        let mut actions = Vec::new();
        for _ in 0..4 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0))),
                    ..Default::default()
                },
                |ui| show(ui.ctx(), p, &mut state, &mut actions),
            );
            output.textures_delta.clear();
        }
        assert!(!state.overlay_focus, "the request is made exactly once");
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(Id::new("workspace-rename"))
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn only_a_fresh_enter_after_the_dialog_is_shown_confirms_it() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let config = crate::config::Config::default();
        theme::apply(&ctx, &config);
        let p = Palette::for_config(&config);
        let mut state = UiState {
            overlay: OverlayState::Rename(WorkspaceId::new(1)),
            overlay_focus: true,
            rename_name: "Renamed".into(),
            ..UiState::default()
        };
        // The toolkit derives key repeat itself: a press with no release
        // since the previous press is a repeat.
        let enter = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let frame = |state: &mut UiState, events: Vec<egui::Event>| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0))),
                    events,
                    ..Default::default()
                },
                |ui| show(ui.ctx(), p, state, &mut actions),
            );
            output.textures_delta.clear();
            actions
        };
        // The press that opened the dialog arrives with its first frame.
        assert!(frame(&mut state, vec![enter(true)]).is_empty());
        // The key is still held, so it keeps repeating into the dialog.
        assert!(frame(&mut state, vec![enter(true)]).is_empty());
        assert!(frame(&mut state, vec![enter(true)]).is_empty());
        assert!(frame(&mut state, vec![enter(false)]).is_empty());
        let actions = frame(&mut state, vec![enter(true)]);
        assert!(
            matches!(&actions[..], [Action::SetName(workspace, name), Action::CloseOverlay]
                if *workspace == WorkspaceId::new(1) && name == "Renamed"),
            "a deliberate Enter renames the original workspace"
        );
    }

    #[test]
    fn each_close_target_names_its_own_consequence() {
        let pane = close_copy(Close::Pane(PaneId::new(1)));
        let workspace = close_copy(Close::Workspace(WorkspaceId::new(1)));
        let app = close_copy(Close::App);
        let connection = close_copy(Close::Connection(WorkspaceId::new(1)));
        assert_eq!(pane.0, "Close terminal?");
        assert_ne!(pane.0, workspace.0);
        assert_ne!(workspace.1, app.1);
        assert_ne!(connection.1, workspace.1);
        assert_eq!(
            (pane.2, app.2, connection.2),
            ("Close", "Quit", "Disconnect")
        );
    }

    /// Runs the sheet for a frame with a deliberate Enter. Returns its actions
    /// and whether the host field asked for the keyboard back.
    fn confirm_ssh(host: &str, workspace: Option<WorkspaceId>) -> (Vec<Action>, bool) {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let config = crate::config::Config::default();
        theme::apply(&ctx, &config);
        let p = Palette::for_config(&config);
        let mut state = UiState {
            overlay: OverlayState::Ssh(workspace),
            ssh_host: host.into(),
            ..UiState::default()
        };
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0))),
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| show(ui.ctx(), p, &mut state, &mut actions),
        );
        output.textures_delta.clear();
        (actions, state.overlay_focus)
    }

    #[test]
    fn the_ssh_sheet_connects_its_own_target_and_only_to_a_usable_host() {
        let existing = WorkspaceId::new(7);
        let (actions, refocus) = confirm_ssh(" me@devbox ", Some(existing));
        assert!(!refocus);
        assert!(matches!(
            &actions[..],
            [Action::Connect { workspace: Some(id), destination }]
                if *id == existing && destination == "me@devbox"
        ));
        assert!(matches!(
            &confirm_ssh("devbox", None).0[..],
            [Action::Connect { workspace: None, destination }] if destination == "devbox"
        ));
        // Nothing is sent for an empty host, or one the client could misread,
        // and the field takes the keyboard back so it can be corrected.
        for host in ["", "  ", "-oProxyCommand=id", "dev box"] {
            let (actions, refocus) = confirm_ssh(host, None);
            assert!(actions.is_empty() && refocus, "{host:?}");
        }
    }
}
