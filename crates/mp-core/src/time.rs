//! Musical time: tempo map (ticks ↔ seconds), meter map (bars and beats) and the
//! editing [`Grid`] with swing-aware snapping.
//!
//! All functions take `ppq` (ticks per quarter note) explicitly so this module has
//! no dependency on [`crate::Song`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::song::Tick;

/// Problems building a tempo or meter map, or parsing a grid.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TimeError {
    #[error("map must have at least one point")]
    Empty,
    #[error("first point must be at tick 0, found tick {0}")]
    FirstNotAtZero(u64),
    #[error("points must be strictly increasing by tick (tick {0} repeated or out of order)")]
    NotSorted(u64),
    #[error("tempo must be a positive, finite BPM, found {0}")]
    BadTempo(String),
    #[error("meter {0}/{1} is invalid (numerator must be ≥ 1, denominator a power of two from 1 to 64)")]
    BadMeter(u8, u8),
    #[error("cannot parse grid {0:?}: expected e.g. \"1/16\", \"1/8T\" or \"1/4.\"")]
    BadGrid(String),
}

/// A tempo change: from `tick` onwards the tempo is `bpm` quarter notes per minute.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TempoPoint {
    pub tick: Tick,
    pub bpm: f64,
}

/// Piecewise-constant tempo. Always has a point at tick 0; points are strictly
/// increasing by tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<TempoPoint>", into = "Vec<TempoPoint>")]
pub struct TempoMap {
    points: Vec<TempoPoint>,
}

impl TempoMap {
    /// A single tempo for the whole song.
    pub fn constant(bpm: f64) -> Self {
        TempoMap::from_points(vec![TempoPoint { tick: Tick::ZERO, bpm }])
            .expect("constant tempo map is always valid for a positive bpm")
    }

    /// Validates and wraps a list of points (must be sorted, first at tick 0, bpm > 0).
    pub fn from_points(points: Vec<TempoPoint>) -> Result<Self, TimeError> {
        validate_ticks(points.iter().map(|p| p.tick))?;
        for p in &points {
            if !(p.bpm.is_finite() && p.bpm > 0.0) {
                return Err(TimeError::BadTempo(p.bpm.to_string()));
            }
        }
        Ok(TempoMap { points })
    }

    /// The points, sorted by tick.
    pub fn points(&self) -> &[TempoPoint] {
        &self.points
    }

    /// Sets the tempo from `tick` onwards, replacing a point already at that tick.
    pub fn set_tempo_at(&mut self, tick: Tick, bpm: f64) -> Result<(), TimeError> {
        if !(bpm.is_finite() && bpm > 0.0) {
            return Err(TimeError::BadTempo(bpm.to_string()));
        }
        match self.points.binary_search_by_key(&tick, |p| p.tick) {
            Ok(i) => self.points[i].bpm = bpm,
            Err(i) => self.points.insert(i, TempoPoint { tick, bpm }),
        }
        Ok(())
    }

    /// Index of the segment containing `tick`.
    fn segment_index(&self, tick: Tick) -> usize {
        match self.points.binary_search_by_key(&tick, |p| p.tick) {
            Ok(i) => i,
            Err(i) => i - 1, // i >= 1 because points[0].tick == 0 <= tick
        }
    }

    /// The tempo in effect at `tick`.
    pub fn bpm_at(&self, tick: Tick) -> f64 {
        self.points[self.segment_index(tick)].bpm
    }

    /// Absolute time in seconds of `tick`, honouring every tempo segment before it.
    pub fn tick_to_seconds(&self, ppq: u32, tick: Tick) -> f64 {
        let ppq = f64::from(ppq);
        let mut secs = 0.0;
        for (i, p) in self.points.iter().enumerate() {
            if p.tick >= tick {
                break;
            }
            let seg_end = self.points.get(i + 1).map(|n| n.tick).unwrap_or(tick).min(tick);
            let ticks = (seg_end.get() - p.tick.get()) as f64;
            secs += ticks * 60.0 / (p.bpm * ppq);
        }
        secs
    }

