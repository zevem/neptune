//! Native update notification and explicit verified-download/install sheet.
use super::{
    Action,
    helpers::{
        ButtonKind, Group, SheetPlacement, button, focus_ring, padded, place, sheet, sheet_footer,
        sheet_header,
    },
};
use crate::{
    icons::{self, Icon},
    runtime::updates::{NextStep, UpdateStatus, Updates},
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontId, Frame, Id, Layout, Margin, Pos2, Rect,
    Response, Sense, Stroke, TextFormat, Ui, Vec2, WidgetInfo, WidgetType, text::LayoutJob, vec2,
};

fn status(updates: &Updates) -> String {
    match &updates.status {
        UpdateStatus::Idle => format!("Neptune {}", env!("CARGO_PKG_VERSION")),
        UpdateStatus::Checking => "Checking for updates…".into(),
        UpdateStatus::Current => "You're up to date.".into(),
        UpdateStatus::Available => updates
            .release
            .as_ref()
            .map(|release| format!("Neptune {} is available.", release.version))
            .unwrap_or_default(),
        UpdateStatus::Downloading => "Downloading and verifying…".into(),
        UpdateStatus::Ready if updates.next_step() == NextStep::Install => {
            "Download verified. Restart Neptune to update.".into()
        }
        UpdateStatus::Ready => "Download verified. Ready to open.".into(),
        UpdateStatus::Installing => "Installing the update…".into(),
        UpdateStatus::Installed => updates
            .release
            .as_ref()
            .map(|release| {
                format!(
                    "Neptune {} is installed. Restart to start using it.",
                    release.version
                )
            })
            .unwrap_or_default(),
        UpdateStatus::Opening => "Opening your download…".into(),
        UpdateStatus::Opened => {
            "Download opened. Finish installation, then relaunch Neptune.".into()
        }
        UpdateStatus::Error(message) => message.clone(),
    }
}

/// The installed version with a manual check, the release on offer and the
/// state of the last check, as rows of one Preferences card.
pub fn preferences_status(
    ui: &mut egui::Ui,
    p: Palette,
    updates: &Updates,
    rows: &mut Group,
    actions: &mut Vec<Action>,
) {
    let installed = updates.status == UpdateStatus::Installed;
    rows.row(
        ui,
        &format!("Neptune {}", env!("CARGO_PKG_VERSION")),
        |ui| {
            ui.add_enabled_ui(!updates.busy() && !installed, |ui| {
                if button(ui, p, "Check for updates", ButtonKind::Secondary).clicked() {
                    actions.push(Action::CheckUpdates);
                }
            });
        },
    );
    if let Some(release) = &updates.release {
        if installed {
            let label = format!("Neptune {} is installed", release.version);
            rows.row(ui, &label, |ui| {
                if button(ui, p, "Restart to update", ButtonKind::Secondary).clicked() {
                    actions.push(Action::RestartUpdate);
                }
            });
        } else {
            let label = format!("Neptune {} is available", release.version);
            rows.row(ui, &label, |ui| {
                if button(ui, p, "Review update", ButtonKind::Secondary).clicked() {
                    actions.push(Action::ReviewUpdate);
                }
            });
        }
    }
    // An idle updater has nothing to add to the installed version, nor an
    // offered or installed release to the row that names it.
    if !matches!(
        updates.status,
        UpdateStatus::Idle | UpdateStatus::Available | UpdateStatus::Installed
    ) {
        rows.note(ui, &status(updates));
    }
}

pub fn notification(ctx: &egui::Context, p: Palette, updates: &Updates, actions: &mut Vec<Action>) {
    let Some(release) = updates.notification() else {
        return;
    };
    egui::Area::new(Id::new("neptune-update-notification"))
        .order(egui::Order::Foreground)
        .anchor(
            Align2::RIGHT_TOP,
            vec2(-14.0, metrics::TOOLBAR_HEIGHT + 8.0),
        )
        .show(ctx, |ui| {
            Frame::new()
                .fill(p.elevated)
                .corner_radius(12)
                .stroke(Stroke::new(1.0, p.border))
                .shadow(p.popup_shadow())
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_max_width((ctx.content_rect().width() - 52.0).clamp(160.0, 340.0));
                    ui.label(
                        egui::RichText::new(format!("Neptune {} is available", release.version))
                            .color(p.fg)
                            .size(13.0),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if button(ui, p, "What's New", ButtonKind::Secondary).clicked() {
                            actions.push(Action::ReviewUpdate);
                        }
                        if button(ui, p, "Later", ButtonKind::Quiet).clicked() {
                            actions.push(Action::DismissUpdate);
                        }
                    });
                });
        });
}

