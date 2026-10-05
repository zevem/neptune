//! The command palette: every action, searchable, with its shortcut.
use super::helpers::{
    SheetPlacement, bare_text_edit, edit_shortcut, elided, galley_at, keycaps, place, sheet,
    shortcut,
};
use super::{Action, UiState, WorkspaceView};
use crate::{
    config::{Config, Theme},
    icons::{self, Icon},
    theme::{self, Palette},
};
use eframe::egui::{
    self, Align, Align2, Id, Key, Layout, Modifiers, Pos2, Rect, Sense, Vec2, WidgetInfo,
    WidgetType, vec2,
};
use neptune_model::{Axis, Destination, FocusDirection, PaneId, WorkspaceId};

pub struct PaletteView<'a> {
    pub pane: Option<PaneId>,
    /// The focused terminal runs on this machine, where git can be run.
    pub local: bool,
    /// The branch of the worktree the focused terminal is in.
    pub worktree: Option<&'a str>,
    pub layout: Option<&'a neptune_model::Layout>,
    pub workspaces: &'a [WorkspaceView],
    pub groups: &'a [neptune_model::WorkspaceGroup],
    pub active: Option<WorkspaceId>,
    pub config: &'a Config,
    pub zoomed: bool,
    /// A message is showing and can be dismissed.
    pub message: bool,
}

struct Command {
    group: &'static str,
    icon: Icon,
    title: String,
    shortcut: String,
    actions: Vec<Action>,
    /// Distinguishes commands that can share a title, such as workspaces
    /// with the same name.
    target: u64,
}

fn command(
    group: &'static str,
    icon: Icon,
    title: impl Into<String>,
    shortcut: impl Into<String>,
    actions: impl Into<Vec<Action>>,
) -> Command {
    Command {
        group,
        icon,
        title: title.into(),
        shortcut: shortcut.into(),
        actions: actions.into(),
        target: 0,
    }
}

