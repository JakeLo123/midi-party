//! The real-time engine and its UI-side handle.
//!
//! [`Engine::process`] is the single entry point that produces audio. It must
//! never allocate, lock, or block: everything it needs is preallocated and
//! every message in or out goes through lock-free queues or atomics.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::error::AudioError;
use crate::render_song::{BeatSegment, EventKind, RenderSong, MAX_TRACKS};
use crate::sequencer::Sequencer;
use crate::synth::Synth;

/// Frames rendered per inner step. Events are applied at sub-block boundaries,
/// so this is the timing granularity (64 frames ≈ 1.3 ms at 48 kHz).
const SUB_BLOCK: usize = 64;
const MAX_AUDITIONS: usize = 64;
const COMMAND_QUEUE_LEN: usize = 256;
const GARBAGE_QUEUE_LEN: usize = 256;
/// After the last event, keep playing this long so release tails finish.
const END_TAIL_SECONDS: f64 = 2.0;
const CLICK_SECONDS: f64 = 0.04;
const CLICK_HZ: f32 = 1320.0;
const DOWNBEAT_CLICK_HZ: f32 = 1760.0;
const CLICK_GAIN: f32 = 0.35;
const DOWNBEAT_CLICK_GAIN: f32 = 0.5;

/// Messages from the UI thread to the audio thread.
pub enum AudioCmd {
    Play,
    Stop,
    Seek { frame: u64 },
    /// Play one note outside the timeline (the "hear edits" feature).
    Audition { track: usize, key: u8, velocity: u8, duration_frames: u32 },
    /// Install (or replace) the instrument for a track.
    SetSynth { track: usize, synth: Box<dyn Synth> },
    /// Install (or replace) the synth that previews sounds outside the song.
    SetPreviewSynth(Box<dyn Synth>),
    /// Play one note of a sound on the preview synth, cutting any preview still sounding.
    PreviewNote { channel: u8, bank: u16, program: u8, key: u8, velocity: u8, duration_frames: u32 },
    SetMasterGain(f32),
    /// Click on every beat while playing.
    SetMetronome(bool),
}

/// The audition slot owner that stands for the preview synth rather than a track.
const PREVIEW_SLOT: usize = usize::MAX;

/// Things the audio thread must not free itself; the UI drops them.
pub enum Garbage {
    Synth(Box<dyn Synth>),
    Song(Arc<RenderSong>),
}

/// Playback state shared with the UI through atomics.
pub struct Transport {
    playing: AtomicBool,
    playhead_frame: AtomicU64,
    sample_rate: u32,
}

