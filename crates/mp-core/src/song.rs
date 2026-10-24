//! The song data model: [`Song`] → [`Track`] → [`Note`], plus controller [`Lanes`].
//!
//! Everything here is plain data: `Clone + PartialEq + Debug + Serialize + Deserialize`.
//! Time is measured in [`Tick`]s at `Song::ppq` ticks per quarter note (960 by default);
//! converting ticks to seconds is the job of [`crate::time::TempoMap`].

use std::collections::BTreeSet;
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::time::{MeterMap, TempoMap};

/// Default pulses (ticks) per quarter note.
pub const DEFAULT_PPQ: u32 = 960;
/// Master reverb of a new song (and of project files saved before it existed).
pub const DEFAULT_MASTER_REVERB: f32 = 0.25;

fn default_master_reverb() -> f32 {
    DEFAULT_MASTER_REVERB
}

/// A position or duration in ticks. Never negative.
///
/// Prefer [`Tick::saturating_sub`] when subtracting positions so cursor and note
/// arithmetic can never underflow; plain `-` panics on underflow in debug builds
/// like an ordinary integer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tick(pub u64);

impl Tick {
    /// Tick zero: the very start of the song.
    pub const ZERO: Tick = Tick(0);

    /// Wraps a raw tick count.
    pub const fn new(ticks: u64) -> Self {
        Tick(ticks)
    }

    /// The raw tick count.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// `self - other`, stopping at zero instead of underflowing.
    pub const fn saturating_sub(self, other: Tick) -> Tick {
        Tick(self.0.saturating_sub(other.0))
    }

    /// `self + other`, stopping at `u64::MAX` instead of overflowing.
    pub const fn saturating_add(self, other: Tick) -> Tick {
        Tick(self.0.saturating_add(other.0))
    }

    /// `self - other`, or `None` if the result would be negative.
    pub const fn checked_sub(self, other: Tick) -> Option<Tick> {
        match self.0.checked_sub(other.0) {
            Some(t) => Some(Tick(t)),
            None => None,
        }
    }

    /// Multiplies the tick count by an integer factor (saturating).
    pub const fn mul(self, factor: u64) -> Tick {
        Tick(self.0.saturating_mul(factor))
    }
}

impl Add for Tick {
    type Output = Tick;
    fn add(self, rhs: Tick) -> Tick {
        Tick(self.0 + rhs.0)
    }
}

impl AddAssign for Tick {
    fn add_assign(&mut self, rhs: Tick) {
        self.0 += rhs.0;
    }
}

impl Sub for Tick {
    type Output = Tick;
    fn sub(self, rhs: Tick) -> Tick {
        Tick(self.0 - rhs.0)
    }
}

impl SubAssign for Tick {
    fn sub_assign(&mut self, rhs: Tick) {
        self.0 -= rhs.0;
    }
}

impl fmt::Display for Tick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifies one note for the lifetime of a song. Allocated by [`Song::alloc_note_id`];
/// never reused, so selections and undo snapshots can refer to notes safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoteId(pub u64);

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// One note on a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    /// Absolute start position.
    pub start: Tick,
    /// Length in ticks; always at least 1.
    pub duration: Tick,
    /// MIDI key number, 0..=127 (60 = middle C).
    pub pitch: u8,
    /// MIDI velocity, 1..=127.
    pub velocity: u8,
}

impl Note {
    /// The first tick after the note: `start + duration`.
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }

    /// `true` if `tick` lies inside `[start, end)`.
    pub fn covers(&self, tick: Tick) -> bool {
        self.start <= tick && tick < self.end()
    }

    /// Sort key used by [`Track::sort_notes`]: by start, then pitch, then id.
    pub fn sort_key(&self) -> (Tick, u8, NoteId) {
        (self.start, self.pitch, self.id)
    }
}

