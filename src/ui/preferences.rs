//! Preferences: a source list of panes whose settings apply as they change.
use super::Action;
use super::helpers::{
    ButtonKind, Group, SheetPlacement, button, elided, focus_ring, galley_at, group, menu_item,
    menu_layout, menu_separator, padded, place, section_label, segmented, select, sheet,
    sheet_header, slider, stepper, text_field, toggle,
};
use super::theme_browser::{self, search_field};
use crate::{
    config::{Config, Cursor},
    icons::{self, Icon},
    platform::fonts::{DEFAULT_FAMILY, Fonts},
    platform::shells::{Detection, Shell},
    runtime::updates::Updates,
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Align2, CursorIcon, Id, Layout, Pos2, Rect, Sense, StrokeKind, Ui, Vec2,
    WidgetInfo, WidgetType, vec2,
};
use std::cell::Cell;

/// The settings screen is this tall when the window has room for it.
const HEIGHT: f32 = 520.0;
const HEADER: f32 = 52.0;
/// The source list sits inside the sheet as a surface of its own.
const INSET: f32 = 6.0;
const SIDEBAR: f32 = 176.0;
/// A sheet too narrow for names keeps the source list as icons.
const RAIL: f32 = 44.0;
const COMPACT_BELOW: f32 = 540.0;

/// A screen of related settings, chosen from the sheet's source list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pane {
    #[default]
    General,
    Appearance,
    Text,
    Shell,
    Notifications,
    Updates,
}

impl Pane {
    pub const ALL: [Self; 6] = [
        Self::General,
        Self::Appearance,
        Self::Text,
        Self::Shell,
        Self::Notifications,
        Self::Updates,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Text => "Text",
            Self::Shell => "Shell",
            Self::Notifications => "Notifications",
            Self::Updates => "Updates",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::General => Icon::Settings,
            Self::Appearance => Icon::Sun,
            Self::Text => Icon::TextSize,
            Self::Shell => Icon::Terminal,
            Self::Notifications => Icon::Bell,
            Self::Updates => Icon::Refresh,
        }
    }
}

/// The pane in view, kept while the app runs, and the settings search.
#[derive(Default)]
pub struct View {
    pub pane: Pane,
    query: String,
    font: FontPicker,
    editors: crate::platform::editor::Detection,
    keybinding_reference: Option<String>,
    /// The search field takes the keyboard when the sheet appears.
    focus_search: bool,
}

impl View {
    /// Preferences opens on the pane last in view, ready to search.
    pub fn opened(&mut self) {
        self.query.clear();
        self.focus_search = true;
        self.font.cancel();
        self.editors.refresh();
        self.keybinding_reference = None;
    }

    /// The sheet with a search under way.
    #[cfg(test)]
    pub fn searching(query: &str) -> Self {
        Self {
            query: query.into(),
            ..Self::default()
        }
    }

    /// Escape leaves a search before it leaves the sheet.
    pub fn back(&mut self) -> bool {
        let searching = !self.query.is_empty();
        self.query.clear();
        searching
    }
}

/// Typing is a draft until Enter, so partial names never reload terminal fonts.
#[derive(Default)]
struct FontPicker {
    custom: bool,
    saved: String,
    draft: String,
    error: Option<&'static str>,
}

impl FontPicker {
    fn sync(&mut self, family: &str) {
        if self.saved != family {
            self.saved = family.to_owned();
            self.custom = false;
            self.cancel();
        }
    }

    fn cancel(&mut self) {
        self.draft.clone_from(&self.saved);
        self.error = None;
    }

    fn commit(&mut self, config: &mut Config) {
        if !crate::config::valid_font_family(&self.draft) {
            self.error = Some(
                "Enter a font family name of at most 128 characters, without control characters.",
            );
            return;
        }
        config.font_family = self.draft.trim().to_owned();
        self.saved.clone_from(&config.font_family);
        self.cancel();
    }
}

/// One thing a search can find: a row, or the rows that belong together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    FileEditor,
    RestoreWorkspaces,
    ConfirmClose,
    WarnProcesses,
    Keybindings,
    Reset,
    Theme,
    WindowZoom,
    FontFamily,
    FontSize,
    LineSpacing,
    CursorStyle,
    CursorBlink,
    ShellProgram,
    Scrollback,
    DesktopBanners,
    CheckUpdates,
    ReleaseChannel,
    Version,
}

