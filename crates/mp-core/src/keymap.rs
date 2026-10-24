//! Key bindings: TOML file → ([`Mode`], key sequence) → [`Command`].
//!
//! # File format
//!
//! One `[normal]` table. Keys are key sequences; values are a bare command name or an
//! inline table with arguments:
//!
//! ```toml
//! [normal]
//! "Right"       = { cmd = "move_cursor", dir = "right", unit = "grid" }
//! "Shift+Right" = { cmd = "extend_selection", dir = "right", unit = "grid" }
//! "n"           = "insert_note"
//! "g g"         = "select_all"          # a two-key sequence
//! ```
//!
//! A chord is `[Ctrl+][Alt+][Shift+][Cmd+]Key`. `Key` is a named key (`Up Down Left
//! Right Space Enter Escape Tab Backspace Delete Home End PageUp PageDown`) or one
//! printable character. Uppercase letters are rejected: write `"Shift+n"`, not `"N"`.
//! A space separates the chords of a multi-key sequence. No sequence may
//! be a prefix of another (`"g"` and `"g g"` together), and no chord may be bound twice.
//!
//! # Feeding keys
//!
//! The app calls [`Keymap::feed`] once per keypress and acts on the [`Resolution`]:
//! `Count` digits accumulate into the count prefix, `Pending` means "wait for more keys",
//! `Command` is handed to [`crate::Editor::execute`] with the count, and `Unbound`
//! drops both the pending keys and the count.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use thiserror::Error;

use crate::editor::{Command, Mode};

/// A physical key, without modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// A printable, non-whitespace character as it appears unshifted (`'n'`, `'['`, `'='`).
    Char(char),
    Up,
    Down,
    Left,
    Right,
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
}

impl Key {
    const NAMED: [(&'static str, Key); 14] = [
        ("Up", Key::Up),
        ("Down", Key::Down),
        ("Left", Key::Left),
        ("Right", Key::Right),
        ("Space", Key::Space),
        ("Enter", Key::Enter),
        ("Escape", Key::Escape),
        ("Tab", Key::Tab),
        ("Backspace", Key::Backspace),
        ("Delete", Key::Delete),
        ("Home", Key::Home),
        ("End", Key::End),
        ("PageUp", Key::PageUp),
        ("PageDown", Key::PageDown),
    ];

    /// The digit value if this is an unmodified `0`..`9` character key.
    pub fn digit(self) -> Option<u8> {
        match self {
            Key::Char(c) => c.to_digit(10).and_then(|d| u8::try_from(d).ok()),
            _ => None,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Char(c) => write!(f, "{c}"),
            other => {
                let name = Key::NAMED.iter().find(|(_, k)| k == other).map(|(n, _)| *n).unwrap_or("?");
                f.write_str(name)
            }
        }
    }
}

/// Why a key or chord string could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyParseError {
    #[error("empty key")]
    Empty,
    #[error("unknown key {0:?}: use a named key (Up, Enter, ...) or one printable character")]
    UnknownKey(String),
    #[error("uppercase {0:?} is not accepted: write \"Shift+{1}\" instead")]
    Uppercase(String, char),
    #[error("unknown modifier {0:?}: use Shift, Ctrl, Alt or Cmd")]
    UnknownModifier(String),
}

impl FromStr for Key {
    type Err = KeyParseError;

    /// Named keys are matched case-insensitively (`"enter"` works); single characters
    /// must be printable, not whitespace and not uppercase.
    fn from_str(s: &str) -> Result<Self, KeyParseError> {
        if s.is_empty() {
            return Err(KeyParseError::Empty);
        }
        if let Some((_, k)) = Key::NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)) {
            return Ok(*k);
        }
        let mut chars = s.chars();
        let c = chars.next().ok_or(KeyParseError::Empty)?;
        if chars.next().is_some() || c.is_whitespace() || c.is_control() {
            return Err(KeyParseError::UnknownKey(s.to_string()));
        }
        if c.is_uppercase() {
            let lower = c.to_lowercase().next().unwrap_or(c);
            return Err(KeyParseError::Uppercase(s.to_string(), lower));
        }
        Ok(Key::Char(c))
    }
}

/// Modifier keys held with a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub cmd: bool,
}