/// What produces sound for a track. An enum so a `Plugin` variant can be added later
/// without changing the project file format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Instrument {
    /// A preset inside a SoundFont (.sf2) file.
    SoundFont {
        /// Path to the `.sf2` file. Empty means "no instrument chosen yet".
        path: PathBuf,
        bank: u16,
        preset: u16,
    },
}

impl Default for Instrument {
    fn default() -> Self {
        Instrument::SoundFont { path: PathBuf::new(), bank: 0, preset: 0 }
    }
}

/// One automation point in a lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanePoint {
    pub tick: Tick,
    /// Value in the lane's own range (0..=127 for CC lanes, -8192..=8191 for pitch bend).
    pub value: i32,
}

/// Which controller lane of a track: the five fields of [`Lanes`] as a value, so the
/// editor can show and edit "the current lane" without five copies of every command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneKind {
    /// Sustain pedal (CC64), 0..=127; 64 and above holds the pedal down.
    Sustain,
    /// Pitch bend, -8192..=8191 (0 = no bend).
    PitchBend,
    /// Modulation wheel (CC1), 0..=127.
    ModWheel,
    /// Volume automation (CC7), 0..=127.
    Volume,
    /// Pan automation (CC10), 0..=127 (64 = centre).
    Pan,
}

/// Why a lane name could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown lane {0:?}: expected sustain, bend, mod, volume or pan")]
pub struct LaneKindParseError(pub String);

impl LaneKind {
    /// All lanes, in display order (the order `cycle_lane` walks).
    pub const ALL: [LaneKind; 5] =
        [LaneKind::Sustain, LaneKind::PitchBend, LaneKind::ModWheel, LaneKind::Volume, LaneKind::Pan];

    /// The snake_case name used in TOML and on the command line (`"pitch_bend"`).
    pub fn name(self) -> &'static str {
        match self {
            LaneKind::Sustain => "sustain",
            LaneKind::PitchBend => "pitch_bend",
            LaneKind::ModWheel => "mod_wheel",
            LaneKind::Volume => "volume",
            LaneKind::Pan => "pan",
        }
    }

    /// The inclusive `(min, max)` a point's value may take.
    pub fn range(self) -> (i32, i32) {
        match self {
            LaneKind::PitchBend => (-8192, 8191),
            LaneKind::Sustain | LaneKind::ModWheel | LaneKind::Volume | LaneKind::Pan => (0, 127),
        }
    }

    /// The value in force where a lane has no point yet: pedal up, no bend, no
    /// modulation, volume 100, pan centred.
    pub fn default_value(self) -> i32 {
        match self {
            LaneKind::Sustain | LaneKind::PitchBend | LaneKind::ModWheel => 0,
            LaneKind::Volume => 100,
            LaneKind::Pan => 64,
        }
    }

    /// Clamps `value` into [`LaneKind::range`].
    pub fn clamp(self, value: i32) -> i32 {
        let (lo, hi) = self.range();
        value.clamp(lo, hi)
    }

    /// The lane after this one in [`LaneKind::ALL`], wrapping.
    pub fn next(self) -> LaneKind {
        let i = LaneKind::ALL.iter().position(|k| *k == self).unwrap_or(0);
        LaneKind::ALL[(i + 1) % LaneKind::ALL.len()]
    }

    /// The lane before this one in [`LaneKind::ALL`], wrapping.
    pub fn prev(self) -> LaneKind {
        let i = LaneKind::ALL.iter().position(|k| *k == self).unwrap_or(0);
        LaneKind::ALL[(i + LaneKind::ALL.len() - 1) % LaneKind::ALL.len()]
    }
}

impl fmt::Display for LaneKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LaneKind::Sustain => "sustain",
            LaneKind::PitchBend => "pitch bend",
            LaneKind::ModWheel => "mod wheel",
            LaneKind::Volume => "volume",
            LaneKind::Pan => "pan",
        })
    }
}

impl FromStr for LaneKind {
    type Err = LaneKindParseError;

