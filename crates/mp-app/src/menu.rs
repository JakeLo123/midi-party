//! The macOS menu bar.
//!
//! macOS hands a key that matches a menu item's shortcut to the menu instead of the
//! window, so the app never sees it as a keypress. To keep both paths identical, a
//! click on an item with a shortcut is reported as that key ([`MenuClick::Key`]) and
//! goes through the same handling as typing it. Shortcuts for editor commands come
//! from the live keymap; only chords with Cmd or Ctrl are shown, because a plain
//! letter as a menu shortcut would swallow typing in the command line.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use mp_core::{KeyChord, Keymap};
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{
    AboutMetadata, CheckMenuItem, Icon, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};

/// What a click on a menu item asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuClick {
    /// Handle this chord exactly as if it had been typed.
    Key(String),
    /// Run the item directly; it has no shortcut. The id is an [`AppItem`] id or an
    /// editor command name.
    Run(&'static str),
    OpenRecent(PathBuf),
    ClearRecent,
}

/// Items the app handles itself, with their fixed shortcuts (in keymap syntax).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppItem {
    pub id: &'static str,
    pub label: &'static str,
    pub chord: Option<&'static str>,
}

pub const SETTINGS: AppItem = AppItem { id: "settings", label: "Settings…", chord: Some("Cmd+,") };
pub const QUIT: AppItem = AppItem { id: "quit", label: "Quit MIDI Party", chord: Some("Cmd+q") };
pub const NEW: AppItem = AppItem { id: "new", label: "New", chord: Some("Cmd+n") };
pub const OPEN: AppItem = AppItem { id: "open", label: "Open…", chord: Some("Cmd+o") };
pub const SAVE: AppItem = AppItem { id: "save", label: "Save", chord: Some("Cmd+s") };
pub const SAVE_AS: AppItem = AppItem { id: "save_as", label: "Save As…", chord: Some("Shift+Cmd+s") };
pub const EXPORT: AppItem = AppItem { id: "export", label: "Export…", chord: None };
pub const KEY_BINDINGS: AppItem = AppItem { id: "cheat_sheet", label: "Key Bindings", chord: None };
pub const KEYMAP_EDITOR: AppItem = AppItem { id: "keymap_editor", label: "Keymap Editor…", chord: Some("Cmd+k") };

const APP_ITEMS: &[AppItem] = &[SETTINGS, QUIT, NEW, OPEN, SAVE, SAVE_AS, EXPORT, KEY_BINDINGS, KEYMAP_EDITOR];

/// Editor commands in the menus: (command name, label).
const EDIT_ITEMS: &[(&str, &str)] = &[
    ("undo", "Undo"),
    ("redo", "Redo"),
    ("cut", "Cut"),
    ("copy", "Copy"),
    ("paste", "Paste"),
    ("delete_selected", "Delete"),
    ("select_all", "Select All"),
];
const VIEW_ITEMS: &[(&str, &str)] = &[("zoom_in", "Zoom In"), ("zoom_out", "Zoom Out"), ("zoom_to_fit", "Zoom to Fit")];
const PIANO_ROLL: (&str, &str) = ("toggle_piano_roll", "Show Piano Roll");
const TRANSPORT_ITEMS: &[(&str, &str)] = &[
    ("toggle_play", "Play or Stop"),
    ("playhead_to_start", "Go to Start"),
    ("toggle_loop", "Loop"),
    ("toggle_metronome", "Metronome"),
    ("tap_tempo", "Tap Tempo"),
];

const RECENT_PREFIX: &str = "recent:";
const CLEAR_RECENT: &str = "clear_recent";

