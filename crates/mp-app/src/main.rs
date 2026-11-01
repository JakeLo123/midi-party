//! midi-party: a keyboard-driven MIDI piano-roll editor.
//!
//! The app is deliberately thin: keys go to `mp_core::Keymap`, commands run in
//! `mp_core::Editor`, and the editor's events drive audio and the status bar.
//! The `:` command line handles files and song-level settings here, and hands
//! everything else to the editor.

mod bridge;
mod cheat_sheet;
mod config;
mod dialogs;
mod input;
mod instrument_picker;
mod keymap_editor;
#[cfg(target_os = "macos")]
mod menu;
mod piano_roll;
mod recent;
mod theme;
mod tracks_area;
mod transport_bar;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use egui::{Rect, RichText, Stroke};
use mp_audio::render::{render_song, write_audio_file, RenderOptions};
use mp_audio::{render_song_from, SoundFontSynth, Synth};
use mp_core::{
    cmdline, default_keymap, feed_one, midi_file, pitch_name, project, try_drive_with, Command, Cursor, DriveError,
    Editor, EditorEvent, KeyChord, Keymap, LoopRegion, Song, Tick, TransportRequest, ViewRequest, DEFAULT_KEYMAP_TOML,
};
use rustysynth::SoundFont;

use crate::bridge::Audio;
use mp_core::instrument_name;
use crate::config::{ConfigEvent, ConfigWatcher};
use crate::dialogs::Unsaved;
use crate::input::{InputState, KeyRepeat};
use crate::instrument_picker::{Entry, InstrumentPicker, Outcome};
use crate::keymap_editor::KeymapEditor;
use crate::piano_roll::{RollAction, RollView};
use crate::theme::Theme;
use crate::tracks_area::{TracksAction, TracksArea};
use crate::transport_bar::{TransportAction, TransportBar, TransportView, ValueField, MAX_BPM, MIN_BPM};

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);
const EXPORT_SAMPLE_RATE: u32 = 44_100;
/// A pause this long between taps starts a new tap-tempo count.
const TAP_RESET: Duration = Duration::from_secs(2);
/// Tap tempo averages over this many of the latest taps.
const TAPS_KEPT: usize = 6;
/// The loop a song gets when the loop is switched on before one has been marked.
const DEFAULT_LOOP_BARS: u32 = 4;

#[derive(Parser)]
#[command(name = "midi-party", version)]
struct Args {
    /// A .ron project or .mid file to open.
    file: Option<PathBuf>,
    /// Default SoundFont, used by tracks that have none of their own.
    #[arg(long, default_value = "assets/soundfonts/florestan-subset.sf2")]
    sf2: PathBuf,
    /// Drum kit for channel-10 tracks, used when the default SoundFont has no kits.
    #[arg(long, default_value = "assets/soundfonts/fluidr3-standard-kit.sf2")]
    drums_sf2: PathBuf,
    /// Config directory holding keymap.toml and theme.toml
    /// (default: the OS config folder, e.g. ~/Library/Application Support/midi-party on macOS;
    /// both files are created on first run).
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// Close by itself after this many seconds (headless smoke test).
    #[arg(long)]
    smoke_seconds: Option<f64>,
    /// Feed this key string to the editor at startup (same syntax as `mp keys`).
    #[arg(long)]
    keys: Option<String>,
    /// With --smoke-seconds: save a screenshot of the window here, halfway through.
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// Start playing from the top immediately (smoke test of the playhead).
    #[arg(long)]
    play: bool,
    /// Run these command-line commands at startup (e.g. --command "export out.mp3").
    #[arg(long = "command")]
    commands: Vec<String>,
}

fn save_png(path: &PathBuf, image: &egui::ColorImage) {
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let file = std::fs::File::create(path)?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.width() as u32, image.height() as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(image.as_raw())?;
        Ok(())
    })();
    match result {
        Ok(()) => eprintln!("screenshot saved to {}", path.display()),
        Err(e) => eprintln!("screenshot failed: {e}"),
    }
}

fn default_config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("midi-party")
}

/// Inside a macOS `.app`, the bundled assets live next to the binary in `Contents/Resources`.
fn resource_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?.join("Resources");
    dir.is_dir().then_some(dir)
}

/// A relative asset path that does not exist from the current directory is
/// looked up in the app bundle's resources instead.
fn resolve_asset(path: PathBuf) -> PathBuf {
    if path.is_relative() && !path.exists() {
        if let Some(bundled) = resource_dir().map(|dir| dir.join(&path)).filter(|p| p.exists()) {
            return bundled;
        }
    }
    path
}

fn app_icon() -> egui::IconData {
    let decoder = png::Decoder::new(&include_bytes!("../../../assets/icon/icon-256.png")[..]);
    let mut reader = decoder.read_info().expect("bundled icon is a valid PNG");
    let mut rgba = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut rgba).expect("bundled icon decodes");
    assert_eq!(info.color_type, png::ColorType::Rgba, "bundled icon must be RGBA");
    rgba.truncate(info.buffer_size());
    egui::IconData { rgba, width: info.width, height: info.height }
}

/// First-run `keymap.toml`: an overlay, so updates to the built-in defaults keep reaching the user.
const KEYMAP_TEMPLATE: &str = r#"# midi-party key bindings.
#
# This file is layered on top of the built-in defaults: anything you put here
# overrides them and everything else stays as shipped (so new default bindings
# keep arriving with updates). Press Shift+/ in the app for the full live list,
# or read keymaps/default.toml in the source tree for the syntax.
#
# Set a key to "unbind" to remove a default binding. Uncomment the next line
# for home-row (h j k l) cursor movement:
#
# preset = "vim"
#
# [normal]
# "Shift+n" = { cmd = "insert_chord", intervals = [0, 3, 7] }   # minor instead of major
# "j"       = { cmd = "move_cursor", dir = "down", unit = "semitone" }
# "h"       = "unbind"
"#;

/// First-run `theme.toml`: an overlay on the built-in theme.
const THEME_TEMPLATE: &str = r##"# midi-party theme.
#
# Layered on top of the built-in theme (themes/default.toml in the source tree):
# set only what you want to change. Colors are "#rrggbb" or "#rrggbbaa".
# Names: background, black_key_row, white_key, black_key, key_label, grid,
# beat_line, bar_line, octave_line, note, note_selected, note_ghost, cursor,
# cursor_fill, playhead, lane, lane_current, roll_window, status_bg, status_fg,
# status_mode, status_dim, accent, error, panel_border, wordmark.
# themes/party.toml in the source tree is a complete alternative palette.
#
# font_size = 14.0
# track_colors = ["#e6a254", "#7cc5a8", "#d98ac4"]   # note colors by track number
#
# [colors]
# note_selected = "#f5c542"
# playhead      = "#e05a5a"
"##;

/// Creates the config files if they don't exist yet. A `theme.toml` that is
/// still the verbatim copy an earlier build wrote is replaced by the overlay
/// template so it stops pinning old colors.
fn ensure_config_files(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let theme_path = dir.join("theme.toml");
    if let Ok(text) = std::fs::read_to_string(&theme_path) {
        if text.starts_with("# midi-party default theme") {
            let _ = std::fs::write(&theme_path, THEME_TEMPLATE);
        }
    }
    for (name, contents) in [("keymap.toml", KEYMAP_TEMPLATE), ("theme.toml", THEME_TEMPLATE)] {
        let path = dir.join(name);
        if !path.exists() {
            if let Err(e) = std::fs::write(&path, contents) {
                eprintln!("config: cannot write {}: {e}", path.display());
            }
        }
    }
}

/// Shipped keymap presets selectable with `preset = "<name>"` in keymap.toml.
const KEYMAP_PRESETS: &[(&str, &str)] = &[("vim", include_str!("../../../keymaps/vim.toml"))];

/// Layers one overlay table onto `base`, mode by mode. An overlay key replaces
/// any binding it would conflict with as a sequence prefix, and the value
/// `"unbind"` removes a binding.
fn overlay_keymap(base: &mut toml::Table, overlay: toml::Table) -> Result<(), String> {
    let is_prefix = |a: &str, b: &str| {
        let (a, b): (Vec<&str>, Vec<&str>) = (a.split_whitespace().collect(), b.split_whitespace().collect());
        a.len() <= b.len() && a.iter().zip(&b).all(|(x, y)| x == y)
    };
    for (mode, bindings) in overlay {
        if mode == "preset" {
            continue;
        }
        let toml::Value::Table(user_bindings) = bindings else {
            return Err(format!("[{mode}] must be a table of bindings"));
        };
        let base_bindings = base.entry(mode).or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let Some(base_bindings) = base_bindings.as_table_mut() else { continue };
        for (key, value) in user_bindings {
            base_bindings.retain(|k, _| !(is_prefix(k, &key) || is_prefix(&key, k)));
            if value.as_str() != Some("unbind") {
                base_bindings.insert(key, value);
            }
        }
    }
    Ok(())
}

