//! Native file panels and alerts. Only macOS has them for now; elsewhere every
//! dialog is a no-op so the keys and `:` commands behave as they always have.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsaved {
    Save,
    Discard,
    Cancel,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

    fn starting_in(dialog: FileDialog, current: Option<&Path>) -> FileDialog {
        match current.and_then(Path::parent) {
            Some(dir) if dir.is_dir() => dialog.set_directory(dir),
            _ => dialog,
        }
    }

    fn stem(current: Option<&Path>) -> String {
        current.and_then(Path::file_stem).and_then(|s| s.to_str()).unwrap_or("Untitled").to_string()
    }

    pub fn pick_song(current: Option<&Path>) -> Option<PathBuf> {
        starting_in(FileDialog::new(), current)
            .add_filter("Songs and MIDI files", &["ron", "mid", "midi"])
            .pick_file()
    }

    pub fn save_song_as(current: Option<&Path>) -> Option<PathBuf> {
        starting_in(FileDialog::new(), current)
            .add_filter("MIDI Party song", &["ron"])
            .add_filter("MIDI file", &["mid"])
            .set_file_name(format!("{}.ron", stem(current)))
            .save_file()
    }

    pub fn export_to(current: Option<&Path>) -> Option<PathBuf> {
        starting_in(FileDialog::new(), current)
            .set_title("Export")
            .add_filter("WAV audio", &["wav"])
            .add_filter("MP3 audio", &["mp3"])
            .add_filter("MIDI file", &["mid"])
            .set_file_name(format!("{}.wav", stem(current)))
            .save_file()
    }

    pub fn ask_unsaved(name: &str) -> Unsaved {
        let result = MessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title(format!("Do you want to save the changes you made to “{name}”?"))
            .set_description("Your changes will be lost if you don't save them.")
            .set_buttons(MessageButtons::YesNoCancelCustom("Save".into(), "Don't Save".into(), "Cancel".into()))
            .show();
        match result {
            MessageDialogResult::Custom(button) if button == "Save" => Unsaved::Save,
            MessageDialogResult::Custom(button) if button == "Don't Save" => Unsaved::Discard,
            _ => Unsaved::Cancel,
        }
    }

    pub fn settings_placeholder() {
        MessageDialog::new()
            .set_title("Settings are coming soon")
            .set_description("For now, keys are in the Keymap Editor (⌘K) and colors in theme.toml in the config folder.")
            .set_buttons(MessageButtons::Ok)
            .show();
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn pick_song(_current: Option<&Path>) -> Option<PathBuf> {
        None
    }

    pub fn save_song_as(_current: Option<&Path>) -> Option<PathBuf> {
        None
    }

    pub fn export_to(_current: Option<&Path>) -> Option<PathBuf> {
        None
    }

    pub fn ask_unsaved(_name: &str) -> Unsaved {
        Unsaved::Discard
    }

    pub fn settings_placeholder() {}
}

pub use imp::*;
