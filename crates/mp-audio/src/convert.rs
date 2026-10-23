//! `mp_core::Song` (ticks) -> [`RenderSong`] (seconds), through the tempo map.

use mp_core::{Instrument, LanePoint, Song, Track};

use crate::render_song::{EventKind, RenderSong, RenderTrack, TimedEvent, TrackMix};

const CC_MOD_WHEEL: u8 = 1;
const CC_VOLUME: u8 = 7;
const CC_PAN: u8 = 10;
const CC_SUSTAIN: u8 = 64;
const CC_REVERB_SEND: u8 = 91;
const PITCH_BEND_CENTER: i32 = 8192;

/// Flattens every track's notes and controller lanes into timed events.
pub fn render_song_from(song: &Song) -> RenderSong {
    RenderSong::new(song.tracks.iter().map(|t| render_track(song, t)).collect())
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
}
