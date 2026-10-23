//! Opens the default output device and runs an [`Engine`](crate::Engine) in its callback.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::engine::{create_engine, EngineHandle};
use crate::error::AudioError;

/// Keeps the device stream alive. Drop it to stop audio.
pub struct AudioOutput {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Max frames per callback we preallocate for the non-stereo fallback path.
const FALLBACK_FRAMES: usize = 8192;

/// Starts live audio on the default output device.
pub fn open_default_output() -> Result<(AudioOutput, EngineHandle), AudioError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(AudioError::NoOutputDevice)?;
    let supported = device.default_output_config()?;
    if supported.sample_format() != cpal::SampleFormat::F32 {
        return Err(AudioError::UnsupportedFormat(supported.sample_format()));
    }
    let channels = supported.channels();
    let sample_rate = supported.sample_rate();
    let config = cpal::StreamConfig {
        channels,
        sample_rate: supported.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };

    let (mut engine, handle) = create_engine(sample_rate, true);
    let mut stereo = vec![0.0f32; 2 * FALLBACK_FRAMES];
    let ch = channels as usize;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
            if ch == 2 {
                engine.process(data);
                return;
            }
            let frames = data.len() / ch;
            let need = frames * 2;
            if stereo.len() < need {
                // Only reachable with an unusually large device buffer; grows once.
                stereo.resize(need, 0.0);
            }
            engine.process(&mut stereo[..need]);
            for (f, frame) in data.chunks_exact_mut(ch).enumerate() {
                for (c, sample) in frame.iter_mut().enumerate() {
                    *sample = if c < 2 { stereo[f * 2 + c] } else { 0.0 };
                }
            }
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )?;
    stream.play()?;

    Ok((AudioOutput { _stream: stream, sample_rate, channels }, handle))
}