/// Built-in defaults, then the chosen preset (if any), then the user's own bindings.
fn merge_keymap_toml(user: &str) -> Result<String, String> {
    let mut base: toml::Table = toml::from_str(DEFAULT_KEYMAP_TOML).map_err(|e| e.to_string())?;
    let user: toml::Table = toml::from_str(user).map_err(|e| e.to_string())?;
    if let Some(preset) = user.get("preset").and_then(|v| v.as_str()) {
        let text = KEYMAP_PRESETS
            .iter()
            .find(|(name, _)| *name == preset)
            .map(|(_, text)| *text)
            .ok_or_else(|| format!("unknown preset {preset:?} (available: vim)"))?;
        let preset_table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        overlay_keymap(&mut base, preset_table)?;
    }
    overlay_keymap(&mut base, user)?;
    toml::to_string(&base).map_err(|e| e.to_string())
}

fn load_keymap(path: &Path) -> (Keymap, Option<String>) {
    let user = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return (default_keymap(), None),
    };
    let merged = match merge_keymap_toml(&user) {
        Ok(m) => m,
        Err(e) => return (default_keymap(), Some(format!("keymap.toml: {e} (using defaults)"))),
    };
    match Keymap::from_toml_str(&merged) {
        Ok(k) => {
            let status = (!k.warnings().is_empty()).then(|| format!("keymap.toml: {}", k.warnings().join("; ")));
            (k, status)
        }
        Err(e) => (default_keymap(), Some(format!("keymap.toml: {e} (using defaults)"))),
    }
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default()
}

fn load_song_file(path: &Path) -> Result<Song, String> {
    match extension(path).as_str() {
        "mid" | "midi" => midi_file::load(path).map_err(|e| e.to_string()),
        _ => project::load(path).map_err(|e| e.to_string()),
    }
}

/// Renders the song to a WAV/MP3 file; runs on a background thread.
fn export_audio(song: Song, fonts: Vec<Option<Arc<SoundFont>>>, out: PathBuf) -> Result<String, String> {
    let synths: Vec<Option<Box<dyn Synth>>> = fonts
        .iter()
        .map(|f| {
            f.as_ref().and_then(|font| {
                SoundFontSynth::new(font, EXPORT_SAMPLE_RATE).ok().map(|s| Box::new(s) as Box<dyn Synth>)
            })
        })
        .collect();
    let opts = RenderOptions { sample_rate: EXPORT_SAMPLE_RATE, ..RenderOptions::default() };
    let samples = render_song(render_song_from(&song), synths, opts).map_err(|e| e.to_string())?;
    write_audio_file(&out, &samples, EXPORT_SAMPLE_RATE).map_err(|e| e.to_string())?;
    let seconds = samples.len() as f64 / 2.0 / EXPORT_SAMPLE_RATE as f64;
    let name = out.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    Ok(format!("exported {name} ({seconds:.1}s)"))
}

struct App {
    editor: Editor,
    keymap: Keymap,
    audio: Result<Audio, String>,
    view: RollView,
    roll_rect: Rect,
    tracks: TracksArea,
    /// Whether the piano roll is open under the tracks area.
    show_roll: bool,
    config: ConfigWatcher,
    config_dir: PathBuf,
    theme: Theme,
    input: InputState,
    status: String,
    show_cheat_sheet: bool,
    keymap_editor: KeymapEditor,
    picker: InstrumentPicker,
    /// Scroll the cursor into view once the roll has been laid out.
    needs_scroll: bool,
    /// The `:` command line while it is open.
    cmdline: Option<String>,
    /// A track name being edited in its header: the track index and the text so far.
    track_rename: Option<(usize, String)>,
    file_path: Option<PathBuf>,
    dirty: bool,
    /// Set once unsaved changes have been saved or knowingly discarded, so the window may close.
    allow_close: bool,
    recent: Vec<PathBuf>,
    #[cfg(target_os = "macos")]
    menu: menu::MacMenu,
    title: String,
    last_autosave: Instant,
    export_rx: Option<Receiver<Result<String, String>>>,
    started: Instant,
    smoke_seconds: Option<f64>,
    screenshot: Option<PathBuf>,
    /// Where playback starts or resumes. Follows the audio engine while playing and
    /// rests where it stopped; a ruler click or a transport key moves it. Independent of the cursor.
    playhead: Tick,
    /// Set while a ruler drag holds the playhead; true if playback should resume on release.
    scrub: Option<bool>,
    transport: TransportBar,
    metronome: bool,
    /// Recent tap-tempo taps, oldest first.
    taps: Vec<Instant>,
    /// Where the tempo change being dragged in the transport bar sits.
    tempo_drag_at: Option<Tick>,
    /// A tempo or time signature being typed in the transport bar, and the text so far.
    value_entry: Option<(ValueField, String)>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        cc.egui_ctx.set_fonts(Theme::font_definitions());
        // egui otherwise takes Cmd+0 / Cmd+- / Cmd+= as UI-scale shortcuts before the keymap sees them.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let config_dir = args.config_dir.unwrap_or_else(default_config_dir);
        ensure_config_files(&config_dir);
        let (theme, theme_error) = Theme::load(&config_dir.join("theme.toml"));
        cc.egui_ctx.set_visuals(theme.visuals());
        let (mut keymap, keymap_error) = load_keymap(&config_dir.join("keymap.toml"));
        let config = ConfigWatcher::new(&config_dir, cc.egui_ctx.clone());

        let mut file_error = None;
        let mut file_path = None;
        let song = match &args.file {
            Some(path) => match load_song_file(path) {
                Ok(song) => {
                    file_path = Some(path.clone());
                    song
                }
                Err(e) => {
                    file_error = Some(format!("{}: {e}", path.display()));
                    Song::new_empty()
                }
            },
            None => Song::new_empty(),
        };
        let mut editor = Editor::new(song);
        editor.cursor.pitch = 60;
        let mut keys_error = None;
        let mut startup_events = Vec::new();
        if let Some(keys) = &args.keys {
            match try_drive_with(&mut editor, &mut keymap, keys) {
                Ok(events) => startup_events = events,
                Err(e) => keys_error = Some(format!("--keys: {e}")),
            }
        }
        let mut audio = Audio::open(&resolve_asset(args.sf2), &resolve_asset(args.drums_sf2)).map_err(|e| e.to_string());
        let mut status = match &audio {
            Ok(a) => format!("audio ready at {} Hz", a.sample_rate),
            Err(e) => format!("audio unavailable: {e}"),
        };
        if let Ok(a) = &mut audio {
            let warnings = a.sync_song(&editor.song);
            if !warnings.is_empty() {
                status = warnings.join("; ");
            }
        }
        if args.file.is_none() && config_dir.join("autosave.ron").exists() {
            status = format!("autosave available: :o {}", config_dir.join("autosave.ron").display());
        }
        if let Some(e) = theme_error.or(keymap_error).or(keys_error).or(file_error) {
            status = e;
        }
        if args.play {
            if let Ok(a) = &mut audio {
                let _ = a.play_from(0.0);
            }
        }

        let recent = recent::load(&config_dir);
        #[cfg(target_os = "macos")]
        let menu = menu::MacMenu::install(cc.egui_ctx.clone(), &keymap, &recent, &app_icon());

