//! The `:` command line: text → [`Command`].
//!
//! The first word is a command name (the snake_case [`Command::name`]) or one of a few
//! short aliases (`q` = quantize, `vel` = set_velocity, `t`/`tr` = transpose_selected,
//! `h` = humanize, `sel <what>` for the select commands, `keep <what>` for the filters,
//! `track <what>` / `lane <what>` / `pedal <what>` for tracks, lanes and the pedal).
//! The remaining words are positional arguments in the order the variant declares its
//! fields, parsed by type: grids as `1/16` or `1/8T`, percentages with an optional `%`,
//! intervals as `0,4,7` or `0 4 7`. `vel 80..110` is a velocity ramp. `rename` takes
//! the rest of the line as the name; `sf2 <path|default> [preset] [bank]` takes the
//! path up to the trailing numbers, so paths with spaces work; `channel` counts from 1.
//!
//! The app owns the text field; it calls [`complete`] while typing, [`usage`] for the
//! hint line, [`parse`] on Enter and hands the result to [`crate::Editor::execute`].
//! [`to_line`] is the inverse of [`parse`], used by the tests to prove every variant
//! round-trips (so a new variant cannot be forgotten here).

use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

use crate::editor::{Command, CycleDir, Dir, Unit};
use crate::song::LaneKind;
use crate::time::Grid;

/// Why a command line could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CmdLineError {
    #[error("type a command, e.g. :quantize 1/16")]
    Empty,
    #[error("unknown command {0:?}")]
    UnknownCommand(String),
    #[error("{command}: missing {arg} — usage: {usage}")]
    MissingArg { command: String, arg: String, usage: String },
    #[error("{command}: {arg} {value:?}: {reason}")]
    BadArg { command: String, arg: String, value: String, reason: String },
    #[error("{command}: too many arguments — usage: {usage}")]
    TooManyArgs { command: String, usage: String },
}

/// What one command takes on the command line. `args` is the usage text after the
/// name; `aliases` are alternative first words.
struct Spec {
    name: &'static str,
    args: &'static str,
    aliases: &'static [&'static str],
}

/// One entry per [`Command`] variant (checked by a test against [`Command::all_names`]).
const SPECS: &[Spec] = &[
    Spec { name: "move_cursor", args: "<left|right|up|down> <grid|beat|bar|semitone|octave>", aliases: &[] },
    Spec { name: "extend_selection", args: "<left|right|up|down> <grid|beat|bar|semitone|octave>", aliases: &[] },
    Spec { name: "cursor_to_start", args: "", aliases: &["start"] },
    Spec { name: "insert_note", args: "", aliases: &["n"] },
    Spec { name: "insert_chord", args: "<intervals e.g. 0,4,7>", aliases: &["chord"] },
    Spec { name: "delete_selected", args: "", aliases: &["del", "delete"] },
    Spec { name: "move_selected", args: "<left|right|up|down> <grid|beat|bar|semitone|octave>", aliases: &[] },
    Spec { name: "nudge_selected", args: "<ticks>", aliases: &["nudge"] },
    Spec { name: "change_length", args: "<delta steps>", aliases: &[] },
    Spec { name: "set_length", args: "<steps>", aliases: &["len"] },
    Spec { name: "legato", args: "", aliases: &[] },
    Spec { name: "nudge_velocity", args: "<delta>", aliases: &[] },
    Spec { name: "set_velocity", args: "<1-127 | from..to>", aliases: &["vel"] },
    Spec { name: "ramp_velocity", args: "<from> <to>", aliases: &["ramp"] },
    Spec { name: "humanize", args: "[velocity] [timing ticks]", aliases: &["h"] },
    Spec { name: "quantize", args: "[grid] [strength%] [swing%]", aliases: &["q"] },
    Spec { name: "quantize_length", args: "", aliases: &["ql"] },
    Spec { name: "set_swing", args: "<percent>", aliases: &["swing"] },
    Spec { name: "transpose_selected", args: "<semitones>", aliases: &["t", "tr", "transpose"] },
    Spec { name: "undo", args: "", aliases: &["u"] },
    Spec { name: "redo", args: "", aliases: &[] },
    Spec { name: "toggle_select_under_cursor", args: "", aliases: &[] },
    Spec { name: "select_all", args: "", aliases: &["sel all"] },
    Spec { name: "clear_selection", args: "", aliases: &["sel none"] },
    Spec { name: "select_bar", args: "", aliases: &["sel bar"] },
    Spec { name: "select_chord", args: "", aliases: &["sel chord"] },
    Spec { name: "select_next_notes", args: "[n]", aliases: &["sel next"] },
    Spec { name: "select_to_end", args: "", aliases: &["sel end"] },
    Spec { name: "select_to_start", args: "", aliases: &["sel start"] },
    Spec { name: "select_same_pitch", args: "", aliases: &["sel pitch"] },
    Spec { name: "select_velocity_above", args: "<velocity>", aliases: &["sel above"] },
    Spec { name: "select_velocity_below", args: "<velocity>", aliases: &["sel below"] },
    Spec { name: "select_top_of_chords", args: "", aliases: &["sel top"] },
    Spec { name: "select_bottom_of_chords", args: "", aliases: &["sel bottom"] },
    Spec { name: "invert_selection", args: "", aliases: &["sel invert"] },
    Spec { name: "selection_focus_next", args: "", aliases: &["focus next"] },
    Spec { name: "selection_focus_prev", args: "", aliases: &["focus prev"] },
    Spec { name: "drop_focused", args: "", aliases: &["drop"] },
    Spec { name: "keep_top_of_chords", args: "", aliases: &["keep top"] },
    Spec { name: "keep_bottom_of_chords", args: "", aliases: &["keep bottom"] },
    Spec { name: "keep_every_other", args: "[n]", aliases: &["keep every"] },
    Spec { name: "keep_on_beat", args: "", aliases: &["keep on_beat"] },
    Spec { name: "keep_off_beat", args: "", aliases: &["keep off_beat"] },
    Spec { name: "keep_above_cursor", args: "", aliases: &["keep above"] },
    Spec { name: "keep_below_cursor", args: "", aliases: &["keep below"] },
    Spec { name: "keep_after_cursor", args: "", aliases: &["keep after"] },
    Spec { name: "keep_before_cursor", args: "", aliases: &["keep before"] },
    Spec { name: "copy", args: "", aliases: &["c"] },
    Spec { name: "cut", args: "", aliases: &[] },
    Spec { name: "paste", args: "", aliases: &["p"] },
    Spec { name: "duplicate", args: "", aliases: &["dup"] },
    Spec { name: "set_grid", args: "<grid e.g. 1/16, 1/8T, 1/4.>", aliases: &["grid"] },
    Spec { name: "cycle_grid", args: "<next|prev>", aliases: &[] },
    Spec { name: "next_track", args: "", aliases: &[] },
    Spec { name: "prev_track", args: "", aliases: &[] },
    Spec { name: "zoom_in", args: "", aliases: &[] },
    Spec { name: "zoom_out", args: "", aliases: &[] },
    Spec { name: "zoom_rows_in", args: "", aliases: &[] },
    Spec { name: "zoom_rows_out", args: "", aliases: &[] },
    Spec { name: "toggle_piano_roll", args: "", aliases: &["roll"] },
    Spec { name: "zoom_to_fit", args: "", aliases: &["fit"] },
    Spec { name: "toggle_cheat_sheet", args: "", aliases: &["help", "keys"] },
    Spec { name: "add_track", args: "", aliases: &["track add"] },
    Spec { name: "remove_track", args: "", aliases: &["track remove", "track rm"] },
    Spec { name: "duplicate_track", args: "", aliases: &["track dup"] },
    Spec { name: "move_track_up", args: "", aliases: &["track up"] },
    Spec { name: "move_track_down", args: "", aliases: &["track down"] },
    Spec { name: "rename_track", args: "<name...>", aliases: &["rename", "track rename"] },
    Spec { name: "edit_track_name", args: "", aliases: &[] },
    Spec { name: "set_track_volume", args: "<0-200%>", aliases: &["volume", "vol", "track volume"] },
    Spec { name: "nudge_track_volume", args: "<delta%>", aliases: &[] },
    Spec { name: "set_track_pan", args: "<-100..100 | L50 | C | R50>", aliases: &["pan", "track pan"] },
    Spec { name: "nudge_track_pan", args: "<delta>", aliases: &[] },
    Spec { name: "toggle_mute", args: "", aliases: &["mute", "track mute"] },
    Spec { name: "toggle_solo", args: "", aliases: &["solo", "track solo"] },
    Spec { name: "set_track_channel", args: "<1-16>", aliases: &["channel", "ch", "track channel"] },
    Spec { name: "set_instrument", args: "<path|default> [preset] [bank]", aliases: &["sf2", "instrument", "track sf2"] },
    Spec { name: "next_preset", args: "", aliases: &["preset next"] },
    Spec { name: "prev_preset", args: "", aliases: &["preset prev"] },
    Spec { name: "set_preset", args: "<0-127>", aliases: &["preset"] },
    Spec { name: "set_drum_kit", args: "[kit 0-127]", aliases: &["drums", "kit"] },
    Spec { name: "open_instrument_picker", args: "", aliases: &["instruments"] },
    Spec { name: "set_master_reverb", args: "<0-100%>", aliases: &["reverb"] },
    Spec { name: "show_lane", args: "<sustain|bend|mod|volume|pan|off>", aliases: &["lane"] },
    Spec { name: "cycle_lane", args: "", aliases: &["lane next"] },
    Spec { name: "set_lane_value", args: "<value>", aliases: &["lane_value", "lane value", "lv"] },
    Spec { name: "nudge_lane_value", args: "<delta>", aliases: &["lane nudge"] },
    Spec { name: "ramp_lane", args: "<from> <to>", aliases: &["lane ramp"] },
    Spec { name: "clear_lane", args: "", aliases: &["lane clear"] },
    Spec { name: "sustain_down", args: "", aliases: &["pedal down"] },
    Spec { name: "sustain_up", args: "", aliases: &["pedal up"] },
    Spec { name: "pedal_selection", args: "", aliases: &["pedal"] },
    Spec { name: "play", args: "", aliases: &[] },
    Spec { name: "stop", args: "", aliases: &[] },
    Spec { name: "toggle_play", args: "", aliases: &[] },
    Spec { name: "playhead_to_start", args: "", aliases: &[] },
    Spec { name: "playhead_to_cursor", args: "", aliases: &[] },
    Spec { name: "cursor_to_playhead", args: "", aliases: &[] },
    Spec { name: "playhead_prev_bar", args: "", aliases: &[] },
    Spec { name: "playhead_next_bar", args: "", aliases: &[] },
    Spec { name: "enter_command_line", args: "", aliases: &[] },
];

