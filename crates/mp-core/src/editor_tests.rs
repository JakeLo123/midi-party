//! End-to-end tests through the key-string harness: every [`Command`] variant has at
//! least one `drive` test here, undo tests assert `song == before`, counts are
//! exercised, and a few RON snapshots pin the resulting songs.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::cmdline;
use crate::editor::{
    Command, Cursor, CycleDir, Dir, EditError, EditorEvent, TransportRequest, Unit, ViewRequest, ZOOM_ROWS_STEP,
    ZOOM_TIME_STEP,
};
use crate::keymap::{parse_sequence, Keymap, Resolution};
use crate::song::{is_default_track_name, Instrument, LaneKind, NoteId, Song, Tick, Track, DRUM_CHANNEL};
use crate::test_harness::{default_keymap, drive, drive_with, try_drive_with, DriveError, DEFAULT_KEYMAP_TOML};
use crate::time::{Division, Grid, GridModifier};
use crate::Editor;

const G: u64 = 240; // one 1/16 grid step at 960 PPQ

fn km() -> Keymap {
    default_keymap()
}

/// A song with three notes on track 1: C4@0, E4@240 (two steps long), G4@960.
fn song3() -> Song {
    let mut s = Song::new_empty();
    let notes = [
        s.new_note(Tick(0), Tick(G), 60, 100),
        s.new_note(Tick(G), Tick(2 * G), 64, 100),
        s.new_note(Tick(4 * G), Tick(G), 67, 100),
    ];
    for n in notes {
        s.tracks[0].insert_note(n);
    }
    s
}

/// Two bars with chords and an off-grid note, ids 1..=9 in `(start, pitch)` order:
/// a C major triad at 0 (ids 1-3), D4 at beat 2 (4), an F4+A4 dyad at beat 3 (5, 6),
/// an off-beat, off-grid B4 at 2900 (7), then C5 at bar 2 (8) and E5 a step later (9).
/// Velocities vary so the velocity selectors have something to find.
fn chords() -> Song {
    let mut s = Song::new_empty();
    let spec = [
        (0, 60, 100),
        (0, 64, 90),
        (0, 67, 80),
        (960, 62, 70),
        (1920, 65, 110),
        (1920, 69, 60),
        (2900, 71, 50),
        (3840, 72, 100),
        (3840 + G, 76, 120),
    ];
    for (start, pitch, vel) in spec {
        let n = s.new_note(Tick(start), Tick(G), pitch, vel);
        s.tracks[0].insert_note(n);
    }
    s
}

fn ids(e: &Editor) -> Vec<u64> {
    e.selection.iter().map(|id| id.0).collect()
}

fn velocities(e: &Editor) -> Vec<u8> {
    e.track().unwrap().notes.iter().map(|n| n.velocity).collect()
}

fn durations(e: &Editor) -> Vec<u64> {
    e.track().unwrap().notes.iter().map(|n| n.duration.get()).collect()
}

fn no_match(err: DriveError) {
    assert!(matches!(err, DriveError::Edit { source: EditError::NoMatch(_), .. }), "{err:?}");
}

fn pitches(e: &Editor) -> Vec<u8> {
    e.track().unwrap().notes.iter().map(|n| n.pitch).collect()
}

fn starts(e: &Editor) -> Vec<u64> {
    e.track().unwrap().notes.iter().map(|n| n.start.get()).collect()
}

fn has_song_changed(ev: &[EditorEvent]) -> bool {
    ev.contains(&EditorEvent::SongChanged)
}

fn auditioned(ev: &[EditorEvent]) -> usize {
    ev.iter().filter_map(|e| match e {
        EditorEvent::Audition(n) => Some(n.len()),
        _ => None,
    })
    .sum()
}

fn status(ev: &[EditorEvent]) -> Vec<&str> {
    ev.iter().filter_map(|e| match e {
        EditorEvent::Status(s) => Some(s.as_str()),
        _ => None,
    })
    .collect()
}

fn last_status(ev: &[EditorEvent]) -> &str {
    status(ev).last().copied().unwrap_or("")
}

/// `"1 1 u"`: a count typed digit by digit before `key`.
fn counted(n: usize, key: &str) -> String {
    let digits: Vec<String> = n.to_string().chars().map(String::from).collect();
    format!("{} {key}", digits.join(" "))
}

/// The command a key (or key sequence) resolves to in Normal mode.
fn bound(keymap: &Keymap, keys: &str) -> Command {
    let mut k = keymap.clone();
    let mut cmd = None;
    for chord in parse_sequence(keys).unwrap() {
        match k.feed(chord) {
            Resolution::Command(c) => cmd = Some(c),
            Resolution::Pending => {}
            other => panic!("{keys}: {other:?}"),
        }
    }
    cmd.unwrap_or_else(|| panic!("{keys} is not bound"))
}

// ---------------------------------------------------------------- keymap file

#[test]
fn default_keymap_loads_and_every_binding_is_a_real_command() {
    let keymap = Keymap::from_toml_str(DEFAULT_KEYMAP_TOML).expect("default.toml parses");
    let mut song = Song::new_empty();
    // a note so note-targeting commands have something to act on
    let n = song.new_note(Tick(0), Tick(G), 60, 100);
    song.tracks[0].insert_note(n);
    let bindings = keymap.bindings();
    assert!(bindings.len() > 100, "{} bindings", bindings.len());
    for (keys, cmd) in &bindings {
        assert!(!cmd.describe().is_empty(), "{keys} -> {cmd:?}");
        // every bound command executes on a fresh editor (errors are fine: it means
        // the variant exists and was matched; panics would mean a todo!())
        let mut e = Editor::new(song.clone());
        e.selection.insert(n.id);
        let _ = e.execute(cmd, None);
    }
    // arrows: plain moves the cursor, Shift grows the box, Alt moves notes, Cmd jumps
    assert_eq!(bound(&keymap, "Right"), Command::MoveCursor { dir: Dir::Right, unit: Unit::Grid });
    assert_eq!(bound(&keymap, "Ctrl+Right"), Command::MoveCursor { dir: Dir::Right, unit: Unit::Beat });
    assert_eq!(bound(&keymap, "Cmd+Right"), Command::MoveCursor { dir: Dir::Right, unit: Unit::Bar });
    assert_eq!(bound(&keymap, "Cmd+Up"), Command::MoveCursor { dir: Dir::Up, unit: Unit::Octave });
    assert_eq!(bound(&keymap, "Shift+Right"), Command::ExtendSelection { dir: Dir::Right, unit: Unit::Grid });
    assert_eq!(bound(&keymap, "Ctrl+Shift+Right"), Command::ExtendSelection { dir: Dir::Right, unit: Unit::Beat });
    assert_eq!(bound(&keymap, "Cmd+Shift+Left"), Command::ExtendSelection { dir: Dir::Left, unit: Unit::Bar });
    assert_eq!(bound(&keymap, "Cmd+Shift+Up"), Command::ExtendSelection { dir: Dir::Up, unit: Unit::Octave });
    assert_eq!(bound(&keymap, "Alt+Right"), Command::MoveSelected { dir: Dir::Right, unit: Unit::Grid });
    assert_eq!(bound(&keymap, "Ctrl+Alt+Right"), Command::MoveSelected { dir: Dir::Right, unit: Unit::Beat });
    assert_eq!(bound(&keymap, "Alt+Shift+Right"), Command::MoveSelected { dir: Dir::Right, unit: Unit::Bar });
    assert_eq!(bound(&keymap, "Alt+Shift+Up"), Command::MoveSelected { dir: Dir::Up, unit: Unit::Octave });
    assert_eq!(bound(&keymap, "Enter"), Command::PlayheadToStart);
    assert_eq!(bound(&keymap, "Shift+Enter"), Command::PlayheadToCursor);
    assert_eq!(bound(&keymap, "Alt+Enter"), Command::CursorToPlayhead);
    assert_eq!(bound(&keymap, "Cmd+x"), Command::Cut);
    assert_eq!(bound(&keymap, "Cmd+Shift+z"), Command::Redo);
    // the keys the plan promises
    for (cmd, key) in [
        (Command::Quantize { grid: None, strength: 100, swing: 0 }, "q"),
        (Command::Legato, "Shift+l"),
        (Command::Humanize { velocity: 8, timing: 10 }, "h"),
        (Command::InsertChord { intervals: vec![0, 4, 7] }, "Shift+n"),
        (Command::SelectChord, "Shift+s"),
        (Command::SelectBar, "b"),
        (Command::InvertSelection, "i"),
        (Command::SelectionFocusNext, "Tab"),
        (Command::SelectionFocusPrev, "Shift+Tab"),
        (Command::DropFocused, "Shift+x"),
        (Command::KeepTopOfChords, "f t"),
        (Command::NudgeSelected { ticks: 10 }, "Alt+."),
        (Command::NudgeSelected { ticks: -10 }, "Alt+,"),
        (Command::TogglePlay, "Space"),
        (Command::PlayheadToStart, "Enter"),
        (Command::PlayheadToCursor, "Shift+Enter"),
        (Command::CursorToPlayhead, "Alt+Enter"),
        (Command::PlayheadPrevBar, ","),
        (Command::PlayheadNextBar, "."),
        (Command::ToggleCheatSheet, "Shift+/"),
        (Command::EnterCommandLine, "Shift+;"),
        (Command::ZoomIn, "Cmd+="),
        (Command::ZoomOut, "Cmd+-"),
        (Command::AddTrack, "Cmd+t"),
        // key_for spells modifiers canonically: Ctrl, Alt, Shift, Cmd
        (Command::RemoveTrack, "Shift+Cmd+t"),
        (Command::DuplicateTrack, "Cmd+d"),
        (Command::MoveTrackUp, "Alt+Cmd+Up"),
        (Command::MoveTrackDown, "Alt+Cmd+Down"),
        (Command::ToggleMute, "Ctrl+m"),
        (Command::ToggleSolo, "Ctrl+s"),
        (Command::NudgeTrackVolume { delta: 5 }, "Ctrl+="),
        (Command::NudgeTrackVolume { delta: -5 }, "Ctrl+-"),
        (Command::NudgeTrackPan { delta: 10 }, "Ctrl+]"),
        (Command::NudgeTrackPan { delta: -10 }, "Ctrl+["),
        (Command::NextPreset, "Ctrl+."),
        (Command::PrevPreset, "Ctrl+,"),
        (Command::CycleLane, "Shift+c"),
        (Command::NudgeLaneValue { delta: 8 }, "Ctrl+Shift+Up"),
        (Command::NudgeLaneValue { delta: -8 }, "Ctrl+Shift+Down"),
        (Command::ClearLane, "Ctrl+Shift+x"),
        (Command::SustainDown, "Ctrl+Shift+d"),
        (Command::SustainUp, "Ctrl+Shift+u"),
    ] {
        assert_eq!(keymap.key_for(&cmd).as_deref(), Some(key), "{}", cmd.name());
    }
    // the plan's original keys are untouched
    for (cmd, key) in [(Command::NextTrack, "t"), (Command::PrevTrack, "Shift+t"), (Command::Duplicate, "d")] {
        assert_eq!(keymap.key_for(&cmd).as_deref(), Some(key), "{}", cmd.name());
    }
    // the letters keep working next to their Cmd aliases; bindings are kept in key
    // order, so key_for (the cheat sheet) names the Cmd alias first
    for (cmd, letter, alias) in [
        (Command::Undo, "u", "Cmd+z"),
        (Command::Redo, "Shift+u", "Shift+Cmd+z"),
        (Command::Copy, "c", "Cmd+c"),
        (Command::Paste, "p", "Cmd+v"),
        (Command::SelectAll, "a", "Cmd+a"),
    ] {
        assert_eq!(bound(&keymap, letter), cmd, "{letter}");
        assert_eq!(bound(&keymap, alias), cmd, "{alias}");
        assert_eq!(keymap.key_for(&cmd).as_deref(), Some(alias), "{}", cmd.name());
    }
    // command-line-only commands have no key
    for cmd in [
        Command::TransposeSelected { semitones: 12 },
        Command::RenameTrack { name: "x".into() },
        Command::SetTrackVolume { percent: 100 },
        Command::SetTrackPan { value: 0 },
        Command::SetTrackChannel { channel: 0 },
        Command::SetInstrument { path: None, bank: 0, preset: 0 },
        Command::SetMasterReverb { percent: 25 },
        Command::ShowLane { kind: Some(LaneKind::Sustain) },
        Command::SetLaneValue { value: 127 },
        Command::RampLane { from: 0, to: 127 },
        Command::PedalSelection,
    ] {
        assert_eq!(keymap.key_for(&cmd), None, "{}", cmd.name());
        assert!(cmdline::usage(cmd.name()).is_some(), "{}", cmd.name());
    }
    for key in ["v", "m", "Shift+v", "Ctrl+Up", "Ctrl+Down"] {
        let chord = parse_sequence(key).unwrap().into_iter().next().unwrap();
        assert_eq!(keymap.clone().feed(chord), Resolution::Unbound, "{key} must stay unbound by default");
    }
}

#[test]
fn cmd_aliases_resolve() {
    let keymap = km();
    assert_eq!(bound(&keymap, "Cmd+a"), Command::SelectAll);
    assert_eq!(bound(&keymap, "Cmd+c"), Command::Copy);
    assert_eq!(bound(&keymap, "Cmd+x"), Command::Cut);
    assert_eq!(bound(&keymap, "Cmd+v"), Command::Paste);
    assert_eq!(bound(&keymap, "Cmd+z"), Command::Undo);
    assert_eq!(bound(&keymap, "Cmd+Shift+z"), Command::Redo);
    assert_eq!(bound(&keymap, "Shift+Cmd+z"), Command::Redo, "modifier order does not matter");
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "Cmd+a Cmd+c Cmd+Right Cmd+v");
    assert_eq!(e.track().unwrap().notes.len(), 6);
    drive_with(&mut e, &mut k, "Cmd+z");
    assert_eq!(e.song, song3());
    drive_with(&mut e, &mut k, "Cmd+Shift+z");
    assert_eq!(e.track().unwrap().notes.len(), 6);
    drive_with(&mut e, &mut k, "Cmd+a Cmd+x");
    assert!(e.track().unwrap().notes.is_empty());
    assert_eq!(e.clipboard.len(), 6);
}

