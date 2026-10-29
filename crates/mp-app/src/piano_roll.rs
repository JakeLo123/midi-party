//! Draws the piano roll (key column, bar ruler, grid, notes, selection box,
//! cursor, playhead) and turns mouse gestures on it into [`RollAction`]s.
//! All editing state lives in the `Editor`; the app applies the actions.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rangef, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use mp_core::{percussion_short, pitch_name, Editor, LaneKind, NoteId, Tick, Track};

use crate::theme::Theme;

const KEY_COLUMN_W: f32 = 56.0;
const RULER_H: f32 = 20.0;
const BLACK_KEYS: [u8; 5] = [1, 3, 6, 8, 10];
const MIN_GRID_PX: f32 = 5.0;
/// Grabbing this close to a note's right edge resizes instead of moving.
const RESIZE_HANDLE_PX: f32 = 7.0;
/// Height of the controller-lane strip under the roll when a lane is shown.
const LANE_STRIP_H: f32 = 90.0;

/// What a mouse gesture asks the app to do. Ticks are absolute; deltas are in
/// whole grid steps / semitones so drags stay on the grid.
#[derive(Debug, Clone, PartialEq)]
pub enum RollAction {
    /// Click on empty space: move the cursor there (and clear the selection).
    SetCursor { tick: u64, pitch: u8 },
    /// Click on a note: select just it (or toggle it with Shift) and move the cursor onto it.
    ClickNote { id: NoteId, toggle: bool },
    /// Double-click on empty space: insert a note there.
    InsertAt { tick: u64, pitch: u8 },
    /// Dragging selected notes crossed grid steps / semitone rows.
    MoveSelection { grid_steps: i64, semitones: i32 },
    /// Dragging a note's right edge crossed grid steps.
    ResizeSelection { grid_steps: i32 },
    /// A rubber-band box: notes starting in [t0, t1) with pitch in [p_lo, p_hi].
    BoxSelect { t0: u64, t1: u64, p_lo: u8, p_hi: u8, add: bool },
    /// Click in the ruler: move the playhead.
    SetPlayhead { tick: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragKind {
    Move,
    Resize,
    Box,
    Scrub,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    kind: DragKind,
    /// Where the pointer went down, in (tick, pitch) space.
    origin: (f64, i32),
    /// Grid steps / semitones already applied to the selection during this drag.
    applied: (i64, i32),
    /// Whether Shift was held when the drag started (box select adds).
    add: bool,
}

/// Scroll and zoom state of the roll.
pub struct RollView {
    pub px_per_tick: f32,
    pub row_h: f32,
    /// Tick at the left edge of the note area.
    pub left_tick: f64,
    /// Pitch shown on the top row.
    pub top_pitch: u8,
    drag: Option<Drag>,
    last_box: Option<Rect>,
}

impl Default for RollView {
    fn default() -> Self {
        Self { px_per_tick: 0.1, row_h: 14.0, left_tick: 0.0, top_pitch: 84, drag: None, last_box: None }
    }
}

impl RollView {
    pub fn zoom_time(&mut self, factor: f32) {
        self.px_per_tick = (self.px_per_tick * factor).clamp(0.005, 2.0);
    }

    pub fn zoom_rows(&mut self, factor: f32) {
        self.row_h = (self.row_h * factor).clamp(6.0, 40.0);
    }

    fn note_area(rect: Rect) -> Rect {
        Rect::from_min_max(Pos2::new(rect.left() + KEY_COLUMN_W, rect.top() + RULER_H), rect.max)
    }

    fn x_of(&self, area: Rect, tick: f64) -> f32 {
        area.left() + ((tick - self.left_tick) * self.px_per_tick as f64) as f32
    }

    fn tick_at_x(&self, area: Rect, x: f32) -> f64 {
        (self.left_tick + ((x - area.left()) / self.px_per_tick) as f64).max(0.0)
    }

    fn tick_at_right(&self, area: Rect) -> f64 {
        self.left_tick + (area.width() / self.px_per_tick) as f64
    }

    /// Top edge of the row for `pitch`.
    fn y_of(&self, area: Rect, pitch: i32) -> f32 {
        area.top() + (self.top_pitch as i32 - pitch) as f32 * self.row_h
    }

    fn pitch_at_y(&self, area: Rect, y: f32) -> i32 {
        (self.top_pitch as i32 - ((y - area.top()) / self.row_h).floor() as i32).clamp(0, 127)
    }

    fn rows_visible(&self, area: Rect) -> i32 {
        (area.height() / self.row_h).ceil() as i32
    }

    /// Scrolls so the cursor cell is on screen (with a margin).
    pub fn ensure_cursor_visible(&mut self, editor: &Editor, rect: Rect) {
        if !rect.is_positive() {
            return; // nothing drawn yet
        }
        let area = Self::note_area(rect);
        if area.width() <= 0.0 || area.height() <= 0.0 {
            return;
        }
        let cell_start = editor.cursor.tick.get() as f64;
        let cell_end = cell_start + editor.grid_ticks().get() as f64;
        let width_ticks = (area.width() / self.px_per_tick) as f64;
        let margin = width_ticks * 0.1;
        if cell_start < self.left_tick + margin && self.left_tick > 0.0 {
            self.left_tick = (cell_start - margin).max(0.0);
        } else if cell_end > self.left_tick + width_ticks - margin {
            self.left_tick = (cell_end - width_ticks + margin).max(0.0);
        }
        if cell_start < self.left_tick {
            self.left_tick = cell_start;
        }

        let rows = self.rows_visible(area);
        let pitch = editor.cursor.pitch as i32;
        let bottom_pitch = self.top_pitch as i32 - rows + 1;
        if pitch > self.top_pitch as i32 - 2 {
            self.top_pitch = (pitch + 2).min(127) as u8;
        } else if pitch < bottom_pitch + 2 {
            self.top_pitch = (pitch - 2 + rows - 1).clamp(rows - 1, 127) as u8;
        }
    }

    /// Keeps the playhead in view while playing.
    pub fn follow_playhead(&mut self, tick: f64, rect: Rect) {
        if !rect.is_positive() || !tick.is_finite() {
            return;
        }
        let area = Self::note_area(rect);
        if area.width() <= 0.0 {
            return;
        }
        let right = self.tick_at_right(area);
        let width = right - self.left_tick;
        if tick > self.left_tick + width * 0.9 || tick < self.left_tick {
            self.left_tick = (tick - width * 0.1).max(0.0);
        }
    }

    /// The tick range the note area is showing, once the roll has been laid out.
    pub fn visible_ticks(&self, rect: Rect) -> Option<(f64, f64)> {
        if !rect.is_positive() {
            return None;
        }
        let area = Self::note_area(rect);
        if area.width() <= 0.0 {
            return None;
        }
        Some((self.left_tick, self.tick_at_right(area)))
    }

    fn scroll_by(&mut self, dx_px: f32, dy_px: f32, area: Rect) {
        if dx_px != 0.0 {
            self.left_tick = (self.left_tick - (dx_px / self.px_per_tick) as f64).max(0.0);
        }
        if dy_px != 0.0 {
            let rows = (dy_px / self.row_h).round() as i32;
            let visible = self.rows_visible(area);
            self.top_pitch = (self.top_pitch as i32 + rows).clamp(visible.min(127) - 1, 127) as u8;
        }
    }

    /// Draws everything and reports mouse gestures. Returns the rect used.
    pub fn draw(
        &mut self,
        ui: &mut Ui,
        editor: &Editor,
        theme: &Theme,
        playhead_tick: Option<f64>,
        actions: &mut Vec<RollAction>,
    ) -> Rect {
        if !self.left_tick.is_finite() || self.left_tick < 0.0 {
            self.left_tick = 0.0;
        }
        let full = ui.available_rect_before_wrap();
        let painter = ui.painter_at(full);
        painter.rect_filled(full, 0.0, theme.color("background"));
        let font = FontId::monospace((theme.font_size - 2.0).max(8.0));
        let rect = match editor.lane {
            Some(_) if full.height() > LANE_STRIP_H * 2.0 => {
                Rect::from_min_max(full.min, Pos2::new(full.right(), full.bottom() - LANE_STRIP_H))
            }
            _ => full,
        };
        let area = Self::note_area(rect);
        if area.width() <= 0.0 || area.height() <= 0.0 {
            return full;
        }
        if let (Some(kind), true) = (editor.lane, rect != full) {
            let strip = Rect::from_min_max(Pos2::new(full.left(), rect.bottom()), full.max);
            self.draw_lane_strip(&painter.with_clip_rect(strip), strip, area, editor, kind, theme, &font);
        }

        self.handle_mouse(ui, rect, area, editor, actions);

        self.draw_rows(&painter.with_clip_rect(area), area, theme);
        self.draw_grid_lines(&painter.with_clip_rect(area), area, editor, theme);
        self.draw_notes(&painter.with_clip_rect(area), area, editor, theme);
        self.draw_selection_box(&painter.with_clip_rect(area), area, editor, theme);
        if let Some(r) = self.last_box {
            let p = painter.with_clip_rect(area);
            p.rect_filled(r, 0.0, theme.color("cursor_fill"));
            p.rect_stroke(r, 0.0, Stroke::new(1.0, theme.color("note_selected")), StrokeKind::Inside);
        }
        self.draw_cursor(&painter.with_clip_rect(area), area, editor, theme);
        if let Some(tick) = playhead_tick {
            let x = self.x_of(area, tick);
            painter.with_clip_rect(area).vline(x, area.y_range(), Stroke::new(2.0, theme.color("playhead")));
        }
        if editor.track().is_some_and(Track::is_drums) {
            self.draw_drum_column(&painter, rect, area, theme, &font);
        } else {
            self.draw_key_column(&painter, rect, area, theme, &font);
        }
        self.draw_ruler(&painter, rect, area, editor, theme, &font);
        if let Some(tick) = playhead_tick {
            let ruler = Rect::from_min_max(Pos2::new(area.left(), rect.top()), Pos2::new(rect.right(), area.top()));
            let x = self.x_of(area, tick);
            let color = theme.color("playhead");
            let p = painter.with_clip_rect(ruler);
            p.vline(x, ruler.y_range(), Stroke::new(2.0, color));
            draw_playhead_marker(&p, x, ruler.bottom(), color);
        }
        rect
    }

    fn note_hit(&self, area: Rect, editor: &Editor, pos: Pos2) -> Option<(NoteId, bool)> {
        let track = editor.track()?;
        let tick = self.tick_at_x(area, pos.x) as u64;
        let pitch = self.pitch_at_y(area, pos.y) as u8;
        let note = track.notes.iter().rev().find(|n| n.pitch == pitch && n.start.get() <= tick && tick < n.end().get())?;
        let right_edge = self.x_of(area, note.end().get() as f64);
        Some((note.id, right_edge - pos.x <= RESIZE_HANDLE_PX))
    }

    fn handle_mouse(&mut self, ui: &mut Ui, rect: Rect, area: Rect, editor: &Editor, actions: &mut Vec<RollAction>) {
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let grid = editor.grid_ticks().get().max(1);
        let snap = |tick: f64| (tick as u64 / grid) * grid;

        if response.hovered() {
            let (scroll, zoom, cmd) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.mac_cmd));
            if zoom != 1.0 {
                self.zoom_time(zoom);
            } else if cmd && scroll.y != 0.0 {
                self.zoom_time(if scroll.y > 0.0 { 1.1 } else { 1.0 / 1.1 });
            } else {
                self.scroll_by(scroll.x, scroll.y, area);
            }
        }

        let Some(pos) = response.interact_pointer_pos() else {
            if response.drag_stopped() {
                self.finish_drag(area, editor, actions, None);
            }
            return;
        };
        let shift = ui.input(|i| i.modifiers.shift);
        let in_ruler = pos.y < area.top() && pos.x >= area.left();
        let in_area = area.contains(pos);

        if response.double_clicked() && in_area && self.note_hit(area, editor, pos).is_none() {
            actions.push(RollAction::InsertAt { tick: snap(self.tick_at_x(area, pos.x)), pitch: self.pitch_at_y(area, pos.y) as u8 });
            return;
        }
        if response.clicked() {
            if in_ruler {
                actions.push(RollAction::SetPlayhead { tick: snap(self.tick_at_x(area, pos.x)) });
            } else if in_area {
                match self.note_hit(area, editor, pos) {
                    Some((id, _)) => actions.push(RollAction::ClickNote { id, toggle: shift }),
                    None => actions.push(RollAction::SetCursor {
                        tick: snap(self.tick_at_x(area, pos.x)),
                        pitch: self.pitch_at_y(area, pos.y) as u8,
                    }),
                }
            }
            return;
        }

        if response.drag_started() && in_ruler {
            self.drag = Some(Drag { kind: DragKind::Scrub, origin: (0.0, 0), applied: (0, 0), add: false });
        } else if response.drag_started() && in_area {
            let origin = (self.tick_at_x(area, pos.x), self.pitch_at_y(area, pos.y));
            let kind = match self.note_hit(area, editor, pos) {
                Some((id, on_edge)) => {
                    if !editor.selection.contains(&id) {
                        actions.push(RollAction::ClickNote { id, toggle: false });
                    }
                    if on_edge {
                        DragKind::Resize
                    } else {
                        DragKind::Move
                    }
                }
                None => DragKind::Box,
            };
            self.drag = Some(Drag { kind, origin, applied: (0, 0), add: shift });
        }

        if response.dragged() {
            if let Some(mut drag) = self.drag {
                let tick = self.tick_at_x(area, pos.x);
                let pitch = self.pitch_at_y(area, pos.y);
                match drag.kind {
                    DragKind::Scrub => actions.push(RollAction::SetPlayhead { tick: snap(tick) }),
                    DragKind::Move | DragKind::Resize => {
                        let steps = ((tick - drag.origin.0) / grid as f64).round() as i64;
                        let semis = if drag.kind == DragKind::Move { pitch - drag.origin.1 } else { 0 };
                        let d_steps = steps - drag.applied.0;
                        let d_semis = semis - drag.applied.1;
                        if d_steps != 0 || d_semis != 0 {
                            if drag.kind == DragKind::Move {
                                actions.push(RollAction::MoveSelection { grid_steps: d_steps, semitones: d_semis });
                            } else {
                                actions.push(RollAction::ResizeSelection { grid_steps: d_steps as i32 });
                            }
                            drag.applied = (steps, semis);
                        }
                    }
                    DragKind::Box => {
                        let x0 = self.x_of(area, drag.origin.0.min(tick));
                        let x1 = self.x_of(area, drag.origin.0.max(tick));
                        let y0 = self.y_of(area, drag.origin.1.max(pitch));
                        let y1 = self.y_of(area, drag.origin.1.min(pitch)) + self.row_h;
                        self.last_box = Some(Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1)));
                    }
                }
                self.drag = Some(drag);
            }
        }

        if response.drag_stopped() {
            self.finish_drag(area, editor, actions, Some(pos));
        }
    }

    fn finish_drag(&mut self, area: Rect, _editor: &Editor, actions: &mut Vec<RollAction>, pos: Option<Pos2>) {
        if let Some(drag) = self.drag.take() {
            if drag.kind == DragKind::Box {
                if let Some(pos) = pos {
                    let tick = self.tick_at_x(area, pos.x);
                    let pitch = self.pitch_at_y(area, pos.y);
                    let (t0, t1) = (drag.origin.0.min(tick) as u64, drag.origin.0.max(tick) as u64 + 1);
                    let (p_lo, p_hi) = (drag.origin.1.min(pitch) as u8, drag.origin.1.max(pitch) as u8);
                    actions.push(RollAction::BoxSelect { t0, t1, p_lo, p_hi, add: drag.add });
                }
            }
        }
        self.last_box = None;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_lane_strip(&self, painter: &Painter, strip: Rect, area: Rect, editor: &Editor, kind: LaneKind, theme: &Theme, font: &FontId) {
        painter.rect_filled(strip, 0.0, theme.color("status_bg"));
        painter.hline(strip.x_range(), strip.top() + 0.5, Stroke::new(1.0, theme.color("bar_line")));
        let Some(track) = editor.track() else { return };
        let (lo, hi) = kind.range();
        let plot = Rect::from_min_max(Pos2::new(area.left(), strip.top() + 6.0), Pos2::new(strip.right(), strip.bottom() - 4.0));
        let y_of = |value: i32| plot.bottom() - (value - lo) as f32 / (hi - lo) as f32 * plot.height();
        // Baseline: zero for pitch bend, bottom otherwise.
        let base = y_of(if kind == LaneKind::PitchBend { 0 } else { lo });
        painter.hline(plot.x_range(), base, Stroke::new(1.0, theme.color("grid")));
        let points = track.lanes.get(kind);
        let left = self.left_tick as u64;
        let right = self.tick_at_right(area) as u64;
        let color = theme.color("accent");
        let mut prev: Option<(f32, f32)> = points
            .iter()
            .rev()
            .find(|p| p.tick.get() < left)
            .map(|p| (plot.left(), y_of(kind.clamp(p.value))));
        for p in points {
            if p.tick.get() > right {
                break;
            }
            let x = self.x_of(area, p.tick.get() as f64);
            let y = y_of(kind.clamp(p.value));
            if let Some((px, py)) = prev {
                painter.hline(Rangef::new(px, x), py, Stroke::new(2.0, color));
            }
            if p.tick.get() >= left {
                painter.vline(x, Rangef::new(y.min(base), y.max(base)), Stroke::new(2.0, color));
                painter.circle_filled(Pos2::new(x, y), 2.5, color);
            }
            prev = Some((x, y));
        }
        if let Some((px, py)) = prev {
            painter.hline(Rangef::new(px, plot.right()), py, Stroke::new(2.0, color));
        }
        let cx = self.x_of(area, editor.cursor.tick.get() as f64);
        painter.vline(cx, plot.y_range(), Stroke::new(1.0, theme.color("cursor")));
        let value = track.lanes.value_at(kind, editor.cursor.tick);
        painter.text(
            Pos2::new(strip.left() + 6.0, strip.top() + 4.0),
            Align2::LEFT_TOP,
            format!("{kind}\n{value}"),
            font.clone(),
            theme.color("status_fg"),
        );
    }

    fn draw_rows(&self, painter: &Painter, area: Rect, theme: &Theme) {
        let rows = self.rows_visible(area);
        for row in 0..=rows {
            let pitch = self.top_pitch as i32 - row;
            if !(0..=127).contains(&pitch) {
                continue;
            }
            let y = self.y_of(area, pitch);
            if BLACK_KEYS.contains(&((pitch % 12) as u8)) {
                let r = Rect::from_min_size(Pos2::new(area.left(), y), Vec2::new(area.width(), self.row_h));
                painter.rect_filled(r, 0.0, theme.color("black_key_row"));
            }
            let color = if pitch % 12 == 11 { "octave_line" } else { "grid" };
            painter.hline(area.x_range(), y, Stroke::new(1.0, theme.color(color)));
        }
    }

    fn draw_grid_lines(&self, painter: &Painter, area: Rect, editor: &Editor, theme: &Theme) {
        let song = &editor.song;
        let grid = editor.grid_ticks().get().max(1);
        let right = self.tick_at_right(area) as u64 + grid;
        let left = self.left_tick as u64;
        let beat = song.meter_map.beat_ticks_at(song.ppq, Tick(left)).get().max(1);
        let bar = song.meter_map.bar_ticks_at(song.ppq, Tick(left)).get().max(1);
        let draw_plain = grid as f32 * self.px_per_tick >= MIN_GRID_PX;
        let step = if draw_plain { grid } else { beat };
        let mut tick = (left / step) * step;
        while tick <= right {
            let color = if tick.is_multiple_of(bar) {
                "bar_line"
            } else if tick.is_multiple_of(beat) {
                "beat_line"
            } else {
                "grid"
            };
            let x = self.x_of(area, tick as f64);
            painter.vline(x, area.y_range(), Stroke::new(1.0, theme.color(color)));
            tick += step;
        }
    }

    fn note_rect(&self, area: Rect, start: u64, end: u64, pitch: u8) -> Rect {
        let x0 = self.x_of(area, start as f64);
        let x1 = self.x_of(area, end as f64).max(x0 + 2.0);
        let y = self.y_of(area, pitch as i32);
        Rect::from_min_max(Pos2::new(x0, y + 1.0), Pos2::new(x1 - 1.0, y + self.row_h - 1.0))
    }

    fn draw_notes(&self, painter: &Painter, area: Rect, editor: &Editor, theme: &Theme) {
        let Some(track) = editor.track() else { return };
        let under_cursor = editor.note_under_cursor().map(|n| n.id);
        let left = self.left_tick as u64;
        let right = self.tick_at_right(area) as u64;
        for note in &track.notes {
            if note.end().get() < left || note.start.get() > right {
                continue;
            }
            let r = self.note_rect(area, note.start.get(), note.end().get(), note.pitch);
            let selected = editor.selection.contains(&note.id);
            let base = theme.color(if selected { "note_selected" } else { "note" });
            let alpha = 110 + (note.velocity as u32 * 145 / 127) as u8;
            let fill = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha);
            painter.rect_filled(r, 2.0, fill);
            let outline = if under_cursor == Some(note.id) || editor.focus == Some(note.id) {
                Stroke::new(1.5, theme.color("cursor"))
            } else {
                Stroke::new(1.0, base.gamma_multiply(0.6))
            };
            painter.rect_stroke(r, 2.0, outline, StrokeKind::Inside);
        }
    }

    fn draw_selection_box(&self, painter: &Painter, area: Rect, editor: &Editor, theme: &Theme) {
        let Some(anchor) = editor.box_anchor else { return };
        let grid = editor.grid_ticks().get();
        let t0 = anchor.tick.get().min(editor.cursor.tick.get());
        let t1 = anchor.tick.get().max(editor.cursor.tick.get()) + grid;
        let p_hi = anchor.pitch.max(editor.cursor.pitch) as i32;
        let p_lo = anchor.pitch.min(editor.cursor.pitch) as i32;
        let r = Rect::from_min_max(
            Pos2::new(self.x_of(area, t0 as f64), self.y_of(area, p_hi)),
            Pos2::new(self.x_of(area, t1 as f64), self.y_of(area, p_lo) + self.row_h),
        );
        painter.rect_filled(r, 0.0, theme.color("cursor_fill"));
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, theme.color("note_selected")), StrokeKind::Inside);
    }

    fn draw_cursor(&self, painter: &Painter, area: Rect, editor: &Editor, theme: &Theme) {
        let start = editor.cursor.tick.get();
        let end = start + editor.grid_ticks().get();
        let r = self.note_rect(area, start, end, editor.cursor.pitch).expand(1.0);
        painter.rect_filled(r, 0.0, theme.color("cursor_fill"));
        painter.rect_stroke(r, 0.0, Stroke::new(2.0, theme.color("cursor")), StrokeKind::Outside);
    }

    /// The key column of a drum track: no piano keys, one drum name per mapped row.
    fn draw_drum_column(&self, painter: &Painter, rect: Rect, area: Rect, theme: &Theme, font: &FontId) {
        let column = Rect::from_min_max(Pos2::new(rect.left(), area.top()), Pos2::new(area.left(), rect.bottom()));
        let painter = painter.with_clip_rect(column);
        painter.rect_filled(column, 0.0, theme.color("white_key"));
        let rows = self.rows_visible(area);
        for row in 0..=rows {
            let pitch = self.top_pitch as i32 - row;
            if !(0..=127).contains(&pitch) {
                continue;
            }
            let y = self.y_of(area, pitch);
            painter.hline(column.x_range(), y + self.row_h - 0.5, Stroke::new(1.0, theme.color("grid")));
            if self.row_h >= 9.0 {
                if let Some(label) = percussion_short(pitch as u8) {
                    painter.text(
                        Pos2::new(column.right() - 4.0, y + self.row_h / 2.0),
                        Align2::RIGHT_CENTER,
                        label,
                        font.clone(),
                        theme.color("key_label"),
                    );
                }
            }
        }
        painter.vline(area.left() - 0.5, column.y_range(), Stroke::new(1.0, theme.color("bar_line")));
    }

    fn draw_key_column(&self, painter: &Painter, rect: Rect, area: Rect, theme: &Theme, font: &FontId) {
        let column = Rect::from_min_max(Pos2::new(rect.left(), area.top()), Pos2::new(area.left(), rect.bottom()));
        let painter = painter.with_clip_rect(column);
        painter.rect_filled(column, 0.0, theme.color("white_key"));
        let rows = self.rows_visible(area);
        let black_w = (column.width() * 0.62).round();
        // White keys first (with their dividers), then the black keys on top, like a keyboard.
        for row in 0..=rows {
            let pitch = self.top_pitch as i32 - row;
            if !(0..=127).contains(&pitch) {
                continue;
            }
            let y = self.y_of(area, pitch);
            let black = BLACK_KEYS.contains(&((pitch % 12) as u8));
            if black {
                let r = Rect::from_min_size(Pos2::new(column.left(), y), Vec2::new(black_w, self.row_h));
                painter.rect_filled(r, 1.0, theme.color("black_key"));
                // The two white keys on either side meet under the black key.
                let divider = Rangef::new(column.left() + black_w, column.right());
                painter.hline(divider, (y + self.row_h / 2.0).round() + 0.5, Stroke::new(1.0, theme.color("black_key")));
            } else if pitch % 12 == 0 || pitch % 12 == 5 {
                // The B/C and E/F edges are where two white keys meet.
                painter.hline(column.x_range(), y + self.row_h - 0.5, Stroke::new(1.0, theme.color("black_key")));
            }
            if pitch % 12 == 0 && self.row_h >= 9.0 {
                painter.text(
                    Pos2::new(column.right() - 4.0, y + self.row_h / 2.0),
                    Align2::RIGHT_CENTER,
                    pitch_name(pitch as u8),
                    font.clone(),
                    theme.color("key_label"),
                );
            }
        }
        painter.vline(area.left() - 0.5, column.y_range(), Stroke::new(1.0, theme.color("bar_line")));
    }

    fn draw_ruler(&self, painter: &Painter, rect: Rect, area: Rect, editor: &Editor, theme: &Theme, font: &FontId) {
        let ruler = Rect::from_min_max(Pos2::new(area.left(), rect.top()), Pos2::new(rect.right(), area.top()));
        let painter = painter.with_clip_rect(ruler);
        painter.rect_filled(ruler, 0.0, theme.color("status_bg"));
        let song = &editor.song;
        let right = self.tick_at_right(area);
        let mut bar = song.meter_map.bar_beat_of(song.ppq, Tick(self.left_tick as u64)).bar;
        loop {
            let tick = song.meter_map.tick_of_bar(song.ppq, bar).get() as f64;
            if tick > right {
                break;
            }
            let x = self.x_of(area, tick);
            painter.vline(x, Rangef::new(ruler.top() + ruler.height() * 0.5, ruler.bottom()), Stroke::new(1.0, theme.color("bar_line")));
            painter.text(
                Pos2::new(x + 3.0, ruler.center().y),
                Align2::LEFT_CENTER,
                bar.to_string(),
                font.clone(),
                theme.color("status_dim"),
            );
            bar += 1;
            if bar > 100_000 {
                break;
            }
        }
        painter.hline(ruler.x_range(), area.top() - 0.5, Stroke::new(1.0, theme.color("bar_line")));
    }
}

/// The playhead's handle in a ruler: a small triangle pointing down at `bottom`.
pub(crate) fn draw_playhead_marker(painter: &Painter, x: f32, bottom: f32, color: Color32) {
    let x = x.round();
    let points = vec![Pos2::new(x - 6.0, bottom - 8.0), Pos2::new(x + 6.0, bottom - 8.0), Pos2::new(x, bottom)];
    painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
}
