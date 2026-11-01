//! The transport bar across the top of the window: go to start, play/stop, loop and
//! metronome buttons, the playhead position, and the tempo and time signature in
//! effect at the playhead, which can be changed right there.

use egui::{Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use mp_core::{BarBeat, Keymap};

use crate::theme::Theme;

pub const BAR_H: f32 = 36.0;
const BUTTON: Vec2 = Vec2::new(30.0, 24.0);
const GAP: f32 = 4.0;
const SECTION_GAP: f32 = 14.0;
/// Pixels of drag per BPM on the tempo display.
const TEMPO_DRAG_PX: f32 = 3.0;
pub const MIN_BPM: f64 = 20.0;
pub const MAX_BPM: f64 = 400.0;
/// The time signatures the menu offers; anything else is typed.
const COMMON_METERS: &[(u8, u8)] = &[(2, 4), (3, 4), (4, 4), (5, 4), (6, 8), (7, 8), (9, 8), (12, 8)];

/// A value in the bar that is being typed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueField {
    Tempo,
    Meter,
}

/// What a click in the transport bar asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportAction {
    ToStart,
    TogglePlay,
    ToggleLoop,
    ToggleMetronome,
    TapTempo,
    /// The tempo display is being dragged to this BPM.
    DragTempo { bpm: f64 },
    /// The tempo drag ended: the next change is a separate undo step.
    EndTempoDrag,
    SetMeter { numerator: u8, denominator: u8 },
    /// Start typing a value in.
    Edit(ValueField),
}

/// What the bar shows this frame.
pub struct TransportView<'a> {
    pub audio_ok: bool,
    pub playing: bool,
    pub position: BarBeat,
    pub seconds: f64,
    pub bpm: f64,
    pub meter: (u8, u8),
    pub loop_on: bool,
    pub metronome: bool,
    /// A value being typed in, and the text so far.
    pub entry: Option<(ValueField, &'a str)>,
    pub keymap: &'a Keymap,
}

#[derive(Default)]
pub struct TransportBar {
    /// The BPM the tempo display showed when the current drag on it began.
    drag_start_bpm: Option<f64>,
}

/// `name`, followed by the key bound to `command` if there is one.
fn tooltip(name: &str, keymap: &Keymap, command: &mp_core::Command) -> String {
    match keymap.key_for(command) {
        Some(key) => format!("{name} ({key})"),
        None => name.to_string(),
    }
}

/// The BPM a drag of `delta` pixels from `start` asks for: up or right is faster.
pub fn bpm_after_drag(start: f64, delta: Vec2) -> f64 {
    (start + f64::from((delta.x - delta.y) / TEMPO_DRAG_PX)).round().clamp(MIN_BPM, MAX_BPM)
}

/// A tempo the way people say it: whole BPM unless it has a fraction.
pub fn bpm_label(bpm: f64) -> String {
    if (bpm - bpm.round()).abs() < 0.05 { format!("{}", bpm.round()) } else { format!("{bpm:.1}") }
}

