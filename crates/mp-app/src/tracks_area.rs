//! The tracks area: every track as a row with its notes drawn small across the
//! whole song, the cursor and playhead over all of them. It has its own zoom and
//! scroll, independent of the piano roll, and starts fitted to the song.

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rangef, Rect, Sense, Stroke, Ui, Vec2};
use mp_core::{pan_label, Editor, Note, Song, Tick, MAX_TRACK_VOLUME_PERCENT};

use crate::theme::Theme;

/// The header column can be dragged no narrower than this.
const MIN_HEADER_W: f32 = 110.0;
/// The lanes always keep at least this much room, whatever the header is dragged to.
const MIN_LANES_W: f32 = 120.0;
/// Width of the invisible strip on the header's right edge that resizes it.
const RESIZE_GRIP_W: f32 = 8.0;
const HEADER_PAD: f32 = 6.0;
const BADGE_SIZE: Vec2 = Vec2::new(16.0, 14.0);
const VOLUME_SLIDER_W: f32 = 64.0;
const PAN_KNOB_RADIUS: f32 = 8.0;
/// How far the pan knob turns either side of centre, in radians (±135°).
const PAN_KNOB_SWEEP: f32 = std::f32::consts::PI * 0.75;
/// Pan units per pixel of knob drag.
const PAN_DRAG_RATE: f32 = 1.0;
const ROW_H: f32 = 46.0;
/// Centre lines of a header's two rows of text and controls, from the row's top.
const LINE1_Y: f32 = 12.0;
const LINE2_Y: f32 = 34.0;
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
    /// Double-click on the name line of a row header: edit that track's name.
    RenameTrack { track: usize },
    /// Click on the instrument line of a row header: choose that track's sound.
    PickInstrument { track: usize },
    /// The header column was dragged to this width; the song remembers it.
    ResizeHeader { width: f32 },
    /// The M badge of a row was clicked.
    ToggleMute { track: usize },
    /// The S badge of a row was clicked.
    ToggleSolo { track: usize },
    /// The volume slider of a row was moved, in percent 0..=200.
    SetVolume { track: usize, percent: i32 },
    /// The pan knob of a row was turned, -100..=100.
    SetPan { track: usize, value: i32 },
    /// A slider or knob drag ended: the next change is a separate undo step.
    EndMixGesture,
}

/// Where a row header's controls sit: badges on the first line, slider and knob on
/// the second, all flush right so they stay put while the names give way.
struct HeaderControls {
    mute: Rect,
    solo: Rect,
    slider: Rect,
    knob: Rect,
    /// The instrument line left of the slider, which opens the instrument picker.
    instrument: Rect,
    line1_y: f32,
    line2_y: f32,
}

fn header_controls(header: Rect) -> HeaderControls {
    let line1_y = header.top() + LINE1_Y;
    let line2_y = header.top() + LINE2_Y;
    let solo = Rect::from_center_size(Pos2::new(header.right() - HEADER_PAD - BADGE_SIZE.x / 2.0, line1_y), BADGE_SIZE);
    let mute = solo.translate(Vec2::new(-(BADGE_SIZE.x + 3.0), 0.0));
    let knob = Rect::from_center_size(Pos2::new(header.right() - HEADER_PAD - PAN_KNOB_RADIUS, line2_y), Vec2::splat(PAN_KNOB_RADIUS * 2.0));
    let slider = Rect::from_center_size(Pos2::new(knob.left() - 8.0 - VOLUME_SLIDER_W / 2.0, line2_y), Vec2::new(VOLUME_SLIDER_W, 14.0));
    let instrument = Rect::from_x_y_ranges(Rangef::new(header.left(), (slider.left() - 4.0).max(header.left())), Rangef::new(header.center().y, header.bottom()));
    HeaderControls { mute, solo, slider, knob, instrument, line1_y, line2_y }
}