    /// The tick at absolute time `secs` (rounded to the nearest tick; negative times
    /// give tick 0).
    pub fn seconds_to_tick(&self, ppq: u32, secs: f64) -> Tick {
        if secs.is_nan() || secs <= 0.0 {
            return Tick::ZERO;
        }
        let ppq = f64::from(ppq);
        let mut seg_start_secs = 0.0;
        for (i, p) in self.points.iter().enumerate() {
            let ticks_per_sec = p.bpm * ppq / 60.0;
            match self.points.get(i + 1) {
                Some(next) => {
                    let seg_len_ticks = (next.tick.get() - p.tick.get()) as f64;
                    let seg_end_secs = seg_start_secs + seg_len_ticks / ticks_per_sec;
                    if secs < seg_end_secs {
                        let t = p.tick.get() as f64 + (secs - seg_start_secs) * ticks_per_sec;
                        return Tick(t.round() as u64);
                    }
                    seg_start_secs = seg_end_secs;
                }
                None => {
                    let t = p.tick.get() as f64 + (secs - seg_start_secs) * ticks_per_sec;
                    return Tick(t.round() as u64);
                }
            }
        }
        Tick::ZERO
    }
}

impl TryFrom<Vec<TempoPoint>> for TempoMap {
    type Error = TimeError;
    fn try_from(points: Vec<TempoPoint>) -> Result<Self, TimeError> {
        TempoMap::from_points(points)
    }
}

impl From<TempoMap> for Vec<TempoPoint> {
    fn from(m: TempoMap) -> Self {
        m.points
    }
}

/// A time-signature change: from `tick` onwards the meter is `numerator/denominator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterPoint {
    pub tick: Tick,
    pub numerator: u8,
    pub denominator: u8,
}

/// Bar/beat position of a tick. `bar` and `beat` are 1-based, as shown to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarBeat {
    pub bar: u32,
    pub beat: u32,
    /// Ticks past the start of `beat`.
    pub tick_in_beat: u64,
}

impl fmt::Display for BarBeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{:03}", self.bar, self.beat, self.tick_in_beat)
    }
}

/// Piecewise-constant time signature. Always has a point at tick 0.
///
/// If a meter change does not land on a bar boundary of the previous meter, the
/// previous segment's last bar is simply short.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<MeterPoint>", into = "Vec<MeterPoint>")]
pub struct MeterMap {
    points: Vec<MeterPoint>,
}

fn validate_meter(numerator: u8, denominator: u8) -> Result<(), TimeError> {
    let ok = numerator >= 1 && (1..=64).contains(&denominator) && denominator.is_power_of_two();
    if ok {
        Ok(())
    } else {
        Err(TimeError::BadMeter(numerator, denominator))
    }
}

impl MeterMap {
    /// A single time signature for the whole song.
    pub fn constant(numerator: u8, denominator: u8) -> Self {
        MeterMap::from_points(vec![MeterPoint { tick: Tick::ZERO, numerator, denominator }])
            .expect("constant meter map is valid for a sane signature")
    }

    /// Validates and wraps a list of points (sorted, first at tick 0, sane signatures).
    pub fn from_points(points: Vec<MeterPoint>) -> Result<Self, TimeError> {
        validate_ticks(points.iter().map(|p| p.tick))?;
        for p in &points {
            validate_meter(p.numerator, p.denominator)?;
        }
        Ok(MeterMap { points })
    }

    /// The points, sorted by tick.
    pub fn points(&self) -> &[MeterPoint] {
        &self.points
    }

    /// Sets the meter from `tick` onwards, replacing a point already at that tick.
    pub fn set_meter_at(&mut self, tick: Tick, numerator: u8, denominator: u8) -> Result<(), TimeError> {
        validate_meter(numerator, denominator)?;
        let p = MeterPoint { tick, numerator, denominator };
        match self.points.binary_search_by_key(&tick, |p| p.tick) {
            Ok(i) => self.points[i] = p,
            Err(i) => self.points.insert(i, p),
        }
        Ok(())
    }

