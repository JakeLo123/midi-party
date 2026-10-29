//! Colors, fonts and sizes from `theme.toml`. Any color name the UI asks for
//! that the file does not define falls back to the built-in default, then to
//! magenta so a typo is obvious instead of invisible.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};
use serde::Deserialize;

pub const DEFAULT_THEME: &str = include_str!("../../../themes/default.toml");

const BOLD: &str = "bold";

#[derive(Deserialize, Debug)]
struct ThemeFile {
    #[serde(default)]
    font_size: Option<f32>,
    #[serde(default)]
    track_colors: Option<Vec<String>>,
    #[serde(default)]
    colors: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub font_size: f32,
    colors: HashMap<String, Color32>,
    track_colors: Vec<Color32>,
}

/// Parses `#rrggbb` or `#rrggbbaa`.
pub fn parse_hex(s: &str) -> Option<Color32> {
    let hex = s.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

impl Theme {
    pub fn default_theme() -> Self {
        Self::from_toml_str(DEFAULT_THEME).expect("built-in theme is valid")
    }

    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        let file: ThemeFile = toml::from_str(text)?;
        let mut theme = if text.as_ptr() == DEFAULT_THEME.as_ptr() {
            Self { font_size: 14.0, colors: HashMap::new(), track_colors: Vec::new() }
        } else {
            Self::default_theme()
        };
        if let Some(size) = file.font_size {
            theme.font_size = size;
        }
        if let Some(list) = file.track_colors {
            theme.track_colors = list
                .iter()
                .filter_map(|hex| {
                    let color = parse_hex(hex);
                    if color.is_none() {
                        eprintln!("theme: bad track color {hex:?} (expected #rrggbb)");
                    }
                    color
                })
                .collect();
        }
        for (name, hex) in file.colors {
            if let Some(color) = parse_hex(&hex) {
                theme.colors.insert(name, color);
            } else {
                eprintln!("theme: bad color {name} = {hex:?} (expected #rrggbb)");
            }
        }
        Ok(theme)
    }

    /// Loads a theme file layered over the defaults. A missing or broken file
    /// yields the defaults and an error message for the status bar.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_toml_str(&text) {
                Ok(theme) => (theme, None),
                Err(e) => (Self::default_theme(), Some(format!("theme.toml: {e}"))),
            },
            Err(_) => (Self::default_theme(), None),
        }
    }

    pub fn color(&self, name: &str) -> Color32 {
        self.colors.get(name).copied().unwrap_or(Color32::from_rgb(255, 0, 255))
    }

    /// The note color of track `index`, cycling through `track_colors`.
    pub fn track_color(&self, index: usize) -> Color32 {
        match self.track_colors.len() {
            0 => self.color("note"),
            n => self.track_colors[index % n],
        }
    }

    pub fn font(&self, size: f32) -> FontId {
        FontId::monospace(size)
    }

    pub fn bold(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(BOLD.into()))
    }

    /// One monospace face for every text in the app, Anonymous Pro, with its bold
    /// for headings. egui's own fonts stay as fallbacks for symbols.
    pub fn font_definitions() -> FontDefinitions {
        let mut fonts = FontDefinitions::default();
        for (name, bytes) in [
            ("AnonymousPro-Regular", &include_bytes!("../../../assets/fonts/AnonymousPro-Regular.ttf")[..]),
            ("AnonymousPro-Bold", &include_bytes!("../../../assets/fonts/AnonymousPro-Bold.ttf")[..]),
        ] {
            fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
        }
        let mut families: BTreeMap<FontFamily, Vec<String>> = BTreeMap::new();
        for (family, first) in [
            (FontFamily::Proportional, "AnonymousPro-Regular"),
            (FontFamily::Monospace, "AnonymousPro-Regular"),
            (FontFamily::Name(BOLD.into()), "AnonymousPro-Bold"),
        ] {
            let mut list = vec![first.to_string()];
            list.extend(fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default());
            list.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
            families.insert(family, list);
        }
        fonts.families = families;
        fonts
    }

    /// egui's own widget colors, so anything drawn by stock widgets matches the theme.
    pub fn visuals(&self) -> Visuals {
        let mut v = Visuals::dark();
        let border = Stroke::new(1.0, self.color("panel_border"));
        v.override_text_color = Some(self.color("status_fg"));
        v.panel_fill = self.color("status_bg");
        v.window_fill = self.color("status_bg");
        v.window_stroke = border;
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = egui::Shadow::NONE;
        v.popup_shadow = egui::Shadow::NONE;
        v.extreme_bg_color = self.color("background");
        v.faint_bg_color = self.color("lane_current");
        v.code_bg_color = self.color("lane_current");
        v.selection.bg_fill = self.color("accent").gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, self.color("accent"));
        v.hyperlink_color = self.color("accent");
        v.warn_fg_color = self.color("accent");
        v.error_fg_color = self.color("error");
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(2);
            w.bg_stroke = border;
        }
        v.widgets.noninteractive.bg_fill = self.color("status_bg");
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.color("status_fg"));
        v.widgets.inactive.bg_fill = self.color("lane_current");
        v.widgets.inactive.weak_bg_fill = self.color("lane_current");
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, self.color("status_fg"));
        v.widgets.hovered.bg_fill = self.color("bar_line");
        v.widgets.hovered.weak_bg_fill = self.color("bar_line");
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, self.color("status_dim"));
        v.widgets.active.bg_fill = self.color("bar_line");
        v.widgets.active.bg_stroke = Stroke::new(1.0, self.color("accent"));
        v.widgets.open.bg_fill = self.color("lane_current");
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_defines_every_core_color() {
        let t = Theme::default_theme();
        for name in [
            "background", "black_key_row", "white_key", "black_key", "key_label", "grid", "bar_line", "note", "note_selected",
            "cursor", "playhead", "status_bg", "status_fg", "lane", "lane_current", "roll_window", "panel_border", "wordmark",
        ] {
            assert_ne!(t.color(name), Color32::from_rgb(255, 0, 255), "missing color {name}");
        }
    }

    #[test]
    fn user_theme_layers_over_default() {
        let t = Theme::from_toml_str("[colors]\nnote = \"#112233\"\n").unwrap();
        assert_eq!(t.color("note"), Color32::from_rgb(0x11, 0x22, 0x33));
        assert_ne!(t.color("cursor"), Color32::from_rgb(255, 0, 255));
        assert_eq!(t.track_color(0), Theme::default_theme().track_color(0), "overlay without track_colors keeps the default list");
    }

    #[test]
    fn track_colors_cycle_and_fall_back_to_note() {
        let t = Theme::from_toml_str("track_colors = [\"#010101\", \"#020202\"]\n").unwrap();
        assert_eq!(t.track_color(0), Color32::from_rgb(1, 1, 1));
        assert_eq!(t.track_color(1), Color32::from_rgb(2, 2, 2));
        assert_eq!(t.track_color(2), Color32::from_rgb(1, 1, 1));
        let empty = Theme::from_toml_str("track_colors = []\n[colors]\nnote = \"#0a0b0c\"\n").unwrap();
        assert_eq!(empty.track_color(5), Color32::from_rgb(0x0a, 0x0b, 0x0c));
    }

    #[test]
    fn party_preset_is_a_complete_overlay() {
        let party = Theme::from_toml_str(include_str!("../../../themes/party.toml")).unwrap();
        assert_ne!(party.color("note"), Theme::default_theme().color("note"));
        assert_ne!(party.track_color(0), Theme::default_theme().track_color(0));
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#ff8000"), Some(Color32::from_rgb(255, 128, 0)));
        assert_eq!(parse_hex("#ff800080"), Some(Color32::from_rgba_unmultiplied(255, 128, 0, 128)));
        assert_eq!(parse_hex("ff8000"), None);
        assert_eq!(parse_hex("#xyz"), None);
    }
}
