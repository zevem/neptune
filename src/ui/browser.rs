//! Native navigation chrome around an owned browser frame.
use super::{
    Action,
    helpers::{self, ButtonKind, menu_item, menu_layout, menu_separator, menu_submenu},
    workspace::{Stage, StageOutput},
};
use crate::{
    icons::{self, Icon},
    keybindings::BindingAction as Binding,
    runtime::browser::{
        View,
        protocol::{Command, Target},
    },
    theme::{self, Palette, metrics},
};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Id, Layout, Margin, Painter, Pos2, Rect,
    Response, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, vec2,
};
use neptune_model::PaneId;

/// Below this width the trailing actions fold into one "more" control.
const WIDE: f32 = 430.0;
/// Height of the browser navigation band.
const BAR_HEIGHT: f32 = 38.0;

/// Side of a navigation control's target, as in a pane header.
const CONTROL: f32 = 28.0;

/// The pointer a page asks for, from Chromium's `cef_cursor_type_t`.
fn cursor(kind: u32) -> egui::CursorIcon {
    use egui::CursorIcon::*;
    match kind {
        1 => Crosshair,
        2 => PointingHand,
        3 => Text,
        4 => Wait,
        5 => Help,
        6 => ResizeEast,
        7 => ResizeNorth,
        8 => ResizeNorthEast,
        9 => ResizeNorthWest,
        10 => ResizeSouth,
        11 => ResizeSouthEast,
        12 => ResizeSouthWest,
        13 => ResizeWest,
        14 => ResizeVertical,
        15 => ResizeHorizontal,
        16 => ResizeNeSw,
        17 => ResizeNwSe,
        18 => ResizeColumn,
        19 => ResizeRow,
        20..=28 | 43 | 44 => AllScroll,
        29 => Move,
        30 => VerticalText,
        31 => Cell,
        32 => ContextMenu,
        33 => Alias,
        34 => Progress,
        35 => NoDrop,
        36 => Copy,
        37 => None,
        38 => NotAllowed,
        39 => ZoomIn,
        40 => ZoomOut,
        41 => Grab,
        42 => Grabbing,
        _ => Default,
    }
}

