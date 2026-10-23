# MIDI Party

A keyboard-driven, modal MIDI piano-roll editor and small DAW, written in Rust.
Every edit — selecting, moving, stretching, velocity, copy/paste, tracks,
controller lanes — is a key press, and every key is remappable. Full design and
milestone plan: `docs/PLAN.md`; what is verified and what is not: `docs/STATUS.md`.

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

## Keys

Press `Shift+/` for the live cheat sheet; `Cmd+k` opens the keymap editor. The basics:

| Keys | Action |
|---|---|
| arrows / Shift+arrows / Ctrl+Left,Right | cursor by grid step / bar & octave / beat |
| `n`, `Shift+n` | insert a note / a major chord at the cursor (`4 n` = four grid steps long) |
| `x`, `Delete` | delete the selection (or the note under the cursor) |
| `s`, `v`, `Shift+v` | toggle note under cursor / box select / time-range select; `Enter` confirms |
| `a`, `b`, `w`, `Shift+s`, `Shift+p`, `i` | select all / bar / next 4 notes / chord / same pitch / invert |
| `Tab`, `Shift+Tab`, `Shift+x` | step through the selection, drop the focused note |
| `f t` `f b` `f e` `f o` `f a` `f l` | keep only: top / bottom of chords, every other, on-beat, above / below cursor |
| `m` then arrows | move the selected notes; `[` `]` length, `-` `=` velocity, `Alt+Left/Right` nudge off-grid |
| `c` `p` `d` `.` | copy, paste at cursor, duplicate, repeat last edit |
| `q`, `Shift+q`, `h`, `Shift+l` | quantize starts / lengths, humanize, legato |
| `u` / `Shift+u` | undo / redo |
| `g` / `Shift+g` | finer / coarser grid |
| `t` / `Shift+t`, `Cmd+t`, `Cmd+Shift+t` | next / previous track, add track, remove track |
| `Ctrl+m`, `Ctrl+s`, `Ctrl+=`/`-`, `Ctrl+[`/`]` | mute, solo, track volume, track pan |
| `Ctrl+.` / `Ctrl+,` | next / previous SoundFont preset |
| `Shift+c`, `Ctrl+Shift+Up/Down`, `Ctrl+Shift+d/u` | cycle controller lane, nudge lane value, sustain pedal down / up |
| `Space`, `Enter` | play/stop, play from cursor |
| `:` | command line: `:w song.ron`, `:o file.mid`, `:export out.mp3`, `:bpm 96`, `:meter 3/4`, `:vel 70..110`, `:quantize 1/8 50%`, `:rename Bass`, `:sf2 bank.sf2 33`, `:lane ramp 0..127`, `:q` |

Type a number before any key to repeat it. The mouse works too: click to place
the cursor or select, Shift+click to toggle, double-click to insert, drag to
move, drag a note's right edge to resize, drag empty space to box-select,
wheel to scroll, pinch or Cmd+wheel to zoom.

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

- `crates/mp-core` — song model, tempo/meter math, the `Command` enum (95 commands), keymap, undo, command-line parser, `.mid` and `.ron` files. Pure, 158 tests.
- `crates/mp-audio` — real-time engine (cpal + rustysynth), sequencer, offline WAV/MP3 render. The audio callback is tested to allocate nothing.
- `crates/mp-app` — the egui window: piano roll, track panel, lane strip, status bar, cheat sheet, command line, keymap editor.
- `crates/mp-cli` — `mp keys` / `mp render` for scripts and tests.

## Licensing note

MP3 export links LAME statically via `mp3lame-encoder` (LGPL-3.0). The test
SoundFont (`assets/soundfonts/florestan-subset.sf2`) is public domain; see the
`LICENSE.txt` beside it. Everything else is MIT/Apache/Unlicense.
