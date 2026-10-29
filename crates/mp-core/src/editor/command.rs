//! The [`Command`] enum: every editing action the user can trigger, as data.
//!
//! The keymap (TOML) and the command line both parse into this one enum, and
//! [`crate::Editor::execute`] matches on it exhaustively, so adding a variant forces
//! every consumer to handle it. [`Command::describe`] and [`Command::category`] feed the
//! cheat sheet so bindings and documentation cannot drift apart, and
//! [`Command::examples`] (one canonical value per variant) feeds the command-line
//! parser's name list and the tests that check every variant is covered.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::song::LaneKind;
use crate::time::Grid;

/// A direction on the piano roll: left/right in time, up/down in pitch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    /// `true` for `Left`/`Right`.
    pub fn is_horizontal(self) -> bool {
        matches!(self, Dir::Left | Dir::Right)
    }
}

impl fmt::Display for Dir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Dir::Left => "left",
            Dir::Right => "right",
            Dir::Up => "up",
            Dir::Down => "down",
        })
    }
}

/// How far a cursor or note move goes. `Grid`/`Beat`/`Bar` are time units (used with
/// `Left`/`Right`); `Semitone`/`Octave` are pitch units (used with `Up`/`Down`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Grid,
    Beat,
    Bar,
    Semitone,
    Octave,
}

impl Unit {
    /// `true` for the time units.
    pub fn is_time(self) -> bool {
        matches!(self, Unit::Grid | Unit::Beat | Unit::Bar)
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Unit::Grid => "grid step",
            Unit::Beat => "beat",
            Unit::Bar => "bar",
            Unit::Semitone => "semitone",
            Unit::Octave => "octave",
        })
    }
}

/// Which way to cycle through a list (grids, tracks, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleDir {
    Next,
    Prev,
}

/// Cheat-sheet grouping for commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Cursor,
    Selection,
    Edit,
    Clipboard,
    Transport,
    View,
    /// Tracks and the mixer: add/remove/reorder, volume, pan, mute, solo, instrument.
    Track,
    /// Controller lanes: sustain, pitch bend, mod wheel, volume and pan automation.
    Lane,
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Category::Cursor => "Cursor",
            Category::Selection => "Selection",
            Category::Edit => "Edit",
            Category::Clipboard => "Clipboard",
            Category::Transport => "Transport",
            Category::View => "View",
            Category::Track => "Track",
            Category::Lane => "Lane",
        })
    }
}

/// Default `strength` of [`Command::Quantize`]: snap all the way.
pub const DEFAULT_QUANTIZE_STRENGTH: u8 = 100;
/// Default `n` of [`Command::SelectNextNotes`].
pub const DEFAULT_SELECT_NEXT: u32 = 4;
/// Default `n` of [`Command::KeepEveryOther`].
pub const DEFAULT_KEEP_EVERY: u32 = 2;

fn default_quantize_strength() -> u8 {
    DEFAULT_QUANTIZE_STRENGTH
}

fn default_select_next() -> u32 {
    DEFAULT_SELECT_NEXT
}

fn default_keep_every() -> u32 {
    DEFAULT_KEEP_EVERY
}