/// A block of the release notes. The changelog is hard-wrapped Markdown, so
/// continuation lines are joined to the block they belong to and the sheet
/// decides where text wraps.
#[derive(Debug, PartialEq, Eq)]
enum Block {
    Heading(String),
    Bullet(String),
    Text(String),
}

fn blocks(notes: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    // Whether the last block is still open to continuation lines.
    let mut open = false;
    for line in notes.lines().map(str::trim) {
        if line.is_empty() {
            open = false;
        } else if line.starts_with('#') {
            blocks.push(Block::Heading(
                line.trim_start_matches('#').trim().to_owned(),
            ));
            open = false;
        } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            blocks.push(Block::Bullet(item.trim().to_owned()));
            open = true;
        } else if open && let Some(Block::Bullet(text) | Block::Text(text)) = blocks.last_mut() {
            text.push(' ');
            text.push_str(line);
        } else {
            blocks.push(Block::Text(line.to_owned()));
            open = true;
        }
    }
    blocks
}

const NOTE_LINE: f32 = 19.0;

/// Body text with `code` spans set in the terminal face. Plain text styling
/// keeps release notes independent of external images or executable rich
/// content.
fn body(text: &str, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    // An unpaired backtick is ordinary text.
    let paired = text.matches('`').count().is_multiple_of(2);
    for (index, span) in text.split('`').enumerate() {
        let code = paired && index % 2 == 1;
        if !paired && index > 0 {
            job.append("`", 0.0, format(theme::regular(13.0), color));
        }
        let font = if code {
            FontId::monospace(12.0)
        } else {
            theme::regular(13.0)
        };
        job.append(span, 0.0, format(font, color));
    }
    job
}

fn format(font_id: FontId, color: Color32) -> TextFormat {
    TextFormat {
        font_id,
        color,
        line_height: Some(NOTE_LINE),
        valign: Align::Center,
        ..Default::default()
    }
}

fn release_notes(ui: &mut Ui, p: Palette, notes: &str) {
    for (index, block) in blocks(notes).iter().enumerate() {
        match block {
            Block::Heading(text) => {
                ui.add_space(if index == 0 { 0.0 } else { 14.0 });
                ui.label(
                    egui::RichText::new(text)
                        .font(theme::medium(11.5))
                        .color(p.secondary),
                );
                ui.add_space(2.0);
            }
            Block::Bullet(text) => {
                ui.add_space(6.0);
                ui.horizontal_top(|ui| {
                    let (_, marker) = ui.allocate_space(vec2(16.0, NOTE_LINE));
                    ui.painter().circle_filled(
                        Pos2::new(marker.left() + 5.0, marker.center().y),
                        1.75,
                        p.muted,
                    );
                    ui.add(egui::Label::new(body(text, p.fg)).wrap());
                });
            }
            Block::Text(text) => {
                ui.add_space(6.0);
                ui.add(egui::Label::new(body(text, p.fg)).wrap());
            }
        }
    }
}