/// Each setting with its pane, its label and the other words people use for
/// it. A search reads the pane's name and the label as well as these.
const INDEX: [(Setting, Pane, &str, &str); 19] = [
    (
        Setting::FileEditor,
        Pane::General,
        "Open file locations in",
        "editor files paths click line vscode visual studio code cursor android studio explorer",
    ),
    (
        Setting::RestoreWorkspaces,
        Pane::General,
        "Restore workspaces on launch",
        "startup start open reopen resume session sessions layout layouts folders tabs windows remember previous last",
    ),
    (
        Setting::ConfirmClose,
        Pane::General,
        "Confirm before closing terminals",
        "close ask prompt dialog question exit quit pane tab workspace",
    ),
    (
        Setting::WarnProcesses,
        Pane::General,
        "Warn about running processes",
        "close closing quit exit kill job jobs command program busy process warning alert",
    ),
    (
        Setting::Keybindings,
        Pane::General,
        "Keyboard shortcuts",
        "keybindings keybind bindings hotkeys remap customize config configuration file toml keyboard keys actions names syntax reference",
    ),
    (
        Setting::Reset,
        Pane::General,
        "Reset to defaults",
        "all settings restore factory original revert clear undo default",
    ),
    (
        Setting::Theme,
        Pane::Appearance,
        "Theme",
        "themes color colors colour colours scheme palette dark light mode night day skin background foreground browse custom favorites iterm graphite dusk",
    ),
    (
        Setting::WindowZoom,
        Pane::Appearance,
        "Window zoom",
        "scale scaling size bigger smaller larger magnify interface ui dpi hidpi percent display",
    ),
    (
        Setting::FontFamily,
        Pane::Text,
        "Font family",
        "fonts typeface typography monospace monospaced installed custom name jetbrains nerd ligatures text letters characters",
    ),
    (
        Setting::FontSize,
        Pane::Text,
        "Font size",
        "fonts typeface type text letters characters bigger smaller larger points pt zoom",
    ),
    (
        Setting::LineSpacing,
        Pane::Text,
        "Line spacing",
        "font height leading rows gap density compact padding space",
    ),
    (
        Setting::CursorStyle,
        Pane::Text,
        "Cursor style",
        "caret shape block beam bar ibeam underline underscore pointer",
    ),
    (
        Setting::CursorBlink,
        Pane::Text,
        "Cursor blink",
        "caret blinking flash flashing animate animation steady solid",
    ),
    (
        Setting::ShellProgram,
        Pane::Shell,
        "Shell program",
        "startup default custom path command executable binary profile login zsh bash fish sh nu nushell powershell pwsh cmd wsl git",
    ),
    (
        Setting::Scrollback,
        Pane::Shell,
        "Scrollback",
        "history lines buffer scroll scrolling back output memory limit",
    ),
    (
        Setting::DesktopBanners,
        Pane::Notifications,
        "Desktop banners",
        "notification notify alert alerts bell banner toast popup system attention sound unread",
    ),
    (
        Setting::CheckUpdates,
        Pane::Updates,
        "Check automatically",
        "update updates automatic auto upgrade upgrades background check",
    ),
    (
        Setting::ReleaseChannel,
        Pane::Updates,
        "Release channel",
        "update stable beta preview prerelease candidate nightly early",
    ),
    (
        Setting::Version,
        Pane::Updates,
        "Check for updates",
        "update version installed about release review download install upgrade new latest neptune",
    ),
];

/// How many slips separate two short words: a letter added, dropped or
/// replaced, or two neighbours swapped.
fn edits(a: &[char], b: &[char]) -> usize {
    let width = b.len() + 1;
    let mut cost: Vec<usize> = (0..(a.len() + 1) * width).map(|i| i % width).collect();
    for i in 1..=a.len() {
        cost[i * width] = i;
        for j in 1..=b.len() {
            let mut best = (cost[(i - 1) * width + j - 1] + usize::from(a[i - 1] != b[j - 1]))
                .min(cost[(i - 1) * width + j] + 1)
                .min(cost[i * width + j - 1] + 1);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(cost[(i - 2) * width + j - 2] + 1);
            }
            cost[i * width + j] = best;
        }
    }
    cost[a.len() * width + b.len()]
}

/// Whether `typed` is `word`, or the start of it, give or take a slip of the
/// keys. Short words must be typed exactly: a slip there is another word.
fn nearly(typed: &str, word: &str) -> bool {
    let typed: Vec<char> = typed.chars().collect();
    let word: Vec<char> = word.chars().collect();
    let allowed = match typed.len() {
        0..=3 => return false,
        4..=7 => 1,
        _ => 2,
    };
    (typed.len().saturating_sub(allowed)..=typed.len() + allowed)
        .filter(|&length| length <= word.len())
        .any(|length| edits(&typed, &word[..length]) <= allowed)
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

/// The settings a query finds, in the order of the panes, or `None` when
/// there is nothing to search for. Every word typed must be found in a
/// setting: at the start of one of its words, or inside one once three
/// letters are typed. Misspellings count only when nothing matches as typed.
fn search(query: &str) -> Option<Vec<Setting>> {
    let typed: Vec<String> = words(query).collect();
    if typed.is_empty() {
        return None;
    }
    let mut exact = Vec::new();
    let mut near = Vec::new();
    for (setting, pane, label, terms) in INDEX {
        let known: Vec<String> = words(pane.title())
            .chain(words(label))
            .chain(words(terms))
            .collect();
        let found = |typed: &String| {
            known.iter().any(|word| {
                word.starts_with(typed.as_str())
                    || (typed.chars().count() >= 3 && word.contains(typed.as_str()))
            })
        };
        if typed.iter().all(found) {
            exact.push(setting);
        } else if typed
            .iter()
            .all(|typed| found(typed) || known.iter().any(|word| nearly(typed, word)))
        {
            near.push(setting);
        }
    }
    Some(if exact.is_empty() { near } else { exact })
}

fn pane_of(setting: Setting) -> Pane {
    INDEX
        .iter()
        .find(|entry| entry.0 == setting)
        .map_or(Pane::General, |entry| entry.1)
}

/// Which settings a pane draws: all of its own, or those a search found.
struct Shown<'a> {
    found: Option<&'a [Setting]>,
    cards: Cell<usize>,
}

impl Shown<'_> {
    fn has(&self, setting: Setting) -> bool {
        self.found.is_none_or(|found| found.contains(&setting))
    }
}

/// A titled card holding those of `members` that are shown, or nothing when
/// none are. Search results name the pane each card comes from.
fn card(
    ui: &mut Ui,
    p: Palette,
    shown: &Shown,
    pane: Pane,
    title: &str,
    members: &[Setting],
    add_rows: impl FnOnce(&mut Ui, &mut Group),
) {
    if !members.iter().any(|&setting| shown.has(setting)) {
        return;
    }
    if shown.cards.replace(shown.cards.get() + 1) > 0 {
        ui.add_space(14.0);
    }
    if shown.found.is_some() {
        section_label(ui, p, &format!("{} · {title}", pane.title()));
    } else {
        section_label(ui, p, title);
    }
    group(ui, p, add_rows);
}