impl Transport {
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }
    pub fn playhead_frame(&self) -> u64 {
        self.playhead_frame.load(Ordering::Relaxed)
    }
    pub fn playhead_seconds(&self) -> f64 {
        self.playhead_frame() as f64 / self.sample_rate as f64
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// The metronome's voice: a short sine blip that dies away.
#[derive(Clone, Copy, Default)]
struct Click {
    /// Frames to wait in the current block before it starts.
    delay: usize,
    remaining: u32,
    phase: f32,
    step: f32,
    amp: f32,
    decay: f32,
}

impl Click {
    fn start(&mut self, delay: usize, downbeat: bool, sample_rate: u32) {
        let (hz, gain) = if downbeat { (DOWNBEAT_CLICK_HZ, DOWNBEAT_CLICK_GAIN) } else { (CLICK_HZ, CLICK_GAIN) };
        let frames = (CLICK_SECONDS * f64::from(sample_rate)) as u32;
        *self = Click {
            delay,
            remaining: frames,
            phase: 0.0,
            step: std::f32::consts::TAU * hz / sample_rate as f32,
            amp: gain,
            // Down to about -60 dB by the end of the click.
            decay: (0.001f32).powf(1.0 / frames.max(1) as f32),
        };
    }

    fn mix_into(&mut self, block: &mut [f32], gain: f32) {
        for frame in block.as_chunks_mut::<2>().0.iter_mut() {
            if self.delay > 0 {
                self.delay -= 1;
                continue;
            }
            if self.remaining == 0 {
                return;
            }
            let s = self.phase.sin() * self.amp * gain;
            frame[0] += s;
            frame[1] += s;
            self.phase = (self.phase + self.step) % std::f32::consts::TAU;
            self.amp *= self.decay;
            self.remaining -= 1;
        }
    }
}

/// The first beat in frames `[from, to)`, as (frame, is the first beat of a bar).
fn beat_in(beats: &[BeatSegment], from: u64, to: u64, sample_rate: u32) -> Option<(u64, bool)> {
    let sr = f64::from(sample_rate);
    let t0 = from as f64 / sr;
    let t1 = to as f64 / sr;
    let first = beats.partition_point(|s| s.end <= t0);
    for s in &beats[first..] {
        if s.first_beat >= t1 {
            break;
        }
        if s.beat_seconds <= 0.0 {
            continue;
        }
        let k0 = ((t0 - s.first_beat) / s.beat_seconds).floor().max(0.0) as u64;
        for k in k0..k0 + 2 {
            let t = s.first_beat + k as f64 * s.beat_seconds;
            let frame = (t * sr).round() as u64;
            if t < s.end && (from..to).contains(&frame) {
                return Some((frame, s.is_downbeat(k)));
            }
        }
    }
    None
}

#[derive(Clone, Copy)]
struct Audition {
    track: usize,
    channel: u8,
    key: u8,
    off_at: u64,
}

/// The audio-thread side. Owned by the cpal callback or the offline renderer.
pub struct Engine {
    sample_rate: u32,
    commands: Consumer<AudioCmd>,
    garbage: Producer<Garbage>,
    shared_song: Arc<ArcSwap<RenderSong>>,
    song: Arc<RenderSong>,
    synths: Vec<Option<Box<dyn Synth>>>,
    /// Plays sounds the user is browsing; mixed in at unity, untouched by mute and solo.
    preview: Option<Box<dyn Synth>>,
    sequencer: Sequencer,
    transport: Arc<Transport>,
    playing: bool,
    stop_at_end: bool,
    /// Total frames ever rendered; drives audition note-offs.
    clock: u64,
    auditions: [Option<Audition>; MAX_AUDITIONS],
    master_gain: f32,
    metronome: bool,
    click: Click,
    scratch_l: [f32; SUB_BLOCK],
    scratch_r: [f32; SUB_BLOCK],
}

/// The UI-thread side.
pub struct EngineHandle {
    commands: Producer<AudioCmd>,
    garbage: Consumer<Garbage>,
    song: Arc<ArcSwap<RenderSong>>,
    transport: Arc<Transport>,
}

/// Creates a connected engine/handle pair.
///
/// `stop_at_end`: stop the transport by itself a couple of seconds after the
/// last event (true for live playback, false for offline rendering, which
/// decides its own length).
pub fn create_engine(sample_rate: u32, stop_at_end: bool) -> (Engine, EngineHandle) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(COMMAND_QUEUE_LEN);
    let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_QUEUE_LEN);
    let song = Arc::new(RenderSong::default());
    let shared_song = Arc::new(ArcSwap::from(Arc::clone(&song)));
    let transport = Arc::new(Transport {
        playing: AtomicBool::new(false),
        playhead_frame: AtomicU64::new(0),
        sample_rate,
    });
    let mut synths = Vec::with_capacity(MAX_TRACKS);
    synths.resize_with(MAX_TRACKS, || None);
    let engine = Engine {
        sample_rate,
        commands: cmd_rx,
        garbage: garbage_tx,
        shared_song: Arc::clone(&shared_song),
        song,
        synths,
        preview: None,
        sequencer: Sequencer::new(),
        transport: Arc::clone(&transport),
        playing: false,
        stop_at_end,
        clock: 0,
        auditions: [None; MAX_AUDITIONS],
        master_gain: 1.0,
        metronome: false,
        click: Click::default(),
        scratch_l: [0.0; SUB_BLOCK],
        scratch_r: [0.0; SUB_BLOCK],
    };
    let handle = EngineHandle { commands: cmd_tx, garbage: garbage_rx, song: shared_song, transport };
    (engine, handle)
}

