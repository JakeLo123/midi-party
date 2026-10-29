//! The UI side of the audio engine: owns the device stream and the handle,
//! keeps one synth per track in step with the song's instruments, and does the
//! per-frame housekeeping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mp_audio::stream::{open_default_output, AudioOutput};
use mp_audio::{
    has_drum_kit, load_soundfont, render_song_from, AudioError, EngineHandle, SoundFontSynth, DRUM_CHANNEL,
    MAX_TRACKS,
};
use mp_core::{Instrument, Song, Track};
use rustysynth::SoundFont;

pub struct Audio {
    _output: AudioOutput,
    pub handle: EngineHandle,
    pub sample_rate: u32,
    /// Used for tracks whose instrument has no path yet.
    pub default_sf2: PathBuf,
    /// Used instead for channel-10 tracks whose instrument has no path yet;
    /// `None` when the default SoundFont has drum kits of its own.
    pub drum_sf2: Option<PathBuf>,
    fonts: HashMap<PathBuf, Arc<SoundFont>>,
    /// Which SoundFont each track's synth was built from.
    installed: Vec<Option<PathBuf>>,
    /// The SoundFont the engine's preview synth was built from.
    preview_font: Option<PathBuf>,
}

/// One sound a SoundFont offers, by the name stored in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetInfo {
    pub bank: u16,
    pub program: u8,
    pub name: String,
}

/// The presets of one SoundFont that the picker can show for a track.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetList {
    pub font: PathBuf,
    pub presets: Vec<PresetInfo>,
}

/// The bank-0 presets (`drums = false`) or the bank-128 kits of a font, sorted by
/// program with duplicates dropped.
fn presets_in(font: &SoundFont, drums: bool) -> Vec<PresetInfo> {
    let mut presets: Vec<PresetInfo> = font
        .get_presets()
        .iter()
        .filter(|p| if drums { p.get_bank_number() >= 128 } else { p.get_bank_number() == 0 })
        .filter(|p| (0..=127).contains(&p.get_patch_number()))
        .map(|p| PresetInfo { bank: p.get_bank_number() as u16, program: p.get_patch_number() as u8, name: p.get_name().to_string() })
        .collect();
    presets.sort_by_key(|p| p.program);
    presets.dedup_by_key(|p| p.program);
    presets
}

/// The SoundFont a track should use: its own, or when it has none, the drum kit
/// on channel 10 and the default elsewhere.
pub fn track_sf2(track: &mp_core::Track, default: &Path, drums: Option<&Path>) -> PathBuf {
    match &track.instrument {
        Instrument::SoundFont { path, .. } if !path.as_os_str().is_empty() => path.clone(),
        Instrument::SoundFont { .. } => match drums {
            Some(kit) if track.channel == DRUM_CHANNEL => kit.to_path_buf(),
            _ => default.to_path_buf(),
        },
    }
}

impl Audio {
    /// Opens the default output device and loads the default SoundFont. The drum
    /// kit is only loaded once a channel-10 track needs it.
    pub fn open(default_sf2: &Path, drum_sf2: &Path) -> Result<Self, AudioError> {
        let (output, handle) = open_default_output()?;
        let sample_rate = output.sample_rate;
        let default_font = load_soundfont(default_sf2)?;
        let drum_sf2 = (!has_drum_kit(&default_font)).then(|| drum_sf2.to_path_buf());
        let mut fonts = HashMap::new();
        fonts.insert(default_sf2.to_path_buf(), default_font);
        Ok(Self {
            _output: output,
            handle,
            sample_rate,
            default_sf2: default_sf2.to_path_buf(),
            drum_sf2,
            fonts,
            installed: Vec::new(),
            preview_font: None,
        })
    }

    fn font(&mut self, path: &Path) -> Result<Arc<SoundFont>, AudioError> {
        if let Some(f) = self.fonts.get(path) {
            return Ok(Arc::clone(f));
        }
        let f = load_soundfont(path)?;
        self.fonts.insert(path.to_path_buf(), Arc::clone(&f));
        Ok(f)
    }

    /// The font holding the drum kits a track can pick: its own if that has kits,
    /// else the app's drum kit, else the default font.
    fn drum_font_for(&mut self, track: &Track) -> PathBuf {
        let own = track_sf2(track, &self.default_sf2, None);
        if own != self.default_sf2 && self.font(&own).map(|f| has_drum_kit(&f)).unwrap_or(false) {
            return own;
        }
        self.drum_sf2.clone().unwrap_or_else(|| self.default_sf2.clone())
    }