        let mut app = Self {
            editor,
            keymap,
            audio,
            view: RollView::default(),
            roll_rect: Rect::NOTHING,
            tracks: TracksArea::default(),
            show_roll: true,
            config,
            config_dir,
            theme,
            input: InputState::new(KeyRepeat::default()),
            status,
            show_cheat_sheet: false,
            keymap_editor: KeymapEditor::default(),
            picker: InstrumentPicker::default(),
            needs_scroll: true,
            cmdline: None,
            track_rename: None,
            file_path,
            dirty: false,
            allow_close: false,
            recent,
            #[cfg(target_os = "macos")]
            menu,
            title: String::new(),
            last_autosave: Instant::now(),
            export_rx: None,
            started: Instant::now(),
            smoke_seconds: args.smoke_seconds,
            screenshot: args.screenshot,
            playhead: Tick::ZERO,
            scrub: None,
            transport: TransportBar::default(),
            metronome: false,
            taps: Vec::new(),
            tempo_drag_at: None,
            value_entry: None,
        };
        app.apply_events(startup_events);
        for command in &args.commands {
            app.run_command(command.trim_start_matches(':'));
        }
        app
    }

    fn tick_seconds(&self, tick: Tick) -> f64 {
        let song = &self.editor.song;
        song.tempo_map.tick_to_seconds(song.ppq, tick)
    }

    fn is_playing(&self) -> bool {
        self.audio.as_ref().is_ok_and(Audio::is_playing)
    }

    /// Pull the playhead along with the audio engine while it plays, so it rests
    /// where playback stopped.
    fn sync_playhead(&mut self) {
        if self.scrub.is_some() {
            return;
        }
        if let Ok(audio) = &self.audio {
            if audio.is_playing() {
                let song = &self.editor.song;
                self.playhead = song.tempo_map.seconds_to_tick(song.ppq, audio.playhead_seconds());
            }
        }
    }

    fn seek_playhead(&mut self, tick: Tick) {
        if tick == self.playhead {
            return;
        }
        self.playhead = tick;
        let seconds = self.tick_seconds(tick);
        if let Ok(audio) = &mut self.audio {
            if let Err(e) = audio.seek(seconds) {
                self.status = format!("audio error: {e}");
            }
        }
    }

    fn scrub_start(&mut self) {
        let was_playing = self.is_playing();
        self.scrub = Some(was_playing);
        if was_playing {
            if let Ok(audio) = &mut self.audio {
                if let Err(e) = audio.stop() {
                    self.status = format!("audio error: {e}");
                }
            }
        }
    }

    fn scrub_end(&mut self) {
        if self.scrub.take() == Some(true) {
            let seconds = self.tick_seconds(self.playhead);
            self.with_audio(|a| a.play_from(seconds).map(|_| "playing".into()));
        }
    }

    fn apply_transport(&mut self, request: TransportRequest) {
        match request {
            TransportRequest::Play { from } => {
                self.playhead = from;
                let seconds = self.tick_seconds(from);
                self.with_audio(|a| a.play_from(seconds).map(|_| "playing".into()));
            }
            TransportRequest::Stop => self.with_audio(|a| a.stop().map(|_| "stopped".into())),
            TransportRequest::Toggle => {
                if self.is_playing() {
                    self.with_audio(|a| a.stop().map(|_| "stopped".into()));
                } else {
                    let seconds = self.tick_seconds(self.playhead);
                    self.with_audio(|a| a.play_from(seconds).map(|_| "playing".into()));
                }
            }
            TransportRequest::Seek { to } => self.seek_playhead(to),
            TransportRequest::CursorToPlayhead => self.editor.cursor_to_grid_line_at(self.playhead),
            TransportRequest::StepBars { bars } => {
                let song = &self.editor.song;
                let to = song.meter_map.step_bars(song.ppq, self.playhead, bars);
                self.seek_playhead(to);
            }
            TransportRequest::ToggleLoop => self.toggle_loop(),
            TransportRequest::ToggleMetronome => self.toggle_metronome(),
            TransportRequest::TapTempo => self.tap_tempo(Instant::now()),
        }
    }

    fn apply_transport_action(&mut self, action: TransportAction) {
        self.finish_rename();
        if !matches!(action, TransportAction::Edit(_)) {
            self.finish_entry();
        }
        match action {
            TransportAction::ToStart => self.apply_transport(TransportRequest::Seek { to: Tick::ZERO }),
            TransportAction::TogglePlay => self.apply_transport(TransportRequest::Toggle),
            TransportAction::ToggleLoop => self.toggle_loop(),
            TransportAction::ToggleMetronome => self.toggle_metronome(),
            TransportAction::TapTempo => self.tap_tempo(Instant::now()),
            TransportAction::DragTempo { bpm } => {
                let song = &self.editor.song;
                let at = *self.tempo_drag_at.get_or_insert_with(|| song.tempo_map.change_at_or_before(self.playhead));
                self.set_tempo_at(at, bpm, Some("tempo_drag"));
            }
            TransportAction::EndTempoDrag => {
                self.tempo_drag_at = None;
                self.editor.undo.break_coalescing();
            }
            TransportAction::SetMeter { numerator, denominator } => self.set_meter_at_playhead(numerator, denominator),
            TransportAction::Edit(field) => self.start_entry(field),
        }
    }

    /// Sets the tempo of the change at `at`, merging into one undo step with the
    /// edits before it that share `coalesce`.
    fn set_tempo_at(&mut self, at: Tick, bpm: f64, coalesce: Option<&str>) {
        let label = transport_bar::bpm_label(bpm);
        self.edit_song_merging(format!("tempo {label} bpm"), coalesce, |song| song.tempo_map.set_tempo_at(at, bpm).map_err(|e| e.to_string()));
        self.keep_playback_in_place();
    }

    fn set_meter_at_playhead(&mut self, numerator: u8, denominator: u8) {
        let at = self.editor.song.meter_map.change_at_or_before(self.playhead);
        self.edit_song(format!("time signature {numerator}/{denominator}"), |song| {
            song.meter_map.set_meter_at(at, numerator, denominator).map_err(|e| e.to_string())
        });
    }

    /// After a tempo change, playback carries on from the same spot in the music
    /// rather than the same number of seconds in.
    fn keep_playback_in_place(&mut self) {
        if self.is_playing() {
            let seconds = self.tick_seconds(self.playhead);
            if let Ok(audio) = &mut self.audio {
                let _ = audio.seek(seconds);
            }
        }
    }

    fn toggle_metronome(&mut self) {
        self.metronome = !self.metronome;
        let on = self.metronome;
        self.with_audio(|a| a.set_metronome(on).map(|_| format!("metronome {}", if on { "on" } else { "off" })));
    }

    /// Counts one tap. From the second tap on, the tempo at the playhead follows the
    /// average gap between the recent taps; one run of taps is one undo step.
    fn tap_tempo(&mut self, now: Instant) {
        if self.taps.last().is_some_and(|last| now.duration_since(*last) > TAP_RESET) {
            self.taps.clear();
            self.editor.undo.break_coalescing();
        }
        self.taps.push(now);
        if self.taps.len() > TAPS_KEPT {
            self.taps.remove(0);
        }
        let (Some(first), Some(last)) = (self.taps.first(), self.taps.last()) else { return };
        let span = last.duration_since(*first).as_secs_f64();
        if self.taps.len() < 2 || span <= 0.0 {
            self.status = "tap: keep tapping in time".into();
            return;
        }
        let bpm = (60.0 * (self.taps.len() - 1) as f64 / span).round().clamp(MIN_BPM, MAX_BPM);
        let at = self.editor.song.tempo_map.change_at_or_before(self.playhead);
        self.set_tempo_at(at, bpm, Some("tap_tempo"));
    }

    /// Switches the loop on or off. A song without a loop region gets one first: around
    /// the selection, or the bars from the playhead on.
    fn toggle_loop(&mut self) {
        let region = match self.editor.song.loop_region {
            Some(r) => LoopRegion { enabled: !r.enabled, ..r },
            None => {
                let (start, end) = self.default_loop_span();
                LoopRegion { start, end, enabled: true }
            }
        };
        self.set_loop(Some(region));
    }

    fn default_loop_span(&self) -> (Tick, Tick) {
        let song = &self.editor.song;
        let meter = &song.meter_map;
        let selected = self.editor.track().map(|t| t.notes.iter().filter(|n| self.editor.selection.contains(&n.id)).collect::<Vec<_>>()).unwrap_or_default();
        let first = selected.iter().map(|n| n.start).min();
        let last = selected.iter().map(|n| n.end()).max();
        if let (Some(first), Some(last)) = (first, last) {
            let start = meter.tick_of_bar(song.ppq, meter.bar_beat_of(song.ppq, first).bar);
            let end_bar = meter.bar_beat_of(song.ppq, last);
            let end = if meter.tick_of_bar(song.ppq, end_bar.bar) == last { last } else { meter.tick_of_bar(song.ppq, end_bar.bar + 1) };
            return (start, end);
        }
        let bar = meter.bar_beat_of(song.ppq, self.playhead).bar;
        (meter.tick_of_bar(song.ppq, bar), meter.tick_of_bar(song.ppq, bar + DEFAULT_LOOP_BARS))
    }

    /// The loop region is part of the song file but not of undo, like the header width.
    fn set_loop(&mut self, region: Option<LoopRegion>) {
        if self.editor.song.loop_region == region {
            return;
        }
        self.editor.song.loop_region = region;
        self.song_changed();
        let song = &self.editor.song;
        self.status = match region {
            Some(r) => {
                let bar = |tick| song.meter_map.bar_beat_of(song.ppq, tick).to_string();
                format!("loop {}: {} to {}", if r.enabled { "on" } else { "off" }, bar(r.start), bar(r.end))
            }
            None => "loop removed".into(),
        };
    }

    fn with_audio(&mut self, f: impl FnOnce(&mut Audio) -> Result<String, mp_audio::AudioError>) {
        match &mut self.audio {
            Ok(audio) => self.status = f(audio).unwrap_or_else(|e| format!("audio error: {e}")),
            Err(e) => self.status = format!("no audio: {e}"),
        }
    }

    /// Keys the app handles before the editor's keymap sees them. Everything
    /// musical is in the keymap; these are OS-level conventions.
    fn handle_app_chord(&mut self, text: &str, ctx: &egui::Context) -> bool {
        match text {
            "Escape" if self.show_cheat_sheet => self.show_cheat_sheet = false,
            "Cmd+s" => {
                self.save();
            }
            "Shift+Cmd+s" => {
                self.save_as();
            }
            "Cmd+o" => self.open_with_dialog(),
            "Cmd+n" => self.new_song_with_prompt(),
            "Cmd+," => dialogs::settings_placeholder(),
            "Cmd+k" => self.open_keymap_editor(),
            "Cmd+q" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            _ => return false,
        }
        true
    }

    fn handle_menu_clicks(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "macos")]
        for click in self.menu.poll() {
            match click {
                menu::MenuClick::Key(text) => self.handle_key(&text, ctx),
                menu::MenuClick::Run(id) => self.run_menu_item(id, ctx),
                menu::MenuClick::OpenRecent(path) => {
                    if self.confirm_discard() {
                        self.open(&path);
                    }
                }
                menu::MenuClick::ClearRecent => recent::clear(&self.config_dir, &mut self.recent),
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = ctx;
    }

    /// A menu item that has no shortcut right now: closes whatever overlay is up and runs it.
    #[cfg(target_os = "macos")]
    fn run_menu_item(&mut self, id: &str, ctx: &egui::Context) {
        self.close_overlays();
        self.cmdline = None;
        match id {
            "settings" => dialogs::settings_placeholder(),
            "quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "new" => self.new_song_with_prompt(),
            "open" => self.open_with_dialog(),
            "save" => {
                self.save();
            }
            "save_as" => {
                self.save_as();
            }
            "export" => self.export_with_dialog(),
            "cheat_sheet" => self.show_cheat_sheet(),
            "keymap_editor" => self.open_keymap_editor(),
            command => self.run_command(command),
        }
    }

    /// Saves to the current file, asking for a name if there is none. True if the song was saved.
    fn save(&mut self) -> bool {
        match self.file_path.clone() {
            Some(path) => {
                self.save_to(&path);
                !self.dirty
            }
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) -> bool {
        match dialogs::save_song_as(self.file_path.as_deref()) {
            Some(path) => {
                self.save_to(&path);
                !self.dirty
            }
            None => {
                if !cfg!(target_os = "macos") {
                    self.status = "no file name: use :w <path.ron>".into();
                }
                false
            }
        }
    }

    /// Asks what to do with unsaved changes. True if it is fine to replace or close the song.
    fn confirm_discard(&mut self) -> bool {
        if !self.dirty {
            return true;
        }
        match dialogs::ask_unsaved(self.file_name()) {
            Unsaved::Save => self.save(),
            Unsaved::Discard => true,
            Unsaved::Cancel => false,
        }
    }

    fn open_with_dialog(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        if let Some(path) = dialogs::pick_song(self.file_path.as_deref()) {
            self.open(&path);
        }
    }

    fn new_song_with_prompt(&mut self) {
        if self.confirm_discard() {
            self.run_command("new!");
        }
    }

    fn export_with_dialog(&mut self) {
        if let Some(path) = dialogs::export_to(self.file_path.as_deref()) {
            self.export(path);
        }
    }

    fn file_name(&self) -> &str {
        self.file_path.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("untitled")
    }

    /// Only one overlay is up at a time, so keys always go to the one on screen.
    fn close_overlays(&mut self) {
        self.show_cheat_sheet = false;
        self.keymap_editor.open = false;
        self.picker.open = false;
        self.track_rename = None;
        self.value_entry = None;
    }

    fn open_keymap_editor(&mut self) {
        self.close_overlays();
        self.keymap_editor.open(&self.keymap);
    }

    fn show_cheat_sheet(&mut self) {
        self.close_overlays();
        self.show_cheat_sheet = true;
    }

    /// Handles one keypress (or a menu click standing in for one), written in keymap syntax.
    fn handle_key(&mut self, text: &str, ctx: &egui::Context) {
        let typing = self.picker.open || self.cmdline.is_some() || self.track_rename.is_some() || self.value_entry.is_some();
        if typing && APP_CHORDS.contains(&text) {
            self.finish_rename();
            self.finish_entry();
            self.close_overlays();
            self.cmdline = None;
            self.handle_app_chord(text, ctx);
            return;
        }
        if self.picker.open {
            match self.picker.handle_chord(text) {
                Outcome::Nothing => {}
                Outcome::Audition(entry) => self.preview_entry(&entry),
                Outcome::Pick(entry) => {
                    if self.picker.track < self.editor.song.tracks.len() {
                        self.editor.show_track(self.picker.track);
                    }
                    self.execute(&entry.command());
                }
            }
            return;
        }
        if self.keymap_editor.open {
            let path = self.config_dir.join("keymap.toml");
            if let Some(status) = self.keymap_editor.handle_chord(text, &path) {
                self.status = status;
            }
            return;
        }
        if self.cmdline.is_some() {
            self.handle_cmdline_chord(text, ctx);
            return;
        }
        if self.track_rename.is_some() {
            self.handle_rename_chord(text);
            return;
        }
        if self.value_entry.is_some() {
            self.handle_entry_chord(text);
            return;
        }
        if self.handle_app_chord(text, ctx) {
            return;
        }
        let chord: KeyChord = match text.parse() {
            Ok(c) => c,
            Err(_) => return, // a key the keymap syntax has no name for (F-keys etc.)
        };
        let mut events = Vec::new();
        match feed_one(&mut self.editor, &mut self.keymap, chord, &mut events) {
            Ok(()) => {}
            Err(DriveError::Unbound { key }) => self.status = format!("{key} is not bound"),
            Err(DriveError::Edit { key, source }) => self.status = format!("{key}: {source}"),
            Err(other) => self.status = other.to_string(),
        }
        self.apply_events(events);
        self.scroll_views_to_cursor();
    }

    /// Brings the cursor into view in both the piano roll and the tracks area.
    fn scroll_views_to_cursor(&mut self) {
        self.view.ensure_cursor_visible(&self.editor, self.roll_rect);
        self.tracks.keep_tick_visible(self.editor.cursor.tick.get() as f64);
    }

    fn apply_events(&mut self, events: Vec<EditorEvent>) {
        for event in events {
            match event {
                EditorEvent::SongChanged => self.song_changed(),
                EditorEvent::Audition(notes) => {
                    let track = self.editor.track_index;
                    self.with_audio(|a| {
                        for n in notes.iter().take(8) {
                            a.audition(track, n.pitch, n.velocity)?;
                        }
                        Ok(String::new())
                    });
                }
                EditorEvent::OpenCommandLine => self.cmdline = Some(String::new()),
                EditorEvent::Status(s) => self.status = s,
                EditorEvent::Transport(request) => self.apply_transport(request),
                EditorEvent::ToggleCheatSheet => {
                    if self.show_cheat_sheet {
                        self.show_cheat_sheet = false;
                    } else {
                        self.show_cheat_sheet();
                    }
                }
                EditorEvent::OpenInstrumentPicker => self.open_picker(self.editor.track_index),
                EditorEvent::EditTrackName => self.start_rename(self.editor.track_index),
                EditorEvent::View(ViewRequest::ZoomTime(factor)) => self.view.zoom_time(factor),
                EditorEvent::View(ViewRequest::ZoomRows(factor)) => self.view.zoom_rows(factor),
                EditorEvent::View(ViewRequest::TogglePianoRoll) => {
                    self.show_roll = !self.show_roll;
                    self.needs_scroll = true;
                }
                EditorEvent::View(ViewRequest::FitTracksToSong) => self.tracks.fit_to_song(),
            }
        }
    }

    fn open_picker(&mut self, track: usize) {
        let Some(t) = self.editor.song.tracks.get(track).cloned() else { return };
        let lists = self.audio.as_mut().ok().map(|a| a.presets_for(&t));
        self.close_overlays();
        self.picker.open(track, &t, lists);
    }

    fn preview_entry(&mut self, entry: &Entry) {
        if entry.font.as_os_str().is_empty() {
            return;
        }
        // On channel 10 the synth selects the kit bank itself, so the song stores bank 0.
        let bank = if entry.drums { 0 } else { entry.bank };
        if let Ok(audio) = &mut self.audio {
            if let Err(e) = audio.preview(&entry.font, entry.preview_channel(), bank, entry.program, entry.preview_key(), 100) {
                self.status = format!("audio error: {e}");
            }
        }
    }

    /// Runs an editor command (from the mouse or the command line) and applies its events.
    fn execute(&mut self, cmd: &Command) {
        match self.editor.execute(cmd, None) {
            Ok(events) => self.apply_events(events),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn apply_roll_action(&mut self, action: RollAction) {
        self.finish_rename();
        self.finish_entry();
        if self.cmdline.is_some() || self.show_cheat_sheet || self.picker.open {
            return;
        }
        match action {
            RollAction::SetCursor { tick, pitch } => {
                self.editor.cursor = Cursor { tick: Tick(tick), pitch };
                self.editor.selection.clear();
                self.editor.box_anchor = None;
                self.editor.focus = None;
            }
            RollAction::SetPlayhead { tick } => self.seek_playhead(Tick(tick)),
            RollAction::ScrubStart => self.scrub_start(),
            RollAction::ScrubEnd => self.scrub_end(),
            RollAction::ClickNote { id, toggle } => {
                if let Some(note) = self.editor.track().and_then(|t| t.note(id)).copied() {
                    self.editor.cursor = Cursor { tick: note.start, pitch: note.pitch };
                    self.editor.box_anchor = None;
                    if toggle {
                        if !self.editor.selection.remove(&id) {
                            self.editor.selection.insert(id);
                        }
                    } else {
                        self.editor.selection.clear();
                        self.editor.selection.insert(id);
                    }
                    self.editor.focus = None;
                    let track = self.editor.track_index;
                    self.with_audio(|a| a.audition(track, note.pitch, note.velocity).map(|_| String::new()));
                }
            }
            RollAction::InsertAt { tick, pitch } => {
                self.editor.cursor = Cursor { tick: Tick(tick), pitch };
                self.execute(&Command::InsertNote);
            }
            RollAction::MoveSelection { grid_steps, semitones } => {
                if grid_steps != 0 {
                    let ticks = grid_steps * self.editor.grid_ticks().get() as i64;
                    self.execute(&Command::NudgeSelected { ticks });
                }
                if semitones != 0 {
                    self.execute(&Command::TransposeSelected { semitones });
                }
            }
            RollAction::ResizeSelection { grid_steps } => {
                self.execute(&Command::ChangeLength { delta_steps: grid_steps });
            }
            RollAction::BoxSelect { t0, t1, p_lo, p_hi, add } => {
                self.editor.box_anchor = None;
                if !add {
                    self.editor.selection.clear();
                }
                if let Some(track) = self.editor.track() {
                    let ids: Vec<_> = track
                        .notes
                        .iter()
                        .filter(|n| (t0..t1).contains(&n.start.get()) && (p_lo..=p_hi).contains(&n.pitch))
                        .map(|n| n.id)
                        .collect();
                    self.editor.selection.extend(ids);
                }
                self.editor.focus = None;
                self.status = format!("{} selected", self.editor.selection.len());
            }
        }
        self.scroll_views_to_cursor();
    }

    fn apply_tracks_action(&mut self, action: TracksAction) {
        if let TracksAction::ResizeHeader { width } = action {
            self.editor.song.track_header_width = width;
            self.dirty = true;
            return;
        }
        self.finish_rename();
        self.finish_entry();
        if self.cmdline.is_some() || self.show_cheat_sheet || self.picker.open {
            return;
        }
        if self.apply_time_action(&action) {
            self.scroll_views_to_cursor();
            return;
        }
        let mix = match action {
            TracksAction::ToggleMute { track } => Some(self.editor.toggle_mute_of(track)),
            TracksAction::ToggleSolo { track } => Some(self.editor.toggle_solo_of(track)),
            TracksAction::SetVolume { track, percent } => Some(self.editor.set_volume_of(track, percent)),
            TracksAction::SetPan { track, value } => Some(self.editor.set_pan_of(track, value)),
            TracksAction::EndMixGesture => {
                self.editor.end_mix_gesture();
                return;
            }
            _ => None,
        };
        if let Some(result) = mix {
            match result {
                Ok(events) => self.apply_events(events),
                Err(e) => self.status = e.to_string(),
            }
            return;
        }
        let tracks = self.editor.song.tracks.len();
        match action {
            TracksAction::ShowTrack { track } if track < tracks => {
                self.editor.show_track(track);
                self.editor.focus = None;
            }
            TracksAction::JumpTo { track, tick } if track < tracks => {
                self.editor.show_track(track);
                self.editor.focus = None;
                self.editor.cursor.tick = Tick(tick);
            }
            TracksAction::SetPlayhead { tick } => self.seek_playhead(Tick(tick)),
            TracksAction::ScrubStart => self.scrub_start(),
            TracksAction::ScrubEnd => self.scrub_end(),
            TracksAction::PickInstrument { track } if track < tracks => {
                self.editor.show_track(track);
                self.editor.focus = None;
                self.open_picker(track);
            }
            TracksAction::RenameTrack { track } if track < tracks => {
                self.editor.show_track(track);
                self.editor.focus = None;
                self.start_rename(track);
            }
            TracksAction::ShowTrack { .. }
            | TracksAction::JumpTo { .. }
            | TracksAction::PickInstrument { .. }
            | TracksAction::RenameTrack { .. }
            | TracksAction::ResizeHeader { .. }
            | TracksAction::ToggleMute { .. }
            | TracksAction::ToggleSolo { .. }
            | TracksAction::SetVolume { .. }
            | TracksAction::SetPan { .. }
            | TracksAction::EndMixGesture
            | TracksAction::SetLoop { .. }
            | TracksAction::ToggleLoopEnabled
            | TracksAction::RemoveLoop
            | TracksAction::MoveTimeChange { .. }
            | TracksAction::EndTimeChangeDrag
            | TracksAction::AddTempoChange { .. }
            | TracksAction::AddMeterChange { .. }
            | TracksAction::RemoveTempoChange { .. }
            | TracksAction::RemoveMeterChange { .. } => {}
        }
        self.scroll_views_to_cursor();
    }

    /// The ruler's loop and tempo/time-signature actions. False for any other action.
    fn apply_time_action(&mut self, action: &TracksAction) -> bool {
        let song = &self.editor.song;
        match *action {
            TracksAction::SetLoop { start, end, enabled } => self.set_loop(Some(LoopRegion { start: Tick(start), end: Tick(end), enabled })),
            TracksAction::ToggleLoopEnabled => self.toggle_loop(),
            TracksAction::RemoveLoop => self.set_loop(None),
            TracksAction::MoveTimeChange { from, to } => {
                let (from, to) = (Tick(from), Tick(to));
                let at = song.meter_map.bar_beat_of(song.ppq, to);
                self.edit_song_merging(format!("change moved to {at}"), Some("move_time_change"), |song| {
                    let tempo = song.tempo_map.move_change(from, to);
                    let meter = song.meter_map.move_change(from, to);
                    if tempo || meter { Ok(()) } else { Err("nothing to move there".into()) }
                });
                self.keep_playback_in_place();
                self.seek_playhead(to);
            }
            TracksAction::EndTimeChangeDrag => self.editor.undo.break_coalescing(),
            TracksAction::AddTempoChange { tick } => {
                let (at, bpm) = (Tick(tick), song.tempo_map.bpm_at(Tick(tick)));
                let bar = song.meter_map.bar_beat_of(song.ppq, at);
                self.edit_song(format!("tempo change at {bar}: drag the tempo in the transport bar to set it"), |song| {
                    song.tempo_map.set_tempo_at(at, bpm).map_err(|e| e.to_string())
                });
                self.seek_playhead(at);
            }
            TracksAction::AddMeterChange { tick } => {
                let at = Tick(tick);
                let (n, d) = song.meter_map.meter_at(at);
                let bar = song.meter_map.bar_beat_of(song.ppq, at);
                self.edit_song(format!("time signature change at {bar}: pick it in the transport bar"), |song| {
                    song.meter_map.set_meter_at(at, n, d).map_err(|e| e.to_string())
                });
                self.seek_playhead(at);
            }
            TracksAction::RemoveTempoChange { tick } => {
                self.edit_song("tempo change deleted".into(), |song| {
                    if song.tempo_map.remove_at(Tick(tick)) { Ok(()) } else { Err("no tempo change there".into()) }
                });
                self.keep_playback_in_place();
            }
            TracksAction::RemoveMeterChange { tick } => {
                self.edit_song("time signature change deleted".into(), |song| {
                    if song.meter_map.remove_at(Tick(tick)) { Ok(()) } else { Err("no time signature change there".into()) }
                });
            }
            _ => return false,
        }
        true
    }

    fn song_changed(&mut self) {
        self.dirty = true;
        if let Ok(audio) = &mut self.audio {
            let warnings = audio.sync_song(&self.editor.song);
            if !warnings.is_empty() {
                self.status = warnings.join("; ");
            }
        }
    }

    // ----- command line -----

    fn handle_cmdline_chord(&mut self, text: &str, ctx: &egui::Context) {
        match text {
            "Escape" => self.cmdline = None,
            "Enter" => {
                if let Some(line) = self.cmdline.take() {
                    self.run_command(line.trim());
                    if self.status == "quit" {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            "Backspace" => {
                if let Some(line) = &mut self.cmdline {
                    line.pop();
                }
            }
            "Tab" => {
                if let Some(line) = &mut self.cmdline {
                    let mut matches: Vec<String> =
                        APP_COMMANDS.iter().map(|(n, _)| n.to_string()).filter(|n| n.starts_with(line.as_str())).collect();
                    matches.extend(cmdline::complete(line));
                    matches.sort();
                    matches.dedup();
                    if matches.len() == 1 {
                        *line = format!("{} ", matches[0]);
                    } else if !matches.is_empty() {
                        self.status = matches.join("  ");
                    }
                }
            }
            _ => {}
        }
    }

    // ----- track rename -----

    fn start_rename(&mut self, track: usize) {
        let Some(name) = self.editor.song.tracks.get(track).map(|t| t.name.clone()) else { return };
        self.close_overlays();
        self.track_rename = Some((track, name));
    }

    /// Renames the track to the edited text, unless it is blank or unchanged.
    fn finish_rename(&mut self) {
        let Some((track, text)) = self.track_rename.take() else { return };
        let name = text.trim();
        let Some(current) = self.editor.song.tracks.get(track) else { return };
        if name.is_empty() || name == current.name {
            return;
        }
        self.editor.show_track(track);
        self.execute(&Command::RenameTrack { name: name.to_string() });
    }

    fn handle_rename_chord(&mut self, text: &str) {
        match text {
            "Escape" => self.track_rename = None,
            "Enter" => self.finish_rename(),
            "Backspace" => {
                if let Some((_, name)) = &mut self.track_rename {
                    name.pop();
                }
            }
            _ => {}
        }
    }

    // ----- typing a tempo or time signature -----

    fn start_entry(&mut self, field: ValueField) {
        self.close_overlays();
        self.cmdline = None;
        self.value_entry = Some((field, String::new()));
        self.status = match field {
            ValueField::Tempo => "type a tempo (20-400), Enter sets it, Escape cancels".into(),
            ValueField::Meter => "type a time signature like 7/4, Enter sets it, Escape cancels".into(),
        };
    }

    /// Applies the typed value. Empty or unreadable text changes nothing.
    fn finish_entry(&mut self) {
        let Some((field, text)) = self.value_entry.take() else { return };
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        match field {
            ValueField::Tempo => match text.parse::<f64>() {
                Ok(bpm) if (MIN_BPM..=MAX_BPM).contains(&bpm) => {
                    let at = self.editor.song.tempo_map.change_at_or_before(self.playhead);
                    self.set_tempo_at(at, bpm, None);
                }
                _ => self.status = format!("{text} is not a tempo from 20 to 400"),
            },
            ValueField::Meter => match parse_meter(text) {
                Some((n, d)) => self.set_meter_at_playhead(n, d),
                None => self.status = format!("{text} is not a time signature (like 3/4 or 6/8)"),
            },
        }
    }

    fn handle_entry_chord(&mut self, text: &str) {
        match text {
            "Escape" => {
                self.value_entry = None;
                self.status.clear();
            }
            "Enter" => self.finish_entry(),
            "Backspace" => {
                if let Some((_, value)) = &mut self.value_entry {
                    value.pop();
                }
            }
            _ => {}
        }
    }

    fn entry_text(&mut self, typed: &str) {
        let Some((_, value)) = &mut self.value_entry else { return };
        value.extend(typed.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | '/')));
    }

    fn rename_text(&mut self, typed: &str) {
        let Some((_, name)) = &mut self.track_rename else { return };
        name.extend(typed.chars().filter(|c| !c.is_control()));
    }

    fn cmdline_text(&mut self, typed: &str) {
        let Some(line) = &mut self.cmdline else { return };
        for c in typed.chars() {
            if line.is_empty() && c == ':' {
                continue; // the keystroke that opened the command line
            }
            if !c.is_control() {
                line.push(c);
            }
        }
    }

    /// Takes an undo snapshot, applies a song-level change the editor has no
    /// command for (tempo, meter, instrument), and republishes the song.
    fn edit_song(&mut self, status: String, f: impl FnOnce(&mut Song) -> Result<(), String>) {
        self.edit_song_merging(status, None, f);
    }

    /// [`Self::edit_song`], merged into one undo step with the edits before it that
    /// share `coalesce`.
    fn edit_song_merging(&mut self, status: String, coalesce: Option<&str>, f: impl FnOnce(&mut Song) -> Result<(), String>) {
        let before = self.editor.snapshot();
        match f(&mut self.editor.song) {
            Ok(()) => {
                self.editor.undo.push(before, coalesce);
                self.editor.song.normalize();
                self.song_changed();
                self.status = status;
            }
            Err(e) => self.status = e,
        }
    }

    fn save_to(&mut self, path: &Path) {
        let result = match extension(path).as_str() {
            "mid" | "midi" => midi_file::save(&self.editor.song, path).map_err(|e| e.to_string()),
            _ => project::save(&self.editor.song, path).map_err(|e| e.to_string()),
        };
        match result {
            Ok(()) => {
                self.dirty = false;
                self.file_path = Some(path.to_path_buf());
                self.remember_recent(path);
                self.status = format!("wrote {}", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"));
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    fn remember_recent(&mut self, path: &Path) {
        recent::remember(&self.config_dir, &mut self.recent, path);
    }

    fn open(&mut self, path: &Path) {
        match load_song_file(path) {
            Ok(song) => {
                self.editor = Editor::new(song);
                self.editor.cursor.pitch = 60;
                self.file_path = Some(path.to_path_buf());
                self.dirty = false;
                self.view = RollView::default();
                self.tracks = TracksArea::default();
                self.playhead = Tick::ZERO;
                self.needs_scroll = true;
                if let Ok(audio) = &mut self.audio {
                    audio.sync_song(&self.editor.song);
                }
                self.remember_recent(path);
                self.status = format!("opened {}", path.display());
            }
            Err(e) => self.status = format!("{}: {e}", path.display()),
        }
    }

    fn run_command(&mut self, line: &str) {
        let mut words = line.split_whitespace();
        let Some(name) = words.next() else { return };
        let rest: Vec<&str> = words.collect();
        let arg_path = || rest.first().map(PathBuf::from);
        let (name, force) = match name.strip_suffix('!') {
            Some(n) => (n, true),
            None => (name, false),
        };
        match name {
            "w" | "write" | "save" => match arg_path().or_else(|| self.file_path.clone()) {
                Some(path) => self.save_to(&path),
                None => self.status = "no file name: use :w <path.ron>".into(),
            },
            "wq" => {
                self.run_command("w");
                if !self.dirty {
                    self.status = "quit".into();
                }
            }
            "q" | "quit" => {
                if self.dirty && !force {
                    self.status = "unsaved changes: :w to save or :q! to discard".into();
                } else {
                    self.allow_close = true;
                    self.status = "quit".into();
                }
            }
            "o" | "open" | "e" | "edit" | "import" => match arg_path() {
                Some(path) if self.dirty && !force => {
                    self.status = format!("unsaved changes: :w first or :{name}! {}", path.display())
                }
                Some(path) => self.open(&path),
                None => self.status = format!("usage: :{name} <file.ron|file.mid>"),
            },
            "new" => {
                if self.dirty && !force {
                    self.status = "unsaved changes: :w first or :new!".into();
                } else {
                    self.editor = Editor::new(Song::new_empty());
                    self.editor.cursor.pitch = 60;
                    self.file_path = None;
                    self.dirty = false;
                    self.view = RollView::default();
                    self.tracks = TracksArea::default();
                    self.playhead = Tick::ZERO;
                    if let Ok(audio) = &mut self.audio {
                        audio.sync_song(&self.editor.song);
                    }
                    self.status = "new song".into();
                }
            }
            "export" => match arg_path() {
                None => self.status = "usage: :export <file.wav|file.mp3|file.mid>".into(),
                Some(path) => self.export(path),
            },
            "bpm" | "tempo" => match rest.first().and_then(|s| s.parse::<f64>().ok()) {
                Some(bpm) if (20.0..=400.0).contains(&bpm) => {
                    let tick = self.editor.cursor.tick;
                    let at = if rest.get(1) == Some(&"here") { tick } else { Tick::ZERO };
                    self.edit_song(format!("tempo {bpm} bpm"), |song| {
                        song.tempo_map.set_tempo_at(at, bpm).map_err(|e| e.to_string())
                    });
                }
                _ => self.status = "usage: :bpm <20-400> [here]".into(),
            },
            "meter" | "time" => {
                match rest.first().and_then(|s| parse_meter(s)) {
                    Some((n, d)) => {
                        let tick = self.editor.cursor.tick;
                        let at = if rest.get(1) == Some(&"here") { tick } else { Tick::ZERO };
                        self.edit_song(format!("meter {n}/{d}"), |song| {
                            song.meter_map.set_meter_at(at, n, d).map_err(|e| e.to_string())
                        });
                    }
                    _ => self.status = "usage: :meter <n>/<d> [here]".into(),
                }
            }
            "help" => self.show_cheat_sheet(),
            "keys" | "keymap" => self.open_keymap_editor(),
            _ => match cmdline::parse(line) {
                Ok(cmd) => match self.editor.execute(&cmd, None) {
                    Ok(events) => {
                        self.status = cmd.describe().to_string();
                        self.apply_events(events);
                        self.view.ensure_cursor_visible(&self.editor, self.roll_rect);
                    }
                    Err(e) => self.status = e.to_string(),
                },
                Err(e) => self.status = e.to_string(),
            },
        }
    }

    fn export(&mut self, path: PathBuf) {
        if matches!(extension(&path).as_str(), "mid" | "midi") {
            match midi_file::save(&self.editor.song, &path) {
                Ok(()) => self.status = format!("exported {}", path.display()),
                Err(e) => self.status = format!("export failed: {e}"),
            }
        } else {
            self.start_export(path);
        }
    }

    fn start_export(&mut self, path: PathBuf) {
        if self.export_rx.is_some() {
            self.status = "an export is already running".into();
            return;
        }
        let fonts = match &mut self.audio {
            Ok(audio) => audio.fonts_for(&self.editor.song),
            Err(e) => {
                self.status = format!("cannot export without audio: {e}");
                return;
            }
        };
        let song = self.editor.song.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(export_audio(song, fonts, path));
        });
        self.export_rx = Some(rx);
        self.status = "exporting…".into();
    }

    fn poll_export(&mut self) {
        let Some(rx) = &self.export_rx else { return };
        if let Ok(result) = rx.try_recv() {
            self.status = result.unwrap_or_else(|e| format!("export failed: {e}"));
            self.export_rx = None;
        }
    }

    /// Holds a close request (window button, Cmd+q, Quit) until unsaved changes are dealt with.
    fn guard_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) || self.allow_close || self.smoke_seconds.is_some() {
            return;
        }
        if self.confirm_discard() {
            self.allow_close = true;
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    fn autosave(&mut self, now: Instant) {
        if self.dirty && now.duration_since(self.last_autosave) >= AUTOSAVE_INTERVAL {
            self.last_autosave = now;
            let _ = project::save(&self.editor.song, self.config_dir.join("autosave.ron"));
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = format!("{}{} — MIDI Party", self.file_name(), if self.dirty { " •" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn handle_config_events(&mut self, ctx: &egui::Context) {
        for event in self.config.poll() {
            match event {
                ConfigEvent::ThemeChanged => {
                    let (theme, error) = Theme::load(&self.config_dir.join("theme.toml"));
                    ctx.set_visuals(theme.visuals());
                    self.theme = theme;
                    self.status = error.unwrap_or_else(|| "theme reloaded".into());
                }
                ConfigEvent::KeymapChanged => {
                    let (keymap, error) = load_keymap(&self.config_dir.join("keymap.toml"));
                    self.keymap = keymap;
                    #[cfg(target_os = "macos")]
                    self.menu.set_keymap(&self.keymap);
                    self.status = error.unwrap_or_else(|| "keymap reloaded".into());
                }
            }
        }
    }

    /// One line of cells separated by thin rules: where the cursor is, then the
    /// editing context, the latest message on the right.
    fn draw_status_bar(&self, ui: &mut egui::Ui) {
        let theme = &self.theme;
        let mono = theme.font(theme.font_size);
        let label_font = theme.font(theme.font_size - 3.0);
        let separator = Stroke::new(1.0, theme.color("panel_border"));
        let dim = theme.color("status_dim");
        let text = |s: String, color: &str| RichText::new(s).font(mono.clone()).color(theme.color(color));
        let cell = |ui: &mut egui::Ui, label: &str, value: RichText| {
            ui.add_space(8.0);
            if !label.is_empty() {
                ui.label(RichText::new(label).font(label_font.clone()).color(dim));
                ui.add_space(5.0);
            }
            ui.label(value);
            ui.add_space(8.0);
            ui.painter().vline(ui.cursor().left().round() + 0.5, ui.max_rect().y_range(), separator);
        };
        let e = &self.editor;
        let song = &e.song;
        ui.painter().hline(ui.max_rect().x_range(), ui.max_rect().top() - 4.5, separator);
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.horizontal(|ui| {
            if let Some(line) = &self.cmdline {
                cell(ui, "", text("command".into(), "status_mode"));
                ui.add_space(8.0);
                ui.label(text(format!(":{line}▏"), "status_fg"));
                return;
            }
            if self.track_rename.is_some() {
                cell(ui, "", text("rename".into(), "status_mode"));
                ui.add_space(8.0);
                ui.label(text("Enter keeps the name, Escape cancels".into(), "status_dim"));
                return;
            }
            if e.box_anchor.is_some() {
                cell(ui, "", text(format!("{} in box", e.selection.len()), "status_mode"));
            }
            let mut pending: Vec<String> = e.count.map(|c| c.to_string()).into_iter().collect();
            pending.extend(self.keymap.pending().iter().map(|k| k.to_string()));
            if !pending.is_empty() {
                cell(ui, "", text(pending.join(" "), "accent"));
            }
            cell(ui, "", text(song.meter_map.bar_beat_of(song.ppq, e.cursor.tick).to_string(), "status_fg"));
            cell(ui, "", text(format!("{} {}", pitch_name(e.cursor.pitch), e.cursor.pitch), "status_fg"));
            cell(ui, "grid", text(e.grid.to_string(), "status_fg"));
            let track_name = e.track().map(|t| t.name.as_str()).unwrap_or("-");
            cell(ui, "track", text(format!("{}/{} {}", e.track_index + 1, song.tracks.len(), track_name), "status_fg"));
            cell(ui, "reverb", text(format!("{}%", (song.master_reverb * 100.0).round() as i32), "status_fg"));
            if !e.selection.is_empty() {
                cell(ui, "", text(format!("{} selected", e.selection.len()), "note_selected"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                ui.add(egui::Label::new(RichText::new(self.status.clone()).font(theme.font(theme.font_size - 1.0)).color(dim)).truncate());
            });
        });
    }
}

/// `n/d` with a numerator of at least 1 and a power-of-two denominator up to 32.
fn parse_meter(text: &str) -> Option<(u8, u8)> {
    let (n, d) = text.split_once('/')?;
    let (n, d) = (n.trim().parse::<u8>().ok()?, d.trim().parse::<u8>().ok()?);
    (n > 0 && [1, 2, 4, 8, 16, 32].contains(&d)).then_some((n, d))
}

/// File and app shortcuts that work even while the command line, a rename or the picker has the keyboard.
const APP_CHORDS: &[&str] = &["Cmd+s", "Shift+Cmd+s", "Cmd+o", "Cmd+n", "Cmd+,", "Cmd+k", "Cmd+q"];

/// Command-line commands the app handles itself (name, usage).
const APP_COMMANDS: &[(&str, &str)] = &[
    ("w", "w [file.ron|file.mid]"),
    ("wq", "wq"),
    ("q", "q (q! discards changes)"),
    ("o", "o <file.ron|file.mid>"),
    ("import", "import <file.mid>"),
    ("new", "new"),
    ("export", "export <file.wav|file.mp3|file.mid>"),
    ("bpm", "bpm <20-400> [here]"),
    ("meter", "meter <n>/<d> [here]"),
    ("help", "help"),
    ("keys", "keys (open the keymap editor; also Cmd+k)"),
];

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        if let Some(limit) = self.smoke_seconds {
            let elapsed = now.duration_since(self.started).as_secs_f64();
            if elapsed >= limit {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if elapsed >= limit / 2.0 {
                if let Some(path) = self.screenshot.take() {
                    let callback = egui::ScreenshotCallback::new(move |image| save_png(&path, &image));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
                }
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if !ctx.input(|i| i.focused) {
            self.input.clear();
        }
        let polled = self.input.poll(&ctx, now);
        for chord in polled.chords {
            self.handle_key(&chord.to_string(), &ctx);
        }
        if !polled.text.is_empty() {
            if self.picker.open {
                self.picker.handle_text(&polled.text);
            } else if self.keymap_editor.open {
                self.keymap_editor.handle_text(&polled.text);
            } else if self.track_rename.is_some() {
                self.rename_text(&polled.text);
            } else if self.value_entry.is_some() {
                self.entry_text(&polled.text);
            } else {
                self.cmdline_text(&polled.text);
            }
        }
        self.handle_menu_clicks(&ctx);
        self.handle_config_events(&ctx);
        self.guard_close(&ctx);
        #[cfg(target_os = "macos")]
        {
            self.menu.set_recent(&self.recent);
            self.menu.set_piano_roll_shown(self.show_roll);
        }
        self.poll_export();
        self.autosave(now);
        self.update_title(&ctx);
        if let Ok(audio) = &mut self.audio {
            audio.frame_tick();
        }
        self.sync_playhead();
        if self.is_playing() {
            let tick = self.playhead.get() as f64;
            self.view.follow_playhead(tick, self.roll_rect);
            self.tracks.keep_tick_visible(tick);
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.export_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        let status_bg = self.theme.color("status_bg");
        egui::Panel::bottom("status")
            .frame(egui::Frame::NONE.fill(status_bg).inner_margin(egui::Margin::symmetric(0, 5)))
            .show(ui, |ui| self.draw_status_bar(ui));
        let mut transport_actions = Vec::new();
        egui::Panel::top("transport")
            .frame(egui::Frame::NONE.fill(status_bg))
            .exact_size(transport_bar::BAR_H)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| {
                let song = &self.editor.song;
                let (playing, seconds) = match &self.audio {
                    Ok(a) if a.is_playing() => (true, a.playhead_seconds()),
                    _ => (false, self.tick_seconds(self.playhead)),
                };
                let view = TransportView {
                    audio_ok: self.audio.is_ok(),
                    playing,
                    position: song.meter_map.bar_beat_of(song.ppq, self.playhead),
                    seconds,
                    bpm: song.tempo_map.bpm_at(self.playhead),
                    meter: song.meter_map.meter_at(self.playhead),
                    loop_on: song.loop_region.is_some_and(|r| r.enabled),
                    metronome: self.metronome,
                    entry: self.value_entry.as_ref().map(|(field, text)| (*field, text.as_str())),
                    keymap: &self.keymap,
                };
                self.transport.draw(ui, &view, &self.theme, &mut transport_actions);
            });
        for action in transport_actions {
            self.apply_transport_action(action);
        }
        let playhead = Some(self.playhead.get() as f64);
        let background = self.theme.color("background");
        let names: Vec<String> = {
            let tracks = &self.editor.song.tracks;
            match &mut self.audio {
                Ok(audio) => tracks.iter().map(|t| audio.preset_name(t).unwrap_or_else(|| instrument_name(t))).collect(),
                Err(_) => tracks.iter().map(instrument_name).collect(),
            }
        };
        let rename = self.track_rename.as_ref().map(|(track, text)| (*track, text.as_str()));
        let mut track_actions = Vec::new();
        let mut actions = Vec::new();
        if self.show_roll {
            let roll_window = self.view.visible_ticks(self.roll_rect);
            let available = ui.available_height();
            egui::Panel::top("tracks")
                .frame(egui::Frame::NONE.fill(status_bg))
                .resizable(true)
                .default_size(220.0)
                .size_range(egui::Rangef::new(80.0, (available - 160.0).max(80.0)))
                .show(ui, |ui| {
                    self.tracks.draw(ui, &self.editor, &self.theme, playhead, roll_window, &names, rename, &mut track_actions);
                });
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(background))
                .show(ui, |ui| {
                    self.roll_rect = self.view.draw(ui, &self.editor, &self.theme, playhead, &mut actions);
                });
        } else {
            self.roll_rect = Rect::NOTHING;
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(status_bg))
                .show(ui, |ui| {
                    self.tracks.draw(ui, &self.editor, &self.theme, playhead, None, &names, rename, &mut track_actions);
                });
        }
        for action in track_actions {
            self.apply_tracks_action(action);
        }
        for action in actions {
            self.apply_roll_action(action);
        }
        if self.needs_scroll && self.roll_rect.is_positive() {
            self.scroll_views_to_cursor();
            self.needs_scroll = false;
        }
        if self.show_cheat_sheet {
            cheat_sheet::draw(&ctx, &self.keymap, &self.theme, &self.config_dir);
        }
        if self.keymap_editor.open {
            self.keymap_editor.draw(&ctx, &self.theme);
        }
        if self.picker.open {
            self.picker.draw(&ctx, &self.theme);
        }
    }
}

fn main() -> eframe::Result {
    let args = Args::parse();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MIDI Party")
            .with_icon(app_icon())
            .with_inner_size([1200.0, 720.0])
            .with_min_inner_size([640.0, 360.0]),
        // Otherwise egui-winit turns Cmd+c/x/v into clipboard events and the keymap never sees them.
        clipboard_shortcuts: false,
        ..Default::default()
    };
    eframe::run_native("midi-party", options, Box::new(move |cc| Ok(Box::new(App::new(cc, args)))))
}

#[cfg(test)]
mod keymap_overlay_tests {
    use super::*;

    #[test]
    fn user_bindings_override_unbind_and_keep_the_rest() {
        let user = r#"[normal]
"n" = "delete_selected"
"x" = "unbind"
"f" = "select_all"
"#;
        let merged = merge_keymap_toml(user).unwrap();
        let keymap = Keymap::from_toml_str(&merged).unwrap();
        let normal = keymap.bindings();
        let find = |k: &str| normal.iter().find(|(keys, _)| keys == k).map(|(_, c)| c.name());
        assert_eq!(find("n"), Some("delete_selected"));
        assert_eq!(find("x"), None);
        assert_eq!(find("f"), Some("select_all"), "user 'f' replaces the default 'f …' sequences");
        assert!(find("f t").is_none());
        assert!(find("Right").is_some(), "untouched defaults survive");
    }

    #[test]
    fn vim_preset_moves_the_cursor_with_hjkl_and_keeps_the_rest() {
        let merged = merge_keymap_toml("preset = \"vim\"\n[normal]\n\"z\" = \"undo\"\n").unwrap();
        let keymap = Keymap::from_toml_str(&merged).unwrap();
        let normal = keymap.bindings();
        let find = |k: &str| normal.iter().find(|(keys, _)| keys == k).map(|(_, c)| c.name());
        assert_eq!(find("h"), Some("move_cursor"));
        assert_eq!(find("k"), Some("move_cursor"));
        assert_eq!(find("Ctrl+Alt+h"), Some("humanize"));
        assert_eq!(find("Alt+h"), Some("move_selected"));
        assert_eq!(find("Shift+l"), Some("extend_selection"));
        assert_eq!(find("z"), Some("undo"));
        assert_eq!(find("x"), Some("delete_selected"));
    }

    #[test]
    fn retired_mode_tables_in_a_user_file_only_warn() {
        let user = "[normal]\n\"z\" = \"undo\"\n\n[select]\n\"v\" = \"select_all\"\n\n[move]\n\"m\" = \"copy\"\n\n[command_line]\n\"Escape\" = \"undo\"\n";
        let merged = merge_keymap_toml(user).unwrap();
        let keymap = Keymap::from_toml_str(&merged).unwrap();
        assert_eq!(keymap.warnings().len(), 3, "{:?}", keymap.warnings());
        assert!(keymap.bindings().iter().any(|(k, c)| k == "z" && c.name() == "undo"));
    }

    #[test]
    fn unknown_preset_is_an_error() {
        assert!(merge_keymap_toml("preset = \"emacs\"\n").is_err());
    }

    #[test]
    fn empty_or_comment_only_file_is_the_default_keymap() {
        let merged = merge_keymap_toml(KEYMAP_TEMPLATE).unwrap();
        let a = Keymap::from_toml_str(&merged).unwrap();
        let b = default_keymap();
        assert_eq!(a.bindings().len(), b.bindings().len());
    }
}