#[test]
fn every_command_variant_executes_without_panicking() {
    // errors are fine (a fresh editor has little to act on); panics would mean a todo!()
    for cmd in Command::examples() {
        for song in [Song::new_empty(), chords()] {
            let mut e = Editor::new(song);
            let _ = e.execute(&cmd, None);
            let _ = e.execute(&cmd, Some(3));
            e.execute(&Command::SelectAll, None).ok();
            let _ = e.execute(&cmd, None);
        }
    }
}

// ---------------------------------------------------------------- cursor

#[test]
fn move_cursor_by_grid_and_semitone() {
    let e = drive(Song::new_empty(), &km(), "Right Right Up");
    assert_eq!(e.cursor.tick, Tick(2 * G));
    assert_eq!(e.cursor.pitch, 61);
    let e = drive(Song::new_empty(), &km(), "Right Left Left Down Down");
    assert_eq!(e.cursor.tick, Tick::ZERO, "tick never goes below 0");
    assert_eq!(e.cursor.pitch, 58);
}

#[test]
fn move_cursor_by_bar_beat_and_octave() {
    let e = drive(Song::new_empty(), &km(), "Cmd+Right Cmd+Up Ctrl+Right");
    assert_eq!(e.cursor.tick, Tick(3840 + 960));
    assert_eq!(e.cursor.pitch, 72);
    let e = drive(Song::new_empty(), &km(), "Cmd+Up Cmd+Up Cmd+Up Cmd+Up Cmd+Up Cmd+Up");
    assert_eq!(e.cursor.pitch, 127, "pitch clamps at 127");
    let e = drive(Song::new_empty(), &km(), "Cmd+Down Cmd+Down Cmd+Down Cmd+Down Cmd+Down Cmd+Down");
    assert_eq!(e.cursor.pitch, 0, "pitch clamps at 0");
}

#[test]
fn counts_repeat_cursor_moves() {
    let e = drive(Song::new_empty(), &km(), "3 Right");
    assert_eq!(e.cursor.tick, Tick(3 * G));
    let e = drive(Song::new_empty(), &km(), "1 2 Up");
    assert_eq!(e.cursor.pitch, 72);
    assert_eq!(e.count, None, "count consumed");
    // a count followed by an unbound key is dropped
    let mut e = Editor::new(Song::new_empty());
    let mut k = km();
    let err = try_drive_with(&mut e, &mut k, "3 z").unwrap_err();
    assert!(matches!(err, DriveError::Unbound { .. }));
    assert_eq!(e.count, None);
    drive_with(&mut e, &mut k, "Right");
    assert_eq!(e.cursor.tick, Tick(G));
}

#[test]
fn plain_arrow_clears_selection() {
    for keys in ["a Right", "a Up", "a 3 Left", "a Ctrl+Right", "a Cmd+Right", "a Cmd+Up"] {
        let e = drive(song3(), &km(), keys);
        assert!(e.selection.is_empty(), "{keys}");
        assert_eq!(e.undo.undo_depth(), 0, "{keys}");
    }
    // a box in progress goes too
    let e = drive(song3(), &km(), "Shift+Right Shift+Up Shift+Up Shift+Up Shift+Up Left");
    assert!(e.selection.is_empty());
    assert_eq!(e.box_anchor, None);
    assert_eq!(e.cursor.tick, Tick::ZERO);
    assert_eq!(e.cursor.pitch, 64);
}

// ---------------------------------------------------------------- insert / delete

#[test]
fn insert_note_at_cursor() {
    let mut e = Editor::new(Song::new_empty());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Right Up n");
    let t = e.track().unwrap();
    assert_eq!(t.notes.len(), 1);
    let n = t.notes[0];
    assert_eq!((n.start, n.duration, n.pitch, n.velocity), (Tick(G), Tick(G), 61, 100));
    assert_eq!(ids(&e), vec![n.id.0], "new note is selected");
    assert_eq!(e.cursor.tick, Tick(G), "cursor stays");
    assert!(has_song_changed(&ev));
    assert_eq!(auditioned(&ev), 1);
    assert_eq!(status(&ev), vec!["inserted C#4 (1/16)"]);
}

#[test]
fn insert_note_replaces_same_start_and_pitch_and_count_sets_length() {
    let e = drive(Song::new_empty(), &km(), "n 4 n");
    let t = e.track().unwrap();
    assert_eq!(t.notes.len(), 1);
    assert_eq!(t.notes[0].duration, Tick(4 * G));
    assert_eq!(e.undo.undo_depth(), 2);
}

#[test]
fn insert_uses_current_grid() {
    let e = drive(Song::new_empty(), &km(), "Shift+g n");
    assert_eq!(e.grid.to_string(), "1/8");
    assert_eq!(e.track().unwrap().notes[0].duration, Tick(480));
}

#[test]
fn delete_selected_or_under_cursor() {
    // nothing selected: deletes the note under the cursor
    let e = drive(song3(), &km(), "x");
    assert_eq!(starts(&e), vec![G, 4 * G]);
    // cursor inside a sounding note counts as under it
    let e = drive(song3(), &km(), "Right Right Up Up Up Up x");
    assert_eq!(starts(&e), vec![0, 4 * G]);
    // with a selection: deletes the selection, not the note under the cursor
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "a x");
    assert!(e.track().unwrap().notes.is_empty());
    assert!(e.selection.is_empty());
    // Delete and Backspace are aliases
    let e = drive(song3(), &km(), "Delete");
    assert_eq!(e.track().unwrap().notes.len(), 2);
    let e = drive(song3(), &km(), "Backspace");
    assert_eq!(e.track().unwrap().notes.len(), 2);
}

#[test]
fn delete_with_nothing_is_an_error_and_changes_nothing() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let before = e.song.clone();
    let err = try_drive_with(&mut e, &mut k, "Up x").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "x".into(), source: EditError::NothingSelected });
    assert_eq!(e.song, before);
    assert_eq!(e.undo.undo_depth(), 0);
}

// ---------------------------------------------------------------- selection

#[test]
fn toggle_select_under_cursor() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "s");
    assert_eq!(ids(&e), vec![1]);
    assert_eq!(auditioned(&ev), 1, "toggled note is auditioned");
    assert_eq!(status(&ev), vec!["selected C4"]);
    assert!(!has_song_changed(&ev));
    let ev = drive_with(&mut e, &mut k, "s");
    assert!(e.selection.is_empty());
    assert_eq!(status(&ev), vec!["deselected C4"]);
    assert_eq!(e.undo.undo_depth(), 0, "selection is not undoable");
}

#[test]
fn shift_arrows_box_select() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Shift+Right");
    assert_eq!(e.box_anchor, Some(Editor::new(song3()).cursor), "the anchor is where the cursor started");
    assert_eq!(e.cursor.tick, Tick(G), "the cursor moves");
    assert_eq!(ids(&e), vec![1], "the anchor cell already selects the note at the cursor");
    assert_eq!(status(&ev), vec!["1 in box"]);
    let ev = drive_with(&mut e, &mut k, "Shift+Up Shift+Up Shift+Up Shift+Up");
    assert_eq!(ids(&e), vec![1, 2], "box is inclusive of the cursor cell");
    assert_eq!(last_status(&ev), "2 in box");
    drive_with(&mut e, &mut k, "Shift+Right Shift+Right Shift+Right Shift+Up Shift+Up Shift+Up");
    assert_eq!(ids(&e), vec![1, 2, 3]);
    drive_with(&mut e, &mut k, "Shift+Left");
    assert_eq!(ids(&e), vec![1, 2], "shrinking the box deselects");
    assert_eq!(e.box_anchor, Some(Editor::new(song3()).cursor), "the anchor stays put");
    assert_eq!(e.undo.undo_depth(), 0, "selecting is not an edit");
    // a count extends that many steps
    let e = drive(song3(), &km(), "4 Shift+Right 7 Shift+Up");
    assert_eq!(ids(&e), vec![1, 2, 3]);
    assert_eq!(e.cursor.tick, Tick(4 * G));
    assert_eq!(e.cursor.pitch, 67);
}

#[test]
fn any_other_key_ends_the_box_but_keeps_the_selection() {
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "Shift+Right Shift+Up Shift+Up Shift+Up Shift+Up");
    assert_eq!(ids(&e), vec![1, 2]);
    assert!(e.box_anchor.is_some());
    drive_with(&mut e, &mut k, "]");
    assert_eq!(e.box_anchor, None, "an edit ends the box");
    assert_eq!(ids(&e), vec![1, 2], "but the selection stays");
    assert_eq!(durations(&e), vec![2 * G, 3 * G, G]);
    // the next Shift+arrow starts a fresh box at the cursor, replacing the selection
    let ev = drive_with(&mut e, &mut k, "Shift+Right");
    assert_eq!(e.box_anchor, Some(Cursor { tick: Tick(G), pitch: 64 }));
    assert_eq!(ids(&e), vec![2]);
    assert_eq!(status(&ev), vec!["1 in box"]);
    // non-edits end the box too
    let e = drive(song3(), &km(), "Shift+Right Shift+Up Shift+Up Shift+Up Shift+Up Space");
    assert_eq!(e.box_anchor, None);
    assert_eq!(ids(&e), vec![1, 2]);
}

#[test]
fn escape_clears_box_selection() {
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "4 Shift+Right 7 Shift+Up");
    assert_eq!(ids(&e), vec![1, 2, 3]);
    let ev = drive_with(&mut e, &mut k, "Escape");
    assert!(e.selection.is_empty());
    assert_eq!(e.box_anchor, None);
    assert_eq!(status(&ev), vec!["selection cleared"]);
}

#[test]
fn box_select_works_backwards() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Cmd+Right Cmd+Up Cmd+Shift+Left");
    assert!(e.selection.is_empty(), "nothing at C5 in the first bar");
    assert_eq!(last_status(&ev), "0 in box");
    drive_with(&mut e, &mut k, "Cmd+Shift+Down");
    assert_eq!(ids(&e), vec![1, 2, 3]);
    assert_eq!(e.cursor, Editor::new(song3()).cursor);
}

#[test]
fn extend_by_bar_and_octave() {
    let mut e = Editor::new(chords());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Cmd+Shift+Right");
    assert_eq!(e.cursor.tick, Tick(3840));
    assert_eq!(ids(&e), vec![1], "only C4 lies on the cursor pitch in the box");
    assert_eq!(status(&ev), vec!["1 in box"]);
    let ev = drive_with(&mut e, &mut k, "Cmd+Shift+Up");
    assert_eq!(e.cursor.pitch, 72);
    assert_eq!(ids(&e), vec![1, 2, 3, 4, 5, 6, 7, 8], "E5 starts one step past the bar line");
    assert_eq!(status(&ev), vec!["8 in box"]);
    drive_with(&mut e, &mut k, "Ctrl+Shift+Right 4 Shift+Up");
    assert_eq!(e.selection.len(), 9);
    assert_eq!(e.box_anchor, Some(Editor::new(chords()).cursor));
}

#[test]
fn box_select_in_other_tracks_uses_their_notes() {
    let mut song = Song::new_empty();
    song.add_track(Track::new("B"));
    let n = song.new_note(Tick(0), Tick(G), 60, 100);
    song.tracks[1].insert_note(n);
    let e = drive(song, &km(), "t Shift+Right");
    assert_eq!(e.selection, [n.id].into_iter().collect::<BTreeSet<NoteId>>());
}

#[test]
fn select_all_and_clear() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a");
    assert_eq!(ids(&e), vec![1, 2, 3]);
    assert_eq!(status(&ev), vec!["selected all 3 notes"]);
    let ev = drive_with(&mut e, &mut k, "Escape");
    assert!(e.selection.is_empty());
    assert_eq!(status(&ev), vec!["selection cleared"]);
    drive_with(&mut e, &mut k, "Cmd+a");
    assert_eq!(ids(&e), vec![1, 2, 3]);
}

// ---------------------------------------------------------------- move selected

#[test]
fn alt_arrows_move_selected_notes_in_time() {
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "a");
    let ev = drive_with(&mut e, &mut k, "Alt+Right");
    assert_eq!(starts(&e), vec![G, 2 * G, 5 * G]);
    assert_eq!(e.cursor.tick, Tick(G), "cursor follows the notes");
    assert_eq!(ids(&e), vec![1, 2, 3], "the notes stay selected");
    assert_eq!(auditioned(&ev), 3);
    assert_eq!(status(&ev), vec!["moved 3 notes +1 grid step"]);
    // a bar right, a beat left
    drive_with(&mut e, &mut k, "Alt+Shift+Right Ctrl+Alt+Left");
    assert_eq!(starts(&e), vec![G + 3840 - 960, 2 * G + 3840 - 960, 5 * G + 3840 - 960]);
    assert_eq!(e.cursor.tick, Tick(G + 3840 - 960));
    assert_eq!(ids(&e), vec![1, 2, 3]);
    assert_eq!(e.box_anchor, None);
}

#[test]
fn move_selected_clamps_at_song_start_as_a_group() {
    let mut e = Editor::new(song3());
    let mut k = km();
    // a note at tick 0 is selected: nothing can move left at all
    drive_with(&mut e, &mut k, "a");
    let before = e.song.clone();
    let err = try_drive_with(&mut e, &mut k, "Alt+Shift+Left").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }));
    assert_eq!(e.song, before);
    assert_eq!(e.undo.undo_depth(), 0);
    // group clamp: box E4@240 and G4@960, move left 3 steps -> limited to 1 step
    let mut e = Editor::new(song3());
    drive_with(&mut e, &mut k, "Right Up Up Up Up Shift+Right Shift+Right Shift+Right Shift+Up Shift+Up Shift+Up");
    assert_eq!(ids(&e), vec![2, 3]);
    drive_with(&mut e, &mut k, "3 Alt+Left");
    assert_eq!(starts(&e), vec![0, 0, 3 * G]);
    assert_eq!(pitches(&e), vec![60, 64, 67]);
}