/// The mark of a page on its way: an arc that turns while it loads.
pub(super) fn turning(painter: &egui::Painter, rect: Rect, color: egui::Color32) {
    let ctx = painter.ctx();
    let start = ctx.input(|i| i.time) as f32 * std::f32::consts::TAU * 0.9;
    let radius = rect.width().min(rect.height()) * 0.5 - 1.5;
    let points = (0..=20)
        .map(|step| {
            let angle = start + step as f32 / 20.0 * std::f32::consts::TAU * 0.7;
            rect.center() + radius * egui::vec2(angle.cos(), angle.sin())
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
    ctx.request_repaint();
}

/// What a browser tab does besides navigation, for the application to carry out.
#[derive(Clone)]
pub enum Tool {
    /// Start picking an element, or cancel a pick under way.
    Pick,
    /// Start recording the page, or end and save a recording under way.
    Record,
    /// Open this tab again in a saved profile, or a private one.
    Profile(Option<String>),
    /// Use this profile for new browser tabs.
    DefaultProfile(Option<String>),
    /// Make a profile and open this tab again in it.
    NewProfile,
    /// Remove this tab's profile and what it kept.
    RemoveProfile,
    ClearCookies,
    /// Look for other browsers to import cookies from.
    Sources,
    /// Copy another browser profile's cookies into this tab's profile.
    Import {
        source: String,
        directory: String,
    },
}

#[derive(Default)]
pub struct State {
    address: String,
    observed: String,
    pub focus_address: bool,
}

/// Browser tabs and navigation share this band without retuning app materials.
pub(super) fn bar(p: Palette) -> Color32 {
    theme::mix(p.bg, p.chrome, if p.dark { 0.72 } else { 0.55 })
}

/// The browser tab's selected and hovered surface. Terminal tabs use their
/// existing paint in the workspace module.
pub(super) fn tab_surface(
    painter: &Painter,
    rect: Rect,
    p: Palette,
    selected: bool,
    hovered: bool,
) {
    let hover = theme::tint(p.fg, if p.dark { 0.055 } else { 0.04 });
    if selected {
        painter.rect_filled(
            rect,
            7,
            theme::tint(p.fg, if p.dark { 0.10 } else { 0.075 }),
        );
        painter.rect_stroke(rect, 7, Stroke::new(1.0, hover), StrokeKind::Inside);
    } else if hovered {
        painter.rect_filled(rect, 7, hover);
    }
}

/// The browser address capsule, with its own focus treatment.
fn address_surface(painter: &Painter, rect: Rect, p: Palette, focused: bool) {
    painter.rect_filled(rect, 14, p.control);
    if focused {
        painter.rect_stroke(
            rect.expand(2.5),
            17,
            Stroke::new(3.0, theme::tint(p.accent, 0.26)),
            StrokeKind::Inside,
        );
        painter.rect_stroke(rect, 14, Stroke::new(1.0, p.accent), StrokeKind::Inside);
    } else {
        painter.rect_stroke(rect, 14, p.hairline(), StrokeKind::Inside);
    }
}

/// The browser's blank/error marker.
fn glyph_tile(painter: &Painter, center: Pos2, icon: Icon, ink: Color32) {
    let tile = Rect::from_center_size(center, Vec2::splat(48.0));
    painter.rect_filled(tile, 13, theme::tint(ink, 0.14));
    painter.rect_stroke(
        tile,
        13,
        Stroke::new(1.0, theme::tint(ink, 0.16)),
        StrokeKind::Inside,
    );
    icons::paint(painter, tile.shrink(13.0), icon, ink);
}

/// A navigation control: the header's icon button, with a resting state for
/// an action that cannot be taken now. `hint` is its shortcut, if any.
fn button(
    ui: &mut Ui,
    rect: Rect,
    (icon, label, hint): (Icon, &str, &str),
    enabled: bool,
    p: Palette,
    pane: PaneId,
) -> Response {
    let response = ui.interact(
        rect,
        ui.id().with(("browser-button", pane, label)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    let down = enabled && response.is_pointer_button_down_on();
    let surface = rect.shrink(1.0);
    let painter = ui.painter();
    if down || open {
        painter.rect_filled(surface, 7, p.pressed);
    } else if enabled && response.hovered() {
        painter.rect_filled(surface, 7, p.hover);
    }
    if response.has_focus() {
        painter.rect_stroke(surface, 7, Stroke::new(1.5, p.accent), StrokeKind::Inside);
    }
    icons::paint(
        painter,
        // Pressing settles the browser glyph to 0.96; its hit area stays fixed.
        rect.shrink(if down { 6.32 } else { 6.0 }),
        icon,
        if !enabled {
            theme::tint(p.muted, 0.6)
        } else if response.hovered() || open {
            p.fg
        } else {
            p.secondary
        },
    );
    if open {
        return response;
    }
    let response = if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    if hint.is_empty() {
        response.on_hover_text(label)
    } else {
        response.on_hover_text(format!("{label}   {hint}"))
    }
}

/// Names painted text for assistive tools and inspection. The label senses
/// only the pointer's presence: it never takes the keyboard from the page,
/// and a press on it reaches the pane beneath.
fn announce(ui: &Ui, bounds: Rect, id: Id, text: &str) -> Response {
    let response = ui.interact(bounds, id, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, text));
    response
}

/// What a pane says in place of a page: a tile, a title and the reason
/// beneath, centred as a block over an optional action. Returns where the
/// action goes. A short pane keeps the action first, then the title, then as
/// many lines of the reason as fit, and the tile last; words it has no room
/// to paint are still named for assistive tools.
fn placeholder(
    ui: &Ui,
    content: Rect,
    (p, pane): (Palette, PaneId),
    (icon, ink): (Icon, Color32),
    (title, detail): (&str, &str),
    action: bool,
) -> Rect {
    let painter = ui
        .painter()
        .with_clip_rect(content.intersect(ui.clip_rect()));
    let width = (content.width() - 32.0).clamp(40.0, 360.0);
    let lay = |text: &str, font: egui::FontId, ink: Color32, rows: usize| {
        let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, ink, width);
        job.halign = Align::Center;
        job.wrap.max_rows = rows;
        painter.layout_job(job)
    };
    let room = (content.height() - 20.0).max(0.0);
    let (button, gap) = if action {
        (metrics::CONTROL_HEIGHT, 14.0)
    } else {
        (0.0, 0.0)
    };
    let heading = lay(title, theme::semibold(15.0), p.fg, 2);
    let titled = heading.size().y + gap + button <= room;
    let reason = (1..=4)
        .rev()
        .map(|rows| lay(detail, theme::regular(12.5), p.secondary, rows))
        .find(|reason| heading.size().y + 6.0 + reason.size().y + gap + button <= room)
        .filter(|_| titled);
    let words = if titled {
        heading.size().y + reason.as_ref().map_or(0.0, |reason| 6.0 + reason.size().y)
    } else {
        0.0
    };
    let tile = titled && words + gap + button + 64.0 <= room;
    let above = if tile { 64.0 } else { 0.0 };
    let between = if titled { gap } else { 0.0 };
    let block = above + words + between + button;
    let middle = content.center().x;
    let mut top = (content.center().y - block * 0.5).max(content.top());
    if tile {
        glyph_tile(&painter, Pos2::new(middle, top + 24.0), icon, ink);
        top += 64.0;
    }
    // A galley centred on `middle`, as the rectangle it is painted in.
    let bounds =
        |top: f32, size: Vec2| Rect::from_min_size(Pos2::new(middle - size.x * 0.5, top), size);
    let mut named = content;
    if titled {
        named = bounds(top, heading.size());
        top += heading.size().y;
        painter.galley(
            Pos2::new(middle, named.top()),
            heading,
            Color32::PLACEHOLDER,
        );
    }
    announce(
        ui,
        named,
        Id::new(("browser-message", pane, "title")),
        title,
    );
    let said = reason.map(|reason| {
        let said = bounds(top + 6.0, reason.size());
        let cut = reason.elided;
        painter.galley(Pos2::new(middle, said.top()), reason, Color32::PLACEHOLDER);
        (said, cut)
    });
    if let Some((said, _)) = said {
        top = said.bottom();
    }
    let (said, cut) = said.unwrap_or((named, true));
    let reason = announce(
        ui,
        said,
        Id::new(("browser-message", pane, "detail")),
        detail,
    );
    // A reason cut short is read in full under the pointer.
    if cut && titled {
        reason.on_hover_text(detail);
    }
    if titled {
        top += gap;
    }
    Rect::from_min_size(
        Pos2::new(content.left(), top),
        vec2(content.width(), button),
    )
}

/// A line of a menu that says something and does nothing.
fn menu_note(ui: &mut Ui, p: Palette, text: &str) {
    let width = ui.available_width() - 20.0;
    let galley = ui.painter().layout(
        text.to_owned(),
        theme::regular(11.5),
        p.muted,
        width.max(40.0),
    );
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), galley.size().y + 12.0));
    ui.painter()
        .galley(rect.min + vec2(10.0, 6.0), galley, Color32::PLACEHOLDER);
    announce(ui, rect, ui.id().with(("menu-note", text)), text);
}

