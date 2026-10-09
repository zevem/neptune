//! Label and window helpers, plus the shared controls of the visual system.
pub use super::controls::*;
use crate::runtime::pull_requests::{Checks, Lookup, Preview, State, unresolved_label};
use eframe::egui::{self, Rect, Sense, Vec2};
pub fn compact_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    if let Some(home) = directories::BaseDirs::new()
        && let Ok(relative) = path.strip_prefix(home.home_dir())
    {
        return format!("~/{}", relative.display());
    }
    text
}
pub fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!(
            "{}…",
            text.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    } else {
        text.into()
    }
}
pub fn regex_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if ".+*?()|[]{}^$\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn tail_label(text: &str, max: usize) -> String {
    let len = text.chars().count();
    if max == 0 {
        return String::new();
    }
    if len > max {
        format!(
            "…{}",
            text.chars()
                .skip(len - max.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        text.into()
    }
}

pub fn resize_edges(ui: &mut egui::Ui, bounds: Rect) {
    if ui.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    use egui::ResizeDirection::*;
    for (dir, rect, cursor) in [
        (
            North,
            Rect::from_min_size(
                bounds.min + Vec2::new(8.0, 0.0),
                Vec2::new(bounds.width() - 16.0, 4.0),
            ),
            egui::CursorIcon::ResizeVertical,
        ),
        (
            South,
            Rect::from_min_size(
                bounds.left_bottom() - Vec2::new(-8.0, 4.0),
                Vec2::new(bounds.width() - 16.0, 4.0),
            ),
            egui::CursorIcon::ResizeVertical,
        ),
        (
            West,
            Rect::from_min_size(
                bounds.min + Vec2::new(0.0, 8.0),
                Vec2::new(4.0, bounds.height() - 16.0),
            ),
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            East,
            Rect::from_min_size(
                bounds.right_top() + Vec2::new(-4.0, 8.0),
                Vec2::new(4.0, bounds.height() - 16.0),
            ),
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            NorthWest,
            Rect::from_min_size(bounds.min, Vec2::splat(8.0)),
            egui::CursorIcon::ResizeNwSe,
        ),
        (
            NorthEast,
            Rect::from_min_size(bounds.right_top() - Vec2::new(8.0, 0.0), Vec2::splat(8.0)),
            egui::CursorIcon::ResizeNeSw,
        ),
        (
            SouthWest,
            Rect::from_min_size(bounds.left_bottom() - Vec2::new(0.0, 8.0), Vec2::splat(8.0)),
            egui::CursorIcon::ResizeNeSw,
        ),
        (
            SouthEast,
            Rect::from_min_size(bounds.max - Vec2::splat(8.0), Vec2::splat(8.0)),
            egui::CursorIcon::ResizeNwSe,
        ),
    ] {
        let r = ui
            .interact(
                rect,
                ui.id().with(("window-resize", format!("{dir:?}"))),
                Sense::drag(),
            )
            .on_hover_cursor(cursor);
        if r.drag_started() {
            crate::platform::window::send(
                ui.ctx(),
                crate::platform::window::WindowOperation::Resize(dir),
            );
        }
    }
}
pub fn path_label(path: &std::path::Path, max: usize) -> String {
    let full = compact_path(path);
    if full.chars().count() <= max {
        return full;
    }
    let mut tail = String::new();
    for component in path.components().rev() {
        let segment = component.as_os_str().to_string_lossy();
        let candidate = if tail.is_empty() {
            segment.to_string()
        } else {
            format!("{segment}/{tail}")
        };
        if candidate.chars().count() + 2 > max {
            break;
        }
        tail = candidate;
    }
    if tail.is_empty() {
        tail = ellipsize(
            &path.file_name().unwrap_or_default().to_string_lossy(),
            max.saturating_sub(2),
        );
    }
    format!("…/{tail}")
}

/// A linked pull request and what is known of its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedPullRequest {
    pub link: neptune_model::PullRequest,
    pub lookup: Lookup,
    /// Its title, author and age, once its state was read.
    pub preview: Option<Preview>,
}
impl LinkedPullRequest {
    /// Its state in the chip's icon and colour. A merged pull request has its
    /// own icon, so the state does not rest on colour alone.
    fn ink(&self, p: crate::theme::Palette) -> (crate::icons::Icon, egui::Color32) {
        use crate::icons::Icon;
        match self.lookup.status().map(|status| status.state) {
            Some(State::Merged) => (Icon::Merged, p.merged),
            Some(State::Closed) => (Icon::PullRequest, p.muted),
            Some(State::Draft) => (Icon::PullRequest, p.secondary),
            Some(State::Open) | None => (Icon::PullRequest, p.accent),
        }
    }
    /// What needs a person: the checks and the unresolved comments.
    fn marks(&self) -> (Checks, u8) {
        self.lookup.status().map_or((Checks::None, 0), |status| {
            (status.checks(), status.unresolved())
        })
    }
    fn hint(&self) -> String {
        match self.lookup {
            Lookup::Known(status) => status.describe(),
            Lookup::Unavailable => "Status unavailable".into(),
            Lookup::Checking => String::new(),
        }
    }
}

