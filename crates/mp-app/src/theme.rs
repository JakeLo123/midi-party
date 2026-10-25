//! Colors and sizes from `theme.toml`. Any color name the UI asks for that the
//! file does not define falls back to the built-in default, then to magenta so
//! a typo is obvious instead of invisible.

use std::collections::HashMap;
use std::path::Path;

use egui::Color32;
use serde::Deserialize;

pub const DEFAULT_THEME: &str = include_str!("../../../themes/default.toml");

#[derive(Deserialize, Debug)]
struct ThemeFile {
    #[serde(default)]
    font_size: Option<f32>,
    #[serde(default)]
    colors: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub font_size: f32,
    colors: HashMap<String, Color32>,
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
            Self { font_size: 14.0, colors: HashMap::new() }
        } else {
            Self::default_theme()
        };
        if let Some(size) = file.font_size {
            theme.font_size = size;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_defines_every_core_color() {
        let t = Theme::default_theme();
        for name in ["background", "black_key_row", "white_key", "black_key", "key_label", "grid", "bar_line", "note", "note_selected", "cursor", "playhead", "status_bg", "status_fg", "lane", "lane_current", "roll_window"] {
            assert_ne!(t.color(name), Color32::from_rgb(255, 0, 255), "missing color {name}");
        }
    }

    #[test]
    fn user_theme_layers_over_default() {
        let t = Theme::from_toml_str("[colors]\nnote = \"#112233\"\n").unwrap();
        assert_eq!(t.color("note"), Color32::from_rgb(0x11, 0x22, 0x33));
        assert_ne!(t.color("cursor"), Color32::from_rgb(255, 0, 255));
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#ff8000"), Some(Color32::from_rgb(255, 128, 0)));
        assert_eq!(parse_hex("#ff800080"), Some(Color32::from_rgba_unmultiplied(255, 128, 0, 128)));
        assert_eq!(parse_hex("ff8000"), None);
        assert_eq!(parse_hex("#xyz"), None);
    }
}
