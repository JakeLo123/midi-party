//! The immutable, pre-flattened snapshot of a song that the audio thread plays.
//!
//! Times are in seconds. Building one of these from `mp-core`'s tick-based
//! `Song` (through its tempo map) is the app's job; this crate never sees ticks.

/// Hard cap on tracks so the engine can use fixed-size arrays (no allocation).
pub const MAX_TRACKS: usize = 64;

/// One MIDI-ish event on a track. The track's channel is implied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventKind {
    NoteOn { key: u8, velocity: u8 },
    NoteOff { key: u8 },
    ControlChange { controller: u8, value: u8 },
    /// 14-bit pitch bend, 0..=16383, 8192 = center.
    PitchBend { value: u16 },
    ProgramChange { bank: u16, program: u8 },
}

impl EventKind {
    /// Ordering for events at the same instant: set up the sound first, then
    /// release old notes, then start new ones.
    fn rank(&self) -> u8 {
        match self {
            EventKind::ProgramChange { .. } => 0,
            EventKind::ControlChange { .. } | EventKind::PitchBend { .. } => 1,
            EventKind::NoteOff { .. } => 2,
            EventKind::NoteOn { .. } => 3,
        }
    }
}

/// An event at an absolute time in seconds from the start of the song.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedEvent {
    pub time: f64,
    pub kind: EventKind,
}

/// Mixer state for one track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackMix {
    /// Linear gain, 1.0 = unity.
    pub gain: f32,
    /// -1.0 = hard left, 0.0 = center, 1.0 = hard right.
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for TrackMix {
    fn default() -> Self {
        Self { gain: 1.0, pan: 0.0, mute: false, solo: false }
    }
}

/// The MIDI channel General MIDI SoundFonts reserve for drum kits (channel 10, counted from 1).
pub const DRUM_CHANNEL: u8 = 9;

/// One track's events, sorted by time (and by [`EventKind::rank`] within an instant).
#[derive(Clone, Debug, PartialEq)]
pub struct RenderTrack {
    /// MIDI channel the synth is driven on. 9 selects drum kits in GM SoundFonts.
    pub channel: u8,
    pub events: Vec<TimedEvent>,
    pub mix: TrackMix,
}

impl RenderTrack {
    /// Builds a track, sorting the events into playback order.
    pub fn new(channel: u8, mut events: Vec<TimedEvent>, mix: TrackMix) -> Self {
        events.sort_by(|a, b| {
            a.time
                .partial_cmp(&b.time)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.kind.rank().cmp(&b.kind.rank()))
        });
        Self { channel, events, mix }
    }

    fn last_event_time(&self) -> f64 {
        self.events.last().map(|e| e.time).unwrap_or(0.0)
    }
}

/// A whole song, ready to play. Immutable once built; edits produce a new one.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RenderSong {
    pub tracks: Vec<RenderTrack>,
    /// Time of the last event, in seconds.
    pub length: f64,
}

impl RenderSong {
    /// Builds a song. Tracks beyond [`MAX_TRACKS`] are dropped.
    pub fn new(mut tracks: Vec<RenderTrack>) -> Self {
        tracks.truncate(MAX_TRACKS);
        let length = tracks.iter().map(RenderTrack::last_event_time).fold(0.0, f64::max);
        Self { tracks, length }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_sort_by_time_then_rank() {
        let t = RenderTrack::new(
            0,
            vec![
                TimedEvent { time: 1.0, kind: EventKind::NoteOn { key: 60, velocity: 100 } },
                TimedEvent { time: 1.0, kind: EventKind::NoteOff { key: 59 } },
                TimedEvent { time: 0.5, kind: EventKind::NoteOn { key: 59, velocity: 100 } },
                TimedEvent { time: 1.0, kind: EventKind::ProgramChange { bank: 0, program: 1 } },
            ],
            TrackMix::default(),
        );
        let kinds: Vec<_> = t.events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                EventKind::NoteOn { key: 59, velocity: 100 },
                EventKind::ProgramChange { bank: 0, program: 1 },
                EventKind::NoteOff { key: 59 },
                EventKind::NoteOn { key: 60, velocity: 100 },
            ]
        );
        assert_eq!(RenderSong::new(vec![t]).length, 1.0);
    }
}
