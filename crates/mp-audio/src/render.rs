//! Offline rendering: the same [`Engine`](crate::Engine) driven in a loop
//! instead of by a device callback, then written to WAV or MP3.

use std::path::Path;

use mp3lame_encoder::{max_required_buffer_size, Bitrate, Builder, FlushNoGap, InterleavedPcm, Quality};

use crate::engine::create_engine;
use crate::error::AudioError;
use crate::render_song::RenderSong;
use crate::synth::Synth;

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub sample_rate: u32,
    /// Seconds of silence rendered after the last event so release tails finish.
    pub tail_seconds: f64,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { sample_rate: 44_100, tail_seconds: 2.0 }
    }
}

const BLOCK_FRAMES: usize = 512;
/// LAME needs at least this much room to flush its final frames.
const FLUSH_BUFFER_BYTES: usize = 7200;

/// Renders a song to interleaved stereo f32 samples.
///
/// `synths[i]` is the instrument for track `i` (created at `opts.sample_rate`).
pub fn render_song(
    song: RenderSong,
    synths: Vec<Option<Box<dyn Synth>>>,
    opts: RenderOptions,
) -> Result<Vec<f32>, AudioError> {
    let (mut engine, mut handle) = create_engine(opts.sample_rate, false);
    for (i, synth) in synths.into_iter().enumerate() {
        if let Some(s) = synth {
            handle.set_synth(i, s)?;
        }
    }
    let total_frames = ((song.length + opts.tail_seconds) * opts.sample_rate as f64).ceil() as usize;
    handle.set_song(song);
    handle.play()?;

    let mut out = Vec::with_capacity(total_frames * 2);
    let mut block = [0.0f32; BLOCK_FRAMES * 2];
    let mut rendered = 0;
    while rendered < total_frames {
        let n = BLOCK_FRAMES.min(total_frames - rendered);
        engine.process(&mut block[..n * 2]);
        out.extend_from_slice(&block[..n * 2]);
        rendered += n;
    }
    handle.collect_garbage();
    Ok(out)
}

/// Writes 16-bit stereo PCM.
pub fn write_wav(path: impl AsRef<Path>, samples: &[f32], sample_rate: u32) -> Result<(), AudioError> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Encodes interleaved stereo f32 samples to MP3 (CBR, LAME).
pub fn encode_mp3(samples: &[f32], sample_rate: u32, bitrate: Bitrate) -> Result<Vec<u8>, AudioError> {
    let mp3 = |e: &dyn std::fmt::Debug| AudioError::Mp3(format!("{e:?}"));
    let mut builder = Builder::new().ok_or_else(|| AudioError::Mp3("LAME init failed".into()))?;
    builder.set_num_channels(2).map_err(|e| mp3(&e))?;
    builder.set_sample_rate(sample_rate).map_err(|e| mp3(&e))?;
    builder.set_brate(bitrate).map_err(|e| mp3(&e))?;
    builder.set_quality(Quality::Best).map_err(|e| mp3(&e))?;
    let mut encoder = builder.build().map_err(|e| mp3(&e))?;
    // LAME reads the output buffer size as "unlimited" when it is 0, so the
    // Vec must have real spare capacity before every call.
    let frames = samples.len() / 2;
    let mut out = Vec::with_capacity(max_required_buffer_size(frames));
    encoder.encode_to_vec(InterleavedPcm(samples), &mut out).map_err(|e| mp3(&e))?;
    out.reserve(FLUSH_BUFFER_BYTES);
    encoder.flush_to_vec::<FlushNoGap>(&mut out).map_err(|e| mp3(&e))?;
    Ok(out)
}

pub fn write_mp3(path: impl AsRef<Path>, samples: &[f32], sample_rate: u32) -> Result<(), AudioError> {
    let bytes = encode_mp3(samples, sample_rate, Bitrate::Kbps192)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Writes WAV or MP3 depending on the file extension.
pub fn write_audio_file(path: impl AsRef<Path>, samples: &[f32], sample_rate: u32) -> Result<(), AudioError> {
    let path = path.as_ref();
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp3") => write_mp3(path, samples, sample_rate),
        _ => write_wav(path, samples, sample_rate),
    }
}
