//! Offline render through the real SoundFont synth, to WAV and MP3.

mod common;

use mp_audio::demo::demo_song;
use mp_audio::render::{encode_mp3, render_song, write_wav, RenderOptions};
use mp_audio::{load_soundfont, SoundFontSynth, Synth};

fn rendered() -> (Vec<f32>, u32) {
    let opts = RenderOptions { sample_rate: 44_100, tail_seconds: 1.0 };
    let sf = load_soundfont(common::test_soundfont()).unwrap();
    let synths: Vec<Option<Box<dyn Synth>>> = (0..2)
        .map(|_| Some(Box::new(SoundFontSynth::new(&sf, opts.sample_rate).unwrap()) as Box<dyn Synth>))
        .collect();
    (render_song(demo_song(), synths, opts).unwrap(), opts.sample_rate)
}

#[test]
fn demo_renders_audible_audio_of_the_right_length() {
    let (samples, sr) = rendered();
    let song = demo_song();
    let expected_frames = ((song.length + 1.0) * sr as f64).ceil() as usize;
    assert_eq!(samples.len(), expected_frames * 2);
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "peak {peak} too quiet: is the SoundFont silent?");
    assert!(peak <= 1.0, "peak {peak} clips");
    // The tail must have decayed.
    let last = &samples[samples.len() - 2000..];
    assert!(last.iter().all(|s| s.abs() < 0.05));
}

#[test]
fn wav_and_mp3_round_trip() {
    let (samples, sr) = rendered();
    let dir = std::env::temp_dir().join(format!("mp-render-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("demo.wav");
    write_wav(&wav, &samples, sr).unwrap();
    let reader = hound::WavReader::open(&wav).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.len() as usize, samples.len());

    let mp3 = encode_mp3(&samples, sr, mp3lame_encoder::Bitrate::Kbps128).unwrap();
    let seconds = samples.len() as f64 / 2.0 / sr as f64;
    let expected_bytes = 128_000.0 / 8.0 * seconds;
    assert!(
        (mp3.len() as f64) > expected_bytes * 0.8 && (mp3.len() as f64) < expected_bytes * 1.3,
        "mp3 is {} bytes, expected about {expected_bytes}",
        mp3.len()
    );
    // MP3 frame sync (0xFFE…) or an ID3 header.
    assert!(mp3.starts_with(b"ID3") || (mp3[0] == 0xFF && mp3[1] & 0xE0 == 0xE0));
    std::fs::remove_dir_all(&dir).ok();
}