fn spec(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

/// The usage line for a command name or alias, e.g.
/// `usage("q")` → `Some("quantize [grid] [strength%] [swing%]")`.
pub fn usage(name: &str) -> Option<String> {
    let name = name.trim();
    let s = spec(name).or_else(|| SPECS.iter().find(|s| s.aliases.contains(&name)))?;
    Some(usage_of(s))
}

fn usage_of(s: &Spec) -> String {
    if s.args.is_empty() {
        s.name.to_string()
    } else {
        format!("{} {}", s.name, s.args)
    }
}

/// Command names and aliases starting with `prefix` (trimmed, case-insensitive),
/// sorted, for the command line's autocompletion. An empty prefix lists everything.
pub fn complete(prefix: &str) -> Vec<String> {
    let prefix = prefix.trim_start().to_ascii_lowercase();
    let mut out: Vec<String> = SPECS
        .iter()
        .flat_map(|s| std::iter::once(s.name).chain(s.aliases.iter().copied()))
        .filter(|candidate| candidate.starts_with(&prefix))
        .map(str::to_string)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Splits a line into the canonical command name and its arguments, resolving
/// one- and two-word aliases (`sel bar`, `keep every 3`).
fn resolve<'a>(words: &'a [&'a str]) -> Result<(&'static Spec, &'a [&'a str]), CmdLineError> {
    let head = words.first().ok_or(CmdLineError::Empty)?.to_ascii_lowercase();
    if let Some(s) = spec(&head) {
        return Ok((s, &words[1..]));
    }
    if let Some(second) = words.get(1) {
        let two = format!("{head} {}", second.to_ascii_lowercase());
        if let Some(s) = SPECS.iter().find(|s| s.aliases.contains(&two.as_str())) {
            return Ok((s, &words[2..]));
        }
    }
    if let Some(s) = SPECS.iter().find(|s| s.aliases.contains(&head.as_str())) {
        return Ok((s, &words[1..]));
    }
    let shown: String = words.iter().take(2).copied().collect::<Vec<_>>().join(" ");
    Err(CmdLineError::UnknownCommand(shown))
}

/// Positional argument access with the command's usage in every error.
struct Args<'a> {
    spec: &'static Spec,
    words: &'a [&'a str],
}

