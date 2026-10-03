//! Zenless shared theme system.
//!
//! This file is shared verbatim between Zenless Download Manager, Zenless Torrent
//! and the Zenless Installer. Keep the copies in sync.
//!
//! * Built-in themes plus user-made custom themes.
//! * Appearance (active theme, font scale, rounding, density) lives in
//!   `<config>/Zenless/appearance.json`, and custom themes in
//!   `<config>/Zenless/themes/*.json`, so every Zenless app shares them.
//! * Running apps pick up changes made by another Zenless app automatically
//!   (see [`ThemeManager::poll`]).

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2,
    Visuals,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

// ---------------------------------------------------------------------------
// Palette & theme
// ---------------------------------------------------------------------------

/// Every color a Zenless UI needs. Stored as `#rrggbb` strings on disk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    /// Window / panel background.
    #[serde(with = "hex")]
    pub bg: Color32,
    /// Cards, dialogs and raised surfaces.
    #[serde(with = "hex")]
    pub surface: Color32,
    /// Buttons and inputs at rest.
    #[serde(with = "hex")]
    pub surface2: Color32,
    /// Text inputs and wells.
    #[serde(with = "hex")]
    pub input: Color32,
    /// Subtle stripe for table rows.
    #[serde(with = "hex")]
    pub stripe: Color32,
    #[serde(with = "hex")]
    pub border: Color32,
    #[serde(with = "hex")]
    pub text: Color32,
    #[serde(with = "hex")]
    pub text_dim: Color32,
    #[serde(with = "hex")]
    pub accent: Color32,
    /// Text drawn on top of `accent`.
    #[serde(with = "hex")]
    pub accent_fg: Color32,
    /// Secondary accent, used for gradients and upload / seeding stats.
    #[serde(with = "hex")]
    pub accent2: Color32,
    #[serde(with = "hex")]
    pub success: Color32,
    #[serde(with = "hex")]
    pub warning: Color32,
    #[serde(with = "hex")]
    pub danger: Color32,
    #[serde(with = "hex")]
    pub info: Color32,
}

impl Palette {
    /// The labelled, editable fields (used by the theme editor).
    pub fn fields_mut(&mut self) -> [(&'static str, &mut Color32); 15] {
        [
            ("Background", &mut self.bg),
            ("Surface", &mut self.surface),
            ("Controls", &mut self.surface2),
            ("Inputs", &mut self.input),
            ("Row stripe", &mut self.stripe),
            ("Border", &mut self.border),
            ("Text", &mut self.text),
            ("Dim text", &mut self.text_dim),
            ("Accent", &mut self.accent),
            ("Text on accent", &mut self.accent_fg),
            ("Second accent", &mut self.accent2),
            ("Success", &mut self.success),
            ("Warning", &mut self.warning),
            ("Danger", &mut self.danger),
            ("Info", &mut self.info),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub palette: Palette,
    /// Built-in themes cannot be edited or deleted.
    #[serde(skip)]
    pub builtin: bool,
}

/// User appearance preferences shared by all Zenless apps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: String,
    /// Multiplies every font size (0.8 – 1.6).
    pub font_scale: f32,
    /// Corner radius of widgets in points (0 – 16).
    pub rounding: f32,
    /// Spacing multiplier (0.75 – 1.5). Lower is more compact.
    pub density: f32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: DEFAULT_THEME.to_owned(),
            font_scale: 1.0,
            rounding: 6.0,
            density: 1.0,
        }
    }
}

pub const DEFAULT_THEME: &str = "Zenless";

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[allow(clippy::too_many_arguments)]
fn theme(name: &str, dark: bool, p: [u32; 15]) -> Theme {
    Theme {
        name: name.to_owned(),
        dark,
        builtin: true,
        palette: Palette {
            bg: c(p[0]),
            surface: c(p[1]),
            surface2: c(p[2]),
            input: c(p[3]),
            stripe: c(p[4]),
            border: c(p[5]),
            text: c(p[6]),
            text_dim: c(p[7]),
            accent: c(p[8]),
            accent_fg: c(p[9]),
            accent2: c(p[10]),
            success: c(p[11]),
            warning: c(p[12]),
            danger: c(p[13]),
            info: c(p[14]),
        },
    }
}

