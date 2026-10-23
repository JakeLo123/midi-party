//! Note-editing primitives on a [`Track`]: shift, transpose, resize, velocity, paste.
//!
//! These are plain functions with no knowledge of modes, undo or the cursor. They
//! return the notes they touched so the editor can audition them. Clamping decisions
//! (how far a group may move) live in [`clamp_time_shift`] and [`clamp_pitch_shift`].

use std::collections::BTreeSet;

use crate::editor::select::notes_with_ids;
use crate::song::{Note, NoteId, Song, Tick, Track};
use crate::time::Grid;

/// Lowest velocity an edit will produce (0 would be a note-off).
pub const MIN_VELOCITY: u8 = 1;
/// Highest MIDI velocity.
pub const MAX_VELOCITY: u8 = 127;
/// Highest MIDI key number.
pub const MAX_PITCH: u8 = 127;

/// Reduces a requested time shift (negative = earlier) so that no note in `notes`
/// would start before tick 0. Returns the shift actually possible (may be 0).
pub fn clamp_time_shift(notes: &[Note], delta: i64) -> i64 {
    if delta >= 0 {
        return delta;
    }
    let min_start = notes.iter().map(|n| n.start.get()).min().unwrap_or(0);
    let max_back = i64::try_from(min_start).unwrap_or(i64::MAX);
    delta.max(-max_back)
}

/// Reduces a requested pitch shift so that every note in `notes` stays in 0..=127.
/// Returns the shift actually possible (may be 0).
pub fn clamp_pitch_shift(notes: &[Note], delta: i32) -> i32 {
    if notes.is_empty() {
        return 0;
    }
    let min_pitch = notes.iter().map(|n| i32::from(n.pitch)).min().unwrap_or(0);
    let max_pitch = notes.iter().map(|n| i32::from(n.pitch)).max().unwrap_or(0);
    if delta >= 0 {
        delta.min(i32::from(MAX_PITCH) - max_pitch)
    } else {
        delta.max(-min_pitch)
    }
}

/// Moves the notes with `ids` by `delta` ticks (already clamped) and re-sorts.
/// Returns the moved notes.
pub fn shift_time(track: &mut Track, ids: &BTreeSet<NoteId>, delta: i64) -> Vec<Note> {
    let mut moved = Vec::new();
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        let start = i64::try_from(n.start.get()).unwrap_or(i64::MAX).saturating_add(delta).max(0);
        n.start = Tick(u64::try_from(start).unwrap_or(0));
        moved.push(*n);
    }
    track.sort_notes();
    moved
}

/// Transposes the notes with `ids` by `delta` semitones (already clamped) and re-sorts.
/// Returns the moved notes.
pub fn shift_pitch(track: &mut Track, ids: &BTreeSet<NoteId>, delta: i32) -> Vec<Note> {
    let mut moved = Vec::new();
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        let p = (i32::from(n.pitch) + delta).clamp(0, i32::from(MAX_PITCH));
        n.pitch = u8::try_from(p).unwrap_or(0);
        moved.push(*n);
    }
    track.sort_notes();
    moved
}

/// Adds `delta_ticks` to the duration of the notes with `ids`, never going below
/// `floor`. Returns how many notes actually changed.
pub fn change_length(track: &mut Track, ids: &BTreeSet<NoteId>, delta_ticks: i64, floor: Tick) -> usize {
    let floor = floor.get().max(1);
    let mut changed = 0;
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        let cur = i64::try_from(n.duration.get()).unwrap_or(i64::MAX);
        let new = cur.saturating_add(delta_ticks);
        let new = u64::try_from(new.max(0)).unwrap_or(0).max(floor);
        if new != n.duration.get() {
            n.duration = Tick(new);
            changed += 1;
        }
    }
    changed
}

/// Adds `delta` to the velocity of the notes with `ids`, clamped to 1..=127.
/// Returns how many notes actually changed.
pub fn nudge_velocity(track: &mut Track, ids: &BTreeSet<NoteId>, delta: i32) -> usize {
    let mut changed = 0;
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        let v = (i32::from(n.velocity) + delta).clamp(i32::from(MIN_VELOCITY), i32::from(MAX_VELOCITY));
        let v = u8::try_from(v).unwrap_or(MIN_VELOCITY);
        if v != n.velocity {
            n.velocity = v;
            changed += 1;
        }
    }
    changed
}

/// Sets the velocity of the notes with `ids` to `value` (clamped to 1..=127).
/// Returns how many notes actually changed.
pub fn set_velocity(track: &mut Track, ids: &BTreeSet<NoteId>, value: u8) -> usize {
    let value = value.clamp(MIN_VELOCITY, MAX_VELOCITY);
    let mut changed = 0;
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        if n.velocity != value {
            n.velocity = value;
            changed += 1;
        }
    }
    changed
}

