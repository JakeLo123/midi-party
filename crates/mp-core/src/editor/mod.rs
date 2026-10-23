//! The headless editor: all editing state and the one place commands are executed.
//!
//! # From keypress to edit
//!
//! 1. The app (or the test harness) turns a keypress into a [`crate::KeyChord`].
//! 2. [`crate::Keymap::feed`] maps the chord, in the current [`Mode`], to a
//!    [`Command`] (or a count digit, or "still pending" for multi-key sequences).
//! 3. [`Editor::execute`] runs the command with the optional count. It is one
//!    exhaustive `match`; nothing else in the program edits the song.
//! 4. Commands that only move the cursor, change mode or change the selection return
//!    straight away and are not undoable. Commands that touch the [`Song`] first call
//!    `commit` (which pushes an undo [`crate::Snapshot`] of the pre-edit state), then
//!    mutate, then emit [`EditorEvent::SongChanged`] so the app can rebuild its audio
//!    snapshot, plus [`EditorEvent::Audition`] for the notes worth hearing.
//! 5. Transport, zoom and cheat-sheet commands change nothing here: they only emit an
//!    [`EditorEvent`] for the app, so their key bindings can live in the keymap file.
//! 6. Errors ([`EditError`]) leave the editor exactly as it was and are meant to be
//!    shown in the status bar.
//! 7. Track commands (add, remove, reorder, mixer, instrument) and lane commands (points
//!    in the controller lane chosen by [`Editor::lane`]) are edits like any other: they
//!    `commit`, emit `SongChanged`, keep the cursor, and never touch notes except that
//!    removing a track removes its notes.

pub mod command;
pub mod edit_ops;
pub mod select;

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::song::{Instrument, LaneKind, Note, NoteId, Song, Tick, Track};
use crate::time::Grid;
use crate::undo::{Snapshot, UndoStack};

pub use command::{pan_label, Category, Command, CycleDir, Dir, Unit};

use edit_ops::XorShift64;

/// Default velocity for inserted notes.
pub const INSERT_VELOCITY: u8 = 100;
/// Cursor pitch of a fresh editor (middle C).
pub const DEFAULT_CURSOR_PITCH: u8 = 60;
/// Factor one `zoom_in` step asks the app to scale time by (`zoom_out` is its inverse).
pub const ZOOM_TIME_STEP: f32 = 1.25;
/// Factor one `zoom_rows_in` step asks the app to scale row height by.
pub const ZOOM_ROWS_STEP: f32 = 1.2;
/// Highest track volume in percent (`set_track_volume`), i.e. +6 dB over unity.
pub const MAX_TRACK_VOLUME_PERCENT: i32 = 200;
/// Highest SoundFont preset number; `next_preset` wraps past it.
pub const MAX_PRESET: u16 = 127;
/// Highest MIDI channel index (0-based).
pub const MAX_CHANNEL: u8 = 15;
/// `nudge_lane_value` multiplies its delta by this in the pitch-bend lane, whose
/// range (-8192..=8191) is 128× wider than a controller's.
pub const PITCH_BEND_NUDGE_SCALE: i32 = 64;
/// Sustain value written by `sustain_down` / `pedal_selection`.
pub const SUSTAIN_DOWN: i32 = 127;
/// Sustain value written by `sustain_up` / `pedal_selection`.
pub const SUSTAIN_UP: i32 = 0;

/// The grid cursor: a time position and a pitch, independent of the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cursor {
    pub tick: Tick,
    /// MIDI key number 0..=127.
    pub pitch: u8,
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor { tick: Tick::ZERO, pitch: DEFAULT_CURSOR_PITCH }
    }
}

/// Editing modes. Each has its own keymap table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Cursor movement and one-shot edits.
    #[default]
    Normal,
    /// Arrows grow a selection box from the anchor; Enter confirms.
    Select,
    /// Arrows move the selected notes instead of the cursor.
    Move,
    /// Typing a `:command`; keys go to the text field, not the keymap.
    CommandLine,
}

impl Mode {
    /// All modes, in keymap-table order.
    pub const ALL: [Mode; 4] = [Mode::Normal, Mode::Select, Mode::Move, Mode::CommandLine];

    /// The snake_case name used as the TOML table name (`[normal]`, `[select]`, ...).
    pub fn name(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Select => "select",
            Mode::Move => "move",
            Mode::CommandLine => "command_line",
        }
    }

    /// Parses a TOML table name.
    pub fn from_name(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == s)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Normal => "Normal",
            Mode::Select => "Select",
            Mode::Move => "Move",
            Mode::CommandLine => "Command",
        })
    }
}

/// What a transport command asks the app to do. The editor has no playhead, so every
/// start position is a tick it chose (the cursor or the song start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportRequest {
    /// Start playing at `from`.
    Play { from: Tick },
    /// Stop playing.
    Stop,
    /// Stop if playing, otherwise start at `from`.
    Toggle { from: Tick },
}

/// What a view command asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewRequest {
    /// Multiply the horizontal (time) zoom by this factor (> 1 zooms in).
    ZoomTime(f32),
    /// Multiply the row height by this factor (> 1 makes rows taller).
    ZoomRows(f32),
}

/// What happened as a result of [`Editor::execute`]. The app forwards these to the
/// audio bridge and the status bar.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorEvent {
    /// The song was modified (an undo snapshot was taken, or undo/redo ran).
    SongChanged,
    /// Play these notes once so the user hears what they just did.
    Audition(Vec<Note>),
    /// The editing mode changed.
    ModeChanged(Mode),
    /// A human-readable summary of the action, e.g. `"moved 3 notes +2 st"`.
    Status(String),
    /// A transport command was run; the app owns playback.
    Transport(TransportRequest),
    /// Show the cheat sheet if hidden, hide it if shown.
    ToggleCheatSheet,
    /// A zoom command was run; the app owns the view.
    View(ViewRequest),
}

/// Why a command could not run. The editor is unchanged when one of these is returned.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EditError {
    #[error("nothing selected and no note under the cursor")]
    NothingSelected,
    #[error("no note under the cursor")]
    NoNoteUnderCursor,
    #[error("clipboard is empty")]
    ClipboardEmpty,
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("nothing to redo")]
    NothingToRedo,
    #[error("nothing to repeat")]
    NothingToRepeat,
    #[error("no focused note: press Tab to focus one")]
    NothingFocused,
    /// A select or keep command found no notes; the selection is left as it was.
    #[error("{0}")]
    NoMatch(String),
    #[error("{0}")]
    OutOfRange(String),
    #[error("cannot move {dir} by {unit}: use a time unit left/right and a pitch unit up/down")]
    UnitDirectionMismatch { dir: Dir, unit: Unit },
    #[error("song has no tracks")]
    NoTracks,
    #[error("cannot remove the last track")]
    LastTrack,
    #[error("no lane shown: cycle_lane or :lane sustain first")]
    NoLaneShown,
    /// An argument is outside what the command accepts (an empty name, channel 17, ...).
    #[error("{0}")]
    BadValue(String),
}

/// All editing state for one open song. See the module docs for the command flow.
///
/// Fields are public so the GUI can draw them; mutate only through [`Editor::execute`]
/// (and the count helpers) so undo and events stay correct. `track_index` and `lane`
/// say which track and which controller lane the piano roll shows; the lane commands
/// act on `lane` and fail with [`EditError::NoLaneShown`] while it is `None`.
#[derive(Clone, Debug)]
pub struct Editor {
    pub song: Song,
    /// Index into `song.tracks` of the track shown in the piano roll.
    pub track_index: usize,
    /// The controller lane strip shown under the piano roll (`show_lane`,
    /// `cycle_lane`), or `None` for hidden. Not part of the undo snapshot.
    pub lane: Option<LaneKind>,
    pub cursor: Cursor,
    pub selection: BTreeSet<NoteId>,
    pub mode: Mode,
    /// The count prefix typed so far (for the status bar); see [`Editor::push_count_digit`].
    pub count: Option<u32>,
    pub grid: Grid,
    /// Swing in percent (0 = straight) applied by `quantize` when it has no swing of its own.
    pub swing: u8,
    /// Notes relative to the clip start (first note at tick 0).
    pub clipboard: Vec<Note>,
    pub undo: UndoStack,
    /// Corner of the selection box fixed when Select mode was entered.
    pub select_anchor: Option<Cursor>,
    /// In Select mode: the box spans every pitch (`enter_time_select`).
    pub select_all_pitches: bool,
    /// In Select mode: the box removes notes from `select_base` instead of selecting
    /// (`enter_subtract_select`).
    pub select_subtract: bool,
    /// The selection as it was when a subtract box was started (restored on Escape).
    pub select_base: BTreeSet<NoteId>,
    /// The selected note Tab last stepped onto, drawn highlighted. Cleared whenever the
    /// selection changes by any other means.
    pub focus: Option<NoteId>,
    /// The last command that changed the song, replayed by `repeat_last_edit`.
    pub last_edit: Option<Command>,
    /// The count `last_edit` ran with.
    pub last_edit_count: Option<u32>,
    /// State of the generator behind `humanize`; advances on every call.
    pub rng_seed: u64,
    committed: bool,
}