impl<'a> Args<'a> {
    fn required(&self, i: usize, arg: &str) -> Result<&'a str, CmdLineError> {
        self.words.get(i).copied().ok_or_else(|| CmdLineError::MissingArg {
            command: self.spec.name.to_string(),
            arg: arg.to_string(),
            usage: usage_of(self.spec),
        })
    }

    fn optional(&self, i: usize) -> Option<&'a str> {
        self.words.get(i).copied()
    }

    fn bad(&self, arg: &str, value: &str, reason: impl fmt::Display) -> CmdLineError {
        CmdLineError::BadArg {
            command: self.spec.name.to_string(),
            arg: arg.to_string(),
            value: value.to_string(),
            reason: reason.to_string(),
        }
    }

    fn at_most(&self, n: usize) -> Result<(), CmdLineError> {
        if self.words.len() > n {
            return Err(CmdLineError::TooManyArgs { command: self.spec.name.to_string(), usage: usage_of(self.spec) });
        }
        Ok(())
    }

    fn number<T: std::str::FromStr>(&self, i: usize, arg: &str, what: &str) -> Result<T, CmdLineError> {
        let s = self.required(i, arg)?;
        s.trim_end_matches('%').parse().map_err(|_| self.bad(arg, s, what))
    }

    fn number_or<T: std::str::FromStr>(&self, i: usize, arg: &str, what: &str, default: T) -> Result<T, CmdLineError> {
        match self.optional(i) {
            Some(_) => self.number(i, arg, what),
            None => Ok(default),
        }
    }

    fn velocity(&self, i: usize, arg: &str) -> Result<u8, CmdLineError> {
        let v: u8 = self.number(i, arg, "expected a velocity 1-127")?;
        if (1..=127).contains(&v) {
            Ok(v)
        } else {
            Err(self.bad(arg, self.required(i, arg)?, "expected a velocity 1-127"))
        }
    }

    fn percent(&self, i: usize, arg: &str, default: u8) -> Result<u8, CmdLineError> {
        let v: u8 = self.number_or(i, arg, "expected a percentage 0-100", default)?;
        if v <= 100 {
            Ok(v)
        } else {
            Err(self.bad(arg, self.required(i, arg)?, "expected a percentage 0-100"))
        }
    }

    fn grid(&self, i: usize, arg: &str) -> Result<Grid, CmdLineError> {
        let s = self.required(i, arg)?;
        s.parse().map_err(|e| self.bad(arg, s, e))
    }

    fn one_of<T: Copy>(&self, i: usize, arg: &str, table: &[(&str, T)]) -> Result<T, CmdLineError> {
        let s = self.required(i, arg)?;
        let lower = s.to_ascii_lowercase();
        table.iter().find(|(k, _)| *k == lower).map(|(_, v)| *v).ok_or_else(|| {
            let options: Vec<&str> = table.iter().map(|(k, _)| *k).collect();
            self.bad(arg, s, format!("expected one of {}", options.join(", ")))
        })
    }

    /// A number that must lie in `lo..=hi`.
    fn bounded<T: std::str::FromStr + PartialOrd + Copy>(
        &self,
        i: usize,
        arg: &str,
        lo: T,
        hi: T,
        what: &str,
    ) -> Result<T, CmdLineError> {
        let v: T = self.number(i, arg, what)?;
        if v >= lo && v <= hi {
            Ok(v)
        } else {
            Err(self.bad(arg, self.required(i, arg)?, what))
        }
    }

    /// A pan position: `-100..100`, or `L50` / `C` / `R50` (case-insensitive).
    fn pan(&self, i: usize, arg: &str) -> Result<i8, CmdLineError> {
        const WHAT: &str = "expected a pan -100..100, or L50 / C / R50";
        let s = self.required(i, arg)?;
        let upper = s.to_ascii_uppercase();
        let (sign, digits) = match upper.as_str() {
            "C" | "CENTER" | "CENTRE" => return Ok(0),
            u => match u.strip_prefix('L') {
                Some(d) => (-1, d),
                None => match u.strip_prefix('R') {
                    Some(d) => (1, d),
                    None => (1, u),
                },
            },
        };
        let magnitude: i32 = digits.parse().map_err(|_| self.bad(arg, s, WHAT))?;
        let value = magnitude * sign;
        if (-100..=100).contains(&value) {
            Ok(value as i8)
        } else {
            Err(self.bad(arg, s, WHAT))
        }
    }

    /// `sustain|bend|mod|volume|pan` → `Some`, `off|none|hide` → `None`.
    fn lane(&self, i: usize, arg: &str) -> Result<Option<LaneKind>, CmdLineError> {
        let s = self.required(i, arg)?;
        match s.to_ascii_lowercase().as_str() {
            "off" | "none" | "hide" | "hidden" => Ok(None),
            _ => s.parse::<LaneKind>().map(Some).map_err(|_| {
                self.bad(arg, s, "expected one of sustain, bend, mod, volume, pan, off")
            }),
        }
    }

    /// `<path|default> [preset] [bank]`: up to two trailing numbers are the preset and
    /// bank, everything before them (joined with spaces) is the path.
    fn instrument(&self) -> Result<Command, CmdLineError> {
        self.required(0, "path")?;
        let mut end = self.words.len();
        let mut numbers: Vec<u16> = Vec::new();
        while end > 1 && numbers.len() < 2 {
            match self.words[end - 1].parse::<u16>() {
                Ok(n) => {
                    numbers.insert(0, n);
                    end -= 1;
                }
                Err(_) => break,
            }
        }
        let path_words = &self.words[..end];
        let path = path_words.join(" ");
        let path = if path.eq_ignore_ascii_case("default") { None } else { Some(PathBuf::from(path)) };
        let preset = numbers.first().copied().unwrap_or(0);
        if preset > 127 {
            return Err(self.bad("preset", &preset.to_string(), "expected a preset 0-127"));
        }
        let bank = numbers.get(1).copied().unwrap_or(0);
        Ok(Command::SetInstrument { path, bank, preset })
    }

    /// `0,4,7` or `0 4 7` (all remaining words).
    fn intervals(&self, from: usize, arg: &str) -> Result<Vec<i8>, CmdLineError> {
        if self.words.len() <= from {
            return Err(CmdLineError::MissingArg {
                command: self.spec.name.to_string(),
                arg: arg.to_string(),
                usage: usage_of(self.spec),
            });
        }
        let mut out = Vec::new();
        for word in &self.words[from..] {
            for part in word.split(',').filter(|p| !p.is_empty()) {
                let v: i8 = part.parse().map_err(|_| self.bad(arg, part, "expected a semitone offset -128..127"))?;
                out.push(v);
            }
        }
        Ok(out)
    }
}

const DIRS: &[(&str, Dir)] = &[("left", Dir::Left), ("right", Dir::Right), ("up", Dir::Up), ("down", Dir::Down)];
const UNITS: &[(&str, Unit)] = &[
    ("grid", Unit::Grid),
    ("beat", Unit::Beat),
    ("bar", Unit::Bar),
    ("semitone", Unit::Semitone),
    ("octave", Unit::Octave),
];
const CYCLE: &[(&str, CycleDir)] = &[("next", CycleDir::Next), ("prev", CycleDir::Prev)];

