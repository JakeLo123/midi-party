//! Pure data model and editing logic. No GUI, no audio device code.
//!
//! The crate is organised around three types:
//!
//! - [`Song`]: the document (tracks of notes, tempo and meter maps), in ticks.
//! - [`Command`]: every editing action as data, parsed from the TOML keymap.
//! - [`Editor`]: the state machine that executes commands, owns undo, and emits
//!   [`EditorEvent`]s for the app to forward to audio and the status bar.
//!
//! The flow is keypress → [`Keymap::feed`] → [`Command`] → [`Editor::execute`] → events.
//! [`test_harness::drive`] runs that whole pipeline from a key string, headless.

pub mod cmdline;
pub mod editor;
pub mod gm;
pub mod keymap;
pub mod midi_file;
pub mod project;
pub mod song;
pub mod test_harness;
pub mod time;
pub mod undo;

pub use cmdline::CmdLineError;
pub use editor::{
    instrument_name, pan_label, pitch_name, Category, Command, Cursor, CycleDir, Dir, EditError, Editor, EditorEvent,
    TransportRequest, Unit, ViewRequest, DEFAULT_CURSOR_PITCH, INSERT_VELOCITY, MAX_CHANNEL, MAX_PRESET,
    MAX_TRACK_VOLUME_PERCENT, PITCH_BEND_NUDGE_SCALE, SUSTAIN_DOWN, SUSTAIN_UP, ZOOM_ROWS_STEP, ZOOM_TIME_STEP,
};
pub use keymap::{
    parse_sequence, sequence_to_string, Binding, Key, KeyChord, KeyParseError, Keymap, KeymapError, Mods,
    Resolution, BINDINGS_TABLE,
};
pub use gm::{drum_kit_name, percussion_name, percussion_short, program_name, GM_PROGRAMS};
pub use project::ProjectError;
pub use song::{
    is_default_track_name, Instrument, LaneKind, LaneKindParseError, LanePoint, Lanes, Note, NoteId, Song, Tick, Track,
    DEFAULT_MASTER_REVERB, DEFAULT_PPQ, DEFAULT_TRACK_HEADER_WIDTH, DRUM_CHANNEL,
};
pub use test_harness::{default_keymap, drive, drive_with, feed_one, try_drive_with, DriveError, DEFAULT_KEYMAP_TOML};
pub use time::{
    beat_ticks, BarBeat, Division, Grid, GridModifier, MeterMap, MeterPoint, TempoMap, TempoPoint, TimeError,
};
pub use undo::{Snapshot, UndoStack, DEFAULT_UNDO_CAP};

#[cfg(test)]
mod editor_tests;