/// Where this tab keeps its cookies, and what can be done with them.
fn profile_menu(
    ui: &mut Ui,
    p: Palette,
    view: &View,
    config: &crate::config::Config,
    actions: &mut Vec<Action>,
    tool: &dyn Fn(Tool) -> Action,
) {
    use crate::config::BrowserProfile;
    if !view.saved {
        // Another Neptune holds the saved profiles, or this one was given none.
        menu_note(
            ui,
            p,
            "Private tab. Saved profiles are in use by another Neptune window.",
        );
        return;
    }
    let current = view.profile.as_deref();
    let listed = std::iter::once((Some(BrowserProfile::DEFAULT), "Default"))
        .chain(
            config
                .browser_profiles
                .iter()
                .map(|profile| (Some(profile.id.as_str()), profile.name.as_str())),
        )
        .chain(std::iter::once((None, "Private")));
    let label = format!("Profile: {}", config.browser_profile_name(current));
    menu_submenu(ui, p, Icon::Person, &label, |ui| {
        menu_layout(ui, 232.0);
        for (id, name) in listed {
            let icon = if id == current {
                Icon::Check
            } else if id.is_none() {
                Icon::Lock
            } else {
                Icon::Person
            };
            if menu_item(ui, p, icon, name, "", false) {
                actions.push(tool(Tool::Profile(id.map(str::to_owned))));
                ui.close();
            }
        }
        menu_separator(ui, p);
        if config.browser_profiles.len() < BrowserProfile::MAX
            && menu_item(ui, p, Icon::Plus, "New profile", "", false)
        {
            actions.push(tool(Tool::NewProfile));
            ui.close();
        }
        if config.browser_profile() != current.map(str::to_owned)
            && menu_item(ui, p, Icon::Star, "Use for new browser tabs", "", false)
        {
            actions.push(tool(Tool::DefaultProfile(current.map(str::to_owned))));
            ui.close();
        }
        if current.is_some() && menu_item(ui, p, Icon::Eraser, "Clear cookies", "", false) {
            actions.push(tool(Tool::ClearCookies));
            ui.close();
        }
        if current.is_some_and(|id| id != BrowserProfile::DEFAULT)
            && menu_item(ui, p, Icon::Trash, "Remove profile and its data", "", true)
        {
            actions.push(tool(Tool::RemoveProfile));
            ui.close();
        }
    });
    if current.is_none() {
        return;
    }
    menu_submenu(ui, p, Icon::ArrowDown, "Import cookies from", |ui| {
        menu_layout(ui, 264.0);
        let Some(sources) = &view.sources else {
            actions.push(tool(Tool::Sources));
            menu_note(ui, p, "Looking for browsers…");
            return;
        };
        if view.importing {
            menu_note(ui, p, "Importing cookies…");
            return;
        }
        if sources.is_empty() {
            menu_note(ui, p, "No other browser with cookies was found.");
            return;
        }
        for source in sources.iter() {
            for profile in &source.profiles {
                let mut label = if source.profiles.len() == 1 {
                    source.name.to_owned()
                } else {
                    format!("{}: {}", source.name, profile.name)
                };
                if let Some(cookies) = profile.cookies {
                    label.push_str(&format!(" ({cookies})"));
                }
                if menu_item(ui, p, Icon::Globe, &label, "", false) {
                    actions.push(tool(Tool::Import {
                        source: source.id.to_owned(),
                        directory: profile.directory.clone(),
                    }));
                    ui.close();
                }
            }
        }
        menu_note(
            ui,
            p,
            "Quit that browser first. Cookies are copied into this profile.",
        );
    });
}