/// All built-in themes, in display order.
pub fn builtin_themes() -> Vec<Theme> {
    //  bg        surface   controls  input     stripe    border    text      dim       accent    on-accent accent2   success   warning   danger    info
    vec![
        theme("Zenless", true, [0x0d0e12, 0x15171d, 0x1e2129, 0x0a0b0e, 0x121419, 0x2a2e38, 0xeceef3, 0x8a90a0, 0xd4ff3f, 0x0d0e12, 0x3fd8ff, 0x5ee29a, 0xffc53d, 0xff5c6c, 0x5aa9ff]),
        theme("Midnight", true, [0x0b1020, 0x121a2e, 0x1a2440, 0x080c18, 0x0f1628, 0x26325a, 0xe6ebff, 0x8791b5, 0x5b8cff, 0x0b1020, 0xa77bff, 0x4fd18b, 0xf5b945, 0xff5d73, 0x5bc0ff]),
        theme("AMOLED Purple", true, [0x000000, 0x0a0910, 0x15121f, 0x000000, 0x07060c, 0x2a2340, 0xf1ecff, 0x9d93bd, 0xb57bff, 0x0b0014, 0xff5fd2, 0x4ade80, 0xfbbf24, 0xff4d6d, 0x7aa7ff]),
        theme("AMOLED Mono", true, [0x000000, 0x0a0a0a, 0x161616, 0x000000, 0x070707, 0x2c2c2c, 0xf5f5f5, 0x9a9a9a, 0xf5f5f5, 0x000000, 0xa3a3a3, 0x4ade80, 0xfbbf24, 0xff4d6d, 0x7aa7ff]),
        theme("AMOLED Crimson", true, [0x000000, 0x0c0708, 0x1a0f11, 0x000000, 0x080506, 0x3d1d23, 0xfff0f2, 0xad9499, 0xff3d5e, 0x000000, 0xff9f43, 0x4ade80, 0xfbbf24, 0xff7a8c, 0x60a5fa]),
        theme("AMOLED Emerald", true, [0x000000, 0x060b09, 0x0f1a15, 0x000000, 0x040806, 0x1d3a2e, 0xecfff6, 0x8fb3a3, 0x34f5a4, 0x00140b, 0x22d3ee, 0x34f5a4, 0xfbbf24, 0xff4d6d, 0x60a5fa]),
        theme("Dracula", true, [0x21222c, 0x282a36, 0x343746, 0x1b1c24, 0x252631, 0x44475a, 0xf8f8f2, 0x9ea3c0, 0xbd93f9, 0x21222c, 0xff79c6, 0x50fa7b, 0xf1fa8c, 0xff5555, 0x8be9fd]),
        theme("Nord", true, [0x2e3440, 0x3b4252, 0x434c5e, 0x272c36, 0x353c4a, 0x4c566a, 0xeceff4, 0xa7b1c2, 0x88c0d0, 0x2e3440, 0x81a1c1, 0xa3be8c, 0xebcb8b, 0xbf616a, 0x5e81ac]),
        theme("Tokyo Night", true, [0x16161e, 0x1a1b26, 0x24283b, 0x111118, 0x1c1d29, 0x2f334d, 0xc0caf5, 0x7a82a8, 0x7aa2f7, 0x16161e, 0xbb9af7, 0x9ece6a, 0xe0af68, 0xf7768e, 0x7dcfff]),
        theme("Catppuccin Mocha", true, [0x181825, 0x1e1e2e, 0x313244, 0x11111b, 0x1c1c2b, 0x45475a, 0xcdd6f4, 0x9399b2, 0xcba6f7, 0x1e1e2e, 0xf5c2e7, 0xa6e3a1, 0xf9e2af, 0xf38ba8, 0x89b4fa]),
        theme("Gruvbox", true, [0x1d2021, 0x282828, 0x3c3836, 0x161819, 0x242424, 0x504945, 0xebdbb2, 0xa89984, 0xfe8019, 0x1d2021, 0xfabd2f, 0xb8bb26, 0xfabd2f, 0xfb4934, 0x83a598]),
        theme("Rosé Pine", true, [0x191724, 0x1f1d2e, 0x26233a, 0x13111e, 0x1c1a29, 0x403d52, 0xe0def4, 0x908caa, 0xebbcba, 0x191724, 0xc4a7e7, 0x9ccfd8, 0xf6c177, 0xeb6f92, 0x31748f]),
        theme("One Dark", true, [0x21252b, 0x282c34, 0x30353f, 0x1b1f24, 0x252930, 0x3e4451, 0xdcdfe4, 0x939aa7, 0x61afef, 0x1b1f24, 0xc678dd, 0x98c379, 0xe5c07b, 0xe06c75, 0x56b6c2]),
        theme("Monokai Pro", true, [0x221f22, 0x2d2a2e, 0x403e41, 0x19181a, 0x282528, 0x5b595c, 0xfcfcfa, 0xa19fa1, 0xffd866, 0x221f22, 0xff6188, 0xa9dc76, 0xfc9867, 0xff4f6e, 0x78dce8]),
        theme("Everforest", true, [0x272e33, 0x2e383c, 0x374145, 0x1e2326, 0x2b3337, 0x495156, 0xd3c6aa, 0x9da9a0, 0xa7c080, 0x1e2326, 0xdbbc7f, 0x83c092, 0xdbbc7f, 0xe67e80, 0x7fbbb3]),
        theme("Kanagawa", true, [0x16161d, 0x1f1f28, 0x2a2a37, 0x121218, 0x1a1a22, 0x363646, 0xdcd7ba, 0xa39e8c, 0x7e9cd8, 0x16161d, 0x957fb8, 0x98bb6c, 0xe6c384, 0xe46876, 0x7fb4ca]),
        theme("Ayu Dark", true, [0x0b0e14, 0x0f131a, 0x1a1f29, 0x07090d, 0x0d1017, 0x273040, 0xd9d7ce, 0x8f949d, 0xe6b450, 0x0b0e14, 0x59c2ff, 0xaad94c, 0xffb454, 0xf07178, 0x39bae6]),
        theme("Night Owl", true, [0x011627, 0x0b2942, 0x13344f, 0x01111d, 0x041d33, 0x1d3b53, 0xd6deeb, 0x8aa1b6, 0x82aaff, 0x011627, 0xc792ea, 0xaddb67, 0xecc48d, 0xef5350, 0x7fdbca]),
        theme("GitHub Dark", true, [0x0d1117, 0x161b22, 0x21262d, 0x010409, 0x11161d, 0x30363d, 0xe6edf3, 0x939ca6, 0x1f6feb, 0xffffff, 0xa371f7, 0x3fb950, 0xd29922, 0xf85149, 0x58a6ff]),
        theme("Solarized Dark", true, [0x002b36, 0x073642, 0x0d4250, 0x00212b, 0x03303b, 0x1e5160, 0xeee8d5, 0x93a1a1, 0x3a9ce3, 0x002b36, 0xd33682, 0x859900, 0xb58900, 0xdc322f, 0x2aa198]),
        theme("Synthwave", true, [0x1f1a2e, 0x262335, 0x34294f, 0x171321, 0x221c33, 0x4a3b6b, 0xf8f4ff, 0xa597c6, 0xff7edb, 0x1a0f24, 0x36f9f6, 0x72f1b8, 0xfede5d, 0xfe4450, 0x03edf9]),
        theme("Neon Cyber", true, [0x07070d, 0x0e0e18, 0x171726, 0x040408, 0x0b0b14, 0x2a2a45, 0xf2f3ff, 0x8b8db0, 0x00f0ff, 0x07070d, 0xff2bd6, 0x39ff88, 0xffe14d, 0xff3860, 0x7c7cff]),
        theme("Night City", true, [0x0a0a0c, 0x121216, 0x1c1c22, 0x050507, 0x0f0f13, 0x2f2f3a, 0xf5f5f0, 0x9e9e94, 0xfcee0a, 0x0a0a0c, 0x00f0ff, 0x39ff14, 0xff9f1c, 0xff2a6d, 0x05d9e8]),
        theme("Matrix", true, [0x000000, 0x030a04, 0x08160a, 0x000000, 0x020803, 0x0f3a14, 0xc8ffcf, 0x64ad6f, 0x00ff41, 0x000000, 0x39ffb0, 0x00ff41, 0xe6ff3f, 0xff3355, 0x3fd8ff]),
        theme("Forest", true, [0x121a16, 0x18221d, 0x223029, 0x0d1410, 0x151e19, 0x2e3f36, 0xe3efe7, 0x8ea596, 0x7fd18b, 0x0f1a13, 0xd6c26b, 0x7fd18b, 0xe8b85c, 0xe8716c, 0x6cb8d6]),
        theme("Ocean", true, [0x06141b, 0x0b1f29, 0x112b38, 0x040f15, 0x081923, 0x1d3d4d, 0xdff6ff, 0x8badbc, 0x2dd4bf, 0x04201c, 0x60a5fa, 0x4ade80, 0xfbbf24, 0xfb7185, 0x38bdf8]),
        theme("Sunset", true, [0x1a1016, 0x23151d, 0x2f1c27, 0x140c11, 0x1e1219, 0x4a2c3b, 0xffece6, 0xb8989e, 0xff8a5b, 0x1a1016, 0xff4f8b, 0x7ee0a1, 0xffd166, 0xff5c6c, 0x7cc6fe]),
        theme("Espresso", true, [0x1b1512, 0x241c18, 0x30251f, 0x15100d, 0x1f1814, 0x47372e, 0xf3e9e1, 0xac9a8e, 0xd4a373, 0x1b1512, 0xe9c46a, 0x8fbf6d, 0xe9c46a, 0xe76f51, 0x7fb7be]),
        theme("High Contrast", true, [0x000000, 0x0a0a0a, 0x1a1a1a, 0x000000, 0x111111, 0xffffff, 0xffffff, 0xd0d0d0, 0xffe600, 0x000000, 0x00e5ff, 0x00ff66, 0xffaa00, 0xff3355, 0x33aaff]),
        theme("Paper", false, [0xf6f6f4, 0xffffff, 0xececea, 0xffffff, 0xf1f1ef, 0xdadad6, 0x1c1d21, 0x6b6e76, 0x2f6feb, 0xffffff, 0x8b5cf6, 0x16a34a, 0xd97706, 0xdc2626, 0x0284c7]),
        theme("Solarized Light", false, [0xfdf6e3, 0xeee8d5, 0xe4ddc8, 0xfffbef, 0xf5efdc, 0xd3cbb4, 0x3b4a50, 0x56666a, 0x1a6fb0, 0xffffff, 0xd33682, 0x6c7d00, 0xb58900, 0xdc322f, 0x2aa198]),
        theme("Catppuccin Latte", false, [0xeff1f5, 0xf8f9fb, 0xe6e9ef, 0xffffff, 0xebeef3, 0xccd0da, 0x4c4f69, 0x5c5f77, 0x8839ef, 0xffffff, 0xea76cb, 0x2f7d1f, 0xdf8e1d, 0xd20f39, 0x1e66f5]),
        theme("Lavender", false, [0xf5f3ff, 0xffffff, 0xebe7fb, 0xffffff, 0xf0edfd, 0xd9d2f5, 0x1f1638, 0x635b88, 0x7c3aed, 0xffffff, 0xdb2777, 0x16a34a, 0xd97706, 0xdc2626, 0x2563eb]),
        theme("Sakura", false, [0xfff6f8, 0xffffff, 0xfbe9ee, 0xffffff, 0xfdf0f3, 0xf3d3dc, 0x3a1f2a, 0x7f5866, 0xd6336c, 0xffffff, 0x7c3aed, 0x2f9e44, 0xe67700, 0xc92a2a, 0x1c7ed6]),
    ]
}