/// Equal-power stereo pan gains for `pan` in -1..=1.
fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (angle.cos(), angle.sin())
}

impl Engine {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// The loop region in frames, while one is switched on.
    fn loop_frames(&self) -> Option<(u64, u64)> {
        let (start, end) = self.song.loop_range?;
        let sr = f64::from(self.sample_rate);
        let (start, end) = ((start * sr).round() as u64, (end * sr).round() as u64);
        (end > start).then_some((start, end))
    }

    /// Renders `out.len() / 2` frames of interleaved stereo audio.
    /// Real-time safe: no allocation, no locks, no I/O.
    pub fn process(&mut self, out: &mut [f32]) {
        self.drain_commands();
        self.sync_song();
        out.fill(0.0);

        let frames = out.len() / 2;
        let mut done = 0;
        while done < frames {
            let mut n = SUB_BLOCK.min(frames - done);
            self.release_finished_auditions();
            let block_start = self.sequencer.frame();
            // Playback that reaches the loop's end from before it jumps back to its start.
            let loop_end = self.loop_frames().filter(|&(_, end)| self.playing && block_start < end);
            if let Some((_, end)) = loop_end {
                n = n.min((end - block_start) as usize);
            }

            if self.playing && self.metronome {
                if let Some((frame, downbeat)) = beat_in(&self.song.beats, block_start, block_start + n as u64, self.sample_rate) {
                    self.click.start((frame - block_start) as usize, downbeat, self.sample_rate);
                }
            }

            if self.playing {
                let song = &self.song;
                let synths = &mut self.synths;
                self.sequencer.advance(song, n, self.sample_rate, |track, _offset, event| {
                    if let Some(Some(synth)) = synths.get_mut(track) {
                        synth.apply(song.tracks[track].channel, event);
                    }
                });
            }

            let any_solo = self.song.tracks.iter().any(|t| t.mix.solo);
            let block = &mut out[done * 2..(done + n) * 2];
            for (i, slot) in self.synths.iter_mut().enumerate() {
                let Some(synth) = slot else { continue };
                // Always render so the synth's internal state advances, even when muted.
                synth.render(&mut self.scratch_l[..n], &mut self.scratch_r[..n]);
                let mix = self.song.tracks.get(i).map(|t| t.mix).unwrap_or_default();
                let audible = !mix.mute && (!any_solo || mix.solo);
                if !audible {
                    continue;
                }
                let (pl, pr) = pan_gains(mix.pan);
                let gl = pl * mix.gain * self.master_gain;
                let gr = pr * mix.gain * self.master_gain;
                for (f, frame) in block.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    frame[0] += self.scratch_l[f] * gl;
                    frame[1] += self.scratch_r[f] * gr;
                }
            }
            if let Some(preview) = &mut self.preview {
                preview.render(&mut self.scratch_l[..n], &mut self.scratch_r[..n]);
                let (pl, pr) = pan_gains(0.0);
                for (f, frame) in block.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    frame[0] += self.scratch_l[f] * pl * self.master_gain;
                    frame[1] += self.scratch_r[f] * pr * self.master_gain;
                }
            }
            self.click.mix_into(block, self.master_gain);

