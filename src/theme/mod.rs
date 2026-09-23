//! Herdr's active palette, resolved from Herdr's `config.toml`.
//!
//! Herdr exposes no theme to plugins, so this repeats Herdr's own resolution
//! (v0.9.1 `theme_runtime_config` / `resolve_palette_for_theme_name`):
//! built-in `theme.name` → `[theme.custom]` → legacy `[ui].accent`.
//! `theme.auto_switch` light/dark cannot be observed from a plugin, so the
//! manual `theme.name` is used.

mod builtin;

use ratatui::style::Color;
use serde::Deserialize;
use std::path::PathBuf;

/// Same fields as Herdr's `Palette` so `builtin.rs` stays a verbatim port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color,
    pub panel_bg: Color,
    pub sidebar_bg: Color,
    pub active_row_bg: Color,
    pub selection_bg: Color,
    pub surface0: Color,
    pub surface1: Color,
    pub surface_dim: Color,
    pub overlay0: Color,
    pub overlay1: Color,
    pub text: Color,
    pub subtext0: Color,
    pub mauve: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub blue: Color,
    pub teal: Color,
    pub peach: Color,
}

pub const THEME_NAMES: &[&str] = &[
    "catppuccin",
    "catppuccin-latte",
    "terminal",
    "tokyo-night",
    "tokyo-night-day",
    "dracula",
    "nord",
    "gruvbox",
    "gruvbox-light",
    "one-dark",
    "one-light",
    "solarized",
    "solarized-light",
    "kanagawa",
    "kanagawa-lotus",
    "rose-pine",
    "rose-pine-dawn",
    "vesper",
];

/// Herdr's theme name aliases (`config/theme.rs::canonical_theme_name`).
pub fn canonical_theme_name(name: &str) -> Option<&'static str> {
    match name.to_lowercase().replace([' ', '_'], "-").as_str() {
        "catppuccin" | "catppuccin-mocha" => Some("catppuccin"),
        "catppuccin-latte" | "latte" | "light" => Some("catppuccin-latte"),
        "terminal" => Some("terminal"),
        "tokyo-night" | "tokyonight" => Some("tokyo-night"),
        "tokyo-night-day" | "tokyo-day" | "tokyonight-day" => Some("tokyo-night-day"),
        "dracula" => Some("dracula"),
        "nord" => Some("nord"),
        "gruvbox" | "gruvbox-dark" => Some("gruvbox"),
        "gruvbox-light" => Some("gruvbox-light"),
        "one-dark" | "onedark" => Some("one-dark"),
        "one-light" | "onelight" => Some("one-light"),
        "solarized" | "solarized-dark" => Some("solarized"),
        "solarized-light" => Some("solarized-light"),
        "kanagawa" => Some("kanagawa"),
        "kanagawa-lotus" | "lotus" => Some("kanagawa-lotus"),
        "rose-pine" | "rosepine" => Some("rose-pine"),
        "rose-pine-dawn" | "rosepine-dawn" | "dawn" => Some("rose-pine-dawn"),
        "vesper" => Some("vesper"),
        _ => None,
    }
}

impl Palette {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match canonical_theme_name(name)? {
            "catppuccin" => Self::catppuccin(),
            "catppuccin-latte" => Self::catppuccin_latte(),
            "terminal" => Self::terminal(),
            "tokyo-night" => Self::tokyo_night(),
            "tokyo-night-day" => Self::tokyo_night_day(),
            "dracula" => Self::dracula(),
            "nord" => Self::nord(),
            "gruvbox" => Self::gruvbox(),
            "gruvbox-light" => Self::gruvbox_light(),
            "one-dark" => Self::one_dark(),
            "one-light" => Self::one_light(),
            "solarized" => Self::solarized(),
            "solarized-light" => Self::solarized_light(),
            "kanagawa" => Self::kanagawa(),
            "kanagawa-lotus" => Self::kanagawa_lotus(),
            "rose-pine" => Self::rose_pine(),
            "rose-pine-dawn" => Self::rose_pine_dawn(),
            "vesper" => Self::vesper(),
            _ => return None,
        })
    }

    /// Foreground for text drawn on an accent background (Herdr's
    /// `contrast`): the panel background, or `surface_dim` when it is reset.
    pub fn contrast(&self) -> Color {
        match self.panel_bg {
            Color::Reset => self.surface_dim,
            c => c,
        }
    }

    fn apply(&mut self, custom: &CustomColors) {
        let slots: [(&Option<String>, &mut Color); 19] = [
            (&custom.accent, &mut self.accent),
            (&custom.panel_bg, &mut self.panel_bg),
            (&custom.sidebar_bg, &mut self.sidebar_bg),
            (&custom.active_row_bg, &mut self.active_row_bg),
            (&custom.selection_bg, &mut self.selection_bg),
            (&custom.surface0, &mut self.surface0),
            (&custom.surface1, &mut self.surface1),
            (&custom.surface_dim, &mut self.surface_dim),
            (&custom.overlay0, &mut self.overlay0),
            (&custom.overlay1, &mut self.overlay1),
            (&custom.text, &mut self.text),
            (&custom.subtext0, &mut self.subtext0),
            (&custom.mauve, &mut self.mauve),
            (&custom.green, &mut self.green),
            (&custom.yellow, &mut self.yellow),
            (&custom.red, &mut self.red),
            (&custom.blue, &mut self.blue),
            (&custom.teal, &mut self.teal),
            (&custom.peach, &mut self.peach),
        ];
        for (value, slot) in slots {
            if let Some(v) = value {
                *slot = parse_color(v);
            }
        }
    }
}