// Keep runtime services and each overlay's view state borrowed independently.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ctx: &egui::Context,
    current: &Config,
    updates: &Updates,
    fonts: &Fonts,
    state: &mut theme_browser::State,
    view: &mut View,
    shells: &mut ShellPicker,
    actions: &mut Vec<Action>,
) {
    let mut config = current.clone();
    let p = Palette::for_config(&config);
    let screen = ctx.content_rect();
    let mut close = false;
    let output = sheet(
        ctx,
        p,
        "Preferences",
        theme_browser::WIDTH,
        SheetPlacement::Center,
        |ui| {
            // Leave the window visible around the sheet when there is room; a
            // short window gives most of that margin back to the settings.
            let chrome = HEADER + 60.0;
            let body_height = (screen.height() - chrome - 96.0)
                .max((screen.height() - chrome - 24.0).min(260.0))
                .clamp(120.0, 560.0);
            if state.open {
                close |= theme_browser::show(ui, p, &mut config, state, body_height);
                return;
            }
            // One height for every pane, so choosing one never moves the sheet.
            let height = (body_height + chrome).min(HEIGHT);
            let (_, rect) = ui.allocate_space(vec2(ui.available_width(), height));
            let compact = rect.width() < COMPACT_BELOW;
            let list = Rect::from_min_size(
                rect.min + Vec2::splat(INSET),
                vec2(if compact { RAIL } else { SIDEBAR }, height - INSET * 2.0),
            );
            let content = Rect::from_min_max(Pos2::new(list.right(), rect.top()), rect.max);
            // Concentric with the sheet's corner.
            let radius = metrics::SHEET_RADIUS - INSET as u8;
            ui.painter().rect_filled(list, radius, p.chrome);
            ui.painter()
                .rect_stroke(list, radius, p.hairline(), StrokeKind::Inside);

            // Beside the names while there is room; otherwise the search
            // titles the content, and the rail says which pane is in view.
            let (field, panes) = if compact {
                (
                    Rect::from_min_max(
                        Pos2::new(content.left() + 20.0, content.top() + 11.0),
                        Pos2::new(content.right() - 50.0, content.top() + 41.0),
                    ),
                    list.with_min_y(list.top() + INSET),
                )
            } else {
                let top = list.top() + HEADER - INSET;
                ui.painter().text(
                    Pos2::new(list.left() + 14.0, (list.top() + top) * 0.5),
                    Align2::LEFT_CENTER,
                    "Preferences",
                    theme::semibold(15.0),
                    p.fg,
                );
                (
                    Rect::from_min_size(
                        Pos2::new(list.left() + 8.0, top),
                        vec2(list.width() - 16.0, metrics::CONTROL_HEIGHT),
                    ),
                    list.with_min_y(top + metrics::CONTROL_HEIGHT + 8.0),
                )
            };
            let searched = place(
                ui,
                field,
                Layout::left_to_right(Align::Center),
                "preferences-search",
                |ui| {
                    search_field(
                        ui,
                        p,
                        Id::new("preferences-search"),
                        &mut view.query,
                        "Search",
                        "Search settings",
                        field.width(),
                    )
                },
            );
            // A sheet measures itself in a hidden first pass, where focus
            // cannot be held.
            if view.focus_search && ui.is_enabled() && !ui.is_sizing_pass() {
                searched.request_focus();
                view.focus_search = false;
            }
            let mut found = search(&view.query);
            // Enter opens the pane of the first setting found.
            if searched.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                && let Some(&first) = found.as_ref().and_then(|found| found.first())
            {
                view.pane = pane_of(first);
                view.query.clear();
                found = None;
            }

            place(
                ui,
                panes,
                Layout::top_down(Align::Min),
                "preferences-sidebar",
                |ui| sidebar(ui, p, view, found.as_deref(), compact),
            );
            // Choosing a pane leaves the search.
            if view.query.is_empty() {
                found = None;
            }
            place(
                ui,
                content,
                Layout::top_down(Align::Min),
                "preferences-pane",
                |ui| {
                    let title = match (compact, &found) {
                        (true, _) => "",
                        (false, Some(_)) => "Results",
                        (false, None) => view.pane.title(),
                    };
                    close |= sheet_header(ui, p, title, Some("Close preferences"));
                    if found.as_ref().is_some_and(Vec::is_empty) {
                        nothing_found(ui, p, height - HEADER);
                        return;
                    }
                    let shown = Shown {
                        found: found.as_deref(),
                        cards: Cell::new(0),
                    };
                    // A search lists every pane; otherwise the one in view.
                    let listed = |pane: Pane| shown.found.is_some() || pane == view.pane;
                    egui::ScrollArea::vertical()
                        .id_salt((
                            "preferences-body",
                            shown.found.is_none().then_some(view.pane),
                        ))
                        .max_height(height - HEADER)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            padded(ui, 20.0, |ui| {
                                if listed(Pane::General) {
                                    general(
                                        ui,
                                        p,
                                        &shown,
                                        &mut config,
                                        &mut view.keybinding_reference,
                                        &mut view.editors,
                                        actions,
                                    );
                                }
                                if listed(Pane::Appearance) {
                                    appearance(ui, p, &shown, &mut config, state);
                                }
                                if listed(Pane::Text) {
                                    text(ui, p, &shown, &mut config, fonts, &mut view.font);
                                }
                                if listed(Pane::Shell) {
                                    shell(ui, p, &shown, &mut config, shells);
                                }
                                if listed(Pane::Notifications) {
                                    notifications(ui, p, &shown, &mut config);
                                }
                                if listed(Pane::Updates) {
                                    update_settings(ui, p, &shown, &mut config, updates, actions);
                                }
                                ui.add_space(20.0);
                            });
                        });
                },
            );
        },
    );
    if (close || output.backdrop_clicked) && state.may_close() {
        actions.push(Action::CloseOverlay);
    }
    if &config != current {
        actions.push(Action::Preferences(config));
    }
}

