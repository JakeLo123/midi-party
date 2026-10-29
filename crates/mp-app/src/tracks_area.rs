//! The tracks area: every track as a row with its notes drawn small across the
//! whole song, the cursor and playhead over all of them. It has its own zoom and
//! scroll, independent of the piano roll, and starts fitted to the song.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rangef, Rect, Sense, Stroke, Ui, Vec2};
use mp_core::{pan_label, Editor, Note, Song, Tick};

use crate::theme::Theme;

const HEADER_W: f32 = 190.0;
const ROW_H: f32 = 40.0;
const RULER_H: f32 = 16.0;
/// The fitted view never shows fewer bars than this, so an empty song still has a map.
const MIN_FIT_BARS: u32 = 16;
/// A lane never squeezes fewer pitch rows than this into its height.
const MIN_PITCH_ROWS: u8 = 12;
const MIN_PX_PER_TICK: f32 = 0.0005;
const MAX_PX_PER_TICK: f32 = 2.0;

/// What a click on the tracks area asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum TracksAction {
    /// Click on a row header: edit that track.
    ShowTrack { track: usize },
    /// Click on a lane: edit that track with the cursor at that tick.
    JumpTo { track: usize, tick: u64 },
    /// Click in the ruler: move the playhead.
    SetPlayhead { tick: u64 },
    /// A drag along the ruler began: pause playback until it ends.
    ScrubStart,
    /// The ruler drag ended: resume playback if the drag paused it.
    ScrubEnd,
    /// Click on the instrument line of a row header: choose that track's sound.
    PickInstrument { track: usize },
}

/// Whether a click in a row header landed on its instrument line (the lower half).
fn header_instrument_hit(header: Rect, pos: Pos2) -> bool {
    pos.y >= header.center().y
}

pub struct TracksArea {
    px_per_tick: f32,
    left_tick: f64,
    /// While set, every frame re-fits the view to the song so the map grows with it.
    fitted_to_song: bool,
    /// Lane width from the last draw, so follow logic can run between frames.
    lanes_width_px: f32,
    /// The grid cell the current ruler drag last sent the playhead to, so a drag that
    /// stays in one cell does not re-seek every frame while the engine plays on.
    scrub_tick: Option<u64>,
    /// The track whose row was last scrolled into view.
    shown_track: Option<usize>,
}

impl Default for TracksArea {
    fn default() -> Self {
        Self { px_per_tick: 0.01, left_tick: 0.0, fitted_to_song: true, lanes_width_px: 0.0, scrub_tick: None, shown_track: None }
    }
}

/// The tick the fitted view ends at: the song end or the cursor, whichever is later,
/// rounded up to the next bar start, and at least `MIN_FIT_BARS` bars.
fn fit_end_tick(song: &Song, cursor_tick: Tick) -> Tick {
    let end = song.end().max(cursor_tick);
    let at = song.meter_map.bar_beat_of(song.ppq, end);
    let bar_start = song.meter_map.tick_of_bar(song.ppq, at.bar);
    let rounded = if bar_start == end { end } else { song.meter_map.tick_of_bar(song.ppq, at.bar + 1) };
    rounded.max(song.meter_map.tick_of_bar(song.ppq, MIN_FIT_BARS + 1))
}

/// The inclusive pitch range a lane shows: the notes' range widened to at least
/// `MIN_PITCH_ROWS` rows, kept inside 0..=127.
fn pitch_span(notes: &[Note]) -> (u8, u8) {
    let (lo, hi) = notes
        .iter()
        .map(|n| n.pitch)
        .fold(None, |acc: Option<(u8, u8)>, p| match acc {
            None => Some((p, p)),
            Some((lo, hi)) => Some((lo.min(p), hi.max(p))),
        })
        .unwrap_or((60, 60));
    let rows = i32::from(hi) - i32::from(lo) + 1;
    let missing = (i32::from(MIN_PITCH_ROWS) - rows).max(0);
    let mut lo = i32::from(lo) - missing / 2;
    let mut hi = i32::from(hi) + (missing - missing / 2);
    if lo < 0 {
        hi -= lo;
        lo = 0;
    }
    if hi > 127 {
        lo -= hi - 127;
        hi = 127;
    }
    (lo.max(0) as u8, hi.min(127) as u8)
}