#[test]
fn move_selected_in_pitch_and_clamps_as_a_group() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a Alt+Up");
    assert_eq!(pitches(&e), vec![61, 65, 68]);
    assert_eq!(e.cursor.pitch, 61);
    assert_eq!(last_status(&ev), "moved 3 notes +1 st");
    drive_with(&mut e, &mut k, "Alt+Shift+Down");
    assert_eq!(pitches(&e), vec![49, 53, 56]);
    // 9 octaves up would push G4 past 127: everything moves by the same clamped amount
    drive_with(&mut e, &mut k, "9 Alt+Shift+Up");
    assert_eq!(pitches(&e), vec![120, 124, 127]);
    let err = try_drive_with(&mut e, &mut k, "Alt+Up").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }));
}

#[test]
fn move_selected_falls_back_to_note_under_cursor() {
    let e = drive(song3(), &km(), "Alt+Right Alt+Right");
    assert_eq!(starts(&e), vec![G, 2 * G, 4 * G]);
    assert_eq!(e.cursor.tick, Tick(2 * G));
}

#[test]
fn repeated_moves_coalesce_into_one_undo_step() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let before = e.song.clone();
    drive_with(&mut e, &mut k, "a Alt+Right Alt+Right Alt+Up Alt+Up");
    assert_eq!(e.undo.undo_depth(), 1);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, before);
    // redo ends the run (depth 2 after the next move); any non-edit in between starts
    // another step (depth 3)
    drive_with(&mut e, &mut k, "Shift+u Alt+Right");
    assert_eq!(e.undo.undo_depth(), 2);
    drive_with(&mut e, &mut k, "Escape Alt+Right");
    assert_eq!(e.undo.undo_depth(), 3);
}

// ---------------------------------------------------------------- length / velocity / transpose

#[test]
fn change_length() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "]");
    assert_eq!(e.track().unwrap().notes[0].duration, Tick(2 * G));
    assert_eq!(status(&ev), vec!["lengthened 1 note by 1 grid step"]);
    drive_with(&mut e, &mut k, "3 ]");
    assert_eq!(e.track().unwrap().notes[0].duration, Tick(5 * G));
    drive_with(&mut e, &mut k, "[ [ [ [ [");
    assert_eq!(e.track().unwrap().notes[0].duration, Tick(G / 4), "floor is a quarter grid step");
    let before = e.song.clone();
    let err = try_drive_with(&mut e, &mut k, "[").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }));
    assert_eq!(e.song, before);
    // every length change coalesced into one undo step
    assert_eq!(e.undo.undo_depth(), 1);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, song3());
    // on a selection
    let e = drive(song3(), &km(), "a ] ]");
    assert!(e.track().unwrap().notes.iter().all(|n| n.duration >= Tick(3 * G)));
}

#[test]
fn nudge_velocity() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a =");
    assert!(e.track().unwrap().notes.iter().all(|n| n.velocity == 105));
    assert_eq!(last_status(&ev), "velocity +5 on 3 notes");
    drive_with(&mut e, &mut k, "Shift+- - 2 -");
    assert!(e.track().unwrap().notes.iter().all(|n| n.velocity == 70));
    drive_with(&mut e, &mut k, "Shift+= Shift+= Shift+=");
    assert!(e.track().unwrap().notes.iter().all(|n| n.velocity == 127), "clamped at 127");
    let err = try_drive_with(&mut e, &mut k, "=").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }));
    assert_eq!(e.undo.undo_depth(), 1, "one coalesced undo step for the whole run");
    drive_with(&mut e, &mut k, "u");
    assert!(e.track().unwrap().notes.iter().all(|n| n.velocity == 100));
    // nothing selected: the note under the cursor
    let e = drive(song3(), &km(), "-");
    assert_eq!(e.track().unwrap().notes[0].velocity, 95);
}

#[test]
fn set_velocity_is_a_command_without_a_default_binding() {
    let mut e = Editor::new(song3());
    let ev = e.execute(&Command::SetVelocity { value: 0 }, None).unwrap();
    assert_eq!(e.track().unwrap().notes[0].velocity, 1, "0 clamps to 1");
    assert!(has_song_changed(&ev));
    assert_eq!(e.undo.undo_depth(), 1);
    // setting the same value again is a no-op: no undo step, still Ok
    let ev = e.execute(&Command::SetVelocity { value: 1 }, None).unwrap();
    assert!(!has_song_changed(&ev));
    assert_eq!(e.undo.undo_depth(), 1);
    e.execute(&Command::SelectAll, None).unwrap();
    e.execute(&Command::SetVelocity { value: 200 }, Some(5)).unwrap();
    assert!(e.track().unwrap().notes.iter().all(|n| n.velocity == 127));
}

#[test]
fn transpose_selected() {
    // no default key: the command line's `t 12`
    assert_eq!(cmdline::parse("t 12").unwrap(), Command::TransposeSelected { semitones: 12 });
    assert_eq!(cmdline::parse("transpose -1").unwrap(), Command::TransposeSelected { semitones: -1 });
    let up = Command::TransposeSelected { semitones: 1 };
    let down = Command::TransposeSelected { semitones: -1 };
    let octave_up = Command::TransposeSelected { semitones: 12 };
    let octave_down = Command::TransposeSelected { semitones: -12 };
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "a");
    let ev = e.execute(&cmdline::parse("t 12").unwrap(), None).unwrap();
    assert_eq!(pitches(&e), vec![72, 76, 79]);
    assert_eq!(auditioned(&ev), 3);
    assert_eq!(last_status(&ev), "transposed 3 notes +12 st");
    e.execute(&down, None).unwrap();
    e.execute(&down, None).unwrap();
    e.execute(&up, Some(2)).unwrap();
    assert_eq!(pitches(&e), vec![72, 76, 79]);
    e.execute(&octave_up, Some(9)).unwrap();
    assert_eq!(pitches(&e), vec![120, 124, 127], "group clamp keeps the chord shape");
    assert!(matches!(e.execute(&octave_up, None), Err(EditError::OutOfRange(_))));
    e.execute(&octave_down, Some(9)).unwrap();
    e.execute(&octave_down, Some(9)).unwrap();
    assert_eq!(pitches(&e), vec![0, 4, 7]);
    assert_eq!(e.cursor.pitch, 60, "transpose does not move the cursor");
    assert_eq!(e.undo.undo_depth(), 1, "transposes coalesce");
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, song3());
}

// ---------------------------------------------------------------- clipboard

#[test]
fn copy_paste() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a c");
    assert_eq!(e.clipboard.len(), 3);
    assert_eq!(e.clipboard[0].start, Tick::ZERO);
    assert_eq!(last_status(&ev), "copied 3 notes");
    assert_eq!(e.undo.undo_depth(), 0, "copy is not an edit");
    // paste at bar 2 with the original pitches
    let ev = drive_with(&mut e, &mut k, "Cmd+Right p");
    assert_eq!(e.track().unwrap().notes.len(), 6);
    assert_eq!(starts(&e), vec![0, G, 4 * G, 3840, 3840 + G, 3840 + 4 * G]);
    assert_eq!(pitches(&e), vec![60, 64, 67, 60, 64, 67]);
    assert_eq!(ids(&e), vec![4, 5, 6], "pasted notes become the selection");
    assert_eq!(e.cursor.tick, Tick(3840 + 5 * G), "cursor jumps to the end of the paste");
    assert_eq!(auditioned(&ev), 3);
    assert!(has_song_changed(&ev));
    // copy with nothing selected uses the note under the cursor
    let e = drive(song3(), &km(), "c Cmd+Right p");
    assert_eq!(starts(&e), vec![0, G, 4 * G, 3840]);
}

#[test]
fn cut_is_copy_plus_delete_in_one_undo_step() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a Cmd+x");
    assert!(e.track().unwrap().notes.is_empty());
    assert!(e.selection.is_empty());
    assert_eq!(e.clipboard.len(), 3);
    assert_eq!(e.clipboard[0].start, Tick::ZERO);
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "cut 3 notes");
    assert_eq!(e.undo.undo_depth(), 1);
    drive_with(&mut e, &mut k, "p");
    assert_eq!(starts(&e), vec![0, G, 4 * G]);
    assert_eq!(pitches(&e), vec![60, 64, 67]);
    assert_eq!(ids(&e), vec![4, 5, 6], "pasted notes are fresh and selected");
    drive_with(&mut e, &mut k, "u");
    assert!(e.track().unwrap().notes.is_empty(), "the first undo takes back the paste");
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, song3(), "the second takes back the cut");
    // nothing selected: the note under the cursor; nothing there is an error
    let e = drive(song3(), &km(), "Cmd+x");
    assert_eq!(starts(&e), vec![G, 4 * G]);
    assert_eq!(e.clipboard.len(), 1);
    let mut e = Editor::new(song3());
    let err = try_drive_with(&mut e, &mut k, "Up Cmd+x").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "Cmd+x".into(), source: EditError::NothingSelected });
    assert!(e.clipboard.is_empty(), "a failed cut leaves the clipboard alone");
    assert_eq!(cmdline::parse("cut").unwrap(), Command::Cut);
}

#[test]
fn paste_count_repeats_back_to_back_and_replaces_overlaps() {
    let e = drive(song3(), &km(), "a c Cmd+Right 2 p");
    // clip spans 0..1200 (end of G4 at 960+240); second copy starts at 3840+1200
    assert_eq!(starts(&e), vec![0, G, 4 * G, 3840, 3840 + G, 3840 + 4 * G, 5040, 5040 + G, 5040 + 4 * G]);
    assert_eq!(e.selection.len(), 6);
    assert_eq!(e.undo.undo_depth(), 1, "one undo step for the whole paste");
    // pasting on top of the originals replaces them rather than doubling up
    let e = drive(song3(), &km(), "a c p");
    assert_eq!(e.track().unwrap().notes.len(), 3);
    assert_eq!(ids(&e), vec![4, 5, 6]);
}

#[test]
fn paste_empty_clipboard_is_an_error() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let err = try_drive_with(&mut e, &mut k, "p").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "p".into(), source: EditError::ClipboardEmpty });
}

#[test]
fn duplicate() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a d");
    assert_eq!(starts(&e), vec![0, G, 4 * G, 5 * G, 6 * G, 9 * G]);
    assert_eq!(ids(&e), vec![4, 5, 6]);
    assert_eq!(e.cursor.tick, Tick(10 * G));
    assert_eq!(last_status(&ev), "duplicated 3 notes");
    assert!(e.clipboard.is_empty(), "duplicate does not touch the clipboard");
    // duplicate again duplicates the new selection
    drive_with(&mut e, &mut k, "d");
    assert_eq!(e.track().unwrap().notes.len(), 9);
    // count: a single note under the cursor, three copies in a row
    let e = drive(song3(), &km(), "3 d");
    assert_eq!(starts(&e), vec![0, G, G, 2 * G, 3 * G, 4 * G]);
    assert_eq!(pitches(&e), vec![60, 60, 64, 60, 60, 67], "sorted by (start, pitch)");
    assert_eq!(e.cursor.tick, Tick(4 * G));
}

// ---------------------------------------------------------------- undo / redo

#[test]
fn undo_restores_song_cursor_and_selection() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let before = e.song.clone();
    drive_with(&mut e, &mut k, "Right Up Up Up Up s c Cmd+Right Shift+b");
    assert_eq!(ids(&e), vec![1, 2, 3]);
    let cursor = e.cursor;
    let selection = e.selection.clone();
    drive_with(&mut e, &mut k, "p");
    assert_ne!(e.cursor, cursor);
    assert_ne!(e.selection, selection);
    assert_ne!(e.song, before);
    let ev = drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, before);
    assert_eq!(e.cursor, cursor);
    assert_eq!(e.selection, selection);
    assert!(has_song_changed(&ev));
    assert_eq!(status(&ev), vec!["undo (0 left)"]);
    let err = try_drive_with(&mut e, &mut k, "u").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "u".into(), source: EditError::NothingToUndo });
}

#[test]
fn undo_leaves_the_header_width_alone() {
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "x");
    e.song.track_header_width = 333.0;
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song.track_header_width, 333.0);
    drive_with(&mut e, &mut k, "Shift+u");
    assert_eq!(e.song.track_header_width, 333.0);
}

#[test]
fn redo_reapplies() {
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "x");
    let after = e.song.clone();
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, song3());
    let ev = drive_with(&mut e, &mut k, "Shift+u");
    assert_eq!(e.song, after);
    assert_eq!(status(&ev), vec!["redo (0 left)"]);
    let err = try_drive_with(&mut e, &mut k, "Shift+u").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "Shift+u".into(), source: EditError::NothingToRedo });
    // a new edit after undo discards the redo history
    drive_with(&mut e, &mut k, "u n");
    assert!(!e.undo.can_redo());
}

#[test]
fn undo_count_and_every_edit_is_undoable() {
    let before = song3();
    let mut e = Editor::new(before.clone());
    let mut k = km();
    drive_with(&mut e, &mut k, "n Right n x a ] = c Cmd+Right p d Alt+Right Alt+Up Cmd+x");
    e.execute(&Command::TransposeSelected { semitones: 12 }, None).unwrap_err();
    drive_with(&mut e, &mut k, "p");
    e.execute(&Command::TransposeSelected { semitones: 12 }, None).unwrap();
    assert_ne!(e.song, before);
    let depth = e.undo.undo_depth();
    assert!(depth >= 10, "{depth}");
    drive_with(&mut e, &mut k, &counted(depth, "u"));
    assert_eq!(e.song, before);
    assert!(!e.undo.can_undo());
    drive_with(&mut e, &mut k, &counted(depth, "Shift+u"));
    assert!(!e.undo.can_redo());
    // a count larger than the history undoes what it can
    drive_with(&mut e, &mut k, "9 9 u");
    assert_eq!(e.song, before);
}