/// What the content says when a search finds no setting.
fn nothing_found(ui: &mut Ui, p: Palette, height: f32) {
    let (_, area) = ui.allocate_space(vec2(ui.available_width(), height));
    let center = area.center() - vec2(0.0, HEADER * 0.5);
    let painter = ui.painter();
    icons::paint(
        painter,
        Rect::from_center_size(center - vec2(0.0, 34.0), Vec2::splat(22.0)),
        Icon::Search,
        p.muted,
    );
    painter.text(
        center - vec2(0.0, 4.0),
        Align2::CENTER_CENTER,
        "No settings found",
        theme::medium(13.0),
        p.fg,
    );
    painter.text(
        center + vec2(0.0, 16.0),
        Align2::CENTER_CENTER,
        "Try another word for it.",
        theme::regular(12.0),
        p.muted,
    );
}

/// The source list: the panes by name, or by icon alone in a narrow sheet.
/// During a search each pane counts what was found in it.
fn sidebar(ui: &mut Ui, p: Palette, view: &mut View, found: Option<&[Setting]>, compact: bool) {
    let width = ui.available_width();
    egui::ScrollArea::vertical()
        .id_salt("preferences-panes")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in Pane::ALL {
                let (_, slot) = ui.allocate_space(vec2(width, 32.0));
                let row = slot.shrink2(vec2(INSET, 1.0));
                let response =
                    ui.interact(row, Id::new(("preferences-pane", entry)), Sense::click());
                if response.clicked() {
                    view.pane = entry;
                    view.query.clear();
                }
                let matches = found.map(|found| {
                    found
                        .iter()
                        .filter(|&&setting| pane_of(setting) == entry)
                        .count()
                });
                // Results take the content; no single pane is in view then.
                let selected = matches.is_none() && view.pane == entry;
                response.widget_info(|| {
                    WidgetInfo::selected(WidgetType::RadioButton, true, selected, entry.title())
                });
                let painter = ui.painter();
                if selected || response.is_pointer_button_down_on() {
                    painter.rect_filled(row, metrics::ROW_RADIUS, p.pressed);
                } else if response.hovered() {
                    painter.rect_filled(row, metrics::ROW_RADIUS, p.hover);
                }
                if response.has_focus() {
                    focus_ring(painter, row.shrink(2.0), metrics::ROW_RADIUS - 2, p);
                }
                let ink = if selected || response.hovered() {
                    p.fg
                } else if matches == Some(0) {
                    p.muted
                } else {
                    p.secondary
                };
                let icon = if compact {
                    row.center()
                } else {
                    Pos2::new(row.left() + 16.0, row.center().y)
                };
                icons::paint(
                    painter,
                    Rect::from_center_size(icon, Vec2::splat(16.0)),
                    entry.icon(),
                    ink,
                );
                if !compact {
                    let mut trailing = row.right() - 10.0;
                    if let Some(count) = matches.filter(|&count| count > 0) {
                        let count =
                            painter.layout_no_wrap(count.to_string(), theme::medium(11.5), ink);
                        trailing -= count.size().x;
                        galley_at(painter, Pos2::new(trailing, row.center().y), count);
                        trailing -= 6.0;
                    }
                    let left = row.left() + 32.0;
                    galley_at(
                        painter,
                        Pos2::new(left, row.center().y),
                        elided(
                            painter,
                            entry.title(),
                            theme::medium(13.0),
                            ink,
                            trailing - left,
                        ),
                    );
                }
                let response = response.on_hover_cursor(CursorIcon::PointingHand);
                if compact {
                    response.on_hover_text(entry.title());
                }
            }
        });
}