    fn segment_index(&self, tick: Tick) -> usize {
        match self.points.binary_search_by_key(&tick, |p| p.tick) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// The `(numerator, denominator)` in effect at `tick`.
    pub fn meter_at(&self, tick: Tick) -> (u8, u8) {
        let p = self.points[self.segment_index(tick)];
        (p.numerator, p.denominator)
    }

    /// Length of one beat in ticks for the meter at `tick`.
    pub fn beat_ticks_at(&self, ppq: u32, tick: Tick) -> Tick {
        let (_, den) = self.meter_at(tick);
        beat_ticks(ppq, den)
    }

    /// Length of one bar in ticks for the meter at `tick`.
    pub fn bar_ticks_at(&self, ppq: u32, tick: Tick) -> Tick {
        let (num, den) = self.meter_at(tick);
        Tick(beat_ticks(ppq, den).get() * u64::from(num))
    }

    /// Converts a tick to a 1-based bar/beat position.
    pub fn bar_beat_of(&self, ppq: u32, tick: Tick) -> BarBeat {
        let mut bars_before: u64 = 0;
        for (i, p) in self.points.iter().enumerate() {
            let beat_len = beat_ticks(ppq, p.denominator).get();
            let bar_len = beat_len * u64::from(p.numerator);
            let next_start = self.points.get(i + 1).map(|n| n.tick);
            if let Some(next) = next_start {
                if tick >= next {
                    bars_before += (next.get() - p.tick.get()).div_ceil(bar_len);
                    continue;
                }
            }
            let offset = tick.get() - p.tick.get();
            let bar = bars_before + offset / bar_len;
            let in_bar = offset % bar_len;
            return BarBeat {
                bar: u32::try_from(bar + 1).unwrap_or(u32::MAX),
                beat: u32::try_from(in_bar / beat_len + 1).unwrap_or(u32::MAX),
                tick_in_beat: in_bar % beat_len,
            };
        }
        BarBeat { bar: 1, beat: 1, tick_in_beat: 0 }
    }

    /// The first tick of 1-based `bar`. Bar 0 is treated as bar 1.
    pub fn tick_of_bar(&self, ppq: u32, bar: u32) -> Tick {
        let target = u64::from(bar.max(1)) - 1; // 0-based
        let mut bars_before: u64 = 0;
        for (i, p) in self.points.iter().enumerate() {
            let bar_len = beat_ticks(ppq, p.denominator).get() * u64::from(p.numerator);
            let next_start = self.points.get(i + 1).map(|n| n.tick);
            if let Some(next) = next_start {
                let bars_in_seg = (next.get() - p.tick.get()).div_ceil(bar_len);
                if target >= bars_before + bars_in_seg {
                    bars_before += bars_in_seg;
                    continue;
                }
            }
            return Tick(p.tick.get() + (target - bars_before) * bar_len);
        }
        Tick::ZERO
    }
}

impl TryFrom<Vec<MeterPoint>> for MeterMap {
    type Error = TimeError;
    fn try_from(points: Vec<MeterPoint>) -> Result<Self, TimeError> {
        MeterMap::from_points(points)
    }
}

impl From<MeterMap> for Vec<MeterPoint> {
    fn from(m: MeterMap) -> Self {
        m.points
    }
}

/// Ticks in one beat of a `x/denominator` meter: a quarter is `ppq`, an eighth `ppq/2`.
pub fn beat_ticks(ppq: u32, denominator: u8) -> Tick {
    Tick(u64::from(ppq) * 4 / u64::from(denominator.max(1)))
}

fn validate_ticks(ticks: impl Iterator<Item = Tick>) -> Result<(), TimeError> {
    let mut prev: Option<Tick> = None;
    for t in ticks {
        match prev {
            None if t != Tick::ZERO => return Err(TimeError::FirstNotAtZero(t.get())),
            Some(p) if t <= p => return Err(TimeError::NotSorted(t.get())),
            _ => {}
        }
        prev = Some(t);
    }
    if prev.is_none() {
        return Err(TimeError::Empty);
    }
    Ok(())
}

/// The note value a grid is based on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Division {
    Whole,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    SixtyFourth,
}

impl Division {
    /// All divisions, longest first.
    pub const ALL: [Division; 7] = [
        Division::Whole,
        Division::Half,
        Division::Quarter,
        Division::Eighth,
        Division::Sixteenth,
        Division::ThirtySecond,
        Division::SixtyFourth,
    ];

