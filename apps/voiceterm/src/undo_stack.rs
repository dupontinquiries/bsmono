//! v0.1.693: editor undo/redo History — the snapshot ring built on the
//! undo_coalesce smart-separation logic. Per docs/design/editor_undo.md the
//! model is full-doc snapshots (robust, composes with the per-block splice
//! model; notes are small).
//!
//! v0.1.693 also folds in the user's idea: VOICE utterances are natural undo
//! boundaries — a dictated phrase undoes as ONE unit and its text labels the
//! entry. record_voice() forces a boundary + stores the utterance label.
//!
//! PURE + testable. Entries are committed states; `idx` points at the current
//! one. undo/redo move `idx`; a new boundary truncates the redo tail.

use crate::undo_coalesce::{edit_kind, should_break, kind_label, EditKind};

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub doc: String,
    pub cursor: usize,
    /// short label for the visual timeline / voice matching
    pub label: String,
}

pub struct History {
    entries: Vec<Snapshot>,
    /// index of the CURRENT committed state in `entries`
    idx: usize,
    cap: usize,
    last_kind: Option<EditKind>,
    last_edit_ms: u64,
    pause_ms: u64,
}

impl History {
    /// Create with the initial document state as entry 0.
    pub fn new(doc: &str, cursor: usize) -> Self {
        History {
            entries: vec![Snapshot { doc: doc.to_string(), cursor, label: "open".into() }],
            idx: 0,
            cap: 300,
            last_kind: None,
            last_edit_ms: 0,
            pause_ms: 700,
        }
    }

    pub fn current(&self) -> &Snapshot { &self.entries[self.idx] }
    pub fn can_undo(&self) -> bool { self.idx > 0 }
    pub fn can_redo(&self) -> bool { self.idx + 1 < self.entries.len() }
    pub fn len(&self) -> usize { self.entries.len() }

    /// Record a typed/deleted edit. `inserted` = the char just typed (None for
    /// a deletion/paste). Coalesces into the current entry unless a boundary
    /// is warranted (then commits a new snapshot, truncating any redo tail).
    pub fn record(&mut self, doc: &str, cursor: usize, inserted: Option<char>, now_ms: u64) {
        let old_len = self.entries[self.idx].doc.chars().count();
        let new_len = doc.chars().count();
        if old_len == new_len && self.entries[self.idx].doc == doc {
            return; // no actual change (e.g. cursor-only)
        }
        let kind = edit_kind(old_len, new_len, inserted);
        let dt = now_ms.saturating_sub(self.last_edit_ms);
        // first edit of a fresh unit (after open, undo, or a voice boundary)
        // starts a NEW entry; otherwise coalesce unless a boundary is warranted.
        let boundary = self.last_kind.is_none() || should_break(self.last_kind, kind, dt, self.pause_ms);
        self.commit(doc, cursor, kind_label(kind), boundary);
        self.last_kind = Some(kind);
        self.last_edit_ms = now_ms;
    }

    /// Record a VOICE utterance edit — always its own boundary, labeled with
    /// the utterance text (truncated). The dictated phrase undoes as one unit.
    pub fn record_voice(&mut self, doc: &str, cursor: usize, utterance: &str, now_ms: u64) {
        if self.entries[self.idx].doc == doc { return; }
        let mut label = utterance.trim().to_string();
        if label.chars().count() > 40 {
            label = label.chars().take(37).collect::<String>() + "…";
        }
        if label.is_empty() { label = "voice".into(); }
        self.commit(doc, cursor, &format!("🎤 {}", label), true);
        // a voice unit is closed; the next typed char starts fresh
        self.last_kind = None;
        self.last_edit_ms = now_ms;
    }

    /// Commit `doc` either as a new boundary entry or by replacing the current
    /// entry (coalesce). New boundaries drop the redo tail and enforce the cap.
    fn commit(&mut self, doc: &str, cursor: usize, label: &str, boundary: bool) {
        if boundary {
            // drop any redo tail beyond the current index
            self.entries.truncate(self.idx + 1);
            self.entries.push(Snapshot { doc: doc.to_string(), cursor, label: label.to_string() });
            // enforce cap (drop oldest)
            if self.entries.len() > self.cap {
                let overflow = self.entries.len() - self.cap;
                self.entries.drain(0..overflow);
            }
            self.idx = self.entries.len() - 1;
        } else {
            // coalesce: update the current entry to the latest state
            let e = &mut self.entries[self.idx];
            e.doc = doc.to_string();
            e.cursor = cursor;
            // keep the entry's original label (the unit's kind), don't relabel
        }
    }

    /// Step back one unit. Returns the snapshot to restore, or None.
    pub fn undo(&mut self) -> Option<Snapshot> {
        if self.idx == 0 { return None; }
        self.idx -= 1;
        self.last_kind = None; // an explicit undo closes any open coalescing
        Some(self.entries[self.idx].clone())
    }

    /// Step forward one unit. Returns the snapshot to restore, or None.
    pub fn redo(&mut self) -> Option<Snapshot> {
        if self.idx + 1 >= self.entries.len() { return None; }
        self.idx += 1;
        self.last_kind = None;
        Some(self.entries[self.idx].clone())
    }

    /// v0.1.695: peek at entry `index`'s doc WITHOUT moving the history
    /// index — for the rail's scrub-preview (hover a tick to preview that
    /// state read-only before committing with goto). None if out of range.
    pub fn peek(&self, index: usize) -> Option<&Snapshot> {
        self.entries.get(index)
    }