#[test]
fn non_edits_are_not_undoable() {
    let e = drive(song3(), &km(), "Right Up Up Up Up s Shift+Right Escape a Escape g t c Cmd+Right Space Shift+/");
    assert_eq!(e.undo.undo_depth(), 0);
}

#[test]
fn undo_cap_is_200() {
    let mut e = Editor::new(Song::new_empty());
    let mut k = km();
    for _ in 0..205 {
        drive_with(&mut e, &mut k, "n Right");
    }
    assert_eq!(e.undo.undo_depth(), 200);
}

// ---------------------------------------------------------------- view

#[test]
fn set_grid_and_cycle_grid() {
    let mut e = Editor::new(Song::new_empty());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "g");
    assert_eq!(e.grid, Grid::plain(Division::ThirtySecond));
    assert_eq!(status(&ev), vec!["grid 1/32"]);
    drive_with(&mut e, &mut k, "3 Shift+g");
    assert_eq!(e.grid, Grid::plain(Division::Quarter));
    let ev = e.execute(&Command::SetGrid { grid: "1/8T".parse().unwrap() }, None).unwrap();
    assert_eq!(e.grid, Grid { base: Division::Eighth, modifier: GridModifier::Triplet });
    assert_eq!(status(&ev), vec!["grid 1/8T"]);
    e.execute(&Command::CycleGrid { dir: CycleDir::Next }, None).unwrap();
    assert_eq!(e.grid.to_string(), "1/16T", "cycling keeps the modifier");
    // cursor moves by the new grid
    drive_with(&mut e, &mut k, "Right");
    assert_eq!(e.cursor.tick, Tick(160));
}

#[test]
fn next_and_prev_track() {
    let mut song = song3();
    song.add_track(Track::new("Bass"));
    song.add_track(Track::new("Drums"));
    let mut e = Editor::new(song);
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a t");
    assert_eq!(e.track_index, 1);
    assert!(e.selection.is_empty(), "switching tracks clears the selection");
    assert_eq!(last_status(&ev), "track 2/3: Bass");
    drive_with(&mut e, &mut k, "t t");
    assert_eq!(e.track_index, 0, "wraps");
    drive_with(&mut e, &mut k, "Shift+t");
    assert_eq!(e.track_index, 2);
    drive_with(&mut e, &mut k, "2 t");
    assert_eq!(e.track_index, 1);
    // edits land on the shown track
    drive_with(&mut e, &mut k, "n");
    assert_eq!(e.song.tracks[1].notes.len(), 1);
    assert_eq!(e.song.tracks[0].notes.len(), 3);
}

#[test]
fn direct_execute_matches_driving() {
    let mut a = Editor::new(song3());
    a.execute(&Command::MoveCursor { dir: Dir::Right, unit: Unit::Grid }, Some(3)).unwrap();
    a.execute(&Command::InsertNote, None).unwrap();
    let b = drive(song3(), &km(), "3 Right n");
    assert_eq!(a.song, b.song);
    assert_eq!(a.cursor, b.cursor);
}

// ---------------------------------------------------------------- select by unit

#[test]
fn select_bar() {
    let mut e = Editor::new(chords());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "b");
    assert_eq!(ids(&e), vec![1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(last_status(&ev), "selected 7 notes in bar 1");
    let ev = drive_with(&mut e, &mut k, "Cmd+Right b");
    assert_eq!(ids(&e), vec![8, 9]);
    assert_eq!(last_status(&ev), "selected 2 notes in bar 2");
    let ev = drive_with(&mut e, &mut k, "Cmd+Left 2 b");
    assert_eq!(e.selection.len(), 9);
    assert_eq!(last_status(&ev), "selected 9 notes in bars 1-2");
    // an empty bar is an error and keeps the selection
    drive_with(&mut e, &mut k, "Cmd+Right Cmd+Right a");
    let err = try_drive_with(&mut e, &mut k, "b").unwrap_err();
    no_match(err);
    assert_eq!(e.selection.len(), 9);
    assert_eq!(e.undo.undo_depth(), 0);
}

#[test]
fn select_chord() {
    let mut e = Editor::new(chords());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Shift+s");
    assert_eq!(ids(&e), vec![1, 2, 3]);
    assert_eq!(last_status(&ev), "selected 3 notes in the chord at 1:1:000");
    // no note under the cursor: the cursor tick itself
    let ev = drive_with(&mut e, &mut k, "Ctrl+Right Shift+s");
    assert_eq!(ids(&e), vec![4]);
    assert_eq!(last_status(&ev), "selected 1 note in the chord at 1:2:000");
    // an empty tick is an error and keeps the selection
    drive_with(&mut e, &mut k, "Right Shift+b");
    assert_eq!(ids(&e), vec![1, 2, 3, 4]);
    let err = try_drive_with(&mut e, &mut k, "Shift+s").unwrap_err();
    no_match(err);
    assert_eq!(ids(&e), vec![1, 2, 3, 4]);
}

#[test]
fn select_next_notes_to_end_and_to_start() {
    let mut e = Editor::new(chords());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "w");
    assert_eq!(ids(&e), vec![1, 2, 3, 4], "default is the next four");
    assert_eq!(last_status(&ev), "selected 4 notes from the cursor on");
    drive_with(&mut e, &mut k, "Ctrl+Right 2 w");
    assert_eq!(ids(&e), vec![4, 5]);
    // nothing after bar 3 is an error and keeps the selection
    drive_with(&mut e, &mut k, "Cmd+Right Cmd+Right Shift+b");
    assert_eq!(e.selection.len(), 9);
    let err = try_drive_with(&mut e, &mut k, "w").unwrap_err();
    no_match(err);
    assert_eq!(e.selection.len(), 9);
    let e = drive(chords(), &km(), "Ctrl+Right Ctrl+Right Shift+e");
    assert_eq!(ids(&e), vec![5, 6, 7, 8, 9]);
    let mut e = Editor::new(chords());
    let ev = drive_with(&mut e, &mut k, "Ctrl+Right Shift+b");
    assert_eq!(ids(&e), vec![1, 2, 3, 4], "to-start includes the note at the cursor tick");
    assert_eq!(last_status(&ev), "selected 4 notes from the start to the cursor");
}

// ---------------------------------------------------------------- select by property

#[test]
fn select_same_pitch_and_velocity() {
    let mut e = Editor::new(chords());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Shift+p");
    assert_eq!(ids(&e), vec![1]);
    assert_eq!(last_status(&ev), "selected 1 note with pitch C4");
    drive_with(&mut e, &mut k, "Up Up Up Up Shift+p");
    assert_eq!(ids(&e), vec![2]);
    no_match(try_drive_with(&mut e, &mut k, "Down Shift+p").unwrap_err());
    let ev = e.execute(&Command::SelectVelocityAbove { value: 100 }, None).unwrap();
    assert_eq!(ids(&e), vec![5, 9]);
    assert_eq!(last_status(&ev), "selected 2 notes with velocity above 100");
    e.execute(&Command::SelectVelocityBelow { value: 60 }, None).unwrap();
    assert_eq!(ids(&e), vec![7]);
    assert!(matches!(e.execute(&Command::SelectVelocityBelow { value: 50 }, None), Err(EditError::NoMatch(_))));
    assert_eq!(ids(&e), vec![7]);
}

#[test]
fn select_top_and_bottom_of_chords() {
    let mut e = Editor::new(chords());
    let ev = e.execute(&Command::SelectTopOfChords, None).unwrap();
    assert_eq!(ids(&e), vec![3, 6], "only real chords (2+ notes) count");
    assert_eq!(last_status(&ev), "selected 2 notes at the top of chords");
    e.execute(&Command::SelectBottomOfChords, None).unwrap();
    assert_eq!(ids(&e), vec![1, 5]);
    let mut e = Editor::new(song3());
    assert!(matches!(e.execute(&Command::SelectTopOfChords, None), Err(EditError::NoMatch(_))));
}

#[test]
fn invert_selection() {
    let mut e = Editor::new(chords());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a i");
    assert!(e.selection.is_empty(), "inverting everything may leave nothing");
    assert_eq!(last_status(&ev), "inverted: 0 notes selected");
    drive_with(&mut e, &mut k, "i");
    assert_eq!(e.selection.len(), 9);
    drive_with(&mut e, &mut k, "Escape s i");
    assert_eq!(ids(&e), vec![2, 3, 4, 5, 6, 7, 8, 9]);
    // inverting a box selection ends the box
    let e = drive(chords(), &km(), "Shift+Right i");
    assert_eq!(e.selection.len(), 8);
    assert_eq!(e.box_anchor, None);
}

// ---------------------------------------------------------------- focus

#[test]
fn tab_steps_through_the_selection_and_drops_the_focused_note() {
    let mut e = Editor::new(chords());
    let mut k = km();
    drive_with(&mut e, &mut k, "b");
    let ev = drive_with(&mut e, &mut k, "Tab");
    assert_eq!(e.focus, Some(NoteId(1)));
    assert_eq!(e.cursor.tick, Tick::ZERO);
    assert_eq!(e.cursor.pitch, 60);
    assert_eq!(auditioned(&ev), 1);
    assert_eq!(last_status(&ev), "focus 1/7: C4 at 1:1:000");
    let ev = drive_with(&mut e, &mut k, "Tab Tab");
    assert_eq!(e.focus, Some(NoteId(3)));
    assert_eq!(e.cursor.pitch, 67);
    assert_eq!(last_status(&ev), "focus 3/7: G4 at 1:1:000");
    drive_with(&mut e, &mut k, "Shift+Tab");
    assert_eq!(e.focus, Some(NoteId(2)));
    drive_with(&mut e, &mut k, "2 Tab");
    assert_eq!(e.focus, Some(NoteId(4)));
    assert_eq!(e.cursor.tick, Tick(960));
    assert_eq!(e.cursor.pitch, 62);
    let ev = drive_with(&mut e, &mut k, "Shift+x");
    assert_eq!(ids(&e), vec![1, 2, 3, 5, 6, 7]);
    assert_eq!(e.focus, Some(NoteId(5)), "focus advances to the next note");
    assert_eq!(e.cursor.tick, Tick(1920));
    assert_eq!(e.cursor.pitch, 65);
    assert_eq!(status(&ev), vec!["dropped D4; 6 notes selected", "focus 4/6: F4 at 1:3:000"]);
    assert_eq!(e.undo.undo_depth(), 0, "focus and drop are not edits");
    // wrapping both ways
    let e = drive(chords(), &km(), "a Shift+Tab");
    assert_eq!(e.focus, Some(NoteId(9)), "Shift+Tab with no focus starts at the end");
    let e = drive(chords(), &km(), "a Tab Shift+Tab");
    assert_eq!(e.focus, Some(NoteId(9)));
    let e = drive(chords(), &km(), "a 9 Tab");
    assert_eq!(e.focus, Some(NoteId(9)));
    let e = drive(chords(), &km(), "a 1 0 Tab");
    assert_eq!(e.focus, Some(NoteId(1)));
    // dropping the last selected note
    let mut e = Editor::new(chords());
    let ev = drive_with(&mut e, &mut k, "s Tab Shift+x");
    assert!(e.selection.is_empty());
    assert_eq!(e.focus, None);
    assert_eq!(last_status(&ev), "dropped C4; 0 notes selected");
    // focus works on a box selection and ends the box
    let e = drive(chords(), &km(), "7 Shift+Up Tab Tab Shift+x");
    assert_eq!(ids(&e), vec![1, 3]);
    assert_eq!(e.focus, Some(NoteId(3)));
    assert_eq!(e.box_anchor, None);
}

#[test]
fn focus_errors_and_clears_when_the_selection_changes() {
    let mut e = Editor::new(chords());
    let mut k = km();
    let err = try_drive_with(&mut e, &mut k, "Tab").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "Tab".into(), source: EditError::NothingSelected });
    let err = try_drive_with(&mut e, &mut k, "a Shift+x").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "Shift+x".into(), source: EditError::NothingFocused });
    drive_with(&mut e, &mut k, "Tab Right");
    assert_eq!(e.focus, None, "a cursor move clears the selection, and the focus with it");
    drive_with(&mut e, &mut k, "Left a Tab s");
    assert_eq!(e.focus, None, "toggling a note changes the selection: focus cleared");
    drive_with(&mut e, &mut k, "a Tab b");
    assert_eq!(e.focus, None);
    drive_with(&mut e, &mut k, "Tab Escape");
    assert_eq!(e.focus, None);
    // undo restores a selection; the focus does not come back
    drive_with(&mut e, &mut k, "a Tab x");
    assert_eq!(e.focus, None);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.selection.len(), 9);
    assert_eq!(e.focus, None);
}

// ---------------------------------------------------------------- filters