/// Where a note from `x0` to `x1` at `pitch` lands in `lane`, low pitches at the bottom.
/// Never thinner or shorter than a pixel so every note shows up.
fn note_rect(lane: Rect, x0: f32, x1: f32, pitch: u8, span: (u8, u8)) -> Rect {
    let (lo, hi) = span;
    let rows = f32::from(hi) - f32::from(lo) + 1.0;
    let row_h = lane.height() / rows;
    let bottom = lane.bottom() - (f32::from(pitch.clamp(lo, hi)) - f32::from(lo)) * row_h;
    let h = row_h.max(1.0);
    let w = (x1 - x0).max(1.0);
    Rect::from_min_size(Pos2::new(x0, bottom - h), Vec2::new(w, h))
}

impl TracksArea {
    pub fn fit_to_song(&mut self) {
        self.fitted_to_song = true;
        self.left_tick = 0.0;
    }

    /// Zooms so the tick under `anchor_x_px` (measured from the lanes' left edge) stays put.
    pub fn zoom_time(&mut self, factor: f32, anchor_tick: f64, anchor_x_px: f32) {
        self.px_per_tick = (self.px_per_tick * factor).clamp(MIN_PX_PER_TICK, MAX_PX_PER_TICK);
        self.left_tick = (anchor_tick - (anchor_x_px / self.px_per_tick) as f64).max(0.0);
        self.fitted_to_song = false;
    }

    pub fn scroll_by(&mut self, dx_px: f32) {
        self.left_tick = (self.left_tick - (dx_px / self.px_per_tick) as f64).max(0.0);
        self.fitted_to_song = false;
    }

    /// Scrolls so `tick` is in view when zoomed in. Does nothing while the whole
    /// song is fitted (everything is visible) or before the first draw.
    pub fn keep_tick_visible(&mut self, tick: f64) {
        if self.fitted_to_song || self.lanes_width_px <= 0.0 || !tick.is_finite() {
            return;
        }
        let width = (self.lanes_width_px / self.px_per_tick) as f64;
        if tick > self.left_tick + width * 0.9 || tick < self.left_tick {
            self.left_tick = (tick - width * 0.1).max(0.0);
        }
    }

    fn x_of(&self, lanes_left: f32, tick: f64) -> f32 {
        lanes_left + ((tick - self.left_tick) * self.px_per_tick as f64) as f32
    }

    fn tick_at_x(&self, lanes_left: f32, x: f32) -> f64 {
        (self.left_tick + ((x - lanes_left) / self.px_per_tick) as f64).max(0.0)
    }

    fn tick_at_right(&self, lanes: Rangef) -> f64 {
        self.left_tick + (lanes.span() / self.px_per_tick) as f64
    }