impl Editor {
    /// Wraps a song with the cursor at tick 0 / middle C, Normal mode, 1/16 grid.
    pub fn new(song: Song) -> Self {
        Editor {
            song,
            track_index: 0,
            lane: None,
            cursor: Cursor::default(),
            selection: BTreeSet::new(),
            mode: Mode::Normal,
            count: None,
            grid: Grid::default(),
            swing: 0,
            clipboard: Vec::new(),
            undo: UndoStack::default(),
            select_anchor: None,
            select_all_pitches: false,
            select_subtract: false,
            select_base: BTreeSet::new(),
            focus: None,
            last_edit: None,
            last_edit_count: None,
            rng_seed: XorShift64::DEFAULT_SEED,
            committed: false,
        }
    }

    /// Appends a digit to the pending count (`"1" "2"` → 12). Saturates at 9999.
    pub fn push_count_digit(&mut self, digit: u8) {
        let cur = self.count.unwrap_or(0);
        self.count = Some(cur.saturating_mul(10).saturating_add(u32::from(digit)).min(9999));
    }

    /// Takes the pending count, leaving `None`.
    pub fn take_count(&mut self) -> Option<u32> {
        self.count.take()
    }

    /// The track being edited.
    pub fn track(&self) -> Option<&Track> {
        self.song.track(self.track_index)
    }

    /// One grid step in ticks at the song's PPQ.
    pub fn grid_ticks(&self) -> Tick {
        self.grid.ticks(self.song.ppq)
    }

    /// The note under the cursor on the current track.
    pub fn note_under_cursor(&self) -> Option<&Note> {
        self.track().and_then(|t| select::note_under_cursor(t, self.cursor))
    }

    /// Notes the cursor-relative commands act on: the selection (restricted to notes
    /// that exist on the current track), else the note under the cursor.
    pub fn target_ids(&self) -> Result<BTreeSet<NoteId>, EditError> {
        let track = self.track().ok_or(EditError::NoTracks)?;
        let on_track: BTreeSet<NoteId> =
            self.selection.iter().copied().filter(|id| track.note(*id).is_some()).collect();
        if !on_track.is_empty() {
            return Ok(on_track);
        }
        match select::note_under_cursor(track, self.cursor) {
            Some(n) => Ok([n.id].into_iter().collect()),
            None => Err(EditError::NothingSelected),
        }
    }

