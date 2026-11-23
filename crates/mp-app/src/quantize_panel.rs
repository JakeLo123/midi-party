//! The quantize panel: grid, swing and strength for the selected notes, applied
//! live so the change can be heard before Enter keeps it.

use egui::{Align2, Color32, CornerRadius, Rect, RichText, Sense, Stroke, Vec2};
use mp_core::{Division, Grid, GridModifier, QuantizeSettings};

use crate::theme::Theme;

/// Named swing amounts, in the percent [`Grid::snap`] takes: 67 puts every off-step
/// two thirds of the way through its pair, like a triplet shuffle.
pub const SWING_PRESETS: [(&str, u8); 5] =
    [("Straight", 0), ("Light", 17), ("Medium", 33), ("Triplet feel", 67), ("Heavy", 84)];

const STRENGTH_STEP: u8 = 10;

/// What a key press or click in the panel asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    /// The settings changed; quantize the notes again from where they started.
    Changed(QuantizeSettings),
    /// Keep the result; the panel has closed.
    Apply,
    /// Put the notes back; the panel has closed.
    Cancel,
    /// Start or stop playback, so the result can be heard with the panel open.
    TogglePlay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Row {
    #[default]
    Grid,
    Swing,
    Strength,
}

impl Row {
    const ALL: [Row; 3] = [Row::Grid, Row::Swing, Row::Strength];

    fn step(self, by: isize) -> Row {
        let i = Row::ALL.iter().position(|r| *r == self).unwrap_or(0) as isize;
        Row::ALL[(i + by).clamp(0, Row::ALL.len() as isize - 1) as usize]
    }
}

#[derive(Default)]
pub struct QuantizePanel {
    pub open: bool,
    row: Row,
    grids: Vec<Grid>,
    grid: usize,
    swing: u8,
    strength: u8,
}

fn triplet(base: Division) -> Grid {
    Grid { base, modifier: GridModifier::Triplet }
}

/// The grids the panel offers, longest first, with `current` added if it is not one of them.
fn grid_choices(current: Grid) -> Vec<Grid> {
    let mut grids = vec![
        Grid::plain(Division::Quarter),
        Grid::plain(Division::Eighth),
        triplet(Division::Eighth),
        Grid::plain(Division::Sixteenth),
        triplet(Division::Sixteenth),
        Grid::plain(Division::ThirtySecond),
    ];
    if !grids.contains(&current) {
        grids.push(current);
        grids.sort_by_key(|g| std::cmp::Reverse(g.ticks(960)));
    }
    grids
}

impl QuantizePanel {
    /// Opens with the last-used settings; a `None` grid there means `editing_grid`.
    pub fn open(&mut self, settings: QuantizeSettings, editing_grid: Grid) -> QuantizeSettings {
        let grid = settings.grid.unwrap_or(editing_grid);
        self.grids = grid_choices(grid);
        self.grid = self.grids.iter().position(|g| *g == grid).unwrap_or(0);
        self.swing = settings.swing.min(100);
        self.strength = settings.strength.min(100);
        self.row = Row::Grid;
        self.open = true;
        self.settings()
    }

    pub fn settings(&self) -> QuantizeSettings {
        QuantizeSettings { grid: self.grids.get(self.grid).copied(), strength: self.strength, swing: self.swing }
    }

    fn grid(&self) -> Grid {
        self.grids.get(self.grid).copied().unwrap_or_default()
    }

    fn swing_applies(&self) -> bool {
        self.grid().modifier == GridModifier::Plain
    }

    /// Handles one chord while the panel is open.
    pub fn handle_chord(&mut self, chord: &str) -> Outcome {
        let before = self.settings();
        match chord {
            "Escape" => {
                self.open = false;
                return Outcome::Cancel;
            }
            "Enter" => {
                self.open = false;
                return Outcome::Apply;
            }
            "Space" => return Outcome::TogglePlay,
            "Up" | "Shift+Tab" => self.row = self.row.step(-1),
            "Down" | "Tab" => self.row = self.row.step(1),
            "Left" => self.adjust(-1, false),
            "Right" => self.adjust(1, false),
            "Alt+Left" => self.adjust(-1, true),
            "Alt+Right" => self.adjust(1, true),
            _ => {}
        }
        self.changed_since(before)
    }

    fn changed_since(&self, before: QuantizeSettings) -> Outcome {
        let now = self.settings();
        if now == before {
            Outcome::Nothing
        } else {
            Outcome::Changed(now)
        }
    }