impl Mods {
    /// No modifiers.
    pub const NONE: Mods = Mods { shift: false, ctrl: false, alt: false, cmd: false };
    /// Shift only.
    pub const SHIFT: Mods = Mods { shift: true, ctrl: false, alt: false, cmd: false };

    /// `true` if no modifier is held.
    pub fn is_none(self) -> bool {
        self == Mods::NONE
    }
}

impl fmt::Display for Mods {
    /// Canonical order `Ctrl+Alt+Shift+Cmd+` (each part only when held).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.cmd {
            f.write_str("Cmd+")?;
        }
        Ok(())
    }
}

/// A key plus modifiers: one keypress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyChord {
    pub key: Key,
    pub mods: Mods,
}

impl KeyChord {
    /// A chord with no modifiers.
    pub const fn plain(key: Key) -> Self {
        KeyChord { key, mods: Mods::NONE }
    }

    /// A chord with Shift held.
    pub const fn shift(key: Key) -> Self {
        KeyChord { key, mods: Mods::SHIFT }
    }

    /// A plain character key.
    pub const fn ch(c: char) -> Self {
        KeyChord::plain(Key::Char(c))
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.mods, self.key)
    }
}

impl FromStr for KeyChord {
    type Err = KeyParseError;

    /// Parses `"Shift+Right"`, `"n"`, `"Ctrl+Shift+="`, `"Shift++"` (the `+` key).
    /// Modifier names are case-insensitive; `Control`, `Opt`/`Option`, `Meta`/`Super`
    /// are accepted aliases.
    fn from_str(s: &str) -> Result<Self, KeyParseError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(KeyParseError::Empty);
        }
        // "+" alone or "...++" means the '+' key itself; otherwise split at the last '+'
        let (mod_part, key_part) = if s == "+" {
            ("", "+")
        } else if let Some(rest) = s.strip_suffix("++") {
            (rest, "+")
        } else {
            match s.rfind('+') {
                Some(i) => (&s[..i], &s[i + 1..]),
                None => ("", s),
            }
        };
        let mut mods = Mods::NONE;
        for m in mod_part.split('+').filter(|m| !m.is_empty()) {
            match m.to_ascii_lowercase().as_str() {
                "shift" => mods.shift = true,
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "opt" | "option" => mods.alt = true,
                "cmd" | "meta" | "super" => mods.cmd = true,
                _ => return Err(KeyParseError::UnknownModifier(m.to_string())),
            }
        }
        let key: Key = key_part.parse()?;
        Ok(KeyChord { key, mods })
    }
}

/// Parses a space-separated key sequence such as `"g g"` or `"v Right Enter"`.
pub fn parse_sequence(s: &str) -> Result<Vec<KeyChord>, KeyParseError> {
    let chords: Vec<KeyChord> = s.split_whitespace().map(str::parse).collect::<Result<_, _>>()?;
    if chords.is_empty() {
        return Err(KeyParseError::Empty);
    }
    Ok(chords)
}

/// Formats a key sequence the way the TOML spells it.
pub fn sequence_to_string(seq: &[KeyChord]) -> String {
    seq.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")
}

/// One binding in a mode table.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub keys: Vec<KeyChord>,
    pub command: Command,
}

/// Problems loading a keymap file.
#[derive(Debug, Error)]
pub enum KeymapError {
    #[error("cannot read keymap {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("keymap is not valid TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("unknown table [{0}]: expected [normal]")]
    UnknownMode(String),
    #[error("[{mode}]: bindings must be a table of key = command")]
    ModeNotATable { mode: String },
    #[error("[{mode}] {key:?}: {source}")]
    BadKey {
        mode: String,
        key: String,
        #[source]
        source: KeyParseError,
    },
    #[error("[{mode}] {key:?}: {reason}")]
    BadCommand { mode: String, key: String, reason: String },
    #[error("[{mode}] {key:?} is bound twice (same keys spelled differently?)")]
    DuplicateBinding { mode: String, key: String },
    #[error("[{mode}] {prefix:?} is a prefix of {longer:?}: a sequence cannot be both a binding and the start of another")]
    PrefixConflict { mode: String, prefix: String, longer: String },
}

/// What one fed keypress resolved to.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// Part of a longer sequence; wait for more keys.
    Pending,
    /// A count digit: multiply into the count prefix.
    Count(u8),
    /// A complete binding: execute it with the count.
    Command(Command),
    /// No binding starts with these keys; pending keys (and the count) are dropped.
    Unbound,
}