/// Commands that apply now. Pane commands exist only with a focused terminal,
/// and each captures its target when the palette is drawn.
fn commands(view: &PaletteView) -> Vec<Command> {
    let mut list = Vec::new();
    if let Some(pane) = view.pane {
        list.extend([
            command(
                "Terminal",
                Icon::Plus,
                "New tab",
                shortcut("T"),
                [Action::NewTab(pane)],
            ),
            command(
                "Terminal",
                Icon::SplitVertical,
                "Split right",
                shortcut("D"),
                [Action::Split(pane, Axis::Vertical)],
            ),
            command(
                "Terminal",
                Icon::SplitHorizontal,
                "Split below",
                shortcut("E"),
                [Action::Split(pane, Axis::Horizontal)],
            ),
            command(
                "Terminal",
                if view.zoomed {
                    Icon::Minimize
                } else {
                    Icon::Maximize
                },
                if view.zoomed {
                    "Show all terminals"
                } else {
                    "Zoom terminal"
                },
                shortcut("Enter"),
                [Action::Zoom],
            ),
            command(
                "Terminal",
                Icon::Search,
                "Find in terminal",
                shortcut("F"),
                [Action::Find],
            ),
            command(
                "Terminal",
                Icon::Copy,
                "Copy selection",
                shortcut("C"),
                [Action::Copy(pane)],
            ),
            command(
                "Terminal",
                Icon::Clipboard,
                "Paste",
                shortcut("V"),
                [Action::Paste(pane)],
            ),
            command(
                "Terminal",
                Icon::Eraser,
                "Clear scrollback",
                "",
                [Action::Clear(pane)],
            ),
            command(
                "Terminal",
                Icon::Refresh,
                "Restart terminal",
                "",
                [Action::Restart(pane)],
            ),
            command(
                "Terminal",
                Icon::Close,
                "Close terminal",
                shortcut("W"),
                [Action::ClosePane(pane)],
            ),
        ]);
        if view.local {
            // Beside "New tab": it opens one, in a worktree of its own.
            list.insert(
                1,
                command(
                    "Terminal",
                    Icon::Branch,
                    "New agent in worktree",
                    shortcut("G"),
                    [Action::Worktree(super::worktrees::Event::New(pane))],
                ),
            );
        }
        if let Some(branch) = view.worktree {
            list.push(command(
                "Terminal",
                Icon::Trash,
                format!("Remove worktree {branch}"),
                "",
                [Action::Worktree(super::worktrees::Event::RemoveOf(pane))],
            ));
        }
        if let Some(layout) = view.layout {
            for (forward, title, key) in
                [(true, "Next tab", "PgDn"), (false, "Previous tab", "PgUp")]
            {
                if let Some(target) = layout.next_tab(pane, forward) {
                    list.push(command(
                        "Terminal",
                        Icon::Terminal,
                        title,
                        shortcut(key),
                        [Action::Focus(target)],
                    ));
                }
            }
            for (direction, title, arrow) in [
                (FocusDirection::Left, "Focus pane to the left", "←"),
                (FocusDirection::Right, "Focus pane to the right", "→"),
                (FocusDirection::Up, "Focus pane above", "↑"),
                (FocusDirection::Down, "Focus pane below", "↓"),
            ] {
                if let Some(target) = layout.adjacent(pane, direction) {
                    list.push(command(
                        "Terminal",
                        Icon::Grid,
                        title,
                        format!("Ctrl+Shift+{arrow}"),
                        [Action::Focus(target)],
                    ));
                }
            }
        }
    }
    list.push(command(
        "Workspace",
        Icon::Plus,
        "New workspace",
        shortcut("N"),
        [Action::New],
    ));
    list.push(command(
        "Workspace",
        Icon::Globe,
        "New SSH workspace",
        "",
        [Action::Ssh(None)],
    ));
    list.push(command(
        "Workspace",
        Icon::Folder,
        "New workspace group",
        "",
        [Action::NewGroup],
    ));
    for group in view.groups {
        for (title, action) in [
            (
                format!("New workspace in {}", group.name()),
                Action::NewInGroup(group.id()),
            ),
            (
                format!("New SSH workspace in {}", group.name()),
                Action::SshInGroup(group.id()),
            ),
            (
                format!("Rename group {}", group.name()),
                Action::RenameGroup(group.id()),
            ),
            (
                format!("Set default directory for group {}", group.name()),
                Action::GroupDefaultDirectory(group.id()),
            ),
            (
                format!(
                    "{} group {}",
                    if group.collapsed() {
                        "Expand"
                    } else {
                        "Collapse"
                    },
                    group.name()
                ),
                Action::SetGroupCollapsed(group.id(), !group.collapsed()),
            ),
            (
                format!("Remove group {} (keep workspaces)", group.name()),
                Action::RemoveGroup(group.id()),
            ),
        ] {
            list.push(Command {
                target: group.id().get(),
                ..command("Groups", Icon::Folder, title, "", [action])
            });
        }
        if let Some(workspace) = view
            .active
            .and_then(|id| view.workspaces.iter().find(|w| w.id == id))
            && workspace.group != Some(group.id())
        {
            list.push(Command {
                target: group.id().get(),
                ..command(
                    "Groups",
                    Icon::Folder,
                    format!("Move workspace to {}", group.name()),
                    "",
                    [Action::MoveToGroup(workspace.id, Some(group.id()))],
                )
            });
        }
    }
    if let Some(workspace) = view
        .active
        .and_then(|id| view.workspaces.iter().find(|w| w.id == id))
        && workspace.group.is_some()
    {
        list.push(command(
            "Groups",
            Icon::Grid,
            "Move workspace out of group",
            "",
            [Action::MoveToGroup(workspace.id, None)],
        ));
    }
    if let Some(active) = view.active {
        let remote = view
            .workspaces
            .iter()
            .any(|workspace| workspace.id == active && workspace.remote.is_some());
        list.extend([
            command(
                "Workspace",
                Icon::Pencil,
                "Rename workspace",
                "",
                [Action::Rename(active)],
            ),
            if remote {
                command(
                    "Workspace",
                    Icon::Globe,
                    "Disconnect workspace from SSH",
                    "",
                    [Action::Disconnect(active)],
                )
            } else {
                command(
                    "Workspace",
                    Icon::Globe,
                    "Connect workspace over SSH",
                    "",
                    [Action::Ssh(Some(active))],
                )
            },
            command(
                "Workspace",
                Icon::Close,
                "Close workspace",
                "",
                [Action::CloseWorkspace(active)],
            ),
        ]);
        // Reordering by keyboard; the sidebar offers the same by dragging.
        if let Some(workspace) = view.workspaces.iter().find(|w| w.id == active) {
            let siblings: Vec<usize> = view
                .workspaces
                .iter()
                .enumerate()
                .filter(|(_, w)| w.group == workspace.group)
                .map(|(index, _)| index)
                .collect();
            let index = siblings
                .iter()
                .position(|index| view.workspaces[*index].id == active)
                .unwrap_or(0);
            if index > 0 {
                list.push(command(
                    "Workspace",
                    Icon::ArrowUp,
                    "Move workspace up",
                    "",
                    [Action::MoveWorkspace(active, siblings[index - 1])],
                ));
            }
            if index + 1 < siblings.len() {
                list.push(command(
                    "Workspace",
                    Icon::ArrowDown,
                    "Move workspace down",
                    "",
                    [Action::MoveWorkspace(active, siblings[index + 1])],
                ));
            }
        }
    }
    for (index, workspace) in view.workspaces.iter().enumerate() {
        if Some(workspace.id) == view.active {
            continue;
        }
        list.push(Command {
            target: workspace.id.get(),
            ..command(
                "Go to",
                Icon::ArrowUpRight,
                format!("Go to {}", workspace.name),
                if index < 9 {
                    shortcut(&(index + 1).to_string())
                } else {
                    String::new()
                },
                [Action::SelectWorkspace(workspace.id)],
            )
        });
    }
    if let Some(pane) = view.pane {
        // A terminal keeps its session, so only workspaces on the same
        // machine as its own can take it.
        let machine = view
            .workspaces
            .iter()
            .find(|workspace| Some(workspace.id) == view.active)
            .map(|workspace| &workspace.remote);
        for workspace in view.workspaces {
            if Some(workspace.id) == view.active || Some(&workspace.remote) != machine {
                continue;
            }
            list.push(Command {
                target: workspace.id.get(),
                ..command(
                    "Move to",
                    Icon::ArrowUpRight,
                    format!("Move terminal to {}", workspace.name),
                    "",
                    [Action::MovePane(pane, Destination::Workspace(workspace.id))],
                )
            });
        }
    }
    if view.message {
        // Escape belongs to the shell while a terminal is focused, so the
        // message has its own keyboard path.
        list.push(command(
            "View",
            Icon::Close,
            "Dismiss message",
            "",
            [Action::DismissError],
        ));
    }
    list.extend([
        command(
            "View",
            Icon::Sidebar,
            "Toggle sidebar",
            shortcut("B"),
            [Action::ToggleSidebar],
        ),
        command(
            "View",
            Icon::PanelRight,
            "Toggle right panel",
            shortcut("O"),
            [Action::Panel(super::panel::Event::Toggle)],
        ),
        command(
            "View",
            Icon::Files,
            "Show files",
            "",
            [Action::Panel(super::panel::Event::Show(
                super::panel::Tab::Files,
            ))],
        ),
        command(
            "View",
            Icon::Terminal,
            "Show agents",
            "",
            [Action::Panel(super::panel::Event::Show(
                super::panel::Tab::Agents,
            ))],
        ),
        command(
            "View",
            Icon::Branch,
            "Show changes",
            "",
            [Action::Panel(super::panel::Event::Show(
                super::panel::Tab::Changes,
            ))],
        ),
        command(
            "View",
            Icon::Bell,
            "Notifications",
            "",
            [Action::Notifications],
        ),
        command(
            "View",
            Icon::Settings,
            "Preferences",
            edit_shortcut(","),
            [Action::Settings],
        ),
    ]);
    for (title, key, action) in [
        ("Zoom app in", "+", Action::ZoomUiIn),
        ("Zoom app out", "-", Action::ZoomUiOut),
        ("Reset app zoom", "0", Action::ResetUiZoom),
    ] {
        list.push(command(
            "View",
            Icon::TextSize,
            title,
            edit_shortcut(key),
            [action],
        ));
    }
    list.push(command(
        "Appearance",
        Icon::Settings,
        "Browse themes",
        "",
        [Action::Themes],
    ));
    for (theme, name, icon) in [
        (Theme::Graphite, "Graphite", Icon::Moon),
        (Theme::Dusk, "Dusk", Icon::Moon),
        (Theme::Light, "Light", Icon::Sun),
    ] {
        if theme != view.config.theme {
            list.push(command(
                "Appearance",
                icon,
                format!("Use {name} theme"),
                "",
                [Action::Preferences(Config {
                    theme,
                    ..view.config.clone()
                })],
            ));
        }
    }
    list
}

