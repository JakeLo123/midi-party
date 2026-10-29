//! The `?` overlay: every binding of the live keymap, grouped by category.

use std::collections::BTreeMap;
use std::path::Path;

use egui::{Align2, FontId, RichText, Stroke, Vec2};
use mp_core::Keymap;

use crate::theme::Theme;

/// Bindings handled by the app itself rather than the editor's keymap.
pub const APP_BINDINGS: &[(&str, &str)] = &[
    ("Cmd+s", "save (:w)"),
    ("Cmd+q", "quit"),
    (":", "command line (Tab completes; :help lists commands)"),
];

pub fn draw(ctx: &egui::Context, keymap: &Keymap, theme: &Theme, config_dir: &Path) {
    let font = theme.font(theme.font_size - 1.0);
    let text = |s: &str| RichText::new(s).font(font.clone());
    let viewport = ctx.content_rect();
    let size = Vec2::new((viewport.width() - 48.0).max(300.0), (viewport.height() - 72.0).max(200.0));
    let frame = egui::Frame::NONE
        .fill(theme.color("status_bg"))
        .stroke(Stroke::new(1.0, theme.color("panel_border")))
        .inner_margin(14.0);

    egui::Window::new("cheat_sheet")
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
            ui.horizontal(|ui| {
                ui.label(RichText::new("Key bindings").font(theme.bold(theme.font_size + 1.0)).color(theme.color("status_mode")));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(text(concat!("MIDI Party ", env!("CARGO_PKG_VERSION"))).color(theme.color("status_dim")));
                });
            });
            ui.add(
                egui::Label::new(
                    text(&format!("Escape or Shift+/ closes. Type a number before a key to repeat it. {} overrides any of this and reloads on save.", config_dir.join("keymap.toml").display()))
                        .color(theme.color("status_dim")),
                )
                .wrap(),
            );
            ui.add_space(8.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let columns = balanced_columns(groups(keymap), 3);
                let column_w = (inner_w - 24.0) / columns.len() as f32;
                ui.horizontal_top(|ui| {
                    for (i, column) in columns.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(12.0);
                        }
                        ui.vertical(|ui| {
                            ui.set_width(column_w);
                            ui.set_max_width(column_w);
                            for (category, rows) in column {
                                draw_group(ui, category, rows, theme, &font);
                            }
                        });
                    }
                });
            });
        });
}

type Group = (String, Vec<(String, String)>);

/// Every binding grouped by category, rows sorted, with the app's own keys appended.
fn groups(keymap: &Keymap) -> Vec<Group> {
    let mut groups: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (keys, cmd) in keymap.bindings() {
        groups.entry(cmd.category().to_string().to_lowercase()).or_default().push((keys, cmd.describe()));
    }
    groups.entry("app".into()).or_default().extend(APP_BINDINGS.iter().map(|(k, d)| (k.to_string(), d.to_string())));
    groups
        .into_iter()
        .map(|(category, mut rows)| {
            rows.sort();
            (category, rows)
        })
        .collect()
}

/// Deals whole groups into `n` columns, each new group going to the shortest column so
/// far, so the columns end up about the same height. Groups keep their order within a column.
fn balanced_columns(groups: Vec<Group>, n: usize) -> Vec<Vec<Group>> {
    let mut columns: Vec<Vec<Group>> = vec![Vec::new(); n.max(1)];
    let mut heights = vec![0usize; columns.len()];
    let mut by_size: Vec<Group> = groups;
    by_size.sort_by_key(|(_, rows)| std::cmp::Reverse(rows.len()));
    for group in by_size {
        let shortest = (0..columns.len()).min_by_key(|&i| heights[i]).unwrap_or(0);
        heights[shortest] += group.1.len() + 2;
        columns[shortest].push(group);
    }
    for column in &mut columns {
        column.sort_by(|a, b| a.0.cmp(&b.0));
    }
    columns
}

fn draw_group(ui: &mut egui::Ui, category: &str, rows: &[(String, String)], theme: &Theme, font: &FontId) {
    ui.add_space(6.0);
    ui.label(RichText::new(category).font(theme.bold(font.size)).color(theme.color("status_dim")));
    for (keys, desc) in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.allocate_ui_with_layout(Vec2::new(130.0, 0.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(keys).font(font.clone()).color(theme.color("accent")));
            });
            ui.add(egui::Label::new(RichText::new(desc).font(font.clone()).color(theme.color("status_fg"))).wrap());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_balance_by_row_count() {
        let groups: Vec<Group> = [("a", 10), ("b", 6), ("c", 5), ("d", 1), ("e", 1)]
            .into_iter()
            .map(|(name, n)| (name.to_string(), vec![(String::new(), String::new()); n]))
            .collect();
        let columns = balanced_columns(groups, 3);
        let heights: Vec<usize> = columns.iter().map(|c| c.iter().map(|g| g.1.len()).sum()).collect();
        assert_eq!(heights, vec![10, 7, 6]);
        assert_eq!(columns[1].iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), vec!["b", "e"]);
    }
}
