//! Headless driver: a key string in, an [`Editor`] out. Used by the unit tests and by
//! `mp-cli keys`, so it is compiled always, not only under `cfg(test)`.
//!
//! Keys use the same syntax as the keymap file, space separated:
//! `"v Right Right Down Enter x"` or `"3 Right n"`. Counts are accumulated here exactly
//! as the app does it: `Count` digits go to [`Editor::push_count_digit`] and the count is
//! taken when the next `Command` resolves; `Unbound` drops it.

use thiserror::Error;

use crate::editor::{EditError, Editor, EditorEvent};
use crate::keymap::{parse_sequence, KeyChord, KeyParseError, Keymap, Resolution};
use crate::song::Song;

/// The shipped default keymap, embedded at compile time.
pub const DEFAULT_KEYMAP_TOML: &str = include_str!("../../../keymaps/default.toml");

/// Parses the embedded default keymap. Panics only if the shipped file is broken,
/// which the test suite checks.
pub fn default_keymap() -> Keymap {
    Keymap::from_toml_str(DEFAULT_KEYMAP_TOML).expect("keymaps/default.toml must parse")
}

/// Why a driven key string stopped.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DriveError {
    #[error("cannot parse keys: {0}")]
    BadKeys(#[from] KeyParseError),
    #[error("key {key} is not bound")]
    Unbound { key: String },
    #[error("key {key}: {source}")]
    Edit {
        key: String,
        #[source]
        source: EditError,
    },
}

/// Feeds `keys` into `editor` through `keymap`, collecting every event. Stops at the
/// first unbound key or editor error and returns it (the editor keeps whatever was
/// applied before it).
pub fn try_drive_with(editor: &mut Editor, keymap: &mut Keymap, keys: &str) -> Result<Vec<EditorEvent>, DriveError> {
    let chords = parse_sequence(keys)?;
    let mut events = Vec::new();
    for chord in chords {
        feed_one(editor, keymap, chord, &mut events)?;
    }
    Ok(events)
}

/// Feeds one chord; the single step behind [`try_drive_with`]. The app's input layer
/// can use this too, since it implements the count handling exactly once.
pub fn feed_one(
    editor: &mut Editor,
    keymap: &mut Keymap,
    chord: KeyChord,
    events: &mut Vec<EditorEvent>,
) -> Result<(), DriveError> {
    match keymap.feed(chord) {
        Resolution::Pending => Ok(()),
        Resolution::Count(d) => {
            editor.push_count_digit(d);
            Ok(())
        }
        Resolution::Command(cmd) => {
            let count = editor.take_count();
            let ev = editor.execute(&cmd, count).map_err(|source| DriveError::Edit { key: chord.to_string(), source })?;
            events.extend(ev);
            Ok(())
        }
        Resolution::Unbound => {
            editor.take_count();
            Err(DriveError::Unbound { key: chord.to_string() })
        }
    }
}

/// Like [`try_drive_with`] but panics with a readable message on any error, for tests.
pub fn drive_with(editor: &mut Editor, keymap: &mut Keymap, keys: &str) -> Vec<EditorEvent> {
    match try_drive_with(editor, keymap, keys) {
        Ok(ev) => ev,
        Err(e) => panic!("drive {keys:?}: {e}"),
    }
}

/// Builds a fresh [`Editor`] on `song`, drives `keys` through a copy of `keymap`, and
/// returns the editor. Panics on an unbound key or an editor error.
pub fn drive(song: Song, keymap: &Keymap, keys: &str) -> Editor {
    let mut editor = Editor::new(song);
    let mut km = keymap.clone();
    drive_with(&mut editor, &mut km, keys);
    editor
}