#[test]
fn keep_filters_thin_the_selection() {
    let k = km();
    let mut e = Editor::new(chords());
    let mut km2 = km();
    let ev = drive_with(&mut e, &mut km2, "a f t");
    assert_eq!(ids(&e), vec![3, 4, 6, 7, 8, 9], "a lone note is the top of its own chord");
    assert_eq!(last_status(&ev), "kept 6 of 9 notes at the top of their chord");
    assert_eq!(ids(&drive(chords(), &k, "a f b")), vec![1, 4, 5, 7, 8, 9]);
    let mut e = Editor::new(chords());
    let ev = drive_with(&mut e, &mut km2, "a f e");
    assert_eq!(ids(&e), vec![1, 3, 5, 7, 9]);
    assert_eq!(last_status(&ev), "kept 5 of 9 notes (every 2nd)");
    assert_eq!(ids(&drive(chords(), &k, "a 3 f e")), vec![1, 4, 7], "count = keep every Nth");
    assert_eq!(ids(&drive(chords(), &k, "a f o")), vec![1, 2, 3, 4, 5, 6, 8]);
    assert_eq!(ids(&drive(chords(), &k, "a f Shift+o")), vec![7, 9]);
    assert_eq!(ids(&drive(chords(), &k, "a f a")), vec![2, 3, 4, 5, 6, 7, 8, 9]);
    assert_eq!(ids(&drive(chords(), &k, "Cmd+Up a f l")), vec![1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(ids(&drive(chords(), &k, "Ctrl+Right a f Shift+a")), vec![4, 5, 6, 7, 8, 9]);
    assert_eq!(ids(&drive(chords(), &k, "Ctrl+Right a f Shift+b")), vec![1, 2, 3]);
    // filters chain, work on a box selection, and are not edits
    let e = drive(chords(), &k, "Cmd+Shift+Right Cmd+Shift+Right Cmd+Shift+Up Cmd+Shift+Up f o f t");
    assert_eq!(ids(&e), vec![3, 4, 6, 8]);
    assert_eq!(e.undo.undo_depth(), 0);
    assert_eq!(e.box_anchor, None);
    // a filter that would keep nothing is an error and keeps the selection
    let mut e = Editor::new(chords());
    drive_with(&mut e, &mut km2, "a");
    let err = try_drive_with(&mut e, &mut km2, "f l").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "l".into(), source: EditError::NoMatch("no selected notes below the cursor".into()) });
    assert_eq!(e.selection.len(), 9);
    // with nothing selected they act on the note under the cursor
    let e = drive(chords(), &k, "f t");
    assert_eq!(ids(&e), vec![1]);
    let err = try_drive_with(&mut Editor::new(chords()), &mut km(), "Right f t").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "t".into(), source: EditError::NothingSelected });
}

// ---------------------------------------------------------------- velocity shaping

#[test]
fn ramp_velocity_over_the_selection() {
    let mut e = Editor::new(chords());
    let mut k = km();
    let before = e.song.clone();
    drive_with(&mut e, &mut k, "a");
    let ev = e.execute(&Command::RampVelocity { from: 40, to: 120 }, None).unwrap();
    assert_eq!(velocities(&e), vec![40, 40, 40, 56, 72, 72, 88, 104, 120]);
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "velocity ramp 40 → 120 on 9 notes");
    assert_eq!(e.undo.undo_depth(), 1);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, before);
    // via the command line, on the note under the cursor, a ramp of one tick is `from`
    let cmd = cmdline::parse("vel 60..110").unwrap();
    e.execute(&Command::ClearSelection, None).unwrap();
    e.execute(&cmd, None).unwrap();
    assert_eq!(velocities(&e)[0], 60);
    // ramping to the values already there is a no-op without an undo step
    let depth = e.undo.undo_depth();
    let ev = e.execute(&Command::RampVelocity { from: 60, to: 60 }, None).unwrap();
    assert!(!has_song_changed(&ev));
    assert_eq!(e.undo.undo_depth(), depth);
}

#[test]
fn humanize_is_deterministic_and_undoable() {
    let before = chords();
    let mut e = Editor::new(before.clone());
    let mut k = km();
    let seed = e.rng_seed;
    let ev = drive_with(&mut e, &mut k, "a h");
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "humanized 9 notes (velocity ±8, timing ±10 ticks)");
    assert_ne!(e.song, before);
    assert_ne!(e.rng_seed, seed, "the generator advances");
    for n in &e.track().unwrap().notes {
        let orig = before.tracks[0].note(n.id).unwrap();
        assert!(n.velocity.abs_diff(orig.velocity) <= 8, "{n:?} vs {orig:?}");
        assert!(n.start.get().abs_diff(orig.start.get()) <= 10, "{n:?} vs {orig:?}");
    }
    let other = drive(before.clone(), &km(), "a h");
    assert_eq!(e.song, other.song, "same seed, same keys, same result");
    drive_with(&mut e, &mut k, "h");
    assert_ne!(e.song, other.song, "a second call draws new values");
    assert_eq!(e.undo.undo_depth(), 2, "humanize does not coalesce");
    drive_with(&mut e, &mut k, "u u");
    assert_eq!(e.song, before);
    // zero amounts do nothing and take no undo step
    let mut e = Editor::new(before.clone());
    e.execute(&Command::SelectAll, None).unwrap();
    let ev = e.execute(&Command::Humanize { velocity: 0, timing: 0 }, None).unwrap();
    assert!(!has_song_changed(&ev));
    assert_eq!(e.undo.undo_depth(), 0);
    // a note at tick 0 never goes before it
    let mut s = Song::new_empty();
    let n = s.new_note(Tick(0), Tick(G), 60, 100);
    s.tracks[0].insert_note(n);
    let e = drive(s, &km(), "a h h h h h h h h");
    assert!(e.track().unwrap().notes[0].start <= Tick(80));
}

// ---------------------------------------------------------------- quantize and timing

#[test]
fn quantize_starts() {
    let mut e = Editor::new(chords());
    let mut k = km();
    let before = e.song.clone();
    let ev = drive_with(&mut e, &mut k, "a q");
    assert_eq!(starts(&e)[6], 2880, "the off-grid B4 snaps to the 1/16 grid");
    assert_eq!(last_status(&ev), "quantized 1 of 9 notes to 1/16");
    assert!(has_song_changed(&ev));
    let ev = drive_with(&mut e, &mut k, "q");
    assert_eq!(last_status(&ev), "already on the 1/16 grid");
    assert!(!has_song_changed(&ev));
    assert_eq!(e.undo.undo_depth(), 1);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, before);
    // strength moves part of the way; an explicit grid overrides the editor's
    let mut e = Editor::new(chords());
    drive_with(&mut e, &mut k, "a");
    let ev = e.execute(&Command::Quantize { grid: None, strength: 50, swing: 0 }, None).unwrap();
    assert_eq!(starts(&e)[6], 2890);
    assert_eq!(last_status(&ev), "quantized 1 of 9 notes to 1/16 at 50%");
    let mut e = Editor::new(chords());
    drive_with(&mut e, &mut k, "a");
    e.execute(&Command::Quantize { grid: Some(Grid::plain(Division::Quarter)), strength: 100, swing: 0 }, None).unwrap();
    assert_eq!(starts(&e), vec![0, 0, 0, 960, 1920, 1920, 2880, 3840, 3840]);
    assert_eq!(e.grid, Grid::default(), "the editor grid is untouched");
}

#[test]
fn quantize_uses_the_editor_swing_when_its_own_is_zero() {
    let mut s = Song::new_empty();
    let n = s.new_note(Tick(500), Tick(G), 60, 100);
    s.tracks[0].insert_note(n);
    let mut e = Editor::new(s.clone());
    let mut k = km();
    drive_with(&mut e, &mut k, "a Shift+g q");
    assert_eq!(e.grid, Grid::plain(Division::Eighth));
    assert_eq!(starts(&e), vec![480], "straight");
    drive_with(&mut e, &mut k, "u");
    let ev = e.execute(&Command::SetSwing { percent: 100 }, None).unwrap();
    assert_eq!(e.swing, 100);
    assert_eq!(status(&ev), vec!["swing 100%"]);
    assert_eq!(e.undo.undo_depth(), 0, "a setting, not an edit");
    let ev = drive_with(&mut e, &mut k, "q");
    assert_eq!(starts(&e), vec![720], "swung off-step");
    assert_eq!(last_status(&ev), "quantized 1 of 1 note to 1/8 with 100% swing");
    // an explicit swing wins over the setting; percentages clamp to 100
    let mut e = Editor::new(s);
    e.execute(&Command::SelectAll, None).unwrap();
    e.execute(&Command::SetSwing { percent: 250 }, None).unwrap();
    assert_eq!(e.swing, 100);
    e.execute(&Command::Quantize { grid: Some(Grid::plain(Division::Eighth)), strength: 100, swing: 1 }, None).unwrap();
    assert_eq!(starts(&e), vec![482], "1% swing: the off-step is delayed by 1% of half a step");
}

#[test]
fn quantize_length_rounds_durations_to_the_grid() {
    let mut s = Song::new_empty();
    for (start, dur) in [(0, 300), (960, 100), (1920, 480)] {
        let n = s.new_note(Tick(start), Tick(dur), 60, 100);
        s.tracks[0].insert_note(n);
    }
    let mut e = Editor::new(s.clone());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a Shift+q");
    assert_eq!(durations(&e), vec![240, 240, 480], "never below one step");
    assert_eq!(last_status(&ev), "rounded 2 of 3 note lengths to 1/16");
    assert!(has_song_changed(&ev));
    let ev = drive_with(&mut e, &mut k, "Shift+q");
    assert_eq!(last_status(&ev), "lengths already on the 1/16 grid");
    assert_eq!(e.undo.undo_depth(), 1);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, s);
}

#[test]
fn nudge_selected_moves_off_grid_and_coalesces() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let before = e.song.clone();
    let ev = drive_with(&mut e, &mut k, "Alt+.");
    assert_eq!(starts(&e), vec![10, G, 4 * G]);
    assert_eq!(e.cursor.tick, Tick::ZERO, "the cursor stays on the grid");
    assert_eq!(auditioned(&ev), 1);
    assert_eq!(last_status(&ev), "nudged 1 note +10 ticks");
    let ev = drive_with(&mut e, &mut k, "Right Up Up Up Up Alt+.");
    assert_eq!(starts(&e), vec![10, G + 10, 4 * G]);
    assert_eq!(auditioned(&ev), 1);
    // count multiplies and the group clamps at tick 0
    let ev = drive_with(&mut e, &mut k, "a 3 Alt+,");
    assert_eq!(starts(&e), vec![0, G, 4 * G - 10]);
    assert_eq!(last_status(&ev), "nudged 3 notes -10 ticks");
    let err = try_drive_with(&mut e, &mut k, "Alt+,").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }));
    assert_eq!(e.undo.undo_depth(), 3, "cursor moves and selection changes break coalescing");
    drive_with(&mut e, &mut k, "3 u");
    assert_eq!(e.song, before);
    // repeated nudges on the same selection are one undo step
    let mut e = Editor::new(song3());
    drive_with(&mut e, &mut k, "a Alt+. Alt+. Alt+,");
    assert_eq!(starts(&e), vec![10, G + 10, 4 * G + 10]);
    assert_eq!(e.undo.undo_depth(), 1);
}

#[test]
fn set_length() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "l");
    assert_eq!(durations(&e)[0], G, "already one step: no change");
    assert!(!has_song_changed(&ev));
    assert_eq!(e.undo.undo_depth(), 0);
    let ev = drive_with(&mut e, &mut k, "4 l");
    assert_eq!(durations(&e)[0], 4 * G);
    assert_eq!(last_status(&ev), "length 4 grid steps on 1 note");
    drive_with(&mut e, &mut k, "a 2 l");
    assert_eq!(durations(&e), vec![2 * G, 2 * G, 2 * G]);
    assert_eq!(e.undo.undo_depth(), 2);
    drive_with(&mut e, &mut k, "u u");
    assert_eq!(e.song, song3());
    // steps < 1 are treated as 1
    e.execute(&Command::SetLength { steps: 0 }, None).unwrap();
    assert_eq!(durations(&e)[0], G);
}

#[test]
fn legato_extends_to_the_next_note() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a Shift+l");
    assert_eq!(durations(&e), vec![G, 3 * G, G], "C4 already touches E4; G4 has nothing after it");
    assert_eq!(last_status(&ev), "legato: extended 1 note");
    assert!(has_song_changed(&ev));
    let ev = drive_with(&mut e, &mut k, "Shift+l");
    assert_eq!(last_status(&ev), "nothing to extend");
    assert_eq!(e.undo.undo_depth(), 1);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, song3());
    // a chord extends every note to the next start
    let e = drive(chords(), &km(), "a Shift+l");
    assert_eq!(durations(&e), vec![960, 960, 960, 960, 980, 980, 940, 240, 240]);
}

// ---------------------------------------------------------------- chords

#[test]
fn insert_chord() {
    let mut e = Editor::new(Song::new_empty());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Shift+n");
    assert_eq!(pitches(&e), vec![60, 64, 67]);
    assert_eq!(durations(&e), vec![G, G, G]);
    assert_eq!(ids(&e), vec![1, 2, 3], "the chord becomes the selection");
    assert_eq!(auditioned(&ev), 3);
    assert_eq!(status(&ev), vec!["inserted chord C4 E4 G4 (1/16)"]);
    drive_with(&mut e, &mut k, "Right 2 Shift+n");
    assert_eq!(durations(&e)[3..], [2 * G, 2 * G, 2 * G]);
    assert_eq!(e.undo.undo_depth(), 2);
    drive_with(&mut e, &mut k, "u u");
    assert!(e.track().unwrap().notes.is_empty());
    // inserting on top replaces notes with the same start and pitch
    let e = drive(song3(), &km(), "Shift+n");
    assert_eq!(pitches(&e), vec![60, 64, 67, 64, 67]);
    // notes that would leave the pitch range are skipped; none left is an error
    let mut e = Editor::new(Song::new_empty());
    e.cursor.pitch = 125;
    e.execute(&Command::InsertChord { intervals: vec![0, 4, 7] }, None).unwrap();
    assert_eq!(pitches(&e), vec![125]);
    let err = e.execute(&Command::InsertChord { intervals: vec![4, 7] }, None).unwrap_err();
    assert!(matches!(err, EditError::OutOfRange(_)));
    assert_eq!(e.undo.undo_depth(), 1);
    // minor chord from the command line, duplicates collapse
    let cmd = cmdline::parse("chord 0,3,7,12,12").unwrap();
    let mut e = Editor::new(Song::new_empty());
    e.execute(&cmd, None).unwrap();
    assert_eq!(pitches(&e), vec![60, 63, 67, 72]);
}

// ---------------------------------------------------------------- transport, view, command line