pub struct MacMenu {
    _bar: Menu,
    events: Receiver<MenuEvent>,
    /// Editor-command items and the chord each one currently shows.
    command_items: Vec<(&'static str, MenuItem, Option<String>)>,
    piano_roll: CheckMenuItem,
    piano_roll_chord: Option<String>,
    recent: Submenu,
    recent_paths: Vec<PathBuf>,
}

impl MacMenu {
    /// Builds the menu bar and installs it as the app's main menu. Clicks wake the UI through `ctx`.
    pub fn install(ctx: egui::Context, keymap: &Keymap, recent: &[PathBuf], icon: &egui::IconData) -> Self {
        let (tx, events) = channel();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = tx.send(event);
            ctx.request_repaint();
        }));

        let app_item = |item: AppItem| MenuItem::with_id(item.id, item.label, true, item.chord.and_then(accelerator));
        let separator = PredefinedMenuItem::separator;

        let about = AboutMetadata {
            name: Some("MIDI Party".into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            icon: Icon::from_rgba(icon.rgba.clone(), icon.width, icon.height).ok(),
            ..Default::default()
        };
        let app_menu = Submenu::with_items(
            "MIDI Party",
            true,
            &[
                &PredefinedMenuItem::about(Some("About MIDI Party"), Some(about)),
                &separator(),
                &app_item(SETTINGS),
                &separator(),
                &PredefinedMenuItem::services(None),
                &separator(),
                &PredefinedMenuItem::hide(Some("Hide MIDI Party")),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &separator(),
                &app_item(QUIT),
            ],
        )
        .expect("app menu builds");

        let recent_menu = Submenu::new("Open Recent", true);
        let file_menu = Submenu::with_items(
            "File",
            true,
            &[
                &app_item(NEW),
                &app_item(OPEN),
                &recent_menu,
                &separator(),
                &app_item(SAVE),
                &app_item(SAVE_AS),
                &separator(),
                &app_item(EXPORT),
            ],
        )
        .expect("file menu builds");

        let mut command_items = Vec::new();
        let mut command_item = |(name, label): (&'static str, &str)| {
            let chord = menu_chord(keymap, name);
            let item = MenuItem::with_id(name, label, true, chord.as_deref().and_then(accelerator));
            command_items.push((name, item.clone(), chord));
            item
        };
        let edit: Vec<MenuItem> = EDIT_ITEMS.iter().map(|&i| command_item(i)).collect();
        // AppKit appends Writing Tools, AutoFill, Dictation and Emoji & Symbols to any menu
        // titled exactly "Edit". They only act on native text fields, which this app has none of,
        // so a zero-width space keeps the title looking the same without the extras.
        let edit_menu = Submenu::with_items(
            "Edit\u{200B}",
            true,
            &[&edit[0], &edit[1], &separator(), &edit[2], &edit[3], &edit[4], &edit[5], &separator(), &edit[6]],
        )
        .expect("edit menu builds");

        let piano_roll_chord = menu_chord(keymap, PIANO_ROLL.0);
        let piano_roll = CheckMenuItem::with_id(
            PIANO_ROLL.0,
            PIANO_ROLL.1,
            true,
            true,
            piano_roll_chord.as_deref().and_then(accelerator),
        );
        let view: Vec<MenuItem> = VIEW_ITEMS.iter().map(|&i| command_item(i)).collect();
        let view_menu = Submenu::with_items(
            "View",
            true,
            &[
                &piano_roll,
                &separator(),
                &view[0],
                &view[1],
                &view[2],
                &separator(),
                &PredefinedMenuItem::fullscreen(None),
            ],
        )
        .expect("view menu builds");

        let transport: Vec<MenuItem> = TRANSPORT_ITEMS.iter().map(|&i| command_item(i)).collect();
        let transport_menu = Submenu::with_items(
            "Transport",
            true,
            &[&transport[0], &transport[1], &separator(), &transport[2], &transport[3], &transport[4]],
        )
        .expect("transport menu builds");

        let window_menu = Submenu::with_items(
            "Window",
            true,
            &[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::maximize(Some("Zoom")),
                &separator(),
                &PredefinedMenuItem::bring_all_to_front(None),
            ],
        )
        .expect("window menu builds");

        let help_menu =
            Submenu::with_items("Help", true, &[&app_item(KEY_BINDINGS), &app_item(KEYMAP_EDITOR)]).expect("help menu builds");

        let bar = Menu::with_items(&[&app_menu, &file_menu, &edit_menu, &view_menu, &transport_menu, &window_menu, &help_menu])
            .expect("menu bar builds");
        bar.init_for_nsapp();
        window_menu.set_as_windows_menu_for_nsapp();
        help_menu.set_as_help_menu_for_nsapp();

        let mut menu = Self {
            _bar: bar,
            events,
            command_items,
            piano_roll,
            piano_roll_chord,
            recent: recent_menu,
            recent_paths: Vec::new(),
        };
        menu.set_recent(recent);
        menu
    }

    /// Clicks since the last call, oldest first.
    pub fn poll(&self) -> Vec<MenuClick> {
        self.events.try_iter().filter_map(|event| self.click(event.id().as_ref())).collect()
    }

    fn click(&self, id: &str) -> Option<MenuClick> {
        if let Some(index) = id.strip_prefix(RECENT_PREFIX) {
            return index.parse::<usize>().ok().and_then(|i| self.recent_paths.get(i)).cloned().map(MenuClick::OpenRecent);
        }
        if id == CLEAR_RECENT {
            return Some(MenuClick::ClearRecent);
        }
        if let Some(item) = APP_ITEMS.iter().find(|i| i.id == id) {
            return Some(match item.chord {
                Some(chord) => MenuClick::Key(chord.to_string()),
                None => MenuClick::Run(item.id),
            });
        }
        let (name, chord) = if id == PIANO_ROLL.0 {
            (PIANO_ROLL.0, &self.piano_roll_chord)
        } else {
            self.command_items.iter().find(|(name, ..)| *name == id).map(|(name, _, chord)| (*name, chord))?
        };
        Some(match chord {
            Some(chord) => MenuClick::Key(chord.clone()),
            None => MenuClick::Run(name),
        })
    }

    /// Re-reads the editor-command shortcuts after the keymap changed.
    pub fn set_keymap(&mut self, keymap: &Keymap) {
        for (name, item, chord) in &mut self.command_items {
            *chord = menu_chord(keymap, name);
            let _ = item.set_accelerator(chord.as_deref().and_then(accelerator));
        }
        self.piano_roll_chord = menu_chord(keymap, PIANO_ROLL.0);
        let _ = self.piano_roll.set_accelerator(self.piano_roll_chord.as_deref().and_then(accelerator));
    }

    pub fn set_piano_roll_shown(&self, shown: bool) {
        if self.piano_roll.is_checked() != shown {
            self.piano_roll.set_checked(shown);
        }
    }

    pub fn set_recent(&mut self, paths: &[PathBuf]) {
        if self.recent_paths == paths {
            return;
        }
        while self.recent.remove_at(0).is_some() {}
        for (i, path) in paths.iter().enumerate() {
            let item = MenuItem::with_id(format!("{RECENT_PREFIX}{i}"), recent_label(path), true, None);
            let _ = self.recent.append(&item);
        }
        if !paths.is_empty() {
            let _ = self.recent.append(&PredefinedMenuItem::separator());
        }
        let _ = self.recent.append(&MenuItem::with_id(CLEAR_RECENT, "Clear Menu", !paths.is_empty(), None));
        self.recent_paths = paths.to_vec();
    }
}