fn general(
    ui: &mut Ui,
    p: Palette,
    shown: &Shown,
    config: &mut Config,
    reference: &mut Option<String>,
    editors: &mut crate::platform::editor::Detection,
    actions: &mut Vec<Action>,
) {
    use Setting::{ConfirmClose, Keybindings, Reset, RestoreWorkspaces, WarnProcesses};
    let pane = Pane::General;
    card(
        ui,
        p,
        shown,
        pane,
        "Keyboard",
        &[Keybindings],
        |ui, rows| {
            rows.row(ui, "Keyboard shortcuts", |ui| {
                if button(ui, p, "Open config file", ButtonKind::Secondary).clicked() {
                    actions.push(Action::OpenConfig);
                }
            });
            rows.note(
                ui,
                "Edit [keybindings] in your config file to customize shortcuts. Restart Neptune to apply changes.",
            );
            let mut visible = reference.is_some();
            if rows
                .disclosure(ui, "Shortcut reference", &mut visible)
                .changed()
            {
                *reference = visible.then(crate::keybindings::Keybindings::reference);
            }
            if let Some(reference) = reference {
                padded(ui, 14.0, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(reference.as_str())
                                .font(egui::FontId::monospace(11.5))
                                .color(p.secondary),
                        )
                        .wrap(),
                    );
                });
            }
        },
    );
    card(
        ui,
        p,
        shown,
        pane,
        "Startup",
        &[RestoreWorkspaces],
        |ui, rows| {
            rows.row(ui, "Restore workspaces on launch", |ui| {
                toggle(
                    ui,
                    p,
                    &mut config.restore_workspaces,
                    "Restore workspaces on launch",
                );
            });
            rows.note(ui, "Reopens folders and layouts with fresh shells.");
        },
    );
    card(
        ui,
        p,
        shown,
        pane,
        "Closing",
        &[ConfirmClose, WarnProcesses],
        |ui, rows| {
            if shown.has(ConfirmClose) {
                rows.row(ui, "Confirm before closing terminals", |ui| {
                    toggle(
                        ui,
                        p,
                        &mut config.confirm_close,
                        "Confirm before closing terminals",
                    );
                });
            }
            if shown.has(WarnProcesses) {
                rows.row(ui, "Warn about running processes", |ui| {
                    toggle(
                        ui,
                        p,
                        &mut config.warn_running_processes,
                        "Warn about running processes",
                    );
                });
                rows.note(
                    ui,
                    "Process warnings also apply when quitting, even with close confirmation off.",
                );
            }
        },
    );
    card(
        ui,
        p,
        shown,
        pane,
        "File locations",
        &[Setting::FileEditor],
        |ui, rows| {
            let detected = editors.poll(ui.ctx());
            let label = if config.editor.is_empty() {
                "Neptune file explorer"
            } else {
                detected
                    .and_then(|list| list.iter().find(|e| e.command == config.editor))
                    .map(|e| e.name.as_str())
                    .unwrap_or("Custom editor")
            };
            rows.row(ui, "Open file locations in", |ui| {
                let width = (ui.available_width() - 150.0).clamp(120.0, 240.0);
                let response = select(
                    ui,
                    p,
                    Id::new("preferences-file-editor"),
                    label,
                    "Open file locations in",
                    width,
                );
                egui::Popup::menu(&response).show(|ui| {
                    menu_layout(ui, width.max(240.0));
                    if menu_item(ui, p, Icon::Folder, "Neptune file explorer", "", false) {
                        config.editor.clear();
                        ui.close();
                    }
                    if let Some(list) = detected {
                        for editor in list {
                            if menu_item(ui, p, Icon::Terminal, &editor.name, "", false) {
                                config.editor.clone_from(&editor.command);
                                ui.close();
                            }
                        }
                        if list.is_empty() {
                            ui.label("No external editors found.");
                        }
                    } else {
                        ui.label("Finding installed editors…");
                    }
                });
            });
            if !config.editor.is_empty()
                && detected.is_some_and(|list| !list.iter().any(|e| e.command == config.editor))
            {
                rows.note(
                    ui,
                    "Custom or unavailable editor. Check its command in config.toml.",
                );
            }
        },
    );
    card(ui, p, shown, pane, "Reset", &[Reset], |ui, rows| {
        rows.row(ui, "All settings", |ui| {
            if button(ui, p, "Reset to defaults", ButtonKind::Secondary).clicked() {
                *config = Config {
                    custom_themes: config.custom_themes.clone(),
                    favorite_themes: config.favorite_themes.clone(),
                    browser_profile: config.browser_profile.clone(),
                    browser_profiles: config.browser_profiles.clone(),
                    ..Config::default()
                };
            }
        });
        rows.note(
            ui,
            "Custom themes, favorites and browser profiles are kept.",
        );
    });
}

fn appearance(
    ui: &mut Ui,
    p: Palette,
    shown: &Shown,
    config: &mut Config,
    state: &mut theme_browser::State,
) {
    let pane = Pane::Appearance;
    card(
        ui,
        p,
        shown,
        pane,
        "Theme",
        &[Setting::Theme],
        |ui, rows| {
            if theme_browser::summary(ui, p, config, rows) {
                state.browse();
            }
            rows.note(ui, "One theme colors the window and every terminal.");
        },
    );
    card(
        ui,
        p,
        shown,
        pane,
        "Window",
        &[Setting::WindowZoom],
        |ui, rows| {
            rows.row(ui, "Window zoom", |ui| {
                let text = format!("{:.0}%", config.window_zoom * 100.0);
                if stepper(
                    ui,
                    p,
                    "Window zoom",
                    &mut config.window_zoom,
                    Config::WINDOW_ZOOM_RANGE,
                    0.1,
                    &text,
                ) {
                    config.window_zoom = (config.window_zoom * 100.0).round() / 100.0;
                }
            });
            rows.note(
                ui,
                &format!(
                    "Zoom in: {}. Zoom out: {}. Reset: {}. Saved for next launch.",
                    config
                        .keybindings
                        .hint_or_unbound(crate::keybindings::BindingAction::ZoomIn),
                    config
                        .keybindings
                        .hint_or_unbound(crate::keybindings::BindingAction::ZoomOut),
                    config
                        .keybindings
                        .hint_or_unbound(crate::keybindings::BindingAction::ResetZoom)
                ),
            );
        },
    );
}