#[test]
fn transport_commands_only_emit_events() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Right Space");
    assert_eq!(ev, vec![EditorEvent::Transport(TransportRequest::Toggle)]);
    let ev = drive_with(&mut e, &mut k, "Enter");
    assert_eq!(ev, vec![EditorEvent::Transport(TransportRequest::Seek { to: Tick::ZERO })]);
    assert_eq!(e.cursor.tick, Tick(G));
    let ev = e.execute(&Command::Play, None).unwrap();
    assert_eq!(ev, vec![EditorEvent::Transport(TransportRequest::Play { from: Tick::ZERO })]);
    let ev = drive_with(&mut e, &mut k, "Shift+Enter");
    assert_eq!(ev, vec![EditorEvent::Transport(TransportRequest::Seek { to: Tick(G) })]);
    let ev = drive_with(&mut e, &mut k, ", 3 .");
    assert_eq!(
        ev,
        vec![
            EditorEvent::Transport(TransportRequest::StepBars { bars: -1 }),
            EditorEvent::Transport(TransportRequest::StepBars { bars: 3 }),
        ]
    );
    let ev = drive_with(&mut e, &mut k, "Alt+Enter");
    assert_eq!(ev, vec![EditorEvent::Transport(TransportRequest::CursorToPlayhead)]);
    assert_eq!(e.cursor.tick, Tick(G));
    let ev = e.execute(&Command::Stop, Some(5)).unwrap();
    assert_eq!(ev, vec![EditorEvent::Transport(TransportRequest::Stop)]);
    assert_eq!(e.song, song3());
    assert_eq!(e.undo.undo_depth(), 0);
    // transport keys end a box in progress but keep what it selected
    let ev = drive_with(&mut e, &mut k, "Shift+Left Space");
    assert!(ev.contains(&EditorEvent::Transport(TransportRequest::Toggle)));
    assert_eq!(e.box_anchor, None);
    assert_eq!(ids(&e), vec![1]);
}

#[test]
fn cursor_to_grid_line_at_snaps_down() {
    let mut e = Editor::new(song3());
    let pitch = e.cursor.pitch;
    e.cursor_to_grid_line_at(Tick(3 * G + G - 1));
    assert_eq!(e.cursor, Cursor { tick: Tick(3 * G), pitch });
    e.cursor_to_grid_line_at(Tick(5 * G));
    assert_eq!(e.cursor.tick, Tick(5 * G), "already on a line: stays");
    e.execute(&cmdline::parse("grid 1/4").unwrap(), None).unwrap();
    e.cursor_to_grid_line_at(Tick(7 * G));
    assert_eq!(e.cursor.tick, Tick(4 * G), "follows the grid size");
}

#[test]
fn cursor_to_start_keeps_the_pitch_and_clears_the_selection() {
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "Cmd+Right Right Up a");
    e.execute(&Command::CursorToStart, None).unwrap();
    assert_eq!(e.cursor.tick, Tick::ZERO);
    assert_eq!(e.cursor.pitch, Cursor::default().pitch + 1);
    assert!(e.selection.is_empty());
    assert_eq!(e.song, song3());
}

#[test]
fn view_and_app_commands_only_emit_events() {
    let mut e = Editor::new(song3());
    let mut k = km();
    assert_eq!(drive_with(&mut e, &mut k, "Shift+/"), vec![EditorEvent::ToggleCheatSheet]);
    assert_eq!(drive_with(&mut e, &mut k, "Shift+Right Shift+/")[1..], [EditorEvent::ToggleCheatSheet]);
    drive_with(&mut e, &mut k, "Escape");
    assert_eq!(drive_with(&mut e, &mut k, "Cmd+="), vec![EditorEvent::View(ViewRequest::ZoomTime(ZOOM_TIME_STEP))]);
    assert_eq!(
        drive_with(&mut e, &mut k, "2 Cmd+-"),
        vec![EditorEvent::View(ViewRequest::ZoomTime(1.0 / ZOOM_TIME_STEP.powi(2)))],
        "count = that many steps"
    );
    assert_eq!(drive_with(&mut e, &mut k, "Cmd+Shift+="), vec![EditorEvent::View(ViewRequest::ZoomRows(ZOOM_ROWS_STEP))]);
    assert_eq!(drive_with(&mut e, &mut k, "Cmd+Shift+-"), vec![EditorEvent::View(ViewRequest::ZoomRows(1.0 / ZOOM_ROWS_STEP))]);
    assert_eq!(drive_with(&mut e, &mut k, "Cmd+e"), vec![EditorEvent::View(ViewRequest::TogglePianoRoll)]);
    assert_eq!(drive_with(&mut e, &mut k, "Cmd+0"), vec![EditorEvent::View(ViewRequest::FitTracksToSong)]);
    let ev = drive_with(&mut e, &mut k, "Shift+;");
    assert_eq!(ev, vec![EditorEvent::OpenCommandLine]);
    assert_eq!(e.undo.undo_depth(), 0);
}

#[test]
fn command_line_round_trip_through_the_editor() {
    let mut e = Editor::new(chords());
    for line in ["sel all", "q 1/4", "vel 60..100", "keep top", "t 12", "sel none"] {
        let cmd = cmdline::parse(line).unwrap_or_else(|err| panic!("{line}: {err}"));
        e.execute(&cmd, None).unwrap_or_else(|err| panic!("{line}: {err}"));
    }
    assert_eq!(starts(&e), vec![0, 0, 0, 960, 1920, 1920, 2880, 3840, 3840]);
    assert_eq!(velocities(&e), vec![60, 60, 60, 70, 80, 80, 90, 100, 100], "five distinct ticks after quantizing");
    assert_eq!(pitches(&e), vec![60, 64, 79, 74, 65, 81, 83, 72, 88], "only the chord tops went up an octave");
    assert!(e.selection.is_empty());
    assert_eq!(e.undo.undo_depth(), 3);
}

// ---------------------------------------------------------------- tracks and mixer

/// song3 plus "Bass" and "Drums" tracks (each with one note so they are distinguishable).
fn three_tracks() -> Song {
    let mut song = song3();
    for (name, pitch) in [("Bass", 40), ("Drums", 36)] {
        let n = song.new_note(Tick(0), Tick(G), pitch, 100);
        let i = song.add_track(Track::new(name));
        song.tracks[i].insert_note(n);
    }
    song
}

fn track_names(e: &Editor) -> Vec<&str> {
    e.song.tracks.iter().map(|t| t.name.as_str()).collect()
}

fn volume_percent(e: &Editor) -> i32 {
    (e.track().unwrap().volume * 100.0).round() as i32
}

fn pan_percent(e: &Editor) -> i32 {
    (e.track().unwrap().pan * 100.0).round() as i32
}

fn preset(e: &Editor) -> u16 {
    let Instrument::SoundFont { preset, .. } = &e.track().unwrap().instrument;
    *preset
}

fn sustain(e: &Editor) -> Vec<(u64, i32)> {
    e.track().unwrap().lanes.sustain.iter().map(|p| (p.tick.get(), p.value)).collect()
}

fn set_sustain(e: &mut Editor, points: &[(u64, i32)]) {
    for (tick, value) in points {
        e.song.tracks[e.track_index].lanes.set_point(LaneKind::Sustain, Tick(*tick), *value);
    }
}

#[test]
fn show_track_switches_and_clears_selection_without_undo() {
    let mut e = Editor::new(three_tracks());
    drive_with(&mut e, &mut km(), "Cmd+a");
    assert!(!e.selection.is_empty());
    e.box_anchor = Some(e.cursor);
    e.show_track(1);
    assert_eq!(e.track_index, 1);
    assert!(e.selection.is_empty());
    assert!(e.box_anchor.is_none());
    assert_eq!(e.undo.undo_depth(), 0);
}

#[test]
fn add_track_inserts_after_the_current_one_and_shows_it() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a Cmd+t");
    assert_eq!(track_names(&e), vec!["Track 1", "Track 2"]);
    assert_eq!(e.track_index, 1);
    assert!(e.selection.is_empty(), "showing a new track drops the selection");
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "added track 2/2: Track 2");
    assert_eq!(e.cursor, Editor::new(song3()).cursor, "cursor stays");
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, song3());
    assert_eq!(e.track_index, 0, "undo shows the track that was shown");
    // count = that many; they go right after the current track, not at the end
    let e = drive(three_tracks(), &km(), "t 2 Cmd+t");
    assert_eq!(track_names(&e), vec!["Track 1", "Bass", "Track 4", "Track 5", "Drums"]);
    assert_eq!(e.track_index, 3);
    assert_eq!(e.undo.undo_depth(), 1, "one undo step for the whole count");
}

#[test]
fn remove_track_refuses_the_last_one_and_undo_brings_it_back() {
    let mut e = Editor::new(three_tracks());
    let mut k = km();
    let before = e.song.clone();
    let ev = drive_with(&mut e, &mut k, "Shift+t a Cmd+Shift+t");
    assert_eq!(track_names(&e), vec!["Track 1", "Bass"]);
    assert_eq!(e.track_index, 1, "index clamps to the new last track");
    assert!(e.selection.is_empty());
    assert_eq!(last_status(&ev), "removed track Drums; showing 2/2: Bass");
    drive_with(&mut e, &mut k, "Shift+t Cmd+Shift+t");
    assert_eq!(track_names(&e), vec!["Bass"]);
    assert_eq!(e.track_index, 0);
    let err = try_drive_with(&mut e, &mut k, "Cmd+Shift+t").unwrap_err();
    assert_eq!(err, DriveError::Edit { key: "Shift+Cmd+t".into(), source: EditError::LastTrack });
    assert_eq!(track_names(&e), vec!["Bass"]);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(track_names(&e), vec!["Track 1", "Bass"]);
    assert_eq!(e.track_index, 0, "Track 1 was shown when it was removed");
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, before);
    assert_eq!(e.track_index, 2, "Drums was shown when it was removed");
}

#[test]
fn duplicate_track_copies_notes_with_fresh_ids() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "a Cmd+d");
    assert_eq!(track_names(&e), vec!["Track 1", "Track 1 copy"]);
    assert_eq!(e.track_index, 1);
    assert!(e.selection.is_empty());
    assert_eq!(last_status(&ev), "duplicated track Track 1; showing 2/2: Track 1 copy");
    let copy = &e.song.tracks[1];
    assert_eq!(copy.notes.len(), 3);
    let copy_ids: Vec<u64> = copy.notes.iter().map(|n| n.id.0).collect();
    assert_eq!(copy_ids, vec![4, 5, 6]);
    assert_eq!(starts(&e), vec![0, G, 4 * G]);
    assert_eq!(e.song.next_note_id, 7);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, song3());
    assert_eq!(e.track_index, 0);
    let e = drive(three_tracks(), &km(), "t 2 Cmd+d");
    assert_eq!(track_names(&e), vec!["Track 1", "Bass", "Bass copy", "Bass copy", "Drums"]);
    assert_eq!(e.track_index, 3);
}

#[test]
fn move_track_up_and_down_follows_the_track() {
    let mut e = Editor::new(three_tracks());
    let mut k = km();
    let err = try_drive_with(&mut e, &mut k, "Cmd+Alt+Up").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }), "{err:?}");
    assert_eq!(e.undo.undo_depth(), 0);
    let ev = drive_with(&mut e, &mut k, "a Cmd+Alt+Down");
    assert_eq!(track_names(&e), vec!["Bass", "Track 1", "Drums"]);
    assert_eq!(e.track_index, 1, "the view follows the track");
    assert_eq!(ids(&e), vec![1, 2, 3], "same notes, selection kept");
    assert_eq!(last_status(&ev), "moved track down to 2/3: Track 1");
    drive_with(&mut e, &mut k, "5 Cmd+Alt+Down");
    assert_eq!(track_names(&e), vec!["Bass", "Drums", "Track 1"], "stops at the bottom");
    assert_eq!(e.track_index, 2);
    let err = try_drive_with(&mut e, &mut k, "Cmd+Alt+Down").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }));
    drive_with(&mut e, &mut k, "Cmd+Alt+Up");
    assert_eq!(track_names(&e), vec!["Bass", "Track 1", "Drums"]);
    assert_eq!(e.track_index, 1);
    drive_with(&mut e, &mut k, "3 u");
    assert_eq!(e.song, three_tracks());
    assert_eq!(e.track_index, 0);
}

#[test]
fn rename_track_takes_the_rest_of_the_command_line() {
    let cmd = cmdline::parse("rename My   Bass").unwrap();
    assert_eq!(cmd, Command::RenameTrack { name: "My Bass".to_string() });
    let mut e = Editor::new(song3());
    let ev = e.execute(&cmd, None).unwrap();
    assert_eq!(e.track().unwrap().name, "My Bass");
    assert!(has_song_changed(&ev));
    assert_eq!(status(&ev), vec!["renamed track to My Bass"]);
    // the same name again is not a new undo step
    e.execute(&cmd, None).unwrap();
    assert_eq!(e.undo.undo_depth(), 1);
    assert_eq!(
        e.execute(&Command::RenameTrack { name: "   ".to_string() }, None),
        Err(EditError::BadValue("a track name cannot be empty".to_string()))
    );
    e.execute(&Command::Undo, None).unwrap();
    assert_eq!(e.song, song3());
    assert!(matches!(cmdline::parse("rename"), Err(crate::CmdLineError::MissingArg { .. })));
}

#[test]
fn track_volume_set_nudge_clamp_and_coalesce() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Ctrl+=");
    assert_eq!(volume_percent(&e), 105);
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "volume 105% on 1/1: Track 1");
    drive_with(&mut e, &mut k, "3 Ctrl+-");
    assert_eq!(volume_percent(&e), 90);
    assert_eq!(e.undo.undo_depth(), 1, "nudges coalesce");
    drive_with(&mut e, &mut k, "u");
    assert_eq!(volume_percent(&e), 100);
    assert_eq!(e.song, song3());
    // set exact, with a count override and clamping at 200
    e.execute(&cmdline::parse("vol 80%").unwrap(), None).unwrap();
    assert_eq!(volume_percent(&e), 80);
    e.execute(&Command::SetTrackVolume { percent: 80 }, Some(50)).unwrap();
    assert_eq!(volume_percent(&e), 50);
    e.execute(&Command::SetTrackVolume { percent: 200 }, Some(999)).unwrap();
    assert_eq!(volume_percent(&e), 200);
    let err = try_drive_with(&mut e, &mut k, "Ctrl+=").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }));
    drive_with(&mut e, &mut k, "9 9 Ctrl+-");
    assert_eq!(volume_percent(&e), 0, "clamps at 0");
    assert!(matches!(cmdline::parse("vol 250"), Err(crate::CmdLineError::BadArg { .. })));
    assert_eq!(e.cursor, Editor::new(song3()).cursor);
}