    /// The denominator as written: 1, 2, 4, 8, 16, 32, 64.
    pub const fn denominator(self) -> u32 {
        match self {
            Division::Whole => 1,
            Division::Half => 2,
            Division::Quarter => 4,
            Division::Eighth => 8,
            Division::Sixteenth => 16,
            Division::ThirtySecond => 32,
            Division::SixtyFourth => 64,
        }
    }

    /// Parses a denominator (1, 2, 4, ..., 64).
    pub const fn from_denominator(d: u32) -> Option<Division> {
        match d {
            1 => Some(Division::Whole),
            2 => Some(Division::Half),
            4 => Some(Division::Quarter),
            8 => Some(Division::Eighth),
            16 => Some(Division::Sixteenth),
            32 => Some(Division::ThirtySecond),
            64 => Some(Division::SixtyFourth),
            _ => None,
        }
    }

    /// The next shorter division, wrapping from 1/64 back to 1/1.
    pub fn next(self) -> Division {
        let i = Division::ALL.iter().position(|d| *d == self).unwrap_or(0);
        Division::ALL[(i + 1) % Division::ALL.len()]
    }

    /// The next longer division, wrapping from 1/1 to 1/64.
    pub fn prev(self) -> Division {
        let i = Division::ALL.iter().position(|d| *d == self).unwrap_or(0);
        Division::ALL[(i + Division::ALL.len() - 1) % Division::ALL.len()]
    }
}

/// Plain, dotted (×1.5) or triplet (×2/3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GridModifier {
    #[default]
    Plain,
    Dotted,
    Triplet,
}

/// The editing grid: cursor step size and default note length.
///
/// Serializes as its display string (`"1/16"`, `"1/8T"`, `"1/4."`) so it reads
/// naturally in the keymap and config files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Grid {
    pub base: Division,
    pub modifier: GridModifier,
}

impl Default for Grid {
    /// Sixteenth notes.
    fn default() -> Self {
        Grid { base: Division::Sixteenth, modifier: GridModifier::Plain }
    }
}

impl Grid {
    /// A plain grid of the given division.
    pub const fn plain(base: Division) -> Self {
        Grid { base, modifier: GridModifier::Plain }
    }

    /// Length of one grid step in ticks. Exact for 960 PPQ at every setting.
    pub fn ticks(self, ppq: u32) -> Tick {
        let whole = u64::from(ppq) * 4;
        let (num, den): (u64, u64) = match self.modifier {
            GridModifier::Plain => (1, 1),
            GridModifier::Dotted => (3, 2),
            GridModifier::Triplet => (2, 3),
        };
        Tick((whole * num / (u64::from(self.base.denominator()) * den)).max(1))
    }

    /// Snaps `tick` to the nearest grid point.
    ///
    /// `swing` in `0.0..=1.0` delays every second grid step: `0.0` is straight, `1.0`
    /// pushes the off-step halfway to the following step (so `2/3` gives a triplet feel).
    /// Ties snap to the earlier point.
    pub fn snap(self, tick: Tick, ppq: u32, swing: f32) -> Tick {
        let step = self.ticks(ppq).get();
        let swing = if swing.is_finite() { swing.clamp(0.0, 1.0) } else { 0.0 };
        let delay = (f64::from(swing) * step as f64 / 2.0).round() as u64;
        let pair = step * 2;
        let t = tick.get();
        let base = t / pair * pair;
        let candidates = [base, base + step + delay, base + pair];
        let mut best = candidates[0];
        let mut best_dist = t.abs_diff(best);
        for c in candidates.into_iter().skip(1) {
            let d = t.abs_diff(c);
            if d < best_dist {
                best = c;
                best_dist = d;
            }
        }
        Tick(best)
    }

    /// Cycles the base division to the next shorter one, keeping the modifier.
    pub fn next(self) -> Grid {
        Grid { base: self.base.next(), modifier: self.modifier }
    }

    /// Cycles the base division to the next longer one, keeping the modifier.
    pub fn prev(self) -> Grid {
        Grid { base: self.base.prev(), modifier: self.modifier }
    }