impl TransportBar {
    pub fn draw(&mut self, ui: &mut Ui, view: &TransportView, theme: &Theme, actions: &mut Vec<TransportAction>) {
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, theme.color("status_bg"));
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme.color("panel_border")));
        let mid = rect.center().y;
        let mut x = rect.left() + 10.0;
        let keymap = view.keymap;
        use mp_core::Command;

        let button = |ui: &mut Ui, x: &mut f32, id: &str, on: bool, tip: String, icon: fn(&Painter, Rect, Color32)| {
            let r = Rect::from_min_size(Pos2::new(*x, mid - BUTTON.y / 2.0), BUTTON);
            *x += BUTTON.x + GAP;
            let response = ui.interact(r, ui.scope_id().with(id), Sense::click()).on_hover_text(tip);
            let hot = response.hovered();
            let painter = ui.painter();
            if on {
                painter.rect_filled(r, CornerRadius::same(2), theme.color("accent"));
            } else {
                let edge = theme.color(if hot { "status_fg" } else { "bar_line" });
                painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, edge), egui::StrokeKind::Inside);
            }
            let ink = if on { theme.color("status_bg") } else if hot { theme.color("status_fg") } else { theme.color("status_dim") };
            icon(painter, r.shrink2(Vec2::new(9.0, 6.0)), ink);
            response.clicked()
        };

        if button(ui, &mut x, "to_start", false, tooltip("Go to start", keymap, &Command::PlayheadToStart), draw_to_start) {
            actions.push(TransportAction::ToStart);
        }
        let (play_icon, play_name): (fn(&Painter, Rect, Color32), &str) =
            if view.playing { (draw_stop, "Stop") } else { (draw_play, "Play") };
        if button(ui, &mut x, "play", view.playing, tooltip(play_name, keymap, &Command::TogglePlay), play_icon) {
            actions.push(TransportAction::TogglePlay);
        }
        if button(ui, &mut x, "loop", view.loop_on, tooltip("Loop", keymap, &Command::ToggleLoop), draw_loop) {
            actions.push(TransportAction::ToggleLoop);
        }
        if button(ui, &mut x, "metronome", view.metronome, tooltip("Metronome", keymap, &Command::ToggleMetronome), draw_metronome) {
            actions.push(TransportAction::ToggleMetronome);
        }
        x += SECTION_GAP - GAP;

        let value_font = theme.bold(theme.font_size + 3.0);
        let label_font = theme.font(theme.font_size - 3.0);
        let small_font = theme.font(theme.font_size - 1.0);
        let painter = ui.painter().clone();
        let label = |x: &mut f32, text: &str| {
            let r = painter.text(Pos2::new(*x, mid), Align2::LEFT_CENTER, text, label_font.clone(), theme.color("status_dim"));
            *x = r.right() + 6.0;
        };
        let separator = |x: &mut f32| {
            painter.vline(x.round() + 0.5, egui::Rangef::new(rect.top() + 7.0, rect.bottom() - 7.0), Stroke::new(1.0, theme.color("panel_border")));
            *x += SECTION_GAP;
        };
        separator(&mut x);

        label(&mut x, "bar");
        let bar_text = format!("{}.{}", view.position.bar, view.position.beat);
        x = fixed_text(&painter, x, mid, &bar_text, "0000.00", &value_font, theme.color("status_fg")) + 10.0;
        let time = format!("{}:{:05.2}", (view.seconds / 60.0) as u64, view.seconds % 60.0);
        x = fixed_text(&painter, x, mid, &time, "00:00.00", &small_font, theme.color("status_dim")) + SECTION_GAP;
        separator(&mut x);

        label(&mut x, "tempo");
        let typing_tempo = view.entry.filter(|(f, _)| *f == ValueField::Tempo).map(|(_, t)| t);
        let tempo_text = if typing_tempo.is_some() { "000.0".to_string() } else { bpm_label(view.bpm) };
        let tempo_w = text_width(&painter, &tempo_text, &value_font) + 8.0;
        let tempo_rect = Rect::from_min_size(Pos2::new(x - 4.0, mid - 12.0), Vec2::new(tempo_w, 24.0));
        let tempo_response = ui
            .interact(tempo_rect, ui.scope_id().with("tempo"), Sense::click_and_drag())
            .on_hover_text("Drag up or down to change the tempo at the playhead; double-click to type it");
        if let Some(text) = typing_tempo {
            draw_entry(&painter, tempo_rect, text, theme, &value_font);
        } else {
            let hot = tempo_response.hovered() || tempo_response.dragged();
            if hot {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
                painter.rect_stroke(tempo_rect, CornerRadius::same(2), Stroke::new(1.0, theme.color("bar_line")), egui::StrokeKind::Inside);
            }
            painter.text(Pos2::new(x, mid), Align2::LEFT_CENTER, bpm_label(view.bpm), value_font.clone(), theme.color("status_fg"));
        }
        if tempo_response.double_clicked() {
            actions.push(TransportAction::Edit(ValueField::Tempo));
        } else if tempo_response.drag_started() {
            self.drag_start_bpm = Some(view.bpm);
        }
        if let (Some(start), Some(delta)) = (self.drag_start_bpm, tempo_response.total_drag_delta()) {
            let bpm = bpm_after_drag(start, delta);
            if (bpm - view.bpm).abs() > f64::EPSILON {
                actions.push(TransportAction::DragTempo { bpm });
            }
        }
        if tempo_response.drag_stopped() {
            self.drag_start_bpm = None;
            actions.push(TransportAction::EndTempoDrag);
        }
        x = tempo_rect.right() + 2.0;
        let unit = painter.text(Pos2::new(x, mid + 1.0), Align2::LEFT_CENTER, "bpm", small_font.clone(), theme.color("status_dim"));
        x = unit.right() + 8.0;

        let tap_w = text_width(&painter, "tap", &small_font) + 16.0;
        let tap_rect = Rect::from_min_size(Pos2::new(x, mid - BUTTON.y / 2.0), Vec2::new(tap_w, BUTTON.y));
        let tap = ui
            .interact(tap_rect, ui.scope_id().with("tap"), Sense::click())
            .on_hover_text(tooltip("Tap in time to set the tempo", keymap, &Command::TapTempo));
        let tap_hot = tap.hovered() || tap.is_pointer_button_down_on();
        painter.rect_stroke(tap_rect, CornerRadius::same(2), Stroke::new(1.0, theme.color(if tap_hot { "status_fg" } else { "bar_line" })), egui::StrokeKind::Inside);
        painter.text(tap_rect.center(), Align2::CENTER_CENTER, "tap", small_font.clone(), theme.color(if tap_hot { "status_fg" } else { "status_dim" }));
        // Pressing, not releasing, is the beat a person taps on.
        if tap.is_pointer_button_down_on() && ui.input(|i| i.pointer.any_pressed()) {
            actions.push(TransportAction::TapTempo);
        }
        x = tap_rect.right() + SECTION_GAP;
        separator(&mut x);

        label(&mut x, "time");
        let typing_meter = view.entry.filter(|(f, _)| *f == ValueField::Meter).map(|(_, t)| t);
        let meter_text = if typing_meter.is_some() { "00/00".to_string() } else { format!("{}/{}", view.meter.0, view.meter.1) };
        let meter_w = text_width(&painter, &meter_text, &value_font) + 22.0;
        let meter_rect = Rect::from_min_size(Pos2::new(x - 4.0, mid - 12.0), Vec2::new(meter_w, 24.0));
        let meter_response = ui
            .interact(meter_rect, ui.scope_id().with("meter"), Sense::click())
            .on_hover_text("Time signature at the playhead");
        if let Some(text) = typing_meter {
            draw_entry(&painter, meter_rect, text, theme, &value_font);
        } else {
            let hot = meter_response.hovered();
            if hot {
                painter.rect_stroke(meter_rect, CornerRadius::same(2), Stroke::new(1.0, theme.color("bar_line")), egui::StrokeKind::Inside);
            }
            let (n, d) = view.meter;
            painter.text(Pos2::new(x, mid), Align2::LEFT_CENTER, format!("{n}/{d}"), value_font.clone(), theme.color("status_fg"));
            let caret = Pos2::new(meter_rect.right() - 9.0, mid);
            let ink = theme.color(if hot { "status_fg" } else { "status_dim" });
            painter.add(Shape::convex_polygon(
                vec![caret + Vec2::new(-4.0, -2.0), caret + Vec2::new(4.0, -2.0), caret + Vec2::new(0.0, 3.0)],
                ink,
                Stroke::NONE,
            ));
        }
        egui::Popup::menu(&meter_response).show(|ui| {
            ui.style_mut().override_font_id = Some(small_font.clone());
            for &(numerator, denominator) in COMMON_METERS {
                let current = view.meter == (numerator, denominator);
                if ui.selectable_label(current, format!("{numerator}/{denominator}")).clicked() {
                    actions.push(TransportAction::SetMeter { numerator, denominator });
                }
            }
            ui.separator();
            if ui.button("Other…").clicked() {
                actions.push(TransportAction::Edit(ValueField::Meter));
            }
        });
        x = meter_rect.right() + SECTION_GAP;

        if !view.audio_ok {
            separator(&mut x);
            painter.text(Pos2::new(x, mid), Align2::LEFT_CENTER, "no audio", small_font.clone(), theme.color("error"));
        }
        ui.allocate_rect(rect, Sense::hover());
    }
}

