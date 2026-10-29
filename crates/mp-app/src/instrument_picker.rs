//! The instrument picker: a keyboard-driven list of every sound the current
//! track's SoundFonts offer, instruments first and drum kits after. Moving the
//! highlight previews a sound; Enter makes it the track's instrument.

use std::path::PathBuf;

use egui::{Align2, RichText, Stroke, Vec2};
use mp_core::{drum_kit_name, program_name, Command, Instrument, Track, DRUM_CHANNEL};

use crate::bridge::PresetList;
use crate::theme::Theme;

/// One row of the picker.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub drums: bool,
    /// The SoundFont to preview it with; empty when no audio is available.
    pub font: PathBuf,
    pub bank: u16,
    pub program: u8,
    pub name: String,
}

impl Entry {
    /// The command that makes this sound the track's instrument.
    pub fn command(&self) -> Command {
        if self.drums {
            Command::SetDrumKit { kit: u16::from(self.program) }
        } else {
            Command::SetPreset { preset: u16::from(self.program) }
        }
    }

    /// The note to preview with: middle C, or the snare on a kit.
    pub fn preview_key(&self) -> u8 {
        if self.drums {
            38
        } else {
            60
        }
    }

    pub fn preview_channel(&self) -> u8 {
        if self.drums {
            DRUM_CHANNEL
        } else {
            0
        }
    }
}

/// What a key press in the picker asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Nothing,
    /// The highlight moved to this sound; play it.
    Audition(Entry),
    /// The user chose this sound; the picker has closed.
    Pick(Entry),
}

#[derive(Default)]
pub struct InstrumentPicker {
    pub open: bool,
    /// The track being edited, so a pick lands on it even if the current track changed.
    pub track: usize,
    entries: Vec<Entry>,
    filter: String,
    selected: usize,
}

/// The General MIDI list, for when no SoundFont can be read (no audio device).
fn general_midi_entries() -> Vec<Entry> {
    let instruments = (0u8..=127).map(|program| Entry {
        drums: false,
        font: PathBuf::new(),
        bank: 0,
        program,
        name: program_name(u16::from(program)).to_string(),
    });
    let kits = (0u8..=127).filter_map(|program| {
        drum_kit_name(u16::from(program)).map(|name| Entry {
            drums: true,
            font: PathBuf::new(),
            bank: 128,
            program,
            name: name.to_string(),
        })
    });
    instruments.chain(kits).collect()
}

fn entries_from(lists: (PresetList, PresetList)) -> Vec<Entry> {
    let (melodic, drums) = lists;
    let melodic_font = melodic.font;
    let drum_font = drums.font;
    melodic
        .presets
        .into_iter()
        .map(|p| Entry { drums: false, font: melodic_font.clone(), bank: p.bank, program: p.program, name: p.name })
        .chain(drums.presets.into_iter().map(|p| Entry {
            drums: true,
            font: drum_font.clone(),
            bank: p.bank,
            program: p.program,
            name: p.name,
        }))
        .collect()
}

impl InstrumentPicker {
    /// Opens the picker for `track`, highlighting its current sound. `lists` is what
    /// its SoundFonts offer; `None` falls back to the General MIDI names.
    pub fn open(&mut self, track_index: usize, track: &Track, lists: Option<(PresetList, PresetList)>) {
        self.entries = match lists {
            Some(lists) => entries_from(lists),
            None => general_midi_entries(),
        };
        if self.entries.is_empty() {
            self.entries = general_midi_entries();
        }
        let Instrument::SoundFont { preset, .. } = &track.instrument;
        let current = self.entries.iter().position(|e| e.drums == track.is_drums() && u16::from(e.program) == *preset);
        self.track = track_index;
        self.filter.clear();
        self.selected = current.unwrap_or(0);
        self.open = true;
    }

    fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                needle.is_empty() || e.name.to_lowercase().contains(&needle) || e.program.to_string() == needle
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn highlighted(&self) -> Option<Entry> {
        let visible = self.visible();
        visible.get(self.selected).map(|i| self.entries[*i].clone())
    }

    /// Typed characters narrow the list.
    pub fn handle_text(&mut self, text: &str) {
        for c in text.chars() {
            if !c.is_control() {
                self.filter.push(c);
            }
        }
        self.selected = 0;
    }

    /// Handles one chord while the picker is open.
    pub fn handle_chord(&mut self, chord: &str) -> Outcome {
        let count = self.visible().len();
        let last = count.saturating_sub(1);
        let before = self.selected;
        match chord {
            "Escape" => {
                if self.filter.is_empty() {
                    self.open = false;
                } else {
                    self.filter.clear();
                    self.selected = 0;
                }
                return Outcome::Nothing;
            }
            "Enter" => {
                return match self.highlighted() {
                    Some(entry) => {
                        self.open = false;
                        Outcome::Pick(entry)
                    }
                    None => Outcome::Nothing,
                };
            }
            "Backspace" => {
                if !self.filter.is_empty() {
                    self.filter.pop();
                    self.selected = 0;
                }
                return Outcome::Nothing;
            }
            "Up" => self.selected = self.selected.saturating_sub(1),
            "Down" => self.selected = (self.selected + 1).min(last),
            "PageUp" => self.selected = self.selected.saturating_sub(15),
            "PageDown" => self.selected = (self.selected + 15).min(last),
            "Home" => self.selected = 0,
            "End" => self.selected = last,
            _ => return Outcome::Nothing,
        }
        if self.selected == before {
            return Outcome::Nothing;
        }
        self.highlighted().map(Outcome::Audition).unwrap_or(Outcome::Nothing)
    }