struct PullRequestChip {
    rect: Rect,
    icon: crate::icons::Icon,
    ink: egui::Color32,
    number: std::sync::Arc<egui::Galley>,
    checks: Checks,
    /// How many comments are unresolved, when any are.
    unresolved: Option<std::sync::Arc<egui::Galley>>,
    response: Option<egui::Response>,
}
impl PullRequestChip {
    /// Its width with or without its marks; the one that lists has a chevron.
    fn width(&self, marks: bool, lists: bool) -> f32 {
        let mut width = self.number.size().x + 28.0;
        if marks && self.checks != Checks::None {
            width += 14.0;
        }
        if marks && let Some(unresolved) = &self.unresolved {
            width += 18.5 + unresolved.size().x;
        }
        if lists { width + 13.0 } else { width }
    }
}

/// Pull request numbers that open their pull request in the panel's tab, or
/// in the browser when pressed as a terminal's link is, laid out leading from
/// a trailing edge. Each carries its state: a colour and icon once merged,
/// closed or a draft, and while in review a mark for its checks and a count of
/// unresolved comments. Interaction is claimed first so the surface beneath
/// can follow their hover; `paint` then draws them over it.
pub struct PullRequestChips<'a> {
    /// A number for each link, or the newest number standing for all of them.
    chips: Vec<PullRequestChip>,
    links: &'a [LinkedPullRequest],
    menu: bool,
    /// Leading edge of the chips; the trailing edge given when there are none.
    pub left: f32,
}
impl<'a> PullRequestChips<'a> {
    /// The most links shown side by side; more are listed in a menu.
    const SIDE_BY_SIDE: usize = 2;
    /// The accessible name of the chip that lists them.
    const LIST: &'static str = "Pull requests";