/// A text link that leaves the application.
fn web_link(ui: &mut Ui, p: Palette, label: &str) -> Response {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), theme::medium(12.5), Color32::PLACEHOLDER);
    let (_, rect) = ui.allocate_space(vec2(galley.size().x + 16.0, 20.0));
    let response = ui.interact(rect, ui.id().with(("link", label)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Link, ui.is_enabled(), label));
    if ui.is_rect_visible(rect) {
        let ink = if response.hovered() || response.has_focus() {
            p.fg
        } else {
            p.secondary
        };
        if response.has_focus() {
            focus_ring(ui.painter(), rect.expand2(vec2(3.0, 0.0)), 5, p);
        }
        let width = galley.size().x;
        ui.painter().galley(
            Pos2::new(rect.left(), rect.center().y - galley.size().y * 0.5),
            galley,
            ink,
        );
        icons::paint(
            ui.painter(),
            Rect::from_center_size(
                Pos2::new(rect.left() + width + 9.0, rect.center().y),
                Vec2::splat(12.0),
            ),
            Icon::ArrowUpRight,
            ink,
        );
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

const STATUS_ICON: f32 = 16.0;
const STATUS_GAP: f32 = 10.0;
const STATUS_MARGIN: Margin = Margin::symmetric(12, 10);
/// Clear space above and below the status card.
const STATUS_SPACE: f32 = 12.0;
const GUTTER: f32 = 20.0;

/// Whether the sheet has progress or an outcome to report. An update that is
/// only on offer is already named by the sheet itself.
fn reports_status(updates: &Updates) -> bool {
    updates.release.is_none() || !matches!(updates.status, UpdateStatus::Available)
}

fn status_text(text: &str, color: Color32) -> LayoutJob {
    LayoutJob::simple(text.to_owned(), theme::regular(12.5), color, f32::INFINITY)
}

/// The height the status card takes in a sheet of `width`, including the
/// space around it.
fn status_height(ui: &Ui, p: Palette, updates: &Updates, width: f32) -> f32 {
    if !reports_status(updates) {
        return 0.0;
    }
    let mut job = status_text(&status(updates), p.fg);
    job.wrap.max_width = width
        - GUTTER * 2.0
        - f32::from(STATUS_MARGIN.left + STATUS_MARGIN.right)
        - STATUS_ICON
        - STATUS_GAP;
    let text = ui.painter().layout_job(job).size().y;
    text.max(STATUS_ICON) + f32::from(STATUS_MARGIN.top + STATUS_MARGIN.bottom) + STATUS_SPACE * 2.0
}

/// Progress, the verified download or a failure, kept in view while the notes
/// scroll.
fn status_card(ui: &mut Ui, p: Palette, updates: &Updates) {
    ui.add_space(STATUS_SPACE);
    padded(ui, GUTTER, |ui| {
        Frame::new()
            .fill(p.control)
            .corner_radius(10)
            .inner_margin(STATUS_MARGIN)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().interact_size.y = STATUS_ICON;
                ui.horizontal_top(|ui| {
                    let (_, icon) = ui.allocate_space(Vec2::splat(STATUS_ICON));
                    match &updates.status {
                        UpdateStatus::Error(_) => {
                            icons::paint(ui.painter(), icon, Icon::Warning, p.red);
                        }
                        UpdateStatus::Ready
                        | UpdateStatus::Opened
                        | UpdateStatus::Installed
                        | UpdateStatus::Current => {
                            icons::paint(ui.painter(), icon, Icon::Check, p.green);
                        }
                        _ if updates.busy() => {
                            egui::Spinner::new()
                                .size(14.0)
                                .color(p.secondary)
                                .paint_at(ui, icon.shrink(1.0));
                        }
                        _ => icons::paint(ui.painter(), icon, Icon::Refresh, p.secondary),
                    }
                    ui.add_space(STATUS_GAP);
                    ui.add(
                        egui::Label::new(status_text(&status(updates), p.fg))
                            .wrap()
                            .selectable(false),
                    );
                });
            });
    });
    ui.add_space(STATUS_SPACE);
}

fn install_guidance(updates: &Updates) -> &'static str {
    if matches!(updates.next_step(), NextStep::Install | NextStep::Restart) {
        "Neptune replaces itself with the verified download and restarts. Workspaces and directories come back with fresh shells; running commands do not, so finish them first."
    } else if cfg!(target_os = "macos") {
        "Open the verified disk image, quit Neptune when you're ready, then drag Neptune into Applications."
    } else if cfg!(windows) {
        "Open the verified installer and follow its steps. Save your shell work before closing Neptune. Windows signing is not available yet."
    } else {
        "Show the verified download in your file manager. For AppImage, make it executable and replace your old file after quitting. For DEB, install with your package manager."
    }
}