    /// Every grid, longest first, plain/dotted/triplet for each division.
    pub fn all() -> Vec<Grid> {
        Division::ALL
            .iter()
            .flat_map(|&base| {
                [GridModifier::Plain, GridModifier::Dotted, GridModifier::Triplet]
                    .into_iter()
                    .map(move |modifier| Grid { base, modifier })
            })
            .collect()
    }
}

impl fmt::Display for Grid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let suffix = match self.modifier {
            GridModifier::Plain => "",
            GridModifier::Dotted => ".",
            GridModifier::Triplet => "T",
        };
        write!(f, "1/{}{}", self.base.denominator(), suffix)
    }
}

impl FromStr for Grid {
    type Err = TimeError;

    /// Accepts `"1/16"`, `"1/8T"`, `"1/8t"`, `"1/4."` (whitespace trimmed).
    fn from_str(s: &str) -> Result<Self, TimeError> {
        let err = || TimeError::BadGrid(s.to_string());
        let s = s.trim();
        let rest = s.strip_prefix("1/").ok_or_else(err)?;
        let (digits, modifier) = if let Some(d) = rest.strip_suffix(['T', 't']) {
            (d, GridModifier::Triplet)
        } else if let Some(d) = rest.strip_suffix('.') {
            (d, GridModifier::Dotted)
        } else {
            (rest, GridModifier::Plain)
        };
        let den: u32 = digits.parse().map_err(|_| err())?;
        let base = Division::from_denominator(den).ok_or_else(err)?;
        Ok(Grid { base, modifier })
    }
}

impl TryFrom<String> for Grid {
    type Error = TimeError;
    fn try_from(s: String) -> Result<Self, TimeError> {
        s.parse()
    }
}

