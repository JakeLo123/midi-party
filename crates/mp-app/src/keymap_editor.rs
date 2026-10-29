//! The in-app keymap editor: a keyboard-driven list of every binding. Pick a
//! row, press Enter, press the new key, and the binding is written to the
//! user's `keymap.toml` (comments preserved), which the watcher then reloads.

use std::path::Path;

use egui::{Align2, RichText, Stroke, Vec2};
use mp_core::{Command, Keymap, BINDINGS_TABLE};
use toml_edit::{DocumentMut, InlineTable, Item, Value};

use crate::theme::Theme;

#[derive(Clone)]
struct Row {
    keys: String,
    command: Command,
}

#[derive(Default)]
pub struct KeymapEditor {
    pub open: bool,
    rows: Vec<Row>,
    filter: String,
    selected: usize,
    capturing: bool,
    message: String,
}

/// Converts a serde-produced TOML value into a `toml_edit` value.
fn edit_value(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::from(s.as_str()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => Value::from(*f),
        toml::Value::Boolean(b) => Value::from(*b),
        toml::Value::Datetime(d) => Value::from(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(edit_value).collect()),
        toml::Value::Table(table) => {
            let mut inline = InlineTable::new();
            // `cmd` first so the file reads naturally.
            if let Some(cmd) = table.get("cmd") {
                inline.insert("cmd", edit_value(cmd));
            }
            for (k, v) in table {
                if k != "cmd" {
                    inline.insert(k, edit_value(v));
                }
            }
            Value::InlineTable(inline)
        }
    }
}

/// The TOML value for a binding: a bare name when the command has no arguments.
fn binding_value(command: &Command) -> Result<Value, String> {
    let value = toml::Value::try_from(command).map_err(|e| e.to_string())?;
    match &value {
        toml::Value::Table(t) if t.len() == 1 && t.contains_key("cmd") => Ok(edit_value(&t["cmd"])),
        _ => Ok(edit_value(&value)),
    }
}

/// Writes (or removes, with `None`) one binding in the user's keymap file.
pub fn write_binding(path: &Path, keys: &str, command: Option<&Command>) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("keymap.toml: {e}"))?;
    let table = doc.entry(BINDINGS_TABLE).or_insert(toml_edit::table());
    let Some(table) = table.as_table_mut() else {
        return Err(format!("[{BINDINGS_TABLE}] in keymap.toml is not a table"));
    };
    let value = match command {
        Some(c) => binding_value(c)?,
        None => Value::from("unbind"),
    };
    table.insert(keys, Item::Value(value));
    std::fs::write(path, doc.to_string()).map_err(|e| e.to_string())
}

impl KeymapEditor {
    pub fn open(&mut self, keymap: &Keymap) {
        let mut rows: Vec<Row> =
            keymap.bindings().into_iter().map(|(keys, c)| Row { keys, command: c.clone() }).collect();
        // Commands with no key yet, so they can be given one.
        for example in Command::examples() {
            if !rows.iter().any(|r| r.command.name() == example.name()) {
                rows.push(Row { keys: String::new(), command: example });
            }
        }
        rows.sort_by(|a, b| {
            (a.command.category().to_string(), a.command.describe()).cmp(&(b.command.category().to_string(), b.command.describe()))
        });
        self.rows = rows;
        self.filter.clear();
        self.selected = 0;
        self.capturing = false;
        self.message = String::new();
        self.open = true;
    }

    fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                needle.is_empty()
                    || r.keys.to_lowercase().contains(&needle)
                    || r.command.name().contains(&needle)
                    || r.command.describe().to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Typed characters narrow the list.
    pub fn handle_text(&mut self, text: &str) {
        if self.capturing {
            return;
        }
        for c in text.chars() {
            if !c.is_control() {
                self.filter.push(c);
            }
        }
        self.selected = 0;
    }

    /// Handles one chord while the editor is open. Returns a status message when something happened.
    pub fn handle_chord(&mut self, chord: &str, keymap_path: &Path) -> Option<String> {
        let visible = self.visible();
        let row = visible.get(self.selected).map(|i| self.rows[*i].clone());
        if self.capturing {
            self.capturing = false;
            if chord == "Escape" {
                self.message = "cancelled".into();
                return None;
            }
            let row = row?;
            return Some(match write_binding(keymap_path, chord, Some(&row.command)) {
                Ok(()) => {
                    self.message = format!("{chord} → {} saved", row.command.name());
                    self.message.clone()
                }
                Err(e) => {
                    self.message = e.clone();
                    e
                }
            });
        }
        match chord {
            "Escape" => {
                if self.filter.is_empty() {
                    self.open = false;
                } else {
                    self.filter.clear();
                    self.selected = 0;
                }
            }
            "Up" => self.selected = self.selected.saturating_sub(1),
            "Down" => self.selected = (self.selected + 1).min(visible.len().saturating_sub(1)),
            "PageUp" => self.selected = self.selected.saturating_sub(15),
            "PageDown" => self.selected = (self.selected + 15).min(visible.len().saturating_sub(1)),
            "Enter" => {
                if row.is_some() {
                    self.capturing = true;
                    self.message = "press the new key (Escape cancels)".into();
                }
            }
            "Backspace" => {
                if !self.filter.is_empty() {
                    self.filter.pop();
                    self.selected = 0;
                } else if let Some(row) = row.filter(|r| !r.keys.is_empty()) {
                    return Some(match write_binding(keymap_path, &row.keys, None) {
                        Ok(()) => {
                            self.message = format!("{} unbound", row.keys);
                            self.message.clone()
                        }
                        Err(e) => e,
                    });
                }
            }
            _ => {}
        }
        None
    }