/// The loaded key bindings plus the small state needed to recognise multi-key
/// sequences and count prefixes. See the module docs for the file format.
///
/// Count rule: an unmodified digit in Normal mode is a count digit unless a
/// sequence is already pending. A digit that is itself bound (say `0`) runs its binding
/// when no count has started, and continues the count once one has (`1 0` is ten).
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    tables: BTreeMap<Mode, Vec<Binding>>,
    pending: Vec<KeyChord>,
    count_started: bool,
    warnings: Vec<String>,
}

/// Tables that are accepted but ignored, so user keymaps written for earlier versions
/// keep loading. Each one produces a warning.
const RETIRED_TABLES: &[&str] = &["select", "move"];

impl Keymap {
    /// Parses a keymap from TOML text and validates it.
    pub fn from_toml_str(text: &str) -> Result<Keymap, KeymapError> {
        let root: toml::Table = toml::from_str(text)?;
        let mut tables: BTreeMap<Mode, Vec<Binding>> = BTreeMap::new();
        let mut warnings = Vec::new();
        for (mode_name, value) in root {
            if RETIRED_TABLES.contains(&mode_name.as_str()) {
                warnings.push(format!("ignored [{mode_name}]: there are no modes any more, only [normal]"));
                continue;
            }
            let mode = Mode::from_name(&mode_name).ok_or_else(|| KeymapError::UnknownMode(mode_name.clone()))?;
            let table = match value {
                toml::Value::Table(t) => t,
                _ => return Err(KeymapError::ModeNotATable { mode: mode_name }),
            };
            let mut bindings = Vec::new();
            for (key_str, spec) in table {
                let keys = parse_sequence(&key_str).map_err(|source| KeymapError::BadKey {
                    mode: mode_name.clone(),
                    key: key_str.clone(),
                    source,
                })?;
                let command = parse_command(spec).map_err(|reason| KeymapError::BadCommand {
                    mode: mode_name.clone(),
                    key: key_str.clone(),
                    reason,
                })?;
                bindings.push(Binding { keys, command });
            }
            validate_bindings(&mode_name, &bindings)?;
            tables.insert(mode, bindings);
        }
        Ok(Keymap { tables, pending: Vec::new(), count_started: false, warnings })
    }

