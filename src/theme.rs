//! The visual system: materials, text weights, metrics and egui defaults.
//!
//! Chrome uses one quiet material; terminals sit on a darker (or, in Light, a
//! brighter) content surface. Depth comes from elevation and hairlines rather
//! than heavy borders, and the accent is reserved for focus and selection.
use crate::config::{Accent, Config, Theme};
use eframe::egui::{self, Color32, FontFamily, FontId, Shadow, Stroke};

pub fn color(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Font family names registered by `platform::fonts`.
pub const MEDIUM: &str = "Medium";
pub const SEMIBOLD: &str = "Semibold";

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Shared geometry. Radii are concentric: the window corner equals the pane
/// corner plus the gutter between them.
pub mod metrics {
    pub const WINDOW_RADIUS: u8 = 16;
    pub const GUTTER: f32 = 6.0;
    pub const PANE_RADIUS: u8 = 10;
    pub const TOOLBAR_HEIGHT: f32 = 44.0;
    pub const PANE_HEADER: f32 = 34.0;
    pub const SHEET_RADIUS: u8 = 14;
    pub const CONTROL_RADIUS: u8 = 8;
    pub const ROW_RADIUS: u8 = 8;
    pub const CONTROL_HEIGHT: f32 = 30.0;
    /// The sidebar yields to terminal content below this window width.
    pub const SIDEBAR_MIN_WINDOW: f32 = 820.0;
}

#[derive(Clone, Copy)]
pub struct Palette {
    /// Terminal content surface.
    pub bg: Color32,
    /// Window material shared by the toolbar and sidebar.
    pub chrome: Color32,
    /// Sheets, popovers, menus and floating bars.
    pub elevated: Color32,
    /// Resting fill of fields, tracks and grouped rows.
    pub control: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    /// Hairline between regions that share a material.
    pub separator: Color32,
    /// Outline of floating surfaces and editable controls.
    pub border: Color32,
    pub fg: Color32,
    pub terminal_fg: Color32,
    pub terminal_bold: Color32,
    pub cursor: Color32,
    pub cursor_text: Option<Color32>,
    pub selection_text: Option<Color32>,
    pub secondary: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub selection: Color32,
    pub green: Color32,
    pub yellow: Color32,
    pub red: Color32,
    /// Unread terminal alerts (the pane ring, the bell dot and unread counts)
    /// and agents waiting for a person.
    pub attention: Color32,
    pub scrim: Color32,
    pub shadow: Color32,
    pub dark: bool,
    pub ansi: [Color32; 16],
}

/// Mixes `top` over `base`; both are treated as opaque.
pub fn mix(base: Color32, top: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    Color32::from_rgb(
        channel(base.r(), top.r()),
        channel(base.g(), top.g()),
        channel(base.b(), top.b()),
    )
}

/// A translucent tint of an opaque colour.
pub fn tint(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

pub fn accent_color(accent: Accent, dark: bool) -> Color32 {
    let (night, day) = match accent {
        Accent::Blue => (0x0a84ff, 0x007aff),
        Accent::Indigo => (0x7d7aff, 0x5856d6),
        Accent::Purple => (0xbf5af2, 0xaf52de),
        Accent::Pink => (0xff4f79, 0xff2d55),
        Accent::Red => (0xff5a52, 0xff3b30),
        Accent::Orange => (0xff9f0a, 0xf08a00),
        Accent::Yellow => (0xffd60a, 0xe0a800),
        Accent::Green => (0x30d158, 0x28a745),
        Accent::Graphite => (0x98989f, 0x7c7c82),
    };
    color(if dark { night } else { day })
}

/// Stable, friendly identity colours for workspace tiles.
pub fn identity_color(seed: u64, dark: bool) -> Color32 {
    const ORDER: [Accent; 8] = [
        Accent::Blue,
        Accent::Purple,
        Accent::Green,
        Accent::Orange,
        Accent::Pink,
        Accent::Indigo,
        Accent::Yellow,
        Accent::Red,
    ];
    // The first workspace has identity one.
    accent_color(
        ORDER[(seed.saturating_sub(1) % ORDER.len() as u64) as usize],
        dark,
    )
}

fn luminance(color: Color32) -> f32 {
    0.299 * color.r() as f32 + 0.587 * color.g() as f32 + 0.114 * color.b() as f32
}

/// Relative luminance of opaque sRGB colors, used only for window readability.
fn relative_luminance(c: Color32) -> f32 {
    let linear = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
}
fn contrast(a: Color32, b: Color32) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn readable(ink: Color32, surfaces: &[Color32], minimum: f32) -> Color32 {
    let score = |ink| {
        surfaces
            .iter()
            .map(|&surface| contrast(ink, surface))
            .fold(f32::INFINITY, f32::min)
    };
    if score(ink) >= minimum {
        return ink;
    }
    let target = if score(Color32::WHITE) > score(Color32::BLACK) {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    for step in 1..=20 {
        let adjusted = mix(ink, target, step as f32 / 20.0);
        if score(adjusted) >= minimum {
            return adjusted;
        }
    }
    target
}

impl Palette {
    /// The theme with the default accent (custom themes require their config).
    pub fn new(theme: Theme) -> Self {
        Self::for_config(&Config {
            theme,
            ..Config::default()
        })
    }

    pub fn for_config(config: &Config) -> Self {
        config
            .theme_colors()
            .map(Self::from_colors)
            .unwrap_or_else(|| Self::with_accent(&config.theme, config.accent))
    }

    /// Derive all window materials from the same palette used by the terminal.
    /// Terminal colors stay exact; chrome text and accents get a contrast floor.
    pub fn from_colors(colors: crate::terminal_theme::ThemeColors) -> Self {
        let dark = colors.is_dark();
        let mut p = Self::with_accent(
            if dark {
                &Theme::Graphite
            } else {
                &Theme::Light
            },
            Accent::default(),
        );
        p.bg = color(colors.background.0);
        p.terminal_fg = color(colors.foreground.0);
        p.terminal_bold = color(colors.bold.0);
        p.cursor = color(colors.cursor.0);
        p.cursor_text = Some(color(colors.cursor_text.0));
        p.selection = color(colors.selection.0);
        p.selection_text = Some(color(colors.selection_text.0));
        p.ansi = colors.ansi.map(|c| color(c.0));
        let lift = if dark { Color32::WHITE } else { Color32::BLACK };
        let anchor = if contrast(p.bg, Color32::WHITE) > contrast(p.bg, Color32::BLACK) {
            Color32::WHITE
        } else {
            Color32::BLACK
        };
        let material = |amount| {
            let surface = mix(p.bg, lift, amount);
            if contrast(anchor, surface) >= 4.5 {
                surface
            } else {
                p.bg
            }
        };
        p.chrome = material(if dark { 0.055 } else { 0.035 });
        p.elevated = material(if dark { 0.095 } else { 0.015 });
        let surfaces = [p.bg, p.chrome, p.elevated];
        p.fg = readable(p.terminal_fg, &surfaces, 4.5);
        p.secondary = readable(mix(p.chrome, p.fg, 0.78), &surfaces, 4.5);
        p.muted = readable(mix(p.chrome, p.fg, 0.60), &surfaces, 4.5);
        p.accent = readable(p.ansi[4], &surfaces, 3.0);
        p.on_accent = readable(p.bg, &[p.accent], 4.5);
        p.green = readable(p.ansi[2], &surfaces, 4.5);
        p.yellow = readable(p.ansi[3], &surfaces, 4.5);
        p.red = readable(p.ansi[1], &surfaces, 4.5);
        // Attention takes the palette's yellow, unless that is also its focus
        // colour; then it keeps the amber of the original themes.
        let amber = p.attention;
        p.attention = readable(p.ansi[3], &surfaces, 3.0);
        if p.attention == p.accent {
            p.attention = readable(amber, &surfaces, 3.0);
        }
        p
    }

    pub fn with_accent(theme: &Theme, accent: Accent) -> Self {
        let dark = theme != &Theme::Light;
        // Attention is amber, and stays apart from an amber focus accent.
        let attention = accent_color(
            if accent == Accent::Orange {
                Accent::Yellow
            } else {
                Accent::Orange
            },
            dark,
        );
        let accent = accent_color(accent, dark);
        let on_accent = if luminance(accent) > 170.0 {
            color(0x1d1d1f)
        } else {
            Color32::WHITE
        };
        let white = Color32::from_white_alpha;
        let black = Color32::from_black_alpha;
        let mut p = Self {
            bg: color(0x101012),
            chrome: color(0x1c1c1f),
            elevated: color(0x29292d),
            control: white(16),
            hover: white(15),
            pressed: white(28),
            separator: white(20),
            border: white(30),
            fg: color(0xececf1),
            terminal_fg: color(0xececf1),
            terminal_bold: color(0xececf1),
            cursor: accent,
            cursor_text: None,
            selection_text: None,
            secondary: color(0xa0a0a8),
            muted: color(0x6d6d76),
            accent,
            on_accent,
            selection: Color32::TRANSPARENT,
            green: color(0x30d158),
            yellow: color(0xffd60a),
            red: color(0xff5a52),
            attention,
            scrim: black(120),
            shadow: black(110),
            dark,
            ansi: [
                0x2c2c31, 0xff6b63, 0x5fd68b, 0xffd166, 0x5aa9ff, 0xc792f6, 0x5fd4e8, 0xd6d6dd,
                0x6d6d76, 0xff8b84, 0x86e5a8, 0xffe08f, 0x86c1ff, 0xddb3ff, 0x8ce4f3, 0xf5f5f7,
            ]
            .map(color),
        };
        match theme {
            Theme::Graphite | Theme::Palette(_) => {}
            Theme::Dusk => {
                p.bg = color(0x12111c);
                p.chrome = color(0x1e1c2b);
                p.elevated = color(0x2b2840);
                p.fg = color(0xe9e7f5);
                p.secondary = color(0xa29fb8);
                p.muted = color(0x6f6c87);
                p.ansi[0] = color(0x2f2c42);
                p.ansi[8] = color(0x6f6c87);
            }
            Theme::Light => {
                p.bg = color(0xffffff);
                p.chrome = color(0xececef);
                p.elevated = color(0xffffff);
                p.control = black(13);
                p.hover = black(13);
                p.pressed = black(26);
                p.separator = black(22);
                p.border = black(34);
                p.fg = color(0x1d1d1f);
                p.secondary = color(0x5e5e66);
                p.muted = color(0x8e8e95);
                p.green = color(0x28a745);
                p.yellow = color(0xe0a800);
                p.red = color(0xe5372d);
                p.scrim = black(70);
                p.shadow = black(46);
                p.ansi = [
                    0x1d1d1f, 0xc9302a, 0x1f8a3d, 0x9a6700, 0x0a63d6, 0x9340c8, 0x0d7d92, 0x8e8e95,
                    0x6e6e75, 0xe0453e, 0x2aa24b, 0xb97b00, 0x1e7bf0, 0xab55e4, 0x1994aa, 0x1d1d1f,
                ]
                .map(color);
            }
        }
        // Opaque, so selected cells keep their exact colour under any glyph.
        p.terminal_fg = p.fg;
        p.terminal_bold = p.fg;
        p.selection = mix(p.bg, p.accent, if dark { 0.34 } else { 0.24 });
        p
    }

    /// Label ink on a filled destructive control: white, unless an imported
    /// palette's red is too light to carry it.
    pub fn on_red(&self) -> Color32 {
        if contrast(Color32::WHITE, self.red) >= 3.0 {
            Color32::WHITE
        } else {
            color(0x1d1d1f)
        }
    }

    pub fn hairline(&self) -> Stroke {
        Stroke::new(1.0, self.separator)
    }

    /// Soft elevation for sheets and the command palette.
    pub fn sheet_shadow(&self) -> Shadow {
        Shadow {
            offset: [0, 18],
            blur: 48,
            spread: 0,
            color: self.shadow,
        }
    }

    /// Lighter elevation for menus, tooltips and floating bars.
    pub fn popup_shadow(&self) -> Shadow {
        Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: self.shadow.gamma_multiply(0.7),
        }
    }
}

pub fn apply(ctx: &egui::Context, config: &Config) {
    let p = Palette::for_config(config);
    let mut style = egui::Style {
        visuals: if p.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        },
        ..Default::default()
    };
    let visuals = &mut style.visuals;
    visuals.panel_fill = p.chrome;
    visuals.window_fill = p.elevated;
    visuals.extreme_bg_color = p.control;
    visuals.override_text_color = Some(p.fg);
    visuals.selection.bg_fill = tint(p.accent, if p.dark { 0.42 } else { 0.28 });
    visuals.selection.stroke = Stroke::new(1.5, p.accent);
    visuals.window_stroke = Stroke::new(1.0, p.border);
    visuals.window_corner_radius = egui::CornerRadius::same(metrics::SHEET_RADIUS);
    visuals.menu_corner_radius = egui::CornerRadius::same(10);
    visuals.window_shadow = p.sheet_shadow();
    visuals.popup_shadow = p.popup_shadow();
    visuals.widgets.noninteractive.bg_fill = p.elevated;
    visuals.widgets.noninteractive.bg_stroke = p.hairline();
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.secondary);
    for w in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = egui::CornerRadius::same(metrics::CONTROL_RADIUS);
        w.bg_stroke = Stroke::NONE;
        w.expansion = 0.0;
    }
    visuals.widgets.inactive.bg_fill = p.control;
    visuals.widgets.inactive.weak_bg_fill = p.control;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.secondary);
    visuals.widgets.hovered.bg_fill = p.hover;
    visuals.widgets.hovered.weak_bg_fill = p.hover;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, p.fg);
    visuals.widgets.active.bg_fill = p.pressed;
    visuals.widgets.active.weak_bg_fill = p.pressed;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, p.fg);
    visuals.widgets.open.bg_fill = p.pressed;
    visuals.widgets.open.weak_bg_fill = p.pressed;
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, p.fg);
    visuals.text_cursor.stroke = Stroke::new(1.5, p.accent);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(16);
    style.spacing.menu_margin = egui::Margin::same(5);
    style.spacing.interact_size.y = 28.0;
    style.spacing.scroll.floating = true;
    style.spacing.scroll.bar_width = 6.0;
    style.interaction.tooltip_delay = 0.45;
    style.animation_time = 0.12;
    for (text_style, size) in [
        (egui::TextStyle::Body, 13.0),
        (egui::TextStyle::Button, 13.0),
        (egui::TextStyle::Small, 11.0),
        (egui::TextStyle::Heading, 17.0),
    ] {
        style
            .text_styles
            .insert(text_style, FontId::new(size, FontFamily::Proportional));
    }
    ctx.set_theme(if p.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.set_global_style(style);
}

