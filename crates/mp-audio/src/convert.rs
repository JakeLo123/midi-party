//! `mp_core::Song` (ticks) -> [`RenderSong`] (seconds), through the tempo map.

use mp_core::{beat_ticks, Instrument, LanePoint, Song, Tick, Track};

use crate::render_song::{BeatSegment, EventKind, RenderSong, RenderTrack, TimedEvent, TrackMix};

const CC_MOD_WHEEL: u8 = 1;
const CC_VOLUME: u8 = 7;
const CC_PAN: u8 = 10;
const CC_SUSTAIN: u8 = 64;
const CC_REVERB_SEND: u8 = 91;
const PITCH_BEND_CENTER: i32 = 8192;

/// Flattens every track's notes and controller lanes into timed events, with the
/// beats for the metronome. The loop region is left out; see [`loop_seconds`].
pub fn render_song_from(song: &Song) -> RenderSong {
    let mut rendered = RenderSong::new(song.tracks.iter().map(|t| render_track(song, t)).collect());
    rendered.beats = beat_segments(song);
    rendered
}

/// The song's loop region in seconds, if it has one switched on.
pub fn loop_seconds(song: &Song) -> Option<(f64, f64)> {
    let region = song.loop_region.filter(|r| r.enabled && r.end > r.start)?;
    let secs = |tick| song.tempo_map.tick_to_seconds(song.ppq, tick);
    Some((secs(region.start), secs(region.end)))
}

/// One segment per stretch between tempo or meter changes.
fn beat_segments(song: &Song) -> Vec<BeatSegment> {
    let secs = |tick| song.tempo_map.tick_to_seconds(song.ppq, tick);
    let mut starts: Vec<Tick> = song.tempo_map.points().iter().map(|p| p.tick).collect();
    starts.extend(song.meter_map.points().iter().map(|p| p.tick));
    starts.sort();
    starts.dedup();
    let mut segments = Vec::with_capacity(starts.len());
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied();
        let meter_start = song.meter_map.change_at_or_before(start);
        let (numerator, denominator) = song.meter_map.meter_at(start);
        let beat = beat_ticks(song.ppq, denominator).get().max(1);
        let k = (start.get() - meter_start.get()).div_ceil(beat);
        let first = Tick(meter_start.get() + k * beat);
        if end.is_some_and(|e| first >= e) {
            continue;
        }
        let bpm = song.tempo_map.bpm_at(start);
        segments.push(BeatSegment {
            first_beat: secs(first),
            beat_seconds: beat as f64 * 60.0 / (bpm * f64::from(song.ppq)),
            end: end.map(secs).unwrap_or(f64::INFINITY),
            beats_per_bar: u32::from(numerator),
            first_beat_in_bar: (k % u64::from(numerator)) as u32,
        });
    }
    segments
}