    /// Current state as an undo snapshot.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            song: self.song.clone(),
            track_index: self.track_index,
            cursor: self.cursor,
            selection: self.selection.clone(),
        }
    }

    /// The cursor position as `bar:beat:tick`, for status strings.
    pub fn bar_beat(&self, tick: Tick) -> String {
        self.song.meter_map.bar_beat_of(self.song.ppq, tick).to_string()
    }

    /// Call before mutating the song: pushes the pre-edit snapshot (merging repeated
    /// edits that share `coalesce_key`) and marks this command as an edit.
    fn commit(&mut self, coalesce_key: Option<&str>) {
        self.undo.push(self.snapshot(), coalesce_key);
        self.committed = true;
    }

    fn restore(&mut self, s: Snapshot) {
        self.song = s.song;
        self.cursor = s.cursor;
        self.selection = s.selection;
        self.track_index = s.track_index.min(self.song.tracks.len().saturating_sub(1));
    }

    fn track_mut(&mut self) -> &mut Track {
        &mut self.song.tracks[self.track_index]
    }

    fn time_step(&self, unit: Unit, dir: Dir) -> Result<Tick, EditError> {
        match unit {
            Unit::Grid => Ok(self.grid_ticks()),
            Unit::Beat => Ok(self.song.meter_map.beat_ticks_at(self.song.ppq, self.cursor.tick)),
            Unit::Bar => Ok(self.song.meter_map.bar_ticks_at(self.song.ppq, self.cursor.tick)),
            Unit::Semitone | Unit::Octave => Err(EditError::UnitDirectionMismatch { dir, unit }),
        }
    }

    fn pitch_step(unit: Unit, dir: Dir) -> Result<u8, EditError> {
        match unit {
            Unit::Semitone => Ok(1),
            Unit::Octave => Ok(12),
            Unit::Grid | Unit::Beat | Unit::Bar => Err(EditError::UnitDirectionMismatch { dir, unit }),
        }
    }

    /// Recomputes the selection from the box between the anchor and the cursor,
    /// honouring the time-select and subtract flags.
    fn refresh_box_selection(&mut self) {
        let (Some(anchor), Some(track)) = (self.select_anchor, self.track()) else { return };
        let cell = self.grid_ticks();
        let in_box = if self.select_all_pitches {
            select::time_box_ids(track, anchor, self.cursor, cell)
        } else {
            select::box_ids(track, anchor, self.cursor, cell)
        };
        self.selection =
            if self.select_subtract { self.select_base.difference(&in_box).copied().collect() } else { in_box };
    }

    /// Forgets the box anchor and the Select-mode flags (not the selection itself).
    fn reset_select_state(&mut self) {
        self.select_anchor = None;
        self.select_all_pitches = false;
        self.select_subtract = false;
        self.select_base.clear();
    }

    fn set_mode(&mut self, mode: Mode, events: &mut Vec<EditorEvent>) {
        if self.mode != mode {
            self.mode = mode;
            events.push(EditorEvent::ModeChanged(mode));
        }
    }

    /// Runs one command. `count` is the typed count prefix (`None` = no prefix); each
    /// [`Command`] variant documents how it uses it. Returns the events to forward to
    /// the app, or an error that left the editor untouched.
    pub fn execute(&mut self, cmd: &Command, count: Option<u32>) -> Result<Vec<EditorEvent>, EditError> {
        let n = count.unwrap_or(1).max(1);
        let mut ev = Vec::new();
        self.committed = false;
        if !cmd.is_edit() {
            // a cursor move, mode change etc. ends a run of coalesced nudges
            self.undo.break_coalescing();
        }
        let selection_before = self.selection.clone();
        match cmd {
            // -------------------------------------------------------------- cursor
            Command::MoveCursor { dir, unit } => self.move_cursor(*dir, *unit, n, &mut ev)?,

            // -------------------------------------------------------------- edit
            Command::InsertNote => self.insert_note(n, &mut ev)?,
            Command::InsertChord { intervals } => self.insert_chord(intervals, n, &mut ev)?,
            Command::DeleteSelected => self.delete_selected(&mut ev)?,
            Command::MoveSelected { dir, unit } => self.move_selected(*dir, *unit, n, &mut ev)?,
            Command::NudgeSelected { ticks } => self.nudge_selected(*ticks, n, &mut ev)?,
            Command::ChangeLength { delta_steps } => self.change_length(*delta_steps, n, &mut ev)?,
            Command::SetLength { steps } => self.set_length(count.unwrap_or(*steps), &mut ev)?,
            Command::Legato => self.legato(&mut ev)?,
            Command::NudgeVelocity { delta } => self.nudge_velocity(*delta, n, &mut ev)?,
            Command::SetVelocity { value } => self.set_velocity(*value, &mut ev)?,
            Command::RampVelocity { from, to } => self.ramp_velocity(*from, *to, &mut ev)?,
            Command::Humanize { velocity, timing } => self.humanize(*velocity, *timing, &mut ev)?,
            Command::Quantize { grid, strength, swing } => self.quantize(*grid, *strength, *swing, &mut ev)?,
            Command::QuantizeLength => self.quantize_length(&mut ev)?,
            Command::SetSwing { percent } => {
                self.swing = (*percent).min(100);
                ev.push(EditorEvent::Status(format!("swing {}%", self.swing)));
            }
            Command::TransposeSelected { semitones } => self.transpose(*semitones, n, &mut ev)?,
            Command::RepeatLastEdit => {
                let last = self.last_edit.clone().ok_or(EditError::NothingToRepeat)?;
                let count = count.or(self.last_edit_count);
                // the inner call does all the bookkeeping, and `last_edit` stays `last`
                return self.execute(&last, count);
            }
            Command::Undo => self.undo_n(n, &mut ev)?,
            Command::Redo => self.redo_n(n, &mut ev)?,

            // -------------------------------------------------------------- selection
            Command::ToggleSelectUnderCursor => self.toggle_select_under_cursor(&mut ev)?,
            Command::EnterMode { mode } => self.enter_mode(*mode, &mut ev),
            Command::EnterTimeSelect => {
                self.enter_select(true, false, &mut ev);
                ev.push(EditorEvent::Status(format!("time select: {} notes in range", self.selection.len())));
            }
            Command::EnterSubtractSelect => {
                self.enter_select(false, true, &mut ev);
                ev.push(EditorEvent::Status(format!("subtract box: {} notes selected", self.selection.len())));
            }
            Command::ConfirmSelection => {
                self.reset_select_state();
                self.set_mode(Mode::Normal, &mut ev);
                ev.push(EditorEvent::Status(format!("{} notes selected", self.selection.len())));
            }
            Command::SelectAll => {
                let track = self.track().ok_or(EditError::NoTracks)?;
                self.selection = select::all_ids(track);
                ev.push(EditorEvent::Status(format!("selected all {} notes", self.selection.len())));
            }
            Command::ClearSelection => {
                self.selection.clear();
                self.reset_select_state();
                ev.push(EditorEvent::Status("selection cleared".to_string()));
            }
            Command::SelectBar => self.select_bar(n, &mut ev)?,
            Command::SelectChord => {
                let tick = self.note_under_cursor().map(|n| n.start).unwrap_or(self.cursor.tick);
                let track = self.track().ok_or(EditError::NoTracks)?;
                let ids = select::chord_at(track, tick);
                let what = format!("in the chord at {}", self.bar_beat(tick));
                self.set_selection(ids, &what, &mut ev)?;
            }
            Command::SelectNextNotes { n: default_n } => {
                let wanted = usize::try_from(count.unwrap_or(*default_n).max(1)).unwrap_or(usize::MAX);
                let track = self.track().ok_or(EditError::NoTracks)?;
                let ids = select::next_n_from(track, self.cursor.tick, wanted);
                self.set_selection(ids, "from the cursor on", &mut ev)?;
            }
            Command::SelectToEnd => {
                let track = self.track().ok_or(EditError::NoTracks)?;
                let tick = self.cursor.tick;
                let ids = select::ids_where(track, |note| note.start >= tick);
                self.set_selection(ids, "from the cursor to the end", &mut ev)?;
            }
            Command::SelectToStart => {
                let track = self.track().ok_or(EditError::NoTracks)?;
                let tick = self.cursor.tick;
                let ids = select::ids_where(track, |note| note.start <= tick);
                self.set_selection(ids, "from the start to the cursor", &mut ev)?;
            }
            Command::SelectSamePitch => {
                let track = self.track().ok_or(EditError::NoTracks)?;
                let pitch = self.cursor.pitch;
                let ids = select::ids_where(track, |note| note.pitch == pitch);
                self.set_selection(ids, &format!("with pitch {}", pitch_name(pitch)), &mut ev)?;
            }
            Command::SelectVelocityAbove { value } => {
                let track = self.track().ok_or(EditError::NoTracks)?;
                let ids = select::ids_where(track, |note| note.velocity > *value);
                self.set_selection(ids, &format!("with velocity above {value}"), &mut ev)?;
            }
            Command::SelectVelocityBelow { value } => {
                let track = self.track().ok_or(EditError::NoTracks)?;
                let ids = select::ids_where(track, |note| note.velocity < *value);
                self.set_selection(ids, &format!("with velocity below {value}"), &mut ev)?;
            }
            Command::SelectTopOfChords => {
                let track = self.track().ok_or(EditError::NoTracks)?;
                let ids = select::chord_extremes(&track.notes, true, 2);
                self.set_selection(ids, "at the top of chords", &mut ev)?;
            }
            Command::SelectBottomOfChords => {
                let track = self.track().ok_or(EditError::NoTracks)?;
                let ids = select::chord_extremes(&track.notes, false, 2);
                self.set_selection(ids, "at the bottom of chords", &mut ev)?;
            }
            Command::InvertSelection => {
                let track = self.track().ok_or(EditError::NoTracks)?;
                let was = &self.selection;
                self.selection = select::ids_where(track, |note| !was.contains(&note.id));
                ev.push(EditorEvent::Status(format!("inverted: {} notes selected", self.selection.len())));
            }
            Command::SelectionFocusNext => self.focus_step(1, n, &mut ev)?,
            Command::SelectionFocusPrev => self.focus_step(-1, n, &mut ev)?,
            Command::DropFocused => self.drop_focused(&mut ev)?,
            Command::KeepTopOfChords => {
                let (ids, notes) = self.target_notes()?;
                let kept = select::chord_extremes(&notes, true, 1);
                self.keep(ids.len(), kept, "at the top of their chord", &mut ev)?;
            }
            Command::KeepBottomOfChords => {
                let (ids, notes) = self.target_notes()?;
                let kept = select::chord_extremes(&notes, false, 1);
                self.keep(ids.len(), kept, "at the bottom of their chord", &mut ev)?;
            }
            Command::KeepEveryOther { n: default_n } => {
                let every = count.unwrap_or(*default_n).max(1);
                let (ids, notes) = self.target_notes()?;
                let kept = select::every_nth(&notes, every);
                self.keep(ids.len(), kept, &format!("(every {})", ordinal(every)), &mut ev)?;
            }
            Command::KeepOnBeat => {
                let (meter, ppq) = (self.song.meter_map.clone(), self.song.ppq);
                self.keep_where("on a beat", |note| select::is_on_beat(&meter, ppq, note.start), &mut ev)?;
            }
            Command::KeepOffBeat => {
                let (meter, ppq) = (self.song.meter_map.clone(), self.song.ppq);
                self.keep_where("off the beat", |note| !select::is_on_beat(&meter, ppq, note.start), &mut ev)?;
            }
            Command::KeepAboveCursor => {
                let pitch = self.cursor.pitch;
                self.keep_where("above the cursor", |note| note.pitch > pitch, &mut ev)?;
            }
            Command::KeepBelowCursor => {
                let pitch = self.cursor.pitch;
                self.keep_where("below the cursor", |note| note.pitch < pitch, &mut ev)?;
            }
            Command::KeepAfterCursor => {
                let tick = self.cursor.tick;
                self.keep_where("at or after the cursor", |note| note.start >= tick, &mut ev)?;
            }
            Command::KeepBeforeCursor => {
                let tick = self.cursor.tick;
                self.keep_where("before the cursor", |note| note.start < tick, &mut ev)?;
            }

            // -------------------------------------------------------------- clipboard
            Command::Copy => self.copy(&mut ev)?,
            Command::Paste => self.paste(n, &mut ev)?,
            Command::Duplicate => self.duplicate(n, &mut ev)?,

            // -------------------------------------------------------------- view
            Command::SetGrid { grid } => {
                self.grid = *grid;
                ev.push(EditorEvent::Status(format!("grid {grid}")));
            }
            Command::CycleGrid { dir } => {
                for _ in 0..n {
                    self.grid = match dir {
                        CycleDir::Next => self.grid.next(),
                        CycleDir::Prev => self.grid.prev(),
                    };
                }
                ev.push(EditorEvent::Status(format!("grid {}", self.grid)));
            }
            Command::NextTrack => self.switch_track(n as i64, &mut ev)?,
            Command::PrevTrack => self.switch_track(-(n as i64), &mut ev)?,
            Command::ZoomIn => ev.push(EditorEvent::View(ViewRequest::ZoomTime(zoom_factor(ZOOM_TIME_STEP, n)))),
            Command::ZoomOut => {
                ev.push(EditorEvent::View(ViewRequest::ZoomTime(1.0 / zoom_factor(ZOOM_TIME_STEP, n))));
            }
            Command::ZoomRowsIn => ev.push(EditorEvent::View(ViewRequest::ZoomRows(zoom_factor(ZOOM_ROWS_STEP, n)))),
            Command::ZoomRowsOut => {
                ev.push(EditorEvent::View(ViewRequest::ZoomRows(1.0 / zoom_factor(ZOOM_ROWS_STEP, n))));
            }
            Command::ToggleCheatSheet => ev.push(EditorEvent::ToggleCheatSheet),

            // -------------------------------------------------------------- track
            Command::AddTrack => self.add_tracks(n, &mut ev)?,
            Command::RemoveTrack => self.remove_track(&mut ev)?,
            Command::DuplicateTrack => self.duplicate_track(n, &mut ev)?,
            Command::MoveTrackUp => self.move_track(-i64::from(n), &mut ev)?,
            Command::MoveTrackDown => self.move_track(i64::from(n), &mut ev)?,
            Command::RenameTrack { name } => self.rename_track(name, &mut ev)?,
            Command::SetTrackVolume { percent } => {
                let percent = i32::try_from(count.unwrap_or(u32::from(*percent))).unwrap_or(i32::MAX);
                self.set_track_volume(percent, None, &mut ev)?;
            }
            Command::NudgeTrackVolume { delta } => {
                let wanted = self.track_volume_percent()? + i32::from(*delta) * i32::try_from(n).unwrap_or(i32::MAX);
                self.set_track_volume(wanted, Some("track_volume"), &mut ev)?;
            }
            Command::SetTrackPan { value } => self.set_track_pan(i32::from(*value), None, &mut ev)?,
            Command::NudgeTrackPan { delta } => {
                let wanted = self.track_pan_percent()? + i32::from(*delta) * i32::try_from(n).unwrap_or(i32::MAX);
                self.set_track_pan(wanted, Some("track_pan"), &mut ev)?;
            }
            Command::ToggleMute => self.toggle_mute(&mut ev)?,
            Command::ToggleSolo => self.toggle_solo(&mut ev)?,
            Command::SetTrackChannel { channel } => self.set_track_channel(*channel, &mut ev)?,
            Command::SetInstrument { path, bank, preset } => self.set_instrument(path.clone(), *bank, *preset, &mut ev)?,
            Command::NextPreset => self.step_preset(i64::from(n), &mut ev)?,
            Command::PrevPreset => self.step_preset(-i64::from(n), &mut ev)?,
            Command::SetMasterReverb { percent } => {
                let percent = count.unwrap_or(u32::from(*percent)).min(100);
                self.commit(None);
                self.song.master_reverb = percent as f32 / 100.0;
                ev.push(EditorEvent::SongChanged);
                ev.push(EditorEvent::Status(format!("master reverb {percent}%")));
            }

            // -------------------------------------------------------------- lane
            Command::ShowLane { kind } => {
                self.lane = *kind;
                ev.push(EditorEvent::Status(lane_status(self.lane)));
            }
            Command::CycleLane => {
                for _ in 0..n {
                    self.lane = match self.lane {
                        None => Some(LaneKind::ALL[0]),
                        Some(LaneKind::Pan) => None,
                        Some(k) => Some(k.next()),
                    };
                }
                ev.push(EditorEvent::Status(lane_status(self.lane)));
            }
            Command::SetLaneValue { value } => {
                let value = count.map(|c| i32::try_from(c).unwrap_or(i32::MAX)).unwrap_or(*value);
                let kind = self.lane.ok_or(EditError::NoLaneShown)?;
                self.set_lane_point(kind, self.cursor.tick, value, None, &mut ev)?;
            }
            Command::NudgeLaneValue { delta } => self.nudge_lane_value(*delta, n, &mut ev)?,
            Command::RampLane { from, to } => self.ramp_lane(*from, *to, &mut ev)?,
            Command::ClearLane => self.clear_lane(&mut ev)?,
            Command::SustainDown => {
                self.set_lane_point(LaneKind::Sustain, self.cursor.tick, SUSTAIN_DOWN, None, &mut ev)?;
            }
            Command::SustainUp => {
                self.set_lane_point(LaneKind::Sustain, self.cursor.tick, SUSTAIN_UP, None, &mut ev)?;
            }
            Command::PedalSelection => self.pedal_selection(&mut ev)?,

            // -------------------------------------------------------------- transport
            Command::Play => ev.push(EditorEvent::Transport(TransportRequest::Play { from: Tick::ZERO })),
            Command::Stop => ev.push(EditorEvent::Transport(TransportRequest::Stop)),
            Command::TogglePlay => {
                ev.push(EditorEvent::Transport(TransportRequest::Toggle { from: self.cursor.tick }));
            }
            Command::PlayFromCursor => {
                ev.push(EditorEvent::Transport(TransportRequest::Play { from: self.cursor.tick }));
            }

            // -------------------------------------------------------------- mode
            Command::EnterCommandLine => self.enter_mode(Mode::CommandLine, &mut ev),
        }
        if self.committed {
            self.last_edit = Some(cmd.clone());
            self.last_edit_count = count;
        }
        let keeps_focus = matches!(cmd, Command::SelectionFocusNext | Command::SelectionFocusPrev | Command::DropFocused);
        if !keeps_focus && self.selection != selection_before {
            self.focus = None;
        }
        if self.focus.is_some_and(|f| !self.selection.contains(&f)) {
            self.focus = None;
        }
        Ok(ev)
    }

    fn move_cursor(&mut self, dir: Dir, unit: Unit, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        match dir {
            Dir::Left | Dir::Right => {
                let step = self.time_step(unit, dir)?.mul(u64::from(n));
                self.cursor.tick = match dir {
                    Dir::Left => self.cursor.tick.saturating_sub(step),
                    _ => self.cursor.tick.saturating_add(step),
                };
            }
            Dir::Up | Dir::Down => {
                let step = u32::from(Self::pitch_step(unit, dir)?) * n;
                let p = u32::from(self.cursor.pitch);
                self.cursor.pitch = match dir {
                    Dir::Up => p.saturating_add(step).min(u32::from(edit_ops::MAX_PITCH)) as u8,
                    _ => p.saturating_sub(step) as u8,
                };
            }
        }
        if self.mode == Mode::Select {
            self.refresh_box_selection();
            let what = if self.select_subtract { "selected" } else { "in box" };
            ev.push(EditorEvent::Status(format!("{} notes {what}", self.selection.len())));
        }
        Ok(())
    }

    fn insert_note(&mut self, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        if self.track().is_none() {
            return Err(EditError::NoTracks);
        }
        let duration = self.grid_ticks().mul(u64::from(n));
        self.commit(None);
        let note = self.song.new_note(self.cursor.tick, duration, self.cursor.pitch, INSERT_VELOCITY);
        let replaced = self.track_mut().insert_note(note);
        for id in replaced {
            self.selection.remove(&id);
        }
        self.selection.clear();
        self.selection.insert(note.id);
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Audition(vec![note]));
        ev.push(EditorEvent::Status(format!("inserted {} ({})", pitch_name(note.pitch), self.grid)));
        Ok(())
    }

    fn insert_chord(&mut self, intervals: &[i8], n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        if self.track().is_none() {
            return Err(EditError::NoTracks);
        }
        let mut pitches: Vec<u8> = intervals
            .iter()
            .filter_map(|i| u8::try_from(i32::from(self.cursor.pitch) + i32::from(*i)).ok())
            .filter(|p| *p <= edit_ops::MAX_PITCH)
            .collect();
        pitches.sort_unstable();
        pitches.dedup();
        if pitches.is_empty() {
            return Err(EditError::OutOfRange("no chord note lands inside the pitch range".to_string()));
        }
        let duration = self.grid_ticks().mul(u64::from(n));
        self.commit(None);
        let mut inserted = Vec::with_capacity(pitches.len());
        for pitch in pitches {
            let note = self.song.new_note(self.cursor.tick, duration, pitch, INSERT_VELOCITY);
            self.track_mut().insert_note(note);
            inserted.push(note);
        }
        self.selection = inserted.iter().map(|n| n.id).collect();
        let names: Vec<String> = inserted.iter().map(|n| pitch_name(n.pitch)).collect();
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Audition(inserted));
        ev.push(EditorEvent::Status(format!("inserted chord {} ({})", names.join(" "), self.grid)));
        Ok(())
    }

    fn delete_selected(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        self.commit(None);
        let removed = self.track_mut().remove_notes(&ids);
        for id in &ids {
            self.selection.remove(id);
        }
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("deleted {removed} note{}", plural(removed))));
        Ok(())
    }

    fn toggle_select_under_cursor(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let note = *self.note_under_cursor().ok_or(EditError::NoNoteUnderCursor)?;
        let now_selected = if self.selection.remove(&note.id) {
            false
        } else {
            self.selection.insert(note.id);
            true
        };
        ev.push(EditorEvent::Audition(vec![note]));
        ev.push(EditorEvent::Status(format!(
            "{} {}",
            if now_selected { "selected" } else { "deselected" },
            pitch_name(note.pitch)
        )));
        Ok(())
    }

    fn enter_mode(&mut self, mode: Mode, ev: &mut Vec<EditorEvent>) {
        match mode {
            Mode::Select => self.enter_select(false, false, ev),
            Mode::Normal => {
                if self.mode == Mode::Select {
                    // cancel: a plain box is dropped, a subtract box is put back
                    self.selection = if self.select_subtract { std::mem::take(&mut self.select_base) } else { BTreeSet::new() };
                }
                self.reset_select_state();
                self.set_mode(Mode::Normal, ev);
            }
            Mode::Move | Mode::CommandLine => {
                self.reset_select_state();
                self.set_mode(mode, ev);
            }
        }
    }

    /// Enters Select mode with the box anchored at the cursor. `all_pitches` makes it
    /// a time range; `subtract` makes it remove from the current selection.
    fn enter_select(&mut self, all_pitches: bool, subtract: bool, ev: &mut Vec<EditorEvent>) {
        self.select_anchor = Some(self.cursor);
        self.select_all_pitches = all_pitches;
        self.select_subtract = subtract;
        self.select_base = if subtract { self.selection.clone() } else { BTreeSet::new() };
        self.set_mode(Mode::Select, ev);
        self.refresh_box_selection();
    }

    /// Replaces the selection with `ids`, or fails (leaving it) if there are none.
    fn set_selection(&mut self, ids: BTreeSet<NoteId>, what: &str, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        if ids.is_empty() {
            return Err(EditError::NoMatch(format!("no notes {what}")));
        }
        let count = ids.len();
        self.selection = ids;
        ev.push(EditorEvent::Status(format!("selected {count} note{} {what}", plural(count))));
        Ok(())
    }

    fn select_bar(&mut self, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let track = self.track().ok_or(EditError::NoTracks)?;
        let ppq = self.song.ppq;
        let bar = self.song.meter_map.bar_beat_of(ppq, self.cursor.tick).bar;
        let lo = self.song.meter_map.tick_of_bar(ppq, bar);
        let hi = self.song.meter_map.tick_of_bar(ppq, bar.saturating_add(n));
        let ids = select::ids_starting_in(track, lo, hi);
        let what = if n == 1 { format!("in bar {bar}") } else { format!("in bars {bar}-{}", bar + n - 1) };
        self.set_selection(ids, &what, ev)
    }

    /// The target ids plus their notes in track order.
    fn target_notes(&self) -> Result<(BTreeSet<NoteId>, Vec<Note>), EditError> {
        let ids = self.target_ids()?;
        let notes = select::notes_with_ids(&self.song.tracks[self.track_index], &ids);
        Ok((ids, notes))
    }

    /// Thins the selection to `kept`, or fails (leaving it) if nothing would remain.
    fn keep(&mut self, before: usize, kept: BTreeSet<NoteId>, what: &str, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        if kept.is_empty() {
            return Err(EditError::NoMatch(format!("no selected notes {what}")));
        }
        let count = kept.len();
        self.selection = kept;
        ev.push(EditorEvent::Status(format!("kept {count} of {before} note{} {what}", plural(before))));
        Ok(())
    }

    fn keep_where(&mut self, what: &str, pred: impl Fn(&Note) -> bool, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let (ids, notes) = self.target_notes()?;
        let kept = notes.iter().filter(|n| pred(n)).map(|n| n.id).collect();
        self.keep(ids.len(), kept, what, ev)
    }

    /// The selected notes of the current track in Tab order.
    fn focus_order(&self) -> Result<Vec<NoteId>, EditError> {
        let track = self.track().ok_or(EditError::NoTracks)?;
        let order = select::in_time_order(&select::notes_with_ids(track, &self.selection));
        if order.is_empty() {
            return Err(EditError::NothingSelected);
        }
        Ok(order)
    }

    /// Puts the cursor on the focused note and reports it.
    fn show_focus(&mut self, id: NoteId, index: usize, total: usize, ev: &mut Vec<EditorEvent>) {
        let Some(note) = self.song.tracks[self.track_index].note(id).copied() else { return };
        self.cursor = Cursor { tick: note.start, pitch: note.pitch };
        ev.push(EditorEvent::Audition(vec![note]));
        ev.push(EditorEvent::Status(format!(
            "focus {}/{total}: {} at {}",
            index + 1,
            pitch_name(note.pitch),
            self.bar_beat(note.start)
        )));
    }

    fn focus_step(&mut self, delta: i64, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let order = self.focus_order()?;
        let len = i64::try_from(order.len()).unwrap_or(i64::MAX);
        let steps = i64::from(n);
        let target = match self.focus.and_then(|f| order.iter().position(|id| *id == f)) {
            Some(i) => i64::try_from(i).unwrap_or(0) + delta * steps,
            // the first step lands on an end of the list, the rest walk from there
            None => (if delta > 0 { 0 } else { len - 1 }) + delta * (steps - 1),
        };
        let index = usize::try_from(target.rem_euclid(len)).unwrap_or(0);
        let id = order[index];
        self.focus = Some(id);
        self.show_focus(id, index, order.len(), ev);
        Ok(())
    }

    fn drop_focused(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let focused = self.focus.ok_or(EditError::NothingFocused)?;
        let order = self.focus_order()?;
        let pos = order.iter().position(|id| *id == focused).ok_or(EditError::NothingFocused)?;
        let dropped = self.song.tracks[self.track_index].note(focused).map(|n| pitch_name(n.pitch)).unwrap_or_default();
        self.selection.remove(&focused);
        let remaining: Vec<NoteId> = order.into_iter().filter(|id| *id != focused).collect();
        ev.push(EditorEvent::Status(format!("dropped {dropped}; {} notes selected", remaining.len())));
        self.focus = if remaining.is_empty() {
            None
        } else {
            let index = pos % remaining.len();
            self.show_focus(remaining[index], index, remaining.len(), ev);
            Some(remaining[index])
        };
        Ok(())
    }

    fn move_selected(&mut self, dir: Dir, unit: Unit, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let notes = select::notes_with_ids(&self.song.tracks[self.track_index], &ids);
        let count = notes.len();
        match dir {
            Dir::Left | Dir::Right => {
                let step = i64::try_from(self.time_step(unit, dir)?.mul(u64::from(n)).get()).unwrap_or(i64::MAX);
                let wanted = if dir == Dir::Left { -step } else { step };
                let delta = edit_ops::clamp_time_shift(&notes, wanted);
                if delta == 0 {
                    return Err(EditError::OutOfRange("already at the start of the song".to_string()));
                }
                self.commit(Some("move_selected"));
                let moved = edit_ops::shift_time(self.track_mut(), &ids, delta);
                self.cursor.tick = if delta < 0 {
                    self.cursor.tick.saturating_sub(Tick(delta.unsigned_abs()))
                } else {
                    self.cursor.tick.saturating_add(Tick(delta.unsigned_abs()))
                };
                let steps = delta.unsigned_abs() / self.time_step(unit, dir)?.get().max(1);
                ev.push(EditorEvent::SongChanged);
                ev.push(EditorEvent::Audition(moved));
                ev.push(EditorEvent::Status(format!(
                    "moved {count} note{} {}{steps} {unit}{}",
                    plural(count),
                    if delta < 0 { "-" } else { "+" },
                    plural(steps as usize)
                )));
            }
            Dir::Up | Dir::Down => {
                let step = i32::from(Self::pitch_step(unit, dir)?) * i32::try_from(n).unwrap_or(i32::MAX);
                let wanted = if dir == Dir::Down { -step } else { step };
                let delta = edit_ops::clamp_pitch_shift(&notes, wanted);
                if delta == 0 {
                    return Err(EditError::OutOfRange("notes are already at the pitch limit".to_string()));
                }
                self.commit(Some("move_selected"));
                let moved = edit_ops::shift_pitch(self.track_mut(), &ids, delta);
                self.cursor.pitch = (i32::from(self.cursor.pitch) + delta).clamp(0, i32::from(edit_ops::MAX_PITCH)) as u8;
                ev.push(EditorEvent::SongChanged);
                ev.push(EditorEvent::Audition(moved));
                ev.push(EditorEvent::Status(format!("moved {count} note{} {delta:+} st", plural(count))));
            }
        }
        Ok(())
    }

    fn nudge_selected(&mut self, ticks: i64, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let (ids, notes) = self.target_notes()?;
        let wanted = ticks.saturating_mul(i64::from(n));
        let delta = edit_ops::clamp_time_shift(&notes, wanted);
        if delta == 0 {
            return Err(EditError::OutOfRange("already at the start of the song".to_string()));
        }
        self.commit(Some("nudge_selected"));
        let moved = edit_ops::shift_time(self.track_mut(), &ids, delta);
        let count = moved.len();
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Audition(moved));
        ev.push(EditorEvent::Status(format!("nudged {count} note{} {delta:+} ticks", plural(count))));
        Ok(())
    }

    fn change_length(&mut self, delta_steps: i32, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let step = i64::try_from(self.grid_ticks().get()).unwrap_or(i64::MAX);
        let delta = i64::from(delta_steps) * i64::from(n) * step;
        let floor = Tick((self.grid_ticks().get() / 4).max(1));
        // probe on a copy so a no-op never pushes an undo entry
        let mut probe = self.song.tracks[self.track_index].clone();
        if edit_ops::change_length(&mut probe, &ids, delta, floor) == 0 {
            return Err(EditError::OutOfRange("length already at the minimum".to_string()));
        }
        self.commit(Some("change_length"));
        self.song.tracks[self.track_index] = probe;
        let count = ids.len();
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!(
            "{} {count} note{} by {} grid step{}",
            if delta >= 0 { "lengthened" } else { "shortened" },
            plural(count),
            (delta_steps * i32::try_from(n).unwrap_or(i32::MAX)).abs(),
            plural((delta_steps * i32::try_from(n).unwrap_or(i32::MAX)).unsigned_abs() as usize)
        )));
        Ok(())
    }

    fn set_length(&mut self, steps: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let steps = steps.max(1);
        let len = self.grid_ticks().mul(u64::from(steps));
        let count = ids.len();
        let mut probe = self.song.tracks[self.track_index].clone();
        if edit_ops::set_length(&mut probe, &ids, len) > 0 {
            self.commit(None);
            self.song.tracks[self.track_index] = probe;
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!(
            "length {steps} grid step{} on {count} note{}",
            plural(steps as usize),
            plural(count)
        )));
        Ok(())
    }

    fn legato(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let mut probe = self.song.tracks[self.track_index].clone();
        let changed = edit_ops::legato(&mut probe, &ids);
        if changed == 0 {
            ev.push(EditorEvent::Status("nothing to extend".to_string()));
            return Ok(());
        }
        self.commit(None);
        self.song.tracks[self.track_index] = probe;
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("legato: extended {changed} note{}", plural(changed))));
        Ok(())
    }

    fn nudge_velocity(&mut self, delta: i32, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let delta = delta.saturating_mul(i32::try_from(n).unwrap_or(i32::MAX));
        let mut probe = self.song.tracks[self.track_index].clone();
        if edit_ops::nudge_velocity(&mut probe, &ids, delta) == 0 {
            return Err(EditError::OutOfRange("velocity already at the limit".to_string()));
        }
        self.commit(Some("nudge_velocity"));
        self.song.tracks[self.track_index] = probe;
        let count = ids.len();
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("velocity {delta:+} on {count} note{}", plural(count))));
        Ok(())
    }

    fn set_velocity(&mut self, value: u8, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let value = value.clamp(edit_ops::MIN_VELOCITY, edit_ops::MAX_VELOCITY);
        let count = ids.len();
        let mut probe = self.song.tracks[self.track_index].clone();
        if edit_ops::set_velocity(&mut probe, &ids, value) > 0 {
            self.commit(None);
            self.song.tracks[self.track_index] = probe;
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!("velocity {value} on {count} note{}", plural(count))));
        Ok(())
    }

    fn ramp_velocity(&mut self, from: u8, to: u8, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let count = ids.len();
        let mut probe = self.song.tracks[self.track_index].clone();
        if edit_ops::ramp_velocity(&mut probe, &ids, from, to) > 0 {
            self.commit(None);
            self.song.tracks[self.track_index] = probe;
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!("velocity ramp {from} → {to} on {count} note{}", plural(count))));
        Ok(())
    }

    fn humanize(&mut self, velocity: u8, timing: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        if velocity == 0 && timing == 0 {
            ev.push(EditorEvent::Status("nothing to humanize: both amounts are 0".to_string()));
            return Ok(());
        }
        self.commit(None);
        let mut rng = XorShift64(self.rng_seed);
        let touched = edit_ops::humanize(self.track_mut(), &ids, velocity, timing, &mut rng);
        self.rng_seed = rng.0;
        let count = touched.len();
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!(
            "humanized {count} note{} (velocity ±{velocity}, timing ±{timing} ticks)",
            plural(count)
        )));
        Ok(())
    }

    fn quantize(&mut self, grid: Option<Grid>, strength: u8, swing: u8, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let grid = grid.unwrap_or(self.grid);
        let strength = strength.min(100);
        let swing = if swing == 0 { self.swing } else { swing.min(100) };
        let count = ids.len();
        let mut probe = self.song.tracks[self.track_index].clone();
        let moved = edit_ops::quantize_starts(&mut probe, &ids, grid, self.song.ppq, strength, f32::from(swing) / 100.0);
        if moved == 0 {
            ev.push(EditorEvent::Status(format!("already on the {grid} grid")));
            return Ok(());
        }
        self.commit(None);
        self.song.tracks[self.track_index] = probe;
        let mut status = format!("quantized {moved} of {count} note{} to {grid}", plural(count));
        if strength != 100 {
            status.push_str(&format!(" at {strength}%"));
        }
        if swing != 0 {
            status.push_str(&format!(" with {swing}% swing"));
        }
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(status));
        Ok(())
    }

    fn quantize_length(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let count = ids.len();
        let mut probe = self.song.tracks[self.track_index].clone();
        let changed = edit_ops::quantize_lengths(&mut probe, &ids, self.grid_ticks());
        if changed == 0 {
            ev.push(EditorEvent::Status(format!("lengths already on the {} grid", self.grid)));
            return Ok(());
        }
        self.commit(None);
        self.song.tracks[self.track_index] = probe;
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!(
            "rounded {changed} of {count} note length{} to {}",
            plural(count),
            self.grid
        )));
        Ok(())
    }

    fn transpose(&mut self, semitones: i32, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let notes = select::notes_with_ids(&self.song.tracks[self.track_index], &ids);
        let wanted = semitones.saturating_mul(i32::try_from(n).unwrap_or(i32::MAX));
        let delta = edit_ops::clamp_pitch_shift(&notes, wanted);
        if delta == 0 {
            return Err(EditError::OutOfRange("notes are already at the pitch limit".to_string()));
        }
        self.commit(Some("transpose"));
        let moved = edit_ops::shift_pitch(self.track_mut(), &ids, delta);
        let count = moved.len();
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Audition(moved));
        ev.push(EditorEvent::Status(format!("transposed {count} note{} {delta:+} st", plural(count))));
        Ok(())
    }

    fn copy(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        self.clipboard = edit_ops::clip_relative(&self.song.tracks[self.track_index], &ids);
        let count = self.clipboard.len();
        ev.push(EditorEvent::Status(format!("copied {count} note{}", plural(count))));
        Ok(())
    }

    fn paste(&mut self, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        if self.clipboard.is_empty() {
            return Err(EditError::ClipboardEmpty);
        }
        if self.track().is_none() {
            return Err(EditError::NoTracks);
        }
        let clip = self.clipboard.clone();
        self.commit(None);
        let pasted = self.paste_copies(&clip, self.cursor.tick, n);
        let count = pasted.len();
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Audition(pasted));
        ev.push(EditorEvent::Status(format!("pasted {count} note{}", plural(count))));
        Ok(())
    }

    fn duplicate(&mut self, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let ids = self.target_ids()?;
        let track = &self.song.tracks[self.track_index];
        let clip = edit_ops::clip_relative(track, &ids);
        let end = edit_ops::group_end(&select::notes_with_ids(track, &ids), self.cursor.tick);
        self.commit(None);
        let pasted = self.paste_copies(&clip, end, n);
        let count = pasted.len();
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Audition(pasted));
        ev.push(EditorEvent::Status(format!("duplicated {count} note{}", plural(count))));
        Ok(())
    }

    /// Pastes `clip` `n` times back to back from `at`; selects the result and moves the
    /// cursor to its end.
    fn paste_copies(&mut self, clip: &[Note], at: Tick, n: u32) -> Vec<Note> {
        let mut all = Vec::new();
        let mut at = at;
        for _ in 0..n {
            let pasted = edit_ops::paste(&mut self.song, self.track_index, clip, at);
            at = edit_ops::group_end(&pasted, at);
            all.extend(pasted);
        }
        self.selection = all.iter().map(|n| n.id).collect();
        self.cursor.tick = at;
        all
    }

    fn undo_n(&mut self, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let mut done = 0;
        for _ in 0..n {
            let current = self.snapshot();
            match self.undo.undo(current) {
                Some(s) => {
                    self.restore(s);
                    done += 1;
                }
                None => break,
            }
        }
        if done == 0 {
            return Err(EditError::NothingToUndo);
        }
        self.committed = false;
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("undo ({} left)", self.undo.undo_depth())));
        Ok(())
    }

    fn redo_n(&mut self, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let mut done = 0;
        for _ in 0..n {
            let current = self.snapshot();
            match self.undo.redo(current) {
                Some(s) => {
                    self.restore(s);
                    done += 1;
                }
                None => break,
            }
        }
        if done == 0 {
            return Err(EditError::NothingToRedo);
        }
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("redo ({} left)", self.undo.redo_depth())));
        Ok(())
    }

    fn switch_track(&mut self, delta: i64, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let len = i64::try_from(self.song.tracks.len()).unwrap_or(0);
        if len == 0 {
            return Err(EditError::NoTracks);
        }
        let cur = i64::try_from(self.track_index).unwrap_or(0);
        let next = (cur + delta).rem_euclid(len);
        self.show_track(usize::try_from(next).unwrap_or(0));
        let name = self.song.tracks[self.track_index].name.clone();
        ev.push(EditorEvent::Status(format!("track {}/{}: {}", self.track_index + 1, len, name)));
        Ok(())
    }

    // ------------------------------------------------------------------ tracks

    /// Shows track `index` (already valid): switching tracks drops the selection and
    /// any box in progress, since the ids belong to the old track.
    fn show_track(&mut self, index: usize) {
        self.track_index = index;
        self.selection.clear();
        self.reset_select_state();
    }

    fn current_track(&self) -> Result<&Track, EditError> {
        self.track().ok_or(EditError::NoTracks)
    }

    /// `"2/3: Bass"` for status strings.
    fn track_label(&self) -> String {
        let name = self.track().map(|t| t.name.as_str()).unwrap_or("");
        format!("{}/{}: {name}", self.track_index + 1, self.song.tracks.len())
    }

    fn track_volume_percent(&self) -> Result<i32, EditError> {
        Ok((self.current_track()?.volume * 100.0).round() as i32)
    }

    fn track_pan_percent(&self) -> Result<i32, EditError> {
        Ok((self.current_track()?.pan * 100.0).round() as i32)
    }

    fn add_tracks(&mut self, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        self.commit(None);
        let mut index = self.track_index;
        for _ in 0..n {
            let name = self.song.next_track_name();
            index = self.song.insert_track(index + 1, Track::new(name));
        }
        self.show_track(index);
        ev.push(EditorEvent::SongChanged);
        let added = if n == 1 { "added track".to_string() } else { format!("added {n} tracks, showing") };
        ev.push(EditorEvent::Status(format!("{added} {}", self.track_label())));
        Ok(())
    }

    fn remove_track(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let name = self.current_track()?.name.clone();
        if self.song.tracks.len() == 1 {
            return Err(EditError::LastTrack);
        }
        self.commit(None);
        self.song.remove_track(self.track_index);
        let index = self.track_index.min(self.song.tracks.len() - 1);
        self.show_track(index);
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("removed track {name}; showing {}", self.track_label())));
        Ok(())
    }

    fn duplicate_track(&mut self, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let name = self.current_track()?.name.clone();
        self.commit(None);
        let source = self.track_index;
        let mut index = source;
        for _ in 0..n {
            let Some(copy) = self.song.duplicate_track(source) else { break };
            index = self.song.insert_track(index + 1, copy);
        }
        self.show_track(index);
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("duplicated track {name}; showing {}", self.track_label())));
        Ok(())
    }

    /// Moves the current track `delta` places (negative = up), stopping at the ends;
    /// the view follows it and the selection (same notes) is kept.
    fn move_track(&mut self, delta: i64, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let name = self.current_track()?.name.clone();
        let last = i64::try_from(self.song.tracks.len() - 1).unwrap_or(i64::MAX);
        let cur = i64::try_from(self.track_index).unwrap_or(0);
        let target = (cur + delta).clamp(0, last);
        if target == cur {
            let edge = if delta < 0 { "first" } else { "last" };
            return Err(EditError::OutOfRange(format!("{name} is already the {edge} track")));
        }
        self.commit(None);
        let track = self.song.tracks.remove(self.track_index);
        let target = usize::try_from(target).unwrap_or(0);
        self.song.tracks.insert(target, track);
        self.track_index = target;
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!(
            "moved track {} to {}",
            if delta < 0 { "up" } else { "down" },
            self.track_label()
        )));
        Ok(())
    }

    fn rename_track(&mut self, name: &str, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EditError::BadValue("a track name cannot be empty".to_string()));
        }
        if self.current_track()?.name != name {
            self.commit(None);
            self.track_mut().name = name.to_string();
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!("renamed track to {name}")));
        Ok(())
    }

    /// Sets the track volume to `percent` (clamped to 0..=200). `coalesce` is the undo
    /// key for nudges, which also makes "no change" an error instead of a no-op.
    fn set_track_volume(&mut self, percent: i32, coalesce: Option<&str>, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let percent = percent.clamp(0, MAX_TRACK_VOLUME_PERCENT);
        if self.track_volume_percent()? == percent {
            if coalesce.is_some() {
                return Err(EditError::OutOfRange(format!("volume already at {percent}%")));
            }
        } else {
            self.commit(coalesce);
            self.track_mut().volume = percent as f32 / 100.0;
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!("volume {percent}% on {}", self.track_label())));
        Ok(())
    }

    /// Sets the track pan to `value` (clamped to -100..=100); see [`Editor::set_track_volume`].
    fn set_track_pan(&mut self, value: i32, coalesce: Option<&str>, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let value = value.clamp(-100, 100);
        if self.track_pan_percent()? == value {
            if coalesce.is_some() {
                return Err(EditError::OutOfRange(format!("pan already at {}", pan_label(value))));
            }
        } else {
            self.commit(coalesce);
            self.track_mut().pan = value as f32 / 100.0;
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!("pan {} on {}", pan_label(value), self.track_label())));
        Ok(())
    }

    fn toggle_mute(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let name = self.current_track()?.name.clone();
        self.commit(None);
        let track = self.track_mut();
        track.mute = !track.mute;
        let muted = track.mute;
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("{} {name}", if muted { "muted" } else { "unmuted" })));
        Ok(())
    }

    fn toggle_solo(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let name = self.current_track()?.name.clone();
        self.commit(None);
        let track = self.track_mut();
        track.solo = !track.solo;
        let solo = track.solo;
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("{} {name}", if solo { "solo" } else { "solo off:" })));
        Ok(())
    }

    fn set_track_channel(&mut self, channel: u8, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        if channel > MAX_CHANNEL {
            return Err(EditError::BadValue(format!("channel {} is not 1-16", u32::from(channel) + 1)));
        }
        if self.current_track()?.channel != channel {
            self.commit(None);
            self.track_mut().channel = channel;
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!("channel {} on {}", u32::from(channel) + 1, self.track_label())));
        Ok(())
    }

    fn set_instrument(
        &mut self,
        path: Option<std::path::PathBuf>,
        bank: u16,
        preset: u16,
        ev: &mut Vec<EditorEvent>,
    ) -> Result<(), EditError> {
        if preset > MAX_PRESET {
            return Err(EditError::BadValue(format!("preset {preset} is not 0-{MAX_PRESET}")));
        }
        let instrument = Instrument::SoundFont { path: path.unwrap_or_default(), bank, preset };
        if self.current_track()?.instrument != instrument {
            self.commit(None);
            self.track_mut().instrument = instrument;
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!("{} on {}", instrument_label(&self.song.tracks[self.track_index].instrument), self.track_label())));
        Ok(())
    }

    /// Moves the preset `delta` steps, wrapping within 0..=127.
    fn step_preset(&mut self, delta: i64, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let Instrument::SoundFont { preset, .. } = &self.current_track()?.instrument;
        let preset = *preset;
        let wrapped = (i64::from(preset) + delta).rem_euclid(i64::from(MAX_PRESET) + 1);
        let next = u16::try_from(wrapped).unwrap_or(0);
        if next != preset {
            self.commit(Some("preset"));
            let Instrument::SoundFont { preset, .. } = &mut self.track_mut().instrument;
            *preset = next;
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!("{} on {}", instrument_label(&self.song.tracks[self.track_index].instrument), self.track_label())));
        Ok(())
    }

    // ------------------------------------------------------------------ lanes

    /// Puts `value` (clamped) at `tick` in `kind`; a point already holding that value
    /// is left alone without an undo step.
    fn set_lane_point(
        &mut self,
        kind: LaneKind,
        tick: Tick,
        value: i32,
        coalesce: Option<&str>,
        ev: &mut Vec<EditorEvent>,
    ) -> Result<(), EditError> {
        let value = kind.clamp(value);
        if self.current_track()?.lanes.point_at(kind, tick) != Some(value) {
            self.commit(coalesce);
            self.track_mut().lanes.set_point(kind, tick, value);
            ev.push(EditorEvent::SongChanged);
        }
        ev.push(EditorEvent::Status(format!("{kind} {value} at {}", self.bar_beat(tick))));
        Ok(())
    }

    fn nudge_lane_value(&mut self, delta: i32, n: u32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let kind = self.lane.ok_or(EditError::NoLaneShown)?;
        let tick = self.cursor.tick;
        let lanes = &self.current_track()?.lanes;
        let current = lanes.point_at(kind, tick).unwrap_or_else(|| lanes.value_at(kind, tick));
        let scale = if kind == LaneKind::PitchBend { PITCH_BEND_NUDGE_SCALE } else { 1 };
        let step = delta.saturating_mul(scale).saturating_mul(i32::try_from(n).unwrap_or(i32::MAX));
        let wanted = kind.clamp(current.saturating_add(step));
        if wanted == current {
            return Err(EditError::OutOfRange(format!("{kind} already at {current}")));
        }
        self.set_lane_point(kind, tick, wanted, Some("lane_value"), ev)
    }

    /// The time range the lane commands cover, `(from, to)` inclusive: the selection's
    /// first start to its last end, else the cursor to the end of the note under it,
    /// else one bar from the cursor. `None` when the second and third do not apply and
    /// `fallback_to_bar` is false.
    fn lane_time_range(&self, fallback_to_bar: bool) -> Result<Option<(Tick, Tick)>, EditError> {
        let track = self.current_track()?;
        let selected = select::notes_with_ids(track, &self.selection);
        if let (Some(lo), Some(hi)) = (selected.iter().map(|n| n.start).min(), selected.iter().map(Note::end).max()) {
            return Ok(Some((lo, hi)));
        }
        if !fallback_to_bar {
            return Ok(None);
        }
        if let Some(note) = self.note_under_cursor() {
            return Ok(Some((self.cursor.tick, note.end())));
        }
        let bar = self.song.meter_map.bar_ticks_at(self.song.ppq, self.cursor.tick);
        Ok(Some((self.cursor.tick, self.cursor.tick.saturating_add(bar))))
    }

    fn ramp_lane(&mut self, from: i32, to: i32, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let kind = self.lane.ok_or(EditError::NoLaneShown)?;
        let (lo, hi) = self.lane_time_range(true)?.unwrap_or((self.cursor.tick, self.cursor.tick));
        let span = hi.saturating_sub(lo).get();
        if span == 0 {
            return Err(EditError::OutOfRange("nothing to ramp across".to_string()));
        }
        let (from, to) = (kind.clamp(from), kind.clamp(to));
        let step = self.grid_ticks().get().max(1);
        self.commit(None);
        let lanes = &mut self.track_mut().lanes;
        lanes.remove_range(kind, lo, hi.saturating_add(Tick(1)));
        let mut count = 0;
        let mut t = lo.get();
        while t < hi.get() {
            let frac = (t - lo.get()) as f64 / span as f64;
            let value = (f64::from(from) + f64::from(to - from) * frac).round() as i32;
            lanes.set_point(kind, Tick(t), value);
            count += 1;
            t = t.saturating_add(step);
        }
        lanes.set_point(kind, hi, to);
        count += 1;
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!(
            "ramped {kind} {from} → {to}: {count} points from {} to {}",
            self.bar_beat(lo),
            self.bar_beat(hi)
        )));
        Ok(())
    }

    fn clear_lane(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let kind = self.lane.ok_or(EditError::NoLaneShown)?;
        let range = self.lane_time_range(false)?;
        let lanes = &self.current_track()?.lanes;
        let (count, what) = match range {
            Some((lo, hi)) => (lanes.get(kind).iter().filter(|p| p.tick >= lo && p.tick <= hi).count(), " in the selection"),
            None => (lanes.get(kind).len(), ""),
        };
        if count == 0 {
            ev.push(EditorEvent::Status(format!("no {kind} points{what}")));
            return Ok(());
        }
        self.commit(None);
        let lanes = &mut self.track_mut().lanes;
        match range {
            Some((lo, hi)) => lanes.remove_range(kind, lo, hi.saturating_add(Tick(1))),
            None => std::mem::take(lanes.get_mut(kind)).len(),
        };
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("cleared {count} {kind} point{}{what}", plural(count))));
        Ok(())
    }

    fn pedal_selection(&mut self, ev: &mut Vec<EditorEvent>) -> Result<(), EditError> {
        let (_, notes) = self.target_notes()?;
        let lo = notes.iter().map(|n| n.start).min().unwrap_or(self.cursor.tick);
        let hi = notes.iter().map(Note::end).max().unwrap_or(self.cursor.tick);
        self.commit(None);
        let lanes = &mut self.track_mut().lanes;
        lanes.remove_range(LaneKind::Sustain, lo, hi.saturating_add(Tick(1)));
        lanes.set_point(LaneKind::Sustain, lo, SUSTAIN_DOWN);
        lanes.set_point(LaneKind::Sustain, hi, SUSTAIN_UP);
        ev.push(EditorEvent::SongChanged);
        ev.push(EditorEvent::Status(format!("pedal {} → {}", self.bar_beat(lo), self.bar_beat(hi))));
        Ok(())
    }
}

