//! The instrument boundary. Everything the engine needs from a sound source.
//!
//! Today the only implementation is [`SoundFontSynth`] (rustysynth). A plugin
//! host would be another implementation of [`Synth`].

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

use crate::error::AudioError;
use crate::render_song::EventKind;

/// A polyphonic instrument the engine can drive with MIDI-like events.
///
/// Implementations must be real-time safe in every method: no allocation,
/// no locking, no I/O. They are created on the UI thread and moved to the
/// audio thread, so they must be `Send`.
pub trait Synth: Send {
    fn note_on(&mut self, channel: u8, key: u8, velocity: u8);
    fn note_off(&mut self, channel: u8, key: u8);
    fn control_change(&mut self, channel: u8, controller: u8, value: u8);
    /// 14-bit pitch bend, 0..=16383, 8192 = center.
    fn pitch_bend(&mut self, channel: u8, value: u16);
    fn program_change(&mut self, channel: u8, bank: u16, program: u8);
    /// Release every sounding note (with its normal release tail).
    fn all_notes_off(&mut self);
    /// Silence everything and reset controllers.
    fn reset(&mut self);
    /// Render `left.len()` frames into the buffers, overwriting them.
    fn render(&mut self, left: &mut [f32], right: &mut [f32]);

    /// Dispatches a sequencer event to the matching method.
    fn apply(&mut self, channel: u8, event: EventKind) {
        match event {
            EventKind::NoteOn { key, velocity } => self.note_on(channel, key, velocity),
            EventKind::NoteOff { key } => self.note_off(channel, key),
            EventKind::ControlChange { controller, value } => {
                self.control_change(channel, controller, value)
            }
            EventKind::PitchBend { value } => self.pitch_bend(channel, value),
            EventKind::ProgramChange { bank, program } => {
                self.program_change(channel, bank, program)
            }
        }
    }
}

/// Reads an `.sf2` file into memory. Share the result between synths with `Arc`.
pub fn load_soundfont(path: impl AsRef<Path>) -> Result<Arc<SoundFont>, AudioError> {
    let mut file = File::open(path)?;
    let sf = SoundFont::new(&mut file).map_err(|e| AudioError::SoundFont(format!("{e:?}")))?;
    Ok(Arc::new(sf))
}

/// Whether the SoundFont carries a drum kit (any bank-128 preset, what channel 10 plays).
pub fn has_drum_kit(font: &SoundFont) -> bool {
    font.get_presets().iter().any(|p| p.get_bank_number() >= 128)
}

/// A SoundFont-backed instrument.
pub struct SoundFontSynth {
    inner: Synthesizer,
}

impl SoundFontSynth {
    pub fn new(sound_font: &Arc<SoundFont>, sample_rate: u32) -> Result<Self, AudioError> {
        let mut settings = SynthesizerSettings::new(sample_rate as i32);
        settings.enable_reverb_and_chorus = true;
        let inner = Synthesizer::new(sound_font, &settings)
            .map_err(|e| AudioError::Synth(format!("{e:?}")))?;
        Ok(Self { inner })
    }
}

const CONTROL_CHANGE: i32 = 0xB0;
const PROGRAM_CHANGE: i32 = 0xC0;
const PITCH_BEND: i32 = 0xE0;
const CC_BANK_MSB: i32 = 0;
const CC_BANK_LSB: i32 = 32;

impl Synth for SoundFontSynth {
    fn note_on(&mut self, channel: u8, key: u8, velocity: u8) {
        self.inner.note_on(channel as i32, key as i32, velocity as i32);
    }

    fn note_off(&mut self, channel: u8, key: u8) {
        self.inner.note_off(channel as i32, key as i32);
    }

    fn control_change(&mut self, channel: u8, controller: u8, value: u8) {
        self.inner
            .process_midi_message(channel as i32, CONTROL_CHANGE, controller as i32, value as i32);
    }

    fn pitch_bend(&mut self, channel: u8, value: u16) {
        let lsb = (value & 0x7F) as i32;
        let msb = ((value >> 7) & 0x7F) as i32;
        self.inner.process_midi_message(channel as i32, PITCH_BEND, lsb, msb);
    }

    fn program_change(&mut self, channel: u8, bank: u16, program: u8) {
        let ch = channel as i32;
        self.inner.process_midi_message(ch, CONTROL_CHANGE, CC_BANK_MSB, ((bank >> 7) & 0x7F) as i32);
        self.inner.process_midi_message(ch, CONTROL_CHANGE, CC_BANK_LSB, (bank & 0x7F) as i32);
        self.inner.process_midi_message(ch, PROGRAM_CHANGE, program as i32, 0);
    }

    fn all_notes_off(&mut self) {
        self.inner.note_off_all(false);
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
        self.inner.render(left, right);
    }
}