/// The volume percent a pointer at `x` on `slider` asks for: 0% at the left edge, 200% at the right.
fn slider_percent_at(slider: Rect, x: f32) -> i32 {
    let t = ((x - slider.left()) / slider.width()).clamp(0.0, 1.0);
    (t * MAX_TRACK_VOLUME_PERCENT as f32).round() as i32
}

/// The pan value after dragging a knob by `delta` pixels from `start`: right or up turns it right.
fn knob_value_after_drag(start: i32, delta: Vec2) -> i32 {
    (start as f32 + (delta.x - delta.y) * PAN_DRAG_RATE).round().clamp(-100.0, 100.0) as i32
}

/// The knob pointer's angle for `pan` in -100..=100: straight up at centre, `PAN_KNOB_SWEEP` either side.
fn knob_angle(pan: i32) -> f32 {
    pan as f32 / 100.0 * PAN_KNOB_SWEEP
}

/// The header width the song asks for, kept inside what `area_width` can show.
fn clamp_header_width(wanted: f32, area_width: f32) -> f32 {
    let max = (area_width - MIN_LANES_W).max(MIN_HEADER_W);
    if wanted.is_finite() {
        wanted.clamp(MIN_HEADER_W, max)
    } else {
        MIN_HEADER_W.min(max)
    }
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

/// Draws `text` left-aligned at `pos` (vertically centred), elided with `…` past `max_width`.
/// Returns the rect the text was drawn in, if any.
fn draw_truncated(painter: &Painter, pos: Pos2, text: &str, max_width: f32, font: &FontId, color: Color32) -> Option<Rect> {
    if max_width <= 0.0 {
        return None;
    }
    let mut job = LayoutJob::simple_singleline(text.to_string(), font.clone(), color);
    job.wrap = TextWrapping::truncate_at_width(max_width);
    let galley = painter.layout_job(job);
    let top_left = Pos2::new(pos.x, pos.y - galley.size().y / 2.0);
    let rect = Rect::from_min_size(top_left, galley.size());
    painter.galley(top_left, galley, color);
    Some(rect)
}

/// A text field holding `text` and a caret. Text too long for it scrolls so the caret stays in view.
fn draw_name_field(painter: &Painter, field: Rect, text: &str, theme: &Theme, font: &FontId) {
    if !field.is_positive() {
        return;
    }
    painter.rect_filled(field, CornerRadius::same(2), theme.color("status_bg"));
    painter.rect_stroke(field, CornerRadius::same(2), Stroke::new(1.0, theme.color("accent")), egui::StrokeKind::Inside);
    let inner = field.shrink2(Vec2::new(3.0, 0.0));
    let galley = painter.layout_no_wrap(format!("{text}▏"), font.clone(), theme.color("status_fg"));
    let x = if galley.size().x <= inner.width() { inner.left() } else { inner.right() - galley.size().x };
    let top_left = Pos2::new(x, field.center().y - galley.size().y / 2.0);
    painter.with_clip_rect(inner).galley(top_left, galley, theme.color("status_fg"));
}

/// Which header controls the pointer is on, for highlighting.
#[derive(Clone, Copy, Default)]
struct HeaderHot {
    mute: bool,
    solo: bool,
    slider: bool,
    knob: bool,
    instrument: bool,
}

/// A one-letter mute or solo badge: filled with `on_color` when on, an outline when off.
#[allow(clippy::too_many_arguments)]
fn draw_badge(painter: &Painter, rect: Rect, letter: &str, on: bool, hot: bool, on_color: Color32, theme: &Theme, font: &FontId) {
    let rounding = CornerRadius::same(2);
    if on {
        painter.rect_filled(rect, rounding, on_color);
        painter.text(rect.center(), Align2::CENTER_CENTER, letter, font.clone(), theme.color("status_bg"));
    } else {
        let edge = theme.color(if hot { "status_fg" } else { "bar_line" });
        painter.rect_stroke(rect, rounding, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        painter.text(rect.center(), Align2::CENTER_CENTER, letter, font.clone(), theme.color(if hot { "status_fg" } else { "status_dim" }));
    }
}

/// A horizontal fader: 0% at the left end, 200% at the right, a tick at 100%.
fn draw_volume_slider(painter: &Painter, rect: Rect, percent: i32, hot: bool, color: Color32, theme: &Theme) {
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 4.0));
    let t = (percent as f32 / MAX_TRACK_VOLUME_PERCENT as f32).clamp(0.0, 1.0);
    let x = track.left() + t * track.width();
    let fill = if hot { theme.color("status_fg") } else { color };
    painter.rect_filled(track, CornerRadius::same(2), theme.color("bar_line"));
    painter.rect_filled(Rect::from_min_max(track.min, Pos2::new(x, track.bottom())), CornerRadius::same(2), fill);
    let unity_x = track.left() + track.width() / 2.0;
    painter.vline(unity_x, Rangef::new(rect.top() + 1.0, rect.bottom() - 1.0), Stroke::new(1.0, theme.color("status_dim")));
    painter.circle_filled(Pos2::new(x, rect.center().y), 5.0, fill);
}