#[test]
fn track_pan_set_nudge_clamp_and_labels() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Ctrl+]");
    assert_eq!(pan_percent(&e), 10);
    assert_eq!(last_status(&ev), "pan R10 on 1/1: Track 1");
    drive_with(&mut e, &mut k, "Ctrl+[ Ctrl+[");
    assert_eq!(pan_percent(&e), -10);
    assert_eq!(e.undo.undo_depth(), 1);
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, song3());
    assert_eq!(cmdline::parse("pan L30").unwrap(), Command::SetTrackPan { value: -30 });
    assert_eq!(cmdline::parse("pan c").unwrap(), Command::SetTrackPan { value: 0 });
    assert_eq!(cmdline::parse("pan R100").unwrap(), Command::SetTrackPan { value: 100 });
    assert_eq!(cmdline::parse("pan -45").unwrap(), Command::SetTrackPan { value: -45 });
    assert!(matches!(cmdline::parse("pan 101"), Err(crate::CmdLineError::BadArg { .. })));
    assert!(matches!(cmdline::parse("pan left"), Err(crate::CmdLineError::BadArg { .. })));
    let ev = e.execute(&cmdline::parse("pan L30").unwrap(), None).unwrap();
    assert_eq!(pan_percent(&e), -30);
    assert_eq!(last_status(&ev), "pan L30 on 1/1: Track 1");
    e.execute(&Command::SetTrackPan { value: 100 }, None).unwrap();
    let err = try_drive_with(&mut e, &mut k, "Ctrl+]").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }));
    drive_with(&mut e, &mut k, "9 9 Ctrl+[");
    assert_eq!(pan_percent(&e), -100, "clamps at hard left");
}

#[test]
fn mute_and_solo_toggle_and_undo() {
    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = drive_with(&mut e, &mut k, "Ctrl+m");
    assert!(e.track().unwrap().mute);
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "muted Track 1");
    let ev = drive_with(&mut e, &mut k, "Ctrl+m");
    assert!(!e.track().unwrap().mute);
    assert_eq!(last_status(&ev), "unmuted Track 1");
    let ev = drive_with(&mut e, &mut k, "Ctrl+s");
    assert!(e.track().unwrap().solo);
    assert_eq!(last_status(&ev), "solo Track 1");
    let ev = drive_with(&mut e, &mut k, "Ctrl+s");
    assert!(!e.track().unwrap().solo);
    assert_eq!(last_status(&ev), "solo off: Track 1");
    assert_eq!(e.undo.undo_depth(), 4, "every toggle is its own undo step");
    drive_with(&mut e, &mut k, "u");
    assert!(e.track().unwrap().solo);
    drive_with(&mut e, &mut k, "3 u");
    assert_eq!(e.song, song3());
    assert_eq!(cmdline::parse("mute").unwrap(), Command::ToggleMute);
    assert_eq!(cmdline::parse("solo").unwrap(), Command::ToggleSolo);
}

#[test]
fn header_mixer_edits_name_the_track_and_leave_the_shown_one_alone() {
    let mut e = Editor::new(three_tracks());
    let mut k = km();
    assert_eq!(e.track_index, 0);
    let ev = e.toggle_mute_of(2).unwrap();
    assert!(e.song.tracks[2].mute);
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "muted Drums");
    let ev = e.toggle_solo_of(1).unwrap();
    assert!(e.song.tracks[1].solo);
    assert_eq!(last_status(&ev), "solo Bass");
    assert_eq!(e.track_index, 0, "the piano roll stays on the first track");

    // a drag: many small steps, one undo entry, then a new drag is a new entry
    for percent in [90, 80, 70] {
        e.set_volume_of(1, percent).unwrap();
    }
    assert_eq!(e.song.tracks[1].volume, 0.7);
    e.end_mix_gesture();
    e.set_volume_of(1, 60).unwrap();
    let ev = e.set_pan_of(2, -40).unwrap();
    assert_eq!(e.song.tracks[2].pan, -0.4);
    assert_eq!(last_status(&ev), "pan L40 on 3/3: Drums");
    assert_eq!(e.undo.undo_depth(), 5, "mute, solo, first drag, second drag, pan");
    drive_with(&mut e, &mut k, "u u");
    assert_eq!(e.song.tracks[1].volume, 0.7);
    assert_eq!(e.song.tracks[2].pan, 0.0);
    assert!(e.set_volume_of(7, 50).is_err(), "no such track");
    assert!(e.set_volume_of(1, 70).is_err(), "no change is an error, like a nudge");
}

#[test]
fn track_channel_is_one_based_on_the_command_line() {
    assert_eq!(cmdline::parse("channel 10").unwrap(), Command::SetTrackChannel { channel: 9 });
    assert_eq!(cmdline::parse("ch 1").unwrap(), Command::SetTrackChannel { channel: 0 });
    assert!(matches!(cmdline::parse("channel 0"), Err(crate::CmdLineError::BadArg { .. })));
    assert!(matches!(cmdline::parse("channel 17"), Err(crate::CmdLineError::BadArg { .. })));
    let mut e = Editor::new(song3());
    let ev = e.execute(&Command::SetTrackChannel { channel: 9 }, None).unwrap();
    assert_eq!(e.track().unwrap().channel, 9);
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "channel 10 on 1/1: Track 1");
    assert_eq!(
        e.execute(&Command::SetTrackChannel { channel: 16 }, None),
        Err(EditError::BadValue("channel 17 is not 1-16".to_string()))
    );
    e.execute(&Command::Undo, None).unwrap();
    assert_eq!(e.song, song3());
}

#[test]
fn set_instrument_and_step_presets() {
    assert_eq!(cmdline::parse("sf2 default").unwrap(), Command::SetInstrument { path: None, bank: 0, preset: 0 });
    assert_eq!(
        cmdline::parse("sf2 sounds/gm.sf2 33").unwrap(),
        Command::SetInstrument { path: Some("sounds/gm.sf2".into()), bank: 0, preset: 33 }
    );
    assert_eq!(
        cmdline::parse("instrument my sounds/gm 2.sf2 5 1").unwrap(),
        Command::SetInstrument { path: Some("my sounds/gm 2.sf2".into()), bank: 1, preset: 5 },
        "a path with spaces; trailing numbers are preset then bank"
    );
    assert_eq!(cmdline::parse("sf2 Default 7").unwrap(), Command::SetInstrument { path: None, bank: 0, preset: 7 });
    assert!(matches!(cmdline::parse("sf2 gm.sf2 200"), Err(crate::CmdLineError::BadArg { ref arg, .. }) if arg == "preset"));
    assert!(matches!(cmdline::parse("sf2"), Err(crate::CmdLineError::MissingArg { .. })));

    let mut e = Editor::new(song3());
    let mut k = km();
    let ev = e.execute(&cmdline::parse("sf2 sounds/gm.sf2 33").unwrap(), None).unwrap();
    assert_eq!(
        e.track().unwrap().instrument,
        crate::song::Instrument::SoundFont { path: "sounds/gm.sf2".into(), bank: 0, preset: 33 }
    );
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "Electric Bass (finger) from sounds/gm.sf2 on 1/1: Track 1");
    let ev = drive_with(&mut e, &mut k, "Ctrl+. Ctrl+.");
    assert_eq!(preset(&e), 35);
    assert_eq!(last_status(&ev), "Fretless Bass from sounds/gm.sf2 on 1/1: Track 1");
    drive_with(&mut e, &mut k, "3 Ctrl+,");
    assert_eq!(preset(&e), 32);
    assert_eq!(e.undo.undo_depth(), 2, "preset steps coalesce into one step after the sf2 change");
    drive_with(&mut e, &mut k, "u");
    assert_eq!(preset(&e), 33);
    e.execute(&cmdline::parse("sf2 default 127").unwrap(), None).unwrap();
    drive_with(&mut e, &mut k, "Ctrl+.");
    assert_eq!(preset(&e), 0, "wraps 127 -> 0");
    drive_with(&mut e, &mut k, "Ctrl+,");
    assert_eq!(preset(&e), 127, "wraps 0 -> 127");
    let ev = drive_with(&mut e, &mut k, "3 Ctrl+.");
    assert_eq!(preset(&e), 2);
    assert_eq!(last_status(&ev), "Electric Grand Piano on 1/1: Track 1");
    assert_eq!(
        e.execute(&Command::SetInstrument { path: None, bank: 0, preset: 128 }, None),
        Err(EditError::BadValue("preset 128 is not 0-127".to_string()))
    );
    let depth = e.undo.undo_depth();
    drive_with(&mut e, &mut k, &counted(depth, "u"));
    assert_eq!(e.song, song3());
}

#[test]
fn master_reverb() {
    assert_eq!(cmdline::parse("reverb 40%").unwrap(), Command::SetMasterReverb { percent: 40 });
    assert!(matches!(cmdline::parse("reverb 150"), Err(crate::CmdLineError::BadArg { .. })));
    assert!(matches!(cmdline::parse("reverb"), Err(crate::CmdLineError::MissingArg { .. })));
    let mut e = Editor::new(song3());
    assert_eq!(e.song.master_reverb, 0.25);
    let ev = e.execute(&Command::SetMasterReverb { percent: 40 }, None).unwrap();
    assert_eq!(e.song.master_reverb, 0.4);
    assert!(has_song_changed(&ev));
    assert_eq!(status(&ev), vec!["master reverb 40%"]);
    e.execute(&Command::SetMasterReverb { percent: 10 }, Some(60)).unwrap();
    assert_eq!(e.song.master_reverb, 0.6, "count overrides the percent");
    e.execute(&Command::SetMasterReverb { percent: 150 }, None).unwrap();
    assert_eq!(e.song.master_reverb, 1.0, "clamped");
    e.execute(&Command::Undo, Some(3)).unwrap();
    assert_eq!(e.song, song3());
}

#[test]
fn track_edits_undo_all_the_way_back() {
    let before = three_tracks();
    let mut e = Editor::new(before.clone());
    let mut k = km();
    drive_with(
        &mut e,
        &mut k,
        "t Cmd+t Ctrl+m Ctrl+s Ctrl+= Ctrl+] Ctrl+. Cmd+Alt+Up Cmd+d Cmd+Shift+t t Cmd+Alt+Up Ctrl+Shift+d Shift+c Ctrl+Shift+Down",
    );
    assert_ne!(e.song, before);
    assert_eq!(track_names(&e), vec!["Track 1", "Track 4", "Drums", "Bass"]);
    assert_eq!(e.undo.undo_depth(), 12);
    drive_with(&mut e, &mut k, "1 2 u");
    assert_eq!(e.song, before);
    assert_eq!(e.track_index, 1, "the first edit happened on track 2");
    assert!(!e.undo.can_undo());
    assert_eq!(e.lane, Some(LaneKind::Sustain), "the shown lane is not part of undo");
    drive_with(&mut e, &mut k, "1 2 Shift+u");
    assert_eq!(track_names(&e), vec!["Track 1", "Track 4", "Drums", "Bass"]);
}

// ---------------------------------------------------------------- controller lanes

#[test]
fn show_and_cycle_lane() {
    let mut e = Editor::new(song3());
    let mut k = km();
    assert_eq!(e.lane, None);
    let ev = drive_with(&mut e, &mut k, "Shift+c");
    assert_eq!(e.lane, Some(LaneKind::Sustain));
    assert_eq!(status(&ev), vec!["lane: sustain"]);
    assert!(!has_song_changed(&ev));
    drive_with(&mut e, &mut k, "Shift+c Shift+c Shift+c Shift+c");
    assert_eq!(e.lane, Some(LaneKind::Pan));
    let ev = drive_with(&mut e, &mut k, "Shift+c");
    assert_eq!(e.lane, None);
    assert_eq!(status(&ev), vec!["lane hidden"]);
    drive_with(&mut e, &mut k, "3 Shift+c");
    assert_eq!(e.lane, Some(LaneKind::ModWheel), "count repeats");
    assert_eq!(cmdline::parse("lane bend").unwrap(), Command::ShowLane { kind: Some(LaneKind::PitchBend) });
    assert_eq!(cmdline::parse("lane mod").unwrap(), Command::ShowLane { kind: Some(LaneKind::ModWheel) });
    assert_eq!(cmdline::parse("lane off").unwrap(), Command::ShowLane { kind: None });
    assert_eq!(cmdline::parse("lane next").unwrap(), Command::CycleLane);
    assert!(matches!(cmdline::parse("lane x"), Err(crate::CmdLineError::BadArg { .. })));
    let ev = e.execute(&cmdline::parse("lane volume").unwrap(), None).unwrap();
    assert_eq!(e.lane, Some(LaneKind::Volume));
    assert_eq!(status(&ev), vec!["lane: volume"]);
    e.execute(&Command::ShowLane { kind: None }, None).unwrap();
    assert_eq!(e.lane, None);
    assert_eq!(e.undo.undo_depth(), 0, "showing a lane is not an edit");
    assert_eq!(e.song, song3());
}

#[test]
fn lane_commands_need_a_shown_lane() {
    let mut e = Editor::new(song3());
    let before = e.clone();
    for cmd in [
        Command::SetLaneValue { value: 1 },
        Command::NudgeLaneValue { delta: 1 },
        Command::RampLane { from: 0, to: 127 },
        Command::ClearLane,
    ] {
        assert_eq!(e.execute(&cmd, None), Err(EditError::NoLaneShown), "{}", cmd.name());
    }
    assert_eq!(e.song, before.song);
    assert_eq!(e.undo, before.undo);
}