// ---------------------------------------------------------------------------
// Applying a theme to egui
// ---------------------------------------------------------------------------

/// Mixes two colors in gamma space (`t = 0` → `a`, `t = 1` → `b`).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

/// Same color with a new alpha (0 – 1).
pub fn alpha(color: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = color.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn radius(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

/// Builds egui [`Visuals`] for a theme.
pub fn visuals(theme: &Theme, appearance: &Appearance) -> Visuals {
    let p = &theme.palette;
    let r = radius(appearance.rounding);
    let mut v = if theme.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    v.dark_mode = theme.dark;
    v.override_text_color = None;
    v.weak_text_color = Some(p.text_dim);
    v.hyperlink_color = p.accent;
    v.faint_bg_color = p.stripe;
    v.extreme_bg_color = p.input;
    v.text_edit_bg_color = Some(p.input);
    v.code_bg_color = p.surface2;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = radius(appearance.rounding * 1.6 + 2.0);
    v.menu_corner_radius = r;
    let shadow_color = if theme.dark {
        Color32::from_black_alpha(140)
    } else {
        Color32::from_black_alpha(40)
    };
    v.window_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: shadow_color,
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 16,
        spread: 0,
        color: shadow_color,
    };
    v.selection.bg_fill = alpha(p.accent, if theme.dark { 0.35 } else { 0.28 });
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.striped = true;
    v.slider_trailing_fill = true;
    v.indent_has_left_vline = true;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = p.surface2;
    w.inactive.weak_bg_fill = p.surface2;
    w.inactive.bg_stroke = Stroke::new(1.0, mix(p.surface2, p.border, 0.6));
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = mix(p.surface2, p.accent, 0.16);
    w.hovered.weak_bg_fill = mix(p.surface2, p.accent, 0.16);
    w.hovered.bg_stroke = Stroke::new(1.0, alpha(p.accent, 0.7));
    w.hovered.fg_stroke = Stroke::new(1.5, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 1.0;

    w.active.bg_fill = mix(p.surface2, p.accent, 0.32);
    w.active.weak_bg_fill = mix(p.surface2, p.accent, 0.32);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(2.0, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 1.0;

    w.open.bg_fill = mix(p.surface2, p.accent, 0.1);
    w.open.weak_bg_fill = mix(p.surface2, p.accent, 0.1);
    w.open.bg_stroke = Stroke::new(1.0, p.border);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r;

    v
}

/// Applies a theme + appearance to every egui style (dark and light).
pub fn apply(ctx: &egui::Context, theme: &Theme, appearance: &Appearance) {
    let visuals = visuals(theme, appearance);
    let fs = appearance.font_scale.clamp(0.7, 2.0);
    let d = appearance.density.clamp(0.6, 1.8);

    let mut style = (*ctx.global_style()).clone();
    style.visuals = visuals;
    style.text_styles = [
        (TextStyle::Heading, FontId::new(20.0 * fs, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0 * fs, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0 * fs, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.5 * fs, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0 * fs, FontFamily::Monospace)),
    ]
    .into();
    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 6.0) * d;
    s.button_padding = Vec2::new(10.0, 5.0) * d;
    s.interact_size = Vec2::new(40.0, 24.0 * d.max(0.85));
    s.window_margin = Margin::same((14.0 * d) as i8);
    s.menu_margin = Margin::same((8.0 * d) as i8);
    s.indent = 18.0 * d;
    s.scroll.bar_width = 8.0;
    s.scroll.floating = true;

    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
    ctx.set_theme(if theme.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.request_repaint();
}

/// Registers the Phosphor icon font as a fallback so `egui_phosphor::regular::*`
/// glyphs render inside any text. Call once at startup.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// `<config dir>/Zenless` (e.g. `%APPDATA%\Zenless` on Windows).
pub fn zenless_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Zenless")
}

pub fn appearance_path() -> PathBuf {
    zenless_config_dir().join("appearance.json")
}

pub fn themes_dir() -> PathBuf {
    zenless_config_dir().join("themes")
}

fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "theme".to_owned() } else { s }
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

pub fn load_appearance() -> Appearance {
    std::fs::read(appearance_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_appearance(a: &Appearance) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(a).map_err(std::io::Error::other)?;
    write_atomic(&appearance_path(), &json)
}

pub fn load_custom_themes() -> Vec<Theme> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(themes_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(t) = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Theme>(&b).ok())
            {
                out.push(t);
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

pub fn save_custom_theme(theme: &Theme) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(theme).map_err(std::io::Error::other)?;
    write_atomic(&themes_dir().join(format!("{}.json", slug(&theme.name))), &json)
}

pub fn delete_custom_theme(name: &str) -> std::io::Result<()> {
    std::fs::remove_file(themes_dir().join(format!("{}.json", slug(name))))
}

mod hex {
    use eframe::egui::Color32;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(c: &Color32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(*c))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color32, D::Error> {
        let s = String::deserialize(d)?;
        super::parse_hex(&s).ok_or_else(|| D::Error::custom(format!("invalid color {s:?}")))
    }
}

pub fn to_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

pub fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).ok()?;
    match s.len() {
        6 => Some(c(v)),
        3 => {
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some(Color32::from_rgb((r * 17) as u8, (g * 17) as u8, (b * 17) as u8))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Theme manager
// ---------------------------------------------------------------------------

/// Owns the theme list and appearance, applies them and keeps them in sync
/// with other running Zenless apps.
pub struct ThemeManager {
    pub appearance: Appearance,
    builtins: Vec<Theme>,
    customs: Vec<Theme>,
    current: Theme,
    last_poll: Instant,
    last_seen_mtime: Option<SystemTime>,
    /// Theme being edited in the appearance UI, if any.
    editor: Option<Theme>,
    editor_error: Option<String>,
}

impl ThemeManager {
    /// Loads appearance + custom themes from disk and applies them to `ctx`.
    pub fn new(ctx: &egui::Context) -> Self {
        install_fonts(ctx);
        let builtins = builtin_themes();
        let mut me = Self {
            appearance: load_appearance(),
            current: builtins[0].clone(),
            builtins,
            customs: load_custom_themes(),
            last_poll: Instant::now(),
            last_seen_mtime: mtime(&appearance_path()),
            editor: None,
            editor_error: None,
        };
        me.current = me.resolve(&me.appearance.theme.clone());
        me.apply(ctx);
        me
    }

    fn resolve(&self, name: &str) -> Theme {
        self.all()
            .find(|t| t.name == name)
            .cloned()
            .unwrap_or_else(|| self.builtins[0].clone())
    }

    /// Built-in themes followed by custom themes.
    pub fn all(&self) -> impl Iterator<Item = &Theme> {
        self.builtins.iter().chain(self.customs.iter())
    }

    pub fn current(&self) -> &Theme {
        self.editor.as_ref().unwrap_or(&self.current)
    }

    pub fn palette(&self) -> &Palette {
        &self.current().palette
    }

    pub fn apply(&self, ctx: &egui::Context) {
        apply(ctx, self.current(), &self.appearance);
    }

    /// Switches to the named theme, saves the choice and applies it.
    pub fn set_theme(&mut self, ctx: &egui::Context, name: &str) {
        self.current = self.resolve(name);
        self.appearance.theme = self.current.name.clone();
        self.persist();
        self.apply(ctx);
    }

    fn persist(&mut self) {
        let _ = save_appearance(&self.appearance);
        self.last_seen_mtime = mtime(&appearance_path());
    }

    /// Call every frame (cheap). Reloads appearance when another Zenless app
    /// changed it.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.last_poll.elapsed() < Duration::from_millis(1500) {
            return;
        }
        self.last_poll = Instant::now();
        let m = mtime(&appearance_path());
        if m.is_some() && m != self.last_seen_mtime {
            self.last_seen_mtime = m;
            self.customs = load_custom_themes();
            self.appearance = load_appearance();
            self.current = self.resolve(&self.appearance.theme.clone());
            self.apply(ctx);
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    /// Full appearance settings UI: theme gallery, sliders and custom theme editor.
    pub fn appearance_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut changed_prefs = false;

        ui.heading("Theme");
        ui.add_space(4.0);
        let names: Vec<(String, bool)> = self
            .all()
            .map(|t| (t.name.clone(), t.builtin))
            .collect();
        let mut clicked: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(10.0);
            for (name, _) in &names {
                let t = self.resolve(name);
                let selected = self.editor.is_none() && self.current.name == *name;
                if theme_card(ui, &t, selected).clicked() {
                    clicked = Some(name.clone());
                }
            }
        });
        if let Some(name) = clicked {
            self.editor = None;
            self.set_theme(&ctx, &name);
        }

        ui.add_space(12.0);
        ui.heading("Layout");
        ui.add_space(4.0);
        egui::Grid::new("zen_appearance_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Text size");
                changed_prefs |= ui
                    .add(egui::Slider::new(&mut self.appearance.font_scale, 0.8..=1.6).step_by(0.05))
                    .changed();
                ui.end_row();
                ui.label("Corner rounding");
                changed_prefs |= ui
                    .add(egui::Slider::new(&mut self.appearance.rounding, 0.0..=16.0).step_by(1.0))
                    .changed();
                ui.end_row();
                ui.label("Density");
                changed_prefs |= ui
                    .add(
                        egui::Slider::new(&mut self.appearance.density, 0.75..=1.5)
                            .step_by(0.05)
                            .custom_formatter(|v, _| {
                                if v < 0.9 {
                                    format!("{v:.2} compact")
                                } else if v > 1.1 {
                                    format!("{v:.2} roomy")
                                } else {
                                    format!("{v:.2}")
                                }
                            }),
                    )
                    .changed();
                ui.end_row();
            });
        if ui.button("Reset layout").clicked() {
            let theme = self.appearance.theme.clone();
            self.appearance = Appearance {
                theme,
                ..Default::default()
            };
            changed_prefs = true;
        }
        if changed_prefs {
            self.persist();
            self.apply(&ctx);
        }

        ui.add_space(12.0);
        ui.heading("Custom themes");
        ui.add_space(4.0);
        if self.editor.is_none() {
            ui.label(
                egui::RichText::new(
                    "Start from the current theme, tweak any color and save it. \
                     Custom themes are shared by all Zenless apps.",
                )
                .color(self.palette().text_dim),
            );
            ui.horizontal(|ui| {
                if ui.button(format!("{}  New from current", egui_phosphor::regular::PLUS)).clicked() {
                    let mut t = self.current.clone();
                    t.builtin = false;
                    t.name = self.unique_name(&format!("{} (custom)", self.current.name));
                    self.editor = Some(t);
                }
                if !self.current.builtin {
                    if ui.button(format!("{}  Edit", egui_phosphor::regular::PENCIL_SIMPLE)).clicked() {
                        self.editor = Some(self.current.clone());
                    }
                    if ui.button(format!("{}  Delete", egui_phosphor::regular::TRASH)).clicked() {
                        let name = self.current.name.clone();
                        let _ = delete_custom_theme(&name);
                        self.customs.retain(|t| t.name != name);
                        self.set_theme(&ctx, DEFAULT_THEME);
                    }
                }
            });
        }

        let mut close_editor = false;
        let mut save: Option<Theme> = None;
        if let Some(editor) = &mut self.editor {
            let mut edited = false;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut editor.name);
                    edited |= ui.checkbox(&mut editor.dark, "Dark theme").changed();
                });
                ui.add_space(6.0);
                egui::Grid::new("zen_theme_editor")
                    .num_columns(4)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for (i, (label, color)) in editor.palette.fields_mut().into_iter().enumerate() {
                            ui.label(label);
                            edited |= egui::color_picker::color_edit_button_srgba(
                                ui,
                                color,
                                egui::color_picker::Alpha::Opaque,
                            )
                            .changed();
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                ui.add_space(6.0);
                if let Some(err) = &self.editor_error {
                    ui.colored_label(editor.palette.danger, err);
                }
                ui.horizontal(|ui| {
                    if ui.button(format!("{}  Save theme", egui_phosphor::regular::FLOPPY_DISK)).clicked() {
                        save = Some(editor.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        close_editor = true;
                    }
                });
            });
            if edited {
                apply(&ctx, editor, &self.appearance);
            }
        }

        if let Some(mut t) = save {
            t.name = t.name.trim().to_owned();
            if t.name.is_empty() {
                self.editor_error = Some("Give your theme a name.".into());
            } else if self.builtins.iter().any(|b| b.name.eq_ignore_ascii_case(&t.name)) {
                self.editor_error = Some("That name belongs to a built-in theme.".into());
            } else {
                match save_custom_theme(&t) {
                    Ok(()) => {
                        self.customs.retain(|c| c.name != t.name);
                        self.customs.push(t.clone());
                        self.customs
                            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                        self.editor = None;
                        self.editor_error = None;
                        self.set_theme(&ctx, &t.name);
                    }
                    Err(e) => self.editor_error = Some(format!("Could not save: {e}")),
                }
            }
        }
        if close_editor {
            self.editor = None;
            self.editor_error = None;
            self.apply(&ctx);
        }
    }

    fn unique_name(&self, base: &str) -> String {
        if !self.all().any(|t| t.name == base) {
            return base.to_owned();
        }
        (2..)
            .map(|i| format!("{base} {i}"))
            .find(|n| !self.all().any(|t| &t.name == n))
            .unwrap()
    }
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// A clickable preview card for a theme.
pub fn theme_card(ui: &mut egui::Ui, t: &Theme, selected: bool) -> egui::Response {
    let size = Vec2::new(150.0, 92.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let p = &t.palette;
    let painter = ui.painter_at(rect.expand(3.0));
    let r = CornerRadius::same(8);

    painter.rect_filled(rect, r, p.bg);
    // fake title bar + sidebar + rows
    let inner = rect.shrink(8.0);
    let bar = egui::Rect::from_min_size(inner.min, Vec2::new(inner.width(), 10.0));
    painter.rect_filled(bar, CornerRadius::same(3), p.surface);
    painter.circle_filled(bar.left_center() + Vec2::new(6.0, 0.0), 2.5, p.accent);
    let side = egui::Rect::from_min_size(
        inner.min + Vec2::new(0.0, 14.0),
        Vec2::new(28.0, inner.height() - 32.0),
    );
    painter.rect_filled(side, CornerRadius::same(3), p.surface);
    for i in 0..3 {
        let y = inner.min.y + 16.0 + i as f32 * 11.0;
        let row = egui::Rect::from_min_size(
            egui::pos2(side.max.x + 5.0, y),
            Vec2::new(inner.max.x - side.max.x - 5.0, 8.0),
        );
        painter.rect_filled(row, CornerRadius::same(2), if i % 2 == 0 { p.surface2 } else { p.stripe });
        let prog = egui::Rect::from_min_size(row.min, Vec2::new(row.width() * [0.8, 0.45, 0.62][i], 3.0));
        painter.rect_filled(prog, CornerRadius::same(1), [p.accent, p.accent2, p.success][i]);
    }
    painter.text(
        egui::pos2(inner.min.x + 1.0, inner.max.y),
        egui::Align2::LEFT_BOTTOM,
        &t.name,
        FontId::proportional(12.0),
        p.text,
    );

    let stroke = if selected {
        Stroke::new(2.5, p.accent)
    } else if response.hovered() {
        Stroke::new(1.5, ui.visuals().widgets.hovered.bg_stroke.color)
    } else {
        Stroke::new(1.0, p.border)
    };
    painter.rect_stroke(rect, r, stroke, egui::StrokeKind::Outside);
    if selected {
        painter.text(
            rect.right_top() + Vec2::new(-8.0, 6.0),
            egui::Align2::RIGHT_TOP,
            egui_phosphor::regular::CHECK_CIRCLE,
            FontId::proportional(14.0),
            p.accent,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