    /// Accepts the snake_case names, the display names and the short forms `bend` and
    /// `mod` (case-insensitive, whitespace trimmed).
    fn from_str(s: &str) -> Result<Self, LaneKindParseError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sustain" | "pedal" | "cc64" => Ok(LaneKind::Sustain),
            "pitch_bend" | "pitch bend" | "bend" | "pitchbend" => Ok(LaneKind::PitchBend),
            "mod_wheel" | "mod wheel" | "mod" | "modwheel" | "cc1" => Ok(LaneKind::ModWheel),
            "volume" | "vol" | "cc7" => Ok(LaneKind::Volume),
            "pan" | "cc10" => Ok(LaneKind::Pan),
            _ => Err(LaneKindParseError(s.to_string())),
        }
    }
}

/// Per-track controller lanes. Each is kept sorted by tick (see [`Lanes::sort`]); the
/// editing methods keep it that way and allow at most one point per tick.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lanes {
    /// Sustain pedal (CC64), 0..=127.
    pub sustain: Vec<LanePoint>,
    /// Pitch bend, -8192..=8191.
    pub pitch_bend: Vec<LanePoint>,
    /// Modulation wheel (CC1), 0..=127.
    pub mod_wheel: Vec<LanePoint>,
    /// Volume automation (CC7), 0..=127.
    pub volume: Vec<LanePoint>,
    /// Pan automation (CC10), 0..=127.
    pub pan: Vec<LanePoint>,
}

impl Lanes {
    /// Sorts every lane by tick (stable, so equal ticks keep insertion order).
    pub fn sort(&mut self) {
        for kind in LaneKind::ALL {
            self.get_mut(kind).sort_by_key(|p| p.tick);
        }
    }

    /// The points of one lane.
    pub fn get(&self, kind: LaneKind) -> &Vec<LanePoint> {
        match kind {
            LaneKind::Sustain => &self.sustain,
            LaneKind::PitchBend => &self.pitch_bend,
            LaneKind::ModWheel => &self.mod_wheel,
            LaneKind::Volume => &self.volume,
            LaneKind::Pan => &self.pan,
        }
    }

    /// The points of one lane, mutably. Call [`Lanes::sort`] after reordering edits.
    pub fn get_mut(&mut self, kind: LaneKind) -> &mut Vec<LanePoint> {
        match kind {
            LaneKind::Sustain => &mut self.sustain,
            LaneKind::PitchBend => &mut self.pitch_bend,
            LaneKind::ModWheel => &mut self.mod_wheel,
            LaneKind::Volume => &mut self.volume,
            LaneKind::Pan => &mut self.pan,
        }
    }

    /// The value of the point exactly at `tick`, if there is one.
    pub fn point_at(&self, kind: LaneKind, tick: Tick) -> Option<i32> {
        self.get(kind).iter().find(|p| p.tick == tick).map(|p| p.value)
    }

    /// Puts a point at `tick`, replacing any point already there, clamped to the
    /// lane's range; the lane stays sorted. Returns the value stored.
    pub fn set_point(&mut self, kind: LaneKind, tick: Tick, value: i32) -> i32 {
        let value = kind.clamp(value);
        let lane = self.get_mut(kind);
        lane.retain(|p| p.tick != tick);
        let at = lane.partition_point(|p| p.tick < tick);
        lane.insert(at, LanePoint { tick, value });
        value
    }

    /// Removes the points with `from <= tick < to`. Returns how many were removed.
    pub fn remove_range(&mut self, kind: LaneKind, from: Tick, to: Tick) -> usize {
        let lane = self.get_mut(kind);
        let before = lane.len();
        lane.retain(|p| !(p.tick >= from && p.tick < to));
        before - lane.len()
    }

    /// The value in force at `tick`: the last point at or before it, or the lane's
    /// [`LaneKind::default_value`] if there is none.
    pub fn value_at(&self, kind: LaneKind, tick: Tick) -> i32 {
        self.get(kind).iter().rev().find(|p| p.tick <= tick).map(|p| p.value).unwrap_or(kind.default_value())
    }
}

