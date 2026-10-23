use thiserror::Error;

/// Every error the audio crate can produce.
#[derive(Error, Debug)]
pub enum AudioError {
    #[error("no default audio output device")]
    NoOutputDevice,
    #[error("audio device sample format {0:?} is not supported (need f32)")]
    UnsupportedFormat(cpal::SampleFormat),
    #[error("audio device: {0}")]
    Device(#[from] cpal::Error),
    #[error("soundfont: {0}")]
    SoundFont(String),
    #[error("synthesizer: {0}")]
    Synth(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("wav: {0}")]
    Wav(#[from] hound::Error),
    #[error("mp3: {0}")]
    Mp3(String),
    #[error("audio command queue is full")]
    QueueFull,
    #[error("track index {0} is out of range (max {max})", max = crate::MAX_TRACKS - 1)]
    TrackOutOfRange(usize),
}