    /// The newest link sits at `right` with the older one before it. More
    /// links, or links without room to stay clear of `limit`, share one chip
    /// that lists them. Marks are given up before a number is.
    pub fn layout(
        ui: &egui::Ui,
        id: egui::Id,
        links: &'a [LinkedPullRequest],
        (limit, right, middle): (f32, f32, f32),
        p: crate::theme::Palette,
    ) -> Self {
        let measure = |linked: &LinkedPullRequest, (checks, unresolved): (Checks, u8)| {
            let (icon, ink) = linked.ink(p);
            let text = |text: String, size: f32, ink| {
                ui.painter()
                    .layout_no_wrap(text, crate::theme::medium(size), ink)
            };
            PullRequestChip {
                rect: Rect::NOTHING,
                icon,
                ink,
                number: text(linked.link.number().to_string(), 11.5, ink),
                checks,
                unresolved: (unresolved > 0)
                    .then(|| text(unresolved_label(unresolved), 11.0, p.attention)),
                response: None,
            }
        };
        let mut chips = Vec::new();
        let mut left = right;
        let mut place = |mut chip: PullRequestChip, marks: bool, id: egui::Id, label: String| {
            let lists = label == Self::LIST;
            let width = chip.width(marks, lists);
            chip.rect = Rect::from_min_max(
                egui::pos2(left - width, middle - 9.0),
                egui::pos2(left, middle + 9.0),
            );
            left = chip.rect.left() - 1.0;
            if !marks {
                (chip.checks, chip.unresolved) = (Checks::None, None);
            }
            let response = ui.interact(chip.rect, id, Sense::click());
            response.widget_info(|| {
                let kind = if lists {
                    egui::WidgetType::Button
                } else {
                    egui::WidgetType::Link
                };
                egui::WidgetInfo::labeled(kind, true, &label)
            });
            chip.response = Some(response);
            chips.push(chip);
        };
        let room = |chips: &[PullRequestChip], lists: bool| {
            [true, false].into_iter().find(|marks| {
                let width: f32 = chips
                    .iter()
                    .map(|chip| chip.width(*marks, lists) + 1.0)
                    .sum();
                right - width >= limit
            })
        };
        let side_by_side: Vec<_> = links
            .iter()
            .rev()
            .map(|linked| measure(linked, linked.marks()))
            .collect();
        let beside = (links.len() <= Self::SIDE_BY_SIDE)
            .then(|| room(&side_by_side, false))
            .flatten();
        let menu = beside.is_none();
        if let Some(marks) = beside {
            for (linked, chip) in links.iter().rev().zip(side_by_side) {
                let label = format!("Open pull request {}", linked.link.label());
                place(chip, marks, id.with(linked.link.url()), label);
            }
        } else if let Some(shown) = links
            .iter()
            .rev()
            .find(|linked| {
                linked
                    .lookup
                    .status()
                    .is_some_and(|status| status.in_review())
            })
            .or(links.last())
            && links.len() > 1
        {
            // The newest still in review, before one that is merged or
            // closed, with what needs a person in any of them.
            let all = links
                .iter()
                .map(LinkedPullRequest::marks)
                .fold((Checks::None, 0u8), |all, marks| {
                    (all.0.max(marks.0), all.1.saturating_add(marks.1))
                });
            let chip = measure(shown, all);
            if let Some(marks) = room(std::slice::from_ref(&chip), true) {
                place(chip, marks, id.with("menu"), Self::LIST.into());
            }
        }
        Self {
            chips,
            links,
            menu,
            left,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.chips.is_empty()
    }
    pub fn hovered(&self) -> bool {
        self.chips
            .iter()
            .filter_map(|chip| chip.response.as_ref())
            .any(egui::Response::hovered)
    }
    pub fn paint(
        self,
        painter: &egui::Painter,
        p: crate::theme::Palette,
        actions: &mut Vec<super::Action>,
    ) {
        use crate::icons::{self, Icon};
        // The press that opens a terminal's link in the browser does so here.
        let outside = painter
            .ctx()
            .input(|input| input.modifiers.ctrl || input.modifiers.mac_cmd);
        let open = |link: &neptune_model::PullRequest, actions: &mut Vec<super::Action>| {
            if !outside {
                let shown = super::pull_request::Event::Open(link.clone());
                actions.push(super::Action::PullRequest(shown));
            } else if let Some(link) = crate::platform::links::WebLink::new(link.url()) {
                actions.push(super::Action::OpenLink(link));
            }
        };
        // Chips run newest first, as the links do from their end.
        for (chip, linked) in self.chips.into_iter().zip(self.links.iter().rev()) {
            let Some(response) = chip.response else {
                continue;
            };
            let (rect, middle) = (chip.rect, chip.rect.center().y);
            let listing = self.menu
                && egui::Popup::is_id_open(
                    &response.ctx,
                    egui::Popup::default_response_id(&response),
                );
            if response.hovered() || listing {
                painter.rect_filled(rect, 5, crate::theme::tint(chip.ink, 0.14));
            }
            icons::paint(
                painter,
                Rect::from_center_size(egui::pos2(rect.left() + 11.5, middle), Vec2::splat(13.0)),
                chip.icon,
                chip.ink,
            );
            let mut x = rect.left() + 22.0 + chip.number.size().x;
            galley_at(
                painter,
                egui::pos2(rect.left() + 22.0, middle + 1.0),
                chip.number,
            );
            // A shape for each state of the checks, as well as its colour.
            let mark = Rect::from_center_size(egui::pos2(x + 9.0, middle + 0.5), Vec2::splat(10.0));
            match chip.checks {
                Checks::None => {}
                Checks::Passing => icons::paint(painter, mark, Icon::Check, p.green),
                Checks::Failing => icons::paint(painter, mark, Icon::Close, p.red),
                Checks::Pending => {
                    painter.circle_stroke(mark.center(), 3.25, egui::Stroke::new(1.5, p.yellow));
                }
            }
            if chip.checks != Checks::None {
                x += 14.0;
            }
            if let Some(unresolved) = chip.unresolved {
                icons::paint(
                    painter,
                    Rect::from_center_size(egui::pos2(x + 10.5, middle + 1.0), Vec2::splat(11.0)),
                    Icon::Comment,
                    p.attention,
                );
                galley_at(painter, egui::pos2(x + 18.5, middle + 1.0), unresolved);
            }
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            if !self.menu {
                // A card that says what it is before it is opened: its
                // title, where it lives, who opened it and when, how it
                // stands, and the other way to open it.
                let stands = match linked.lookup {
                    Lookup::Known(status) => status.describe(),
                    Lookup::Unavailable => {
                        "Status unavailable. Neptune reads it with the GitHub CLI (gh), signed in."
                            .to_owned()
                    }
                    Lookup::Checking => String::new(),
                };
                let said = linked
                    .preview
                    .as_ref()
                    .filter(|said| !said.title.is_empty());
                let hint = if cfg!(target_os = "macos") {
                    "Click to read it here · ⌘-click for the browser"
                } else {
                    "Click to read it here · Ctrl+click for the browser"
                };
                let (icon, ink) = (chip.icon, chip.ink);
                let response = response.on_hover_ui(|ui| {
                    use crate::theme;
                    ui.set_max_width(300.0);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    let label = |ui: &mut egui::Ui, text: String, font, color| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).font(font).color(color))
                                .wrap()
                                .selectable(false),
                        );
                    };
                    match said {
                        Some(said) => {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |since| since.as_secs() as i64);
                            label(ui, said.title.clone(), theme::medium(12.5), p.fg);
                            label(
                                ui,
                                format!(
                                    "{} · {} · opened {}",
                                    linked.link.label(),
                                    said.author,
                                    super::pull_request::ago(now - said.opened)
                                ),
                                theme::regular(11.5),
                                p.secondary,
                            );
                        }
                        None => label(
                            ui,
                            format!("Pull request {}", linked.link.label()),
                            theme::medium(12.5),
                            p.fg,
                        ),
                    }
                    if !stands.is_empty() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            let (_, mark) = ui.allocate_space(Vec2::splat(13.0));
                            icons::paint(ui.painter(), mark, icon, ink);
                            label(ui, stands.clone(), theme::regular(11.5), ink);
                        });
                    }
                    label(ui, hint.to_owned(), theme::regular(11.0), p.muted);
                });
                if response.clicked() {
                    open(&linked.link, actions);
                }
                continue;
            }
            icons::paint(
                painter,
                Rect::from_center_size(
                    egui::pos2(rect.right() - 10.0, middle + 0.5),
                    Vec2::splat(10.0),
                ),
                Icon::ChevronDown,
                chip.ink,
            );
            egui::Popup::menu(&response).show(|ui| {
                // As wide as its longest entry and that entry's state.
                let widest = self
                    .links
                    .iter()
                    .map(|linked| {
                        let width = |text: String, font| {
                            ui.painter().layout_no_wrap(text, font, p.fg).size().x
                        };
                        width(linked.link.label(), crate::theme::regular(13.0))
                            + width(linked.hint(), crate::theme::regular(12.0))
                    })
                    .fold(0.0, f32::max);
                menu_layout(ui, (widest + 74.0).clamp(120.0, 460.0));
                for linked in self.links.iter().rev() {
                    let (icon, _) = linked.ink(p);
                    if menu_item(ui, p, icon, &linked.link.label(), &linked.hint(), false) {
                        open(&linked.link, actions);
                        ui.close();
                    }
                }
            });
            if !listing {
                response.on_hover_text(format!("{} pull requests", self.links.len()));
            }
        }
    }
}