/// Every word of the query must appear in the title or its group.
fn matches(command: &Command, query: &str) -> bool {
    let haystack = format!("{} {}", command.title, command.group).to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| haystack.contains(word))
}

pub fn show(
    ctx: &egui::Context,
    p: Palette,
    state: &mut UiState,
    view: &PaletteView,
    actions: &mut Vec<Action>,
) {
    let screen = ctx.content_rect();
    let commands: Vec<Command> = commands(view)
        .into_iter()
        .filter(|command| matches(command, &state.palette_query))
        .collect();
    let grouped = state.palette_query.trim().is_empty();
    let mut chosen = None;
    let output = sheet(
        ctx,
        p,
        "Commands",
        560.0,
        SheetPlacement::Top((screen.height() * 0.14).clamp(24.0, 110.0)),
        |ui| {
            // Arrow keys move the highlight; the field keeps typing focus.
            let mut moved = false;
            if !commands.is_empty() {
                let down = ui.input_mut(|input| {
                    input.count_and_consume_key(Modifiers::NONE, Key::ArrowDown)
                });
                let up = ui
                    .input_mut(|input| input.count_and_consume_key(Modifiers::NONE, Key::ArrowUp));
                if down + up > 0 {
                    let count = commands.len() as i64;
                    let next = state.palette_selected as i64 + down as i64 - up as i64;
                    state.palette_selected = next.rem_euclid(count) as usize;
                    moved = true;
                }
            }
            state.palette_selected = state.palette_selected.min(commands.len().saturating_sub(1));
            let run = ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));

            let (_, field) = ui.allocate_space(vec2(ui.available_width(), 52.0));
            icons::paint(
                ui.painter(),
                Rect::from_center_size(
                    Pos2::new(field.left() + 24.0, field.center().y),
                    Vec2::splat(16.0),
                ),
                Icon::Search,
                p.secondary,
            );
            let input = Rect::from_min_max(
                Pos2::new(field.left() + 44.0, field.top()),
                Pos2::new(field.right() - 16.0, field.bottom()),
            );
            let response = place(
                ui,
                input,
                Layout::left_to_right(Align::Center),
                "palette-field",
                |ui| {
                    bare_text_edit(
                        ui,
                        Id::new("palette-input"),
                        &mut state.palette_query,
                        "Search commands",
                        15.0,
                        input.width(),
                    )
                },
            );
            response
                .widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, "Command search"));
            // Requested only when lost: a request interrupts input-method
            // composition, so it must not repeat every frame.
            if !response.has_focus() {
                response.request_focus();
            }
            if response.changed() {
                state.palette_selected = 0;
            }
            ui.painter()
                .line_segment([field.left_bottom(), field.right_bottom()], p.hairline());

            let list_height = (screen.height() - 260.0).clamp(120.0, 372.0);
            egui::ScrollArea::vertical()
                .id_salt("palette-commands")
                .max_height(list_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    if commands.is_empty() {
                        let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 64.0));
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            "No matching commands",
                            theme::regular(13.0),
                            p.muted,
                        );
                    }
                    let mut group = "";
                    for (index, command) in commands.iter().enumerate() {
                        if grouped && command.group != group {
                            group = command.group;
                            let (_, rect) = ui.allocate_space(vec2(ui.available_width(), 26.0));
                            ui.painter().text(
                                Pos2::new(rect.left() + 18.0, rect.center().y + 2.0),
                                Align2::LEFT_CENTER,
                                group,
                                theme::medium(11.0),
                                p.muted,
                            );
                        }
                        let (_, slot) = ui.allocate_space(vec2(ui.available_width(), 36.0));
                        let row = slot.shrink2(vec2(6.0, 0.0));
                        let response = ui.interact(
                            row,
                            ui.id()
                                .with(("palette-command", &command.title, command.target)),
                            Sense::click(),
                        );
                        response.widget_info(|| {
                            WidgetInfo::labeled(WidgetType::Button, true, &command.title)
                        });
                        // The pointer selects only when it moves, so it never
                        // fights the keyboard highlight.
                        if response.hovered()
                            && ui.input(|input| input.pointer.delta() != Vec2::ZERO)
                        {
                            state.palette_selected = index;
                        }
                        let selected = index == state.palette_selected;
                        let painter = ui.painter();
                        let (ink, hint) = if selected {
                            painter.rect_filled(row, 8, p.accent);
                            (p.on_accent, p.on_accent)
                        } else {
                            (p.fg, p.muted)
                        };
                        icons::paint(
                            painter,
                            Rect::from_center_size(
                                Pos2::new(row.left() + 19.0, row.center().y),
                                Vec2::splat(15.0),
                            ),
                            command.icon,
                            if selected { ink } else { p.secondary },
                        );
                        let caps = keycaps(
                            painter,
                            Pos2::new(row.right() - 9.0, row.center().y),
                            &command.shortcut,
                            hint,
                        );
                        galley_at(
                            painter,
                            Pos2::new(row.left() + 40.0, row.center().y),
                            elided(
                                painter,
                                &command.title,
                                theme::regular(13.0),
                                ink,
                                row.width() - 58.0 - caps,
                            ),
                        );
                        if selected && moved {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() || (selected && run) {
                            chosen = Some(index);
                        }
                    }
                    ui.add_space(6.0);
                });

            let (_, footer) = ui.allocate_space(vec2(ui.available_width(), 32.0));
            ui.painter()
                .line_segment([footer.left_top(), footer.right_top()], p.hairline());
            let mut right = footer.right() - 14.0;
            for (keys, label) in [("Esc", "Close"), ("Enter", "Run"), ("↑+↓", "Navigate")] {
                let text = ui.painter().text(
                    Pos2::new(right, footer.center().y),
                    Align2::RIGHT_CENTER,
                    label,
                    theme::regular(11.0),
                    p.muted,
                );
                right = text.left() - 6.0;
                right -= keycaps(
                    ui.painter(),
                    Pos2::new(right, footer.center().y),
                    keys,
                    p.muted,
                ) + 12.0;
            }
        },
    );
    if let Some(index) = chosen {
        // Close first: the command itself may open another overlay.
        actions.push(Action::CloseOverlay);
        actions.extend(commands[index].actions.iter().cloned());
    } else if output.backdrop_clicked {
        actions.push(Action::CloseOverlay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_directory_commands_capture_each_group_even_with_duplicate_names() {
        let config = Config::default();
        let model = neptune_model::Model::restore_grouped(
            Vec::new(),
            [1, 2]
                .into_iter()
                .map(|id| neptune_model::WorkspaceGroupSpec {
                    id: neptune_model::WorkspaceGroupId::new(id),
                    name: "Projects".into(),
                    collapsed: true,
                    default_directory: None,
                })
                .collect(),
            None,
            true,
            Default::default(),
        )
        .unwrap();
        let list = commands(&PaletteView {
            groups: model.groups(),
            ..view(&config, &[])
        });
        let targets: Vec<_> = list
            .iter()
            .flat_map(|command| &command.actions)
            .filter_map(|action| match action {
                Action::GroupDefaultDirectory(group) => Some(group.get()),
                _ => None,
            })
            .collect();
        assert_eq!(targets, [1, 2]);
    }

    fn view<'a>(config: &'a Config, workspaces: &'a [WorkspaceView]) -> PaletteView<'a> {
        PaletteView {
            pane: None,
            local: true,
            worktree: None,
            layout: None,
            workspaces,
            groups: &[],
            active: None,
            config,
            zoomed: false,
            message: false,
        }
    }

    #[test]
    fn same_named_workspaces_stay_distinct_and_messages_can_be_dismissed() {
        let config = Config::default();
        let workspaces: Vec<WorkspaceView> = [1, 2]
            .into_iter()
            .map(|id| WorkspaceView {
                unread: 0,
                alert: None,
                group: None,
                id: WorkspaceId::new(id),
                name: "app".into(),
                cwd: "/srv/app".into(),
                remote: None,
                branch: None,
                panes: 1,
                running: true,
            })
            .collect();
        let list = commands(&PaletteView {
            message: true,
            ..view(&config, &workspaces)
        });
        let targets: Vec<u64> = list
            .iter()
            .filter(|command| command.title == "Go to app")
            .map(|command| command.target)
            .collect();
        assert_eq!(targets, [1, 2]);
        assert!(list.iter().any(|command| {
            command.title == "Dismiss message"
                && matches!(command.actions[..], [Action::DismissError])
        }));
        assert!(
            !commands(&view(&config, &workspaces))
                .iter()
                .any(|command| command.title == "Dismiss message")
        );
    }

    #[test]
    fn the_active_workspace_can_move_only_where_there_is_room() {
        let config = Config::default();
        let workspaces: Vec<WorkspaceView> = [4, 7, 9]
            .into_iter()
            .map(|id| WorkspaceView {
                unread: 0,
                alert: None,
                group: None,
                id: WorkspaceId::new(id),
                name: "app".into(),
                cwd: "/srv/app".into(),
                remote: None,
                branch: None,
                panes: 1,
                running: true,
            })
            .collect();
        let moves = |active: u64| -> Vec<(String, usize)> {
            commands(&PaletteView {
                active: Some(WorkspaceId::new(active)),
                ..view(&config, &workspaces)
            })
            .into_iter()
            .filter_map(|command| match command.actions[..] {
                [Action::MoveWorkspace(id, index)] if id == WorkspaceId::new(active) => {
                    Some((command.title, index))
                }
                _ => None,
            })
            .collect()
        };
        assert_eq!(moves(4), [("Move workspace down".to_owned(), 1)]);
        assert_eq!(
            moves(7),
            [
                ("Move workspace up".to_owned(), 0),
                ("Move workspace down".to_owned(), 2)
            ]
        );
        assert_eq!(moves(9), [("Move workspace up".to_owned(), 1)]);
    }

    #[test]
    fn ssh_commands_target_the_active_workspace_and_offer_the_reverse_when_connected() {
        let config = Config::default();
        let workspaces: Vec<WorkspaceView> = [(1, None), (2, Some("me@devbox"))]
            .into_iter()
            .map(|(id, remote): (u64, Option<&str>)| WorkspaceView {
                unread: 0,
                alert: None,
                group: None,
                id: WorkspaceId::new(id),
                name: "app".into(),
                cwd: "/srv/app".into(),
                remote: remote.map(str::to_owned),
                branch: None,
                panes: 1,
                running: true,
            })
            .collect();
        let titles = |active: Option<u64>| -> Vec<(String, Vec<Action>)> {
            commands(&PaletteView {
                active: active.map(WorkspaceId::new),
                ..view(&config, &workspaces)
            })
            .into_iter()
            .filter(|command| command.title.contains("SSH"))
            .map(|command| (command.title, command.actions))
            .collect()
        };
        let none = titles(None);
        assert!(matches!(
            &none[..],
            [(title, actions)] if title == "New SSH workspace"
                && matches!(actions[..], [Action::Ssh(None)])
        ));
        let local = titles(Some(1));
        assert!(matches!(
            &local[..],
            [_, (title, actions)] if title == "Connect workspace over SSH"
                && matches!(actions[..], [Action::Ssh(Some(id))] if id == WorkspaceId::new(1))
        ));
        let remote = titles(Some(2));
        assert!(matches!(
            &remote[..],
            [_, (title, actions)] if title == "Disconnect workspace from SSH"
                && matches!(actions[..], [Action::Disconnect(id)] if id == WorkspaceId::new(2))
        ));
    }

    #[test]
    fn a_focused_terminal_can_be_sent_to_each_other_workspace() {
        let config = Config::default();
        let workspaces: Vec<WorkspaceView> =
            [(1, None), (2, None), (3, None), (4, Some("me@devbox"))]
                .into_iter()
                .map(|(id, remote): (u64, Option<&str>)| WorkspaceView {
                    unread: 0,
                    alert: None,
                    group: None,
                    id: WorkspaceId::new(id),
                    name: "app".into(),
                    cwd: "/srv/app".into(),
                    remote: remote.map(str::to_owned),
                    branch: None,
                    panes: 1,
                    running: true,
                })
                .collect();
        let moves = |pane| -> Vec<(u64, Vec<Action>)> {
            commands(&PaletteView {
                pane,
                active: Some(WorkspaceId::new(2)),
                ..view(&config, &workspaces)
            })
            .into_iter()
            .filter(|command| command.group == "Move to")
            .map(|command| (command.target, command.actions))
            .collect()
        };
        assert!(moves(None).is_empty());
        let list = moves(Some(PaneId::new(7)));
        // Same-named destinations stay distinct. The terminal's own workspace
        // is not offered, nor is one on another machine.
        assert_eq!(
            list.iter().map(|(target, _)| *target).collect::<Vec<_>>(),
            [1, 3]
        );
        assert!(matches!(
            list[1].1[..],
            [Action::MovePane(pane, Destination::Workspace(workspace))]
                if pane == PaneId::new(7) && workspace == WorkspaceId::new(3)
        ));
    }

    #[test]
    fn terminal_commands_require_a_focused_pane() {
        let config = Config::default();
        let without = commands(&view(&config, &[]));
        assert!(without.iter().all(|command| command.group != "Terminal"));
        assert!(
            without
                .iter()
                .any(|command| command.title == "New workspace")
        );
        let with = commands(&PaletteView {
            pane: Some(PaneId::new(7)),
            ..view(&config, &[])
        });
        let split = with
            .iter()
            .find(|command| command.title == "Split right")
            .expect("split command");
        assert!(matches!(
            split.actions[..],
            [Action::Split(pane, Axis::Vertical)] if pane == PaneId::new(7)
        ));
    }

    #[test]
    fn pane_navigation_commands_capture_available_neighbours() {
        let config = Config::default();
        let layout = neptune_model::Layout::Split {
            id: neptune_model::SplitId::new(1),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(neptune_model::Layout::Tabs {
                panes: vec![PaneId::new(1), PaneId::new(3)],
                shown: PaneId::new(1),
            }),
            second: Box::new(neptune_model::Layout::pane(PaneId::new(2))),
        };
        let list = commands(&PaletteView {
            pane: Some(PaneId::new(1)),
            layout: Some(&layout),
            zoomed: true,
            ..view(&config, &[])
        });
        let navigation: Vec<_> = list
            .iter()
            .filter(|command| command.title.starts_with("Focus pane"))
            .collect();
        assert_eq!(navigation.len(), 1);
        assert_eq!(navigation[0].title, "Focus pane to the right");
        assert_eq!(navigation[0].shortcut, "Ctrl+Shift+→");
        assert!(
            matches!(navigation[0].actions[..], [Action::Focus(target)] if target == PaneId::new(2))
        );
        // Two tabs share the focused place, so either direction reaches the other.
        for title in ["Next tab", "Previous tab"] {
            let command = list.iter().find(|command| command.title == title).unwrap();
            assert!(
                matches!(command.actions[..], [Action::Focus(target)] if target == PaneId::new(3))
            );
        }
    }

    #[test]
    fn a_local_terminal_offers_an_agent_in_a_worktree_and_removal_of_its_own() {
        let config = Config::default();
        let pane = PaneId::new(7);
        let titles = |view: PaletteView| -> Vec<String> {
            commands(&view)
                .into_iter()
                .filter(|command| command.title.contains("worktree"))
                .map(|command| command.title)
                .collect()
        };
        let local = PaletteView {
            pane: Some(pane),
            ..view(&config, &[])
        };
        let list = commands(&local);
        assert_eq!(list[1].title, "New agent in worktree");
        assert!(matches!(
            list[1].actions[..],
            [Action::Worktree(crate::ui::worktrees::Event::New(target))] if target == pane
        ));
        assert_eq!(
            titles(PaletteView {
                worktree: Some("fix-login"),
                ..local
            }),
            ["New agent in worktree", "Remove worktree fix-login"]
        );
        // git runs on this machine: an SSH terminal is not offered one, and
        // neither is a window without a terminal.
        assert!(
            titles(PaletteView {
                pane: Some(pane),
                local: false,
                ..view(&config, &[])
            })
            .is_empty()
        );
        assert!(titles(view(&config, &[])).is_empty());
    }

    #[test]
    fn query_words_match_in_any_order_and_by_group() {
        let config = Config::default();
        let all = commands(&PaletteView {
            pane: Some(PaneId::new(1)),
            ..view(&config, &[])
        });
        let titles = |query: &str| -> Vec<&str> {
            all.iter()
                .filter(|command| matches(command, query))
                .map(|command| command.title.as_str())
                .collect()
        };
        assert_eq!(titles("right split"), ["Split right"]);
        assert_eq!(titles("zoom reset"), ["Reset app zoom"]);
        assert!(titles("font").is_empty());
        assert_eq!(
            titles("appearance"),
            ["Browse themes", "Use Dusk theme", "Use Light theme"]
        );
        assert!(titles("zzz").is_empty());
        // The active theme is not offered again.
        assert!(titles("graphite").is_empty());
        assert_eq!(titles("").len(), all.len());
    }
}