fn text(
    ui: &mut Ui,
    p: Palette,
    shown: &Shown,
    config: &mut Config,
    fonts: &Fonts,
    picker: &mut FontPicker,
) {
    use Setting::{CursorBlink, CursorStyle, FontFamily, FontSize, LineSpacing};
    let pane = Pane::Text;
    card(
        ui,
        p,
        shown,
        pane,
        "Font",
        &[FontFamily, FontSize, LineSpacing],
        |ui, rows| {
            if shown.has(FontFamily) {
                font_picker(ui, p, rows, config, fonts, picker);
            }
            if shown.has(FontSize) {
                rows.row(ui, "Font size", |ui| {
                    let text = format!("{} pt", config.font_size.round() as i32);
                    stepper(
                        ui,
                        p,
                        "Font size",
                        &mut config.font_size,
                        9.0..=32.0,
                        1.0,
                        &text,
                    );
                });
            }
            if shown.has(LineSpacing) {
                rows.row(ui, "Line spacing", |ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let (_, value) = ui.allocate_space(vec2(34.0, 20.0));
                    ui.painter().text(
                        value.right_center(),
                        Align2::RIGHT_CENTER,
                        format!("{:.2}", config.line_height),
                        theme::medium(12.5),
                        p.secondary,
                    );
                    slider(
                        ui,
                        p,
                        "Line spacing",
                        &mut config.line_height,
                        1.0..=2.0,
                        0.05,
                        150.0,
                    );
                });
            }
            if shown.has(FontSize) {
                rows.note(
                    ui,
                    &format!(
                        "Larger text: {}. Smaller text: {}. Reset: {}.",
                        config
                            .keybindings
                            .hint_or_unbound(crate::keybindings::BindingAction::IncreaseFontSize),
                        config
                            .keybindings
                            .hint_or_unbound(crate::keybindings::BindingAction::DecreaseFontSize),
                        config
                            .keybindings
                            .hint_or_unbound(crate::keybindings::BindingAction::ResetFontSize)
                    ),
                );
            }
        },
    );
    card(
        ui,
        p,
        shown,
        pane,
        "Cursor",
        &[CursorStyle, CursorBlink],
        |ui, rows| {
            if shown.has(CursorStyle) {
                rows.row(ui, "Style", |ui| {
                    segmented(
                        ui,
                        p,
                        "cursor-style",
                        &mut config.cursor,
                        &[
                            (Cursor::Block, "Block"),
                            (Cursor::Beam, "Beam"),
                            (Cursor::Underline, "Underline"),
                        ],
                        228.0,
                    );
                });
            }
            if shown.has(CursorBlink) {
                rows.row(ui, "Blink", |ui| {
                    toggle(ui, p, &mut config.cursor_blink, "Blink cursor");
                });
            }
        },
    );
}

fn font_picker(
    ui: &mut Ui,
    p: Palette,
    rows: &mut Group,
    config: &mut Config,
    fonts: &Fonts,
    picker: &mut FontPicker,
) {
    picker.sync(&config.font_family);
    let width = (ui.available_width() - 128.0).clamp(120.0, 240.0);
    let unlisted = !config.font_family.eq_ignore_ascii_case(DEFAULT_FAMILY)
        && fonts.families().is_some_and(|families| {
            !families
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&config.font_family))
        });
    let custom = picker.custom || unlisted;
    let resolved = fonts
        .resolved_family(&config.font_family)
        .map(|name| format!("Using {name}."));
    let note = if let Some(error) = picker.error {
        Some(error)
    } else if fonts.unavailable(&config.font_family) {
        Some(
            "This font is unavailable. Using JetBrains Mono; choose or type an installed monospace font.",
        )
    } else if fonts.loading(&config.font_family) {
        Some("Loading fonts…")
    } else {
        resolved.as_deref()
    };
    let mut reveal = false;
    rows.row(ui, "Font family", |ui| {
        let response = select(
            ui,
            p,
            Id::new("preferences-font-family"),
            &config.font_family,
            "Font family",
            width,
        )
        .on_hover_text(&config.font_family);
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_value(config.font_family.as_str());
            if let Some(note) = note {
                node.set_description(note);
            }
        });
        let menu = egui::Popup::menu(&response).show(|ui| {
            menu_layout(ui, width.max(260.0));
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .min_scrolled_height(260.0)
                .show(ui, |ui| {
                    let mut choose = |ui: &mut Ui, name: &str, label: &str| {
                        let selected = !custom && config.font_family.eq_ignore_ascii_case(name);
                        if menu_item(
                            ui,
                            p,
                            if selected {
                                Icon::Check
                            } else {
                                Icon::TextSize
                            },
                            label,
                            "",
                            false,
                        ) {
                            config.font_family = name.to_owned();
                            picker.custom = false;
                            picker.sync(name);
                            picker.cancel();
                        }
                    };
                    choose(ui, DEFAULT_FAMILY, "JetBrains Mono (Default)");
                    if let Some(families) = fonts.families() {
                        for family in families {
                            choose(ui, family, family);
                        }
                    } else {
                        ui.label(
                            egui::RichText::new("Finding installed fonts…")
                                .font(theme::regular(12.5))
                                .color(p.muted),
                        );
                    }
                });
            menu_separator(ui, p);
            if menu_item(
                ui,
                p,
                if custom { Icon::Check } else { Icon::Pencil },
                "Custom…",
                "",
                false,
            ) {
                picker.custom = true;
                reveal = true;
            }
        });
        if let Some(menu) = menu {
            ui.ctx().move_to_top(menu.response.layer_id);
        }
    });
    if picker.custom || unlisted || reveal {
        rows.row(ui, "Custom name", |ui| {
            let response = text_field(
                ui,
                p,
                Id::new("preferences-custom-font"),
                &mut picker.draft,
                "Font family name",
                "Custom font family",
                width,
            );
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_description("Press Enter to apply the font family name.")
            });
            if reveal {
                response.request_focus();
            }
            if reveal || response.gained_focus() {
                ui.scroll_to_rect(response.rect.expand(14.0), None);
            }
            if response.changed() {
                picker.error = None;
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                picker.commit(config);
                ui.ctx().request_repaint();
            }
        });
    }
    if let Some(note) = note {
        rows.note(ui, note);
    }
}

fn shell(ui: &mut Ui, p: Palette, shown: &Shown, config: &mut Config, shells: &mut ShellPicker) {
    let pane = Pane::Shell;
    card(
        ui,
        p,
        shown,
        pane,
        "Startup program",
        &[Setting::ShellProgram],
        |ui, rows| {
            shell_rows(ui, p, config, shells, rows);
            rows.note(ui, "New terminals start this program.");
        },
    );
    card(
        ui,
        p,
        shown,
        pane,
        "History",
        &[Setting::Scrollback],
        |ui, rows| {
            rows.row(ui, "Scrollback", |ui| {
                ui.add(
                    egui::DragValue::new(&mut config.scrollback)
                        .range(0..=1_000_000)
                        .speed(100)
                        .suffix(" lines"),
                )
                .widget_info(|| {
                    WidgetInfo::labeled(WidgetType::DragValue, true, "Scrollback lines")
                });
            });
            rows.note(
                ui,
                "Lines each terminal keeps above the screen. Applies to new terminals.",
            );
        },
    );
}

