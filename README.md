# MIDI Party

A midi workstation with good keybindings.

## Build and run

Requires a Rust toolchain (rustc 1.95+). From the repo root:

```bash
cargo run -p mp-app --release
```

Optional: a `.ron` project or `.mid` file to open, and `--sf2 <file>` for the
default SoundFont. A tiny public-domain test bank ships in `assets/soundfonts/`;
for real instruments download a General MIDI bank such as **GeneralUser GS**
(32 MB) or **MuseScore_General** (MIT) and pass it with `--sf2`, or set it per
track with `:sf2 path/to/bank.sf2 <preset>`.

## Configuration

On first run the app writes `~/.config/midi-party/keymap.toml` and `theme.toml`.
Both are *overlays* on the built-in defaults: anything in them overrides, the
rest keeps tracking updates. `preset = "vim"` at the top of `keymap.toml`
switches cursor movement to `h j k l`. Set a key to `"unbind"` to remove it.
Both files reload the moment you save them.

## Headless tools

```bash
cargo run -p mp-cli -- keys "n Right n Right Up Up Up Up n" --out song.ron
cargo run -p mp-cli -- render --sf2 assets/soundfonts/florestan-subset.sf2 --song song.ron --out song.mp3
```

## Layout

- `crates/mp-core` — song model, tempo/meter math, the `Command` enum (95 commands), keymap, undo, command-line parser, `.mid` and `.ron` files. Pure, 161 tests.
- `crates/mp-audio` — real-time engine (cpal + rustysynth), sequencer, offline WAV/MP3 render. The audio callback is tested to allocate nothing.
- `crates/mp-app` — the egui window: piano roll, track panel, lane strip, status bar, cheat sheet, command line, keymap editor.
- `crates/mp-cli` — `mp keys` / `mp render` for scripts and tests.

## Versions

The version number lives in one place, `version` in the root `Cargo.toml`, and
follows [semantic versioning](https://semver.org/).

Saved `.ron` songs carry a separate `format_version`. A file from a newer
format is refused with a clear message; raise `FORMAT_VERSION` in
`crates/mp-core/src/project.rs` only when older builds could no longer read
new files.

## Licensing note

MP3 export links LAME statically via `mp3lame-encoder` (LGPL-3.0). The test
SoundFont (`assets/soundfonts/florestan-subset.sf2`) is public domain; see the
`LICENSE.txt` beside it. Everything else is MIT/Apache/Unlicense.
