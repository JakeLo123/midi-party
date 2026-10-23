//! midi-party: a keyboard-driven MIDI piano-roll editor.
//!
//! The app is deliberately thin: keys go to `mp_core::Keymap`, commands run in
//! `mp_core::Editor`, and the editor's events drive audio and the status bar.
//! The `:` command line handles files and song-level settings here, and hands
//! everything else to the editor.

mod bridge;
mod cheat_sheet;
mod config;
mod input;
mod keymap_editor;
mod piano_roll;
mod theme;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use egui::{FontId, Rect, RichText};
use mp_audio::render::{render_song, write_audio_file, RenderOptions};
use mp_audio::{render_song_from, SoundFontSynth, Synth};
use mp_core::{
    cmdline, default_keymap, feed_one, midi_file, pitch_name, project, try_drive_with, Command, Cursor, DriveError,
    Editor, EditorEvent, KeyChord, Keymap, Mode, Song, Tick, TransportRequest, ViewRequest, DEFAULT_KEYMAP_TOML,
};
use rustysynth::SoundFont;

use crate::bridge::Audio;
use crate::config::{ConfigEvent, ConfigWatcher};
use crate::input::{Chord, InputState, KeyRepeat};
use crate::keymap_editor::KeymapEditor;
use crate::piano_roll::{RollAction, RollView};
use crate::theme::Theme;

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);
const EXPORT_SAMPLE_RATE: u32 = 44_100;

#[derive(Parser)]
#[command(name = "midi-party")]
struct Args {
    /// A .ron project or .mid file to open.
    file: Option<PathBuf>,
    /// Default SoundFont, used by tracks that have none of their own.
    #[arg(long, default_value = "assets/soundfonts/florestan-subset.sf2")]
    sf2: PathBuf,
    /// Config directory holding keymap.toml and theme.toml
    /// (default: ~/.config/midi-party; both files are created on first run).
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
# cursor_fill, playhead, status_bg, status_fg, status_mode, status_dim, accent, error.
#
# font_size = 14.0
#
# [colors]
# note          = "#5aa0d8"
# note_selected = "#f5c542"
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
        Ok(k) => (k, None),
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
    config: ConfigWatcher,
    config_dir: PathBuf,
    theme: Theme,
    input: InputState,
    status: String,
    show_cheat_sheet: bool,
    keymap_editor: KeymapEditor,
    /// Scroll the cursor into view once the roll has been laid out.
    needs_scroll: bool,
    /// The `:` command line while it is open.
    cmdline: Option<String>,
    file_path: Option<PathBuf>,
    dirty: bool,
    title: String,
    last_autosave: Instant,
    export_rx: Option<Receiver<Result<String, String>>>,
    started: Instant,
    smoke_seconds: Option<f64>,
    screenshot: Option<PathBuf>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        let config_dir = args.config_dir.unwrap_or_else(default_config_dir);
        ensure_config_files(&config_dir);
        let (theme, theme_error) = Theme::load(&config_dir.join("theme.toml"));
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
        let mut audio = Audio::open(&args.sf2).map_err(|e| e.to_string());
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

        let mut app = Self {
            editor,
            keymap,
            audio,
            view: RollView::default(),
            roll_rect: Rect::NOTHING,
            config,
            config_dir,
            theme,
            input: InputState::new(KeyRepeat::default()),
            status,
            show_cheat_sheet: false,
            keymap_editor: KeymapEditor::default(),
            needs_scroll: true,
            cmdline: None,
            file_path,
            dirty: false,
            title: String::new(),
            last_autosave: Instant::now(),
            export_rx: None,
            started: Instant::now(),
            smoke_seconds: args.smoke_seconds,
            screenshot: args.screenshot,
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

    fn playhead_tick(&self) -> Option<f64> {
        let audio = self.audio.as_ref().ok()?;
        if !audio.is_playing() {
            return None;
        }
        let song = &self.editor.song;
        Some(song.tempo_map.seconds_to_tick(song.ppq, audio.playhead_seconds()).get() as f64)
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
            "Cmd+s" => self.run_command("w"),
            "Cmd+k" => self.keymap_editor.open(&self.keymap),
            "Cmd+q" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            _ => return false,
        }
        true
    }

