# Changelog

What changed in each version of midi-party, newest first. follows [semantic versioning](https://semver.org/).

## Unreleased

- Instrument picker: `Ctrl+i` or a click on a track header's sound opens a list of everything the SoundFont has, instruments then drum kits, with a preview as you arrow through. `:preset <n>` and `:drums [kit]` on the command line. Track headers and the status bar show sound names instead of preset numbers.
- Drum tracks: picking a kit puts the track on MIDI channel 10 and names it Drums; the piano roll's key column then shows the drum each row plays.
- New tracks take their own MIDI channel (skipping 10), so exported `.mid` files keep each track's sound in players with one synth.
- Fixed: a track's sound only reached its synth after playback had started, so the first notes you entered could play with the wrong instrument.
- Fixed: the minus key did nothing in the window, so `-` (Velocity -5), `Shift+-` (Velocity -20) and `Cmd+-` (zoom out) never fired.
- Drum sounds: a track on channel 10 (`:channel 10`) plays a bundled drum kit, the standard kit from FluidR3 (MIT), unless the default SoundFont has kits of its own. `--drums-sf2` swaps the kit file.
- Tracks area above the piano roll: every track as a lane with its notes drawn across the whole song, the cursor and playhead on all of them, click to jump. It has its own zoom (Cmd+scroll or pinch, `Cmd+0` refits) and `Cmd+e` hides the piano roll so it fills the window. The left track panel is gone; its info is in the lane headers.

## 0.1.0 — 2026-10-23

The first version.