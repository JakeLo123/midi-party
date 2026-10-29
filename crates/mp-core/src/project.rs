//! Project files: a [`Song`] saved as RON (`.ron`, human-readable, diff-friendly).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::song::Song;

/// Layout version written into every project file. Raise it when a change
/// would stop older builds from reading new files, and make [`from_ron_str`]
/// upgrade files with a lower number.
pub const FORMAT_VERSION: u32 = 1;

/// Just the version stamp of a project file, read before the rest so a file
/// from a newer build gets a clear message instead of a parse error.
#[derive(Deserialize)]
#[serde(rename = "Song")]
struct Stamp {
    #[serde(default)]
    format_version: u32,
}

/// Problems saving or loading a project file.
#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("project file is not valid: {0}")]
    Parse(#[from] ron::error::SpannedError),
    #[error("saved by a newer midi-party (project format {found}; this build reads up to {FORMAT_VERSION}): update midi-party to open it")]
    NewerFormat { found: u32 },
    #[error("cannot serialize project: {0}")]
    Serialize(#[from] ron::Error),
}

/// Serializes a song to pretty-printed RON.
pub fn to_ron_string(song: &Song) -> Result<String, ProjectError> {
    let config = ron::ser::PrettyConfig::new().depth_limit(6);
    Ok(ron::ser::to_string_pretty(song, config)?)
}

/// Parses a song from RON text. Tempo and meter maps are validated on the way in.
pub fn from_ron_str(text: &str) -> Result<Song, ProjectError> {
    let Stamp { format_version } = ron::from_str(text)?;
    if format_version > FORMAT_VERSION {
        return Err(ProjectError::NewerFormat { found: format_version });
    }
    let mut song: Song = ron::from_str(text)?;
    song.format_version = FORMAT_VERSION;
    song.normalize();
    Ok(song)
}

/// Writes the song to `path` as RON.
pub fn save(song: &Song, path: impl AsRef<Path>) -> Result<(), ProjectError> {
    let path = path.as_ref();
    let text = to_ron_string(song)?;
    std::fs::write(path, text).map_err(|source| ProjectError::Write { path: path.to_path_buf(), source })
}

/// Reads a song from a RON file at `path`.
pub fn load(path: impl AsRef<Path>) -> Result<Song, ProjectError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| ProjectError::Read { path: path.to_path_buf(), source })?;
    from_ron_str(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::{Instrument, LanePoint, Tick, Track};

    fn sample() -> Song {
        let mut s = Song::new_empty();
        s.tempo_map.set_tempo_at(Tick(3840), 90.5).unwrap();
        s.meter_map.set_meter_at(Tick(3840), 3, 4).unwrap();
        let n1 = s.new_note(Tick(480), Tick(240), 60, 100);
        let n2 = s.new_note(Tick(0), Tick(960), 64, 80);
        s.tracks[0].insert_note(n1);
        s.tracks[0].insert_note(n2);
        s.tracks[0].lanes.sustain.push(LanePoint { tick: Tick(10), value: 127 });
        let mut t2 = Track::new("Bass");
        t2.instrument = Instrument::SoundFont { path: "sounds/gm.sf2".into(), bank: 0, preset: 33 };
        t2.volume = 0.8;
        t2.pan = -0.25;
        t2.mute = true;
        t2.channel = 1;
        s.add_track(t2);
        s
    }

    #[test]
    fn ron_round_trip() {
        let song = sample();
        let text = to_ron_string(&song).unwrap();
        assert!(text.contains("Bass"));
        let back = from_ron_str(&text).unwrap();
        assert_eq!(back, song);
    }

    #[test]
    fn file_round_trip() {
        let song = sample();
        let dir = std::env::temp_dir().join(format!("mp-core-project-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.ron");
        save(&song, &path).unwrap();
        assert_eq!(load(&path).unwrap(), song);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(load(&path), Err(ProjectError::Read { .. })));
        assert!(matches!(save(&song, dir.join("nope").join("x.ron")), Err(ProjectError::Write { .. })));
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(matches!(from_ron_str("(ppq: 960"), Err(ProjectError::Parse(_))));
        // a tempo map whose first point is not at tick 0 fails validation on load
        let text = to_ron_string(&sample()).unwrap().replacen("tick: 0,", "tick: 7,", 1);
        let err = from_ron_str(&text).unwrap_err();
        assert!(err.to_string().contains("tick 0"), "{err}");
    }

    #[test]
    fn saved_files_carry_the_format_version() {
        let text = to_ron_string(&sample()).unwrap();
        assert!(text.contains(&format!("format_version: {FORMAT_VERSION},")), "{text}");
    }

    #[test]
    fn unstamped_file_loads_at_the_current_version() {
        let text = to_ron_string(&sample()).unwrap().replacen(&format!("format_version: {FORMAT_VERSION},"), "", 1);
        assert_eq!(from_ron_str(&text).unwrap(), sample());
    }

    #[test]
    fn newer_format_is_refused_with_a_clear_message() {
        let newer = FORMAT_VERSION + 1;
        let text = to_ron_string(&sample())
            .unwrap()
            .replacen(&format!("format_version: {FORMAT_VERSION},"), &format!("format_version: {newer}, future_field: 1,"), 1);
        let err = from_ron_str(&text).unwrap_err();
        assert!(matches!(err, ProjectError::NewerFormat { found } if found == newer));
        assert!(err.to_string().contains("update midi-party"), "{err}");
    }

    #[test]
    fn header_width_round_trips_and_defaults_when_missing() {
        let mut song = sample();
        song.track_header_width = 300.0;
        let text = to_ron_string(&song).unwrap();
        assert!(text.contains("track_header_width: 300.0,"), "{text}");
        assert_eq!(from_ron_str(&text).unwrap().track_header_width, 300.0);
        let older = text.replacen("track_header_width: 300.0,", "", 1);
        assert_eq!(from_ron_str(&older).unwrap().track_header_width, crate::song::DEFAULT_TRACK_HEADER_WIDTH);
        let broken = text.replacen("track_header_width: 300.0,", "track_header_width: -5.0,", 1);
        assert_eq!(from_ron_str(&broken).unwrap().track_header_width, crate::song::DEFAULT_TRACK_HEADER_WIDTH);
    }

    #[test]
    fn load_sorts_notes() {
        let mut song = sample();
        song.tracks[0].notes.reverse();
        let back = from_ron_str(&to_ron_string(&song).unwrap()).unwrap();
        assert_eq!(back.tracks[0].notes[0].start, Tick(0));
    }
}
