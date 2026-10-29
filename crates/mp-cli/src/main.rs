//! Headless tools: drive the editor with key strings, play, or render without the GUI.

use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use clap::{Parser, Subcommand};
use mp_audio::demo::demo_song;
use mp_audio::render::{render_song, write_audio_file, RenderOptions};
use mp_audio::stream::open_default_output;
use mp_audio::{
    has_drum_kit, load_soundfont, render_song_from, RenderSong, RenderTrack, SoundFontSynth, Synth, DRUM_CHANNEL,
};
use mp_core::{default_keymap, project, try_drive_with, Editor, Keymap, Song};

#[derive(Parser)]
#[command(name = "mp", version, about = "midi-party headless tools")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Feed a key string to the headless editor and print the resulting song.
    ///
    /// Example: mp keys "n Right n Right n Shift+Left Shift+Left Alt+Up"
    Keys {
        keys: String,
        /// Start from this .ron project instead of an empty song.
        #[arg(long)]
        song: Option<PathBuf>,
        /// Keymap file (default: the built-in default keymap).
        #[arg(long)]
        keymap: Option<PathBuf>,
        /// Write the resulting song here as .ron.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Play one note through the default output device.
    PlayNote {
        #[arg(long)]
        sf2: PathBuf,
        #[arg(long, default_value_t = 60)]
        key: u8,
        #[arg(long, default_value_t = 1.5)]
        seconds: f64,
    },
    /// Play the built-in demo song live.
    Demo {
        #[arg(long)]
        sf2: PathBuf,
    },
    /// Render a .ron project (or the built-in demo) to a .wav or .mp3 file.
    Render {
        #[arg(long)]
        sf2: PathBuf,
        /// Drum kit for channel-10 tracks, used when --sf2 has no kits of its own.
        #[arg(long, default_value = "assets/soundfonts/fluidr3-standard-kit.sf2")]
        drums_sf2: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Project file; omit to render the demo.
        #[arg(long)]
        song: Option<PathBuf>,
        #[arg(long, default_value_t = 44_100)]
        sample_rate: u32,
    },
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

type SynthList = Vec<Option<Box<dyn Synth>>>;

/// One instrument per track from the SoundFont, except that channel-10 tracks
/// play the drum kit when the SoundFont has no kits of its own.
fn synths_for(sf2: &Path, drums_sf2: &Path, tracks: &[RenderTrack], sample_rate: u32) -> Result<SynthList, Box<dyn Error>> {
    let sf = load_soundfont(sf2)?;
    let needs_kit = !has_drum_kit(&sf) && tracks.iter().any(|t| t.channel == DRUM_CHANNEL);
    let kit = if needs_kit { Some(load_soundfont(drums_sf2)?) } else { None };
    tracks
        .iter()
        .map(|t| {
            let font = match &kit {
                Some(kit) if t.channel == DRUM_CHANNEL => kit,
                _ => &sf,
            };
            Ok(Some(Box::new(SoundFontSynth::new(font, sample_rate)?) as Box<dyn Synth>))
        })
        .collect()
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.cmd {
        Cmd::Keys { keys, song, keymap, out } => {
            let song = match song {
                Some(path) => project::load(&path)?,
                None => Song::new_empty(),
            };
            let mut keymap = match keymap {
                Some(path) => Keymap::load(&path)?,
                None => default_keymap(),
            };
            let mut editor = Editor::new(song);
            let events = try_drive_with(&mut editor, &mut keymap, &keys)?;
            for event in &events {
                println!("event: {event:?}");
            }
            println!(
                "cursor {}:{}  selected {}",
                editor.cursor.tick.get(),
                editor.cursor.pitch,
                editor.selection.len()
            );
            if let Some(track) = editor.track() {
                for n in &track.notes {
                    println!("note {:>6} +{:<5} pitch {:3} vel {:3}", n.start.get(), n.duration.get(), n.pitch, n.velocity);
                }
            }
            if let Some(out) = out {
                project::save(&editor.song, &out)?;
                println!("wrote {}", out.display());
            }
        }
        Cmd::PlayNote { sf2, key, seconds } => {
            let (output, mut handle) = open_default_output()?;
            let sf = load_soundfont(sf2)?;
            handle.set_synth(0, Box::new(SoundFontSynth::new(&sf, output.sample_rate)?))?;
            handle.audition(0, key, 100, seconds)?;
            println!("playing key {key} for {seconds}s at {} Hz", output.sample_rate);
            sleep(Duration::from_secs_f64(seconds + 1.0));
            handle.collect_garbage();
        }
        Cmd::Demo { sf2 } => {
            let (output, mut handle) = open_default_output()?;
            let sf = load_soundfont(sf2)?;
            for track in 0..2 {
                handle.set_synth(track, Box::new(SoundFontSynth::new(&sf, output.sample_rate)?))?;
            }
            handle.set_song(demo_song());
            handle.play()?;
            sleep(Duration::from_millis(50));
            while handle.transport().is_playing() {
                print!("\r{:6.2}s", handle.transport().playhead_seconds());
                std::io::stdout().flush().ok();
                sleep(Duration::from_millis(100));
                handle.collect_garbage();
            }
            println!();
        }
        Cmd::Render { sf2, drums_sf2, out, song, sample_rate } => {
            let render: RenderSong = match song {
                Some(path) => render_song_from(&project::load(&path)?),
                None => demo_song(),
            };
            let synths = synths_for(&sf2, &drums_sf2, &render.tracks, sample_rate)?;
            let opts = RenderOptions { sample_rate, ..RenderOptions::default() };
            let samples = render_song(render, synths, opts)?;
            write_audio_file(&out, &samples, sample_rate)?;
            println!("wrote {} ({:.1}s)", out.display(), samples.len() as f64 / 2.0 / sample_rate as f64);
        }
    }
    Ok(())
}
