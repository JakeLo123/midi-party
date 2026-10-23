//! Snapshot-based undo. Every committed edit pushes a full clone of the song plus the
//! cursor and selection; undo/redo swap whole snapshots back in. No inverse-edit logic.

use std::collections::BTreeSet;

use crate::editor::Cursor;
use crate::song::{NoteId, Song};

/// Default maximum number of undo steps kept.
pub const DEFAULT_UNDO_CAP: usize = 200;

/// Everything restored by an undo: the song, which track was shown, where the cursor
/// was and what was selected.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub song: Song,
    /// Index of the shown track, so undoing a track move or removal shows the same
    /// track again.
    pub track_index: usize,
    pub cursor: Cursor,
    pub selection: BTreeSet<NoteId>,
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    snapshot: Snapshot,
    coalesce_key: Option<String>,
}

/// A bounded undo/redo stack of [`Snapshot`]s.
///
/// Coalescing: [`UndoStack::push`] takes an optional `coalesce_key`. While consecutive
/// pushes carry the same key (and nothing else happened in between), only the first
/// snapshot is kept, so twenty velocity nudges undo in one step. Any push with a
/// different key (or `None`), an undo/redo, or [`UndoStack::break_coalescing`] ends
/// the run.
#[derive(Clone, Debug, PartialEq)]
pub struct UndoStack {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    cap: usize,
    open_key: Option<String>,
}

impl Default for UndoStack {
    fn default() -> Self {
        UndoStack::new(DEFAULT_UNDO_CAP)
    }
}

impl UndoStack {
    /// An empty stack keeping at most `cap` undo steps (oldest dropped first).
    pub fn new(cap: usize) -> Self {
        UndoStack { undo: Vec::new(), redo: Vec::new(), cap: cap.max(1), open_key: None }
    }

    /// Records `before` (the state prior to an edit). Clears the redo stack.
    /// With `coalesce_key = Some(k)`, repeats of the same `k` are merged into the first.
    pub fn push(&mut self, before: Snapshot, coalesce_key: Option<&str>) {
        self.redo.clear();
        if let Some(k) = coalesce_key {
            if self.open_key.as_deref() == Some(k) && !self.undo.is_empty() {
                return; // merged into the open run
            }
        }
        self.open_key = coalesce_key.map(str::to_string);
        self.undo.push(Entry { snapshot: before, coalesce_key: self.open_key.clone() });
        if self.undo.len() > self.cap {
            let excess = self.undo.len() - self.cap;
            self.undo.drain(..excess);
        }
    }

    /// Ends the current coalescing run: the next push always creates a new entry.
    pub fn break_coalescing(&mut self) {
        self.open_key = None;
    }

    /// Pops the most recent snapshot, storing `current` for redo. `None` if empty.
    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        self.open_key = None;
        let entry = self.undo.pop()?;
        self.redo.push(Entry { snapshot: current, coalesce_key: None });
        Some(entry.snapshot)
    }

    /// Pops the most recently undone snapshot, storing `current` for undo.
    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        self.open_key = None;
        let entry = self.redo.pop()?;
        self.undo.push(Entry { snapshot: current, coalesce_key: None });
        if self.undo.len() > self.cap {
            self.undo.remove(0);
        }
        Some(entry.snapshot)
    }

    /// Number of undo steps available.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Number of redo steps available.
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Forgets all history.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open_key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::Tick;

    fn snap(tag: u64) -> Snapshot {
        let mut song = Song::new_empty();
        song.next_note_id = tag;
        Snapshot { song, track_index: 0, cursor: Cursor { tick: Tick(tag), pitch: 60 }, selection: BTreeSet::new() }
    }

    #[test]
    fn push_undo_redo() {
        let mut s = UndoStack::new(10);
        assert!(!s.can_undo());
        s.push(snap(1), None);
        s.push(snap(2), None);
        assert_eq!(s.undo_depth(), 2);
        let back = s.undo(snap(3)).unwrap();
        assert_eq!(back.cursor.tick, Tick(2));
        assert!(s.can_redo());
        let fwd = s.redo(snap(2)).unwrap();
        assert_eq!(fwd.cursor.tick, Tick(3));
        assert!(!s.can_redo());
        assert_eq!(s.undo_depth(), 2);
    }

    #[test]
    fn push_clears_redo() {
        let mut s = UndoStack::new(10);
        s.push(snap(1), None);
        s.undo(snap(2));
        assert_eq!(s.redo_depth(), 1);
        s.push(snap(3), None);
        assert_eq!(s.redo_depth(), 0);
    }

    #[test]
    fn cap_drops_oldest() {
        let mut s = UndoStack::new(3);
        for i in 1..=5 {
            s.push(snap(i), None);
        }
        assert_eq!(s.undo_depth(), 3);
        assert_eq!(s.undo(snap(9)).unwrap().cursor.tick, Tick(5));
        assert_eq!(s.undo(snap(9)).unwrap().cursor.tick, Tick(4));
        assert_eq!(s.undo(snap(9)).unwrap().cursor.tick, Tick(3));
        assert!(s.undo(snap(9)).is_none());
    }

    #[test]
    fn coalescing_merges_runs() {
        let mut s = UndoStack::new(10);
        s.push(snap(1), Some("vel"));
        s.push(snap(2), Some("vel"));
        s.push(snap(3), Some("vel"));
        assert_eq!(s.undo_depth(), 1);
        s.push(snap(4), Some("len"));
        assert_eq!(s.undo_depth(), 2);
        s.push(snap(5), None);
        s.push(snap(6), Some("len"));
        assert_eq!(s.undo_depth(), 4);
        s.break_coalescing();
        s.push(snap(7), Some("len"));
        assert_eq!(s.undo_depth(), 5);
        // undo ends the run too
        s.undo(snap(8));
        s.push(snap(9), Some("len"));
        assert_eq!(s.undo_depth(), 5);
        // the first snapshot of a run is the one restored
        let mut s = UndoStack::new(10);
        s.push(snap(1), Some("vel"));
        s.push(snap(2), Some("vel"));
        assert_eq!(s.undo(snap(3)).unwrap().cursor.tick, Tick(1));
    }

    #[test]
    fn clear_forgets_everything() {
        let mut s = UndoStack::default();
        s.push(snap(1), None);
        s.undo(snap(2));
        s.push(snap(1), None);
        s.clear();
        assert!(!s.can_undo() && !s.can_redo());
    }
}