/// One editing action. Spelled in TOML as `{ cmd = "move_cursor", dir = "right", unit = "grid" }`
/// or, for variants without fields, just `"insert_note"`.
///
/// Each variant documents what a count prefix (`3` + key) does to it. "Needs notes"
/// means the command acts on the selection, or on the note under the cursor if nothing
/// is selected, and fails with [`crate::EditError::NothingSelected`] if there is neither.
/// The `Select*` commands replace the selection; the `Keep*` filters thin it and fail
/// with [`crate::EditError::NoMatch`] rather than emptying it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    // ------------------------------------------------------------------ cursor
    /// Move the cursor one `unit` in `dir` and clear the selection. Pitch clamps to
    /// 0..=127, time to ≥ 0. Count: repeat.
    MoveCursor { dir: Dir, unit: Unit },
    /// Move the cursor like `move_cursor` while growing a selection box from where the
    /// cursor was before the first `extend_selection`; the notes starting inside the
    /// box become the selection. Any other command ends the box and keeps the
    /// selection. Count: repeat.
    ExtendSelection { dir: Dir, unit: Unit },
    /// Move the cursor to the start of the song, keeping its pitch, and clear the
    /// selection. Count: ignored.
    CursorToStart,

    // ------------------------------------------------------------------ edit
    /// Insert a note at the cursor with the current grid length and velocity 100,
    /// replacing any note with the same start and pitch. The new note becomes the
    /// selection; the cursor stays. Count: note length in grid steps.
    InsertNote,
    /// Insert a chord at the cursor: one note per interval (semitones above the cursor
    /// pitch, e.g. `[0, 4, 7]` for a major triad), each with the grid length and
    /// velocity 100. Intervals that leave 0..=127 are skipped. The chord becomes the
    /// selection. Count: note length in grid steps.
    InsertChord { intervals: Vec<i8> },
    /// Delete the selected notes, or the note under the cursor. Needs notes. Count: ignored.
    DeleteSelected,
    /// Shift the selected notes (or the note under the cursor) by one `unit` in `dir`.
    /// The whole group is clamped so no note leaves 0..=127 or goes before tick 0; the
    /// cursor moves with the notes. Needs notes. Count: repeat.
    MoveSelected { dir: Dir, unit: Unit },
    /// Move the selected notes by `ticks` (negative = earlier), off the grid. The group
    /// is clamped at tick 0; the cursor stays where it is. Needs notes. Count: multiplier.
    NudgeSelected { ticks: i64 },
    /// Lengthen (positive) or shorten (negative) the selected notes by `delta_steps` grid
    /// steps, never below a quarter of a grid step. Needs notes. Count: multiplier.
    ChangeLength { delta_steps: i32 },
    /// Set the length of the selected notes to `steps` grid steps (at least 1).
    /// Needs notes. Count: overrides `steps`.
    SetLength { steps: u32 },
    /// Extend each selected note up to the start of the next note on the track (any
    /// pitch); notes with nothing after them are left alone. Needs notes. Count: ignored.
    Legato,
    /// Add `delta` to the velocity of the selected notes, clamped to 1..=127.
    /// Needs notes. Count: multiplier.
    NudgeVelocity { delta: i32 },
    /// Set the velocity of the selected notes to `value` (clamped to 1..=127).
    /// Needs notes. Count: ignored.
    SetVelocity { value: u8 },
    /// Velocity ramp over the selection in time order: the earliest note(s) get `from`,
    /// the latest `to`, linear in between; notes sharing a start tick share a value.
    /// Needs notes. Count: ignored.
    RampVelocity { from: u8, to: u8 },
    /// Randomise the selected notes: velocity by up to ±`velocity`, start by up to
    /// ±`timing` ticks (never before tick 0). Deterministic for a given editor seed.
    /// Needs notes. Count: ignored.
    Humanize { velocity: u8, timing: u32 },
    /// Snap note starts to `grid` (the current grid if omitted). `strength` is the
    /// percentage of the way each note moves towards its grid point (100 = all the way).
    /// `swing` is a percentage delaying every second grid step; `0` means "use the
    /// editor's swing set by `set_swing`". Needs notes. Count: ignored.
    Quantize {
        #[serde(default)]
        grid: Option<Grid>,
        #[serde(default = "default_quantize_strength")]
        strength: u8,
        #[serde(default)]
        swing: u8,
    },
    /// Round note lengths to the nearest whole number of grid steps (at least one).
    /// Needs notes. Count: ignored.
    QuantizeLength,
    /// Set the editor's swing (percent, clamped to 100) used by `quantize` when its own
    /// swing is 0. Not an edit. Count: ignored.
    SetSwing { percent: u8 },
    /// Transpose the selected notes by `semitones`, clamped as a group to 0..=127.
    /// Needs notes. Count: multiplier.
    TransposeSelected { semitones: i32 },
    /// Run the last song-changing command again at the current cursor/selection, with
    /// the count it had. Count: overrides that count. Not itself remembered.
    RepeatLastEdit,
    /// Undo the last edit (restores cursor and selection too). Count: repeat.
    Undo,
    /// Redo the last undone edit. Count: repeat.
    Redo,

    // ------------------------------------------------------------------ selection
    /// Add or remove the note under the cursor from the selection (and audition it).
    /// Count: ignored.
    ToggleSelectUnderCursor,
    /// Select every note on the current track. Count: ignored.
    SelectAll,
    /// Deselect everything. Count: ignored.
    ClearSelection,
    /// Select the notes starting in the cursor's bar. Count: that many bars from the
    /// cursor's bar.
    SelectBar,
    /// Select every note starting at the same tick as the note under the cursor (or at
    /// the cursor tick if there is none). Count: ignored.
    SelectChord,
    /// Select the next `n` notes starting at or after the cursor, in time order.
    /// Count: overrides `n`.
    SelectNextNotes {
        #[serde(default = "default_select_next")]
        n: u32,
    },
    /// Select every note starting at or after the cursor. Count: ignored.
    SelectToEnd,
    /// Select every note starting at or before the cursor. Count: ignored.
    SelectToStart,
    /// Select every note with the cursor's pitch. Count: ignored.
    SelectSamePitch,
    /// Select every note with velocity greater than `value`. Count: ignored.
    SelectVelocityAbove { value: u8 },
    /// Select every note with velocity less than `value`. Count: ignored.
    SelectVelocityBelow { value: u8 },
    /// Select the highest note of every chord (two or more notes sharing a start tick).
    /// Count: ignored.
    SelectTopOfChords,
    /// Select the lowest note of every chord (two or more notes sharing a start tick).
    /// Count: ignored.
    SelectBottomOfChords,
    /// Select the unselected notes and deselect the selected ones. Count: ignored.
    InvertSelection,
    /// Focus the next selected note in time order (the first if none is focused) and
    /// move the cursor onto it. Count: repeat.
    SelectionFocusNext,
    /// Focus the previous selected note in time order (the last if none is focused)
    /// and move the cursor onto it. Count: repeat.
    SelectionFocusPrev,
    /// Remove the focused note from the selection and focus the next one.
    /// Count: ignored.
    DropFocused,
    /// Keep only the highest selected note at each start tick. Needs notes. Count: ignored.
    KeepTopOfChords,
    /// Keep only the lowest selected note at each start tick. Needs notes. Count: ignored.
    KeepBottomOfChords,
    /// Keep every `n`th selected note in time order, starting with the first.
    /// Needs notes. Count: overrides `n`.
    KeepEveryOther {
        #[serde(default = "default_keep_every")]
        n: u32,
    },
    /// Keep only the selected notes that start exactly on a beat. Needs notes. Count: ignored.
    KeepOnBeat,
    /// Keep only the selected notes that do not start on a beat. Needs notes. Count: ignored.
    KeepOffBeat,
    /// Keep only the selected notes above the cursor pitch. Needs notes. Count: ignored.
    KeepAboveCursor,
    /// Keep only the selected notes below the cursor pitch. Needs notes. Count: ignored.
    KeepBelowCursor,
    /// Keep only the selected notes starting at or after the cursor. Needs notes.
    /// Count: ignored.
    KeepAfterCursor,
    /// Keep only the selected notes starting before the cursor. Needs notes. Count: ignored.
    KeepBeforeCursor,

    // ------------------------------------------------------------------ clipboard
    /// Copy the selected notes (or the note under the cursor) to the clipboard.
    /// Needs notes. Count: ignored.
    Copy,
    /// Copy the selected notes (or the note under the cursor) to the clipboard and
    /// delete them, as one undo step. Needs notes. Count: ignored.
    Cut,
    /// Paste the clipboard at the cursor tick with the original pitches. The pasted notes
    /// become the selection and the cursor jumps to their end. Count: paste that many
    /// copies back to back.
    Paste,
    /// Copy the selected notes and paste them right after the selection's end.
    /// Needs notes. Count: that many copies back to back.
    Duplicate,

    // ------------------------------------------------------------------ view
    /// Set the grid to an exact value, e.g. `grid = "1/8T"`. Count: ignored.
    SetGrid { grid: Grid },
    /// Cycle the grid division (1/4 → 1/8 → 1/16 ...), keeping dotted/triplet.
    /// Count: repeat.
    CycleGrid { dir: CycleDir },
    /// Show the next track (wraps). Clears the selection. Count: repeat.
    NextTrack,
    /// Show the previous track (wraps). Clears the selection. Count: repeat.
    PrevTrack,
    /// Ask the app to zoom in on time (emits a view request). Count: repeat.
    ZoomIn,
    /// Ask the app to zoom out on time. Count: repeat.
    ZoomOut,
    /// Ask the app to make piano-roll rows taller. Count: repeat.
    ZoomRowsIn,
    /// Ask the app to make piano-roll rows shorter. Count: repeat.
    ZoomRowsOut,
    /// Ask the app to show or hide the piano roll under the tracks area. Count: ignored.
    TogglePianoRoll,
    /// Ask the app to fit the whole song in the tracks area. Count: ignored.
    ZoomToFit,
    /// Ask the app to show or hide the cheat sheet. Count: ignored.
    ToggleCheatSheet,
    /// Open the instrument picker for the current track. Count: ignored.
    OpenInstrumentPicker,

    // ------------------------------------------------------------------ track
    /// Add an empty track named "Track N" right after the current one and show it.
    /// Count: that many tracks (the last one is shown).
    AddTrack,
    /// Remove the current track and show the one that takes its place (or the new
    /// last one). Fails with [`crate::EditError::LastTrack`] on the only track.
    /// Count: ignored.
    RemoveTrack,
    /// Insert a copy of the current track (notes with fresh ids, same mixer state and
    /// lanes) named `<name> copy` right after it, and show the copy. Count: that many
    /// copies (the last one is shown).
    DuplicateTrack,
    /// Swap the current track with the one above it; the view follows the track.
    /// Fails if it is already first. Count: that many places (stops at the top).
    MoveTrackUp,
    /// Swap the current track with the one below it; the view follows the track.
    /// Fails if it is already last. Count: that many places (stops at the bottom).
    MoveTrackDown,
    /// Rename the current track. On the command line the rest of the line is the name
    /// (`rename My Bass`). Fails with [`crate::EditError::BadValue`] if it is empty.
    /// Count: ignored.
    RenameTrack { name: String },
    /// Set the current track's volume in percent of unity gain, 0..=200 (clamped).
    /// Count: overrides `percent`.
    SetTrackVolume { percent: u8 },
    /// Add `delta` percent to the current track's volume, clamped to 0..=200.
    /// Count: multiplier.
    NudgeTrackVolume { delta: i8 },
    /// Set the current track's pan: -100 (hard left) ..= 100 (hard right), 0 centre
    /// (clamped). Count: ignored.
    SetTrackPan { value: i8 },
    /// Add `delta` to the current track's pan, clamped to -100..=100. Count: multiplier.
    NudgeTrackPan { delta: i8 },
    /// Mute or unmute the current track. Count: ignored.
    ToggleMute,
    /// Solo or unsolo the current track. Count: ignored.
    ToggleSolo,
    /// Set the current track's MIDI channel, 0..=15 here (channel 9 is the GM drum
    /// kit). The command line and status bar count from 1: `channel 10` sets 9.
    /// Fails with [`crate::EditError::BadValue`] above 15. Count: ignored.
    SetTrackChannel { channel: u8 },
    /// Set the current track's SoundFont and program. `path = None` means the app's
    /// default SoundFont (the command line spells it `sf2 default`); `preset` must be
    /// 0..=127. Count: ignored.
    SetInstrument {
        #[serde(default)]
        path: Option<PathBuf>,
        #[serde(default)]
        bank: u16,
        #[serde(default)]
        preset: u16,
    },
    /// Select the next SoundFont preset on the current track, wrapping 127 → 0.
    /// Count: that many presets.
    NextPreset,
    /// Select the previous SoundFont preset on the current track, wrapping 0 → 127.
    /// Count: that many presets.
    PrevPreset,
    /// Play General MIDI program `preset` (0..=127) on the current track, keeping its
    /// SoundFont. A drum track moves to a free melodic channel. Count: ignored.
    SetPreset { preset: u16 },
    /// Turn the current track into a drum track on MIDI channel 10 playing kit
    /// program `kit` (0 = the standard kit); a track still named `Track N` becomes
    /// `Drums`. Count: ignored.
    SetDrumKit {
        #[serde(default)]
        kit: u16,
    },
    /// Set the master reverb amount in percent, 0..=100 (clamped). Count: overrides
    /// `percent`.
    SetMasterReverb { percent: u8 },

    // ------------------------------------------------------------------ lane
    /// Show one controller lane strip under the piano roll (`kind = "sustain"`), or
    /// hide it when `kind` is omitted. The lane commands below act on the shown lane.
    /// Not an edit. Count: ignored.
    ShowLane {
        #[serde(default)]
        kind: Option<LaneKind>,
    },
    /// Show the next lane: hidden → sustain → pitch bend → mod wheel → volume → pan →
    /// hidden. Not an edit. Count: repeat.
    CycleLane,
    /// Put a point with `value` at the cursor tick in the shown lane (clamped to the
    /// lane's range). Fails with [`crate::EditError::NoLaneShown`] if no lane is shown.
    /// Count: overrides `value`.
    SetLaneValue { value: i32 },
    /// Add `delta` to the lane value at the cursor tick, creating a point from the
    /// value in force there if none exists. In the pitch-bend lane `delta` is scaled by
    /// 64 (its range is 128× wider), so ±8 nudges a bend by ±512. Fails if no lane is
    /// shown or the value is already at the limit. Count: multiplier.
    NudgeLaneValue { delta: i32 },
    /// Draw a straight line of points, one per grid step, from `from` to `to` across
    /// the selection's time range (first start to last end), else from the cursor to
    /// the end of the note under it, else one bar from the cursor. Points already in
    /// that range are replaced. Fails if no lane is shown. Count: ignored.
    RampLane { from: i32, to: i32 },
    /// Remove the shown lane's points in the selection's time range, or every point of
    /// the lane if nothing is selected. Fails if no lane is shown. Count: ignored.
    ClearLane,
    /// Put a sustain-pedal-down point (127) at the cursor tick, whatever lane is shown.
    /// Count: ignored.
    SustainDown,
    /// Put a sustain-pedal-up point (0) at the cursor tick, whatever lane is shown.
    /// Count: ignored.
    SustainUp,
    /// Hold the pedal under the selection: sustain down at its first start, up at its
    /// last end, any sustain points in between removed. Needs notes. Count: ignored.
    PedalSelection,

    // ------------------------------------------------------------------ transport
    /// Ask the app to play from the start of the song. Count: ignored.
    Play,
    /// Ask the app to stop playback. Count: ignored.
    Stop,
    /// Ask the app to stop if playing, else play from the cursor. Count: ignored.
    TogglePlay,
    /// Ask the app to play from the cursor. Count: ignored.
    PlayFromCursor,

    // ------------------------------------------------------------------ mode
    /// Enter Command-line mode (the app then owns the text field). Count: ignored.
    EnterCommandLine,
}

