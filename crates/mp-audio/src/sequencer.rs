//! Walks a [`RenderSong`] in sample frames and hands out the events that fall
//! inside each block. Pure, allocation-free, and unit-testable.

use crate::render_song::{EventKind, RenderSong, MAX_TRACKS};

#[derive(Debug)]
pub struct Sequencer {
    /// Current position, in sample frames from the start of the song.
    frame: u64,
    /// Per track: index of the next event not yet emitted.
    next: [usize; MAX_TRACKS],
}

impl Default for Sequencer {
    fn default() -> Self {
        Self::new()
    }
}

impl Sequencer {
    pub fn new() -> Self {
        Self { frame: 0, next: [0; MAX_TRACKS] }
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    pub fn seconds(&self, sample_rate: u32) -> f64 {
        self.frame as f64 / sample_rate as f64
    }

    /// Jumps to `frame` and re-finds the next event on every track.
    /// Also used to re-sync after the song was swapped for an edited one.
    pub fn seek(&mut self, song: &RenderSong, frame: u64, sample_rate: u32) {
        self.frame = frame;
        let t = frame as f64 / sample_rate as f64;
        for (i, track) in song.tracks.iter().enumerate().take(MAX_TRACKS) {
            self.next[i] = track.events.partition_point(|e| e.time < t);
        }
    }

    /// Emits every event in the next `n_frames` frames as
    /// `emit(track_index, frame_offset_within_block, event)`, then advances.
    pub fn advance(
        &mut self,
        song: &RenderSong,
        n_frames: usize,
        sample_rate: u32,
        mut emit: impl FnMut(usize, usize, EventKind),
    ) {
        if n_frames == 0 {
            return;
        }
        let sr = sample_rate as f64;
        let t0 = self.frame as f64 / sr;
        let t1 = (self.frame + n_frames as u64) as f64 / sr;
        for (i, track) in song.tracks.iter().enumerate().take(MAX_TRACKS) {
            let events = &track.events;
            let mut idx = self.next[i];
            while idx < events.len() && events[idx].time < t1 {
                let offset = ((events[idx].time - t0) * sr).floor().max(0.0) as usize;
                emit(i, offset.min(n_frames - 1), events[idx].kind);
                idx += 1;
            }
            self.next[i] = idx;
        }
        self.frame += n_frames as u64;
    }

    /// Current controller state on a track at the sequencer position:
    /// the last program change, pitch bend, and CC values before `frame`.
    /// Used after a seek so sustain pedal etc. are correct mid-song.
    pub fn controller_state(&self, song: &RenderSong, track: usize) -> ControllerState {
        let mut state = ControllerState::default();
        let Some(t) = song.tracks.get(track) else { return state };
        for e in &t.events[..self.next[track].min(t.events.len())] {
            match e.kind {
                EventKind::ControlChange { controller, value } => {
                    state.cc[controller as usize & 0x7F] = Some(value)
                }
                EventKind::PitchBend { value } => state.pitch_bend = Some(value),
                EventKind::ProgramChange { bank, program } => state.program = Some((bank, program)),
                _ => {}
            }
        }
        state
    }
}

/// Snapshot of a track's controllers, see [`Sequencer::controller_state`].
#[derive(Clone, Copy, Debug)]
pub struct ControllerState {
    pub cc: [Option<u8>; 128],
    pub pitch_bend: Option<u16>,
    pub program: Option<(u16, u8)>,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self { cc: [None; 128], pitch_bend: None, program: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_song::{RenderTrack, TimedEvent, TrackMix};

    fn song() -> RenderSong {
        let ev = |time, kind| TimedEvent { time, kind };
        RenderSong::new(vec![RenderTrack::new(
            0,
            vec![
                ev(0.0, EventKind::ProgramChange { bank: 0, program: 5 }),
                ev(0.0, EventKind::NoteOn { key: 60, velocity: 100 }),
                ev(0.5, EventKind::NoteOff { key: 60 }),
                ev(0.5, EventKind::ControlChange { controller: 64, value: 127 }),
                ev(1.0, EventKind::NoteOn { key: 62, velocity: 90 }),
            ],
            TrackMix::default(),
        )])
    }

    #[test]
    fn emits_events_with_block_offsets() {
        let s = song();
        let mut seq = Sequencer::new();
        let mut got = Vec::new();
        // 1000 Hz sample rate: 1 frame = 1 ms. Block of 600 frames covers 0..0.6 s.
        seq.advance(&s, 600, 1000, |t, off, k| got.push((t, off, k)));
        assert_eq!(got.len(), 4);
        assert_eq!(got[0], (0, 0, EventKind::ProgramChange { bank: 0, program: 5 }));
        assert_eq!(got[2].1, 500);
        got.clear();
        seq.advance(&s, 600, 1000, |t, off, k| got.push((t, off, k)));
        assert_eq!(got, vec![(0, 400, EventKind::NoteOn { key: 62, velocity: 90 })]);
        assert_eq!(seq.frame(), 1200);
    }

    #[test]
    fn seek_skips_past_events_and_reports_controllers() {
        let s = song();
        let mut seq = Sequencer::new();
        seq.seek(&s, 700, 1000);
        let state = seq.controller_state(&s, 0);
        assert_eq!(state.program, Some((0, 5)));
        assert_eq!(state.cc[64], Some(127));
        let mut got = Vec::new();
        seq.advance(&s, 1000, 1000, |_, _, k| got.push(k));
        assert_eq!(got, vec![EventKind::NoteOn { key: 62, velocity: 90 }]);
    }
}
