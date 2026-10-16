//! Official Tabler Icons 3.49.0, embedded as dedicated native font subsets.
//!
//! Source SVGs, exhaustive mappings, regeneration and MIT notices live in
//! assets/icons/tabler. No icon file is loaded or decoded during a frame.

use eframe::egui::{
    Align2, Color32, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Painter, Pos2,
    Rect, Response, Sense, Shape, StrokeKind, Ui, WidgetInfo, WidgetType, emath::TSTransform,
    epaint::TextShape, vec2,
};
use neptune_model::AgentKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Bell,
    Terminal,
    Plus,
    Close,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    Settings,
    Search,
    Sidebar,
    PanelRight,
    SplitVertical,
    SplitHorizontal,
    Folder,
    Grid,
    Keyboard,
    Check,
    ArrowUpRight,
    ArrowUp,
    ArrowDown,
    Minus,
    Maximize,
    Command,
    Sun,
    Moon,
    Copy,
    Code,
    Wrap,
    Ellipsis,
    Refresh,
    Pencil,
    Clipboard,
    Warning,
    Minimize,
    Eraser,
    TextSize,
    Swap,
    Globe,
    Star,
    Image,
    PullRequest,
    Merged,
    Comment,
    Branch,
    Agents,
    File,
    Files,
    FilePlus,
    FolderPlus,
    Trash,
    Paperclip,
    Pause,
    Play,
    Stop,
    Lock,
    /// The pointer that picks an element in a page.
    Pointer,
    Record,
    Person,
    /// The eight reactions a host has, each a small picture.
    ThumbUp,
    ThumbDown,
    Smile,
    Party,
    Unsure,
    Heart,
    Rocket,
    Eyes,
    /// The official ringed-planet glyph used for Neptune branding.
    Planet,
    GitHub,
    ChevronUp,
    ArrowRight,
    ExternalLink,
    ArrowLeft,
    Circle,
    CircleFilled,
    Loader2,
    CircleDashed,
    CircleX,
    CircleCheck,
    Square,
    Checkbox,
    /// Claude fallback: the pinned Tabler release has no Claude brand glyph.
    Sparkles,
    BrandOpenAi,
    MathPi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variant {
    Outline,
    Filled,
}

impl Variant {
    fn family(self) -> &'static str {
        match self {
            Self::Outline => "Tabler Outline",
            Self::Filled => "Tabler Filled",
        }
    }

    fn font(self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(self.family().into()))
    }
}

mod tabler {
    use super::{Icon, Variant};
    include!("../assets/icons/tabler/glyphs.rs");
}

/// Add dedicated icon families without changing the text families' fallbacks.
pub fn register_fonts(definitions: &mut FontDefinitions) {
    for (variant, bytes) in [
        (
            Variant::Outline,
            include_bytes!("../assets/icons/tabler/outline.ttf").as_slice(),
        ),
        (
            Variant::Filled,
            include_bytes!("../assets/icons/tabler/filled.ttf").as_slice(),
        ),
    ] {
        let family = variant.family();
        definitions
            .font_data
            .insert(family.into(), FontData::from_static(bytes).into());
        definitions
            .families
            .insert(FontFamily::Name(family.into()), vec![family.into()]);
    }
}

/// Paint an icon inside its square design grid. The caller owns the hit area.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    paint_rotated(painter, rect, icon, color, 0.0);
}

/// Paint a glyph rotated clockwise around the design-cell center, in radians.
/// Rotation does not change the caller's layout bounds or hit target.
pub fn paint_rotated(painter: &Painter, rect: Rect, icon: Icon, color: Color32, angle: f32) {
    let side = rect.width().min(rect.height());
    if !side.is_finite()
        || side <= 0.0
        || !angle.is_finite()
        || color.a() == 0
        || !painter.clip_rect().intersects(rect)
    {
        return;
    }
    let (variant, glyph) = tabler::mapping(icon);
    // Fractional/animated bounds reuse whole physical-pixel rasters. Mesh
    // scaling keeps movement smooth without growing the font atlas per frame.
    let pixels_per_point = painter.pixels_per_point();
    let raster_size = (side * pixels_per_point).round().max(1.0) / pixels_per_point;
    let galley = painter.layout_no_wrap(
        glyph.to_string(),
        variant.font(raster_size),
        Color32::PLACEHOLDER,
    );
    let scale = side / raster_size;
    // Preserve Tabler's full 24-point cell and optical offsets, not tight ink
    // centering. Rotate around that same cell for spinners and disclosure.
    let translation = rect.center().to_vec2() - scale * galley.rect.center().to_vec2();
    let shape = if scale == 1.0 {
        Shape::Text(
            TextShape::new(translation.to_pos2(), galley, color)
                .with_angle_and_anchor(angle, Align2::CENTER_CENTER),
        )
    } else {
        let mut shape = Shape::Text(
            TextShape::new(Pos2::ZERO, galley, color)
                .with_angle_and_anchor(angle, Align2::CENTER_CENTER),
        );
        shape.transform(TSTransform::new(translation, scale));
        shape
    };
    painter.add(shape);
}

