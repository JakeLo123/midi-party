//! General MIDI names: the 128 program names, the standard drum kits, and the
//! percussion key map, so the UI can show "Finger Bass" and "Snare" while the
//! song keeps storing the numbers that every other MIDI program understands.

/// The General MIDI level 1 program names, indexed by program number 0..=127.
pub const GM_PROGRAMS: [&str; 128] = [
    "Acoustic Grand Piano",
    "Bright Acoustic Piano",
    "Electric Grand Piano",
    "Honky-tonk Piano",
    "Electric Piano 1",
    "Electric Piano 2",
    "Harpsichord",
    "Clavinet",
    "Celesta",
    "Glockenspiel",
    "Music Box",
    "Vibraphone",
    "Marimba",
    "Xylophone",
    "Tubular Bells",
    "Dulcimer",
    "Drawbar Organ",
    "Percussive Organ",
    "Rock Organ",
    "Church Organ",
    "Reed Organ",
    "Accordion",
    "Harmonica",
    "Tango Accordion",
    "Acoustic Guitar (nylon)",
    "Acoustic Guitar (steel)",
    "Electric Guitar (jazz)",
    "Electric Guitar (clean)",
    "Electric Guitar (muted)",
    "Overdriven Guitar",
    "Distortion Guitar",
    "Guitar Harmonics",
    "Acoustic Bass",
    "Electric Bass (finger)",
    "Electric Bass (pick)",
    "Fretless Bass",
    "Slap Bass 1",
    "Slap Bass 2",
    "Synth Bass 1",
    "Synth Bass 2",
    "Violin",
    "Viola",
    "Cello",
    "Contrabass",
    "Tremolo Strings",
    "Pizzicato Strings",
    "Orchestral Harp",
    "Timpani",
    "String Ensemble 1",
    "String Ensemble 2",
    "Synth Strings 1",
    "Synth Strings 2",
    "Choir Aahs",
    "Voice Oohs",
    "Synth Voice",
    "Orchestra Hit",
    "Trumpet",
    "Trombone",
    "Tuba",
    "Muted Trumpet",
    "French Horn",
    "Brass Section",
    "Synth Brass 1",
    "Synth Brass 2",
    "Soprano Sax",
    "Alto Sax",
    "Tenor Sax",
    "Baritone Sax",
    "Oboe",
    "English Horn",
    "Bassoon",
    "Clarinet",
    "Piccolo",
    "Flute",
    "Recorder",
    "Pan Flute",
    "Blown Bottle",
    "Shakuhachi",
    "Whistle",
    "Ocarina",
    "Lead 1 (square)",
    "Lead 2 (sawtooth)",
    "Lead 3 (calliope)",
    "Lead 4 (chiff)",
    "Lead 5 (charang)",
    "Lead 6 (voice)",
    "Lead 7 (fifths)",
    "Lead 8 (bass + lead)",
    "Pad 1 (new age)",
    "Pad 2 (warm)",
    "Pad 3 (polysynth)",
    "Pad 4 (choir)",
    "Pad 5 (bowed)",
    "Pad 6 (metallic)",
    "Pad 7 (halo)",
    "Pad 8 (sweep)",
    "FX 1 (rain)",
    "FX 2 (soundtrack)",
    "FX 3 (crystal)",
    "FX 4 (atmosphere)",
    "FX 5 (brightness)",
    "FX 6 (goblins)",
    "FX 7 (echoes)",
    "FX 8 (sci-fi)",
    "Sitar",
    "Banjo",
    "Shamisen",
    "Koto",
    "Kalimba",
    "Bag pipe",
    "Fiddle",
    "Shanai",
    "Tinkle Bell",
    "Agogo",
    "Steel Drums",
    "Woodblock",
    "Taiko Drum",
    "Melodic Tom",
    "Synth Drum",
    "Reverse Cymbal",
    "Guitar Fret Noise",
    "Breath Noise",
    "Seashore",
    "Bird Tweet",
    "Telephone Ring",
    "Helicopter",
    "Applause",
    "Gunshot",
];

/// The General MIDI name of a program; programs above 127 fall back to 127.
pub fn program_name(program: u16) -> &'static str {
    GM_PROGRAMS[usize::from(program.min(127))]
}

/// The drum kits General MIDI and GS place on channel 10, by program number.
const DRUM_KITS: [(u16, &str); 9] = [
    (0, "Standard"),
    (8, "Room"),
    (16, "Power"),
    (24, "Electronic"),
    (25, "TR-808"),
    (32, "Jazz"),
    (40, "Brush"),
    (48, "Orchestra"),
    (56, "SFX"),
];