impl Command {
    /// The snake_case name used in TOML (`"move_cursor"`), without arguments.
    pub fn name(&self) -> &'static str {
        match self {
            Command::MoveCursor { .. } => "move_cursor",
            Command::ExtendSelection { .. } => "extend_selection",
            Command::CursorToStart => "cursor_to_start",
            Command::InsertNote => "insert_note",
            Command::InsertChord { .. } => "insert_chord",
            Command::DeleteSelected => "delete_selected",
            Command::MoveSelected { .. } => "move_selected",
            Command::NudgeSelected { .. } => "nudge_selected",
            Command::ChangeLength { .. } => "change_length",
            Command::SetLength { .. } => "set_length",
            Command::Legato => "legato",
            Command::NudgeVelocity { .. } => "nudge_velocity",
            Command::SetVelocity { .. } => "set_velocity",
            Command::RampVelocity { .. } => "ramp_velocity",
            Command::Humanize { .. } => "humanize",
            Command::Quantize { .. } => "quantize",
            Command::QuantizeLength => "quantize_length",
            Command::SetSwing { .. } => "set_swing",
            Command::TransposeSelected { .. } => "transpose_selected",
            Command::RepeatLastEdit => "repeat_last_edit",
            Command::Undo => "undo",
            Command::Redo => "redo",
            Command::ToggleSelectUnderCursor => "toggle_select_under_cursor",
            Command::SelectAll => "select_all",
            Command::ClearSelection => "clear_selection",
            Command::SelectBar => "select_bar",
            Command::SelectChord => "select_chord",
            Command::SelectNextNotes { .. } => "select_next_notes",
            Command::SelectToEnd => "select_to_end",
            Command::SelectToStart => "select_to_start",
            Command::SelectSamePitch => "select_same_pitch",
            Command::SelectVelocityAbove { .. } => "select_velocity_above",
            Command::SelectVelocityBelow { .. } => "select_velocity_below",
            Command::SelectTopOfChords => "select_top_of_chords",
            Command::SelectBottomOfChords => "select_bottom_of_chords",
            Command::InvertSelection => "invert_selection",
            Command::SelectionFocusNext => "selection_focus_next",
            Command::SelectionFocusPrev => "selection_focus_prev",
            Command::DropFocused => "drop_focused",
            Command::KeepTopOfChords => "keep_top_of_chords",
            Command::KeepBottomOfChords => "keep_bottom_of_chords",
            Command::KeepEveryOther { .. } => "keep_every_other",
            Command::KeepOnBeat => "keep_on_beat",
            Command::KeepOffBeat => "keep_off_beat",
            Command::KeepAboveCursor => "keep_above_cursor",
            Command::KeepBelowCursor => "keep_below_cursor",
            Command::KeepAfterCursor => "keep_after_cursor",
            Command::KeepBeforeCursor => "keep_before_cursor",
            Command::Copy => "copy",
            Command::Cut => "cut",
            Command::Paste => "paste",
            Command::Duplicate => "duplicate",
            Command::SetGrid { .. } => "set_grid",
            Command::CycleGrid { .. } => "cycle_grid",
            Command::NextTrack => "next_track",
            Command::PrevTrack => "prev_track",
            Command::ZoomIn => "zoom_in",
            Command::ZoomOut => "zoom_out",
            Command::ZoomRowsIn => "zoom_rows_in",
            Command::ZoomRowsOut => "zoom_rows_out",
            Command::TogglePianoRoll => "toggle_piano_roll",
            Command::ZoomToFit => "zoom_to_fit",
            Command::ToggleCheatSheet => "toggle_cheat_sheet",
            Command::OpenInstrumentPicker => "open_instrument_picker",
            Command::AddTrack => "add_track",
            Command::RemoveTrack => "remove_track",
            Command::DuplicateTrack => "duplicate_track",
            Command::MoveTrackUp => "move_track_up",
            Command::MoveTrackDown => "move_track_down",
            Command::RenameTrack { .. } => "rename_track",
            Command::SetTrackVolume { .. } => "set_track_volume",
            Command::NudgeTrackVolume { .. } => "nudge_track_volume",
            Command::SetTrackPan { .. } => "set_track_pan",
            Command::NudgeTrackPan { .. } => "nudge_track_pan",
            Command::ToggleMute => "toggle_mute",
            Command::ToggleSolo => "toggle_solo",
            Command::SetTrackChannel { .. } => "set_track_channel",
            Command::SetInstrument { .. } => "set_instrument",
            Command::NextPreset => "next_preset",
            Command::PrevPreset => "prev_preset",
            Command::SetPreset { .. } => "set_preset",
            Command::SetDrumKit { .. } => "set_drum_kit",
            Command::SetMasterReverb { .. } => "set_master_reverb",
            Command::ShowLane { .. } => "show_lane",
            Command::CycleLane => "cycle_lane",
            Command::SetLaneValue { .. } => "set_lane_value",
            Command::NudgeLaneValue { .. } => "nudge_lane_value",
            Command::RampLane { .. } => "ramp_lane",
            Command::ClearLane => "clear_lane",
            Command::SustainDown => "sustain_down",
            Command::SustainUp => "sustain_up",
            Command::PedalSelection => "pedal_selection",
            Command::Play => "play",
            Command::Stop => "stop",
            Command::TogglePlay => "toggle_play",
            Command::PlayFromCursor => "play_from_cursor",
            Command::EnterCommandLine => "enter_command_line",
        }
    }

    /// A short human-readable description for the cheat sheet and status bar.
    pub fn describe(&self) -> String {
        match self {
            Command::MoveCursor { dir, unit } => format!("Move cursor {dir} one {unit}"),
            Command::ExtendSelection { dir, unit } => format!("Grow the selection box {dir} one {unit}"),
            Command::CursorToStart => "Move the cursor to the start".to_string(),
            Command::InsertNote => "Insert a note at the cursor".to_string(),
            Command::InsertChord { intervals } => {
                let list: Vec<String> = intervals.iter().map(ToString::to_string).collect();
                format!("Insert a chord [{}] at the cursor", list.join(","))
            }
            Command::DeleteSelected => "Delete selected notes (or the note under the cursor)".to_string(),
            Command::MoveSelected { dir, unit } => format!("Move selected notes {dir} one {unit}"),
            Command::NudgeSelected { ticks } => format!("Nudge selected notes {ticks:+} ticks"),
            Command::ChangeLength { delta_steps } => {
                if *delta_steps >= 0 {
                    format!("Lengthen selected notes by {delta_steps} grid step(s)")
                } else {
                    format!("Shorten selected notes by {} grid step(s)", -delta_steps)
                }
            }
            Command::SetLength { steps } => format!("Set note length to {steps} grid step(s)"),
            Command::Legato => "Extend selected notes to the next note (legato)".to_string(),
            Command::NudgeVelocity { delta } => format!("Velocity {delta:+}"),
            Command::SetVelocity { value } => format!("Set velocity to {value}"),
            Command::RampVelocity { from, to } => format!("Ramp velocity {from} → {to}"),
            Command::Humanize { velocity, timing } => {
                format!("Humanize (velocity ±{velocity}, timing ±{timing} ticks)")
            }
            Command::Quantize { grid, strength, swing } => {
                let mut s = match grid {
                    Some(g) => format!("Quantize to {g}"),
                    None => "Quantize to the current grid".to_string(),
                };
                if *strength != DEFAULT_QUANTIZE_STRENGTH {
                    s.push_str(&format!(" at {strength}%"));
                }
                if *swing != 0 {
                    s.push_str(&format!(" with {swing}% swing"));
                }
                s
            }
            Command::QuantizeLength => "Round note lengths to the grid".to_string(),
            Command::SetSwing { percent } => format!("Set swing to {percent}%"),
            Command::TransposeSelected { semitones } => format!("Transpose {semitones:+} semitone(s)"),
            Command::RepeatLastEdit => "Repeat the last edit".to_string(),
            Command::Undo => "Undo".to_string(),
            Command::Redo => "Redo".to_string(),
            Command::ToggleSelectUnderCursor => "Toggle selection of the note under the cursor".to_string(),
            Command::SelectAll => "Select all notes on this track".to_string(),
            Command::ClearSelection => "Clear the selection".to_string(),
            Command::SelectBar => "Select the notes in this bar".to_string(),
            Command::SelectChord => "Select the chord at the cursor".to_string(),
            Command::SelectNextNotes { n } => format!("Select the next {n} notes"),
            Command::SelectToEnd => "Select from the cursor to the end".to_string(),
            Command::SelectToStart => "Select from the start to the cursor".to_string(),
            Command::SelectSamePitch => "Select every note with the cursor's pitch".to_string(),
            Command::SelectVelocityAbove { value } => format!("Select notes with velocity above {value}"),
            Command::SelectVelocityBelow { value } => format!("Select notes with velocity below {value}"),
            Command::SelectTopOfChords => "Select the top note of every chord".to_string(),
            Command::SelectBottomOfChords => "Select the bottom note of every chord".to_string(),
            Command::InvertSelection => "Invert the selection".to_string(),
            Command::SelectionFocusNext => "Focus the next selected note".to_string(),
            Command::SelectionFocusPrev => "Focus the previous selected note".to_string(),
            Command::DropFocused => "Drop the focused note from the selection".to_string(),
            Command::KeepTopOfChords => "Keep only the top note of each chord".to_string(),
            Command::KeepBottomOfChords => "Keep only the bottom note of each chord".to_string(),
            Command::KeepEveryOther { n } => match n {
                2 => "Keep every other note".to_string(),
                _ => format!("Keep every {n}th note"),
            },
            Command::KeepOnBeat => "Keep only notes on a beat".to_string(),
            Command::KeepOffBeat => "Keep only notes off the beat".to_string(),
            Command::KeepAboveCursor => "Keep only notes above the cursor".to_string(),
            Command::KeepBelowCursor => "Keep only notes below the cursor".to_string(),
            Command::KeepAfterCursor => "Keep only notes at or after the cursor".to_string(),
            Command::KeepBeforeCursor => "Keep only notes before the cursor".to_string(),
            Command::Copy => "Copy selected notes".to_string(),
            Command::Cut => "Cut selected notes".to_string(),
            Command::Paste => "Paste at the cursor".to_string(),
            Command::Duplicate => "Duplicate selected notes after themselves".to_string(),
            Command::SetGrid { grid } => format!("Set grid to {grid}"),
            Command::CycleGrid { dir: CycleDir::Next } => "Finer grid".to_string(),
            Command::CycleGrid { dir: CycleDir::Prev } => "Coarser grid".to_string(),
            Command::NextTrack => "Next track".to_string(),
            Command::PrevTrack => "Previous track".to_string(),
            Command::ZoomIn => "Zoom in".to_string(),
            Command::ZoomOut => "Zoom out".to_string(),
            Command::ZoomRowsIn => "Taller rows".to_string(),
            Command::ZoomRowsOut => "Shorter rows".to_string(),
            Command::TogglePianoRoll => "Show or hide the piano roll".to_string(),
            Command::ZoomToFit => "Fit the whole song in the tracks area".to_string(),
            Command::ToggleCheatSheet => "Show or hide the cheat sheet".to_string(),
            Command::OpenInstrumentPicker => "Choose this track's instrument".to_string(),
            Command::AddTrack => "Add a track".to_string(),
            Command::RemoveTrack => "Remove this track".to_string(),
            Command::DuplicateTrack => "Duplicate this track".to_string(),
            Command::MoveTrackUp => "Move this track up".to_string(),
            Command::MoveTrackDown => "Move this track down".to_string(),
            Command::RenameTrack { name } => format!("Rename this track to {name:?}"),
            Command::SetTrackVolume { percent } => format!("Set track volume to {percent}%"),
            Command::NudgeTrackVolume { delta } => format!("Track volume {delta:+}%"),
            Command::SetTrackPan { value } => format!("Set track pan to {}", pan_label(i32::from(*value))),
            Command::NudgeTrackPan { delta } => {
                if *delta >= 0 {
                    format!("Pan {delta} to the right")
                } else {
                    format!("Pan {} to the left", -i32::from(*delta))
                }
            }
            Command::ToggleMute => "Mute or unmute this track".to_string(),
            Command::ToggleSolo => "Solo or unsolo this track".to_string(),
            Command::SetTrackChannel { channel } => format!("Set MIDI channel to {}", u32::from(*channel) + 1),
            Command::SetInstrument { path, bank, preset } => match path {
                Some(p) => format!("Use SoundFont {} preset {preset} bank {bank}", p.display()),
                None => format!("Use the default SoundFont, preset {preset} bank {bank}"),
            },
            Command::NextPreset => "Next preset".to_string(),
            Command::PrevPreset => "Previous preset".to_string(),
            Command::SetPreset { preset } => format!("Play {}", crate::gm::program_name(*preset)),
            Command::SetDrumKit { kit } => match crate::gm::drum_kit_name(*kit) {
                Some(name) => format!("Drums: {name} kit"),
                None => format!("Drums: kit {kit}"),
            },
            Command::SetMasterReverb { percent } => format!("Set master reverb to {percent}%"),
            Command::ShowLane { kind: Some(kind) } => format!("Show the {kind} lane"),
            Command::ShowLane { kind: None } => "Hide the lane".to_string(),
            Command::CycleLane => "Show the next lane".to_string(),
            Command::SetLaneValue { value } => format!("Set the lane value at the cursor to {value}"),
            Command::NudgeLaneValue { delta } => format!("Lane value {delta:+}"),
            Command::RampLane { from, to } => format!("Ramp the lane {from} → {to}"),
            Command::ClearLane => "Clear the lane (selection range or all)".to_string(),
            Command::SustainDown => "Sustain pedal down at the cursor".to_string(),
            Command::SustainUp => "Sustain pedal up at the cursor".to_string(),
            Command::PedalSelection => "Hold the sustain pedal under the selection".to_string(),
            Command::Play => "Play from the start".to_string(),
            Command::Stop => "Stop".to_string(),
            Command::TogglePlay => "Play from the cursor / stop".to_string(),
            Command::PlayFromCursor => "Play from the cursor".to_string(),
            Command::EnterCommandLine => "Open the command line".to_string(),
        }
    }

    /// Cheat-sheet group.
    pub fn category(&self) -> Category {
        match self {
            Command::MoveCursor { .. } => Category::Cursor,
            Command::ToggleSelectUnderCursor
            | Command::ExtendSelection { .. }
            | Command::SelectAll
            | Command::ClearSelection
            | Command::SelectBar
            | Command::SelectChord
            | Command::SelectNextNotes { .. }
            | Command::SelectToEnd
            | Command::SelectToStart
            | Command::SelectSamePitch
            | Command::SelectVelocityAbove { .. }
            | Command::SelectVelocityBelow { .. }
            | Command::SelectTopOfChords
            | Command::SelectBottomOfChords
            | Command::InvertSelection
            | Command::SelectionFocusNext
            | Command::SelectionFocusPrev
            | Command::DropFocused
            | Command::KeepTopOfChords
            | Command::KeepBottomOfChords
            | Command::KeepEveryOther { .. }
            | Command::KeepOnBeat
            | Command::KeepOffBeat
            | Command::KeepAboveCursor
            | Command::KeepBelowCursor
            | Command::KeepAfterCursor
            | Command::KeepBeforeCursor => Category::Selection,
            Command::InsertNote
            | Command::InsertChord { .. }
            | Command::DeleteSelected
            | Command::MoveSelected { .. }
            | Command::NudgeSelected { .. }
            | Command::ChangeLength { .. }
            | Command::SetLength { .. }
            | Command::Legato
            | Command::NudgeVelocity { .. }
            | Command::SetVelocity { .. }
            | Command::RampVelocity { .. }
            | Command::Humanize { .. }
            | Command::Quantize { .. }
            | Command::QuantizeLength
            | Command::TransposeSelected { .. }
            | Command::RepeatLastEdit
            | Command::Undo
            | Command::Redo => Category::Edit,
            Command::Copy | Command::Cut | Command::Paste | Command::Duplicate => Category::Clipboard,
            Command::SetGrid { .. }
            | Command::CycleGrid { .. }
            | Command::SetSwing { .. }
            | Command::NextTrack
            | Command::PrevTrack
            | Command::ZoomIn
            | Command::ZoomOut
            | Command::ZoomRowsIn
            | Command::ZoomRowsOut
            | Command::TogglePianoRoll
            | Command::ZoomToFit
            | Command::ToggleCheatSheet
            | Command::OpenInstrumentPicker
            | Command::EnterCommandLine => Category::View,
            Command::CursorToStart
            | Command::Play
            | Command::Stop
            | Command::TogglePlay
            | Command::PlayFromCursor => Category::Transport,
            Command::AddTrack
            | Command::RemoveTrack
            | Command::DuplicateTrack
            | Command::MoveTrackUp
            | Command::MoveTrackDown
            | Command::RenameTrack { .. }
            | Command::SetTrackVolume { .. }
            | Command::NudgeTrackVolume { .. }
            | Command::SetTrackPan { .. }
            | Command::NudgeTrackPan { .. }
            | Command::ToggleMute
            | Command::ToggleSolo
            | Command::SetTrackChannel { .. }
            | Command::SetInstrument { .. }
            | Command::NextPreset
            | Command::PrevPreset
            | Command::SetPreset { .. }
            | Command::SetDrumKit { .. }
            | Command::SetMasterReverb { .. } => Category::Track,
            Command::ShowLane { .. }
            | Command::CycleLane
            | Command::SetLaneValue { .. }
            | Command::NudgeLaneValue { .. }
            | Command::RampLane { .. }
            | Command::ClearLane
            | Command::SustainDown
            | Command::SustainUp
            | Command::PedalSelection => Category::Lane,
        }
    }

    /// `true` if executing this command may change the song (and therefore push an
    /// undo snapshot). Undo/Redo count as edits; every `Track` command is one, and so
    /// is every `Lane` command except showing or cycling the lane.
    pub fn is_edit(&self) -> bool {
        match self.category() {
            Category::Edit | Category::Track => true,
            Category::Clipboard => !matches!(self, Command::Copy),
            Category::Lane => !matches!(self, Command::ShowLane { .. } | Command::CycleLane),
            Category::Cursor | Category::Selection | Category::Transport | Category::View => false,
        }
    }

    /// One canonical value of every variant, in declaration order. This is the single
    /// list the command-line parser and the coverage tests are built from: when a
    /// variant is added, the exhaustive matches above force it to be named here too
    /// (see the `examples_cover_every_variant` test).
    pub fn examples() -> Vec<Command> {
        vec![
            Command::MoveCursor { dir: Dir::Right, unit: Unit::Grid },
            Command::ExtendSelection { dir: Dir::Right, unit: Unit::Grid },
            Command::CursorToStart,
            Command::InsertNote,
            Command::InsertChord { intervals: vec![0, 4, 7] },
            Command::DeleteSelected,
            Command::MoveSelected { dir: Dir::Right, unit: Unit::Grid },
            Command::NudgeSelected { ticks: 10 },
            Command::ChangeLength { delta_steps: 1 },
            Command::SetLength { steps: 1 },
            Command::Legato,
            Command::NudgeVelocity { delta: 5 },
            Command::SetVelocity { value: 100 },
            Command::RampVelocity { from: 60, to: 110 },
            Command::Humanize { velocity: 8, timing: 10 },
            Command::Quantize { grid: None, strength: DEFAULT_QUANTIZE_STRENGTH, swing: 0 },
            Command::QuantizeLength,
            Command::SetSwing { percent: 0 },
            Command::TransposeSelected { semitones: 12 },
            Command::RepeatLastEdit,
            Command::Undo,
            Command::Redo,
            Command::ToggleSelectUnderCursor,
            Command::SelectAll,
            Command::ClearSelection,
            Command::SelectBar,
            Command::SelectChord,
            Command::SelectNextNotes { n: DEFAULT_SELECT_NEXT },
            Command::SelectToEnd,
            Command::SelectToStart,
            Command::SelectSamePitch,
            Command::SelectVelocityAbove { value: 100 },
            Command::SelectVelocityBelow { value: 60 },
            Command::SelectTopOfChords,
            Command::SelectBottomOfChords,
            Command::InvertSelection,
            Command::SelectionFocusNext,
            Command::SelectionFocusPrev,
            Command::DropFocused,
            Command::KeepTopOfChords,
            Command::KeepBottomOfChords,
            Command::KeepEveryOther { n: DEFAULT_KEEP_EVERY },
            Command::KeepOnBeat,
            Command::KeepOffBeat,
            Command::KeepAboveCursor,
            Command::KeepBelowCursor,
            Command::KeepAfterCursor,
            Command::KeepBeforeCursor,
            Command::Copy,
            Command::Cut,
            Command::Paste,
            Command::Duplicate,
            Command::SetGrid { grid: Grid::default() },
            Command::CycleGrid { dir: CycleDir::Next },
            Command::NextTrack,
            Command::PrevTrack,
            Command::ZoomIn,
            Command::ZoomOut,
            Command::ZoomRowsIn,
            Command::ZoomRowsOut,
            Command::TogglePianoRoll,
            Command::ZoomToFit,
            Command::ToggleCheatSheet,
            Command::OpenInstrumentPicker,
            Command::AddTrack,
            Command::RemoveTrack,
            Command::DuplicateTrack,
            Command::MoveTrackUp,
            Command::MoveTrackDown,
            Command::RenameTrack { name: "Bass".to_string() },
            Command::SetTrackVolume { percent: 100 },
            Command::NudgeTrackVolume { delta: 5 },
            Command::SetTrackPan { value: 0 },
            Command::NudgeTrackPan { delta: 10 },
            Command::ToggleMute,
            Command::ToggleSolo,
            Command::SetTrackChannel { channel: 0 },
            Command::SetInstrument { path: None, bank: 0, preset: 0 },
            Command::NextPreset,
            Command::PrevPreset,
            Command::SetPreset { preset: 33 },
            Command::SetDrumKit { kit: 0 },
            Command::SetMasterReverb { percent: 25 },
            Command::ShowLane { kind: Some(LaneKind::Sustain) },
            Command::CycleLane,
            Command::SetLaneValue { value: 127 },
            Command::NudgeLaneValue { delta: 8 },
            Command::RampLane { from: 0, to: 127 },
            Command::ClearLane,
            Command::SustainDown,
            Command::SustainUp,
            Command::PedalSelection,
            Command::Play,
            Command::Stop,
            Command::TogglePlay,
            Command::PlayFromCursor,
            Command::EnterCommandLine,
        ]
    }

    /// Every command name, in [`Command::examples`] order.
    pub fn all_names() -> Vec<&'static str> {
        Command::examples().iter().map(Command::name).collect()
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// A pan position for humans: `C` for centre, `L40` / `R25` for left/right in percent.
pub fn pan_label(value: i32) -> String {
    match value {
        0 => "C".to_string(),
        v if v < 0 => format!("L{}", -v),
        v => format!("R{v}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{Division, GridModifier};

    #[test]
    fn toml_spelling() {
        let c: Command = toml::from_str::<toml::Table>("cmd = \"move_cursor\"\ndir = \"right\"\nunit = \"grid\"")
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(c, Command::MoveCursor { dir: Dir::Right, unit: Unit::Grid });
        let c: Command = toml::from_str::<toml::Table>("cmd = \"insert_note\"").unwrap().try_into().unwrap();
        assert_eq!(c, Command::InsertNote);
        let c: Command =
            toml::from_str::<toml::Table>("cmd = \"set_grid\"\ngrid = \"1/8T\"").unwrap().try_into().unwrap();
        assert_eq!(c, Command::SetGrid { grid: Grid { base: Division::Eighth, modifier: GridModifier::Triplet } });
        let c: Command = toml::from_str::<toml::Table>("cmd = \"extend_selection\"\ndir = \"up\"\nunit = \"octave\"")
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(c, Command::ExtendSelection { dir: Dir::Up, unit: Unit::Octave });
        // optional fields take their defaults
        let c: Command = toml::from_str::<toml::Table>("cmd = \"quantize\"").unwrap().try_into().unwrap();
        assert_eq!(c, Command::Quantize { grid: None, strength: 100, swing: 0 });
        let c: Command =
            toml::from_str::<toml::Table>("cmd = \"quantize\"\ngrid = \"1/8\"\nswing = 50").unwrap().try_into().unwrap();
        assert_eq!(c, Command::Quantize { grid: Some(Grid::plain(Division::Eighth)), strength: 100, swing: 50 });
        let c: Command = toml::from_str::<toml::Table>("cmd = \"select_next_notes\"").unwrap().try_into().unwrap();
        assert_eq!(c, Command::SelectNextNotes { n: 4 });
        let c: Command = toml::from_str::<toml::Table>("cmd = \"keep_every_other\"\nn = 3").unwrap().try_into().unwrap();
        assert_eq!(c, Command::KeepEveryOther { n: 3 });
        let c: Command =
            toml::from_str::<toml::Table>("cmd = \"insert_chord\"\nintervals = [0, 3, 7]").unwrap().try_into().unwrap();
        assert_eq!(c, Command::InsertChord { intervals: vec![0, 3, 7] });
        // lanes and instruments
        let c: Command = toml::from_str::<toml::Table>("cmd = \"show_lane\"\nkind = \"pitch_bend\"").unwrap().try_into().unwrap();
        assert_eq!(c, Command::ShowLane { kind: Some(LaneKind::PitchBend) });
        let c: Command = toml::from_str::<toml::Table>("cmd = \"show_lane\"").unwrap().try_into().unwrap();
        assert_eq!(c, Command::ShowLane { kind: None });
        let c: Command = toml::from_str::<toml::Table>("cmd = \"set_instrument\"\npreset = 33").unwrap().try_into().unwrap();
        assert_eq!(c, Command::SetInstrument { path: None, bank: 0, preset: 33 });
        let c: Command = toml::from_str::<toml::Table>("cmd = \"set_instrument\"\npath = \"gm.sf2\"\nbank = 1\npreset = 2")
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(c, Command::SetInstrument { path: Some("gm.sf2".into()), bank: 1, preset: 2 });
        let c: Command = toml::from_str::<toml::Table>("cmd = \"nudge_track_pan\"\ndelta = -10").unwrap().try_into().unwrap();
        assert_eq!(c, Command::NudgeTrackPan { delta: -10 });
    }

    #[test]
    fn names_and_descriptions_are_nonempty() {
        let mut all = Command::examples();
        all.extend([
            Command::MoveCursor { dir: Dir::Left, unit: Unit::Bar },
            Command::ExtendSelection { dir: Dir::Left, unit: Unit::Beat },
            Command::MoveSelected { dir: Dir::Up, unit: Unit::Octave },
            Command::ChangeLength { delta_steps: -1 },
            Command::TransposeSelected { semitones: -12 },
            Command::CycleGrid { dir: CycleDir::Prev },
            Command::Quantize { grid: Some(Grid::default()), strength: 50, swing: 30 },
            Command::KeepEveryOther { n: 3 },
            Command::ShowLane { kind: None },
            Command::ShowLane { kind: Some(LaneKind::Pan) },
            Command::SetInstrument { path: Some("a/b.sf2".into()), bank: 2, preset: 7 },
            Command::SetTrackPan { value: -40 },
            Command::NudgeTrackPan { delta: -10 },
            Command::NudgeTrackVolume { delta: -5 },
            Command::RenameTrack { name: "My Bass".to_string() },
        ]);
        for c in &all {
            assert!(!c.describe().is_empty());
            assert!(!c.name().is_empty());
            assert_eq!(c.to_string(), c.describe());
            // the name round-trips through serde's tag
            let value = toml::Value::try_from(c).unwrap();
            assert_eq!(value.get("cmd").and_then(|v| v.as_str()), Some(c.name()));
            // and the whole command round-trips through TOML
            let back: Command = value.try_into().unwrap();
            assert_eq!(&back, c);
        }
        assert!(Command::InsertNote.is_edit());
        assert!(!Command::Copy.is_edit());
        assert!(!Command::SelectAll.is_edit());
        assert!(!Command::Play.is_edit());
        assert!(!Command::SetSwing { percent: 50 }.is_edit());
        assert!(Command::ToggleMute.is_edit());
        assert!(Command::AddTrack.is_edit());
        assert!(Command::SustainDown.is_edit());
        assert!(!Command::ShowLane { kind: None }.is_edit());
        assert!(!Command::CycleLane.is_edit());
        assert_eq!(Category::Edit.to_string(), "Edit");
        assert_eq!(Category::Track.to_string(), "Track");
        assert_eq!(Category::Lane.to_string(), "Lane");
        assert_eq!(pan_label(0), "C");
        assert_eq!(pan_label(-40), "L40");
        assert_eq!(pan_label(25), "R25");
        assert_eq!(Command::SetTrackPan { value: -40 }.describe(), "Set track pan to L40");
        assert_eq!(Command::SetTrackChannel { channel: 9 }.describe(), "Set MIDI channel to 10");
        assert_eq!(Command::ShowLane { kind: Some(LaneKind::PitchBend) }.describe(), "Show the pitch bend lane");
    }

    #[test]
    fn examples_cover_every_variant() {
        let names = Command::all_names();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "every variant appears exactly once in examples()");
        // the examples are spelled with their defaults so the TOML form is the bare name
        for c in Command::examples() {
            let value = toml::Value::try_from(&c).unwrap();
            assert_eq!(value.get("cmd").and_then(|v| v.as_str()), Some(c.name()));
        }
        assert_eq!(Command::Quantize { grid: None, strength: 100, swing: 0 }.describe(), "Quantize to the current grid");
        assert_eq!(
            Command::Quantize { grid: Some(Grid::default()), strength: 50, swing: 20 }.describe(),
            "Quantize to 1/16 at 50% with 20% swing"
        );
    }
}