/// One instrument track: a continuous stream of notes plus mixer state and lanes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub instrument: Instrument,
    /// Linear gain, 1.0 = unity.
    pub volume: f32,
    /// -1.0 = hard left, 0.0 = centre, 1.0 = hard right.
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    /// MIDI channel 0..=15 (9 selects drum kits in GM SoundFonts).
    pub channel: u8,
    /// Kept sorted by `(start, pitch, id)`; call [`Track::sort_notes`] after editing.
    pub notes: Vec<Note>,
    pub lanes: Lanes,
}

impl Track {
    /// An empty track with unity volume, centre pan, channel 0 and no instrument.
    pub fn new(name: impl Into<String>) -> Self {
        Track {
            name: name.into(),
            instrument: Instrument::default(),
            volume: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            channel: 0,
            notes: Vec::new(),
            lanes: Lanes::default(),
        }
    }

    /// Restores the `(start, pitch, id)` ordering invariant. Idempotent and cheap.
    pub fn sort_notes(&mut self) {
        self.notes.sort_by_key(Note::sort_key);
    }

    /// Finds a note by id.
    pub fn note(&self, id: NoteId) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    /// Finds a note by id, mutably. Re-sort afterwards if you change `start` or `pitch`.
    pub fn note_mut(&mut self, id: NoteId) -> Option<&mut Note> {
        self.notes.iter_mut().find(|n| n.id == id)
    }

    /// The note that starts exactly at `start` with pitch `pitch`, if any.
    pub fn note_at(&self, start: Tick, pitch: u8) -> Option<&Note> {
        self.notes.iter().find(|n| n.start == start && n.pitch == pitch)
    }

    /// The note of pitch `pitch` that sounds at `tick` (starts at or before it and has
    /// not ended). If several overlap, the one that started last wins.
    pub fn note_sounding_at(&self, tick: Tick, pitch: u8) -> Option<&Note> {
        self.notes.iter().rfind(|n| n.pitch == pitch && n.covers(tick))
    }

    /// Inserts a note, removing any existing note with the same `start` and `pitch`
    /// first, then re-sorts. Returns the ids of the notes it replaced.
    pub fn insert_note(&mut self, note: Note) -> Vec<NoteId> {
        let replaced: Vec<NoteId> = self
            .notes
            .iter()
            .filter(|n| n.start == note.start && n.pitch == note.pitch)
            .map(|n| n.id)
            .collect();
        self.notes.retain(|n| !(n.start == note.start && n.pitch == note.pitch));
        self.notes.push(note);
        self.sort_notes();
        replaced
    }

    /// Removes every note whose id is in `ids`. Returns how many were removed.
    pub fn remove_notes(&mut self, ids: &BTreeSet<NoteId>) -> usize {
        let before = self.notes.len();
        self.notes.retain(|n| !ids.contains(&n.id));
        before - self.notes.len()
    }

    /// The end of the last note (or tick 0 for an empty track).
    pub fn end(&self) -> Tick {
        self.notes.iter().map(Note::end).max().unwrap_or(Tick::ZERO)
    }
}

/// A whole project: timing maps plus tracks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Song {
    /// Project-file layout this song was saved with ([`crate::project::FORMAT_VERSION`]).
    /// Files saved before the stamp existed read as 0.
    #[serde(default)]
    pub format_version: u32,
    /// Ticks per quarter note. Fixed at creation (960).
    pub ppq: u32,
    pub tempo_map: TempoMap,
    pub meter_map: MeterMap,
    pub tracks: Vec<Track>,
    /// Next id handed out by [`Song::alloc_note_id`].
    pub next_note_id: u64,
    /// Master reverb send, 0.0 (dry) ..= 1.0. Missing in older project files, which
    /// load with [`DEFAULT_MASTER_REVERB`].
    #[serde(default = "default_master_reverb")]
    pub master_reverb: f32,
}