/// How many agents the terminal's agent started, as a chip that lists them
/// and opens the terminal of the one chosen, which has no tab until then. Laid out and painted like
/// `PullRequestChips`, before which it sits.
pub struct SpawnedChip<'a> {
    chip: Option<(Rect, std::sync::Arc<egui::Galley>, egui::Response)>,
    agents: &'a [super::agents::Spawned],
    /// Leading edge of the chip; the trailing edge given when there is none.
    pub left: f32,
}
impl<'a> SpawnedChip<'a> {
    pub fn layout(
        ui: &egui::Ui,
        id: egui::Id,
        agents: &'a [super::agents::Spawned],
        (limit, right, middle): (f32, f32, f32),
        p: crate::theme::Palette,
    ) -> Self {
        let mut chip = None;
        let mut left = right;
        if !agents.is_empty() {
            let count = ui.painter().layout_no_wrap(
                agents.len().to_string(),
                crate::theme::medium(11.5),
                Self::ink(agents, p),
            );
            let width = count.size().x + 41.0;
            if right - width >= limit {
                let rect = Rect::from_min_max(
                    egui::pos2(right - width, middle - 9.0),
                    egui::pos2(right, middle + 9.0),
                );
                left = rect.left() - 1.0;
                let response = ui.interact(rect, id, Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Started agents")
                });
                chip = Some((rect, count, response));
            }
        }
        Self { chip, agents, left }
    }
    /// The attention colour while a started agent waits for a person.
    fn ink(agents: &[super::agents::Spawned], p: crate::theme::Palette) -> egui::Color32 {
        if agents.iter().any(super::agents::Spawned::waits) {
            p.attention
        } else {
            p.accent
        }
    }
    pub fn is_empty(&self) -> bool {
        self.chip.is_none()
    }
    pub fn hovered(&self) -> bool {
        self.chip.as_ref().is_some_and(|chip| chip.2.hovered())
    }
    pub fn paint(
        self,
        painter: &egui::Painter,
        p: crate::theme::Palette,
        actions: &mut Vec<super::Action>,
    ) {
        use crate::icons::{self, Icon};
        let Some((chip, count, response)) = self.chip else {
            return;
        };
        let ink = Self::ink(self.agents, p);
        let listing =
            egui::Popup::is_id_open(&response.ctx, egui::Popup::default_response_id(&response));
        if response.hovered() || listing {
            painter.rect_filled(chip, 5, crate::theme::tint(ink, 0.14));
        }
        icons::paint(
            painter,
            Rect::from_center_size(
                egui::pos2(chip.left() + 11.5, chip.center().y),
                Vec2::splat(13.0),
            ),
            Icon::Agents,
            ink,
        );
        galley_at(
            painter,
            egui::pos2(chip.left() + 22.0, chip.center().y + 1.0),
            count,
        );
        icons::paint(
            painter,
            Rect::from_center_size(
                egui::pos2(chip.right() - 10.0, chip.center().y + 0.5),
                Vec2::splat(10.0),
            ),
            Icon::ChevronDown,
            ink,
        );
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        egui::Popup::menu(&response).show(|ui| {
            // As wide as its longest entry and that entry's state.
            let widest = self
                .agents
                .iter()
                .map(|agent| {
                    let width = |text: &str, font| {
                        ui.painter()
                            .layout_no_wrap(text.to_owned(), font, p.fg)
                            .size()
                            .x
                    };
                    width(&ellipsize(&agent.title, 40), crate::theme::regular(13.0))
                        + width(agent.state(), crate::theme::regular(12.0))
                })
                .fold(0.0, f32::max);
            menu_layout(ui, (widest + 74.0).clamp(160.0, 420.0));
            for agent in self.agents {
                let chosen = ui
                    .push_id(agent.pane, |ui| {
                        menu_item(
                            ui,
                            p,
                            Icon::Terminal,
                            &ellipsize(&agent.title, 40),
                            agent.state(),
                            false,
                        )
                    })
                    .inner;
                if chosen {
                    actions.push(super::Action::Focus(agent.pane));
                    ui.close();
                }
            }
            // Under them, the way back out of view for each one that was
            // opened as a tab.
            if self.agents.iter().any(|agent| agent.can_background) {
                menu_separator(ui, p);
            }
            for agent in self.agents.iter().filter(|agent| agent.can_background) {
                let label = format!("Send {} to background", ellipsize(&agent.title, 24));
                let chosen = ui
                    .push_id(("background", agent.pane), |ui| {
                        menu_item(ui, p, Icon::Minus, &label, "", false)
                    })
                    .inner;
                if chosen {
                    actions.push(super::Action::Background(agent.pane));
                    ui.close();
                }
            }
        });
        if !listing {
            response.on_hover_text(match self.agents.len() {
                1 => "1 agent started from this terminal".to_owned(),
                count => format!("{count} agents started from this terminal"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_labels_truncate_safely() {
        assert_eq!(ellipsize("日本語の端末", 4), "日本語…");
        assert_eq!(ellipsize("", 0), "");
        assert_eq!(regex_escape("a.b[0]"), "a\\.b\\[0\\]");
    }

    #[test]
    fn pull_request_chips_give_up_their_marks_before_their_numbers() {
        use crate::runtime::pull_requests::Status;
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let p = crate::theme::Palette::new(crate::config::Theme::Graphite);
        let linked = |number: u64, state, checks, unresolved| LinkedPullRequest {
            link: neptune_model::PullRequest::parse(&format!(
                "https://github.com/zevem/neptune/pull/{number}"
            ))
            .unwrap(),
            lookup: Lookup::Known(Status {
                state,
                checks,
                unresolved,
            }),
            preview: None,
        };
        let failing = linked(7, State::Open, Checks::Failing, 2);
        let merged = linked(8, State::Merged, Checks::Failing, 3);
        assert_eq!(failing.ink(p), (crate::icons::Icon::PullRequest, p.accent));
        assert_eq!(merged.ink(p), (crate::icons::Icon::Merged, p.merged));
        // Checks and comments stop mattering once the review is over.
        assert_eq!(merged.marks(), (Checks::None, 0));
        assert_eq!(merged.hint(), "Merged");
        let draft = linked(9, State::Draft, Checks::Pending, 1);
        let links = [failing, merged, draft];
        // Each layout: how many chips, whether they list, and their width.
        let layout = |links: &[LinkedPullRequest], room: f32| {
            let mut found = None;
            let mut output = ctx.run_ui(Default::default(), |ui| {
                let chips = PullRequestChips::layout(
                    ui,
                    ui.id().with(room.to_bits()),
                    links,
                    (400.0 - room, 400.0, 20.0),
                    p,
                );
                found = Some((chips.chips.len(), chips.menu, 400.0 - chips.left));
            });
            output.textures_delta.clear();
            found.unwrap()
        };
        let (count, menu, marked) = layout(&links[..1], 200.0);
        assert_eq!((count, menu), (1, false));
        // Without room for its marks, the number stays and is narrower.
        let (count, menu, plain) = layout(&links[..1], marked - 2.0);
        assert_eq!((count, menu), (1, false));
        assert!(plain < marked - 30.0, "{plain} {marked}");
        // Without room for the number either, nothing is shown.
        assert_eq!(layout(&links[..1], plain - 2.0).0, 0);
        // Two sit side by side while they fit; then one number lists both.
        let (count, menu, both) = layout(&links[..2], 200.0);
        assert_eq!((count, menu), (2, false));
        let (count, menu, _) = layout(&links[..2], both - 60.0);
        assert_eq!((count, menu), (1, true));
        // Three always share one number, which carries the worst of their
        // checks and all their unresolved comments.
        let (count, menu, several) = layout(&links, 200.0);
        assert_eq!((count, menu), (1, true));
        assert!(several > layout(&links[2..], 200.0).2);
        // The number shown is the newest still in review, not a newer one
        // that is merged: the same chip whichever was linked last.
        let reordered = [links[0].clone(), links[2].clone(), links[1].clone()];
        assert_eq!(layout(&reordered, 200.0), (1, true, several));
        let narrower = [links[1].clone(), linked(10, State::Closed, Checks::None, 0)];
        let shown = |links: &[LinkedPullRequest]| {
            let mut ink = None;
            let mut output = ctx.run_ui(Default::default(), |ui| {
                let chips = PullRequestChips::layout(ui, ui.id(), links, (200.0, 400.0, 20.0), p);
                ink = chips.chips.first().map(|chip| chip.ink);
            });
            output.textures_delta.clear();
            ink
        };
        assert_eq!(shown(&reordered), Some(p.secondary));
        // With none in review, the newest stands for them.
        assert_eq!(shown(&narrower), Some(p.muted));
    }

    #[test]
    fn a_pressed_number_opens_its_tab_and_the_browser_when_pressed_as_a_link_is() {
        use eframe::egui::{Event, Modifiers, PointerButton, RawInput, pos2};
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::platform::fonts::bundled_definitions());
        let p = crate::theme::Palette::new(crate::config::Theme::Graphite);
        let links = [LinkedPullRequest {
            link: neptune_model::PullRequest::parse("https://github.com/zevem/neptune/pull/83")
                .unwrap(),
            lookup: Lookup::Checking,
            preview: None,
        }];
        let press = |modifiers: Modifiers| {
            let mut actions = Vec::new();
            let mut at = pos2(390.0, 20.0);
            let button = |pressed| Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            };
            for events in [
                vec![Event::PointerMoved(at)],
                vec![button(true)],
                vec![button(false)],
            ] {
                let mut events = events;
                events.insert(0, Event::ModifiersChanged(modifiers));
                let input = RawInput {
                    events,
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    let chips =
                        PullRequestChips::layout(ui, ui.id(), &links, (200.0, 400.0, 20.0), p);
                    at = pos2((chips.left + 400.0) * 0.5, 20.0);
                    let painter = ui.painter().clone();
                    chips.paint(&painter, p, &mut actions);
                });
                output.textures_delta.clear();
            }
            actions
        };
        assert!(matches!(
            press(Modifiers::NONE).as_slice(),
            [crate::ui::Action::PullRequest(crate::ui::pull_request::Event::Open(link))]
                if link.number() == 83
        ));
        for modifiers in [Modifiers::CTRL, Modifiers::MAC_CMD] {
            assert!(matches!(
                press(modifiers).as_slice(),
                [crate::ui::Action::OpenLink(link)] if link.as_str().ends_with("/pull/83")
            ));
        }
    }
}