/// Parses one command line (without the leading `:`).
pub fn parse(line: &str) -> Result<Command, CmdLineError> {
    let words: Vec<&str> = line.trim().trim_start_matches(':').split_whitespace().collect();
    if words.is_empty() {
        return Err(CmdLineError::Empty);
    }
    let (spec, rest) = resolve(&words)?;
    let a = Args { spec, words: rest };
    let cmd = match spec.name {
        "move_cursor" => {
            a.at_most(2)?;
            Command::MoveCursor { dir: a.one_of(0, "direction", DIRS)?, unit: a.one_of(1, "unit", UNITS)? }
        }
        "cursor_to_start" => Command::CursorToStart,
        "extend_selection" => {
            a.at_most(2)?;
            Command::ExtendSelection { dir: a.one_of(0, "direction", DIRS)?, unit: a.one_of(1, "unit", UNITS)? }
        }
        "insert_note" => Command::InsertNote,
        "insert_chord" => Command::InsertChord { intervals: a.intervals(0, "intervals")? },
        "delete_selected" => Command::DeleteSelected,
        "move_selected" => {
            a.at_most(2)?;
            Command::MoveSelected { dir: a.one_of(0, "direction", DIRS)?, unit: a.one_of(1, "unit", UNITS)? }
        }
        "nudge_selected" => {
            a.at_most(1)?;
            Command::NudgeSelected { ticks: a.number(0, "ticks", "expected a number of ticks")? }
        }
        "change_length" => {
            a.at_most(1)?;
            Command::ChangeLength { delta_steps: a.number(0, "delta steps", "expected a number of grid steps")? }
        }
        "set_length" => {
            a.at_most(1)?;
            Command::SetLength { steps: a.number(0, "steps", "expected a number of grid steps")? }
        }
        "legato" => Command::Legato,
        "nudge_velocity" => {
            a.at_most(1)?;
            Command::NudgeVelocity { delta: a.number(0, "delta", "expected a velocity change")? }
        }
        "set_velocity" => {
            a.at_most(1)?;
            let s = a.required(0, "velocity")?;
            match s.split_once("..") {
                Some((from, to)) => {
                    let ramp = Args { spec, words: &[from, to] };
                    Command::RampVelocity { from: ramp.velocity(0, "from")?, to: ramp.velocity(1, "to")? }
                }
                None => Command::SetVelocity { value: a.velocity(0, "velocity")? },
            }
        }
        "ramp_velocity" => {
            a.at_most(2)?;
            let first = a.required(0, "from")?;
            match first.split_once("..") {
                Some((from, to)) => {
                    let ramp = Args { spec, words: &[from, to] };
                    Command::RampVelocity { from: ramp.velocity(0, "from")?, to: ramp.velocity(1, "to")? }
                }
                None => Command::RampVelocity { from: a.velocity(0, "from")?, to: a.velocity(1, "to")? },
            }
        }
        "humanize" => {
            a.at_most(2)?;
            Command::Humanize {
                velocity: a.number_or(0, "velocity", "expected a velocity amount 0-127", 8)?,
                timing: a.number_or(1, "timing", "expected a number of ticks", 10)?,
            }
        }
        "quantize" => {
            // the grid is optional and recognisable by its slash, so `q 50%` is a strength
            let (grid, at) = match a.optional(0) {
                Some(s) if s.contains('/') => (Some(a.grid(0, "grid")?), 1),
                _ => (None, 0),
            };
            a.at_most(at + 2)?;
            Command::Quantize { grid, strength: a.percent(at, "strength", 100)?, swing: a.percent(at + 1, "swing", 0)? }
        }
        "quantize_length" => Command::QuantizeLength,
        "set_swing" => {
            a.at_most(1)?;
            a.required(0, "percent")?;
            Command::SetSwing { percent: a.percent(0, "percent", 0)? }
        }
        "transpose_selected" => {
            a.at_most(1)?;
            Command::TransposeSelected { semitones: a.number(0, "semitones", "expected a number of semitones")? }
        }
        "undo" => Command::Undo,
        "redo" => Command::Redo,
        "toggle_select_under_cursor" => Command::ToggleSelectUnderCursor,
        "select_all" => Command::SelectAll,
        "clear_selection" => Command::ClearSelection,
        "select_bar" => Command::SelectBar,
        "select_chord" => Command::SelectChord,
        "select_next_notes" => {
            a.at_most(1)?;
            Command::SelectNextNotes { n: a.number_or(0, "n", "expected a number of notes", 4)? }
        }
        "select_to_end" => Command::SelectToEnd,
        "select_to_start" => Command::SelectToStart,
        "select_same_pitch" => Command::SelectSamePitch,
        "select_velocity_above" => {
            a.at_most(1)?;
            Command::SelectVelocityAbove { value: a.velocity(0, "velocity")? }
        }
        "select_velocity_below" => {
            a.at_most(1)?;
            Command::SelectVelocityBelow { value: a.velocity(0, "velocity")? }
        }
        "select_top_of_chords" => Command::SelectTopOfChords,
        "select_bottom_of_chords" => Command::SelectBottomOfChords,
        "invert_selection" => Command::InvertSelection,
        "selection_focus_next" => Command::SelectionFocusNext,
        "selection_focus_prev" => Command::SelectionFocusPrev,
        "drop_focused" => Command::DropFocused,
        "keep_top_of_chords" => Command::KeepTopOfChords,
        "keep_bottom_of_chords" => Command::KeepBottomOfChords,
        "keep_every_other" => {
            a.at_most(1)?;
            Command::KeepEveryOther { n: a.number_or(0, "n", "expected a number", 2)? }
        }
        "keep_on_beat" => Command::KeepOnBeat,
        "keep_off_beat" => Command::KeepOffBeat,
        "keep_above_cursor" => Command::KeepAboveCursor,
        "keep_below_cursor" => Command::KeepBelowCursor,
        "keep_after_cursor" => Command::KeepAfterCursor,
        "keep_before_cursor" => Command::KeepBeforeCursor,
        "copy" => Command::Copy,
        "cut" => Command::Cut,
        "paste" => Command::Paste,
        "duplicate" => Command::Duplicate,
        "set_grid" => {
            a.at_most(1)?;
            Command::SetGrid { grid: a.grid(0, "grid")? }
        }
        "cycle_grid" => {
            a.at_most(1)?;
            Command::CycleGrid { dir: a.one_of(0, "direction", CYCLE)? }
        }
        "next_track" => Command::NextTrack,
        "prev_track" => Command::PrevTrack,
        "zoom_in" => Command::ZoomIn,
        "zoom_out" => Command::ZoomOut,
        "zoom_rows_in" => Command::ZoomRowsIn,
        "zoom_rows_out" => Command::ZoomRowsOut,
        "toggle_piano_roll" => Command::TogglePianoRoll,
        "zoom_to_fit" => Command::ZoomToFit,
        "toggle_cheat_sheet" => Command::ToggleCheatSheet,
        "add_track" => Command::AddTrack,
        "remove_track" => Command::RemoveTrack,
        "duplicate_track" => Command::DuplicateTrack,
        "move_track_up" => Command::MoveTrackUp,
        "move_track_down" => Command::MoveTrackDown,
        "rename_track" => {
            a.required(0, "name")?;
            Command::RenameTrack { name: a.words.join(" ") }
        }
        "set_track_volume" => {
            a.at_most(1)?;
            a.required(0, "percent")?;
            Command::SetTrackVolume { percent: a.bounded(0, "percent", 0, 200, "expected a percentage 0-200")? }
        }
        "nudge_track_volume" => {
            a.at_most(1)?;
            Command::NudgeTrackVolume { delta: a.number(0, "delta", "expected a percentage change -128..127")? }
        }
        "set_track_pan" => {
            a.at_most(1)?;
            Command::SetTrackPan { value: a.pan(0, "pan")? }
        }
        "nudge_track_pan" => {
            a.at_most(1)?;
            Command::NudgeTrackPan { delta: a.number(0, "delta", "expected a pan change -128..127")? }
        }
        "toggle_mute" => Command::ToggleMute,
        "toggle_solo" => Command::ToggleSolo,
        "set_track_channel" => {
            a.at_most(1)?;
            let one_based: u8 = a.bounded(0, "channel", 1, 16, "expected a channel 1-16")?;
            Command::SetTrackChannel { channel: one_based - 1 }
        }
        "set_instrument" => a.instrument()?,
        "next_preset" => Command::NextPreset,
        "prev_preset" => Command::PrevPreset,
        "set_preset" => {
            a.at_most(1)?;
            a.required(0, "preset")?;
            Command::SetPreset { preset: a.bounded(0, "preset", 0, 127, "expected a program 0-127")? }
        }
        "set_drum_kit" => {
            a.at_most(1)?;
            let kit = match a.optional(0) {
                None => 0,
                Some(_) => a.bounded(0, "kit", 0, 127, "expected a kit 0-127")?,
            };
            Command::SetDrumKit { kit }
        }
        "open_instrument_picker" => Command::OpenInstrumentPicker,
        "edit_track_name" => Command::EditTrackName,
        "set_master_reverb" => {
            a.at_most(1)?;
            a.required(0, "percent")?;
            Command::SetMasterReverb { percent: a.percent(0, "percent", 0)? }
        }
        "show_lane" => {
            a.at_most(1)?;
            Command::ShowLane { kind: a.lane(0, "lane")? }
        }
        "cycle_lane" => Command::CycleLane,
        "set_lane_value" => {
            a.at_most(1)?;
            Command::SetLaneValue { value: a.number(0, "value", "expected a lane value")? }
        }
        "nudge_lane_value" => {
            a.at_most(1)?;
            Command::NudgeLaneValue { delta: a.number(0, "delta", "expected a lane value change")? }
        }
        "ramp_lane" => {
            a.at_most(2)?;
            let first = a.required(0, "from")?;
            match first.split_once("..") {
                Some((from, to)) => {
                    let ramp = Args { spec, words: &[from, to] };
                    Command::RampLane {
                        from: ramp.number(0, "from", "expected a lane value")?,
                        to: ramp.number(1, "to", "expected a lane value")?,
                    }
                }
                None => Command::RampLane {
                    from: a.number(0, "from", "expected a lane value")?,
                    to: a.number(1, "to", "expected a lane value")?,
                },
            }
        }
        "clear_lane" => Command::ClearLane,
        "sustain_down" => Command::SustainDown,
        "sustain_up" => Command::SustainUp,
        "pedal_selection" => Command::PedalSelection,
        "play" => Command::Play,
        "stop" => Command::Stop,
        "toggle_play" => Command::TogglePlay,
        "playhead_to_start" => Command::PlayheadToStart,
        "playhead_to_cursor" => Command::PlayheadToCursor,
        "cursor_to_playhead" => Command::CursorToPlayhead,
        "playhead_prev_bar" => Command::PlayheadPrevBar,
        "playhead_next_bar" => Command::PlayheadNextBar,
        "enter_command_line" => Command::EnterCommandLine,
        other => return Err(CmdLineError::UnknownCommand(other.to_string())),
    };
    if spec.args.is_empty() {
        a.at_most(0)?;
    }
    Ok(cmd)
}