/// Copies the notes with `ids` into clipboard form: sorted, with `start` made relative
/// to the earliest note (so the first note starts at tick 0). Ids are kept but are
/// meaningless; [`paste`] allocates fresh ones.
pub fn clip_relative(track: &Track, ids: &BTreeSet<NoteId>) -> Vec<Note> {
    let mut notes = notes_with_ids(track, ids);
    let base = notes.iter().map(|n| n.start).min().unwrap_or(Tick::ZERO);
    for n in &mut notes {
        n.start -= base;
    }
    notes.sort_by_key(Note::sort_key);
    notes
}

/// Inserts copies of `clip` (relative notes, see [`clip_relative`]) on track
/// `track_index` starting at `at`, with fresh ids, replacing notes that share a start
/// and pitch. Returns the new notes. Does nothing if the track does not exist.
pub fn paste(song: &mut Song, track_index: usize, clip: &[Note], at: Tick) -> Vec<Note> {
    if track_index >= song.tracks.len() {
        return Vec::new();
    }
    let mut pasted = Vec::with_capacity(clip.len());
    for c in clip {
        let note = song.new_note(at.saturating_add(c.start), c.duration.max(Tick(1)), c.pitch, c.velocity);
        song.tracks[track_index].insert_note(note);
        pasted.push(note);
    }
    pasted
}

/// The end (max `start + duration`) of a group of notes, or `at` if empty.
pub fn group_end(notes: &[Note], fallback: Tick) -> Tick {
    notes.iter().map(Note::end).max().unwrap_or(fallback)
}

/// Sets the duration of the notes with `ids` to `len` (at least 1 tick).
/// Returns how many notes actually changed.
pub fn set_length(track: &mut Track, ids: &BTreeSet<NoteId>, len: Tick) -> usize {
    let len = Tick(len.get().max(1));
    let mut changed = 0;
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        if n.duration != len {
            n.duration = len;
            changed += 1;
        }
    }
    changed
}

/// Linear velocity ramp over the notes with `ids`, in time order: the earliest start
/// tick gets `from`, the latest `to`, and notes sharing a start tick share a value.
/// A single start tick gets `from`. Returns how many notes actually changed.
pub fn ramp_velocity(track: &mut Track, ids: &BTreeSet<NoteId>, from: u8, to: u8) -> usize {
    let mut ticks: Vec<Tick> = notes_with_ids(track, ids).iter().map(|n| n.start).collect();
    ticks.sort_unstable();
    ticks.dedup();
    let last = ticks.len().saturating_sub(1);
    let from = f64::from(from.clamp(MIN_VELOCITY, MAX_VELOCITY));
    let to = f64::from(to.clamp(MIN_VELOCITY, MAX_VELOCITY));
    let mut changed = 0;
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        let Ok(i) = ticks.binary_search(&n.start) else { continue };
        let frac = if last == 0 { 0.0 } else { i as f64 / last as f64 };
        let v = (from + (to - from) * frac).round();
        let v = u8::try_from(v as i64).unwrap_or(MAX_VELOCITY).clamp(MIN_VELOCITY, MAX_VELOCITY);
        if v != n.velocity {
            n.velocity = v;
            changed += 1;
        }
    }
    changed
}

/// A tiny deterministic generator (xorshift64) so humanize is reproducible in tests
/// and across undo/redo. The state is kept in the editor and advances on every call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XorShift64(pub u64);

impl XorShift64 {
    /// Seed used by a fresh editor.
    pub const DEFAULT_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

    /// Next raw 64-bit value. A zero state is nudged to the default seed first, since
    /// xorshift would otherwise stay at zero forever.
    pub fn next_u64(&mut self) -> u64 {
        if self.0 == 0 {
            self.0 = Self::DEFAULT_SEED;
        }
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A value in `-spread..=spread`, uniformly.
    pub fn symmetric(&mut self, spread: u32) -> i64 {
        if spread == 0 {
            return 0;
        }
        let range = u64::from(spread) * 2 + 1;
        let r = self.next_u64() % range;
        i64::try_from(r).unwrap_or(0) - i64::from(spread)
    }
}

/// Randomises velocity (±`velocity`) and start (±`timing` ticks, never before tick 0)
/// of the notes with `ids`, drawing from `rng`. Re-sorts. Returns the touched notes.
pub fn humanize(track: &mut Track, ids: &BTreeSet<NoteId>, velocity: u8, timing: u32, rng: &mut XorShift64) -> Vec<Note> {
    let mut touched = Vec::new();
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        let dv = rng.symmetric(u32::from(velocity));
        let v = (i64::from(n.velocity) + dv).clamp(i64::from(MIN_VELOCITY), i64::from(MAX_VELOCITY));
        n.velocity = u8::try_from(v).unwrap_or(MIN_VELOCITY);
        let dt = rng.symmetric(timing);
        let start = i64::try_from(n.start.get()).unwrap_or(i64::MAX).saturating_add(dt).max(0);
        n.start = Tick(u64::try_from(start).unwrap_or(0));
        touched.push(*n);
    }
    track.sort_notes();
    touched
}