#[test]
fn set_and_nudge_lane_value_at_the_cursor() {
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "Shift+c");
    let ev = e.execute(&cmdline::parse("lane_value 90").unwrap(), None).unwrap();
    assert_eq!(sustain(&e), vec![(0, 90)]);
    assert!(has_song_changed(&ev));
    assert_eq!(status(&ev), vec!["sustain 90 at 1:1:000"]);
    // count overrides the value; values clamp to the lane's range; same tick replaces
    drive_with(&mut e, &mut k, "Right");
    e.execute(&Command::SetLaneValue { value: 90 }, Some(127)).unwrap();
    assert_eq!(sustain(&e), vec![(0, 90), (G, 127)]);
    let ev = e.execute(&Command::SetLaneValue { value: 300 }, None).unwrap();
    assert!(!has_song_changed(&ev), "300 clamps to 127, which is already there: a no-op");
    assert_eq!(sustain(&e), vec![(0, 90), (G, 127)]);
    assert_eq!(e.undo.undo_depth(), 2);
    // nudging at the limit fails; nudging where there is no point starts from the
    // value in force there
    let err = try_drive_with(&mut e, &mut k, "Ctrl+Shift+Up").unwrap_err();
    assert!(matches!(err, DriveError::Edit { source: EditError::OutOfRange(_), .. }));
    let ev = drive_with(&mut e, &mut k, "Right Ctrl+Shift+Down");
    assert_eq!(sustain(&e), vec![(0, 90), (G, 127), (2 * G, 119)]);
    assert_eq!(last_status(&ev), "sustain 119 at 1:1:480");
    drive_with(&mut e, &mut k, "2 Ctrl+Shift+Down");
    assert_eq!(sustain(&e), vec![(0, 90), (G, 127), (2 * G, 103)]);
    assert_eq!(e.undo.undo_depth(), 3, "nudges coalesce");
    drive_with(&mut e, &mut k, "u");
    assert_eq!(sustain(&e), vec![(0, 90), (G, 127)]);
    assert_eq!(starts(&e), vec![0, G, 4 * G], "notes are never touched");
    assert_eq!(e.cursor.tick, Tick(2 * G), "cursor stays");
    // pitch bend nudges are scaled by 64 and clamp to -8192..=8191
    e.execute(&Command::ShowLane { kind: Some(LaneKind::PitchBend) }, None).unwrap();
    drive_with(&mut e, &mut k, "Ctrl+Shift+Up");
    assert_eq!(e.track().unwrap().lanes.pitch_bend[0].value, 512);
    drive_with(&mut e, &mut k, "9 9 Ctrl+Shift+Down");
    assert_eq!(e.track().unwrap().lanes.pitch_bend[0].value, -8192);
    assert_eq!(e.track().unwrap().lanes.pitch_bend.len(), 1);
}

#[test]
fn ramp_lane_over_the_selection_the_note_or_a_bar() {
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "Shift+c");
    // nothing selected, cursor on C4@0 (one grid step long): cursor to the note's end
    let ev = e.execute(&cmdline::parse("ramp_lane 0 127").unwrap(), None).unwrap();
    assert_eq!(sustain(&e), vec![(0, 0), (G, 127)]);
    assert!(has_song_changed(&ev));
    assert_eq!(status(&ev), vec!["ramped sustain 0 → 127: 2 points from 1:1:000 to 1:1:240"]);
    // a selection: its first start to its last end, one point per grid step
    drive_with(&mut e, &mut k, "a");
    e.execute(&cmdline::parse("lane ramp 0..127").unwrap(), None).unwrap();
    assert_eq!(sustain(&e), vec![(0, 0), (G, 25), (2 * G, 51), (3 * G, 76), (4 * G, 102), (5 * G, 127)]);
    assert_eq!(starts(&e), vec![0, G, 4 * G], "notes untouched");
    assert_eq!(ids(&e), vec![1, 2, 3], "selection kept");
    // no note, no selection: one bar from the cursor; points in the range are replaced
    drive_with(&mut e, &mut k, "Escape Cmd+Right");
    e.execute(&Command::SetLaneValue { value: 5 }, None).unwrap();
    drive_with(&mut e, &mut k, "Right");
    e.execute(&Command::SetLaneValue { value: 5 }, None).unwrap();
    drive_with(&mut e, &mut k, "Left");
    e.execute(&Command::RampLane { from: 127, to: 0 }, Some(9)).unwrap();
    let s = sustain(&e);
    assert_eq!(s.len(), 6 + 17, "six from before plus 16 grid steps and the bar end");
    assert_eq!(s[6], (3840, 127));
    assert_eq!(s[7], (3840 + G, 119));
    assert_eq!(s[22], (7680, 0));
    assert!(s.iter().all(|(t, v)| *t != 3840 + G || *v == 119), "the old point at the second step was replaced");
    let depth = e.undo.undo_depth();
    drive_with(&mut e, &mut k, &counted(depth, "u"));
    assert_eq!(e.song, song3());
    // volume lane clamps the ramp ends to 0..=127 (undo put the cursor back on C4@0)
    assert_eq!(e.cursor.tick, Tick::ZERO);
    e.execute(&cmdline::parse("lane volume").unwrap(), None).unwrap();
    e.execute(&Command::RampLane { from: -50, to: 500 }, None).unwrap();
    let vol: Vec<i32> = e.track().unwrap().lanes.volume.iter().map(|p| p.value).collect();
    assert_eq!(vol, vec![0, 127]);
    assert!(sustain(&e).is_empty(), "other lanes untouched");
}

#[test]
fn clear_lane_in_the_selection_range_or_everything() {
    let mut e = Editor::new(song3());
    let mut k = km();
    drive_with(&mut e, &mut k, "Shift+c");
    set_sustain(&mut e, &[(0, 127), (G, 64), (3 * G, 0), (2000, 127)]);
    let with_points = e.song.clone();
    // box C4 and E4 (0..=720): the point exactly at the selection's end goes too
    let ev = drive_with(&mut e, &mut k, "Shift+Right Shift+Up Shift+Up Shift+Up Shift+Up Ctrl+Shift+x");
    assert_eq!(sustain(&e), vec![(2000, 127)]);
    assert!(has_song_changed(&ev));
    assert_eq!(last_status(&ev), "cleared 3 sustain points in the selection");
    assert_eq!(ids(&e), vec![1, 2], "selection kept");
    let ev = drive_with(&mut e, &mut k, "Ctrl+Shift+x");
    assert!(!has_song_changed(&ev), "nothing left in the range");
    assert_eq!(last_status(&ev), "no sustain points in the selection");
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, with_points);
    // nothing selected: the whole lane
    let ev = drive_with(&mut e, &mut k, "Escape Ctrl+Shift+x");
    assert!(sustain(&e).is_empty());
    assert_eq!(last_status(&ev), "cleared 4 sustain points");
    assert_eq!(starts(&e), vec![0, G, 4 * G]);
    let ev = drive_with(&mut e, &mut k, "Ctrl+Shift+x");
    assert_eq!(last_status(&ev), "no sustain points");
    assert_eq!(e.undo.undo_depth(), 1, "no-op clears take no undo step");
    drive_with(&mut e, &mut k, "u");
    assert_eq!(e.song, with_points);
}

#[test]
fn sustain_down_up_and_pedal_selection() {
    let mut e = Editor::new(song3());
    let mut k = km();
    assert_eq!(e.lane, None, "the pedal keys work without a lane shown");
    let ev = drive_with(&mut e, &mut k, "Ctrl+Shift+d");
    assert_eq!(sustain(&e), vec![(0, 127)]);
    assert!(has_song_changed(&ev));
    assert_eq!(status(&ev), vec!["sustain 127 at 1:1:000"]);
    let ev = drive_with(&mut e, &mut k, "Cmd+Right Ctrl+Shift+u");
    assert_eq!(sustain(&e), vec![(0, 127), (3840, 0)]);
    assert_eq!(last_status(&ev), "sustain 0 at 2:1:000");
    let ev = drive_with(&mut e, &mut k, "Ctrl+Shift+u");
    assert!(!has_song_changed(&ev), "already up here");
    assert_eq!(e.undo.undo_depth(), 2);
    drive_with(&mut e, &mut k, "2 u");
    assert_eq!(e.song, song3());
    assert_eq!(cmdline::parse("pedal down").unwrap(), Command::SustainDown);
    assert_eq!(cmdline::parse("pedal up").unwrap(), Command::SustainUp);
    assert_eq!(cmdline::parse("pedal").unwrap(), Command::PedalSelection);
    // pedal: down at the first start, up at the last end, anything between removed
    set_sustain(&mut e, &[(500, 0)]);
    drive_with(&mut e, &mut k, "Cmd+Right");
    let err = e.execute(&Command::PedalSelection, None).unwrap_err();
    assert_eq!(err, EditError::NothingSelected, "cursor at bar 2 has no note under it");
    drive_with(&mut e, &mut k, "a");
    let ev = e.execute(&Command::PedalSelection, None).unwrap();
    assert_eq!(sustain(&e), vec![(0, 127), (5 * G, 0)]);
    assert_eq!(status(&ev), vec!["pedal 1:1:000 → 1:2:240"]);
    assert_eq!(ids(&e), vec![1, 2, 3]);
    assert_eq!(e.cursor.tick, Tick(3840), "cursor stays");
    // with nothing selected it pedals the note under the cursor
    drive_with(&mut e, &mut k, "Escape Cmd+Left Right Up Up Up Up");
    e.execute(&Command::PedalSelection, None).unwrap();
    assert_eq!(sustain(&e), vec![(0, 127), (G, 127), (3 * G, 0), (5 * G, 0)]);
    drive_with(&mut e, &mut k, "u u");
    assert_eq!(sustain(&e), vec![(500, 0)]);
}

// ---------------------------------------------------------------- snapshots

#[test]
fn snapshot_insert_and_edit() {
    let e = drive(Song::new_empty(), &km(), "n Right Up Up Up Up 2 n Right Right Cmd+Up n a ] Shift+=");
    insta::assert_ron_snapshot!("insert_and_edit", e.song);
}

#[test]
fn snapshot_copy_paste_duplicate() {
    let e = drive(song3(), &km(), "a c Cmd+Right p d Alt+Shift+Down");
    insta::assert_ron_snapshot!("copy_paste_duplicate", e.song);
}

#[test]
fn snapshot_box_and_move() {
    let e = drive(song3(), &km(), "n Right n Right n Shift+Left Shift+Left Alt+Up Alt+Up [ Escape");
    insta::assert_ron_snapshot!("box_and_move", e.song);
}

#[test]
fn new_tracks_take_distinct_melodic_channels() {
    let mut e = Editor::new(song3());
    assert_eq!(e.song.free_channel(), 1, "track 1 holds channel 0");
    e.execute(&Command::AddTrack, Some(10)).unwrap();
    let channels: Vec<u8> = e.song.tracks.iter().map(|t| t.channel).collect();
    assert_eq!(channels, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11]);
    assert!(!channels.contains(&DRUM_CHANNEL));
    e.execute(&Command::AddTrack, Some(5)).unwrap();
    let channels: Vec<u8> = e.song.tracks.iter().map(|t| t.channel).collect();
    assert_eq!(&channels[11..], &[12, 13, 14, 15, 0], "the sixteenth melodic track reuses the least-used channel");
}

#[test]
fn drum_kit_moves_the_track_to_channel_ten_and_names_it() {
    let mut e = Editor::new(song3());
    let ev = e.execute(&Command::SetDrumKit { kit: 0 }, None).unwrap();
    assert!(has_song_changed(&ev));
    assert!(e.song.tracks[0].is_drums());
    assert_eq!(e.song.tracks[0].name, "Drums");
    assert_eq!(e.song.tracks[0].instrument, Instrument::SoundFont { path: PathBuf::new(), bank: 0, preset: 0 });
    assert_eq!(last_status(&ev), "Drums: Standard on 1/1: Drums");

    e.execute(&Command::RenameTrack { name: "Beat".to_string() }, None).unwrap();
    let ev = e.execute(&Command::SetDrumKit { kit: 8 }, None).unwrap();
    assert_eq!(e.song.tracks[0].name, "Beat", "a name the user chose is kept");
    assert_eq!(last_status(&ev), "Drums: Room on 1/1: Beat");

    e.execute(&Command::Undo, None).unwrap();
    e.execute(&Command::Undo, None).unwrap();
    e.execute(&Command::Undo, None).unwrap();
    assert_eq!(e.song, song3(), "one undo step per pick");
}

#[test]
fn a_preset_takes_a_drum_track_back_to_a_melodic_channel() {
    let mut e = Editor::new(song3());
    e.execute(&Command::AddTrack, None).unwrap();
    e.execute(&Command::SetDrumKit { kit: 0 }, None).unwrap();
    assert_eq!(e.song.tracks[1].channel, DRUM_CHANNEL);
    let ev = e.execute(&Command::SetPreset { preset: 33 }, None).unwrap();
    assert_eq!(e.song.tracks[1].channel, 1, "the lowest channel no other track uses");
    assert_eq!(e.song.tracks[1].name, "Drums", "the name is left alone");
    assert_eq!(last_status(&ev), "Electric Bass (finger) on 2/2: Drums");
    assert!(matches!(e.execute(&Command::SetPreset { preset: 128 }, None), Err(EditError::BadValue(_))));
}

#[test]
fn duplicating_keeps_drums_on_channel_ten_but_melodic_copies_get_their_own() {
    let mut e = Editor::new(song3());
    e.execute(&Command::DuplicateTrack, None).unwrap();
    assert_eq!(e.song.tracks[1].channel, 1);
    e.execute(&Command::SetDrumKit { kit: 0 }, None).unwrap();
    e.execute(&Command::DuplicateTrack, None).unwrap();
    assert_eq!(e.song.tracks[2].channel, DRUM_CHANNEL);
}

#[test]
fn default_track_names_are_recognised() {
    assert!(is_default_track_name("Track 1"));
    assert!(is_default_track_name("Track 12"));
    assert!(!is_default_track_name("Track"));
    assert!(!is_default_track_name("Track one"));
    assert!(!is_default_track_name("Bass"));
}
