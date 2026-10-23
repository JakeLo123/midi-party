//! Standard MIDI file (.mid) import and export via `midly`.
//!
//! Export writes a format-1 file: track 0 carries the tempo and meter maps,
//! then one MIDI track per song track. Import accepts format 0/1/2 files with
//! metrical timing, rescales ticks to [`DEFAULT_PPQ`], and turns every
//! (track, channel) pair that carries notes into a song track.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, PitchBend, Smf, Timing, TrackEvent, TrackEventKind};
use thiserror::Error;

use crate::song::{Instrument, LanePoint, Lanes, Song, Tick, Track, DEFAULT_PPQ};
use crate::time::{MeterMap, MeterPoint, TempoMap, TempoPoint, TimeError};

#[derive(Debug, Error)]
pub enum MidiFileError {
    #[error("not a valid MIDI file: {0}")]
    Parse(String),
    #[error("could not write MIDI data: {0}")]
    Write(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("SMPTE-timecode MIDI files are not supported")]
    Timecode,
    #[error("ppq {0} cannot be stored in a MIDI file")]
    BadPpq(u32),
    #[error(transparent)]
    Time(#[from] TimeError),
}

const CC_BANK_MSB: u8 = 0;
const CC_MOD_WHEEL: u8 = 1;
const CC_VOLUME: u8 = 7;
const CC_PAN: u8 = 10;
const CC_BANK_LSB: u8 = 32;
const CC_SUSTAIN: u8 = 64;
const DEFAULT_BPM: f64 = 120.0;

/// Sort rank for events at the same tick: set-up first, releases before attacks.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Meta,
    Control,
    NoteOff,
    NoteOn,
}

fn bpm_to_us_per_beat(bpm: f64) -> u32 {
    (60_000_000.0 / bpm.max(1.0)).round().clamp(1.0, 16_777_215.0) as u32
}

fn us_per_beat_to_bpm(us: u32) -> f64 {
    60_000_000.0 / us.max(1) as f64
}

fn to_track<'a>(mut events: Vec<(u64, Rank, TrackEventKind<'a>)>) -> Vec<TrackEvent<'a>> {
    events.sort_by_key(|(tick, rank, _)| (*tick, *rank));
    let mut out = Vec::with_capacity(events.len() + 1);
    let mut last = 0u64;
    for (tick, _, kind) in events {
        let delta = (tick - last).min(u28::max_value().as_int() as u64) as u32;
        out.push(TrackEvent { delta: u28::new(delta), kind });
        last = tick;
    }
    out.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
    out
}

fn midi<'a>(channel: u8, message: MidiMessage) -> TrackEventKind<'a> {
    TrackEventKind::Midi { channel: u4::new(channel & 0x0F), message }
}

fn cc<'a>(channel: u8, controller: u8, value: i32) -> TrackEventKind<'a> {
    midi(
        channel,
        MidiMessage::Controller { controller: u7::new(controller & 0x7F), value: u7::new(value.clamp(0, 127) as u8) },
    )
}

fn track_events(track: &Track) -> Vec<(u64, Rank, TrackEventKind<'_>)> {
    let ch = track.channel;
    let mut events: Vec<(u64, Rank, TrackEventKind)> = Vec::with_capacity(track.notes.len() * 2 + 4);
    events.push((0, Rank::Meta, TrackEventKind::Meta(MetaMessage::TrackName(track.name.as_bytes()))));
    let Instrument::SoundFont { bank, preset, .. } = &track.instrument;
    if *bank != 0 {
        events.push((0, Rank::Meta, cc(ch, CC_BANK_MSB, (*bank >> 7) as i32 & 0x7F)));
        events.push((0, Rank::Meta, cc(ch, CC_BANK_LSB, (*bank & 0x7F) as i32)));
    }
    events.push((0, Rank::Meta, midi(ch, MidiMessage::ProgramChange { program: u7::new((*preset).min(127) as u8) })));
    for n in &track.notes {
        let key = u7::new(n.pitch.min(127));
        events.push((n.start.get(), Rank::NoteOn, midi(ch, MidiMessage::NoteOn { key, vel: u7::new(n.velocity.clamp(1, 127)) })));
        events.push((n.end().get(), Rank::NoteOff, midi(ch, MidiMessage::NoteOff { key, vel: u7::new(0) })));
    }
    let lane = |events: &mut Vec<(u64, Rank, TrackEventKind)>, points: &[LanePoint], controller: u8| {
        for p in points {
            events.push((p.tick.get(), Rank::Control, cc(ch, controller, p.value)));
        }
    };
    lane(&mut events, &track.lanes.sustain, CC_SUSTAIN);
    lane(&mut events, &track.lanes.mod_wheel, CC_MOD_WHEEL);
    lane(&mut events, &track.lanes.volume, CC_VOLUME);
    lane(&mut events, &track.lanes.pan, CC_PAN);
    for p in &track.lanes.pitch_bend {
        let bend = PitchBend::from_int(p.value.clamp(-8192, 8191) as i16);
        events.push((p.tick.get(), Rank::Control, midi(ch, MidiMessage::PitchBend { bend })));
    }
    events
}