            done += n;
            self.clock += n as u64;
            if let Some((start, end)) = loop_end {
                if self.sequencer.frame() >= end {
                    self.jump_to(start);
                }
            }
        }

        if self.playing {
            self.transport.playhead_frame.store(self.sequencer.frame(), Ordering::Relaxed);
            let end = self.song.length + END_TAIL_SECONDS;
            let loop_ahead = self.loop_frames().is_some_and(|(_, end)| self.sequencer.frame() < end);
            let keeps_going = self.metronome || loop_ahead;
            if self.stop_at_end && !keeps_going && self.sequencer.seconds(self.sample_rate) >= end {
                self.stop();
            }
        }
    }

    /// Moves playback to `frame`, releasing the notes that were sounding on the tracks.
    fn jump_to(&mut self, frame: u64) {
        for synth in self.synths.iter_mut().flatten() {
            synth.all_notes_off();
        }
        self.sequencer.seek(&self.song, frame, self.sample_rate);
        self.resend_controller_state();
    }

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.commands.pop() {
            match cmd {
                AudioCmd::Play => {
                    self.playing = true;
                    self.transport.playing.store(true, Ordering::Relaxed);
                }
                AudioCmd::Stop => self.stop(),
                AudioCmd::Seek { frame } => {
                    self.all_notes_off();
                    self.sequencer.seek(&self.song, frame, self.sample_rate);
                    self.resend_controller_state();
                    self.transport.playhead_frame.store(frame, Ordering::Relaxed);
                }
                AudioCmd::Audition { track, key, velocity, duration_frames } => {
                    let channel = self.song.tracks.get(track).map(|t| t.channel).unwrap_or(0);
                    if let Some(Some(synth)) = self.synths.get_mut(track) {
                        if let Some(slot) = self.auditions.iter_mut().find(|a| a.is_none()) {
                            synth.note_on(channel, key, velocity);
                            *slot = Some(Audition {
                                track,
                                channel,
                                key,
                                off_at: self.clock + duration_frames as u64,
                            });
                        }
                    }
                }
                AudioCmd::SetSynth { track, synth } => {
                    let old = match self.synths.get_mut(track) {
                        Some(slot) => slot.replace(synth),
                        None => Some(synth),
                    };
                    if let Some(old) = old {
                        // If the garbage queue is full the synth drops here; rare and harmless.
                        let _ = self.garbage.push(Garbage::Synth(old));
                    }
                    self.resend_track(track);
                }
                AudioCmd::SetPreviewSynth(synth) => {
                    self.cut_preview();
                    if let Some(old) = self.preview.replace(synth) {
                        let _ = self.garbage.push(Garbage::Synth(old));
                    }
                }
                AudioCmd::PreviewNote { channel, bank, program, key, velocity, duration_frames } => {
                    self.cut_preview();
                    let off_at = self.clock + duration_frames as u64;
                    if let Some(preview) = &mut self.preview {
                        if let Some(slot) = self.auditions.iter_mut().find(|a| a.is_none()) {
                            preview.program_change(channel, bank, program);
                            preview.note_on(channel, key, velocity);
                            *slot = Some(Audition { track: PREVIEW_SLOT, channel, key, off_at });
                        }
                    }
                }
                AudioCmd::SetMasterGain(gain) => self.master_gain = gain.max(0.0),
                AudioCmd::SetMetronome(on) => self.metronome = on,
            }
        }
    }

    fn stop(&mut self) {
        self.playing = false;
        self.all_notes_off();
        self.transport.playing.store(false, Ordering::Relaxed);
    }

    fn all_notes_off(&mut self) {
        for synth in self.synths.iter_mut().flatten() {
            synth.all_notes_off();
        }
        self.cut_preview();
    }

    /// Releases whatever the preview synth is still playing.
    fn cut_preview(&mut self) {
        for slot in self.auditions.iter_mut() {
            if matches!(*slot, Some(a) if a.track == PREVIEW_SLOT) {
                *slot = None;
            }
        }
        if let Some(preview) = &mut self.preview {
            preview.all_notes_off();
        }
    }

    /// Picks up an edited song published by the UI, without interrupting playback.
    fn sync_song(&mut self) {
        let latest = self.shared_song.load();
        if Arc::ptr_eq(&latest, &self.song) {
            return;
        }
        let new = Arc::clone(&latest);
        drop(latest);
        let old = std::mem::replace(&mut self.song, new);
        let _ = self.garbage.push(Garbage::Song(old));
        self.sequencer.seek(&self.song, self.sequencer.frame(), self.sample_rate);
        self.resend_controller_state();
    }

    /// After a seek or a song swap, re-send each track's program, pitch bend and CC
    /// values so e.g. a held sustain pedal is honored mid-song.
    fn resend_controller_state(&mut self) {
        for track in 0..self.song.tracks.len().min(MAX_TRACKS) {
            self.resend_track(track);
        }
    }

    /// Brings one track's synth up to date with the song at the sequencer position.
    /// Before the first event has played, the program is the one the track starts
    /// with, so a freshly installed synth never sits on program 0 by accident.
    fn resend_track(&mut self, track: usize) {
        let state = self.sequencer.controller_state(&self.song, track);
        let song = &self.song;
        let Some(render_track) = song.tracks.get(track) else { return };
        let channel = render_track.channel;
        let Some(Some(synth)) = self.synths.get_mut(track) else { return };
        let program = state.program.or_else(|| {
            render_track.events.iter().find_map(|e| match e.kind {
                EventKind::ProgramChange { bank, program } => Some((bank, program)),
                _ => None,
            })
        });
        if let Some((bank, program)) = program {
            synth.program_change(channel, bank, program);
        }
        if let Some(value) = state.pitch_bend {
            synth.pitch_bend(channel, value);
        }
        for (cc, value) in state.cc.iter().enumerate() {
            if let Some(v) = value {
                synth.control_change(channel, cc as u8, *v);
            }
        }
    }

    fn release_finished_auditions(&mut self) {
        let now = self.clock;
        for slot in self.auditions.iter_mut() {
            let Some(a) = *slot else { continue };
            if a.off_at <= now {
                if a.track == PREVIEW_SLOT {
                    if let Some(preview) = &mut self.preview {
                        preview.note_off(a.channel, a.key);
                    }
                } else if let Some(Some(synth)) = self.synths.get_mut(a.track) {
                    synth.note_off(a.channel, a.key);
                }
                *slot = None;
            }
        }
    }
}