fn text_width(painter: &Painter, text: &str, font: &FontId) -> f32 {
    painter.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x
}

/// Draws `text` left-aligned in a slot as wide as `widest`, so the numbers after it
/// stay put as the value changes. Returns the slot's right edge.
fn fixed_text(painter: &Painter, x: f32, mid: f32, text: &str, widest: &str, font: &FontId, color: Color32) -> f32 {
    painter.text(Pos2::new(x, mid), Align2::LEFT_CENTER, text, font.clone(), color);
    x + text_width(painter, widest, font)
}

/// A value being typed: the text so far and a caret, in an outlined box.
fn draw_entry(painter: &Painter, rect: Rect, text: &str, theme: &Theme, font: &FontId) {
    painter.rect_filled(rect, CornerRadius::same(2), theme.color("background"));
    painter.rect_stroke(rect, CornerRadius::same(2), Stroke::new(1.0, theme.color("accent")), egui::StrokeKind::Inside);
    painter.text(Pos2::new(rect.left() + 4.0, rect.center().y), Align2::LEFT_CENTER, format!("{text}▏"), font.clone(), theme.color("status_fg"));
}

fn draw_play(painter: &Painter, r: Rect, ink: Color32) {
    let r = r.shrink2(Vec2::new(1.0, 0.0));
    painter.add(Shape::convex_polygon(vec![r.left_top(), r.right_center(), r.left_bottom()], ink, Stroke::NONE));
}

