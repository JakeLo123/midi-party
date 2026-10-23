//! Real-time audio engine, sequencer, and offline render for midi-party.
//!
//! The audio side works purely in **seconds and sample frames**. Ticks and the
//! tempo map live in `mp-core`; [`convert::render_song_from`] turns a `Song`
//! into an immutable [`RenderSong`] (events in seconds) that the engine plays
//! after [`EngineHandle::set_song`].
//!
//! Threading model:
//! - UI -> audio commands: an SPSC ring buffer of [`AudioCmd`].
//! - Song edits: an `ArcSwap<RenderSong>` the audio thread reads once per callback.
//! - Audio -> UI: atomics in [`Transport`] (playing flag, playhead frame).
//! - Anything the audio thread must free (old songs, replaced synths) is sent
//!   back to the UI through a garbage queue so the callback never deallocates.
//!
//! [`Engine::process`] has no device knowledge: the cpal stream calls it, and
//! so does the offline renderer, so playback and export share one code path.

pub mod convert;
pub mod demo;
pub mod engine;
pub mod error;
pub mod render;
pub mod render_song;
pub mod sequencer;
pub mod stream;
pub mod synth;

pub use convert::render_song_from;
pub use engine::{create_engine, AudioCmd, Engine, EngineHandle, Transport};
pub use error::AudioError;
pub use render_song::{EventKind, RenderSong, RenderTrack, TimedEvent, TrackMix, MAX_TRACKS};
pub use synth::{load_soundfont, SoundFontSynth, Synth};