pub fn show(ctx: &egui::Context, p: Palette, updates: &Updates, actions: &mut Vec<Action>) {
    let mut close = false;
    let output = sheet(
        ctx,
        p,
        "Software update",
        520.0,
        SheetPlacement::Center,
        |ui| {
            close |= sheet_header(ui, p, "Software update", Some("Close update"));
            let status = status_height(ui, p, updates, ui.available_width());
            // The header, the status and the action bar stay in view; the
            // release itself scrolls between them.
            let fixed = 52.0 + status + 60.0 + 24.0;
            egui::ScrollArea::vertical()
                .id_salt("update-body")
                .max_height((ctx.content_rect().height() - fixed).clamp(72.0, 440.0))
                .show(ui, |ui| {
                    padded(ui, GUTTER, |ui| {
                        let Some(release) = &updates.release else {
                            return;
                        };
                        ui.spacing_mut().interact_size.y = NOTE_LINE;
                        ui.label(
                            egui::RichText::new(format!("Neptune {}", release.version))
                                .font(theme::semibold(20.0))
                                .color(p.fg),
                        );
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "You have {}",
                                        env!("CARGO_PKG_VERSION")
                                    ))
                                    .size(12.5)
                                    .color(p.secondary),
                                )
                                .selectable(false),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new("·").size(12.5).color(p.muted),
                                )
                                .selectable(false),
                            );
                            if web_link(ui, p, "View on GitHub").clicked()
                                && let Some(link) =
                                    crate::platform::links::WebLink::new(&release.release_url())
                            {
                                actions.push(Action::OpenLink(link));
                            }
                        });
                        ui.add_space(16.0);
                        let (_, rule) = ui.allocate_space(vec2(ui.available_width(), 1.0));
                        ui.painter()
                            .line_segment([rule.left_center(), rule.right_center()], p.hairline());
                        ui.add_space(16.0);
                        release_notes(ui, p, &release.notes);
                        ui.add_space(14.0);
                        ui.label(
                            egui::RichText::new("Installing")
                                .font(theme::medium(11.5))
                                .color(p.secondary),
                        );
                        ui.add_space(8.0);
                        ui.add(
                            egui::Label::new(body(install_guidance(updates), p.secondary)).wrap(),
                        );
                        ui.add_space(18.0);
                    });
                });
            if status > 0.0 {
                status_card(ui, p, updates);
            }
            let bar = sheet_footer(ui, p);
            place(
                ui,
                bar,
                Layout::right_to_left(Align::Center),
                "update-actions",
                |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if let Some(release) = &updates.release {
                        let version = release.version.clone();
                        let (label, action) = match updates.next_step() {
                            NextStep::Download => {
                                ("Download update", Action::DownloadUpdate(version))
                            }
                            NextStep::Install => {
                                ("Restart to update", Action::InstallUpdate(version))
                            }
                            NextStep::Open if cfg!(target_os = "linux") => {
                                ("Show download", Action::OpenUpdate(version))
                            }
                            NextStep::Open => ("Open installer", Action::OpenUpdate(version)),
                            NextStep::Restart => ("Restart to update", Action::RestartUpdate),
                        };
                        ui.add_enabled_ui(!updates.busy(), |ui| {
                            if button(ui, p, label, ButtonKind::Primary).clicked() {
                                actions.push(action);
                            }
                        });
                    }
                    if updates.busy()
                        && updates.status != UpdateStatus::Installing
                        && button(ui, p, "Cancel", ButtonKind::Secondary).clicked()
                    {
                        actions.push(Action::CancelUpdate);
                    }
                    close |= button(ui, p, "Later", ButtonKind::Secondary).clicked();
                },
            );
        },
    );
    if close || output.backdrop_clicked {
        actions.push(Action::DismissUpdate);
        actions.push(Action::CloseOverlay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_wrapped_notes_become_whole_blocks() {
        let notes = "### What's New\n\n- Open a tab with\n  Ctrl+Shift+T, and drag tabs\n  between splits.\n- Shorter rows.\n\nA closing line\nin two parts.\n\nSeparate.";
        assert_eq!(
            blocks(notes),
            [
                Block::Heading("What's New".into()),
                Block::Bullet("Open a tab with Ctrl+Shift+T, and drag tabs between splits.".into()),
                Block::Bullet("Shorter rows.".into()),
                Block::Text("A closing line in two parts.".into()),
                Block::Text("Separate.".into()),
            ]
        );
    }

    #[test]
    fn code_spans_keep_every_character_of_unpaired_notes() {
        let text = |job: LayoutJob| job.text;
        assert_eq!(
            text(body("run `chmod u+x` first", Color32::WHITE)),
            "run chmod u+x first"
        );
        assert_eq!(text(body("a ` alone", Color32::WHITE)), "a ` alone");
    }
}