    /// Non-fatal problems found while parsing, one line each.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Reads and parses a keymap file.
    pub fn load(path: impl AsRef<Path>) -> Result<Keymap, KeymapError> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).map_err(|source| KeymapError::Io { path: path.to_path_buf(), source })?;
        Keymap::from_toml_str(&text)
    }

    /// Feeds one keypress in `mode`. See [`Resolution`] and the count rule on [`Keymap`].
    pub fn feed(&mut self, mode: Mode, chord: KeyChord) -> Resolution {
        if mode == Mode::CommandLine {
            self.reset();
            return Resolution::Unbound;
        }
        let empty = Vec::new();
        let table = self.tables.get(&mode).unwrap_or(&empty);
        if self.pending.is_empty() && chord.mods.is_none() {
            if let Some(d) = chord.key.digit() {
                let bound = table.iter().any(|b| b.keys.first() == Some(&chord));
                if self.count_started || !bound {
                    self.count_started = true;
                    return Resolution::Count(d);
                }
            }
        }
        self.pending.push(chord);
        if let Some(b) = table.iter().find(|b| b.keys == self.pending) {
            let cmd = b.command.clone();
            self.reset();
            return Resolution::Command(cmd);
        }
        let is_prefix = table.iter().any(|b| b.keys.len() > self.pending.len() && b.keys.starts_with(&self.pending));
        if is_prefix {
            return Resolution::Pending;
        }
        self.reset();
        Resolution::Unbound
    }

    /// Drops any pending key sequence and count state (e.g. when the app loses focus).
    pub fn reset(&mut self) {
        self.pending.clear();
        self.count_started = false;
    }

    /// The keys of the sequence in progress, for the status bar.
    pub fn pending(&self) -> &[KeyChord] {
        &self.pending
    }

    /// `true` once a count digit has been fed and not yet consumed by a command.
    pub fn count_started(&self) -> bool {
        self.count_started
    }

    /// All bindings of a mode as `(key sequence, command)`, sorted by key string, for
    /// the cheat sheet.
    pub fn bindings(&self, mode: Mode) -> Vec<(String, &Command)> {
        let mut out: Vec<(String, &Command)> = self
            .tables
            .get(&mode)
            .map(|v| v.iter().map(|b| (sequence_to_string(&b.keys), &b.command)).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The modes that have at least one binding.
    pub fn modes(&self) -> Vec<Mode> {
        self.tables.keys().copied().collect()
    }

    /// Looks up the exact sequence bound to a command in a mode, for the cheat sheet
    /// ("undo: u").
    pub fn key_for(&self, mode: Mode, command: &Command) -> Option<String> {
        self.tables.get(&mode)?.iter().find(|b| &b.command == command).map(|b| sequence_to_string(&b.keys))
    }
}

fn parse_command(spec: toml::Value) -> Result<Command, String> {
    let table = match spec {
        toml::Value::String(name) => {
            let mut t = toml::Table::new();
            t.insert("cmd".to_string(), toml::Value::String(name));
            t
        }
        toml::Value::Table(t) => {
            if !t.contains_key("cmd") {
                return Err("inline table needs a `cmd = \"...\"` entry".to_string());
            }
            t
        }
        other => return Err(format!("expected a command name or an inline table, found {}", other.type_str())),
    };
    table.try_into::<Command>().map_err(|e| e.message().to_string())
}

fn validate_bindings(mode: &str, bindings: &[Binding]) -> Result<(), KeymapError> {
    for (i, a) in bindings.iter().enumerate() {
        for b in &bindings[i + 1..] {
            if a.keys == b.keys {
                return Err(KeymapError::DuplicateBinding { mode: mode.to_string(), key: sequence_to_string(&a.keys) });
            }
            let (short, long) = if a.keys.len() < b.keys.len() { (a, b) } else { (b, a) };
            if long.keys.starts_with(&short.keys) {
                return Err(KeymapError::PrefixConflict {
                    mode: mode.to_string(),
                    prefix: sequence_to_string(&short.keys),
                    longer: sequence_to_string(&long.keys),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Dir, Unit};

    #[test]
    fn key_parsing() {
        assert_eq!("Right".parse::<Key>(), Ok(Key::Right));
        assert_eq!("enter".parse::<Key>(), Ok(Key::Enter));
        assert_eq!("n".parse::<Key>(), Ok(Key::Char('n')));
        assert_eq!("[".parse::<Key>(), Ok(Key::Char('[')));
        assert_eq!("N".parse::<Key>(), Err(KeyParseError::Uppercase("N".into(), 'n')));
        assert_eq!("".parse::<Key>(), Err(KeyParseError::Empty));
        assert_eq!("Bogus".parse::<Key>(), Err(KeyParseError::UnknownKey("Bogus".into())));
        assert_eq!("\t".parse::<Key>(), Err(KeyParseError::UnknownKey("\t".into())));
        assert_eq!(Key::Char('7').digit(), Some(7));
        assert_eq!(Key::Char('a').digit(), None);
        assert_eq!(Key::Up.digit(), None);
        for (name, key) in Key::NAMED {
            assert_eq!(key.to_string(), name);
        }
    }

    #[test]
    fn chord_parsing_and_display() {
        let c: KeyChord = "Shift+Right".parse().unwrap();
        assert_eq!(c, KeyChord::shift(Key::Right));
        assert_eq!(c.to_string(), "Shift+Right");
        let c: KeyChord = "ctrl+shift+=".parse().unwrap();
        assert_eq!(c, KeyChord { key: Key::Char('='), mods: Mods { shift: true, ctrl: true, alt: false, cmd: false } });
        assert_eq!(c.to_string(), "Ctrl+Shift+=");
        let c: KeyChord = "Shift++".parse().unwrap();
        assert_eq!(c, KeyChord::shift(Key::Char('+')));
        assert_eq!(c.to_string(), "Shift++");
        assert_eq!("+".parse::<KeyChord>(), Ok(KeyChord::ch('+')));
        let c: KeyChord = "Alt+Cmd+x".parse().unwrap();
        assert_eq!(c.to_string(), "Alt+Cmd+x");
        assert_eq!("Hyper+x".parse::<KeyChord>(), Err(KeyParseError::UnknownModifier("Hyper".into())));
        assert_eq!("Shift+".parse::<KeyChord>(), Err(KeyParseError::Empty));
        assert_eq!("Ctrl++".parse::<KeyChord>().unwrap().to_string(), "Ctrl++");
        assert_eq!(" ".parse::<KeyChord>(), Err(KeyParseError::Empty));
        assert_eq!(parse_sequence("g  g"), Ok(vec![KeyChord::ch('g'), KeyChord::ch('g')]));
        assert_eq!(parse_sequence("   "), Err(KeyParseError::Empty));
        assert_eq!(sequence_to_string(&[KeyChord::ch('g'), KeyChord::shift(Key::Up)]), "g Shift+Up");
        assert!(Mods::NONE.is_none());
        assert!(!Mods::SHIFT.is_none());
    }

    const SAMPLE: &str = r#"
[normal]
"Right" = { cmd = "move_cursor", dir = "right", unit = "grid" }
"n" = "insert_note"
"g g" = "select_all"
"g u" = "undo"
"0" = "clear_selection"
"Shift+u" = "redo"

[command_line]
"Escape" = "clear_selection"
"#;

    fn feed_str(km: &mut Keymap, mode: Mode, keys: &str) -> Vec<Resolution> {
        parse_sequence(keys).unwrap().into_iter().map(|c| km.feed(mode, c)).collect()
    }

    #[test]
    fn feed_resolves_bindings() {
        let mut km = Keymap::from_toml_str(SAMPLE).unwrap();
        assert_eq!(
            feed_str(&mut km, Mode::Normal, "Right"),
            vec![Resolution::Command(Command::MoveCursor { dir: Dir::Right, unit: Unit::Grid })]
        );
        assert_eq!(feed_str(&mut km, Mode::Normal, "n"), vec![Resolution::Command(Command::InsertNote)]);
        assert_eq!(feed_str(&mut km, Mode::Normal, "Shift+u"), vec![Resolution::Command(Command::Redo)]);
        assert_eq!(feed_str(&mut km, Mode::Normal, "q"), vec![Resolution::Unbound]);
        // the command line never consults the keymap, even with a table
        assert_eq!(feed_str(&mut km, Mode::CommandLine, "n"), vec![Resolution::Unbound]);
        assert_eq!(feed_str(&mut km, Mode::CommandLine, "Escape"), vec![Resolution::Unbound]);
    }

    #[test]
    fn feed_sequences() {
        let mut km = Keymap::from_toml_str(SAMPLE).unwrap();
        assert_eq!(
            feed_str(&mut km, Mode::Normal, "g g"),
            vec![Resolution::Pending, Resolution::Command(Command::SelectAll)]
        );
        assert_eq!(feed_str(&mut km, Mode::Normal, "g u"), vec![Resolution::Pending, Resolution::Command(Command::Undo)]);
        assert_eq!(feed_str(&mut km, Mode::Normal, "g"), vec![Resolution::Pending]);
        assert_eq!(km.pending(), &[KeyChord::ch('g')]);
        assert_eq!(feed_str(&mut km, Mode::Normal, "x"), vec![Resolution::Unbound]);
        assert!(km.pending().is_empty());
        // a digit while pending is not a count
        assert_eq!(feed_str(&mut km, Mode::Normal, "g 3"), vec![Resolution::Pending, Resolution::Unbound]);
        feed_str(&mut km, Mode::Normal, "g");
        km.reset();
        assert!(km.pending().is_empty());
    }

    #[test]
    fn feed_counts() {
        let mut km = Keymap::from_toml_str(SAMPLE).unwrap();
        assert_eq!(
            feed_str(&mut km, Mode::Normal, "1 2 n"),
            vec![Resolution::Count(1), Resolution::Count(2), Resolution::Command(Command::InsertNote)]
        );
        assert!(!km.count_started());
        // bound 0 wins when no count has started ...
        assert_eq!(feed_str(&mut km, Mode::Normal, "0"), vec![Resolution::Command(Command::ClearSelection)]);
        // ... but continues a count once one has started
        assert_eq!(
            feed_str(&mut km, Mode::Normal, "1 0 n"),
            vec![Resolution::Count(1), Resolution::Count(0), Resolution::Command(Command::InsertNote)]
        );
        assert_eq!(feed_str(&mut km, Mode::Normal, "4"), vec![Resolution::Count(4)]);
        assert!(km.count_started());
        km.reset();
        assert!(!km.count_started());
        // modified digits are never counts
        assert_eq!(feed_str(&mut km, Mode::Normal, "Shift+4"), vec![Resolution::Unbound]);
        // an unbound key ends the count
        assert_eq!(feed_str(&mut km, Mode::Normal, "3 q"), vec![Resolution::Count(3), Resolution::Unbound]);
        assert!(!km.count_started());
    }

    #[test]
    fn bindings_listing() {
        let km = Keymap::from_toml_str(SAMPLE).unwrap();
        let b = km.bindings(Mode::Normal);
        let keys: Vec<&str> = b.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["0", "Right", "Shift+u", "g g", "g u", "n"]);
        assert_eq!(km.modes(), vec![Mode::Normal, Mode::CommandLine]);
        assert_eq!(km.key_for(Mode::Normal, &Command::SelectAll), Some("g g".to_string()));
        assert_eq!(km.key_for(Mode::Normal, &Command::Paste), None);
        assert_eq!(km.key_for(Mode::CommandLine, &Command::Paste), None);
    }

    #[test]
    fn retired_mode_tables_are_skipped_with_a_warning() {
        let km = Keymap::from_toml_str("[normal]\n\"n\" = \"insert_note\"\n\n[select]\n\"v\" = \"undo\"\n\n[move]\n\"Left\" = 7")
            .unwrap();
        assert_eq!(km.bindings(Mode::Normal).len(), 1);
        assert_eq!(km.modes(), vec![Mode::Normal]);
        assert_eq!(km.warnings().len(), 2);
        assert!(km.warnings()[0].contains("[move]"), "{:?}", km.warnings());
        assert!(km.warnings()[1].contains("[select]"), "{:?}", km.warnings());
        assert!(Keymap::from_toml_str("[normal]\n\"n\" = \"insert_note\"").unwrap().warnings().is_empty());
    }

    #[test]
    fn load_errors() {
        let err = Keymap::from_toml_str("[normal]\n\"N\" = \"insert_note\"").unwrap_err();
        assert!(matches!(err, KeymapError::BadKey { .. }), "{err}");
        assert!(err.to_string().contains("Shift+n"));

        let err = Keymap::from_toml_str("[normal]\n\"n\" = \"fly\"").unwrap_err();
        assert!(matches!(err, KeymapError::BadCommand { .. }), "{err}");

        let err = Keymap::from_toml_str("[normal]\n\"n\" = { dir = \"left\" }").unwrap_err();
        assert!(matches!(err, KeymapError::BadCommand { .. }), "{err}");

        let err = Keymap::from_toml_str("[normal]\n\"n\" = { cmd = \"move_cursor\", dir = \"left\" }").unwrap_err();
        assert!(matches!(err, KeymapError::BadCommand { .. }), "{err}");

        let err = Keymap::from_toml_str("[normal]\n\"n\" = 3").unwrap_err();
        assert!(matches!(err, KeymapError::BadCommand { .. }), "{err}");

        let err = Keymap::from_toml_str("[normal]\n\"g\" = \"undo\"\n\"g g\" = \"redo\"").unwrap_err();
        assert!(matches!(err, KeymapError::PrefixConflict { .. }), "{err}");

        let err = Keymap::from_toml_str("[normal]\n\"Shift+Ctrl+a\" = \"undo\"\n\"Ctrl+Shift+a\" = \"redo\"").unwrap_err();
        assert!(matches!(err, KeymapError::DuplicateBinding { .. }), "{err}");

        let err = Keymap::from_toml_str("[insert]\n\"n\" = \"undo\"").unwrap_err();
        assert!(matches!(err, KeymapError::UnknownMode(_)), "{err}");

        let err = Keymap::from_toml_str("normal = 5").unwrap_err();
        assert!(matches!(err, KeymapError::ModeNotATable { .. }), "{err}");

        let err = Keymap::from_toml_str("[normal\n").unwrap_err();
        assert!(matches!(err, KeymapError::Toml(_)), "{err}");

        let err = Keymap::load("/nonexistent/keymap.toml").unwrap_err();
        assert!(matches!(err, KeymapError::Io { .. }), "{err}");
    }

    #[test]
    fn load_from_file() {
        let dir = std::env::temp_dir().join(format!("mp-core-keymap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("k.toml");
        std::fs::write(&path, SAMPLE).unwrap();
        let km = Keymap::load(&path).unwrap();
        assert_eq!(km.bindings(Mode::Normal).len(), 6);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