    /// Draws the map and reports clicks. `roll_window` is the tick range the piano
    /// roll is showing, drawn as a faint box when the roll is open.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        ui: &mut Ui,
        editor: &Editor,
        theme: &Theme,
        playhead_tick: Option<f64>,
        roll_window: Option<(f64, f64)>,
        instrument_names: &[String],
        actions: &mut Vec<TracksAction>,
    ) {
        let rect = ui.available_rect_before_wrap();
        if !rect.is_positive() || rect.width() <= HEADER_W + 1.0 {
            return;
        }
        // A resizable panel sizes itself from what its content allocates, so claim the whole area.
        ui.allocate_rect(rect, Sense::hover());
        ui.painter().rect_filled(rect, 0.0, theme.color("status_bg"));
        let lanes_x = Rangef::new(rect.left() + HEADER_W, rect.right());
        self.lanes_width_px = lanes_x.span();
        let song = &editor.song;
        if self.fitted_to_song {
            let end = fit_end_tick(song, editor.cursor.tick).get().max(1) as f32;
            self.px_per_tick = (lanes_x.span() / end).clamp(MIN_PX_PER_TICK, MAX_PX_PER_TICK);
            self.left_tick = 0.0;
        }

        let grid = editor.grid_ticks().get().max(1);
        let snap = |tick: f64| (tick as u64 / grid) * grid;

        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
            if rect.contains(pos) && pos.x >= lanes_x.min {
                let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
                if zoom != 1.0 {
                    let anchor = self.tick_at_x(lanes_x.min, pos.x);
                    self.zoom_time(zoom, anchor, pos.x - lanes_x.min);
                } else if scroll.x != 0.0 {
                    self.scroll_by(scroll.x);
                }
            }
        }

        let font = FontId::monospace((theme.font_size - 3.0).max(8.0));
        let ruler = Rect::from_min_max(Pos2::new(lanes_x.min, rect.top()), Pos2::new(rect.right(), rect.top() + RULER_H));
        self.draw_ruler(ui.painter(), ruler, lanes_x, song, theme, &font);
        let ruler_response = ui.interact(ruler, ui.scope_id().with("tracks_ruler"), Sense::click_and_drag());
        if ruler_response.drag_started() {
            actions.push(TracksAction::ScrubStart);
        }
        if ruler_response.clicked() || ruler_response.dragged() {
            if let Some(pos) = ruler_response.interact_pointer_pos() {
                let tick = snap(self.tick_at_x(lanes_x.min, pos.x));
                if self.scrub_tick != Some(tick) {
                    actions.push(TracksAction::SetPlayhead { tick });
                    self.scrub_tick = Some(tick);
                }
            }
        }
        if ruler_response.drag_stopped() {
            actions.push(TracksAction::ScrubEnd);
        }
        if !ruler_response.dragged() {
            self.scrub_tick = None;
        }

        let rows_rect = Rect::from_min_max(Pos2::new(rect.left(), ruler.bottom()), rect.max);
        let mut rows_top = rows_rect.top();
        let mut rows_bottom = rows_rect.top();
        let any_solo = song.tracks.iter().any(|t| t.solo);
        let header_font = FontId::monospace(theme.font_size - 1.0);
        let mut rows_ui = ui.new_child(egui::UiBuilder::new().max_rect(rows_rect));
        egui::ScrollArea::vertical().id_salt("tracks_rows").auto_shrink([false, false]).show(&mut rows_ui, |ui| {
            for (i, track) in song.tracks.iter().enumerate() {
                let current = i == editor.track_index;
                let (row, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                if i == 0 {
                    rows_top = row.top();
                }
                rows_bottom = row.bottom();
                let header = Rect::from_min_max(row.min, Pos2::new(row.left() + HEADER_W, row.bottom()));
                let lane = Rect::from_min_max(Pos2::new(header.right(), row.top()), row.max);
                let painter = ui.painter();
                let muted = track.mute || (any_solo && !track.solo);
                let instrument = instrument_names.get(i).map(String::as_str).unwrap_or("");
                self.draw_header(painter, header, i, track, instrument, current, muted, theme, &header_font);
                self.draw_lane(painter, lane, track, current, muted, song, theme);
                painter.hline(row.x_range(), row.bottom() - 0.5, Stroke::new(1.0, theme.color("grid")));
                if response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        if pos.x < lane.left() {
                            if header_instrument_hit(header, pos) {
                                actions.push(TracksAction::PickInstrument { track: i });
                            } else {
                                actions.push(TracksAction::ShowTrack { track: i });
                            }
                        } else {
                            actions.push(TracksAction::JumpTo { track: i, tick: snap(self.tick_at_x(lane.left(), pos.x)) });
                        }
                    }
                }
                if current && self.shown_track != Some(i) {
                    ui.scroll_to_rect(row, None);
                }
            }
        });
        self.shown_track = Some(editor.track_index);

        let lanes_clip = Rect::from_min_max(Pos2::new(lanes_x.min, rect.top()), rect.max);
        let overlay = ui.painter().with_clip_rect(lanes_clip.intersect(rows_rect.union(ruler)));
        let rows_y = Rangef::new(rows_top.max(rows_rect.top()), rows_bottom.min(rows_rect.bottom()));
        if let Some((t0, t1)) = roll_window {
            if rows_y.span() > 0.0 {
                let x0 = self.x_of(lanes_x.min, t0);
                let x1 = self.x_of(lanes_x.min, t1);
                let window = Rect::from_x_y_ranges(Rangef::new(x0, x1), rows_y);
                overlay.rect_filled(window, 0.0, theme.color("roll_window"));
            }
        }
        let full_y = Rangef::new(rect.top(), rows_y.max.max(ruler.bottom()));
        let cursor_x = self.x_of(lanes_x.min, editor.cursor.tick.get() as f64);
        overlay.vline(cursor_x.round() + 0.5, full_y, Stroke::new(1.0, theme.color("cursor")));
        if let Some(tick) = playhead_tick {
            let x = self.x_of(lanes_x.min, tick);
            let color = theme.color("playhead");
            overlay.vline(x, full_y, Stroke::new(2.0, color));
            crate::piano_roll::draw_playhead_marker(&overlay, x, ruler.bottom(), color);
        }
    }

    fn draw_ruler(&self, painter: &Painter, ruler: Rect, lanes_x: Rangef, song: &Song, theme: &Theme, font: &FontId) {
        let painter = painter.with_clip_rect(ruler);
        let right = self.tick_at_right(lanes_x);
        let mut bar = song.meter_map.bar_beat_of(song.ppq, Tick(self.left_tick as u64)).bar;
        let mut last_label_x = f32::NEG_INFINITY;
        loop {
            let tick = song.meter_map.tick_of_bar(song.ppq, bar).get() as f64;
            if tick > right {
                break;
            }
            let x = self.x_of(lanes_x.min, tick);
            painter.vline(x, Rangef::new(ruler.top() + ruler.height() * 0.5, ruler.bottom()), Stroke::new(1.0, theme.color("bar_line")));
            // Labels overlap when bars are only a few pixels wide; keep the ones that fit.
            if x - last_label_x >= 24.0 {
                painter.text(Pos2::new(x + 2.0, ruler.center().y), Align2::LEFT_CENTER, bar.to_string(), font.clone(), theme.color("status_dim"));
                last_label_x = x;
            }
            bar += 1;
            if bar > 100_000 {
                break;
            }
        }
        painter.hline(ruler.x_range(), ruler.bottom() - 0.5, Stroke::new(1.0, theme.color("bar_line")));
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_header(&self, painter: &Painter, header: Rect, index: usize, track: &mp_core::Track, instrument: &str, current: bool, muted: bool, theme: &Theme, font: &FontId) {
        let painter = painter.with_clip_rect(header);
        if current {
            painter.rect_filled(header, 0.0, theme.color("black_key_row"));
        }
        let flags = format!("{}{}", if track.mute { "M" } else { "·" }, if track.solo { "S" } else { "·" });
        let line1 = format!("{:>2} {} {flags}", index + 1, track.name);
        let line2 = format!("   {instrument} {:>3}% {}", (track.volume * 100.0).round() as i32, pan_label((track.pan * 100.0).round() as i32));
        let name_color = if current { "note_selected" } else if muted { "status_dim" } else { "status_fg" };
        let x = header.left() + 6.0;
        painter.text(Pos2::new(x, header.top() + header.height() * 0.3), Align2::LEFT_CENTER, line1, font.clone(), theme.color(name_color));
        painter.text(Pos2::new(x, header.top() + header.height() * 0.72), Align2::LEFT_CENTER, line2, font.clone(), theme.color("status_dim"));
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_lane(&self, painter: &Painter, lane: Rect, track: &mp_core::Track, current: bool, muted: bool, song: &Song, theme: &Theme) {
        let painter = painter.with_clip_rect(lane);
        painter.rect_filled(lane, 0.0, theme.color(if current { "lane_current" } else { "lane" }));
        let right = self.tick_at_right(lane.x_range());
        let mut bar = song.meter_map.bar_beat_of(song.ppq, Tick(self.left_tick as u64)).bar;
        loop {
            let tick = song.meter_map.tick_of_bar(song.ppq, bar).get() as f64;
            if tick > right {
                break;
            }
            painter.vline(self.x_of(lane.left(), tick), lane.y_range(), Stroke::new(1.0, theme.color("beat_line")));
            bar += 1;
            if bar > 100_000 {
                break;
            }
        }
        let span = pitch_span(&track.notes);
        let inner = lane.shrink2(Vec2::new(0.0, 2.0));
        let left = self.left_tick as u64;
        let right = right as u64;
        let base = theme.color(if muted { "note_ghost" } else { "note" });
        for note in &track.notes {
            if note.end().get() < left || note.start.get() > right {
                continue;
            }
            let x0 = self.x_of(lane.left(), note.start.get() as f64);
            let x1 = self.x_of(lane.left(), note.end().get() as f64);
            let alpha = 110 + (note.velocity as u32 * 145 / 127) as u8;
            let fill = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha);
            painter.rect_filled(note_rect(inner, x0, x1, note.pitch, span), 0.0, fill);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lower_half_of_a_header_is_its_instrument_line() {
        let header = Rect::from_min_size(Pos2::new(0.0, 100.0), Vec2::new(HEADER_W, ROW_H));
        assert!(!header_instrument_hit(header, Pos2::new(10.0, 105.0)));
        assert!(header_instrument_hit(header, Pos2::new(10.0, 130.0)));
    }
    use mp_core::{Note, NoteId, Track};

    fn note(pitch: u8) -> Note {
        Note { id: NoteId(pitch as u64), start: Tick::ZERO, duration: Tick(1), pitch, velocity: 100 }
    }

    fn bar(song: &Song, n: u32) -> Tick {
        song.meter_map.tick_of_bar(song.ppq, n)
    }

    #[test]
    fn fit_end_is_at_least_16_bars_for_empty_song() {
        let song = Song::new_empty();
        assert_eq!(fit_end_tick(&song, Tick::ZERO), bar(&song, 17));
        assert_eq!(fit_end_tick(&song, bar(&song, 5)), bar(&song, 17), "a cursor inside the minimum changes nothing");
    }

    #[test]
    fn fit_end_rounds_up_to_next_bar_and_includes_cursor() {
        let mut song = Song::new_empty();
        let n = song.new_note(bar(&song, 20), Tick(10), 60, 100);
        song.tracks[0].insert_note(n);
        assert_eq!(fit_end_tick(&song, Tick::ZERO), bar(&song, 21), "a note just past bar 20 rounds up to bar 21");
        assert_eq!(fit_end_tick(&song, bar(&song, 30)), bar(&song, 30), "a cursor exactly on a bar start ends there");
        assert_eq!(fit_end_tick(&song, Tick(bar(&song, 30).get() + 1)), bar(&song, 31));
    }

    #[test]
    fn pitch_span_covers_notes_and_at_least_12_rows() {
        assert_eq!(pitch_span(&[]), (55, 66));
        assert_eq!(pitch_span(&[note(60)]), (55, 66));
        let (lo, hi) = pitch_span(&[note(40), note(90)]);
        assert_eq!((lo, hi), (40, 90));
        assert_eq!(pitch_span(&[note(0), note(2)]), (0, 11), "shifts up instead of going below 0");
        assert_eq!(pitch_span(&[note(127)]), (116, 127), "shifts down instead of going above 127");
        let empty_track = Track::new("x");
        assert_eq!(pitch_span(&empty_track.notes), (55, 66));
    }

    #[test]
    fn note_rect_puts_low_pitches_at_bottom_and_is_at_least_1px() {
        let lane = Rect::from_min_size(Pos2::new(100.0, 0.0), Vec2::new(200.0, 36.0));
        let span = (48, 59);
        let low = note_rect(lane, 110.0, 120.0, 48, span);
        let high = note_rect(lane, 110.0, 120.0, 59, span);
        assert_eq!(low.bottom(), lane.bottom());
        assert_eq!(high.top(), lane.top());
        assert!(high.bottom() < low.top());
        let sliver = note_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(10.0, 3.0)), 5.0, 5.1, 50, (0, 127));
        assert!(sliver.width() >= 1.0);
        assert!(sliver.height() >= 1.0);
    }
}