/// Serializes the song as a format-1 standard MIDI file.
pub fn export(song: &Song) -> Result<Vec<u8>, MidiFileError> {
    let ppq = u16::try_from(song.ppq).ok().filter(|p| *p < 0x8000).ok_or(MidiFileError::BadPpq(song.ppq))?;
    let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Metrical(u15::new(ppq))));

    let mut conductor: Vec<(u64, Rank, TrackEventKind)> = Vec::new();
    conductor.push((0, Rank::Meta, TrackEventKind::Meta(MetaMessage::TrackName(b"midi-party"))));
    for p in song.tempo_map.points() {
        let tempo = MetaMessage::Tempo(u24::new(bpm_to_us_per_beat(p.bpm)));
        conductor.push((p.tick.get(), Rank::Meta, TrackEventKind::Meta(tempo)));
    }
    for m in song.meter_map.points() {
        let denominator_log2 = m.denominator.max(1).trailing_zeros() as u8;
        let sig = MetaMessage::TimeSignature(m.numerator, denominator_log2, 24, 8);
        conductor.push((m.tick.get(), Rank::Meta, TrackEventKind::Meta(sig)));
    }
    smf.tracks.push(to_track(conductor));
    for track in &song.tracks {
        smf.tracks.push(to_track(track_events(track)));
    }

    let mut out = Vec::new();
    smf.write_std(&mut out).map_err(|e| MidiFileError::Write(e.to_string()))?;
    Ok(out)
}

pub fn save(song: &Song, path: impl AsRef<Path>) -> Result<(), MidiFileError> {
    std::fs::write(path, export(song)?)?;
    Ok(())
}

/// Notes and lanes gathered for one channel of one MIDI track.
#[derive(Default)]
struct ChannelBuild {
    /// Sounding notes per key: (start tick, velocity), newest last.
    open: HashMap<u8, Vec<(Tick, u8)>>,
    notes: Vec<(Tick, Tick, u8, u8)>,
    lanes: Lanes,
    program: Option<u8>,
    bank_msb: u8,
    bank_lsb: u8,
    last_tick: Tick,
}

impl ChannelBuild {
    fn note_on(&mut self, tick: Tick, key: u8, vel: u8) {
        self.open.entry(key).or_default().push((tick, vel));
        self.last_tick = tick;
    }

    fn note_off(&mut self, tick: Tick, key: u8) {
        if let Some((start, vel)) = self.open.get_mut(&key).and_then(|v| v.pop()) {
            let duration = Tick(tick.get().saturating_sub(start.get()).max(1));
            self.notes.push((start, duration, key, vel));
        }
        self.last_tick = tick;
    }