    pub fn draw(&self, ctx: &egui::Context, theme: &Theme) {
        let font = theme.font(theme.font_size - 1.0);
        let mono = |s: &str| RichText::new(s).font(font.clone());
        let viewport = ctx.content_rect();
        let size = Vec2::new((viewport.width() - 120.0).clamp(300.0, 900.0), (viewport.height() - 72.0).max(200.0));
        let frame = egui::Frame::NONE
            .fill(theme.color("status_bg"))
            .stroke(Stroke::new(1.0, theme.color("panel_border")))
            .inner_margin(14.0);
        let visible = self.visible();
        egui::Window::new("keymap_editor")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .frame(frame)
            .fixed_size(size)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let inner_w = size.x - 28.0;
                ui.set_min_size(Vec2::new(inner_w, size.y - 28.0));
                ui.set_max_width(inner_w);
                ui.label(RichText::new("Keymap editor").font(theme.bold(theme.font_size + 1.0)).color(theme.color("status_mode")));
                ui.add(
                    egui::Label::new(
                        mono("Up/Down pick a row · Enter then press a key to rebind · Backspace unbinds · type to filter · Escape closes. Changes are written to keymap.toml and apply immediately.")
                            .color(theme.color("status_dim")),
                    )
                    .wrap(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(mono(&format!("filter: {}▏", self.filter)).color(theme.color("status_fg")));
                    if !self.message.is_empty() {
                        ui.label(mono(&self.message).color(theme.color(if self.capturing { "note_selected" } else { "accent" })));
                    }
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (pos, index) in visible.iter().enumerate() {
                        let row = &self.rows[*index];
                        let selected = pos == self.selected;
                        let bg = if selected { theme.color("lane_current") } else { theme.color("status_bg") };
                        let response = egui::Frame::NONE.fill(bg).inner_margin(2.0).show(ui, |ui| {
                            ui.set_width(inner_w - 8.0);
                            ui.horizontal(|ui| {
                                let keys = if row.keys.is_empty() { "—" } else { &row.keys };
                                let key_color = theme.color(if selected { "note_selected" } else { "accent" });
                                ui.allocate_ui_with_layout(Vec2::new(150.0, 0.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(mono(keys).color(key_color));
                                });
                                ui.add(egui::Label::new(mono(&row.command.describe()).color(theme.color("status_fg"))).wrap());
                            });
                        });
                        if selected {
                            ui.scroll_to_rect(response.response.rect, None);
                        }
                    }
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_bindings_without_losing_comments() {
        let dir = std::env::temp_dir().join(format!("mp-keymap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keymap.toml");
        std::fs::write(&path, "# my notes\npreset = \"vim\"\n\n[normal]\n\"z\" = \"undo\" # keep me\n").unwrap();
        write_binding(&path, "Shift+n", Some(&Command::InsertChord { intervals: vec![0, 3, 7] })).unwrap();
        write_binding(&path, "x", None).unwrap();
        write_binding(&path, "y", Some(&Command::Copy)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my notes"));
        assert!(text.contains("# keep me"));
        assert!(text.contains("preset = \"vim\""));
        // The result must still be a valid overlay with the expected values
        // (toml_edit may write bare keys where TOML allows them).
        let table: toml::Table = toml::from_str(&text).unwrap();
        let normal = table["normal"].as_table().unwrap();
        assert_eq!(normal["z"].as_str(), Some("undo"));
        assert_eq!(normal["y"].as_str(), Some("copy"));
        let chord = normal["Shift+n"].as_table().unwrap();
        assert_eq!(chord["cmd"].as_str(), Some("insert_chord"));
        assert_eq!(chord["intervals"].as_array().map(|a| a.len()), Some(3));
        assert_eq!(normal["x"].as_str(), Some("unbind"));
        assert!(text.contains("{ cmd = \"insert_chord\", intervals = [0, 3, 7] }"), "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