/// Moves each note start `strength` percent of the way to its nearest grid point
/// (`swing` in 0..=1, see [`Grid::snap`]). Re-sorts. Returns how many notes moved.
pub fn quantize_starts(track: &mut Track, ids: &BTreeSet<NoteId>, grid: Grid, ppq: u32, strength: u8, swing: f32) -> usize {
    let strength = f64::from(strength.min(100)) / 100.0;
    let mut changed = 0;
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        let target = grid.snap(n.start, ppq, swing);
        let from = n.start.get() as f64;
        let to = target.get() as f64;
        let new = Tick((from + (to - from) * strength).round().max(0.0) as u64);
        if new != n.start {
            n.start = new;
            changed += 1;
        }
    }
    track.sort_notes();
    changed
}

/// Rounds each note's duration to the nearest whole number of `step`s, at least one.
/// Returns how many notes changed.
pub fn quantize_lengths(track: &mut Track, ids: &BTreeSet<NoteId>, step: Tick) -> usize {
    let step = step.get().max(1);
    let mut changed = 0;
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        let steps = ((n.duration.get() as f64 / step as f64).round() as u64).max(1);
        let new = Tick(steps * step);
        if new != n.duration {
            n.duration = new;
            changed += 1;
        }
    }
    changed
}

/// Extends each note with an id in `ids` so it ends where the next note on the track
/// (any pitch, strictly later start) begins. Notes with nothing after them, or already
/// ending exactly there, are left alone. Returns how many notes changed.
pub fn legato(track: &mut Track, ids: &BTreeSet<NoteId>) -> usize {
    let mut starts: Vec<Tick> = track.notes.iter().map(|n| n.start).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut changed = 0;
    for n in track.notes.iter_mut().filter(|n| ids.contains(&n.id)) {
        let next = match starts.binary_search(&n.start) {
            Ok(i) => starts.get(i + 1),
            Err(i) => starts.get(i),
        };
        let Some(next) = next else { continue };
        let new = next.saturating_sub(n.start);
        if new != n.duration {
            n.duration = new;
            changed += 1;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> (Song, BTreeSet<NoteId>) {
        let mut s = Song::new_empty();
        let a = s.new_note(Tick(480), Tick(240), 60, 100);
        let b = s.new_note(Tick(960), Tick(240), 67, 100);
        s.tracks[0].insert_note(a);
        s.tracks[0].insert_note(b);
        (s, [a.id, b.id].into_iter().collect())
    }

    #[test]
    fn clamps() {
        let (s, _) = song();
        let notes = &s.tracks[0].notes;
        assert_eq!(clamp_time_shift(notes, -1000), -480);
        assert_eq!(clamp_time_shift(notes, -100), -100);
        assert_eq!(clamp_time_shift(notes, 5000), 5000);
        assert_eq!(clamp_pitch_shift(notes, 100), 60);
        assert_eq!(clamp_pitch_shift(notes, -100), -60);
        assert_eq!(clamp_pitch_shift(notes, 3), 3);
        assert_eq!(clamp_pitch_shift(&[], 3), 0);
    }

    #[test]
    fn shifts() {
        let (mut s, ids) = song();
        let moved = shift_time(&mut s.tracks[0], &ids, -480);
        assert_eq!(moved.len(), 2);
        assert_eq!(s.tracks[0].notes[0].start, Tick::ZERO);
        let moved = shift_pitch(&mut s.tracks[0], &ids, 12);
        assert_eq!(moved[0].pitch, 72);
        assert_eq!(s.tracks[0].notes[1].pitch, 79);
    }

    #[test]
    fn lengths_and_velocities() {
        let (mut s, ids) = song();
        assert_eq!(change_length(&mut s.tracks[0], &ids, -1000, Tick(60)), 2);
        assert!(s.tracks[0].notes.iter().all(|n| n.duration == Tick(60)));
        assert_eq!(change_length(&mut s.tracks[0], &ids, -1, Tick(60)), 0);
        assert_eq!(change_length(&mut s.tracks[0], &ids, 240, Tick(60)), 2);
        assert_eq!(s.tracks[0].notes[0].duration, Tick(300));

        assert_eq!(nudge_velocity(&mut s.tracks[0], &ids, 50), 2);
        assert_eq!(s.tracks[0].notes[0].velocity, 127);
        assert_eq!(nudge_velocity(&mut s.tracks[0], &ids, 5), 0);
        assert_eq!(set_velocity(&mut s.tracks[0], &ids, 0), 2);
        assert_eq!(s.tracks[0].notes[0].velocity, 1);
        assert_eq!(set_velocity(&mut s.tracks[0], &ids, 1), 0);
    }

    #[test]
    fn clip_and_paste() {
        let (mut s, ids) = song();
        let clip = clip_relative(&s.tracks[0], &ids);
        assert_eq!(clip[0].start, Tick::ZERO);
        assert_eq!(clip[1].start, Tick(480));
        let pasted = paste(&mut s, 0, &clip, Tick(3840));
        assert_eq!(pasted.len(), 2);
        assert_eq!(pasted[1].start, Tick(4320));
        assert_eq!(s.tracks[0].notes.len(), 4);
        assert!(pasted.iter().all(|p| !ids.contains(&p.id)));
        assert_eq!(group_end(&pasted, Tick::ZERO), Tick(4560));
        assert_eq!(group_end(&[], Tick(7)), Tick(7));
        // pasting on top replaces
        let again = paste(&mut s, 0, &clip, Tick(3840));
        assert_eq!(s.tracks[0].notes.len(), 4);
        assert!(s.tracks[0].note(again[0].id).is_some());
        assert!(s.tracks[0].note(pasted[0].id).is_none());
        assert!(paste(&mut s, 9, &clip, Tick(0)).is_empty());
    }

    #[test]
    fn set_length_and_quantize_lengths() {
        let (mut s, ids) = song();
        assert_eq!(set_length(&mut s.tracks[0], &ids, Tick(0)), 2);
        assert!(s.tracks[0].notes.iter().all(|n| n.duration == Tick(1)));
        assert_eq!(set_length(&mut s.tracks[0], &ids, Tick(1)), 0);
        assert_eq!(set_length(&mut s.tracks[0], &ids, Tick(300)), 2);
        // 300 rounds to 240 (one 1/16); 1 rounds up to the minimum of one step
        assert_eq!(quantize_lengths(&mut s.tracks[0], &ids, Tick(240)), 2);
        assert!(s.tracks[0].notes.iter().all(|n| n.duration == Tick(240)));
        assert_eq!(set_length(&mut s.tracks[0], &ids, Tick(1)), 2);
        assert_eq!(quantize_lengths(&mut s.tracks[0], &ids, Tick(240)), 2);
        assert!(s.tracks[0].notes.iter().all(|n| n.duration == Tick(240)));
        assert_eq!(quantize_lengths(&mut s.tracks[0], &ids, Tick(240)), 0);
    }

    #[test]
    fn ramp() {
        let mut s = Song::new_empty();
        let mut ids = BTreeSet::new();
        for (start, pitch) in [(0, 60), (240, 62), (240, 65), (480, 64), (720, 65)] {
            let n = s.new_note(Tick(start), Tick(100), pitch, 100);
            s.tracks[0].insert_note(n);
            ids.insert(n.id);
        }
        // the last note is already at 100, so four of five change
        assert_eq!(ramp_velocity(&mut s.tracks[0], &ids, 40, 100), 4);
        let vels: Vec<u8> = s.tracks[0].notes.iter().map(|n| n.velocity).collect();
        assert_eq!(vels, vec![40, 60, 60, 80, 100], "chords share a value");
        assert_eq!(ramp_velocity(&mut s.tracks[0], &ids, 100, 40), 5);
        let vels: Vec<u8> = s.tracks[0].notes.iter().map(|n| n.velocity).collect();
        assert_eq!(vels, vec![100, 80, 80, 60, 40]);
        // a single start tick gets `from`; 0 clamps to 1
        let one: BTreeSet<NoteId> = [NoteId(1)].into_iter().collect();
        assert_eq!(ramp_velocity(&mut s.tracks[0], &one, 0, 127), 1);
        assert_eq!(s.tracks[0].notes[0].velocity, 1);
    }

    #[test]
    fn xorshift_is_deterministic_and_bounded() {
        let mut a = XorShift64(XorShift64::DEFAULT_SEED);
        let mut b = XorShift64(XorShift64::DEFAULT_SEED);
        let xs: Vec<u64> = (0..5).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..5).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);
        assert!(xs.windows(2).all(|w| w[0] != w[1]));
        let mut z = XorShift64(0);
        assert_ne!(z.next_u64(), 0, "a zero state is nudged off zero");
        let mut r = XorShift64(7);
        for _ in 0..1000 {
            let v = r.symmetric(3);
            assert!((-3..=3).contains(&v), "{v}");
        }
        assert_eq!(r.symmetric(0), 0);
    }

    #[test]
    fn humanize_stays_in_range() {
        let (mut s, ids) = song();
        let mut rng = XorShift64(42);
        let mut moved_any = false;
        for _ in 0..20 {
            let touched = humanize(&mut s.tracks[0], &ids, 10, 30, &mut rng);
            assert_eq!(touched.len(), 2);
            for n in &s.tracks[0].notes {
                assert!((1..=127).contains(&n.velocity));
            }
            moved_any |= s.tracks[0].notes[0].start != Tick(480);
        }
        assert!(moved_any);
        // a note at tick 0 with extreme amounts stays a valid note (start can only
        // saturate at 0, velocity stays 1..=127)
        let mut s = Song::new_empty();
        let n = s.new_note(Tick(0), Tick(10), 60, 1);
        s.tracks[0].insert_note(n);
        let ids: BTreeSet<NoteId> = [n.id].into_iter().collect();
        for _ in 0..50 {
            humanize(&mut s.tracks[0], &ids, 127, 1000, &mut rng);
            assert!((1..=127).contains(&s.tracks[0].notes[0].velocity));
            assert!(s.tracks[0].notes[0].start <= Tick(1000 * 50));
        }
        // the same seed gives the same result
        let mut s1 = song().0;
        let mut s2 = song().0;
        let ids = all_two(&s1);
        humanize(&mut s1.tracks[0], &ids, 20, 20, &mut XorShift64(99));
        humanize(&mut s2.tracks[0], &ids, 20, 20, &mut XorShift64(99));
        assert_eq!(s1, s2);
    }

    fn all_two(s: &Song) -> BTreeSet<NoteId> {
        s.tracks[0].notes.iter().map(|n| n.id).collect()
    }

    #[test]
    fn quantize_starts_with_strength_and_swing() {
        let mut s = Song::new_empty();
        let mut ids = BTreeSet::new();
        for start in [10, 230, 500, 1180] {
            let n = s.new_note(Tick(start), Tick(100), 60, 100);
            s.tracks[0].insert_note(n);
            ids.insert(n.id);
        }
        let g = Grid::default(); // 240
        let mut full = s.tracks[0].clone();
        assert_eq!(quantize_starts(&mut full, &ids, g, 960, 100, 0.0), 4);
        let starts: Vec<u64> = full.notes.iter().map(|n| n.start.get()).collect();
        assert_eq!(starts, vec![0, 240, 480, 1200]);
        assert_eq!(quantize_starts(&mut full, &ids, g, 960, 100, 0.0), 0);
        // half strength moves halfway (rounded)
        let mut half = s.tracks[0].clone();
        assert_eq!(quantize_starts(&mut half, &ids, g, 960, 50, 0.0), 4);
        let starts: Vec<u64> = half.notes.iter().map(|n| n.start.get()).collect();
        assert_eq!(starts, vec![5, 235, 490, 1190]);
        // zero strength is a no-op
        let mut none = s.tracks[0].clone();
        assert_eq!(quantize_starts(&mut none, &ids, g, 960, 0, 0.0), 0);
        // full swing delays the off-steps by half a step (240 -> 360, 1200 -> 1320)
        let mut swung = s.tracks[0].clone();
        quantize_starts(&mut swung, &ids, g, 960, 100, 1.0);
        let starts: Vec<u64> = swung.notes.iter().map(|n| n.start.get()).collect();
        assert_eq!(starts, vec![0, 360, 480, 1320]);
    }

    #[test]
    fn legato_extends_to_the_next_start() {
        let mut s = Song::new_empty();
        let mut ids = BTreeSet::new();
        for (start, pitch) in [(0, 60), (0, 64), (300, 62), (1000, 65)] {
            let n = s.new_note(Tick(start), Tick(100), pitch, 100);
            s.tracks[0].insert_note(n);
            ids.insert(n.id);
        }
        assert_eq!(legato(&mut s.tracks[0], &ids), 3);
        let durs: Vec<u64> = s.tracks[0].notes.iter().map(|n| n.duration.get()).collect();
        assert_eq!(durs, vec![300, 300, 700, 100], "the last note has nothing after it");
        assert_eq!(legato(&mut s.tracks[0], &ids), 0);
    }
}