/// `"lane: sustain"` / `"lane hidden"` for the status bar.
fn lane_status(lane: Option<LaneKind>) -> String {
    match lane {
        Some(kind) => format!("lane: {kind}"),
        None => "lane hidden".to_string(),
    }
}

/// `"sf2 gm.sf2 preset 33 bank 0"` (or `"sf2 default ..."`) for the status bar.
fn instrument_label(instrument: &Instrument) -> String {
    match instrument {
        Instrument::SoundFont { path, bank, preset } => {
            let file = if path.as_os_str().is_empty() { "default".to_string() } else { path.display().to_string() };
            format!("sf2 {file} preset {preset} bank {bank}")
        }
    }
}

/// Note name with octave, middle C (60) = `C4`.
pub fn pitch_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let octave = i32::from(pitch / 12) - 1;
    format!("{}{}", NAMES[usize::from(pitch % 12)], octave)
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// `2nd`, `3rd`, `4th`, ... for status strings.
fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// `step` raised to the count, so `3 zoom_in` zooms three steps at once.
fn zoom_factor(step: f32, n: u32) -> f32 {
    step.powi(i32::try_from(n).unwrap_or(i32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_names() {
        assert_eq!(pitch_name(60), "C4");
        assert_eq!(pitch_name(61), "C#4");
        assert_eq!(pitch_name(0), "C-1");
        assert_eq!(pitch_name(127), "G9");
    }

    #[test]
    fn ordinals_and_zoom() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(4), "4th");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(13), "13th");
        assert_eq!(ordinal(22), "22nd");
        assert_eq!(ordinal(101), "101st");
        assert!((zoom_factor(2.0, 3) - 8.0).abs() < 1e-6);
        assert!((zoom_factor(2.0, 1) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn mode_names_round_trip() {
        for m in Mode::ALL {
            assert_eq!(Mode::from_name(m.name()), Some(m));
            assert!(!m.to_string().is_empty());
        }
        assert_eq!(Mode::from_name("bogus"), None);
    }

    #[test]
    fn count_digits() {
        let mut e = Editor::new(Song::new_empty());
        e.push_count_digit(1);
        e.push_count_digit(2);
        assert_eq!(e.count, Some(12));
        assert_eq!(e.take_count(), Some(12));
        assert_eq!(e.count, None);
        for _ in 0..10 {
            e.push_count_digit(9);
        }
        assert_eq!(e.count, Some(9999));
    }

    #[test]
    fn errors_leave_state_unchanged() {
        let mut e = Editor::new(Song::new_empty());
        let before = e.clone();
        assert_eq!(e.execute(&Command::DeleteSelected, None), Err(EditError::NothingSelected));
        assert_eq!(e.execute(&Command::Paste, None), Err(EditError::ClipboardEmpty));
        assert_eq!(e.execute(&Command::Undo, None), Err(EditError::NothingToUndo));
        assert_eq!(e.execute(&Command::Redo, None), Err(EditError::NothingToRedo));
        assert_eq!(e.execute(&Command::ToggleSelectUnderCursor, None), Err(EditError::NoNoteUnderCursor));
        assert_eq!(e.execute(&Command::RepeatLastEdit, None), Err(EditError::NothingToRepeat));
        assert_eq!(e.execute(&Command::DropFocused, None), Err(EditError::NothingFocused));
        assert_eq!(e.execute(&Command::SelectionFocusNext, None), Err(EditError::NothingSelected));
        assert_eq!(e.execute(&Command::KeepOnBeat, None), Err(EditError::NothingSelected));
        assert!(matches!(e.execute(&Command::SelectBar, None), Err(EditError::NoMatch(_))));
        assert!(matches!(e.execute(&Command::SelectChord, None), Err(EditError::NoMatch(_))));
        assert!(matches!(e.execute(&Command::SelectSamePitch, None), Err(EditError::NoMatch(_))));
        assert_eq!(
            e.execute(&Command::MoveCursor { dir: Dir::Left, unit: Unit::Octave }, None),
            Err(EditError::UnitDirectionMismatch { dir: Dir::Left, unit: Unit::Octave })
        );
        assert_eq!(
            e.execute(&Command::MoveCursor { dir: Dir::Up, unit: Unit::Bar }, None),
            Err(EditError::UnitDirectionMismatch { dir: Dir::Up, unit: Unit::Bar })
        );
        assert_eq!(e.execute(&Command::RemoveTrack, None), Err(EditError::LastTrack));
        assert_eq!(e.execute(&Command::SetLaneValue { value: 1 }, None), Err(EditError::NoLaneShown));
        assert_eq!(e.execute(&Command::PedalSelection, None), Err(EditError::NothingSelected));
        assert!(matches!(e.execute(&Command::MoveTrackUp, None), Err(EditError::OutOfRange(_))));
        assert!(matches!(e.execute(&Command::MoveTrackDown, Some(4)), Err(EditError::OutOfRange(_))));
        assert!(matches!(e.execute(&Command::SetTrackChannel { channel: 16 }, None), Err(EditError::BadValue(_))));
        assert!(matches!(e.execute(&Command::RenameTrack { name: String::new() }, None), Err(EditError::BadValue(_))));
        assert_eq!(e.track_index, before.track_index);
        assert_eq!(e.lane, None);
        assert_eq!(e.song, before.song);
        assert_eq!(e.cursor, before.cursor);
        assert_eq!(e.selection, before.selection);
        assert_eq!(e.undo, before.undo);
        assert!(e.last_edit.is_none());
    }

    #[test]
    fn no_tracks_is_an_error() {
        let mut song = Song::new_empty();
        song.tracks.clear();
        let mut e = Editor::new(song);
        assert_eq!(e.execute(&Command::InsertNote, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::InsertChord { intervals: vec![0] }, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::SelectAll, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::SelectBar, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::SelectToEnd, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::InvertSelection, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::NextTrack, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::DeleteSelected, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::Quantize { grid: None, strength: 100, swing: 0 }, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::RemoveTrack, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::DuplicateTrack, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::MoveTrackUp, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::ToggleMute, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::NudgeTrackVolume { delta: 5 }, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::NextPreset, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::SustainDown, None), Err(EditError::NoTracks));
        e.lane = Some(LaneKind::Sustain);
        assert_eq!(e.execute(&Command::RampLane { from: 0, to: 1 }, None), Err(EditError::NoTracks));
        assert_eq!(e.execute(&Command::ClearLane, None), Err(EditError::NoTracks));
        // adding a track is the way out
        e.execute(&Command::AddTrack, None).unwrap();
        assert_eq!(e.song.tracks.len(), 1);
        assert_eq!(e.song.tracks[0].name, "Track 1");
        assert_eq!(e.track_index, 0);
    }

    #[test]
    fn error_display() {
        assert_eq!(EditError::NothingSelected.to_string(), "nothing selected and no note under the cursor");
        assert_eq!(EditError::OutOfRange("x".into()).to_string(), "x");
        assert_eq!(EditError::NoMatch("no notes in bar 2".into()).to_string(), "no notes in bar 2");
        assert_eq!(EditError::NothingToRepeat.to_string(), "nothing to repeat");
        assert_eq!(EditError::LastTrack.to_string(), "cannot remove the last track");
        assert_eq!(EditError::NoLaneShown.to_string(), "no lane shown: cycle_lane or :lane sustain first");
        assert_eq!(EditError::BadValue("channel 17 is not 1-16".into()).to_string(), "channel 17 is not 1-16");
        assert_eq!(lane_status(None), "lane hidden");
        assert_eq!(lane_status(Some(LaneKind::ModWheel)), "lane: mod wheel");
        assert_eq!(instrument_label(&Instrument::default()), "sf2 default preset 0 bank 0");
    }
}
