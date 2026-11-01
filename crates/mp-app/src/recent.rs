//! The File > Open Recent list, kept as one path per line in `recent-files.txt`.

use std::path::{Path, PathBuf};

const FILE_NAME: &str = "recent-files.txt";
const MAX: usize = 10;

/// The remembered files, newest first, leaving out any that no longer exist.
pub fn load(config_dir: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(config_dir.join(FILE_NAME)).unwrap_or_default();
    text.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).filter(|p| p.exists()).take(MAX).collect()
}

/// Moves `path` to the front of `list` and writes the list out.
pub fn remember(config_dir: &Path, list: &mut Vec<PathBuf>, path: &Path) {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(MAX);
    write(config_dir, list);
}

pub fn clear(config_dir: &Path, list: &mut Vec<PathBuf>) {
    list.clear();
    write(config_dir, list);
}

fn write(config_dir: &Path, list: &[PathBuf]) {
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    let _ = std::fs::write(config_dir.join(FILE_NAME), text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mp-recent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn newest_first_without_duplicates_and_capped() {
        let dir = temp_dir("order");
        let mut list = Vec::new();
        let files: Vec<PathBuf> = (0..12).map(|i| dir.join(format!("song{i}.ron"))).collect();
        for f in &files {
            std::fs::write(f, "").unwrap();
            remember(&dir, &mut list, f);
        }
        remember(&dir, &mut list, &files[5]);
        let names: Vec<String> = load(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), MAX);
        assert_eq!(names[0], "song5.ron");
        assert_eq!(names[1], "song11.ron");
        assert_eq!(names.iter().filter(|n| *n == "song5.ron").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_files_are_dropped_and_clear_empties() {
        let dir = temp_dir("missing");
        let mut list = Vec::new();
        let kept = dir.join("kept.ron");
        let gone = dir.join("gone.ron");
        std::fs::write(&kept, "").unwrap();
        std::fs::write(&gone, "").unwrap();
        remember(&dir, &mut list, &kept);
        remember(&dir, &mut list, &gone);
        std::fs::remove_file(&gone).unwrap();
        assert_eq!(load(&dir).len(), 1);
        clear(&dir, &mut list);
        assert!(load(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