    pub fn draw(&self, ctx: &egui::Context, theme: &Theme) {
        let font = theme.font(theme.font_size - 1.0);
        let mono = |s: &str| RichText::new(s).font(font.clone());
        let heading = |s: &str| RichText::new(s).font(theme.bold(theme.font_size));
        let viewport = ctx.content_rect();
        let size = Vec2::new((viewport.width() - 160.0).clamp(300.0, 560.0), (viewport.height() - 72.0).max(200.0));
        let frame = egui::Frame::NONE
            .fill(theme.color("status_bg"))
            .stroke(Stroke::new(1.0, theme.color("panel_border")))
            .inner_margin(14.0);
        let visible = self.visible();
        egui::Window::new("instrument_picker")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .frame(frame)
            .fixed_size(size)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let inner_w = size.x - 28.0;
                ui.set_min_size(Vec2::new(inner_w, size.y - 28.0));
                ui.set_max_width(inner_w);
                ui.label(RichText::new("Instrument").font(theme.bold(theme.font_size + 1.0)).color(theme.color("status_mode")));
                ui.add(
                    egui::Label::new(
                        mono("Up/Down to hear each sound · Enter picks it · type to filter · Escape closes.")
                            .color(theme.color("status_dim")),
                    )
                    .wrap(),
                );
                ui.add_space(4.0);
                ui.label(mono(&format!("filter: {}▏", self.filter)).color(theme.color("status_fg")));
                ui.add_space(6.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut group: Option<bool> = None;
                    for (pos, index) in visible.iter().enumerate() {
                        let entry = &self.entries[*index];
                        if group != Some(entry.drums) {
                            group = Some(entry.drums);
                            ui.add_space(if pos == 0 { 0.0 } else { 8.0 });
                            ui.label(heading(if entry.drums { "Drums" } else { "Instruments" }).color(theme.color("accent")));
                        }
                        let selected = pos == self.selected;
                        let bg = if selected { theme.color("lane_current") } else { theme.color("status_bg") };
                        let response = egui::Frame::NONE.fill(bg).inner_margin(2.0).show(ui, |ui| {
                            ui.set_width(inner_w - 8.0);
                            ui.horizontal(|ui| {
                                let number = format!("{:>3}", entry.program);
                                ui.add_sized(
                                    [36.0, 0.0],
                                    egui::Label::new(mono(&number).color(theme.color(if selected { "note_selected" } else { "status_dim" }))),
                                );
                                ui.add(egui::Label::new(mono(&entry.name).color(theme.color("status_fg"))).wrap());
                            });
                        });
                        if selected {
                            ui.scroll_to_rect(response.response.rect, None);
                        }
                    }
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::PresetInfo;

    fn lists() -> (PresetList, PresetList) {
        let melodic = PresetList {
            font: PathBuf::from("gm.sf2"),
            presets: vec![
                PresetInfo { bank: 0, program: 0, name: "Piano".into() },
                PresetInfo { bank: 0, program: 33, name: "Finger Bass".into() },
            ],
        };
        let drums = PresetList {
            font: PathBuf::from("kit.sf2"),
            presets: vec![PresetInfo { bank: 128, program: 0, name: "Standard".into() }],
        };
        (melodic, drums)
    }

    #[test]
    fn opens_on_the_tracks_current_sound_and_moves_with_audition() {
        let mut picker = InstrumentPicker::default();
        let mut track = Track::new("Drums");
        track.channel = DRUM_CHANNEL;
        picker.open(2, &track, Some(lists()));
        assert!(picker.open);
        assert_eq!(picker.track, 2);
        assert_eq!(picker.highlighted().unwrap().name, "Standard");
        match picker.handle_chord("Up") {
            Outcome::Audition(e) => {
                assert_eq!(e.name, "Finger Bass");
                assert_eq!(e.preview_key(), 60);
                assert_eq!(e.command(), Command::SetPreset { preset: 33 });
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(picker.handle_chord("Up"), Outcome::Audition(picker.entries[0].clone()));
        assert_eq!(picker.handle_chord("Up"), Outcome::Nothing, "already at the top");
        assert_eq!(picker.handle_chord("End"), Outcome::Audition(picker.entries[2].clone()));
        match picker.handle_chord("Enter") {
            Outcome::Pick(e) => {
                assert_eq!(e.command(), Command::SetDrumKit { kit: 0 });
                assert_eq!(e.preview_channel(), DRUM_CHANNEL);
            }
            other => panic!("{other:?}"),
        }
        assert!(!picker.open);
    }

    #[test]
    fn typing_filters_and_escape_clears_before_closing() {
        let mut picker = InstrumentPicker::default();
        picker.open(0, &Track::new("Track 1"), Some(lists()));
        picker.handle_text("bass");
        assert_eq!(picker.visible().len(), 1);
        assert_eq!(picker.highlighted().unwrap().name, "Finger Bass");
        picker.handle_chord("Escape");
        assert!(picker.open);
        assert_eq!(picker.visible().len(), 3);
        picker.handle_text("33");
        assert_eq!(picker.highlighted().unwrap().program, 33, "numbers match the program");
        picker.handle_chord("Backspace");
        picker.handle_chord("Backspace");
        picker.handle_chord("Escape");
        assert!(!picker.open);
    }

    #[test]
    fn falls_back_to_general_midi_without_audio() {
        let mut picker = InstrumentPicker::default();
        picker.open(0, &Track::new("Track 1"), None);
        assert_eq!(picker.entries.len(), 128 + 9);
        assert_eq!(picker.highlighted().unwrap().name, "Acoustic Grand Piano");
        assert!(picker.entries.iter().any(|e| e.drums && e.name == "TR-808"));
    }
}
