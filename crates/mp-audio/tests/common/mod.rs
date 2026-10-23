//! Shared helpers for integration tests.

use std::path::PathBuf;

/// First `.sf2` in `assets/soundfonts/` at the repo root.
pub fn test_soundfont() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/soundfonts");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("sf2")))
        .collect();
    files.sort();
    files.into_iter().next().unwrap_or_else(|| panic!("no .sf2 in {}", dir.display()))
}