fn render_track(song: &Song, track: &Track) -> RenderTrack {
    let secs = |tick| song.tempo_map.tick_to_seconds(song.ppq, tick);
    let mut events = Vec::with_capacity(track.notes.len() * 2 + 1);

    let (bank, program) = match &track.instrument {
        Instrument::SoundFont { bank, preset, .. } => (*bank, (*preset).min(127) as u8),
    };
    events.push(TimedEvent { time: 0.0, kind: EventKind::ProgramChange { bank, program } });
    let reverb = (song.master_reverb.clamp(0.0, 1.0) * 127.0).round() as u8;
    events.push(TimedEvent { time: 0.0, kind: EventKind::ControlChange { controller: CC_REVERB_SEND, value: reverb } });

    for note in &track.notes {
        events.push(TimedEvent {
            time: secs(note.start),
            kind: EventKind::NoteOn { key: note.pitch, velocity: note.velocity.clamp(1, 127) },
        });
        events.push(TimedEvent { time: secs(note.end()), kind: EventKind::NoteOff { key: note.pitch } });
    }

    let cc = |events: &mut Vec<TimedEvent>, points: &[LanePoint], controller: u8| {
        for p in points {
            let value = p.value.clamp(0, 127) as u8;
            events.push(TimedEvent { time: secs(p.tick), kind: EventKind::ControlChange { controller, value } });
        }
    };
    cc(&mut events, &track.lanes.sustain, CC_SUSTAIN);
    cc(&mut events, &track.lanes.mod_wheel, CC_MOD_WHEEL);
    cc(&mut events, &track.lanes.volume, CC_VOLUME);
    cc(&mut events, &track.lanes.pan, CC_PAN);
    for p in &track.lanes.pitch_bend {
        let value = (p.value + PITCH_BEND_CENTER).clamp(0, 16383) as u16;
        events.push(TimedEvent { time: secs(p.tick), kind: EventKind::PitchBend { value } });
    }

    let mix = TrackMix { gain: track.volume, pan: track.pan, mute: track.mute, solo: track.solo };
    RenderTrack::new(track.channel, events, mix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mp_core::Tick;

    #[test]
    fn notes_land_at_tempo_mapped_seconds() {
        let mut song = Song::new_empty(); // 120 BPM, ppq 960: one beat = 0.5 s
        let ppq = song.ppq as u64;
        let n1 = song.new_note(Tick(ppq), Tick(ppq / 2), 60, 100);
        let n2 = song.new_note(Tick(0), Tick(ppq), 48, 90);
        let track = song.track_mut(0).unwrap();
        track.insert_note(n1);
        track.insert_note(n2);
        track.lanes.sustain.push(LanePoint { tick: Tick(ppq * 2), value: 127 });

        let rendered = render_song_from(&song);
        let ev = &rendered.tracks[0].events;
        assert_eq!(ev[0].kind, EventKind::ProgramChange { bank: 0, program: 0 });
        assert_eq!(ev[1].kind, EventKind::ControlChange { controller: 91, value: 32 }); // master reverb 0.25
        assert_eq!(ev[2], TimedEvent { time: 0.0, kind: EventKind::NoteOn { key: 48, velocity: 90 } });
        assert_eq!(ev[3], TimedEvent { time: 0.5, kind: EventKind::NoteOff { key: 48 } });
        assert_eq!(ev[4], TimedEvent { time: 0.5, kind: EventKind::NoteOn { key: 60, velocity: 100 } });
        assert_eq!(ev[5], TimedEvent { time: 0.75, kind: EventKind::NoteOff { key: 60 } });
        assert_eq!(ev[6], TimedEvent { time: 1.0, kind: EventKind::ControlChange { controller: 64, value: 127 } });
        assert_eq!(rendered.length, 1.0);
    }

    #[test]
    fn beats_follow_tempo_and_meter_changes() {
        let mut song = Song::new_empty(); // 120 BPM 4/4
        let ppq = song.ppq as u64;
        song.tempo_map.set_tempo_at(Tick(ppq * 4), 60.0).unwrap(); // bar 2 at half speed
        song.meter_map.set_meter_at(Tick(ppq * 6), 3, 4).unwrap(); // 3/4 from beat 3 of bar 2
        let beats = render_song_from(&song).beats;
        assert_eq!(beats.len(), 3);
        assert_eq!((beats[0].first_beat, beats[0].beat_seconds, beats[0].end), (0.0, 0.5, 2.0));
        assert_eq!((beats[1].first_beat, beats[1].beat_seconds, beats[1].end), (2.0, 1.0, 4.0));
        assert!(beats[1].is_downbeat(0));
        assert_eq!((beats[2].first_beat, beats[2].beats_per_bar), (4.0, 3));
        assert!(beats[2].is_downbeat(0) && !beats[2].is_downbeat(1) && beats[2].is_downbeat(3));
    }

    #[test]
    fn loop_seconds_only_for_an_enabled_region() {
        let mut song = Song::new_empty();
        let ppq = song.ppq as u64;
        assert_eq!(loop_seconds(&song), None);
        song.loop_region = Some(mp_core::LoopRegion { start: Tick(ppq), end: Tick(ppq * 3), enabled: true });
        assert_eq!(loop_seconds(&song), Some((0.5, 1.5)));
        song.loop_region = Some(mp_core::LoopRegion { start: Tick(ppq), end: Tick(ppq * 3), enabled: false });
        assert_eq!(loop_seconds(&song), None);
    }
}