impl EngineHandle {
    pub fn transport(&self) -> &Arc<Transport> {
        &self.transport
    }

    pub fn sample_rate(&self) -> u32 {
        self.transport.sample_rate
    }

    /// Publishes a new song. The audio thread picks it up on its next callback.
    pub fn set_song(&self, song: RenderSong) {
        self.song.store(Arc::new(song));
    }

    pub fn song(&self) -> Arc<RenderSong> {
        self.song.load_full()
    }

    pub fn send(&mut self, cmd: AudioCmd) -> Result<(), AudioError> {
        self.commands.push(cmd).map_err(|_| AudioError::QueueFull)
    }

    pub fn play(&mut self) -> Result<(), AudioError> {
        self.send(AudioCmd::Play)
    }

    pub fn stop(&mut self) -> Result<(), AudioError> {
        self.send(AudioCmd::Stop)
    }

    pub fn seek_seconds(&mut self, seconds: f64) -> Result<(), AudioError> {
        let frame = (seconds.max(0.0) * self.sample_rate() as f64).round() as u64;
        self.send(AudioCmd::Seek { frame })
    }

    pub fn audition(
        &mut self,
        track: usize,
        key: u8,
        velocity: u8,
        seconds: f64,
    ) -> Result<(), AudioError> {
        let duration_frames = (seconds.max(0.0) * self.sample_rate() as f64).round() as u32;
        self.send(AudioCmd::Audition { track, key, velocity, duration_frames })
    }

    pub fn set_synth(&mut self, track: usize, synth: Box<dyn Synth>) -> Result<(), AudioError> {
        if track >= MAX_TRACKS {
            return Err(AudioError::TrackOutOfRange(track));
        }
        self.send(AudioCmd::SetSynth { track, synth })
    }

    pub fn set_preview_synth(&mut self, synth: Box<dyn Synth>) -> Result<(), AudioError> {
        self.send(AudioCmd::SetPreviewSynth(synth))
    }