    /// v0.1.694: jump directly to entry `index` (for the visual timeline
    /// rail — click a tick to land on that state). Clamps; returns the
    /// snapshot to restore, or None if already there / out of range.
    pub fn goto(&mut self, index: usize) -> Option<Snapshot> {
        if index >= self.entries.len() || index == self.idx { return None; }
        self.idx = index;
        self.last_kind = None;
        Some(self.entries[self.idx].clone())
    }

    /// Labels of all entries oldest→newest, with the current marked — for the
    /// visual timeline rail. Wire: "label|cur" per entry, '\n'-joined.
    pub fn ticks_wire(&self) -> String {
        self.entries.iter().enumerate()
            .map(|(i, e)| format!("{}|{}", e.label.replace(['\n', '|'], " "),
                if i == self.idx { 1 } else { 0 }))
            .collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_a_word_then_undo_removes_the_word() {
        let mut h = History::new("", 0);
        // type "cat" quickly (same word unit → coalesces into one entry)
        h.record("c", 1, Some('c'), 100);
        h.record("ca", 2, Some('a'), 150);
        h.record("cat", 3, Some('t'), 200);
        // one boundary beyond the initial "open" entry
        assert!(h.can_undo());
        let u = h.undo().unwrap();
        assert_eq!(u.doc, "", "undo removes the whole word unit");
        assert!(!h.can_undo());
    }

    #[test]
    fn pause_splits_into_two_undo_units() {
        let mut h = History::new("", 0);
        h.record("a", 1, Some('a'), 100);
        h.record("ab", 2, Some('b'), 150);
        // long pause then more typing → new unit
        h.record("abc", 3, Some('c'), 2000);
        h.record("abcd", 4, Some('d'), 2050);
        // undo once → back to "ab"
        assert_eq!(h.undo().unwrap().doc, "ab");
        // undo again → back to ""
        assert_eq!(h.undo().unwrap().doc, "");
    }

    #[test]
    fn redo_restores() {
        let mut h = History::new("", 0);
        h.record("x", 1, Some('x'), 100);
        h.record("xy", 2, Some('y'), 2000); // boundary (pause)
        assert_eq!(h.undo().unwrap().doc, "x");
        assert_eq!(h.redo().unwrap().doc, "xy");
        assert!(!h.can_redo());
    }

    #[test]
    fn new_edit_after_undo_truncates_redo() {
        let mut h = History::new("", 0);
        h.record("a", 1, Some('a'), 100);
        h.record("ab", 2, Some('b'), 2000); // boundary
        h.undo(); // back to "a"
        // type something new → redo tail ("ab") is dropped
        h.record("aZ", 2, Some('Z'), 4000);
        assert!(!h.can_redo());
        assert_eq!(h.undo().unwrap().doc, "a");
    }

    #[test]
    fn voice_utterance_is_one_labeled_unit() {
        let mut h = History::new("Hello ", 6);
        h.record_voice("Hello world from voice", 22, "world from voice", 1000);
        let u = h.undo().unwrap();
        assert_eq!(u.doc, "Hello ", "the whole dictated phrase undoes at once");
        // label carries the utterance
        h.redo();
        assert!(h.current().label.contains("world from voice"));
        assert!(h.current().label.contains('🎤'));
    }

    #[test]
    fn voice_label_truncated() {
        let mut h = History::new("", 0);
        let long = "this is a very long dictated utterance that should be truncated nicely";
        h.record_voice(long, long.len(), long, 100);
        assert!(h.current().label.chars().count() <= 45);
        assert!(h.current().label.contains('…'));
    }

    #[test]
    fn ticks_wire_marks_current() {
        let mut h = History::new("", 0);
        h.record("a", 1, Some('a'), 100);
        h.record("ab", 2, Some('b'), 2000);
        let w = h.ticks_wire();
        let lines: Vec<&str> = w.lines().collect();
        assert_eq!(lines.len(), 3); // open + 2 units
        assert!(lines.last().unwrap().ends_with("|1"), "newest is current");
    }

    #[test]
    fn peek_does_not_move_index() {
        let mut h = History::new("", 0);
        h.record("a", 1, Some('a'), 100);
        h.record("ab", 2, Some('b'), 2000);
        let before = h.ticks_wire();
        assert_eq!(h.peek(0).unwrap().doc, "");
        assert_eq!(h.peek(1).unwrap().doc, "a");
        assert!(h.peek(99).is_none());
        // index unchanged after peeking
        assert_eq!(h.ticks_wire(), before);
        assert!(!h.can_redo()); // still at the newest
    }

    #[test]
    fn goto_jumps_to_entry() {
        let mut h = History::new("", 0);
        h.record("a", 1, Some('a'), 100);
        h.record("ab", 2, Some('b'), 2000);  // entry 2
        h.record("abc", 3, Some('c'), 4000); // entry 3
        // jump back to entry 1 ("a")
        assert_eq!(h.goto(1).unwrap().doc, "a");
        // jump forward to entry 3
        assert_eq!(h.goto(3).unwrap().doc, "abc");
        // same index → None
        assert!(h.goto(3).is_none());
        // out of range → None
        assert!(h.goto(99).is_none());
    }

    #[test]
    fn typing_then_deleting_are_separate_units() {
        let mut h = History::new("", 0);
        h.record("ab", 2, Some('b'), 100); // (open had "", first char 'a' coalesced? start fresh)
        h.record("a", 1, None, 150);       // delete → boundary (kind change)
        // undo → back to "ab"
        assert_eq!(h.undo().unwrap().doc, "ab");
    }

    #[test]
    fn no_op_record_ignored() {
        let mut h = History::new("hi", 2);
        h.record("hi", 2, None, 100);
        assert!(!h.can_undo());
    }
}