/// Herdr's `parse_color`: reset aliases, `#rrggbb`, `#rgb`, `rgb(r,g,b)`,
/// named ANSI colors; anything else is cyan.
pub fn parse_color(s: &str) -> Color {
    let s = s.trim().to_lowercase();
    if matches!(s.as_str(), "reset" | "default" | "none" | "transparent") {
        return Color::Reset;
    }
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[0..2], 16),
                u8::from_str_radix(&hex[2..4], 16),
                u8::from_str_radix(&hex[4..6], 16),
            ) {
                return Color::Rgb(r, g, b);
            }
        } else if hex.len() == 3 {
            let d: Vec<u8> = hex.chars().filter_map(|c| c.to_digit(16).map(|v| v as u8)).collect();
            if d.len() == 3 {
                return Color::Rgb(d[0] * 17, d[1] * 17, d[2] * 17);
            }
        }
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<_> = inner.split(',').map(|p| p.trim().parse::<u8>()).collect();
        if let [Ok(r), Ok(g), Ok(b)] = parts.as_slice() {
            return Color::Rgb(*r, *g, *b);
        }
    }
    match s.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => Color::Cyan,
    }
}

#[derive(Debug, Default, Deserialize)]
struct CustomColors {
    accent: Option<String>,
    panel_bg: Option<String>,
    sidebar_bg: Option<String>,
    active_row_bg: Option<String>,
    selection_bg: Option<String>,
    surface0: Option<String>,
    surface1: Option<String>,
    surface_dim: Option<String>,
    overlay0: Option<String>,
    overlay1: Option<String>,
    text: Option<String>,
    subtext0: Option<String>,
    mauve: Option<String>,
    green: Option<String>,
    yellow: Option<String>,
    red: Option<String>,
    blue: Option<String>,
    teal: Option<String>,
    peach: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ThemeSection {
    name: Option<String>,
    custom: Option<CustomColors>,
}

#[derive(Debug, Default, Deserialize)]
struct UiSection {
    accent: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct HerdrConfig {
    #[serde(default)]
    theme: ThemeSection,
    #[serde(default)]
    ui: UiSection,
}

/// Resolve the palette from Herdr config text. Unparseable config yields
/// the default theme, as Herdr itself does.
pub fn resolve(config_text: &str) -> Palette {
    let cfg: HerdrConfig = toml::from_str(config_text).unwrap_or_default();
    let name = cfg.theme.name.as_deref().unwrap_or("catppuccin");
    let mut palette = Palette::from_name(name).unwrap_or_else(Palette::catppuccin);
    let custom_accent = cfg.theme.custom.as_ref().and_then(|c| c.accent.as_ref()).is_some();
    if let Some(custom) = &cfg.theme.custom {
        palette.apply(custom);
    }
    if let Some(accent) = cfg.ui.accent.as_deref()
        && accent != "cyan"
        && !custom_accent
    {
        palette.accent = parse_color(accent);
    }
    palette
}

/// `$HERDR_CONFIG_PATH`, else `~/.config/herdr/config.toml`.
pub fn herdr_config_path(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(p) = get("HERDR_CONFIG_PATH").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    get("HOME").map(|home| PathBuf::from(home).join(".config/herdr/config.toml"))
}

pub fn load() -> Palette {
    herdr_config_path(|k| std::env::var(k).ok())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| resolve(&text))
        .unwrap_or_else(Palette::catppuccin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_theme_resolves() {
        for name in THEME_NAMES {
            assert!(Palette::from_name(name).is_some(), "{name}");
        }
    }

    #[test]
    fn default_is_catppuccin() {
        assert_eq!(resolve(""), Palette::catppuccin());
        assert_eq!(resolve("[theme]\nname = \"nope\""), Palette::catppuccin());
    }

    #[test]
    fn theme_name_aliases() {
        let p = resolve("[theme]\nname = \"rosepine\"");
        assert_eq!(p.accent, Color::Rgb(196, 167, 231));
    }

    #[test]
    fn custom_accent_beats_legacy_ui_accent() {
        let p = resolve("[ui]\naccent = \"#010203\"\n[theme.custom]\naccent = \"#040506\"");
        assert_eq!(p.accent, Color::Rgb(4, 5, 6));
    }

    #[test]
    fn legacy_ui_accent_applies_unless_cyan() {
        assert_eq!(resolve("[ui]\naccent = \"#010203\"").accent, Color::Rgb(1, 2, 3));
        assert_eq!(resolve("[ui]\naccent = \"cyan\"").accent, Palette::catppuccin().accent);
    }

    #[test]
    fn custom_overrides_apply_on_top_of_theme() {
        let p =
            resolve("[theme]\nname = \"nord\"\n[theme.custom]\npanel_bg = \"reset\"\nmauve = \"rgb(1,2,3)\"");
        assert_eq!(p.panel_bg, Color::Reset);
        assert_eq!(p.mauve, Color::Rgb(1, 2, 3));
        assert_eq!(p.accent, Palette::nord().accent);
        assert_eq!(p.contrast(), Palette::nord().surface_dim);
    }

    #[test]
    fn parse_color_forms() {
        assert_eq!(parse_color("#fff"), Color::Rgb(255, 255, 255));
        assert_eq!(parse_color("Blue"), Color::Blue);
        assert_eq!(parse_color("nonsense"), Color::Cyan);
    }
}