fn notifications(ui: &mut Ui, p: Palette, shown: &Shown, config: &mut Config) {
    card(
        ui,
        p,
        shown,
        Pane::Notifications,
        "Alerts",
        &[Setting::DesktopBanners],
        |ui, rows| {
            rows.row(ui, "Desktop banners", |ui| {
                toggle(ui, p, &mut config.desktop_notifications, "Desktop banners");
            });
            rows.note(
                ui,
                "Show a system banner when a terminal asks for attention. Pane rings, unread counts and history stay on.",
            );
        },
    );
}

fn update_settings(
    ui: &mut Ui,
    p: Palette,
    shown: &Shown,
    config: &mut Config,
    updates: &Updates,
    actions: &mut Vec<Action>,
) {
    use crate::runtime::updates::ReleaseChannel;
    use Setting::{CheckUpdates, Version};
    let pane = Pane::Updates;
    card(
        ui,
        p,
        shown,
        pane,
        "Automatic updates",
        &[CheckUpdates, Setting::ReleaseChannel],
        |ui, rows| {
            if shown.has(CheckUpdates) {
                rows.row(ui, "Check automatically", |ui| {
                    toggle(
                        ui,
                        p,
                        &mut config.check_updates,
                        "Check for updates automatically",
                    );
                });
            }
            if shown.has(Setting::ReleaseChannel) {
                rows.row(ui, "Release channel", |ui| {
                    segmented(
                        ui,
                        p,
                        "release-channel",
                        &mut config.release_channel,
                        &[
                            (ReleaseChannel::Stable, "Stable"),
                            (ReleaseChannel::Beta, "Beta"),
                        ],
                        150.0,
                    );
                });
                rows.note(
                    ui,
                    "Beta includes previews and newer stable releases. Updates are installed only when you choose.",
                );
            }
        },
    );
    card(
        ui,
        p,
        shown,
        pane,
        "This version",
        &[Version],
        |ui, rows| {
            super::updates::preferences_status(ui, p, updates, rows, actions);
        },
    );
}

/// The shells found on this computer, and whether the user chose to enter a
/// program. Reset each time Preferences opens, so new installs appear.
#[derive(Default)]
pub struct ShellPicker {
    detection: Detection,
    custom: bool,
}