impl Song {
    /// One empty track named "Track 1", 120 BPM, 4/4, 960 PPQ.
    pub fn new_empty() -> Self {
        Song {
            format_version: crate::project::FORMAT_VERSION,
            ppq: DEFAULT_PPQ,
            tempo_map: TempoMap::constant(120.0),
            meter_map: MeterMap::constant(4, 4),
            tracks: vec![Track::new("Track 1")],
            next_note_id: 1,
            master_reverb: DEFAULT_MASTER_REVERB,
        }
    }

    /// Hands out a fresh, never-before-used note id.
    pub fn alloc_note_id(&mut self) -> NoteId {
        let id = NoteId(self.next_note_id);
        self.next_note_id += 1;
        id
    }

    /// Builds a note with a fresh id. Does not insert it anywhere.
    pub fn new_note(&mut self, start: Tick, duration: Tick, pitch: u8, velocity: u8) -> Note {
        let id = self.alloc_note_id();
        Note { id, start, duration, pitch, velocity }
    }

    /// Borrows a track by index.
    pub fn track(&self, index: usize) -> Option<&Track> {
        self.tracks.get(index)
    }

    /// Borrows a track by index, mutably.
    pub fn track_mut(&mut self, index: usize) -> Option<&mut Track> {
        self.tracks.get_mut(index)
    }

    /// Appends a track and returns its index.
    pub fn add_track(&mut self, track: Track) -> usize {
        self.tracks.push(track);
        self.tracks.len() - 1
    }

    /// Inserts a track at `index` (clamped to the end) and returns its index.
    pub fn insert_track(&mut self, index: usize, track: Track) -> usize {
        let index = index.min(self.tracks.len());
        self.tracks.insert(index, track);
        index
    }

    /// Removes and returns the track at `index`, or `None` if there is no such track.
    pub fn remove_track(&mut self, index: usize) -> Option<Track> {
        (index < self.tracks.len()).then(|| self.tracks.remove(index))
    }

    /// `"Track N"` for the next track added, where N is one more than the track count.
    pub fn next_track_name(&self) -> String {
        format!("Track {}", self.tracks.len() + 1)
    }

    /// A copy of the track at `index` named `"<name> copy"`, its notes given fresh
    /// ids. Does not insert it. `None` if there is no such track.
    pub fn duplicate_track(&mut self, index: usize) -> Option<Track> {
        let mut copy = self.tracks.get(index)?.clone();
        copy.name = format!("{} copy", copy.name);
        for note in &mut copy.notes {
            note.id = self.alloc_note_id();
        }
        Some(copy)
    }

    /// Sorts the notes and lanes of every track.
    pub fn normalize(&mut self) {
        for t in &mut self.tracks {
            t.sort_notes();
            t.lanes.sort();
        }
    }

    /// The end of the last note on any track.
    pub fn end(&self) -> Tick {
        self.tracks.iter().map(Track::end).max().unwrap_or(Tick::ZERO)
    }
}