fn draw_stop(painter: &Painter, r: Rect, ink: Color32) {
    painter.rect_filled(Rect::from_center_size(r.center(), Vec2::splat(r.height())), CornerRadius::same(1), ink);
}

fn draw_to_start(painter: &Painter, r: Rect, ink: Color32) {
    let bar = Rect::from_min_max(r.left_top(), Pos2::new(r.left() + 2.0, r.bottom()));
    painter.rect_filled(bar, 0.0, ink);
    let tip = Pos2::new(bar.right() + 1.0, r.center().y);
    painter.add(Shape::convex_polygon(vec![tip, r.right_top(), r.right_bottom()], ink, Stroke::NONE));
}

/// A circle of arrow, open at the top right where the arrowhead sits.
fn draw_loop(painter: &Painter, r: Rect, ink: Color32) {
    let centre = r.center();
    let radius = r.height() / 2.0;
    let start = -0.25f32;
    let end = std::f32::consts::TAU - 1.1;
    let steps = 24;
    let at = |a: f32| centre + Vec2::new(a.cos(), a.sin()) * radius;
    let arc: Vec<Pos2> = (0..=steps).map(|k| at(start + (end - start) * k as f32 / steps as f32)).collect();
    painter.add(Shape::line(arc, Stroke::new(1.6, ink)));
    let tip = at(end);
    let along = Vec2::new(-end.sin(), end.cos());
    let out = Vec2::new(end.cos(), end.sin());
    painter.add(Shape::convex_polygon(
        vec![tip + along * 3.5, tip - out * 3.0 - along * 0.5, tip + out * 3.0 - along * 0.5],
        ink,
        Stroke::NONE,
    ));
}

/// A metronome's body with its arm swung to one side.
fn draw_metronome(painter: &Painter, r: Rect, ink: Color32) {
    let top = Pos2::new(r.center().x, r.top());
    let body = vec![top + Vec2::new(-2.0, 0.0), top + Vec2::new(2.0, 0.0), r.right_bottom(), r.left_bottom()];
    painter.add(Shape::closed_line(body, Stroke::new(1.4, ink)));
    let pivot = Pos2::new(r.center().x, r.bottom() - 2.5);
    painter.line_segment([pivot, Pos2::new(r.right() + 1.0, r.top() + 1.0)], Stroke::new(1.4, ink));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_up_or_right_speeds_the_tempo_up() {
        assert_eq!(bpm_after_drag(120.0, Vec2::new(0.0, -30.0)), 130.0);
        assert_eq!(bpm_after_drag(120.0, Vec2::new(30.0, 0.0)), 130.0);
        assert_eq!(bpm_after_drag(120.0, Vec2::new(0.0, 30.0)), 110.0);
        assert_eq!(bpm_after_drag(25.0, Vec2::new(0.0, 300.0)), MIN_BPM);
        assert_eq!(bpm_after_drag(395.0, Vec2::new(0.0, -300.0)), MAX_BPM);
    }

    #[test]
    fn whole_tempos_drop_the_fraction() {
        assert_eq!(bpm_label(120.0), "120");
        assert_eq!(bpm_label(90.5), "90.5");
        assert_eq!(bpm_label(119.98), "120");
    }
}