    /// The instruments and the drum kits a track can be switched to, read from the
    /// SoundFonts it would play them with. Only sounds that exist are listed.
    pub fn presets_for(&mut self, track: &Track) -> (PresetList, PresetList) {
        let melodic_font = track_sf2(track, &self.default_sf2, None);
        let melodic = self.font(&melodic_font).map(|f| presets_in(&f, false)).unwrap_or_default();
        let drum_font = self.drum_font_for(track);
        let drums = self.font(&drum_font).map(|f| presets_in(&f, true)).unwrap_or_default();
        (PresetList { font: melodic_font, presets: melodic }, PresetList { font: drum_font, presets: drums })
    }

    /// The name the track's SoundFont gives its current sound, if the font has it.
    pub fn preset_name(&mut self, track: &Track) -> Option<String> {
        let Instrument::SoundFont { bank, preset, .. } = &track.instrument;
        let path = track_sf2(track, &self.default_sf2, self.drum_sf2.as_deref());
        let font = self.font(&path).ok()?;
        let wanted_bank = if track.is_drums() { 128 + i32::from(*bank) } else { i32::from(*bank) };
        font.get_presets()
            .iter()
            .find(|p| p.get_bank_number() == wanted_bank && p.get_patch_number() == i32::from(*preset))
            .map(|p| p.get_name().to_string())
    }

    /// Plays one short note of a sound on the engine's preview synth, which is
    /// separate from every track's synth so browsing changes nothing in the song.
    pub fn preview(&mut self, font: &Path, channel: u8, bank: u16, program: u8, key: u8, velocity: u8) -> Result<(), AudioError> {
        if self.preview_font.as_deref() != Some(font) {
            let loaded = self.font(font)?;
            let synth = SoundFontSynth::new(&loaded, self.sample_rate)?;
            self.handle.set_preview_synth(Box::new(synth))?;
            self.preview_font = Some(font.to_path_buf());
        }
        self.handle.preview_note(channel, bank, program, key, velocity, 0.3)
    }

    /// The loaded SoundFont for each track (falling back to the default), for
    /// offline rendering. Loads anything not cached yet.
    pub fn fonts_for(&mut self, song: &Song) -> Vec<Option<Arc<SoundFont>>> {
        let default = self.default_sf2.clone();
        let drums = self.drum_sf2.clone();
        song.tracks
            .iter()
            .map(|t| {
                let path = track_sf2(t, &default, drums.as_deref());
                self.font(&path).or_else(|_| self.font(&default)).ok()
            })
            .collect()
    }

    /// Publishes the song to the engine and (re)builds the synth of any track
    /// whose SoundFont changed. Returns human-readable warnings (a SoundFont
    /// that failed to load falls back to the default one).
    pub fn sync_song(&mut self, song: &Song) -> Vec<String> {
        let mut warnings = Vec::new();
        for (i, track) in song.tracks.iter().enumerate().take(MAX_TRACKS) {
            let wanted = track_sf2(track, &self.default_sf2, self.drum_sf2.as_deref());
            if self.installed.get(i).and_then(|p| p.as_deref()) == Some(wanted.as_path()) {
                continue;
            }
            let font = match self.font(&wanted) {
                Ok(f) => f,
                Err(e) => {
                    warnings.push(format!("{}: {e}; using default SoundFont", wanted.display()));
                    match self.font(&self.default_sf2.clone()) {
                        Ok(f) => f,
                        Err(e) => {
                            warnings.push(format!("default SoundFont: {e}"));
                            continue;
                        }
                    }
                }
            };
            let result = SoundFontSynth::new(&font, self.sample_rate)
                .and_then(|synth| self.handle.set_synth(i, Box::new(synth)));
            match result {
                Ok(()) => {
                    if self.installed.len() <= i {
                        self.installed.resize(i + 1, None);
                    }
                    self.installed[i] = Some(wanted);
                }
                Err(e) => warnings.push(format!("track {}: {e}", i + 1)),
            }
        }
        self.handle.set_song(render_song_from(song));
        warnings
    }

    pub fn is_playing(&self) -> bool {
        self.handle.transport().is_playing()
    }

    pub fn playhead_seconds(&self) -> f64 {
        self.handle.transport().playhead_seconds()
    }

    pub fn play_from(&mut self, seconds: f64) -> Result<(), AudioError> {
        self.handle.seek_seconds(seconds)?;
        self.handle.play()
    }

    pub fn stop(&mut self) -> Result<(), AudioError> {
        self.handle.stop()
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), AudioError> {
        self.handle.seek_seconds(seconds)
    }

    pub fn audition(&mut self, track: usize, key: u8, velocity: u8) -> Result<(), AudioError> {
        self.handle.audition(track, key, velocity, 0.3)
    }

    /// Once-per-frame housekeeping: free what the audio thread handed back.
    pub fn frame_tick(&mut self) {
        self.handle.collect_garbage();
    }
}