    /// Plays one note of (`bank`, `program`) on the preview synth for `seconds`.
    pub fn preview_note(
        &mut self,
        channel: u8,
        bank: u16,
        program: u8,
        key: u8,
        velocity: u8,
        seconds: f64,
    ) -> Result<(), AudioError> {
        let duration_frames = (seconds.max(0.0) * self.sample_rate() as f64).round() as u32;
        self.send(AudioCmd::PreviewNote { channel, bank, program, key, velocity, duration_frames })
    }

    pub fn set_master_gain(&mut self, gain: f32) -> Result<(), AudioError> {
        self.send(AudioCmd::SetMasterGain(gain))
    }

    pub fn set_metronome(&mut self, on: bool) -> Result<(), AudioError> {
        self.send(AudioCmd::SetMetronome(on))
    }

    /// Frees whatever the audio thread handed back. Call once per UI frame.
    pub fn collect_garbage(&mut self) -> usize {
        let mut n = 0;
        while self.garbage.pop().is_ok() {
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_song::{RenderTrack, TimedEvent, TrackMix};

    /// A synth that emits DC 1.0 while any note is held, for testing the mixer.
    struct Gate {
        held: u32,
    }
    impl Synth for Gate {
        fn note_on(&mut self, _: u8, _: u8, _: u8) {
            self.held += 1;
        }
        fn note_off(&mut self, _: u8, _: u8) {
            self.held = self.held.saturating_sub(1);
        }
        fn control_change(&mut self, _: u8, _: u8, _: u8) {}
        fn pitch_bend(&mut self, _: u8, _: u16) {}
        fn program_change(&mut self, _: u8, _: u16, _: u8) {}
        fn all_notes_off(&mut self) {
            self.held = 0;
        }
        fn reset(&mut self) {
            self.held = 0;
        }
        fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
            let v = if self.held > 0 { 1.0 } else { 0.0 };
            left.fill(v);
            right.fill(v);
        }
    }

    fn one_note_song(mix: TrackMix) -> RenderSong {
        RenderSong::new(vec![RenderTrack::new(
            0,
            vec![
                TimedEvent { time: 0.0, kind: EventKind::NoteOn { key: 60, velocity: 100 } },
                TimedEvent { time: 1.0, kind: EventKind::NoteOff { key: 60 } },
            ],
            mix,
        )])
    }

    #[test]
    fn plays_note_then_silence_and_reports_playhead() {
        let (mut engine, mut handle) = create_engine(1000, true);
        handle.set_synth(0, Box::new(Gate { held: 0 })).unwrap();
        handle.set_song(one_note_song(TrackMix { pan: -1.0, ..TrackMix::default() }));
        handle.play().unwrap();
        let mut out = vec![0.0; 2 * 500];
        engine.process(&mut out);
        // Hard left: left ≈ 1.0, right ≈ 0.0.
        assert!((out[0] - 1.0).abs() < 1e-5, "left={}", out[0]);
        assert!(out[1].abs() < 1e-5, "right={}", out[1]);
        assert_eq!(handle.transport().playhead_frame(), 500);
        engine.process(&mut out); // 0.5..1.0 still sounding
        engine.process(&mut out); // 1.0..1.5 silent
        assert_eq!(out[0], 0.0);
        assert!(handle.transport().is_playing());
        // Auto-stop after length + tail (1 + 2 s = 3000 frames).
        for _ in 0..5 {
            engine.process(&mut out);
        }
        assert!(!handle.transport().is_playing());
    }

    #[test]
    fn mute_and_solo() {
        let (mut engine, mut handle) = create_engine(1000, false);
        handle.set_synth(0, Box::new(Gate { held: 0 })).unwrap();
        handle.set_song(one_note_song(TrackMix { mute: true, ..TrackMix::default() }));
        handle.play().unwrap();
        let mut out = vec![0.0; 2 * 100];
        engine.process(&mut out);
        assert_eq!(out[0], 0.0, "muted track must be silent");
    }

    #[test]
    fn audition_plays_for_requested_duration() {
        let (mut engine, mut handle) = create_engine(1000, false);
        handle.set_synth(0, Box::new(Gate { held: 0 })).unwrap();
        handle.audition(0, 60, 100, 0.1).unwrap();
        let mut out = vec![0.0; 2 * 64];
        let sounding = |out: &[f32]| out[0] > 0.5; // center pan: 1.0 * cos(45°)
        engine.process(&mut out);
        assert!(sounding(&out));
        engine.process(&mut out); // frames 64..128: note-off is due at 100 but
        assert!(sounding(&out)); // releases happen at sub-block boundaries only
        engine.process(&mut out); // frames 128..192: released
        assert_eq!(out[0], 0.0);
    }

    /// Remembers every program change it is sent, for the tests below.
    struct Recorder {
        programs: Arc<std::sync::Mutex<Vec<(u8, u16, u8)>>>,
    }
    impl Synth for Recorder {
        fn note_on(&mut self, _: u8, _: u8, _: u8) {}
        fn note_off(&mut self, _: u8, _: u8) {}
        fn control_change(&mut self, _: u8, _: u8, _: u8) {}
        fn pitch_bend(&mut self, _: u8, _: u16) {}
        fn program_change(&mut self, channel: u8, bank: u16, program: u8) {
            self.programs.lock().unwrap().push((channel, bank, program));
        }
        fn all_notes_off(&mut self) {}
        fn reset(&mut self) {}
        fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
            left.fill(0.0);
            right.fill(0.0);
        }
    }

