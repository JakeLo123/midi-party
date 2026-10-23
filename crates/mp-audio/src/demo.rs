//! A tiny built-in song used by the CLI and tests until `mp-core` can supply real ones.

use crate::render_song::{EventKind, RenderSong, RenderTrack, TimedEvent, TrackMix};

fn note(events: &mut Vec<TimedEvent>, time: f64, duration: f64, key: u8, velocity: u8) {
    events.push(TimedEvent { time, kind: EventKind::NoteOn { key, velocity } });
    events.push(TimedEvent { time: time + duration, kind: EventKind::NoteOff { key } });
}

/// Four bars at 120 BPM: a piano arpeggio (track 0) over a bass line (track 1).
pub fn demo_song() -> RenderSong {
    let beat = 0.5;
    let chords: [[u8; 3]; 4] = [[60, 64, 67], [57, 60, 64], [53, 57, 60], [55, 59, 62]];
    let roots: [u8; 4] = [36, 33, 29, 31];

    let mut piano = vec![TimedEvent { time: 0.0, kind: EventKind::ProgramChange { bank: 0, program: 0 } }];
    let mut bass = vec![TimedEvent { time: 0.0, kind: EventKind::ProgramChange { bank: 0, program: 32 } }];
    for (bar, chord) in chords.iter().enumerate() {
        let bar_start = bar as f64 * 4.0 * beat;
        for eighth in 0..8 {
            let key = chord[eighth % 3] + if eighth >= 6 { 12 } else { 0 };
            let vel = if eighth % 2 == 0 { 100 } else { 80 };
            note(&mut piano, bar_start + eighth as f64 * beat / 2.0, beat / 2.0 * 0.9, key, vel);
        }
        note(&mut bass, bar_start, beat * 2.0 * 0.9, roots[bar], 100);
        note(&mut bass, bar_start + beat * 2.0, beat * 2.0 * 0.9, roots[bar] + 7, 90);
    }

    RenderSong::new(vec![
        RenderTrack::new(0, piano, TrackMix { pan: 0.2, ..TrackMix::default() }),
        RenderTrack::new(0, bass, TrackMix { pan: -0.2, gain: 0.9, ..TrackMix::default() }),
    ])
}
