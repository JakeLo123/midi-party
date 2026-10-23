//! Selection helpers: which notes are under the cursor, inside a box, in a bar or a
//! chord, or pass a filter. Pure functions over a [`Track`] (or a list of notes); the
//! [`crate::Editor`] decides when to call them and what to do with the ids.

use std::collections::{BTreeMap, BTreeSet};

use crate::editor::Cursor;
use crate::song::{Note, NoteId, Tick, Track};
use crate::time::MeterMap;

/// The note the cursor is "on": a note of the cursor's pitch that starts exactly at the
/// cursor tick, or failing that one that is still sounding there.
pub fn note_under_cursor(track: &Track, cursor: Cursor) -> Option<&Note> {
    track
        .note_at(cursor.tick, cursor.pitch)
        .or_else(|| track.note_sounding_at(cursor.tick, cursor.pitch))
}

/// Ids of the notes whose start lies in the box spanned by `a` and `b`.
///
/// The time range is inclusive of the whole grid cell at both ends: a note starting
/// anywhere in `[min_tick, max_tick + cell)` counts. Pitch is inclusive on both ends.
pub fn box_ids(track: &Track, a: Cursor, b: Cursor, cell: Tick) -> BTreeSet<NoteId> {
    let lo_pitch = a.pitch.min(b.pitch);
    let hi_pitch = a.pitch.max(b.pitch);
    box_ids_in_pitch_range(track, a, b, cell, lo_pitch, hi_pitch)
}

/// Like [`box_ids`] but spanning every pitch: a time range only.
pub fn time_box_ids(track: &Track, a: Cursor, b: Cursor, cell: Tick) -> BTreeSet<NoteId> {
    box_ids_in_pitch_range(track, a, b, cell, 0, u8::MAX)
}

fn box_ids_in_pitch_range(track: &Track, a: Cursor, b: Cursor, cell: Tick, lo_pitch: u8, hi_pitch: u8) -> BTreeSet<NoteId> {
    let lo_tick = a.tick.min(b.tick);
    let hi_tick = a.tick.max(b.tick).saturating_add(cell);
    track
        .notes
        .iter()
        .filter(|n| n.start >= lo_tick && n.start < hi_tick && n.pitch >= lo_pitch && n.pitch <= hi_pitch)
        .map(|n| n.id)
        .collect()
}

/// Ids of every note on the track.
pub fn all_ids(track: &Track) -> BTreeSet<NoteId> {
    track.notes.iter().map(|n| n.id).collect()
}

/// The notes of `track` whose ids are in `ids`, in track order `(start, pitch, id)`.
pub fn notes_with_ids(track: &Track, ids: &BTreeSet<NoteId>) -> Vec<Note> {
    track.notes.iter().filter(|n| ids.contains(&n.id)).copied().collect()
}

/// Ids of the notes starting in `[lo, hi)`.
pub fn ids_starting_in(track: &Track, lo: Tick, hi: Tick) -> BTreeSet<NoteId> {
    ids_where(track, |n| n.start >= lo && n.start < hi)
}

/// Ids of the notes starting exactly at `tick` (a chord, or a single note).
pub fn chord_at(track: &Track, tick: Tick) -> BTreeSet<NoteId> {
    ids_where(track, |n| n.start == tick)
}

/// Ids of the first `n` notes starting at or after `tick`, in track order.
pub fn next_n_from(track: &Track, tick: Tick, n: usize) -> BTreeSet<NoteId> {
    track.notes.iter().filter(|note| note.start >= tick).take(n).map(|note| note.id).collect()
}

/// Ids of the notes for which `pred` holds.
pub fn ids_where(track: &Track, pred: impl Fn(&Note) -> bool) -> BTreeSet<NoteId> {
    track.notes.iter().filter(|n| pred(n)).map(|n| n.id).collect()
}

/// The highest (`top`) or lowest note of every group of `notes` sharing a start tick,
/// counting only groups of at least `min_size` notes. Pass the whole track with
/// `min_size = 2` for "the top of every chord", or the selection with `min_size = 1`
/// for "thin each chord to its top note".
pub fn chord_extremes(notes: &[Note], top: bool, min_size: usize) -> BTreeSet<NoteId> {
    let mut groups: BTreeMap<Tick, Vec<&Note>> = BTreeMap::new();
    for n in notes {
        groups.entry(n.start).or_default().push(n);
    }
    groups
        .values()
        .filter(|g| g.len() >= min_size.max(1))
        .filter_map(|g| {
            let pick = if top { g.iter().max_by_key(|n| n.pitch) } else { g.iter().min_by_key(|n| n.pitch) };
            pick.map(|n| n.id)
        })
        .collect()
}

/// Every `n`th of `notes` (which must already be in the wanted order), starting with
/// the first. `n` of 0 or 1 keeps everything.
pub fn every_nth(notes: &[Note], n: u32) -> BTreeSet<NoteId> {
    let n = usize::try_from(n.max(1)).unwrap_or(1);
    notes.iter().step_by(n).map(|note| note.id).collect()
}

/// `true` if `tick` falls exactly on a beat of the meter in force there.
pub fn is_on_beat(meter: &MeterMap, ppq: u32, tick: Tick) -> bool {
    meter.bar_beat_of(ppq, tick).tick_in_beat == 0
}