    fn song_with_program(program: u8) -> RenderSong {
        RenderSong::new(vec![RenderTrack::new(
            3,
            vec![
                TimedEvent { time: 0.0, kind: EventKind::ProgramChange { bank: 0, program } },
                TimedEvent { time: 0.0, kind: EventKind::NoteOn { key: 60, velocity: 100 } },
                TimedEvent { time: 1.0, kind: EventKind::NoteOff { key: 60 } },
            ],
            TrackMix::default(),
        )])
    }

    #[test]
    fn a_new_synth_and_a_new_song_get_the_tracks_program_without_a_seek() {
        let (mut engine, mut handle) = create_engine(1000, false);
        let programs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut out = vec![0.0; 2 * 64];
        handle.set_song(song_with_program(5));
        engine.process(&mut out);
        handle.set_synth(0, Box::new(Recorder { programs: Arc::clone(&programs) })).unwrap();
        engine.process(&mut out);
        assert_eq!(*programs.lock().unwrap(), vec![(3, 0, 5)], "installing a synth sends the program");
        handle.set_song(song_with_program(7));
        engine.process(&mut out);
        assert_eq!(*programs.lock().unwrap(), vec![(3, 0, 5), (3, 0, 7)], "swapping the song sends the new one");
        engine.process(&mut out);
        assert_eq!(programs.lock().unwrap().len(), 2, "nothing is re-sent while nothing changes");
    }

    #[test]
    fn preview_plays_for_the_requested_duration_and_ignores_mute() {
        let (mut engine, mut handle) = create_engine(1000, false);
        handle.set_synth(0, Box::new(Gate { held: 0 })).unwrap();
        handle.set_song(one_note_song(TrackMix { mute: true, ..TrackMix::default() }));
        handle.play().unwrap();
        let programs = Arc::new(std::sync::Mutex::new(Vec::new()));
        handle.set_preview_synth(Box::new(Recorder { programs: Arc::clone(&programs) })).unwrap();
        handle.set_preview_synth(Box::new(Gate { held: 0 })).unwrap();
        handle.preview_note(9, 0, 8, 38, 100, 0.1).unwrap();
        let mut out = vec![0.0; 2 * 64];
        let sounding = |out: &[f32]| out[0] > 0.5; // centre: 1.0 * cos(45°), the muted track adds nothing
        engine.process(&mut out);
        assert!(sounding(&out));
        engine.process(&mut out);
        assert!(sounding(&out));
        engine.process(&mut out);
        assert_eq!(out[0], 0.0);
        assert_eq!(handle.collect_garbage(), 2, "the first song and the replaced preview synth come back");
        assert!(programs.lock().unwrap().is_empty(), "the replaced preview synth was never played");
    }

