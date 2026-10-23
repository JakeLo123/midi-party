//! Project files: a [`Song`] saved as RON (`.ron`, human-readable, diff-friendly).

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::song::Song;

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
    let mut song: Song = ron::from_str(text)?;
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
    fn load_sorts_notes() {
        let mut song = sample();
        song.tracks[0].notes.reverse();
        let back = from_ron_str(&to_ron_string(&song).unwrap()).unwrap();
        assert_eq!(back.tracks[0].notes[0].start, Tick(0));
    }
}