/// A menu of detected shells, as Windows Terminal offers profiles, with a
/// program field for anything else. Only the program and its arguments are
/// saved, so a shell that is later uninstalled shows as a custom program.
fn shell_rows(
    ui: &mut Ui,
    p: Palette,
    config: &mut Config,
    picker: &mut ShellPicker,
    rows: &mut Group,
) {
    let width = (ui.available_width() - 110.0).clamp(120.0, 250.0);
    let detected = picker.detection.poll(ui.ctx());
    let shells = detected.unwrap_or_default();
    let default = match shells.iter().find(|shell| shell.default) {
        Some(shell) => format!("System default ({})", shell.name),
        None => "System default".into(),
    };
    let chosen: Option<&Shell> = config.shell.as_ref().and_then(|program| {
        shells
            .iter()
            .find(|shell| shell.runs(program, &config.shell_args))
    });
    let custom =
        picker.custom || (detected.is_some() && config.shell.is_some() && chosen.is_none());
    let value = match (&config.shell, chosen) {
        _ if custom => "Custom".to_owned(),
        (None, _) => default.clone(),
        (Some(_), Some(shell)) => shell.name.clone(),
        // Still detecting: name the program rather than guess its profile.
        (Some(program), None) => std::path::Path::new(program).file_stem().map_or_else(
            || program.clone(),
            |name| name.to_string_lossy().into_owned(),
        ),
    };
    // Choosing Custom… brings its field into view, ready for typing.
    let mut reveal = false;
    rows.row(ui, "Program", |ui| {
        let response = select(
            ui,
            p,
            Id::new("preferences-shell-menu"),
            &value,
            "Shell",
            width,
        );
        let menu = egui::Popup::menu(&response).show(|ui| {
            menu_layout(ui, 300.0);
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    let mark = |selected: bool| {
                        if selected {
                            Icon::Check
                        } else {
                            Icon::Terminal
                        }
                    };
                    if menu_item(
                        ui,
                        p,
                        mark(!custom && config.shell.is_none()),
                        &default,
                        "",
                        false,
                    ) {
                        config.shell = None;
                        config.shell_args.clear();
                        picker.custom = false;
                    }
                    for shell in shells.iter().filter(|shell| !shell.default) {
                        let selected = !custom && chosen == Some(shell);
                        if menu_item(ui, p, mark(selected), &shell.name, "", false) {
                            config.shell = Some(shell.program.clone());
                            config.shell_args = shell.args.clone();
                            picker.custom = false;
                        }
                    }
                    if detected.is_none() {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("   Finding shells…")
                                .font(theme::regular(12.5))
                                .color(p.muted),
                        );
                        ui.add_space(4.0);
                    }
                    menu_separator(ui, p);
                    let icon = if custom { Icon::Check } else { Icon::Pencil };
                    if menu_item(ui, p, icon, "Custom…", "", false) {
                        // The program stays for editing; a profile's arguments do not.
                        config.shell_args.clear();
                        picker.custom = true;
                        reveal = true;
                    }
                });
        });
        // Both the sheet and the menu are foreground layers; a reopened sheet
        // can otherwise stay above a menu that was shown before.
        if let Some(menu) = menu {
            ui.ctx().move_to_top(menu.response.layer_id);
        }
    });
    if custom || reveal {
        rows.row(ui, "Path", |ui| {
            let mut shell = config.shell.clone().unwrap_or_default();
            let response = text_field(
                ui,
                p,
                Id::new("preferences-shell"),
                &mut shell,
                "Program path",
                "Shell program",
                width,
            );
            if reveal {
                response.request_focus();
            }
            if reveal || response.gained_focus() {
                // Leave room for the focus ring and the row below the field.
                ui.scroll_to_rect(response.rect.expand(14.0), None);
            }
            if response.changed() {
                config.shell = (!shell.trim().is_empty()).then_some(shell);
                if config.shell.is_none() {
                    config.shell_args.clear();
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finds(query: &str) -> Vec<Setting> {
        search(query).unwrap_or_default()
    }

    #[test]
    fn custom_font_drafts_only_apply_when_committed() {
        let mut config = Config::default();
        let mut picker = FontPicker::default();
        picker.sync(&config.font_family);
        picker.custom = true;
        picker.draft = "  Installed Mono  ".into();
        assert_eq!(config.font_family, DEFAULT_FAMILY);
        picker.commit(&mut config);
        assert_eq!(config.font_family, "Installed Mono");
        assert_eq!(picker.draft, config.font_family);
        assert!(picker.custom);
    }

    #[test]
    fn invalid_custom_font_drafts_keep_the_applied_font() {
        let mut config = Config::default();
        let mut picker = FontPicker::default();
        picker.sync(&config.font_family);
        for draft in [
            "".to_owned(),
            " ".into(),
            "bad\nfont".into(),
            "x".repeat(129),
        ] {
            picker.draft = draft;
            picker.commit(&mut config);
            assert!(picker.error.is_some());
            assert_eq!(config.font_family, DEFAULT_FAMILY);
        }
        picker.cancel();
        assert_eq!(picker.draft, DEFAULT_FAMILY);
        assert!(picker.error.is_none());
    }

    #[test]
    fn selecting_a_font_replaces_a_custom_draft_and_reopening_cancels_it() {
        let mut view = View::default();
        view.font.sync(DEFAULT_FAMILY);
        view.font.custom = true;
        view.font.draft = "unsaved".into();
        view.opened();
        assert_eq!(view.font.draft, DEFAULT_FAMILY);
        view.font.draft = "another draft".into();
        view.font.sync("Selected Mono");
        assert_eq!(view.font.draft, "Selected Mono");
        assert!(!view.font.custom);
    }

    #[test]
    fn a_blank_query_is_not_a_search() {
        assert_eq!(search(""), None);
        assert_eq!(search("  , "), None);
        assert_eq!(search("zzzz"), Some(vec![]));
    }

    #[test]
    fn labels_match_from_the_start_of_any_word_in_any_case() {
        assert_eq!(finds("scroll"), [Setting::Scrollback]);
        assert_eq!(finds("ZOOM")[0], Setting::WindowZoom);
        assert_eq!(finds("line sp"), [Setting::LineSpacing]);
        // Three letters also find the inside of a word.
        assert!(finds("back").contains(&Setting::Scrollback));
    }

    #[test]
    fn other_words_for_a_setting_find_it() {
        for query in [
            "keybindings",
            "shortcuts",
            "hotkeys",
            "remap",
            "config file",
            "shortcut reference",
            "key syntax",
            "action names",
        ] {
            assert_eq!(finds(query), [Setting::Keybindings]);
        }
        assert_eq!(finds("monospace"), [Setting::FontFamily]);
        assert_eq!(finds("font family"), [Setting::FontFamily]);
        assert_eq!(finds("custom font"), [Setting::FontFamily]);
        assert_eq!(finds("dark mode"), [Setting::Theme]);
        assert_eq!(finds("caret"), [Setting::CursorStyle, Setting::CursorBlink]);
        assert_eq!(finds("history"), [Setting::Scrollback]);
        assert_eq!(finds("zsh"), [Setting::ShellProgram]);
        assert!(finds("colour").contains(&Setting::Theme));
    }

    #[test]
    fn a_pane_name_finds_all_of_its_settings() {
        assert_eq!(
            finds("updates"),
            [
                Setting::CheckUpdates,
                Setting::ReleaseChannel,
                Setting::Version
            ]
        );
        // Further words narrow the pane down.
        assert_eq!(finds("updates beta"), [Setting::ReleaseChannel]);
    }

    #[test]
    fn misspellings_count_only_when_nothing_matches_as_typed() {
        assert_eq!(finds("scrolback"), [Setting::Scrollback]);
        assert_eq!(finds("notifcations"), [Setting::DesktopBanners]);
        assert_eq!(finds("blnik"), [Setting::CursorBlink]);
        // "beam" is a cursor style; it is not also a slip for "beta".
        assert_eq!(finds("beam"), [Setting::CursorStyle]);
        // Short words are taken as typed.
        assert_eq!(finds("zsj"), []);
    }

    #[test]
    fn every_setting_is_indexed_once_under_its_own_label() {
        for (index, (setting, pane, label, _)) in INDEX.iter().enumerate() {
            assert_eq!(pane_of(*setting), *pane);
            assert!(!INDEX[..index].iter().any(|entry| entry.0 == *setting));
            assert!(finds(label).contains(setting), "{label}");
        }
    }

    #[test]
    fn escape_leaves_a_search_before_the_sheet() {
        let mut view = View::searching("font");
        assert!(view.back());
        assert!(!view.back());
    }
}