/// Paint an official Tabler equivalent for a CLI agent, preserving its tint.
/// Gemini and Omp still return false and leave the caller's fallback unchanged.
pub fn paint_provider(painter: &Painter, rect: Rect, kind: AgentKind, ink: Color32) -> bool {
    let (icon, tint) = match kind {
        AgentKind::Claude => (Icon::Sparkles, Color32::from_rgb(0xd9, 0x77, 0x57)),
        AgentKind::Codex => (Icon::BrandOpenAi, ink),
        AgentKind::Opencode => (Icon::Terminal, ink),
        AgentKind::Pi => (Icon::MathPi, ink),
        AgentKind::Gemini | AgentKind::Omp => return false,
    };
    paint(painter, rect, icon, tint);
    true
}

/// A compact, keyboard-focusable icon button with a rounded hover surface.
pub fn button(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    button_with_hint(ui, icon, tooltip, "")
}

/// As [`button`], with a keyboard shortcut shown beside the tooltip. The
/// accessible name stays the plain label.
pub fn button_with_hint(ui: &mut Ui, icon: Icon, label: &str, shortcut: &str) -> Response {
    let (_, rect) = ui.allocate_space(vec2(28.0, 28.0));
    let response = ui.interact(rect, ui.id().with(("icon-button", label)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let surface = rect.shrink(1.0);
        if response.hovered() || response.is_pointer_button_down_on() {
            ui.painter().rect_filled(surface, 7, visuals.weak_bg_fill);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                surface,
                7,
                ui.visuals().selection.stroke,
                StrokeKind::Inside,
            );
        }
        // Pressing settles the glyph slightly, like a physical key.
        let inset = if response.is_pointer_button_down_on() {
            6.5
        } else {
            6.0
        };
        paint(
            ui.painter(),
            rect.shrink(inset),
            icon,
            visuals.fg_stroke.color,
        );
    }
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if shortcut.is_empty() {
        response.on_hover_text(label)
    } else {
        response.on_hover_text(format!("{label}   {shortcut}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{self, Align, Event, Layout, Modifiers, PointerButton, UiBuilder};

    #[test]
    fn every_tabler_icon_is_present_in_its_dedicated_font() {
        let ctx = egui::Context::default();
        crate::platform::fonts::install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for icon in tabler::ALL {
                let (variant, glyph) = tabler::mapping(icon);
                assert!(
                    ui.ctx()
                        .fonts_mut(|fonts| fonts.has_glyph(&variant.font(16.0), glyph)),
                    "{icon:?} is missing from {variant:?}"
                );
            }
        });
        output.textures_delta.clear();
    }

    #[test]
    fn all_icons_draw_at_small_large_and_fractional_sizes_on_both_display_scales() {
        for pixels_per_point in [1.0, 2.0] {
            let ctx = egui::Context::default();
            crate::platform::fonts::install(&ctx);
            ctx.set_pixels_per_point(pixels_per_point);
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let painter = ui.ctx().layer_painter(egui::LayerId::background());
                for icon in tabler::ALL {
                    for side in [8.0, 14.0, 16.0, 16.25, 24.0, 40.0] {
                        let rect = Rect::from_center_size(Pos2::new(64.0, 64.0), vec2(side, side));
                        paint(&painter, rect, icon, Color32::WHITE);
                    }
                }
            });
            output.textures_delta.clear();
            assert_eq!(
                output
                    .shapes
                    .iter()
                    .filter(|shape| matches!(shape.shape, Shape::Text(_)))
                    .count(),
                tabler::ALL.len() * 6,
            );
            let meshes = ctx.tessellate(output.shapes, output.pixels_per_point);
            assert!(meshes.iter().any(|mesh| {
                matches!(&mesh.primitive, egui::epaint::Primitive::Mesh(mesh) if !mesh.is_empty())
            }));
        }
    }

    #[test]
    fn rotating_glyphs_preserves_the_design_cell_center() {
        for pixels_per_point in [1.0, 2.0] {
            let ctx = egui::Context::default();
            crate::platform::fonts::install(&ctx);
            ctx.set_pixels_per_point(pixels_per_point);
            let center = Pos2::new(64.0, 64.0);
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let painter = ui.ctx().layer_painter(egui::LayerId::background());
                for side in [14.0, 16.25] {
                    for angle in [
                        0.0,
                        std::f32::consts::FRAC_PI_2,
                        std::f32::consts::TAU / 3.0,
                    ] {
                        paint_rotated(
                            &painter,
                            Rect::from_center_size(center, vec2(side, side)),
                            Icon::Loader2,
                            Color32::WHITE,
                            angle,
                        );
                    }
                }
            });
            output.textures_delta.clear();
            let mut checked = 0;
            for clipped in output.shapes {
                if let Shape::Text(shape) = clipped.shape {
                    let rotated_center = shape.pos
                        + eframe::egui::emath::Rot2::from_angle(shape.angle)
                            * shape.galley.rect.center().to_vec2();
                    assert!(rotated_center.distance(center) < 0.001);
                    checked += 1;
                }
            }
            assert_eq!(checked, 6);
        }
    }

    #[test]
    fn agents_with_a_mark_paint_it_and_the_rest_leave_the_tab_as_it_was() {
        let ctx = egui::Context::default();
        crate::platform::fonts::install(&ctx);
        let mut painted = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let rect = Rect::from_min_size(Pos2::ZERO, vec2(13.0, 13.0));
            painted = AgentKind::ALL
                .into_iter()
                .filter(|kind| paint_provider(ui.painter(), rect, *kind, Color32::WHITE))
                .collect();
        });
        output.textures_delta.clear();
        assert_eq!(
            painted,
            [
                AgentKind::Claude,
                AgentKind::Codex,
                AgentKind::Opencode,
                AgentKind::Pi
            ]
        );
    }

    fn title_controls_frame(ctx: &egui::Context, events: Vec<Event>) -> Response {
        let mut close = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1180.0, 760.0))),
                events,
                ..Default::default()
            },
            |ui| {
                let title = Rect::from_min_size(Pos2::ZERO, vec2(1180.0, 46.0));
                ui.interact(
                    title.shrink2(vec2(180.0, 0.0)),
                    ui.id().with("title-drag"),
                    Sense::click_and_drag(),
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(Pos2::new(112.0, 9.0), vec2(64.0, 28.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        button(ui, Icon::Sidebar, "Toggle sidebar");
                    },
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_max(
                            Pos2::new(title.right() - 164.0, 9.0),
                            title.max - vec2(9.0, 9.0),
                        ))
                        .layout(Layout::right_to_left(Align::Center)),
                    |ui| {
                        close = Some(button(ui, Icon::Close, "Close window"));
                        button(ui, Icon::Maximize, "Maximize");
                        button(ui, Icon::Minus, "Minimize");
                        ui.add_space(8.0);
                        button(ui, Icon::Command, "Commands");
                    },
                );
            },
        );
        output.textures_delta.clear();
        close.expect("titlebar close control")
    }

    #[test]
    fn rtl_titlebar_icon_retains_identity_and_clicks_on_release() {
        let ctx = egui::Context::default();
        crate::platform::fonts::install(&ctx);
        crate::theme::apply(&ctx, &crate::config::Config::default());
        let initial = title_controls_frame(&ctx, vec![]);
        let pos = initial.rect.center();
        assert_eq!(initial.rect.size(), vec2(28.0, 28.0));
        assert!(!initial.clicked());

        let hover = title_controls_frame(&ctx, vec![Event::PointerMoved(pos)]);
        assert_eq!(hover.id, initial.id);
        assert!(hover.hovered());
        assert!(!hover.clicked());

        let press = title_controls_frame(
            &ctx,
            vec![Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
        );
        assert_eq!(press.id, initial.id);
        assert!(press.is_pointer_button_down_on());
        assert!(!press.clicked());

        let release = title_controls_frame(
            &ctx,
            vec![Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert_eq!(release.id, initial.id);
        assert!(release.clicked());
        assert!(!release.is_pointer_button_down_on());
        assert!(!title_controls_frame(&ctx, vec![]).clicked());
    }

    fn action_controls_frame(
        ctx: &egui::Context,
        show_sidebar_control: bool,
        focus_find: bool,
        events: Vec<Event>,
    ) -> (Response, Response) {
        let mut actions = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0))),
                events,
                ..Default::default()
            },
            |ui| {
                ui.horizontal(|ui| {
                    if show_sidebar_control {
                        button(ui, Icon::Sidebar, "Toggle sidebar");
                    }
                    let find = button(ui, Icon::Search, "Find in terminal");
                    let close = button(ui, Icon::Close, "Close terminal");
                    if focus_find {
                        find.request_focus();
                    }
                    actions = Some((find, close));
                });
            },
        );
        output.textures_delta.clear();
        actions.expect("action controls")
    }

    #[test]
    fn hiding_neighboring_controls_preserves_focus_and_keyboard_activation() {
        let ctx = egui::Context::default();
        crate::platform::fonts::install(&ctx);
        let (initial_find, initial_close) = action_controls_frame(&ctx, true, true, vec![]);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(initial_find.id));

        // Responsive chrome can remove an earlier control. Positional auto IDs
        // would now assign Find's focused ID to Close and activate that action.
        let (find, close) = action_controls_frame(
            &ctx,
            false,
            false,
            vec![Event::Key {
                key: egui::Key::Enter,
                physical_key: Some(egui::Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );

        assert_eq!(find.id, initial_find.id);
        assert_eq!(close.id, initial_close.id);
        assert!(find.has_focus());
        assert!(
            find.clicked(),
            "Enter did not activate the focused Find control"
        );
        assert!(!close.has_focus());
        assert!(
            !close.clicked(),
            "Enter activated a different control after layout changed"
        );
    }
}