/// A rotary pan knob: pointer straight up at centre, turned left or right for L/R.
fn draw_pan_knob(painter: &Painter, rect: Rect, pan: i32, hot: bool, color: Color32, theme: &Theme) {
    let centre = rect.center();
    let radius = rect.width() / 2.0;
    painter.circle(centre, radius, theme.color("status_bg"), Stroke::new(1.0, theme.color(if hot { "status_fg" } else { "bar_line" })));
    // The arc from straight up to the pointer shows how far off centre the pan is.
    let angle = knob_angle(pan);
    let steps = (angle.abs() / 0.15).ceil().max(1.0) as usize;
    let arc_radius = radius + 1.5;
    let arc: Vec<Pos2> = (0..=steps)
        .map(|k| {
            let a = angle * k as f32 / steps as f32;
            centre + Vec2::new(a.sin(), -a.cos()) * arc_radius
        })
        .collect();
    if pan != 0 {
        painter.add(egui::Shape::line(arc, Stroke::new(2.0, color)));
    }
    let tip = centre + Vec2::new(angle.sin(), -angle.cos()) * (radius - 1.5);
    painter.line_segment([centre, tip], Stroke::new(2.0, if hot { theme.color("status_fg") } else { color }));
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
        rename: Option<(usize, &str)>,
        actions: &mut Vec<TracksAction>,
    ) {
        let rect = ui.available_rect_before_wrap();
        if !rect.is_positive() || rect.width() <= MIN_HEADER_W + MIN_LANES_W {
            return;
        }
        // A resizable panel sizes itself from what its content allocates, so claim the whole area.
        ui.allocate_rect(rect, Sense::hover());
        ui.painter().rect_filled(rect, 0.0, theme.color("status_bg"));
        let header_w = clamp_header_width(editor.song.track_header_width, rect.width());
        let lanes_x = Rangef::new(rect.left() + header_w, rect.right());
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

        let font = theme.font((theme.font_size - 3.0).max(8.0));
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
        let header_font = theme.font(theme.font_size - 1.0);
        let mut rows_ui = ui.new_child(egui::UiBuilder::new().max_rect(rows_rect));
        egui::ScrollArea::vertical().id_salt("tracks_rows").auto_shrink([false, false]).show(&mut rows_ui, |ui| {
            for (i, track) in song.tracks.iter().enumerate() {
                let current = i == editor.track_index;
                let (row, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                if i == 0 {
                    rows_top = row.top();
                }
                rows_bottom = row.bottom();
                let header = Rect::from_min_max(row.min, Pos2::new(row.left() + header_w, row.bottom()));
                let lane = Rect::from_min_max(Pos2::new(header.right(), row.top()), row.max);
                let muted = track.mute || (any_solo && !track.solo);
                let instrument = instrument_names.get(i).map(String::as_str).unwrap_or("");
                let controls = header_controls(header);
                // Registered after the row so they take the pointer where they overlap it.
                let row_id = ui.scope_id().with(("track_header", i));
                let mute_response = ui.interact(controls.mute, row_id.with("mute"), Sense::click());
                let solo_response = ui.interact(controls.solo, row_id.with("solo"), Sense::click());
                let slider_response = ui.interact(controls.slider, row_id.with("volume"), Sense::click_and_drag());
                let knob_response = ui.interact(controls.knob, row_id.with("pan"), Sense::click_and_drag());
                let instrument_response = ui.interact(controls.instrument, row_id.with("instrument"), Sense::click());
                instrument_response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), format!("Instrument for {}: {instrument}", track.name)));
                if instrument_response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if instrument_response.clicked() {
                    actions.push(TracksAction::PickInstrument { track: i });
                }
                let volume_percent = (track.volume * 100.0).round() as i32;
                let pan_value = (track.pan * 100.0).round() as i32;
                if mute_response.clicked() {
                    actions.push(TracksAction::ToggleMute { track: i });
                }
                if solo_response.clicked() {
                    actions.push(TracksAction::ToggleSolo { track: i });
                }
                if slider_response.double_clicked() {
                    actions.push(TracksAction::SetVolume { track: i, percent: 100 });
                } else if slider_response.clicked() || slider_response.dragged() {
                    if let Some(pos) = slider_response.interact_pointer_pos() {
                        let percent = slider_percent_at(controls.slider, pos.x);
                        if percent != volume_percent {
                            actions.push(TracksAction::SetVolume { track: i, percent });
                        }
                    }
                }
                if knob_response.double_clicked() {
                    actions.push(TracksAction::SetPan { track: i, value: 0 });
                } else if knob_response.dragged() {
                    let value = knob_value_after_drag(pan_value, knob_response.drag_delta());
                    if value != pan_value {
                        actions.push(TracksAction::SetPan { track: i, value });
                    }
                }
                if slider_response.drag_stopped() || knob_response.drag_stopped() {
                    actions.push(TracksAction::EndMixGesture);
                }
                let painter = ui.painter();
                let hot = HeaderHot {
                    mute: mute_response.hovered(),
                    solo: solo_response.hovered(),
                    slider: slider_response.hovered() || slider_response.dragged(),
                    knob: knob_response.hovered() || knob_response.dragged(),
                    instrument: instrument_response.hovered() || instrument_response.has_focus(),
                };
                let editing = rename.and_then(|(track, text)| (track == i).then_some(text));
                self.draw_header(painter, header, &controls, i, track, instrument, current, muted, hot, editing, theme, &header_font);
                self.draw_lane(painter, lane, i, track, current, muted, song, theme);
                painter.hline(row.x_range(), row.bottom() - 0.5, Stroke::new(1.0, theme.color("panel_border")));
                if response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        if pos.x < lane.left() {
                            actions.push(TracksAction::ShowTrack { track: i });
                        } else {
                            actions.push(TracksAction::JumpTo { track: i, tick: snap(self.tick_at_x(lane.left(), pos.x)) });
                        }
                    }
                }
                if response.double_clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        if pos.x < lane.left() {
                            actions.push(TracksAction::RenameTrack { track: i });
                        }
                    }
                }
                if current && self.shown_track != Some(i) {
                    ui.scroll_to_rect(row, None);
                }
            }
        });
        self.shown_track = Some(editor.track_index);

        let free = Rect::from_min_max(Pos2::new(rect.left(), rows_bottom.max(rows_rect.top())), rows_rect.max);
        if free.height() >= 60.0 && song.tracks.iter().all(|t| t.notes.is_empty()) {
            ui.painter().text(free.center(), Align2::CENTER_CENTER, "MIDI Party", theme.bold(theme.font_size * 2.5), theme.color("wordmark"));
        }

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

        // Registered after the rows and ruler so it wins the pointer where they overlap.
        let grip = Rect::from_x_y_ranges(Rangef::new(lanes_x.min - RESIZE_GRIP_W / 2.0, lanes_x.min + RESIZE_GRIP_W / 2.0), rect.y_range());
        let grip_response = ui.interact(grip, ui.scope_id().with("tracks_header_grip"), Sense::drag());
        if grip_response.hovered() || grip_response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            ui.painter().vline(lanes_x.min, rect.y_range(), Stroke::new(2.0, theme.color("accent")));
        }
        if grip_response.dragged() {
            if let Some(pos) = grip_response.interact_pointer_pos() {
                let width = clamp_header_width(pos.x - rect.left(), rect.width());
                if width != header_w {
                    actions.push(TracksAction::ResizeHeader { width });
                }
            }
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
        painter.hline(ruler.x_range(), ruler.bottom() - 0.5, Stroke::new(1.0, theme.color("panel_border")));
    }

    /// Two lines: `N Name` with the mute and solo badges on the right, then the
    /// instrument with the volume slider and pan knob on the right. The names give
    /// way to the controls, so those stay usable however narrow the header is dragged.
    /// `editing` is the text of a rename in progress, drawn as a field over the name.
    #[allow(clippy::too_many_arguments)]
    fn draw_header(&self, painter: &Painter, header: Rect, controls: &HeaderControls, index: usize, track: &mp_core::Track, instrument: &str, current: bool, muted: bool, hot: HeaderHot, editing: Option<&str>, theme: &Theme, font: &FontId) {
        let painter = painter.with_clip_rect(header);
        if current {
            painter.rect_filled(header, 0.0, theme.color("black_key_row"));
        }
        let dim = theme.color("status_dim");
        let color = theme.track_color(index);
        let volume_percent = (track.volume * 100.0).round() as i32;
        let pan_value = (track.pan * 100.0).round() as i32;

        let badge_font = theme.font(font.size - 2.0);
        draw_badge(&painter, controls.mute, "M", track.mute, hot.mute, theme.color("error"), theme, &badge_font);
        draw_badge(&painter, controls.solo, "S", track.solo, hot.solo, theme.color("note_selected"), theme, &badge_font);
        draw_volume_slider(&painter, controls.slider, volume_percent, hot.slider, color, theme);
        draw_pan_knob(&painter, controls.knob, pan_value, hot.knob, color, theme);

        let name_color = if muted { dim } else { theme.color("status_fg") };
        let name_font = if current { theme.bold(font.size) } else { font.clone() };
        let x = header.left() + HEADER_PAD;
        let swatch = Rect::from_center_size(Pos2::new(x + 4.0, controls.line1_y), Vec2::splat(8.0));
        painter.rect_filled(swatch, CornerRadius::same(1), if muted { theme.color("note_ghost") } else { color });
        let indent = 14.0;
        let name_right = controls.mute.left() - HEADER_PAD;
        if let Some(text) = editing {
            let field = Rect::from_x_y_ranges(Rangef::new(x + indent - 3.0, name_right), Rangef::new(controls.line1_y - 9.0, controls.line1_y + 9.0));
            draw_name_field(&painter, field, text, theme, font);
        } else {
            let name = format!("{} {}", index + 1, track.name);
            draw_truncated(&painter, Pos2::new(x + indent, controls.line1_y), &name, name_right - x - indent, &name_font, name_color);
        }
        let instrument_font = theme.font(font.size - 1.0);
        let instrument_color = if hot.instrument { theme.color("status_fg") } else { dim };
        let instrument_rect = draw_truncated(&painter, Pos2::new(x + indent, controls.line2_y), instrument, controls.slider.left() - HEADER_PAD - x - indent, &instrument_font, instrument_color);
        if let (true, Some(rect)) = (hot.instrument, instrument_rect) {
            painter.hline(rect.x_range(), rect.bottom() + 0.5, Stroke::new(1.0, instrument_color));
        }

        // The value of the control under the pointer, over the first line so it never hides the control itself.
        let value_label = if hot.slider {
            Some((controls.slider.center().x, format!("{volume_percent}%")))
        } else if hot.knob {
            Some((controls.knob.center().x, pan_label(pan_value)))
        } else {
            None
        };
        if let Some((x, text)) = value_label {
            let x = x.min(controls.mute.left() - HEADER_PAD - 20.0);
            let rect = painter.text(Pos2::new(x, controls.line1_y), Align2::CENTER_CENTER, text.clone(), font.clone(), theme.color("status_fg"));
            painter.rect_filled(rect.expand(3.0), CornerRadius::same(2), theme.color("status_bg"));
            painter.text(Pos2::new(x, controls.line1_y), Align2::CENTER_CENTER, text, font.clone(), theme.color("status_fg"));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_lane(&self, painter: &Painter, lane: Rect, index: usize, track: &mp_core::Track, current: bool, muted: bool, song: &Song, theme: &Theme) {
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
        let base = if muted { theme.color("note_ghost") } else { theme.track_color(index) };
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
    fn the_instrument_button_fills_the_lower_half_left_of_the_slider() {
        let header = Rect::from_min_size(Pos2::new(0.0, 100.0), Vec2::new(190.0, ROW_H));
        let c = header_controls(header);
        assert!(!c.instrument.contains(Pos2::new(10.0, 105.0)));
        assert!(c.instrument.contains(Pos2::new(10.0, 130.0)));
        assert!(c.instrument.right() < c.slider.left());
    }
    use mp_core::{Note, NoteId, Track};

    #[test]
    fn slider_runs_from_0_to_200_percent() {
        let slider = Rect::from_min_size(Pos2::new(100.0, 0.0), Vec2::new(64.0, 14.0));
        assert_eq!(slider_percent_at(slider, 100.0), 0);
        assert_eq!(slider_percent_at(slider, 132.0), 100);
        assert_eq!(slider_percent_at(slider, 164.0), 200);
        assert_eq!(slider_percent_at(slider, 50.0), 0, "past the left end is 0");
        assert_eq!(slider_percent_at(slider, 900.0), 200, "past the right end is 200");
    }

    #[test]
    fn knob_turns_right_when_dragged_right_or_up_and_stops_at_the_ends() {
        assert_eq!(knob_value_after_drag(0, Vec2::new(10.0, 0.0)), 10);
        assert_eq!(knob_value_after_drag(0, Vec2::new(0.0, -10.0)), 10);
        assert_eq!(knob_value_after_drag(0, Vec2::new(-10.0, 0.0)), -10);
        assert_eq!(knob_value_after_drag(95, Vec2::new(50.0, 0.0)), 100);
        assert_eq!(knob_value_after_drag(-95, Vec2::new(-50.0, 0.0)), -100);
        assert_eq!(knob_angle(0), 0.0);
        assert_eq!(knob_angle(100), PAN_KNOB_SWEEP);
        assert_eq!(knob_angle(-100), -PAN_KNOB_SWEEP);
    }

    #[test]
    fn header_controls_sit_inside_the_header_flush_right() {
        let header = Rect::from_min_size(Pos2::new(0.0, 100.0), Vec2::new(220.0, ROW_H));
        let c = header_controls(header);
        for r in [c.mute, c.solo, c.slider, c.knob] {
            assert!(header.contains_rect(r), "{r:?} outside {header:?}");
        }
        assert!(c.mute.right() < c.solo.left());
        assert!(c.slider.right() < c.knob.left());
        assert!(c.solo.right() <= header.right() - HEADER_PAD + 0.01);
        assert!(c.knob.right() <= header.right() - HEADER_PAD + 0.01);
        assert!(c.knob.top() - c.solo.bottom() >= 5.0, "badges and knob do not crowd each other");
    }

    #[test]
    fn header_width_stays_between_its_minimum_and_the_lanes_minimum() {
        assert_eq!(clamp_header_width(220.0, 1000.0), 220.0);
        assert_eq!(clamp_header_width(10.0, 1000.0), MIN_HEADER_W);
        assert_eq!(clamp_header_width(950.0, 1000.0), 1000.0 - MIN_LANES_W);
        assert_eq!(clamp_header_width(f32::NAN, 1000.0), MIN_HEADER_W);
        assert_eq!(clamp_header_width(500.0, 200.0), MIN_HEADER_W, "a tiny window never squeezes the header below its minimum");
    }

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