/// The standard name of a drum kit program, or `None` for a program with no
/// General MIDI kit name.
pub fn drum_kit_name(program: u16) -> Option<&'static str> {
    DRUM_KITS.iter().find(|(p, _)| *p == program).map(|(_, name)| *name)
}

/// The General MIDI percussion map: what each key plays on a drum track, as a
/// full name and a label short enough for the piano roll's key column.
const PERCUSSION: [(u8, &str, &str); 47] = [
    (35, "Acoustic Bass Drum", "Kick 2"),
    (36, "Bass Drum 1", "Kick"),
    (37, "Side Stick", "Stick"),
    (38, "Acoustic Snare", "Snare"),
    (39, "Hand Clap", "Clap"),
    (40, "Electric Snare", "Snare 2"),
    (41, "Low Floor Tom", "LoTom 2"),
    (42, "Closed Hi-Hat", "Cl HH"),
    (43, "High Floor Tom", "LoTom"),
    (44, "Pedal Hi-Hat", "Pd HH"),
    (45, "Low Tom", "MidTom2"),
    (46, "Open Hi-Hat", "Op HH"),
    (47, "Low-Mid Tom", "MidTom"),
    (48, "Hi-Mid Tom", "HiTom 2"),
    (49, "Crash Cymbal 1", "Crash"),
    (50, "High Tom", "HiTom"),
    (51, "Ride Cymbal 1", "Ride"),
    (52, "Chinese Cymbal", "China"),
    (53, "Ride Bell", "RideBel"),
    (54, "Tambourine", "Tamb"),
    (55, "Splash Cymbal", "Splash"),
    (56, "Cowbell", "Cowbell"),
    (57, "Crash Cymbal 2", "Crash 2"),
    (58, "Vibraslap", "Vibra"),
    (59, "Ride Cymbal 2", "Ride 2"),
    (60, "Hi Bongo", "HiBongo"),
    (61, "Low Bongo", "LoBongo"),
    (62, "Mute Hi Conga", "MtConga"),
    (63, "Open Hi Conga", "HiConga"),
    (64, "Low Conga", "LoConga"),
    (65, "High Timbale", "HiTimb"),
    (66, "Low Timbale", "LoTimb"),
    (67, "High Agogo", "HiAgogo"),
    (68, "Low Agogo", "LoAgogo"),
    (69, "Cabasa", "Cabasa"),
    (70, "Maracas", "Maracas"),
    (71, "Short Whistle", "Whistl"),
    (72, "Long Whistle", "Whistl2"),
    (73, "Short Guiro", "Guiro"),
    (74, "Long Guiro", "Guiro 2"),
    (75, "Claves", "Claves"),
    (76, "Hi Wood Block", "HiBlock"),
    (77, "Low Wood Block", "LoBlock"),
    (78, "Mute Cuica", "MtCuica"),
    (79, "Open Cuica", "OpCuica"),
    (80, "Mute Triangle", "MtTri"),
    (81, "Open Triangle", "OpTri"),
];

/// The percussion sound a key plays on a drum track, for keys 35..=81.
pub fn percussion_name(key: u8) -> Option<&'static str> {
    PERCUSSION.iter().find(|(k, _, _)| *k == key).map(|(_, name, _)| *name)
}

/// A label of at most seven characters for the same key, for narrow columns.
pub fn percussion_short(key: u8) -> Option<&'static str> {
    PERCUSSION.iter().find(|(k, _, _)| *k == key).map(|(_, _, short)| *short)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_names_follow_the_general_midi_order() {
        assert_eq!(program_name(0), "Acoustic Grand Piano");
        assert_eq!(program_name(33), "Electric Bass (finger)");
        assert_eq!(program_name(127), "Gunshot");
        assert_eq!(program_name(300), "Gunshot");
    }

    #[test]
    fn drum_kits_have_names_only_at_the_standard_programs() {
        assert_eq!(drum_kit_name(0), Some("Standard"));
        assert_eq!(drum_kit_name(25), Some("TR-808"));
        assert_eq!(drum_kit_name(1), None);
    }

    #[test]
    fn percussion_map_covers_35_to_81_with_short_labels() {
        for key in 35..=81 {
            let short = percussion_short(key).expect("mapped key");
            assert!(short.chars().count() <= 7, "{short:?} is too long for the key column");
            assert!(percussion_name(key).is_some());
        }
        assert_eq!(percussion_name(36), Some("Bass Drum 1"));
        assert_eq!(percussion_short(38), Some("Snare"));
        assert_eq!(percussion_name(34), None);
        assert_eq!(percussion_short(82), None);
    }
}
