//! The `?` overlay: every binding of the live keymap, grouped by mode and category.

use std::collections::BTreeMap;

use egui::{Align2, FontId, RichText, Stroke, Vec2};
use mp_core::{Keymap, Mode};

use crate::theme::Theme;

/// Bindings handled by the app itself rather than the editor's keymap.
pub const APP_BINDINGS: &[(&str, &str)] = &[
    ("Cmd+s", "save (:w)"),
    ("Cmd+q", "quit"),
    (":", "command line (Tab completes; :help lists commands)"),
];

pub fn draw(ctx: &egui::Context, keymap: &Keymap, theme: &Theme) {
    let font = FontId::monospace(theme.font_size - 1.0);
    let mono = |s: &str| RichText::new(s).font(font.clone());
    let viewport = ctx.content_rect();
    let size = Vec2::new((viewport.width() - 48.0).max(300.0), (viewport.height() - 72.0).max(200.0));
    let frame = egui::Frame::NONE
        .fill(theme.color("status_bg"))
        .stroke(Stroke::new(1.0, theme.color("bar_line")))
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
            ui.label(mono("KEY BINDINGS").color(theme.color("status_mode")).strong());
            ui.add(
                egui::Label::new(
                    mono("Escape or Shift+/ closes. Type a number before a key to repeat it. ~/.config/midi-party/keymap.toml overrides any of this and reloads on save.")
                        .color(theme.color("status_dim")),
                )
                .wrap(),
            );
            ui.add_space(8.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let modes = [Mode::Normal, Mode::Select, Mode::Move];
                let column_w = (inner_w - 24.0) / modes.len() as f32;
                ui.horizontal_top(|ui| {
                    for (i, mode) in modes.into_iter().enumerate() {
                        if i > 0 {
                            ui.add_space(12.0);
                        }
                        ui.vertical(|ui| {
                            ui.set_width(column_w);
                            ui.set_max_width(column_w);
                            draw_mode(ui, keymap, mode, theme, &font);
                        });
                    }
                });
            });
        });
}

fn draw_mode(ui: &mut egui::Ui, keymap: &Keymap, mode: Mode, theme: &Theme, font: &FontId) {
    let mono = |s: &str| RichText::new(s).font(font.clone());
    ui.label(mono(&format!("{} mode", mode.name().to_uppercase())).color(theme.color("status_mode")).strong());
    let mut groups: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (keys, cmd) in keymap.bindings(mode) {
        groups.entry(cmd.category().to_string()).or_default().push((keys, cmd.describe()));
    }
    if mode == Mode::Normal {
        let app = groups.entry("app".into()).or_default();
        app.extend(APP_BINDINGS.iter().map(|(k, d)| (k.to_string(), d.to_string())));
    }
    for (category, mut rows) in groups {
        rows.sort();
        ui.add_space(6.0);
        ui.label(mono(&category.to_lowercase()).color(theme.color("status_dim")));
        for (keys, desc) in rows {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_sized(
                    [110.0, 0.0],
                    egui::Label::new(mono(&keys).color(theme.color("accent"))).halign(egui::Align::RIGHT),
                );
                ui.add(egui::Label::new(mono(&desc).color(theme.color("status_fg"))).wrap());
            });
        }
    }
}