    fn handle_chord(&mut self, chord: Chord, ctx: &egui::Context) {
        let text = chord.to_string();
        if self.keymap_editor.open {
            let path = self.config_dir.join("keymap.toml");
            if let Some(status) = self.keymap_editor.handle_chord(&text, &path) {
                self.status = status;
            }
            return;
        }
        if self.cmdline.is_some() {
            self.handle_cmdline_chord(&text, ctx);
            return;
        }
        if self.handle_app_chord(&text, ctx) {
            return;
        }
        let chord: KeyChord = match text.parse() {
            Ok(c) => c,
            Err(_) => return, // a key the keymap syntax has no name for (F-keys etc.)
        };
        let mut events = Vec::new();
        match feed_one(&mut self.editor, &mut self.keymap, chord, &mut events) {
            Ok(()) => {}
            Err(DriveError::Unbound { key, mode }) => self.status = format!("{key} is not bound in {mode} mode"),
            Err(DriveError::Edit { key, source }) => self.status = format!("{key}: {source}"),
            Err(other) => self.status = other.to_string(),
        }
        self.apply_events(events);
        self.view.ensure_cursor_visible(&self.editor, self.roll_rect);
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
                EditorEvent::ModeChanged(Mode::CommandLine) => self.cmdline = Some(String::new()),
                EditorEvent::ModeChanged(_) => {}
                EditorEvent::Status(s) => self.status = s,
                EditorEvent::Transport(request) => {
                    let secs = |t: Tick| self.tick_seconds(t);
                    let from = match request {
                        TransportRequest::Play { from } | TransportRequest::Toggle { from } => secs(from),
                        TransportRequest::Stop => 0.0,
                    };
                    self.with_audio(|a| match request {
                        TransportRequest::Play { .. } => a.play_from(from).map(|_| "playing".into()),
                        TransportRequest::Stop => a.stop().map(|_| "stopped".into()),
                        TransportRequest::Toggle { .. } => {
                            if a.is_playing() {
                                a.stop().map(|_| "stopped".into())
                            } else {
                                a.play_from(from).map(|_| "playing".into())
                            }
                        }
                    });
                }
                EditorEvent::ToggleCheatSheet => self.show_cheat_sheet = !self.show_cheat_sheet,
                EditorEvent::View(ViewRequest::ZoomTime(factor)) => self.view.zoom_time(factor),
                EditorEvent::View(ViewRequest::ZoomRows(factor)) => self.view.zoom_rows(factor),
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
        if self.cmdline.is_some() || self.show_cheat_sheet {
            return;
        }
        match action {
            RollAction::SetCursor { tick, pitch } => {
                self.editor.cursor = Cursor { tick: Tick(tick), pitch };
                self.editor.selection.clear();
                self.editor.focus = None;
            }
            RollAction::SetCursorTick { tick } => self.editor.cursor.tick = Tick(tick),
            RollAction::ClickNote { id, toggle } => {
                if let Some(note) = self.editor.track().and_then(|t| t.note(id)).copied() {
                    self.editor.cursor = Cursor { tick: note.start, pitch: note.pitch };
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
        self.view.ensure_cursor_visible(&self.editor, self.roll_rect);
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

    fn close_cmdline(&mut self) -> Option<String> {
        if self.editor.mode == Mode::CommandLine {
            self.editor.mode = Mode::Normal;
        }
        self.cmdline.take()
    }

    fn handle_cmdline_chord(&mut self, text: &str, ctx: &egui::Context) {
        match text {
            "Escape" => {
                self.close_cmdline();
            }
            "Enter" => {
                if let Some(line) = self.close_cmdline() {
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
        let before = self.editor.snapshot();
        match f(&mut self.editor.song) {
            Ok(()) => {
                self.editor.undo.push(before, None);
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
                self.status = format!("wrote {}", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"));
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    fn open(&mut self, path: &Path) {
        match load_song_file(path) {
            Ok(song) => {
                self.editor = Editor::new(song);
                self.editor.cursor.pitch = 60;
                self.file_path = Some(path.to_path_buf());
                self.dirty = false;
                self.view = RollView::default();
                self.needs_scroll = true;
                if let Ok(audio) = &mut self.audio {
                    audio.sync_song(&self.editor.song);
                }
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
                    if let Ok(audio) = &mut self.audio {
                        audio.sync_song(&self.editor.song);
                    }
                    self.status = "new song".into();
                }
            }
            "export" => match arg_path() {
                None => self.status = "usage: :export <file.wav|file.mp3|file.mid>".into(),
                Some(path) if matches!(extension(&path).as_str(), "mid" | "midi") => {
                    match midi_file::save(&self.editor.song, &path) {
                        Ok(()) => self.status = format!("exported {}", path.display()),
                        Err(e) => self.status = format!("export failed: {e}"),
                    }
                }
                Some(path) => self.start_export(path),
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
                let parsed = rest.first().and_then(|s| {
                    let (n, d) = s.split_once('/')?;
                    Some((n.parse::<u8>().ok()?, d.parse::<u8>().ok()?))
                });
                match parsed {
                    Some((n, d)) if n > 0 && [1, 2, 4, 8, 16, 32].contains(&d) => {
                        let tick = self.editor.cursor.tick;
                        let at = if rest.get(1) == Some(&"here") { tick } else { Tick::ZERO };
                        self.edit_song(format!("meter {n}/{d}"), |song| {
                            song.meter_map.set_meter_at(at, n, d).map_err(|e| e.to_string())
                        });
                    }
                    _ => self.status = "usage: :meter <n>/<d> [here]".into(),
                }
            }
            "help" => self.show_cheat_sheet = true,
            "keys" | "keymap" => self.keymap_editor.open(&self.keymap),
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

    fn autosave(&mut self, now: Instant) {
        if self.dirty && now.duration_since(self.last_autosave) >= AUTOSAVE_INTERVAL {
            self.last_autosave = now;
            let _ = project::save(&self.editor.song, self.config_dir.join("autosave.ron"));
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let name = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("untitled");
        let title = format!("{name}{} — midi-party", if self.dirty { " •" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn handle_config_events(&mut self) {
        for event in self.config.poll() {
            match event {
                ConfigEvent::ThemeChanged => {
                    let (theme, error) = Theme::load(&self.config_dir.join("theme.toml"));
                    self.theme = theme;
                    self.status = error.unwrap_or_else(|| "theme reloaded".into());
                }
                ConfigEvent::KeymapChanged => {
                    let (keymap, error) = load_keymap(&self.config_dir.join("keymap.toml"));
                    self.keymap = keymap;
                    self.status = error.unwrap_or_else(|| "keymap reloaded".into());
                }
            }
        }
    }

    /// The track list with mixer state. Returns a track index when one is clicked.
    fn draw_track_panel(&self, ui: &mut egui::Ui) -> Option<usize> {
        let font = FontId::monospace(self.theme.font_size - 1.0);
        let text = |s: String, color: &str| RichText::new(s).font(font.clone()).color(self.theme.color(color));
        let song = &self.editor.song;
        let mut clicked = None;
        let any_solo = song.tracks.iter().any(|t| t.solo);
        ui.label(text("TRACKS".into(), "status_mode").strong());
        ui.add_space(2.0);
        for (i, track) in song.tracks.iter().enumerate() {
            let current = i == self.editor.track_index;
            let instrument = match &track.instrument {
                mp_core::Instrument::SoundFont { path, preset, .. } => {
                    let font_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("default");
                    format!("{font_name} p{preset}")
                }
            };
            let muted = track.mute || (any_solo && !track.solo);
            let flags = format!("{}{}", if track.mute { "M" } else { "·" }, if track.solo { "S" } else { "·" });
            let line1 = format!("{:>2} {}", i + 1, track.name);
            let line2 = format!("   {instrument}");
            let line3 = format!("   {:>3}% {:<4} {flags}", (track.volume * 100.0).round() as i32, mp_core::pan_label((track.pan * 100.0).round() as i32));
            let name_color = if current { "note_selected" } else if muted { "status_dim" } else { "status_fg" };
            let frame = egui::Frame::NONE.fill(if current { self.theme.color("black_key_row") } else { self.theme.color("status_bg") }).inner_margin(3.0);
            let response = frame
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(text(line1, name_color).strong());
                    ui.label(text(line2, "status_dim"));
                    ui.label(text(line3, if muted { "status_dim" } else { "status_fg" }));
                })
                .response
                .interact(egui::Sense::click());
            if response.clicked() {
                clicked = Some(i);
            }
        }
        ui.add_space(6.0);
        ui.label(text(format!("reverb {:>3}%", (song.master_reverb * 100.0).round() as i32), "status_dim"));
        ui.label(text(format!("{:.0} bpm", song.tempo_map.bpm_at(Tick::ZERO)), "status_dim"));
        clicked
    }

    fn draw_status_bar(&self, ui: &mut egui::Ui) {
        let font = FontId::monospace(self.theme.font_size);
        let text = |s: String, color: &str| RichText::new(s).font(font.clone()).color(self.theme.color(color));
        let e = &self.editor;
        let song = &e.song;
        ui.horizontal(|ui| {
            if let Some(line) = &self.cmdline {
                ui.label(text(" COMMAND ".into(), "status_mode").strong());
                ui.label(text(format!(":{line}▏"), "status_fg"));
                return;
            }
            ui.label(text(format!(" {} ", e.mode.name().to_uppercase()), "status_mode").strong());
            let mut pending: Vec<String> = e.count.map(|c| c.to_string()).into_iter().collect();
            pending.extend(self.keymap.pending().iter().map(|k| k.to_string()));
            if !pending.is_empty() {
                ui.label(text(pending.join(" "), "accent"));
            }
            ui.label(text(song.meter_map.bar_beat_of(song.ppq, e.cursor.tick).to_string(), "status_fg"));
            ui.label(text(format!("{} ({})", pitch_name(e.cursor.pitch), e.cursor.pitch), "status_fg"));
            ui.label(text(format!("grid {}", e.grid), "status_dim"));
            let track_name = e.track().map(|t| t.name.as_str()).unwrap_or("-");
            ui.label(text(format!("track {}/{} {}", e.track_index + 1, song.tracks.len(), track_name), "status_dim"));
            ui.label(text(format!("{:.0} bpm", song.tempo_map.bpm_at(e.cursor.tick)), "status_dim"));
            if !e.selection.is_empty() {
                ui.label(text(format!("{} selected", e.selection.len()), "note_selected"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let transport = match &self.audio {
                    Ok(a) if a.is_playing() => format!("▶ {:6.2}s", a.playhead_seconds()),
                    Ok(_) => "■".to_string(),
                    Err(_) => "no audio".to_string(),
                };
                ui.label(text(transport, "status_fg"));
                ui.add(egui::Label::new(text(self.status.clone(), "status_dim")).truncate());
            });
        });
    }
}

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
            self.handle_chord(chord, &ctx);
        }
        if !polled.text.is_empty() {
            if self.keymap_editor.open {
                self.keymap_editor.handle_text(&polled.text);
            } else {
                self.cmdline_text(&polled.text);
            }
        }
        self.handle_config_events();
        self.poll_export();
        self.autosave(now);
        self.update_title(&ctx);
        if let Ok(audio) = &mut self.audio {
            audio.frame_tick();
        }
        if let Some(tick) = self.playhead_tick() {
            self.view.follow_playhead(tick, self.roll_rect);
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.export_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        let status_bg = self.theme.color("status_bg");
        egui::Panel::bottom("status")
            .frame(egui::Frame::NONE.fill(status_bg).inner_margin(4.0))
            .show(ui, |ui| self.draw_status_bar(ui));
        let mut clicked_track = None;
        egui::Panel::left("tracks")
            .frame(egui::Frame::NONE.fill(status_bg).inner_margin(6.0))
            .exact_size(190.0)
            .resizable(false)
            .show(ui, |ui| clicked_track = self.draw_track_panel(ui));
        if let Some(index) = clicked_track {
            if index != self.editor.track_index && index < self.editor.song.tracks.len() {
                self.editor.track_index = index;
                self.editor.selection.clear();
                self.editor.focus = None;
            }
        }
        let playhead = self.playhead_tick();
        let background = self.theme.color("background");
        let mut actions = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(background))
            .show(ui, |ui| {
                self.roll_rect = self.view.draw(ui, &self.editor, &self.theme, playhead, &mut actions);
            });
        for action in actions {
            self.apply_roll_action(action);
        }
        if self.needs_scroll && self.roll_rect.is_positive() {
            self.view.ensure_cursor_visible(&self.editor, self.roll_rect);
            self.needs_scroll = false;
        }
        if self.show_cheat_sheet {
            cheat_sheet::draw(&ctx, &self.keymap, &self.theme);
        }
        if self.keymap_editor.open {
            self.keymap_editor.draw(&ctx, &self.theme);
        }
    }
}

fn main() -> eframe::Result {
    let args = Args::parse();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("midi-party")
            .with_inner_size([1200.0, 720.0])
            .with_min_inner_size([640.0, 360.0]),
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
        let normal = keymap.bindings(Mode::Normal);
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
        let normal = keymap.bindings(Mode::Normal);
        let find = |k: &str| normal.iter().find(|(keys, _)| keys == k).map(|(_, c)| c.name());
        assert_eq!(find("h"), Some("move_cursor"));
        assert_eq!(find("k"), Some("move_cursor"));
        assert_eq!(find("Alt+h"), Some("humanize"));
        assert_eq!(find("z"), Some("undo"));
        assert_eq!(find("x"), Some("delete_selected"));
        assert!(keymap.bindings(Mode::Move).iter().any(|(k, c)| k == "j" && c.name() == "move_selected"));
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
        assert_eq!(a.bindings(Mode::Normal).len(), b.bindings(Mode::Normal).len());
    }
}
