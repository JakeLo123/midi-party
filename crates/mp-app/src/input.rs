//! Turns egui's raw key events into the chord strings the keymap understands.
//!
//! The OS key-repeat is ignored (egui reports it with `repeat: true`) and we
//! generate our own repeats with a configurable delay and rate, so holding
//! `Right` moves the cursor at a steady, predictable pace.

use std::fmt;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers};

/// One key press with its modifiers, in the keymap's text syntax
/// (`Right`, `Shift+Right`, `n`, `Ctrl+Alt+Shift+Cmd+x`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: String,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub cmd: bool,
}

impl Chord {
    fn has_modifiers(&self, mods: Modifiers) -> bool {
        self.shift == mods.shift && self.ctrl == mods.ctrl && self.alt == mods.alt && self.cmd == mods.mac_cmd
    }
}

impl fmt::Display for Chord {
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
        f.write_str(&self.key)
    }
}

/// Maps an egui key to the keymap's key name. `None` for keys we never bind.
fn key_name(key: Key) -> Option<String> {
    let named = match key {
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::Space => "Space",
        Key::Enter => "Enter",
        Key::Escape => "Escape",
        Key::Tab => "Tab",
        Key::Backspace => "Backspace",
        Key::Delete => "Delete",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::Insert => "Insert",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    let symbol = key.symbol_or_name();
    if symbol.starts_with('F') && symbol[1..].chars().all(|c| c.is_ascii_digit()) && symbol.len() <= 3 {
        return Some(symbol.to_string()); // F1..F12
    }
    let mut chars = symbol.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c.to_ascii_lowercase().to_string()),
        _ => None,
    }
}

/// macOS reports the *shifted* symbol as the logical key (`?` for Shift+/).
/// The keymap spells those as the unshifted key plus `Shift+`, so map them back.
fn unshift(key: Key) -> (Key, bool) {
    match key {
        Key::Questionmark => (Key::Slash, true),
        Key::Colon => (Key::Semicolon, true),
        Key::Plus => (Key::Equals, true),
        Key::Pipe => (Key::Backslash, true),
        Key::Exclamationmark => (Key::Num1, true),
        Key::OpenCurlyBracket => (Key::OpenBracket, true),
        Key::CloseCurlyBracket => (Key::CloseBracket, true),
        other => (other, false),
    }
}

fn chord_from(key: Key, mods: Modifiers) -> Option<Chord> {
    let (key, shifted) = unshift(key);
    Some(Chord {
        key: key_name(key)?,
        shift: mods.shift || shifted,
        ctrl: mods.ctrl,
        alt: mods.alt,
        cmd: mods.mac_cmd,
    })
}

/// Custom key-repeat timing.
#[derive(Clone, Copy, Debug)]
pub struct KeyRepeat {
    pub delay: Duration,
    pub interval: Duration,
}

impl Default for KeyRepeat {
    fn default() -> Self {
        Self { delay: Duration::from_millis(250), interval: Duration::from_millis(40) }
    }
}

/// What one frame of keyboard input amounted to.
#[derive(Default, Debug)]
pub struct Polled {
    /// Key presses (and synthetic repeats), in order.
    pub chords: Vec<Chord>,
    /// Typed text, for the command line. Empty unless the user typed characters.
    pub text: String,
}

/// Per-frame key event collector with its own repeat timer.
#[derive(Default)]
pub struct InputState {
    pub repeat: KeyRepeat,
    /// The chord currently held down and when it next repeats.
    held: Option<(Chord, Instant)>,
}

impl InputState {
    pub fn new(repeat: KeyRepeat) -> Self {
        Self { repeat, held: None }
    }

    /// Collects this frame's key presses and typed text. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context, now: Instant) -> Polled {
        let (events, mods) = ctx.input(|i| (i.events.clone(), i.modifiers));
        let mut out = Polled::default();
        // Modifier keys produce no key events of their own, so a held `Shift+Right`
        // would keep repeating as `Shift+Right` after Shift is released. Stop instead.
        if self.held.as_ref().is_some_and(|(h, _)| !h.has_modifiers(mods)) {
            self.held = None;
        }
        for event in events {
            match event {
                Event::Text(t) => out.text.push_str(&t),
                Event::Key { key, pressed, repeat, modifiers, .. } => {
                    if repeat {
                        continue; // we do our own
                    }
                    let Some(chord) = chord_from(key, modifiers) else { continue };
                    if pressed {
                        self.held = Some((chord.clone(), now + self.repeat.delay));
                        out.chords.push(chord);
                    } else if self.held.as_ref().is_some_and(|(h, _)| h.key == chord.key) {
                        self.held = None;
                    }
                }
                _ => {}
            }
        }
        if let Some((chord, next)) = &mut self.held {
            while *next <= now {
                out.chords.push(chord.clone());
                *next += self.repeat.interval;
            }
            ctx.request_repaint_after(*next - now);
        }
        out
    }

    /// Drop any held key, e.g. when the window loses focus.
    pub fn clear(&mut self) {
        self.held = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_keymap_syntax() {
        assert_eq!(key_name(Key::ArrowRight).as_deref(), Some("Right"));
        assert_eq!(key_name(Key::A).as_deref(), Some("a"));
        assert_eq!(key_name(Key::Num3).as_deref(), Some("3"));
        assert_eq!(key_name(Key::OpenBracket).as_deref(), Some("["));
        assert_eq!(key_name(Key::Equals).as_deref(), Some("="));
        assert_eq!(key_name(Key::F5).as_deref(), Some("F5"));
    }

    #[test]
    fn shifted_symbols_become_unshifted_key_plus_shift() {
        let shift = Modifiers { shift: true, ..Default::default() };
        assert_eq!(chord_from(Key::Questionmark, shift).unwrap().to_string(), "Shift+/");
        assert_eq!(chord_from(Key::Colon, shift).unwrap().to_string(), "Shift+;");
        assert_eq!(chord_from(Key::Plus, shift).unwrap().to_string(), "Shift+=");
        assert_eq!(chord_from(Key::OpenCurlyBracket, shift).unwrap().to_string(), "Shift+[");
        // Even if the OS forgets to report Shift, the symbol itself implies it.
        assert_eq!(chord_from(Key::Questionmark, Modifiers::default()).unwrap().to_string(), "Shift+/");
        assert_eq!(chord_from(Key::Slash, Modifiers::default()).unwrap().to_string(), "/");
    }

    #[test]
    fn chord_display_orders_modifiers() {
        let c = chord_from(Key::N, Modifiers { shift: true, mac_cmd: true, ..Default::default() }).unwrap();
        assert_eq!(c.to_string(), "Shift+Cmd+n");
    }
}