/// The command-line spelling of a command: the inverse of [`parse`]
/// (`parse(&to_line(&c)) == Ok(c)` for every command).
pub fn to_line(cmd: &Command) -> String {
    let name = cmd.name();
    let args: Vec<String> = match cmd {
        Command::MoveCursor { dir, unit } | Command::ExtendSelection { dir, unit } | Command::MoveSelected { dir, unit } => {
            vec![dir.to_string(), unit_name(*unit).to_string()]
        }
        Command::InsertChord { intervals } => {
            vec![intervals.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")]
        }
        Command::NudgeSelected { ticks } => vec![ticks.to_string()],
        Command::ChangeLength { delta_steps } => vec![delta_steps.to_string()],
        Command::SetLength { steps } => vec![steps.to_string()],
        Command::NudgeVelocity { delta } => vec![delta.to_string()],
        Command::SetVelocity { value } => vec![value.to_string()],
        Command::RampVelocity { from, to } => vec![from.to_string(), to.to_string()],
        Command::Humanize { velocity, timing } => vec![velocity.to_string(), timing.to_string()],
        Command::Quantize { grid, strength, swing } => {
            let mut v: Vec<String> = grid.iter().map(ToString::to_string).collect();
            v.push(format!("{strength}%"));
            v.push(format!("{swing}%"));
            v
        }
        Command::SetSwing { percent } => vec![format!("{percent}%")],
        Command::TransposeSelected { semitones } => vec![semitones.to_string()],
        Command::SelectNextNotes { n } | Command::KeepEveryOther { n } => vec![n.to_string()],
        Command::SelectVelocityAbove { value } | Command::SelectVelocityBelow { value } => vec![value.to_string()],
        Command::SetGrid { grid } => vec![grid.to_string()],
        Command::CycleGrid { dir: CycleDir::Next } => vec!["next".to_string()],
        Command::CycleGrid { dir: CycleDir::Prev } => vec!["prev".to_string()],
        Command::RenameTrack { name } => vec![name.clone()],
        Command::SetTrackVolume { percent } | Command::SetMasterReverb { percent } => vec![format!("{percent}%")],
        Command::NudgeTrackVolume { delta } | Command::NudgeTrackPan { delta } => vec![delta.to_string()],
        Command::SetTrackPan { value } => vec![value.to_string()],
        Command::SetTrackChannel { channel } => vec![(u32::from(*channel) + 1).to_string()],
        Command::SetPreset { preset } => vec![preset.to_string()],
        Command::SetDrumKit { kit } => vec![kit.to_string()],
        Command::SetInstrument { path, bank, preset } => {
            let path = match path {
                Some(p) if !p.as_os_str().is_empty() => p.display().to_string(),
                _ => "default".to_string(),
            };
            vec![path, preset.to_string(), bank.to_string()]
        }
        Command::ShowLane { kind } => vec![kind.map(LaneKind::name).unwrap_or("off").to_string()],
        Command::SetLaneValue { value } => vec![value.to_string()],
        Command::NudgeLaneValue { delta } => vec![delta.to_string()],
        Command::RampLane { from, to } => vec![from.to_string(), to.to_string()],
        Command::CursorToStart
        | Command::InsertNote
        | Command::DeleteSelected
        | Command::Legato
        | Command::QuantizeLength
        | Command::Undo
        | Command::Redo
        | Command::ToggleSelectUnderCursor
        | Command::SelectAll
        | Command::ClearSelection
        | Command::SelectBar
        | Command::SelectChord
        | Command::SelectToEnd
        | Command::SelectToStart
        | Command::SelectSamePitch
        | Command::SelectTopOfChords
        | Command::SelectBottomOfChords
        | Command::InvertSelection
        | Command::SelectionFocusNext
        | Command::SelectionFocusPrev
        | Command::DropFocused
        | Command::KeepTopOfChords
        | Command::KeepBottomOfChords
        | Command::KeepOnBeat
        | Command::KeepOffBeat
        | Command::KeepAboveCursor
        | Command::KeepBelowCursor
        | Command::KeepAfterCursor
        | Command::KeepBeforeCursor
        | Command::Copy
        | Command::Cut
        | Command::Paste
        | Command::Duplicate
        | Command::NextTrack
        | Command::PrevTrack
        | Command::ZoomIn
        | Command::ZoomOut
        | Command::ZoomRowsIn
        | Command::ZoomRowsOut
        | Command::TogglePianoRoll
        | Command::ZoomToFit
        | Command::ToggleCheatSheet
        | Command::OpenInstrumentPicker
        | Command::EditTrackName
        | Command::AddTrack
        | Command::RemoveTrack
        | Command::DuplicateTrack
        | Command::MoveTrackUp
        | Command::MoveTrackDown
        | Command::ToggleMute
        | Command::ToggleSolo
        | Command::NextPreset
        | Command::PrevPreset
        | Command::CycleLane
        | Command::ClearLane
        | Command::SustainDown
        | Command::SustainUp
        | Command::PedalSelection
        | Command::Play
        | Command::Stop
        | Command::TogglePlay
        | Command::PlayheadToStart
        | Command::PlayheadToCursor
        | Command::CursorToPlayhead
        | Command::PlayheadPrevBar
        | Command::PlayheadNextBar
        | Command::EnterCommandLine => Vec::new(),
    };
    if args.is_empty() {
        name.to_string()
    } else {
        format!("{name} {}", args.join(" "))
    }
}

fn unit_name(unit: Unit) -> &'static str {
    match unit {
        Unit::Grid => "grid",
        Unit::Beat => "beat",
        Unit::Bar => "bar",
        Unit::Semitone => "semitone",
        Unit::Octave => "octave",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{Division, GridModifier};

    #[test]
    fn specs_match_the_enum_exactly() {
        let mut from_enum = Command::all_names();
        let mut from_specs: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
        from_enum.sort_unstable();
        from_specs.sort_unstable();
        assert_eq!(from_specs, from_enum, "cmdline SPECS must list every Command variant exactly once");
        // no alias collides with a name or another alias
        let mut all = complete("");
        let n = all.len();
        all.dedup();
        assert_eq!(all.len(), n);
        for s in SPECS {
            for alias in s.aliases {
                assert!(spec(alias).is_none(), "alias {alias} shadows a command name");
            }
        }
    }

    #[test]
    fn every_variant_round_trips_through_the_command_line() {
        let mut all = Command::examples();
        all.extend([
            Command::MoveCursor { dir: Dir::Down, unit: Unit::Octave },
            Command::MoveSelected { dir: Dir::Left, unit: Unit::Bar },
            Command::InsertChord { intervals: vec![-12, 0, 7, 12] },
            Command::NudgeSelected { ticks: -10 },
            Command::ChangeLength { delta_steps: -2 },
            Command::Quantize { grid: Some(Grid { base: Division::Eighth, modifier: GridModifier::Triplet }), strength: 50, swing: 66 },
            Command::ExtendSelection { dir: Dir::Up, unit: Unit::Semitone },
            Command::CycleGrid { dir: CycleDir::Prev },
            Command::SetSwing { percent: 55 },
            Command::TransposeSelected { semitones: -7 },
            Command::RenameTrack { name: "My Bass Line".to_string() },
            Command::SetTrackVolume { percent: 200 },
            Command::NudgeTrackVolume { delta: -5 },
            Command::SetTrackPan { value: -100 },
            Command::NudgeTrackPan { delta: -10 },
            Command::SetTrackChannel { channel: 15 },
            Command::SetInstrument { path: Some("my sounds/gm 2.sf2".into()), bank: 3, preset: 127 },
            Command::SetMasterReverb { percent: 0 },
            Command::ShowLane { kind: None },
            Command::ShowLane { kind: Some(LaneKind::PitchBend) },
            Command::SetLaneValue { value: -8192 },
            Command::NudgeLaneValue { delta: -512 },
            Command::RampLane { from: 127, to: 0 },
        ]);
        for c in all {
            let line = to_line(&c);
            assert_eq!(parse(&line), Ok(c.clone()), "{line}");
            assert!(usage(c.name()).is_some(), "{}", c.name());
            assert!(complete(c.name()).contains(&c.name().to_string()));
        }
    }

    #[test]
    fn parses_aliases_and_argument_types() {
        assert_eq!(parse("q"), Ok(Command::Quantize { grid: None, strength: 100, swing: 0 }));
        assert_eq!(
            parse(":quantize 1/8T 50% 20"),
            Ok(Command::Quantize {
                grid: Some(Grid { base: Division::Eighth, modifier: GridModifier::Triplet }),
                strength: 50,
                swing: 20
            })
        );
        assert_eq!(parse("  Q 1/16  "), Ok(Command::Quantize { grid: Some(Grid::default()), strength: 100, swing: 0 }));
        assert_eq!(parse("q 50%"), Ok(Command::Quantize { grid: None, strength: 50, swing: 0 }));
        assert_eq!(parse("q 100 66"), Ok(Command::Quantize { grid: None, strength: 100, swing: 66 }));
        assert_eq!(parse("vel 80"), Ok(Command::SetVelocity { value: 80 }));
        assert_eq!(parse("vel 80..110"), Ok(Command::RampVelocity { from: 80, to: 110 }));
        assert_eq!(parse("ramp 110..40"), Ok(Command::RampVelocity { from: 110, to: 40 }));
        assert_eq!(parse("ramp_velocity 40 90"), Ok(Command::RampVelocity { from: 40, to: 90 }));
        assert_eq!(parse("t 12"), Ok(Command::TransposeSelected { semitones: 12 }));
        assert_eq!(parse("tr -5"), Ok(Command::TransposeSelected { semitones: -5 }));
        assert_eq!(parse("h"), Ok(Command::Humanize { velocity: 8, timing: 10 }));
        assert_eq!(parse("h 3 25"), Ok(Command::Humanize { velocity: 3, timing: 25 }));
        assert_eq!(parse("sel bar"), Ok(Command::SelectBar));
        assert_eq!(parse("roll"), Ok(Command::TogglePianoRoll));
        assert_eq!(parse("fit"), Ok(Command::ZoomToFit));
        assert_eq!(parse(&to_line(&Command::TogglePianoRoll)), Ok(Command::TogglePianoRoll));
        assert_eq!(parse(&to_line(&Command::ZoomToFit)), Ok(Command::ZoomToFit));
        assert_eq!(parse("sel next"), Ok(Command::SelectNextNotes { n: 4 }));
        assert_eq!(parse("sel next 8"), Ok(Command::SelectNextNotes { n: 8 }));
        assert_eq!(parse("sel above 100"), Ok(Command::SelectVelocityAbove { value: 100 }));
        assert_eq!(parse("sel none"), Ok(Command::ClearSelection));
        assert_eq!(parse("keep every 3"), Ok(Command::KeepEveryOther { n: 3 }));
        assert_eq!(parse("keep on_beat"), Ok(Command::KeepOnBeat));
        assert_eq!(parse("chord 0,4,7"), Ok(Command::InsertChord { intervals: vec![0, 4, 7] }));
        assert_eq!(parse("insert_chord 0 3 7 10"), Ok(Command::InsertChord { intervals: vec![0, 3, 7, 10] }));
        assert_eq!(parse("chord -12,0"), Ok(Command::InsertChord { intervals: vec![-12, 0] }));
        assert_eq!(parse("grid 1/4."), Ok(Command::SetGrid { grid: Grid { base: Division::Quarter, modifier: GridModifier::Dotted } }));
        assert_eq!(parse("move_cursor Right BAR"), Ok(Command::MoveCursor { dir: Dir::Right, unit: Unit::Bar }));
        assert_eq!(parse("cut"), Ok(Command::Cut));
        assert_eq!(parse("swing 60%"), Ok(Command::SetSwing { percent: 60 }));
        assert_eq!(parse("nudge -15"), Ok(Command::NudgeSelected { ticks: -15 }));
        assert_eq!(parse("len 4"), Ok(Command::SetLength { steps: 4 }));
        assert_eq!(parse("help"), Ok(Command::ToggleCheatSheet));
        assert_eq!(parse("u"), Ok(Command::Undo));
        assert_eq!(parse("focus next"), Ok(Command::SelectionFocusNext));
        // tracks, mixer, lanes
        assert_eq!(parse("track add"), Ok(Command::AddTrack));
        assert_eq!(parse("track rm"), Ok(Command::RemoveTrack));
        assert_eq!(parse("track dup"), Ok(Command::DuplicateTrack));
        assert_eq!(parse("track up"), Ok(Command::MoveTrackUp));
        assert_eq!(parse("rename My Bass"), Ok(Command::RenameTrack { name: "My Bass".into() }));
        assert_eq!(parse("track rename Lead"), Ok(Command::RenameTrack { name: "Lead".into() }));
        assert_eq!(parse("vol 80"), Ok(Command::SetTrackVolume { percent: 80 }));
        assert_eq!(parse("volume 150%"), Ok(Command::SetTrackVolume { percent: 150 }));
        assert_eq!(parse("nudge_track_volume -5"), Ok(Command::NudgeTrackVolume { delta: -5 }));
        assert_eq!(parse("pan L30"), Ok(Command::SetTrackPan { value: -30 }));
        assert_eq!(parse("pan r5"), Ok(Command::SetTrackPan { value: 5 }));
        assert_eq!(parse("pan C"), Ok(Command::SetTrackPan { value: 0 }));
        assert_eq!(parse("track pan 40"), Ok(Command::SetTrackPan { value: 40 }));
        assert_eq!(parse("mute"), Ok(Command::ToggleMute));
        assert_eq!(parse("solo"), Ok(Command::ToggleSolo));
        assert_eq!(parse("channel 10"), Ok(Command::SetTrackChannel { channel: 9 }));
        assert_eq!(parse("ch 16"), Ok(Command::SetTrackChannel { channel: 15 }));
        assert_eq!(parse("sf2 default"), Ok(Command::SetInstrument { path: None, bank: 0, preset: 0 }));
        assert_eq!(parse("sf2 gm.sf2 33 1"), Ok(Command::SetInstrument { path: Some("gm.sf2".into()), bank: 1, preset: 33 }));
        assert_eq!(parse("preset next"), Ok(Command::NextPreset));
        assert_eq!(parse("preset prev"), Ok(Command::PrevPreset));
        assert_eq!(parse("preset 34"), Ok(Command::SetPreset { preset: 34 }));
        assert_eq!(parse("drums"), Ok(Command::SetDrumKit { kit: 0 }));
        assert_eq!(parse("drums 8"), Ok(Command::SetDrumKit { kit: 8 }));
        assert_eq!(parse("kit 25"), Ok(Command::SetDrumKit { kit: 25 }));
        assert_eq!(parse("instruments"), Ok(Command::OpenInstrumentPicker));
        assert!(matches!(parse("preset 128"), Err(CmdLineError::BadArg { .. })));
        assert_eq!(parse("reverb 30"), Ok(Command::SetMasterReverb { percent: 30 }));
        assert_eq!(parse("lane sustain"), Ok(Command::ShowLane { kind: Some(LaneKind::Sustain) }));
        assert_eq!(parse("lane Bend"), Ok(Command::ShowLane { kind: Some(LaneKind::PitchBend) }));
        assert_eq!(parse("lane pan"), Ok(Command::ShowLane { kind: Some(LaneKind::Pan) }));
        assert_eq!(parse("lane none"), Ok(Command::ShowLane { kind: None }));
        assert_eq!(parse("lane next"), Ok(Command::CycleLane));
        assert_eq!(parse("lane_value 90"), Ok(Command::SetLaneValue { value: 90 }));
        assert_eq!(parse("lane value -8192"), Ok(Command::SetLaneValue { value: -8192 }));
        assert_eq!(parse("lv 5"), Ok(Command::SetLaneValue { value: 5 }));
        assert_eq!(parse("lane nudge -512"), Ok(Command::NudgeLaneValue { delta: -512 }));
        assert_eq!(parse("ramp_lane 0 127"), Ok(Command::RampLane { from: 0, to: 127 }));
        assert_eq!(parse("lane ramp 127..0"), Ok(Command::RampLane { from: 127, to: 0 }));
        assert_eq!(parse("lane clear"), Ok(Command::ClearLane));
        assert_eq!(parse("pedal down"), Ok(Command::SustainDown));
        assert_eq!(parse("pedal up"), Ok(Command::SustainUp));
        assert_eq!(parse("pedal"), Ok(Command::PedalSelection));
    }

    #[test]
    fn errors_are_specific() {
        assert_eq!(parse(""), Err(CmdLineError::Empty));
        assert_eq!(parse("   :  "), Err(CmdLineError::Empty));
        assert_eq!(parse("fly away"), Err(CmdLineError::UnknownCommand("fly away".into())));
        assert_eq!(parse("sel nowhere"), Err(CmdLineError::UnknownCommand("sel nowhere".into())));
        assert!(matches!(parse("vel"), Err(CmdLineError::MissingArg { ref arg, .. }) if arg == "velocity"));
        assert!(matches!(parse("vel 0"), Err(CmdLineError::BadArg { .. })));
        assert!(matches!(parse("vel 200"), Err(CmdLineError::BadArg { .. })));
        assert!(matches!(parse("vel 10..300"), Err(CmdLineError::BadArg { ref arg, .. }) if arg == "to"));
        assert!(matches!(parse("q 1/3"), Err(CmdLineError::BadArg { ref arg, .. }) if arg == "grid"));
        assert!(matches!(parse("q 1/16 150"), Err(CmdLineError::BadArg { ref arg, .. }) if arg == "strength"));
        assert!(matches!(parse("q 1/16 50 10 extra"), Err(CmdLineError::TooManyArgs { .. })));
        assert!(matches!(parse("undo now"), Err(CmdLineError::TooManyArgs { .. })));
        assert!(matches!(parse("move_cursor sideways grid"), Err(CmdLineError::BadArg { ref arg, .. }) if arg == "direction"));
        assert!(matches!(parse("chord"), Err(CmdLineError::MissingArg { .. })));
        assert!(matches!(parse("chord 0,x"), Err(CmdLineError::BadArg { .. })));
        assert!(matches!(parse("swing"), Err(CmdLineError::MissingArg { .. })));
        let err = parse("t").unwrap_err().to_string();
        assert!(err.contains("transpose_selected <semitones>"), "{err}");
        assert!(matches!(parse("rename"), Err(CmdLineError::MissingArg { ref arg, .. }) if arg == "name"));
        assert!(matches!(parse("vol"), Err(CmdLineError::MissingArg { .. })));
        assert!(matches!(parse("vol 201"), Err(CmdLineError::BadArg { ref arg, .. }) if arg == "percent"));
        assert!(matches!(parse("pan 101"), Err(CmdLineError::BadArg { .. })));
        assert!(matches!(parse("pan L"), Err(CmdLineError::BadArg { .. })));
        assert!(matches!(parse("channel 0"), Err(CmdLineError::BadArg { .. })));
        assert!(matches!(parse("channel 17"), Err(CmdLineError::BadArg { .. })));
        assert!(matches!(parse("sf2"), Err(CmdLineError::MissingArg { ref arg, .. }) if arg == "path"));
        assert!(matches!(parse("sf2 gm.sf2 128"), Err(CmdLineError::BadArg { ref arg, .. }) if arg == "preset"));
        assert!(matches!(parse("reverb 101"), Err(CmdLineError::BadArg { .. })));
        assert!(matches!(parse("lane"), Err(CmdLineError::MissingArg { .. })));
        assert!(matches!(parse("lane fader"), Err(CmdLineError::BadArg { ref arg, .. }) if arg == "lane"));
        assert!(matches!(parse("lane sustain now"), Err(CmdLineError::TooManyArgs { .. })));
        assert!(matches!(parse("lane_value x"), Err(CmdLineError::BadArg { .. })));
        assert!(matches!(parse("ramp_lane 0"), Err(CmdLineError::MissingArg { ref arg, .. }) if arg == "to"));
        assert!(matches!(parse("mute now"), Err(CmdLineError::TooManyArgs { .. })));
        assert_eq!(parse("").unwrap_err().to_string(), "type a command, e.g. :quantize 1/16");
    }

    #[test]
    fn completion_and_usage() {
        assert_eq!(complete("qu"), vec!["quantize", "quantize_length"]);
        assert_eq!(complete("Q"), vec!["q", "ql", "quantize", "quantize_length"]);
        let sel = complete("sel");
        assert!(sel.contains(&"sel bar".to_string()));
        assert!(sel.contains(&"select_all".to_string()));
        assert!(sel.iter().all(|s| s.starts_with("sel")));
        assert!(complete("zzz").is_empty());
        assert!(complete("").len() > SPECS.len());
        assert_eq!(usage("quantize").as_deref(), Some("quantize [grid] [strength%] [swing%]"));
        assert_eq!(usage("q").as_deref(), Some("quantize [grid] [strength%] [swing%]"));
        assert_eq!(usage("undo").as_deref(), Some("undo"));
        assert_eq!(usage("sel bar").as_deref(), Some("select_bar"));
        assert_eq!(usage("nope"), None);
        assert_eq!(usage("sf2").as_deref(), Some("set_instrument <path|default> [preset] [bank]"));
        assert_eq!(usage("lane").as_deref(), Some("show_lane <sustain|bend|mod|volume|pan|off>"));
        assert_eq!(usage("pedal").as_deref(), Some("pedal_selection"));
        let lane = complete("lane");
        assert!(lane.contains(&"lane ramp".to_string()));
        assert!(lane.contains(&"lane_value".to_string()));
        assert!(lane.iter().all(|s| s.starts_with("lane")));
    }
}
