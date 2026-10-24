# Changelog

What changed in each version of midi-party, newest first. Version numbers
follow `0.MINOR.PATCH`: the middle number goes up for new features or changed
keys, the last for fixes only.

## Unreleased

## 0.1.0 — 2026-10-23

The first version.

- Piano roll driven entirely from the keyboard, with Normal, Select and Move
  modes, counts (`4 n`), `f …` selection filters, and a mouse for clicking,
  dragging, resizing and box-selecting.
- Several tracks, each with its own SoundFont instrument, volume, pan, mute and
  solo, plus controller lanes and a sustain pedal.
- Playback, tempo and meter changes, quantize, humanize and legato, and undo/redo.
- Opens and saves `.ron` projects and `.mid` files; exports WAV and MP3.
- Remappable keys and theme via `~/.config/midi-party/keymap.toml` and
  `theme.toml`, both reloaded on save; `preset = "vim"` for `h j k l`; a
  keymap editor (`Cmd+k`) and a cheat sheet (`Shift+/`) that shows the version.
- `midi-party --version` and `mp --version`.
- Saved songs record a `format_version`, so later versions can recognise and
  upgrade older files.