impl From<Grid> for String {
    fn from(g: Grid) -> String {
        g.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const PPQ: u32 = 960;

    #[test]
    fn constant_tempo_round_trip() {
        let m = TempoMap::constant(120.0);
        // 120 bpm: one quarter = 0.5 s
        assert!((m.tick_to_seconds(PPQ, Tick(960)) - 0.5).abs() < 1e-12);
        assert_eq!(m.seconds_to_tick(PPQ, 0.5), Tick(960));
        assert_eq!(m.seconds_to_tick(PPQ, -1.0), Tick::ZERO);
        assert_eq!(m.seconds_to_tick(PPQ, f64::NAN), Tick::ZERO);
    }

    #[test]
    fn multi_segment_tempo() {
        let mut m = TempoMap::constant(120.0);
        m.set_tempo_at(Tick(960), 60.0).unwrap();
        // first quarter at 120 = 0.5s, second quarter at 60 = 1.0s
        assert!((m.tick_to_seconds(PPQ, Tick(1920)) - 1.5).abs() < 1e-12);
        assert_eq!(m.seconds_to_tick(PPQ, 1.5), Tick(1920));
        assert_eq!(m.seconds_to_tick(PPQ, 0.25), Tick(480));
        assert_eq!(m.bpm_at(Tick(959)), 120.0);
        assert_eq!(m.bpm_at(Tick(960)), 60.0);
        // replacing an existing point
        m.set_tempo_at(Tick(960), 240.0).unwrap();
        assert_eq!(m.points().len(), 2);
        assert_eq!(m.bpm_at(Tick(5000)), 240.0);
    }

    #[test]
    fn tempo_validation() {
        assert_eq!(TempoMap::from_points(vec![]), Err(TimeError::Empty));
        assert_eq!(
            TempoMap::from_points(vec![TempoPoint { tick: Tick(5), bpm: 100.0 }]),
            Err(TimeError::FirstNotAtZero(5))
        );
        assert_eq!(
            TempoMap::from_points(vec![
                TempoPoint { tick: Tick(0), bpm: 100.0 },
                TempoPoint { tick: Tick(0), bpm: 100.0 }
            ]),
            Err(TimeError::NotSorted(0))
        );
        assert!(matches!(
            TempoMap::from_points(vec![TempoPoint { tick: Tick(0), bpm: 0.0 }]),
            Err(TimeError::BadTempo(_))
        ));
        assert!(matches!(TempoMap::constant(100.0).set_tempo_at(Tick(1), -3.0), Err(TimeError::BadTempo(_))));
    }

    #[test]
    fn meter_bar_beat() {
        let m = MeterMap::constant(4, 4);
        assert_eq!(m.bar_beat_of(PPQ, Tick(0)), BarBeat { bar: 1, beat: 1, tick_in_beat: 0 });
        assert_eq!(m.bar_beat_of(PPQ, Tick(960 * 5 + 10)), BarBeat { bar: 2, beat: 2, tick_in_beat: 10 });
        assert_eq!(m.tick_of_bar(PPQ, 1), Tick(0));
        assert_eq!(m.tick_of_bar(PPQ, 0), Tick(0));
        assert_eq!(m.tick_of_bar(PPQ, 3), Tick(960 * 8));
        assert_eq!(m.bar_ticks_at(PPQ, Tick(0)), Tick(3840));
        assert_eq!(m.beat_ticks_at(PPQ, Tick(0)), Tick(960));
        assert_eq!(m.bar_beat_of(PPQ, Tick(100)).to_string(), "1:1:100");
    }

    #[test]
    fn meter_changes() {
        let mut m = MeterMap::constant(4, 4);
        // after one bar of 4/4, switch to 3/8
        m.set_meter_at(Tick(3840), 3, 8).unwrap();
        assert_eq!(m.meter_at(Tick(3840)), (3, 8));
        assert_eq!(m.bar_beat_of(PPQ, Tick(3840)), BarBeat { bar: 2, beat: 1, tick_in_beat: 0 });
        // a 3/8 bar is 3 * 480 = 1440 ticks
        assert_eq!(m.bar_beat_of(PPQ, Tick(3840 + 1440 + 480)), BarBeat { bar: 3, beat: 2, tick_in_beat: 0 });
        assert_eq!(m.tick_of_bar(PPQ, 3), Tick(3840 + 1440));
        // a meter change mid-bar leaves a short bar
        let mut m = MeterMap::constant(4, 4);
        m.set_meter_at(Tick(1920), 2, 4).unwrap();
        assert_eq!(m.bar_beat_of(PPQ, Tick(1920)), BarBeat { bar: 2, beat: 1, tick_in_beat: 0 });
        assert_eq!(m.tick_of_bar(PPQ, 2), Tick(1920));
        assert_eq!(m.tick_of_bar(PPQ, 3), Tick(3840));
        assert!(matches!(m.set_meter_at(Tick(5), 4, 3), Err(TimeError::BadMeter(4, 3))));
    }

    #[test]
    fn grid_ticks_and_display() {
        for (s, ticks) in [
            ("1/1", 3840),
            ("1/2", 1920),
            ("1/4", 960),
            ("1/8", 480),
            ("1/16", 240),
            ("1/32", 120),
            ("1/64", 60),
            ("1/4.", 1440),
            ("1/8T", 320),
            ("1/16T", 160),
            ("1/64T", 40),
            ("1/64.", 90),
        ] {
            let g: Grid = s.parse().unwrap();
            assert_eq!(g.ticks(PPQ), Tick(ticks), "{s}");
            assert_eq!(g.to_string(), s);
        }
        assert_eq!("1/8t".parse::<Grid>().unwrap().to_string(), "1/8T");
        assert!(matches!("1/3".parse::<Grid>(), Err(TimeError::BadGrid(_))));
        assert!(matches!("16".parse::<Grid>(), Err(TimeError::BadGrid(_))));
        assert!(matches!("1/".parse::<Grid>(), Err(TimeError::BadGrid(_))));
        assert_eq!(Grid::all().len(), 21);
    }

    #[test]
    fn grid_cycle() {
        let g = Grid::default();
        assert_eq!(g.next().to_string(), "1/32");
        assert_eq!(g.prev().to_string(), "1/8");
        assert_eq!(Grid::plain(Division::SixtyFourth).next().to_string(), "1/1");
        assert_eq!(Grid::plain(Division::Whole).prev().to_string(), "1/64");
        let t = Grid { base: Division::Eighth, modifier: GridModifier::Triplet };
        assert_eq!(t.next().to_string(), "1/16T");
    }

    #[test]
    fn grid_snap_straight() {
        let g = Grid::default(); // 240 ticks
        assert_eq!(g.snap(Tick(0), PPQ, 0.0), Tick(0));
        assert_eq!(g.snap(Tick(119), PPQ, 0.0), Tick(0));
        assert_eq!(g.snap(Tick(120), PPQ, 0.0), Tick(0)); // tie -> earlier
        assert_eq!(g.snap(Tick(121), PPQ, 0.0), Tick(240));
        assert_eq!(g.snap(Tick(500), PPQ, 0.0), Tick(480));
        assert_eq!(g.snap(Tick(700), PPQ, 0.0), Tick(720));
    }

    #[test]
    fn grid_snap_swing() {
        let g = Grid::default(); // 240; swing 1.0 delays off-steps by 120
        assert_eq!(g.snap(Tick(240), PPQ, 1.0), Tick(360));
        assert_eq!(g.snap(Tick(350), PPQ, 1.0), Tick(360));
        assert_eq!(g.snap(Tick(480), PPQ, 1.0), Tick(480));
        assert_eq!(g.snap(Tick(100), PPQ, 1.0), Tick(0));
        // swing 2/3 -> delay 80 -> triplet feel
        assert_eq!(g.snap(Tick(330), PPQ, 2.0 / 3.0), Tick(320));
        // garbage swing is ignored
        assert_eq!(g.snap(Tick(240), PPQ, f32::NAN), Tick(240));
    }

    #[test]
    fn grid_serde_is_a_string() {
        let g: Grid = serde_json_like_roundtrip("1/8T");
        assert_eq!(g.modifier, GridModifier::Triplet);
    }

    fn serde_json_like_roundtrip(s: &str) -> Grid {
        let ron = format!("\"{s}\"");
        let g: Grid = ron::from_str(&ron).unwrap();
        assert_eq!(ron::to_string(&g).unwrap(), ron);
        g
    }

    fn tempo_map_strategy() -> impl Strategy<Value = TempoMap> {
        prop::collection::vec((1u64..200_000, 20.0f64..300.0), 0..6).prop_map(|extra| {
            let mut m = TempoMap::constant(120.0);
            for (tick, bpm) in extra {
                m.set_tempo_at(Tick(tick), bpm).unwrap();
            }
            m
        })
    }

    proptest! {
        #[test]
        fn tick_seconds_round_trip(m in tempo_map_strategy(), tick in 0u64..400_000) {
            let secs = m.tick_to_seconds(PPQ, Tick(tick));
            let back = m.seconds_to_tick(PPQ, secs);
            prop_assert!(back.get().abs_diff(tick) <= 1, "tick {tick} -> {secs}s -> {back}");
        }

        #[test]
        fn seconds_are_monotonic(m in tempo_map_strategy(), a in 0u64..400_000, b in 0u64..400_000) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(m.tick_to_seconds(PPQ, Tick(lo)) <= m.tick_to_seconds(PPQ, Tick(hi)));
        }

        #[test]
        fn bar_round_trip(bar in 1u32..500, num in 1u8..13, den_pow in 0u32..7) {
            let den = 1u8 << den_pow;
            let mut m = MeterMap::constant(4, 4);
            m.set_meter_at(Tick(3840 * 3), num, den).unwrap();
            let t = m.tick_of_bar(PPQ, bar);
            let bb = m.bar_beat_of(PPQ, t);
            prop_assert_eq!(bb, BarBeat { bar, beat: 1, tick_in_beat: 0 });
        }

        #[test]
        fn snap_lands_on_grid_when_straight(t in 0u64..1_000_000, idx in 0usize..21) {
            let g = Grid::all()[idx];
            let s = g.snap(Tick(t), PPQ, 0.0);
            prop_assert_eq!(s.get() % g.ticks(PPQ).get(), 0);
            prop_assert!(s.get().abs_diff(t) <= g.ticks(PPQ).get() / 2);
        }
    }
}