pub(crate) fn show(
    ui: &mut Ui,
    rect: Rect,
    (pane, generation, view): (PaneId, u64, &View),
    state: &mut State,
    stage: &Stage,
    actions: &mut Vec<Action>,
    output: &mut StageOutput,
) {
    let p = stage.p;
    let keys = &stage.config.keybindings;
    let target = Target {
        pane: pane.get(),
        generation,
    };
    let selected = pane == stage.active;
    // The navigation bar continues the band of the tabs above it; the page
    // is a content surface that keeps the pane's lower corners.
    let toolbar = Rect::from_min_size(rect.min, vec2(rect.width(), BAR_HEIGHT.min(rect.height())));
    let content = Rect::from_min_max(egui::pos2(rect.left(), toolbar.bottom()), rect.max);
    let corners = CornerRadius {
        sw: metrics::PANE_RADIUS,
        se: metrics::PANE_RADIUS,
        ..CornerRadius::ZERO
    };
    ui.painter().rect_filled(content, corners, p.bg);
    let top = toolbar.center().y - CONTROL * 0.5;
    let mut x = toolbar.left() + 6.0;
    let mut control = |icon, label, hint: String, enabled, command| {
        let bounds = Rect::from_min_size(egui::pos2(x, top), Vec2::splat(CONTROL));
        x += CONTROL;
        if button(ui, bounds, (icon, label, hint.as_str()), enabled, p, pane).clicked() {
            actions.push(Action::Browser(pane, generation, command));
        }
    };
    control(
        Icon::ChevronLeft,
        "Back",
        keys.hint(Binding::BrowserBack),
        view.state.back,
        Command::Back { target },
    );
    control(
        Icon::ChevronRight,
        "Forward",
        keys.hint(Binding::BrowserForward),
        view.state.forward,
        Command::Forward { target },
    );
    control(
        if view.state.loading {
            Icon::Close
        } else {
            Icon::Refresh
        },
        if view.state.loading {
            "Stop loading"
        } else {
            "Reload page"
        },
        if view.state.loading {
            String::new()
        } else {
            keys.hint(Binding::BrowserReload)
        },
        view.state.ready && !view.failed,
        if view.state.loading {
            Command::Stop { target }
        } else {
            Command::Reload { target }
        },
    );
    let wide = rect.width() >= WIDE;
    let extras = 6.0 + if wide { CONTROL * 5.0 } else { CONTROL };
    let address = Rect::from_min_max(
        egui::pos2(x + 6.0, top),
        egui::pos2((toolbar.right() - extras - 6.0).max(x + 6.0), top + CONTROL),
    );
    let field_id = Id::new(("browser-address", pane));
    let focused = ui.memory(|m| m.focused() == Some(field_id));
    if state.observed != view.state.url && !focused {
        state.observed.clone_from(&view.state.url);
        state.address = if view.state.url == "about:blank" {
            String::new()
        } else {
            view.state.url.clone()
        };
    }
    let blank = view.state.url.is_empty() || view.state.url == "about:blank";
    if address.width() > 8.0 {
        address_surface(ui.painter(), address, p, focused);
        // The field leads with where the page is from, while it has room.
        let marked = address.width() >= 120.0;
        // The lock is a claim about the page in view: one that has arrived
        // over https and stands without an error. An address alone, a page
        // on its way or one that failed shows the globe.
        let secure = view.state.ready
            && !view.state.loading
            && !view.failed
            && view.state.error.is_none()
            && view.state.url.starts_with("https://");
        if marked {
            icons::paint(
                ui.painter(),
                Rect::from_center_size(
                    Pos2::new(address.left() + 16.0, address.center().y),
                    Vec2::splat(12.0),
                ),
                if focused || blank {
                    Icon::Search
                } else if secure {
                    Icon::Lock
                } else {
                    Icon::Globe
                },
                if focused { p.secondary } else { p.muted },
            );
        }
        // A page on its way shows it at the field's trailing edge.
        let loading = view.state.loading && address.width() >= 160.0;
        if loading {
            egui::Spinner::new().size(12.0).color(p.muted).paint_at(
                ui,
                Rect::from_center_size(
                    Pos2::new(address.right() - 15.0, address.center().y),
                    Vec2::splat(12.0),
                ),
            );
        }
        let editor = Rect::from_min_max(
            Pos2::new(
                address.left() + if marked { 29.0 } else { 11.0 },
                address.top(),
            ),
            Pos2::new(
                address.right() - if loading { 28.0 } else { 11.0 },
                address.bottom(),
            ),
        );
        ui.scope_builder(
            UiBuilder::new()
                .id_salt((pane, "browser-navigation"))
                .max_rect(editor)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                // Address hints stay readable without changing the app's fields.
                ui.visuals_mut().weak_text_color = Some(p.secondary);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut state.address)
                        .id(field_id)
                        .hint_text(if editor.width() >= 190.0 {
                            "Enter a URL or localhost:3000"
                        } else {
                            "Address"
                        })
                        .font(theme::regular(12.5))
                        .frame(Frame::NONE)
                        .background_color(Color32::TRANSPARENT)
                        .margin(Margin::ZERO)
                        .desired_width(editor.width())
                        .char_limit(crate::runtime::browser::protocol::MAX_URL),
                );
                response.widget_info(|| {
                    WidgetInfo::labeled(WidgetType::TextEdit, true, "Browser address")
                });
                if state.focus_address {
                    state.focus_address = false;
                    response.request_focus();
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    // Submission belongs to the address field even though it
                    // releases focus in this frame. Never submit a page form too.
                    ui.input_mut(|i| {
                        i.events.retain(|e| {
                            !matches!(
                                e,
                                egui::Event::Key {
                                    key: egui::Key::Enter,
                                    ..
                                }
                            )
                        })
                    });
                    actions.push(Action::NavigateBrowser(
                        pane,
                        generation,
                        state.address.clone(),
                    ));
                }
            },
        );
    }
    let external = !blank;
    let inspectable = view.state.ready && !view.failed;
    let inspect_hint = keys.hint(Binding::BrowserDevTools);
    let open_external = |actions: &mut Vec<Action>| {
        if let Some(link) = crate::platform::links::WebLink::new(&view.state.url) {
            actions.push(Action::OpenLink(link));
        }
    };
    let trailing = |index: f32| {
        Rect::from_min_size(
            egui::pos2(toolbar.right() - 6.0 - CONTROL * index, top),
            Vec2::splat(CONTROL),
        )
    };
    let tool = |tool| Action::BrowserTool(pane, generation, tool);
    let (picking, recording) = (view.state.picking, view.state.recording);
    let pick = if picking {
        (Icon::Close, "Cancel picking")
    } else {
        (Icon::Pointer, "Pick an element")
    };
    let record = if recording {
        (Icon::Stop, "Stop recording")
    } else {
        (Icon::Record, "Record the page")
    };
    // A page that failed or has yet to load has nothing to pick or record.
    let page = inspectable && !blank && view.state.error.is_none();
    if wide {
        if button(
            ui,
            trailing(5.0),
            (Icon::ArrowUpRight, "Open in external browser", ""),
            external,
            p,
            pane,
        )
        .clicked()
        {
            open_external(actions);
        }
        if button(
            ui,
            trailing(4.0),
            (Icon::Code, "Developer tools", inspect_hint.as_str()),
            inspectable,
            p,
            pane,
        )
        .clicked()
        {
            actions.push(Action::Browser(
                pane,
                generation,
                Command::DevTools { target },
            ));
        }
        if button(
            ui,
            trailing(3.0),
            (pick.0, pick.1, ""),
            page || picking,
            p,
            pane,
        )
        .clicked()
        {
            actions.push(tool(Tool::Pick));
        }
        let bounds = trailing(2.0);
        if button(
            ui,
            bounds,
            (record.0, record.1, ""),
            page || recording,
            p,
            pane,
        )
        .clicked()
        {
            actions.push(tool(Tool::Record));
        }
        if recording {
            ui.painter()
                .circle_filled(bounds.right_top() + vec2(-7.0, 7.0), 3.0, p.red);
        }
    }
    if toolbar.width() >= CONTROL * 4.0 + 12.0 {
        // A narrow pane lists every action here, and only those it can take.
        let more = button(
            ui,
            trailing(1.0),
            (Icon::Ellipsis, "More browser actions", ""),
            view.state.ready && !view.failed,
            p,
            pane,
        );
        if recording && !wide {
            ui.painter()
                .circle_filled(more.rect.right_top() + vec2(-7.0, 7.0), 3.0, p.red);
        }
        egui::Popup::menu(&more).show(|ui| {
            menu_layout(ui, 248.0);
            if !wide {
                if external
                    && menu_item(
                        ui,
                        p,
                        Icon::ArrowUpRight,
                        "Open in external browser",
                        "",
                        false,
                    )
                {
                    open_external(actions);
                    ui.close();
                }
                if inspectable
                    && menu_item(ui, p, Icon::Code, "Developer tools", &inspect_hint, false)
                {
                    actions.push(Action::Browser(
                        pane,
                        generation,
                        Command::DevTools { target },
                    ));
                    ui.close();
                }
                if (page || picking) && menu_item(ui, p, pick.0, pick.1, "", false) {
                    actions.push(tool(Tool::Pick));
                    ui.close();
                }
                if (page || recording) && menu_item(ui, p, record.0, record.1, "", false) {
                    actions.push(tool(Tool::Record));
                    ui.close();
                }
                menu_separator(ui, p);
            }
            profile_menu(ui, p, view, stage.config, actions, &tool);
        });
    }
    let response = ui.interact(
        content,
        ui.id().with(("browser-content", pane)),
        Sense::click_and_drag(),
    );
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Other,
            true,
            format!("Browser pane {}", pane.get()),
        )
    });
    if response.clicked() || response.drag_started() {
        actions.push(Action::Focus(pane));
        response.request_focus();
    }
    // The page says what the pointer is over: a link's hand, a field's
    // bar, the edge of something that resizes.
    let over = response.hovered() && view.state.error.is_none() && view.texture.is_some();
    if over {
        ui.ctx().set_cursor_icon(cursor(view.state.cursor));
        if let Some(tooltip) = &view.state.tooltip {
            response.clone().on_hover_text_at_pointer(tooltip);
        }
    }
    if selected && stage.keyboard {
        let focused = ui.memory(|m| m.focused());
        if focused != Some(response.id) && (focused.is_none() || focused == stage.previous_terminal)
        {
            response.request_focus();
            ui.ctx().request_repaint();
        }
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
    } else if response.has_focus() {
        response.surrender_focus();
    }
    if response.has_focus() && selected && stage.keyboard {
        let cursor = view.state.caret.map_or_else(
            || Rect::from_min_size(content.min, vec2(1.0, 20.0)),
            |[x, y, w, h]| {
                Rect::from_min_size(
                    content.min + vec2(x as f32, y as f32),
                    vec2(w as f32, h as f32),
                )
            },
        );
        ui.ctx().output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                rect: content,
                cursor_rect: cursor.intersect(content),
                purpose: egui::IMEPurpose::Normal,
                should_interrupt_composition: false,
            })
        });
    }
    let painter = ui
        .painter()
        .with_clip_rect(content.intersect(ui.clip_rect()));
    if let Some(texture) = view.texture {
        painter.add(
            egui::epaint::RectShape::filled(content, corners, egui::Color32::WHITE).with_texture(
                texture,
                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            ),
        );
    }
    if let Some((texture, popup)) = view.popup {
        painter.image(
            texture,
            popup.translate(content.min.to_vec2()),
            Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    if let Some(link) = view.state.link.as_ref().filter(|_| over) {
        // Where a link leads, at the foot of the page as browsers say it.
        let room = (content.width() * 0.7 - 20.0).max(40.0);
        let galley = painter.layout_job(egui::text::LayoutJob {
            wrap: egui::text::TextWrapping::truncate_at_width(room),
            ..egui::text::LayoutJob::simple_singleline(
                link.clone(),
                egui::FontId::proportional(11.5),
                p.secondary,
            )
        });
        let chip = Rect::from_min_size(
            Pos2::new(content.left(), content.bottom() - galley.size().y - 8.0),
            galley.size() + vec2(16.0, 8.0),
        );
        painter.rect(
            chip,
            egui::CornerRadius {
                ne: 6,
                ..Default::default()
            },
            p.elevated,
            egui::Stroke::new(1.0, p.border),
            egui::StrokeKind::Inside,
        );
        painter.galley(chip.min + vec2(8.0, 4.0), galley, p.secondary);
    }
    if let Some(error) = &view.state.error {
        // What went wrong, in the pane's own surface, with the one way on.
        painter.rect_filled(content, corners, p.bg);
        let action = if view.failed {
            "Restart preview"
        } else {
            "Reload"
        };
        // The way on is always there: the words above it give way first.
        let room = placeholder(
            ui,
            content,
            (p, pane),
            (Icon::Warning, p.red),
            ("Preview unavailable", error.as_str()),
            true,
        );
        let clicked = ui
            .scope_builder(
                UiBuilder::new()
                    .id_salt((pane, "browser-error"))
                    .max_rect(room)
                    .layout(Layout::top_down(Align::Center)),
                |ui| {
                    ui.set_clip_rect(content.intersect(ui.clip_rect()));
                    helpers::button(ui, p, action, ButtonKind::Secondary).clicked()
                },
            )
            .inner;
        if clicked {
            actions.push(if view.failed {
                Action::Restart(pane)
            } else {
                Action::Browser(pane, generation, Command::Reload { target })
            });
        }
    } else if blank {
        painter.rect_filled(content, corners, p.bg);
        if view.state.ready {
            placeholder(
                ui,
                content,
                (p, pane),
                (Icon::Globe, p.accent),
                (
                    "Preview your project",
                    if view.unsandboxed {
                        "Enter an address above, or open a port your dev server is listening on. Pages run without Chromium's sandbox on this system."
                    } else {
                        "Enter an address above, or open a port your dev server is listening on."
                    },
                ),
                false,
            );
        } else {
            // As a terminal says it while its shell starts.
            let center = content.center();
            egui::Spinner::new().size(14.0).color(p.muted).paint_at(
                ui,
                Rect::from_center_size(center - vec2(58.0, 0.0), Vec2::splat(14.0)),
            );
            let said = painter.text(
                center - vec2(44.0, 0.0),
                Align2::LEFT_CENTER,
                "Starting browser…",
                theme::regular(12.5),
                p.secondary,
            );
            announce(
                ui,
                said,
                Id::new(("browser-message", pane, "starting")),
                "Starting browser…",
            );
        }
    }
    ui.painter().line_segment(
        [toolbar.left_bottom(), toolbar.right_bottom()],
        Stroke::new(1.0, p.separator),
    );
    output.browser_bodies.push((pane, generation, content));
    if selected {
        output.active_body = Some(content);
        output.active_terminal = Some(response.id);
    }
}