impl Default for Song {
    fn default() -> Self {
        Song::new_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_arithmetic() {
        assert_eq!(Tick(5) + Tick(3), Tick(8));
        assert_eq!(Tick(5) - Tick(3), Tick(2));
        assert_eq!(Tick(3).saturating_sub(Tick(5)), Tick::ZERO);
        assert_eq!(Tick(3).checked_sub(Tick(5)), None);
        assert_eq!(Tick(3).mul(4), Tick(12));
        let mut t = Tick(1);
        t += Tick(1);
        t -= Tick(2);
        assert_eq!(t, Tick::ZERO);
    }

    #[test]
    fn new_empty_song() {
        let s = Song::new_empty();
        assert_eq!(s.ppq, 960);
        assert_eq!(s.tracks.len(), 1);
        assert_eq!(s.tracks[0].name, "Track 1");
        assert_eq!(s.tempo_map.bpm_at(Tick::ZERO), 120.0);
        assert_eq!(s.meter_map.meter_at(Tick::ZERO), (4, 4));
    }

    #[test]
    fn note_ids_are_unique() {
        let mut s = Song::new_empty();
        let a = s.alloc_note_id();
        let b = s.alloc_note_id();
        assert_ne!(a, b);
        assert_eq!(s.next_note_id, 3);
    }

    #[test]
    fn insert_replaces_same_start_and_pitch() {
        let mut s = Song::new_empty();
        let n1 = s.new_note(Tick(960), Tick(240), 60, 100);
        let n2 = s.new_note(Tick(0), Tick(240), 64, 100);
        let n3 = s.new_note(Tick(960), Tick(480), 60, 50);
        let t = &mut s.tracks[0];
        assert!(t.insert_note(n1).is_empty());
        assert!(t.insert_note(n2).is_empty());
        assert_eq!(t.insert_note(n3), vec![n1.id]);
        assert_eq!(t.notes.len(), 2);
        // sorted by start
        assert_eq!(t.notes[0].id, n2.id);
        assert_eq!(t.notes[1].id, n3.id);
        assert_eq!(t.note_at(Tick(960), 60).map(|n| n.velocity), Some(50));
        assert!(t.note_sounding_at(Tick(1000), 60).is_some());
        assert!(t.note_sounding_at(Tick(1440), 60).is_none());
        assert_eq!(t.end(), Tick(1440));
    }

    #[test]
    fn remove_notes_by_id() {
        let mut s = Song::new_empty();
        let n1 = s.new_note(Tick(0), Tick(10), 60, 100);
        let n2 = s.new_note(Tick(0), Tick(10), 62, 100);
        s.tracks[0].insert_note(n1);
        s.tracks[0].insert_note(n2);
        let ids: BTreeSet<NoteId> = [n1.id].into_iter().collect();
        assert_eq!(s.tracks[0].remove_notes(&ids), 1);
        assert_eq!(s.tracks[0].notes.len(), 1);
        assert_eq!(s.tracks[0].notes[0].id, n2.id);
    }

    #[test]
    fn lanes_sort() {
        let mut l = Lanes {
            sustain: vec![LanePoint { tick: Tick(5), value: 1 }, LanePoint { tick: Tick(1), value: 0 }],
            ..Lanes::default()
        };
        l.sort();
        assert_eq!(l.sustain[0].tick, Tick(1));
    }

    #[test]
    fn lane_kind_names_ranges_and_cycling() {
        for k in LaneKind::ALL {
            assert_eq!(k.name().parse::<LaneKind>(), Ok(k));
            assert_eq!(k.to_string().parse::<LaneKind>(), Ok(k), "{k}");
            assert_eq!(k.next().prev(), k);
            let (lo, hi) = k.range();
            assert!(lo <= k.default_value() && k.default_value() <= hi);
            // serde spelling is the snake_case name
            assert_eq!(ron::to_string(&k).unwrap(), k.name());
        }
        assert_eq!("bend".parse::<LaneKind>(), Ok(LaneKind::PitchBend));
        assert_eq!(" MOD ".parse::<LaneKind>(), Ok(LaneKind::ModWheel));
        assert_eq!("cc64".parse::<LaneKind>(), Ok(LaneKind::Sustain));
        assert_eq!("x".parse::<LaneKind>(), Err(LaneKindParseError("x".into())));
        assert_eq!(LaneKind::Pan.next(), LaneKind::Sustain);
        assert_eq!(LaneKind::Sustain.prev(), LaneKind::Pan);
        assert_eq!(LaneKind::PitchBend.range(), (-8192, 8191));
        assert_eq!(LaneKind::Volume.range(), (0, 127));
        assert_eq!(LaneKind::PitchBend.clamp(-9000), -8192);
        assert_eq!(LaneKind::Sustain.clamp(200), 127);
        assert_eq!(LaneKind::Volume.default_value(), 100);
        assert_eq!(LaneKind::Pan.default_value(), 64);
    }

    #[test]
    fn lane_points_set_remove_and_value_at() {
        let mut l = Lanes::default();
        assert_eq!(l.value_at(LaneKind::Sustain, Tick(50)), 0);
        assert_eq!(l.value_at(LaneKind::Volume, Tick(50)), 100);
        assert_eq!(l.set_point(LaneKind::Sustain, Tick(100), 127), 127);
        assert_eq!(l.set_point(LaneKind::Sustain, Tick(10), 300), 127, "clamped");
        assert_eq!(l.set_point(LaneKind::Sustain, Tick(50), 0), 0);
        let ticks: Vec<u64> = l.sustain.iter().map(|p| p.tick.get()).collect();
        assert_eq!(ticks, vec![10, 50, 100], "kept sorted");
        // replacing a point at the same tick
        l.set_point(LaneKind::Sustain, Tick(50), 64);
        assert_eq!(l.sustain.len(), 3);
        assert_eq!(l.point_at(LaneKind::Sustain, Tick(50)), Some(64));
        assert_eq!(l.point_at(LaneKind::Sustain, Tick(51)), None);
        assert_eq!(l.value_at(LaneKind::Sustain, Tick(9)), 0);
        assert_eq!(l.value_at(LaneKind::Sustain, Tick(10)), 127);
        assert_eq!(l.value_at(LaneKind::Sustain, Tick(99)), 64);
        assert_eq!(l.value_at(LaneKind::Sustain, Tick(5000)), 127);
        assert_eq!(l.get(LaneKind::Sustain).len(), 3);
        assert!(l.get(LaneKind::Pan).is_empty());
        // pitch bend has its own range
        assert_eq!(l.set_point(LaneKind::PitchBend, Tick(0), -9000), -8192);
        assert_eq!(l.get(LaneKind::PitchBend)[0].value, -8192);
        // remove [from, to)
        assert_eq!(l.remove_range(LaneKind::Sustain, Tick(10), Tick(100)), 2);
        assert_eq!(l.sustain.len(), 1);
        assert_eq!(l.sustain[0].tick, Tick(100));
        assert_eq!(l.remove_range(LaneKind::Sustain, Tick(0), Tick(100)), 0);
        l.get_mut(LaneKind::Pan).push(LanePoint { tick: Tick(1), value: 0 });
        assert_eq!(l.pan.len(), 1);
    }

    #[test]
    fn track_insert_remove_duplicate_and_names() {
        let mut s = Song::new_empty();
        let n = s.new_note(Tick(0), Tick(10), 60, 100);
        s.tracks[0].insert_note(n);
        assert_eq!(s.next_track_name(), "Track 2");
        assert_eq!(s.insert_track(99, Track::new(s.next_track_name())), 1);
        assert_eq!(s.tracks[1].name, "Track 2");
        assert_eq!(s.insert_track(0, Track::new("First")), 0);
        assert_eq!(s.tracks[0].name, "First");
        let copy = s.duplicate_track(1).unwrap();
        assert_eq!(copy.name, "Track 1 copy");
        assert_eq!(copy.notes.len(), 1);
        assert_ne!(copy.notes[0].id, n.id, "fresh ids");
        assert_eq!(copy.notes[0].pitch, 60);
        assert!(s.duplicate_track(7).is_none());
        assert_eq!(s.remove_track(0).map(|t| t.name), Some("First".to_string()));
        assert!(s.remove_track(5).is_none());
        assert_eq!(s.tracks.len(), 2);
    }

    #[test]
    fn master_reverb_defaults_when_missing() {
        let s = Song::new_empty();
        assert_eq!(s.master_reverb, DEFAULT_MASTER_REVERB);
        let text = ron::to_string(&s).unwrap();
        assert!(text.contains("master_reverb"));
        let stripped = text.replace(",master_reverb:0.25", "");
        assert_ne!(stripped, text, "the field was removed from the text");
        let back: Song = ron::from_str(&stripped).unwrap();
        assert_eq!(back.master_reverb, DEFAULT_MASTER_REVERB);
    }
}