    fn close_all(&mut self, ppq: u32) {
        let open: Vec<(u8, Vec<(Tick, u8)>)> = self.open.drain().collect();
        for (key, starts) in open {
            for (start, vel) in starts {
                self.notes.push((start, Tick(ppq as u64), key, vel));
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.notes.is_empty()
            && self.lanes.sustain.is_empty()
            && self.lanes.pitch_bend.is_empty()
            && self.lanes.mod_wheel.is_empty()
            && self.lanes.volume.is_empty()
            && self.lanes.pan.is_empty()
    }
}

/// Keeps the last point per tick, sorted, and guarantees a point at tick 0.
fn dedupe_sorted<T: Clone>(mut points: Vec<(Tick, T)>, default: T) -> Vec<(Tick, T)> {
    points.sort_by_key(|(t, _)| *t);
    let mut out: Vec<(Tick, T)> = Vec::with_capacity(points.len() + 1);
    for (tick, value) in points {
        if out.last().map(|(t, _)| *t) == Some(tick) {
            out.pop();
        }
        out.push((tick, value));
    }
    match out.first() {
        Some((t, _)) if t.get() == 0 => {}
        Some((_, v)) => {
            let v = v.clone();
            out.insert(0, (Tick::ZERO, v));
        }
        None => out.push((Tick::ZERO, default)),
    }
    out
}

/// Parses a standard MIDI file into a song at [`DEFAULT_PPQ`].
pub fn import(bytes: &[u8]) -> Result<Song, MidiFileError> {
    let smf = Smf::parse(bytes).map_err(|e| MidiFileError::Parse(e.to_string()))?;
    let src_ppq = match smf.header.timing {
        Timing::Metrical(p) => p.as_int().max(1) as u64,
        Timing::Timecode(..) => return Err(MidiFileError::Timecode),
    };
    let ppq = DEFAULT_PPQ;
    let scale = |t: u64| Tick((t * ppq as u64 + src_ppq / 2) / src_ppq);

    let mut song = Song::new_empty();
    song.tracks.clear();
    let mut tempo_points: Vec<(Tick, f64)> = Vec::new();
    let mut meter_points: Vec<(Tick, (u8, u8))> = Vec::new();

    for track in &smf.tracks {
        let mut abs = 0u64;
        let mut name: Option<String> = None;
        let mut channels: BTreeMap<u8, ChannelBuild> = BTreeMap::new();
        for ev in track {
            abs += ev.delta.as_int() as u64;
            let tick = scale(abs);
            match &ev.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(us)) => tempo_points.push((tick, us_per_beat_to_bpm(us.as_int()))),
                TrackEventKind::Meta(MetaMessage::TimeSignature(n, d_log2, _, _)) => {
                    let denominator = if *d_log2 <= 6 { 1u8 << d_log2 } else { 4 };
                    meter_points.push((tick, ((*n).max(1), denominator)));
                }
                TrackEventKind::Meta(MetaMessage::TrackName(bytes)) => {
                    let s = String::from_utf8_lossy(bytes).trim().to_string();
                    if !s.is_empty() {
                        name = Some(s);
                    }
                }
                TrackEventKind::Midi { channel, message } => {
                    let cb = channels.entry(channel.as_int()).or_default();
                    match *message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => cb.note_on(tick, key.as_int(), vel.as_int()),
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => cb.note_off(tick, key.as_int()),
                        MidiMessage::Controller { controller, value } => {
                            let (c, v) = (controller.as_int(), value.as_int() as i32);
                            let point = LanePoint { tick, value: v };
                            match c {
                                CC_SUSTAIN => cb.lanes.sustain.push(point),
                                CC_MOD_WHEEL => cb.lanes.mod_wheel.push(point),
                                CC_VOLUME => cb.lanes.volume.push(point),
                                CC_PAN => cb.lanes.pan.push(point),
                                CC_BANK_MSB => cb.bank_msb = v as u8,
                                CC_BANK_LSB => cb.bank_lsb = v as u8,
                                _ => {}
                            }
                        }
                        MidiMessage::ProgramChange { program } => {
                            if cb.program.is_none() {
                                cb.program = Some(program.as_int());
                            }
                        }
                        MidiMessage::PitchBend { bend } => {
                            cb.lanes.pitch_bend.push(LanePoint { tick, value: bend.as_int() as i32 })
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let multi = channels.values().filter(|c| !c.is_empty()).count() > 1;
        for (channel, mut cb) in channels {
            cb.close_all(ppq);
            if cb.is_empty() {
                continue;
            }
            let base = name.clone().unwrap_or_else(|| format!("Track {}", song.tracks.len() + 1));
            let track_name = if multi { format!("{base} ch{}", channel + 1) } else { base };
            let mut t = Track::new(track_name);
            t.channel = channel;
            t.instrument = Instrument::SoundFont {
                path: PathBuf::new(),
                bank: ((cb.bank_msb as u16) << 7) | cb.bank_lsb as u16,
                preset: cb.program.unwrap_or(0) as u16,
            };
            for (start, duration, pitch, vel) in cb.notes {
                let note = song.new_note(start, duration, pitch, vel.clamp(1, 127));
                t.insert_note(note);
            }
            t.lanes = cb.lanes;
            t.lanes.sort();
            song.add_track(t);
        }
    }
    if song.tracks.is_empty() {
        song.add_track(Track::new("Track 1"));
    }

    let tempo = dedupe_sorted(tempo_points, DEFAULT_BPM)
        .into_iter()
        .map(|(tick, bpm)| TempoPoint { tick, bpm })
        .collect();
    song.tempo_map = TempoMap::from_points(tempo)?;
    let meter = dedupe_sorted(meter_points, (4, 4))
        .into_iter()
        .map(|(tick, (numerator, denominator))| MeterPoint { tick, numerator, denominator })
        .collect();
    song.meter_map = MeterMap::from_points(meter)?;
    song.normalize();
    Ok(song)
}

pub fn load(path: impl AsRef<Path>) -> Result<Song, MidiFileError> {
    import(&std::fs::read(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(track: &Track) -> Vec<(u64, u64, u8, u8)> {
        track.notes.iter().map(|n| (n.start.get(), n.duration.get(), n.pitch, n.velocity)).collect()
    }

    fn sample_song() -> Song {
        let mut song = Song::new_empty();
        let ppq = song.ppq as u64;
        song.tempo_map.set_tempo_at(Tick(ppq * 8), 90.0).unwrap();
        song.meter_map.set_meter_at(Tick(ppq * 8), 3, 4).unwrap();
        let n1 = song.new_note(Tick(0), Tick(ppq), 60, 100);
        let n2 = song.new_note(Tick(ppq), Tick(ppq / 2), 64, 80);
        let n3 = song.new_note(Tick(ppq), Tick(ppq / 2), 67, 80); // chord with n2
        {
            let t = song.track_mut(0).unwrap();
            t.name = "Piano".into();
            t.insert_note(n1);
            t.insert_note(n2);
            t.insert_note(n3);
            t.lanes.sustain.push(LanePoint { tick: Tick(ppq / 4), value: 127 });
            t.lanes.pitch_bend.push(LanePoint { tick: Tick(ppq / 2), value: -2000 });
            t.instrument = Instrument::SoundFont { path: PathBuf::new(), bank: 1, preset: 33 };
        }
        let mut bass = Track::new("Bass");
        bass.channel = 1;
        let b1 = song.new_note(Tick(0), Tick(ppq * 2), 36, 110);
        bass.insert_note(b1);
        song.add_track(bass);
        song
    }

    #[test]
    fn round_trip_keeps_notes_lanes_tempo_and_meter() {
        let song = sample_song();
        let bytes = export(&song).unwrap();
        let back = import(&bytes).unwrap();
        assert_eq!(back.tracks.len(), 2);
        assert_eq!(back.tracks[0].name, "Piano");
        assert_eq!(shape(&back.tracks[0]), shape(&song.tracks[0]));
        assert_eq!(shape(&back.tracks[1]), shape(&song.tracks[1]));
        assert_eq!(back.tracks[1].channel, 1);
        assert_eq!(back.tracks[0].instrument, Instrument::SoundFont { path: PathBuf::new(), bank: 1, preset: 33 });
        assert_eq!(back.tracks[0].lanes.sustain, song.tracks[0].lanes.sustain);
        assert_eq!(back.tracks[0].lanes.pitch_bend, song.tracks[0].lanes.pitch_bend);
        // Tempo is stored as integer microseconds per beat, so compare with a tolerance.
        let tempos: Vec<_> = back.tempo_map.points().iter().map(|p| (p.tick, (p.bpm * 100.0).round() / 100.0)).collect();
        let expected: Vec<_> = song.tempo_map.points().iter().map(|p| (p.tick, p.bpm)).collect();
        assert_eq!(tempos, expected);
        assert_eq!(back.meter_map.points(), song.meter_map.points());
    }

    #[test]
    fn imports_other_ppq_and_running_note_on_zero_velocity() {
        // Format 0, 480 ppq: a note from tick 480 to 960 ended by NoteOn vel 0.
        let mut smf = Smf::new(Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))));
        smf.tracks.push(vec![
            TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))) },
            TrackEvent { delta: u28::new(480), kind: midi(0, MidiMessage::NoteOn { key: u7::new(62), vel: u7::new(90) }) },
            TrackEvent { delta: u28::new(480), kind: midi(0, MidiMessage::NoteOn { key: u7::new(62), vel: u7::new(0) }) },
            TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) },
        ]);
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        let song = import(&bytes).unwrap();
        assert_eq!(song.ppq, DEFAULT_PPQ);
        assert_eq!(shape(&song.tracks[0]), vec![(960, 960, 62, 90)]);
        assert_eq!(song.tempo_map.bpm_at(Tick::ZERO), 120.0);
    }

    #[test]
    fn splits_multi_channel_tracks_and_names_them() {
        let mut smf = Smf::new(Header::new(Format::SingleTrack, Timing::Metrical(u15::new(960))));
        smf.tracks.push(vec![
            TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(b"Band")) },
            TrackEvent { delta: u28::new(0), kind: midi(0, MidiMessage::NoteOn { key: u7::new(60), vel: u7::new(100) }) },
            TrackEvent { delta: u28::new(0), kind: midi(9, MidiMessage::NoteOn { key: u7::new(36), vel: u7::new(100) }) },
            TrackEvent { delta: u28::new(240), kind: midi(0, MidiMessage::NoteOff { key: u7::new(60), vel: u7::new(0) }) },
            TrackEvent { delta: u28::new(0), kind: midi(9, MidiMessage::NoteOff { key: u7::new(36), vel: u7::new(0) }) },
            TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) },
        ]);
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        let song = import(&bytes).unwrap();
        let names: Vec<&str> = song.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Band ch1", "Band ch10"]);
        assert_eq!(song.tracks[1].channel, 9);
    }

    #[test]
    fn timecode_files_are_rejected() {
        let smf = Smf::new(Header::new(Format::SingleTrack, Timing::Timecode(midly::Fps::Fps25, 40)));
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        assert!(matches!(import(&bytes), Err(MidiFileError::Timecode)));
    }
}