    /// Moves the current row's value one notch, or one percent when `fine`.
    fn adjust(&mut self, dir: i32, fine: bool) {
        match self.row {
            Row::Grid => {
                let last = self.grids.len().saturating_sub(1);
                self.grid = if dir < 0 { self.grid.saturating_sub(1) } else { (self.grid + 1).min(last) };
            }
            Row::Swing if !self.swing_applies() => {}
            Row::Swing if fine => self.swing = nudge(self.swing, dir, 1),
            Row::Swing => {
                let next = if dir < 0 {
                    SWING_PRESETS.iter().rev().map(|p| p.1).find(|v| *v < self.swing)
                } else {
                    SWING_PRESETS.iter().map(|p| p.1).find(|v| *v > self.swing)
                };
                self.swing = next.unwrap_or(self.swing);
            }
            Row::Strength => self.strength = nudge(self.strength, dir, if fine { 1 } else { STRENGTH_STEP }),
        }
    }

    pub fn draw(&mut self, ctx: &egui::Context, theme: &Theme) -> Outcome {
        let before = self.settings();
        let mut outcome = Outcome::Nothing;
        let font = theme.font(theme.font_size - 1.0);
        let text = |s: &str, color: &str| RichText::new(s).font(font.clone()).color(theme.color(color));
        let width = 520.0;
        let frame = egui::Frame::NONE
            .fill(theme.color("status_bg"))
            .stroke(Stroke::new(1.0, theme.color("panel_border")))
            .inner_margin(14.0);
        egui::Window::new("quantize_panel")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .frame(frame)
            .fixed_size(Vec2::new(width, 0.0))
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.label(RichText::new("Quantize").font(theme.bold(theme.font_size + 1.0)).color(theme.color("status_mode")));
                ui.add(
                    egui::Label::new(text(
                        "Up/Down picks a row · Left/Right changes it (Alt for 1%) · Space plays · Enter keeps · Escape puts the notes back.",
                        "status_dim",
                    ))
                    .wrap(),
                );
                ui.add_space(10.0);

                self.row_label(ui, theme, Row::Grid, "Grid");
                ui.horizontal_wrapped(|ui| {
                    for i in 0..self.grids.len() {
                        let label = grid_label(self.grids[i]);
                        if chip(ui, theme, &label, i == self.grid, true) {
                            self.grid = i;
                            self.row = Row::Grid;
                        }
                    }
                });
                ui.add_space(10.0);

                let swing_on = self.swing_applies();
                self.row_label(ui, theme, Row::Swing, &format!("Swing {}%", if swing_on { self.swing } else { 0 }));
                ui.horizontal_wrapped(|ui| {
                    for (name, value) in SWING_PRESETS {
                        if chip(ui, theme, name, swing_on && self.swing == value, swing_on) {
                            self.swing = value;
                            self.row = Row::Swing;
                        }
                    }
                });
                if let Some(v) = bar(ui, theme, if swing_on { self.swing } else { 0 }, swing_on, width) {
                    self.swing = v;
                    self.row = Row::Swing;
                }
                if !swing_on {
                    ui.label(text("Swing applies to straight grids; triplets already have their own feel.", "status_dim"));
                }
                ui.add_space(10.0);

                self.row_label(ui, theme, Row::Strength, &format!("Strength {}%", self.strength));
                if let Some(v) = bar(ui, theme, self.strength, true, width) {
                    self.strength = v;
                    self.row = Row::Strength;
                }
                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    if chip(ui, theme, "Keep (Enter)", false, true) {
                        outcome = Outcome::Apply;
                    }
                    if chip(ui, theme, "Cancel (Esc)", false, true) {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        match outcome {
            Outcome::Apply | Outcome::Cancel => {
                self.open = false;
                outcome
            }
            _ => self.changed_since(before),
        }
    }

    fn row_label(&self, ui: &mut egui::Ui, theme: &Theme, row: Row, label: &str) {
        let current = self.row == row;
        let marker = if current { "> " } else { "  " };
        let color = if current { "accent" } else { "status_fg" };
        ui.label(RichText::new(format!("{marker}{label}")).font(theme.bold(theme.font_size - 1.0)).color(theme.color(color)));
    }
}

fn nudge(value: u8, dir: i32, by: u8) -> u8 {
    if dir < 0 {
        value.saturating_sub(by)
    } else {
        value.saturating_add(by).min(100)
    }
}

fn grid_label(grid: Grid) -> String {
    match grid.modifier {
        GridModifier::Triplet => format!("1/{} triplet", grid.base.denominator()),
        GridModifier::Dotted => format!("1/{} dotted", grid.base.denominator()),
        GridModifier::Plain => grid.to_string(),
    }
}

/// A flat clickable label. True when clicked.
fn chip(ui: &mut egui::Ui, theme: &Theme, label: &str, selected: bool, enabled: bool) -> bool {
    let (fg, bg) = match (selected, enabled) {
        (true, _) => ("background", "accent"),
        (false, true) => ("status_fg", "lane_current"),
        (false, false) => ("status_dim", "status_bg"),
    };
    let rich = RichText::new(label).font(theme.font(theme.font_size - 1.0)).color(theme.color(fg));
    let button = egui::Button::new(rich)
        .fill(theme.color(bg))
        .stroke(Stroke::new(1.0, theme.color("panel_border")))
        .corner_radius(CornerRadius::ZERO);
    ui.add_enabled(enabled, button).clicked()
}

/// A 0–100 bar set by clicking or dragging. Drawn by hand rather than with an egui
/// slider so it never takes keyboard focus away from the panel's own keys.
fn bar(ui: &mut egui::Ui, theme: &Theme, value: u8, enabled: bool, width: f32) -> Option<u8> {
    let sense = if enabled { Sense::click_and_drag() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 12.0), sense);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, theme.color("lane"));
    let filled = Rect::from_min_max(rect.min, egui::pos2(rect.left() + rect.width() * f32::from(value) / 100.0, rect.bottom()));
    let fill: Color32 = if enabled { theme.color("accent") } else { theme.color("status_dim") };
    painter.rect_filled(filled, CornerRadius::ZERO, fill);
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, theme.color("panel_border")), egui::StrokeKind::Inside);
    if !(response.clicked() || response.dragged()) {
        return None;
    }
    let x = response.interact_pointer_pos()?.x;
    let v = ((x - rect.left()) / rect.width() * 100.0).round().clamp(0.0, 100.0) as u8;
    (v != value).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened() -> QuantizePanel {
        let mut p = QuantizePanel::default();
        p.open(QuantizeSettings::default(), Grid::plain(Division::Eighth));
        p
    }

    #[test]
    fn opens_on_the_editing_grid_and_steps_through_grids() {
        let mut p = opened();
        assert!(p.open);
        assert_eq!(p.grid().to_string(), "1/8");
        match p.handle_chord("Right") {
            Outcome::Changed(s) => assert_eq!(s.grid.unwrap().to_string(), "1/8T"),
            other => panic!("{other:?}"),
        }
        p.handle_chord("Right");
        p.handle_chord("Right");
        p.handle_chord("Right");
        assert_eq!(p.handle_chord("Right"), Outcome::Nothing, "1/32 is the last");
    }

    #[test]
    fn swing_steps_through_presets_and_is_off_for_triplets() {
        let mut p = opened();
        p.handle_chord("Down");
        assert_eq!(p.handle_chord("Right"), Outcome::Changed(QuantizeSettings { swing: 17, ..p.settings() }));
        p.handle_chord("Right");
        p.handle_chord("Right");
        assert_eq!(p.settings().swing, 67);
        p.handle_chord("Alt+Left");
        assert_eq!(p.settings().swing, 66);
        p.handle_chord("Right");
        assert_eq!(p.settings().swing, 67, "the next preset up");
        p.handle_chord("Up");
        p.handle_chord("Right"); // 1/8T
        p.handle_chord("Down");
        assert_eq!(p.handle_chord("Right"), Outcome::Nothing);
    }

    #[test]
    fn strength_and_closing() {
        let mut p = opened();
        p.handle_chord("Down");
        p.handle_chord("Down");
        p.handle_chord("Left");
        assert_eq!(p.settings().strength, 90);
        assert_eq!(p.handle_chord("Space"), Outcome::TogglePlay);
        assert!(p.open);
        assert_eq!(p.handle_chord("Enter"), Outcome::Apply);
        assert!(!p.open);
        let mut p = opened();
        assert_eq!(p.handle_chord("Escape"), Outcome::Cancel);
        assert!(!p.open);
    }

    #[test]
    fn an_unusual_grid_joins_the_list() {
        let mut p = QuantizePanel::default();
        let dotted: Grid = "1/8.".parse().unwrap();
        p.open(QuantizeSettings::default(), dotted);
        assert_eq!(p.grid(), dotted);
        assert_eq!(p.grids.iter().map(|g| g.to_string()).collect::<Vec<_>>(), ["1/4", "1/8.", "1/8", "1/8T", "1/16", "1/16T", "1/32"]);
    }
}
