//! The UI side of the audio engine: owns the device stream and the handle,
//! keeps one synth per track in step with the song's instruments, and does the
//! per-frame housekeeping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mp_audio::stream::{open_default_output, AudioOutput};
use mp_audio::{load_soundfont, render_song_from, AudioError, EngineHandle, SoundFontSynth, MAX_TRACKS};
use mp_core::{Instrument, Song};
use rustysynth::SoundFont;

pub struct Audio {
    _output: AudioOutput,
    pub handle: EngineHandle,
    pub sample_rate: u32,
    /// Used for tracks whose instrument has no path yet.
    pub default_sf2: PathBuf,
    fonts: HashMap<PathBuf, Arc<SoundFont>>,
    /// Which SoundFont each track's synth was built from.
    installed: Vec<Option<PathBuf>>,
}

/// The SoundFont a track should use: its own, or the default when it has none.
pub fn track_sf2(track: &mp_core::Track, default: &Path) -> PathBuf {
    match &track.instrument {
        Instrument::SoundFont { path, .. } if !path.as_os_str().is_empty() => path.clone(),
        Instrument::SoundFont { .. } => default.to_path_buf(),
    }
}

impl Audio {
    /// Opens the default output device and loads the default SoundFont.
    pub fn open(default_sf2: &Path) -> Result<Self, AudioError> {
        let (output, handle) = open_default_output()?;
        let sample_rate = output.sample_rate;
        let mut fonts = HashMap::new();
        fonts.insert(default_sf2.to_path_buf(), load_soundfont(default_sf2)?);
        Ok(Self {
            _output: output,
            handle,
            sample_rate,
            default_sf2: default_sf2.to_path_buf(),
            fonts,
            installed: Vec::new(),
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

    /// The loaded SoundFont for each track (falling back to the default), for
    /// offline rendering. Loads anything not cached yet.
    pub fn fonts_for(&mut self, song: &Song) -> Vec<Option<Arc<SoundFont>>> {
        let default = self.default_sf2.clone();
        song.tracks
            .iter()
            .map(|t| {
                let path = track_sf2(t, &default);
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
            let wanted = track_sf2(track, &self.default_sf2);
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

    pub fn audition(&mut self, track: usize, key: u8, velocity: u8) -> Result<(), AudioError> {
        self.handle.audition(track, key, velocity, 0.3)
    }

    /// Once-per-frame housekeeping: free what the audio thread handed back.
    pub fn frame_tick(&mut self) {
        self.handle.collect_garbage();
    }
}