/// The ids of `notes` sorted by `(start, pitch, id)`: the order Tab steps through a
/// selection.
pub fn in_time_order(notes: &[Note]) -> Vec<NoteId> {
    let mut sorted: Vec<&Note> = notes.iter().collect();
    sorted.sort_by_key(|n| n.sort_key());
    sorted.into_iter().map(|n| n.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::Song;

    fn song() -> Song {
        let mut s = Song::new_empty();
        let notes = [
            s.new_note(Tick(0), Tick(240), 60, 100),
            s.new_note(Tick(240), Tick(480), 62, 100),
            s.new_note(Tick(960), Tick(240), 64, 100),
            s.new_note(Tick(960), Tick(240), 72, 100),
        ];
        for n in notes {
            s.tracks[0].insert_note(n);
        }
        s
    }

    fn set(ids: &[u64]) -> BTreeSet<NoteId> {
        ids.iter().map(|i| NoteId(*i)).collect()
    }

    #[test]
    fn under_cursor_prefers_exact_start() {
        let s = song();
        let t = &s.tracks[0];
        assert_eq!(note_under_cursor(t, Cursor { tick: Tick(240), pitch: 62 }).map(|n| n.id), Some(NoteId(2)));
        // still sounding
        assert_eq!(note_under_cursor(t, Cursor { tick: Tick(500), pitch: 62 }).map(|n| n.id), Some(NoteId(2)));
        assert!(note_under_cursor(t, Cursor { tick: Tick(720), pitch: 62 }).is_none());
        assert!(note_under_cursor(t, Cursor { tick: Tick(0), pitch: 61 }).is_none());
    }

    #[test]
    fn box_is_inclusive_of_cell() {
        let s = song();
        let t = &s.tracks[0];
        let a = Cursor { tick: Tick(0), pitch: 60 };
        let b = Cursor { tick: Tick(240), pitch: 64 };
        let ids = box_ids(t, a, b, Tick(240));
        assert_eq!(ids, set(&[1, 2]));
        // reversed corners give the same box
        assert_eq!(box_ids(t, b, a, Tick(240)), ids);
        // a bar-sized cell reaches the notes at 960
        let ids = box_ids(t, a, Cursor { tick: Tick(720), pitch: 64 }, Tick(240));
        assert_eq!(ids.len(), 2);
        let ids = box_ids(t, a, Cursor { tick: Tick(720), pitch: 64 }, Tick(960));
        assert_eq!(ids, set(&[1, 2, 3]));
        assert_eq!(all_ids(t).len(), 4);
        assert_eq!(notes_with_ids(t, &all_ids(t)).len(), 4);
        // a time box ignores pitch entirely
        let ids = time_box_ids(t, Cursor { tick: Tick(960), pitch: 0 }, Cursor { tick: Tick(960), pitch: 0 }, Tick(240));
        assert_eq!(ids, set(&[3, 4]));
    }

    #[test]
    fn ranges_chords_and_next() {
        let s = song();
        let t = &s.tracks[0];
        assert_eq!(ids_starting_in(t, Tick(0), Tick(960)), set(&[1, 2]));
        assert_eq!(ids_starting_in(t, Tick(960), Tick(961)), set(&[3, 4]));
        assert_eq!(chord_at(t, Tick(960)), set(&[3, 4]));
        assert_eq!(chord_at(t, Tick(1)), set(&[]));
        assert_eq!(next_n_from(t, Tick(100), 2), set(&[2, 3]));
        assert_eq!(next_n_from(t, Tick(100), 10), set(&[2, 3, 4]));
        assert_eq!(next_n_from(t, Tick(5000), 2), set(&[]));
        assert_eq!(ids_where(t, |n| n.pitch > 63), set(&[3, 4]));
    }

    #[test]
    fn chord_extremes_and_every_nth() {
        let s = song();
        let notes = &s.tracks[0].notes;
        assert_eq!(chord_extremes(notes, true, 2), set(&[4]));
        assert_eq!(chord_extremes(notes, false, 2), set(&[3]));
        assert_eq!(chord_extremes(notes, true, 1), set(&[1, 2, 4]));
        assert_eq!(chord_extremes(notes, false, 0), set(&[1, 2, 3]));
        assert_eq!(every_nth(notes, 2), set(&[1, 3]));
        assert_eq!(every_nth(notes, 3), set(&[1, 4]));
        assert_eq!(every_nth(notes, 0), set(&[1, 2, 3, 4]));
        assert_eq!(in_time_order(notes), vec![NoteId(1), NoteId(2), NoteId(3), NoteId(4)]);
    }

    #[test]
    fn on_beat() {
        let m = MeterMap::constant(4, 4);
        assert!(is_on_beat(&m, 960, Tick(0)));
        assert!(is_on_beat(&m, 960, Tick(960)));
        assert!(is_on_beat(&m, 960, Tick(3840 * 7 + 960 * 3)));
        assert!(!is_on_beat(&m, 960, Tick(240)));
        assert!(!is_on_beat(&m, 960, Tick(959)));
    }
}