    #[test]
    fn a_loop_jumps_back_to_its_start_and_keeps_playing() {
        let (mut engine, mut handle) = create_engine(1000, true);
        handle.set_synth(0, Box::new(Gate { held: 0 })).unwrap();
        let mut song = one_note_song(TrackMix::default());
        song.loop_range = Some((0.25, 0.5));
        handle.set_song(song);
        handle.play().unwrap();
        let mut out = vec![0.0; 2 * 400];
        engine.process(&mut out);
        assert_eq!(handle.transport().playhead_frame(), 400);
        engine.process(&mut out);
        assert_eq!(handle.transport().playhead_frame(), 300, "400..500, 250..500, then 250..300");
        for _ in 0..20 {
            engine.process(&mut out);
        }
        assert!(handle.transport().is_playing(), "a loop never runs into the auto-stop");
        let frame = handle.transport().playhead_frame();
        assert!((250..500).contains(&frame), "{frame}");
    }

    #[test]
    fn playback_starting_after_the_loop_plays_on_through() {
        let (mut engine, mut handle) = create_engine(1000, false);
        let mut song = one_note_song(TrackMix::default());
        song.loop_range = Some((0.25, 0.5));
        handle.set_song(song);
        handle.seek_seconds(0.6).unwrap();
        handle.play().unwrap();
        let mut out = vec![0.0; 2 * 500];
        engine.process(&mut out);
        assert_eq!(handle.transport().playhead_frame(), 1100);
    }

    #[test]
    fn the_metronome_clicks_on_each_beat_louder_on_the_downbeat() {
        let beats = [BeatSegment { first_beat: 0.0, beat_seconds: 0.5, end: f64::INFINITY, beats_per_bar: 4, first_beat_in_bar: 0 }];
        assert_eq!(beat_in(&beats, 0, 64, 1000), Some((0, true)));
        assert_eq!(beat_in(&beats, 64, 500, 1000), None);
        assert_eq!(beat_in(&beats, 450, 514, 1000), Some((500, false)));
        assert_eq!(beat_in(&beats, 500, 564, 1000), Some((500, false)), "a beat on a block edge belongs to one block");
        assert_eq!(beat_in(&beats, 436, 500, 1000), None);
        assert_eq!(beat_in(&beats, 1990, 2054, 1000), Some((2000, true)));

        let (mut engine, mut handle) = create_engine(1000, true);
        handle.set_song(RenderSong { beats: beats.to_vec(), ..RenderSong::default() });
        handle.play().unwrap();
        let mut out = vec![0.0; 2 * 64];
        engine.process(&mut out);
        assert!(out.iter().all(|s| *s == 0.0), "silent while the metronome is off");
        handle.set_metronome(true).unwrap();
        handle.seek_seconds(0.0).unwrap();
        engine.process(&mut out);
        assert!(out.iter().any(|s| s.abs() > 0.1), "clicks once switched on");
        for _ in 0..100 {
            engine.process(&mut out);
        }
        assert!(handle.transport().is_playing(), "the metronome keeps an empty song playing");
    }

    #[test]
    fn replaced_synth_comes_back_as_garbage() {
        let (mut engine, mut handle) = create_engine(1000, false);
        handle.set_synth(0, Box::new(Gate { held: 0 })).unwrap();
        handle.set_synth(0, Box::new(Gate { held: 0 })).unwrap();
        let mut out = vec![0.0; 2 * 64];
        engine.process(&mut out);
        assert_eq!(handle.collect_garbage(), 1);
    }
}