pub use crate::platform::fonts::install as fonts;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_accent_keeps_readable_text_in_every_theme() {
        for theme in [Theme::Graphite, Theme::Dusk, Theme::Light] {
            for (accent, _) in Accent::ALL {
                let p = Palette::with_accent(&theme, accent);
                let contrast = (luminance(p.accent) - luminance(p.on_accent)).abs();
                assert!(contrast > 70.0, "{theme:?}/{accent:?} label contrast");
                // The original themes keep white labels on destructive controls.
                assert_eq!(p.on_red(), Color32::WHITE);
                // Selected terminal cells stay distinct from the surface.
                assert_ne!(p.selection, p.bg);
                assert_eq!(p.selection.a(), 255);
                // An unread ring is never mistaken for the focus ring.
                assert_ne!(p.attention, p.accent, "{theme:?}/{accent:?}");
            }
        }
    }

    #[test]
    fn every_imported_theme_drives_window_and_terminal_with_readable_chrome() {
        for theme in crate::terminal_theme::BUNDLED {
            let config = Config {
                theme: Theme::Palette(format!("iterm:{}", theme.name)),
                ..Config::default()
            };
            let p = Palette::for_config(&config);
            assert_eq!(p.bg, color(theme.colors.background.0));
            assert_eq!(p.terminal_fg, color(theme.colors.foreground.0));
            assert_eq!(p.ansi, theme.colors.ansi.map(|c| color(c.0)));
            for surface in [p.bg, p.chrome, p.elevated] {
                for ink in [p.fg, p.secondary, p.muted, p.red, p.green, p.yellow] {
                    assert!(
                        contrast(ink, surface) >= 4.5,
                        "{} text contrast",
                        theme.name
                    );
                }
                assert!(
                    contrast(p.accent, surface) >= 3.0,
                    "{} accent contrast",
                    theme.name
                );
            }
            assert!(
                contrast(p.on_accent, p.accent) >= 4.5,
                "{} button text",
                theme.name
            );
            for surface in [p.bg, p.chrome, p.elevated] {
                assert!(
                    contrast(p.attention, surface) >= 3.0,
                    "{} attention contrast",
                    theme.name
                );
            }
            assert!(
                contrast(p.on_red(), p.red) >= 3.0,
                "{} destructive button text",
                theme.name
            );
        }
    }

    #[test]
    fn custom_theme_keeps_exact_terminal_colors_and_repairs_chrome_contrast() {
        let mut colors = crate::terminal_theme::bundled("Dracula").unwrap().colors;
        colors.background = crate::terminal_theme::HexColor(0x777777);
        colors.foreground = colors.background;
        colors.ansi.fill(colors.background);
        let p = Palette::from_colors(colors);
        assert_eq!(p.bg, p.terminal_fg);
        for surface in [p.bg, p.chrome, p.elevated] {
            assert!(contrast(p.fg, surface) >= 4.5);
        }
    }

    #[test]
    fn workspace_identity_is_stable_and_distinct_for_neighbours() {
        assert_eq!(identity_color(1, true), identity_color(9, true));
        assert_ne!(identity_color(1, true), identity_color(2, true));
        // Unassigned identities never underflow.
        let _ = identity_color(0, false);
    }
}