fn recent_label(path: &Path) -> String {
    path.file_name().and_then(|n| n.to_str()).map(str::to_string).unwrap_or_else(|| path.display().to_string())
}

/// The chord a menu item shows for an editor command: its first single-chord binding
/// that holds Cmd or Ctrl, preferring Cmd. `None` if every binding is a plain key or a sequence.
pub fn menu_chord(keymap: &Keymap, command: &str) -> Option<String> {
    let mut chords: Vec<KeyChord> = keymap
        .bindings()
        .into_iter()
        .filter(|(_, cmd)| cmd.name() == command)
        .filter_map(|(keys, _)| keys.parse::<KeyChord>().ok())
        .filter(|chord| chord.mods.cmd || chord.mods.ctrl)
        .collect();
    chords.sort_by_key(|chord| (!chord.mods.cmd, chord.mods));
    chords.first().map(|chord| chord.to_string())
}

/// Converts a keymap chord (`"Shift+Cmd+z"`) to a menu shortcut. `None` for sequences
/// and keys the menu can't show.
pub fn accelerator(chord: &str) -> Option<Accelerator> {
    let chord: KeyChord = chord.parse().ok()?;
    let mut mods = Modifiers::empty();
    mods.set(Modifiers::SHIFT, chord.mods.shift);
    mods.set(Modifiers::CONTROL, chord.mods.ctrl);
    mods.set(Modifiers::ALT, chord.mods.alt);
    mods.set(Modifiers::META, chord.mods.cmd);
    let key = match chord.key.to_string().as_str() {
        "Up" => Code::ArrowUp,
        "Down" => Code::ArrowDown,
        "Left" => Code::ArrowLeft,
        "Right" => Code::ArrowRight,
        other => Accelerator::try_from(other).ok()?.key(),
    };
    Some(Accelerator::new(mods, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mp_core::default_keymap;

    #[test]
    fn accelerators_keep_every_modifier() {
        let a = accelerator("Shift+Cmd+z").unwrap();
        assert_eq!(a.modifiers(), Modifiers::SHIFT | Modifiers::META);
        assert_eq!(a.key(), Code::KeyZ);
        assert_eq!(accelerator("Cmd+=").unwrap().key(), Code::Equal);
        assert_eq!(accelerator("Cmd+,").unwrap().key(), Code::Comma);
        assert_eq!(accelerator("Ctrl+Alt+Up").unwrap().key(), Code::ArrowUp);
        assert_eq!(accelerator("Cmd+Backspace").unwrap().key(), Code::Backspace);
    }

    #[test]
    fn sequences_and_junk_have_no_accelerator() {
        assert_eq!(accelerator("f t"), None);
        assert_eq!(accelerator(""), None);
    }

    #[test]
    fn menu_chords_come_from_the_keymap_and_skip_plain_keys() {
        let keymap = default_keymap();
        assert_eq!(menu_chord(&keymap, "undo").as_deref(), Some("Cmd+z"));
        assert_eq!(menu_chord(&keymap, "redo").as_deref(), Some("Shift+Cmd+z"));
        assert_eq!(menu_chord(&keymap, "select_all").as_deref(), Some("Cmd+a"), "not the plain 'a'");
        assert_eq!(menu_chord(&keymap, "toggle_piano_roll").as_deref(), Some("Cmd+e"));
        assert_eq!(menu_chord(&keymap, "delete_selected"), None, "x, Delete and Backspace are all plain keys");
    }

    #[test]
    fn rebinding_moves_the_menu_chord() {
        let toml = format!("{}\n", mp_core::DEFAULT_KEYMAP_TOML).replace("\"Cmd+z\"       = \"undo\"", "\"Ctrl+z\" = \"undo\"");
        let keymap = Keymap::from_toml_str(&toml).unwrap();
        assert_eq!(menu_chord(&keymap, "undo").as_deref(), Some("Ctrl+z"));
    }
}
