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

On first run the app writes `keymap.toml` and `theme.toml` to its config folder:
`~/Library/Application Support/midi-party` on macOS, `~/.config/midi-party` on Linux.
Both are *overlays* on the built-in defaults: anything in them overrides, the
rest keeps tracking updates. `preset = "vim"` at the top of `keymap.toml`
switches cursor movement to `h j k l`. Set a key to `"unbind"` to remove it.
Both files reload the moment you save them. `Cmd+e` hides and shows the piano
roll under the tracks area, and `Cmd+0` fits the whole song in the tracks area again.

The built-in look is the warm dark palette in `themes/default.toml`; copy
`themes/party.toml` over your `theme.toml` for the louder one. `track_colors`
lists the note colors by track number.

## macOS app

```bash
scripts/bundle-macos.sh
```

builds a release binary and wraps it as `target/bundle/MIDI Party.app` with the
icon and the bundled SoundFonts.

## Headless tools

```bash
cargo run -p mp-cli -- keys "n Right n Right Up Up Up Up n" --out song.ron
cargo run -p mp-cli -- render --sf2 assets/soundfonts/florestan-subset.sf2 --song song.ron --out song.mp3
```

## Layout

- `crates/mp-core` — song model, tempo/meter math, the `Command` enum (93 commands), keymap, undo, command-line parser, `.mid` and `.ron` files.
- `crates/mp-audio` — real-time engine (cpal + rustysynth), sequencer, offline WAV/MP3 render. The audio callback is tested to allocate nothing.
- `crates/mp-app` — the egui window: tracks area, piano roll, lane strip, status bar, cheat sheet, command line, keymap editor.
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
SoundFont (`assets/soundfonts/florestan-subset.sf2`) is public domain and the
drum kit (`fluidr3-standard-kit.sf2`) is MIT; see the `LICENSE.txt` beside
them. The bundled font (Anonymous Pro) is under the SIL Open Font
License, see `assets/fonts/LICENSE.txt`. Everything else is MIT/Apache/Unlicense.
