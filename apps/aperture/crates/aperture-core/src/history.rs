//! history — grouped undo/redo over [`Op`]s, with typing coalescing.
//!
//! A history entry is a **group** of ops (one user gesture). Undo applies the
//! group's inverses in reverse order; redo replays the group. Coalescing turns
//! a burst of single-char inserts (or backspaces) into one group so undo eats
//! a word, not a keystroke — the rule is: adjacent position, same character
//! class (word / space / punct), within a time window. Any cursor motion or
//! command in between calls [`History::seal`] and the next keystroke starts a
//! fresh group.
//!
//! Time is a parameter (`now_ms`), not a clock read — the coalescing rule is
//! pure and the tests below drive it with fake timestamps.

use crate::op::Op;

const COALESCE_WINDOW_MS: u64 = 900;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Space,
    Other,
}

fn class_of(text: &str) -> CharClass {
    match text.chars().next() {
        Some(c) if c.is_alphanumeric() || c == '_' => CharClass::Word,
        Some(c) if c.is_whitespace() => CharClass::Space,
        _ => CharClass::Other,
    }
}

#[derive(Default)]
pub struct History {
    undo: Vec<Vec<Op>>,
    redo: Vec<Vec<Op>>,
    /// Whether the top undo group may still absorb keystrokes.
    open: bool,
    last_ms: u64,
}

impl History {
    /// Record an already-applied op. `coalesce` is the caller's hint that this
    /// was a plain keystroke (single-char insert / single-char backspace);
    /// everything else always starts its own group.
    pub fn record(&mut self, op: Op, coalesce: bool, now_ms: u64) {
        self.redo.clear();
        if coalesce && self.open && now_ms.saturating_sub(self.last_ms) <= COALESCE_WINDOW_MS {
            if let Some(group) = self.undo.last_mut() {
                if let Some(merged) = try_merge(group.last_mut(), &op) {
                    if merged {
                        self.last_ms = now_ms;
                        return;
                    }
                }
            }
        }
        self.undo.push(vec![op]);
        self.open = coalesce;
        self.last_ms = now_ms;
    }

    /// Record a multi-op gesture (e.g. "replace selection" = delete + insert).
    pub fn record_group(&mut self, ops: Vec<Op>) {
        if ops.is_empty() {
            return;
        }
        self.redo.clear();
        self.undo.push(ops);
        self.open = false;
    }

    /// Like [`record_group`], but a multi-cursor typing analogue of
    /// [`record`]'s single-op coalescing: within the same window, APPENDS
    /// onto the open group instead of starting a new one, so a run of
    /// multi-cursor keystrokes undoes as one gesture instead of one per
    /// keystroke. No `try_merge` char-class check (unlike `record`) — a
    /// fan-out group interleaves several cursors' ops, so there is no single
    /// "last op" to compare a class against; appending whole groups is the
    /// right granularity here; splitting hairs finer than "still typing,
    /// still in the window" buys nothing a multi-cursor caller needs.
    pub fn record_group_coalescing(&mut self, ops: Vec<Op>, coalesce: bool, now_ms: u64) {
        if ops.is_empty() {
            return;
        }
        self.redo.clear();
        if coalesce && self.open && now_ms.saturating_sub(self.last_ms) <= COALESCE_WINDOW_MS {
            if let Some(group) = self.undo.last_mut() {
                group.extend(ops);
                self.last_ms = now_ms;
                return;
            }
        }
        self.undo.push(ops);
        self.open = coalesce;
        self.last_ms = now_ms;
    }

    /// Close the open coalescing group (cursor moved, command ran, focus lost).
    pub fn seal(&mut self) {
        self.open = false;
    }

    /// Ops to apply (already inverted, in application order) to undo one group.
    pub fn pop_undo(&mut self) -> Option<Vec<Op>> {
        let group = self.undo.pop()?;
        self.open = false;
        let inv: Vec<Op> = group.iter().rev().map(Op::invert).collect();
        self.redo.push(group);
        Some(inv)
    }

    /// Ops to apply (in application order) to redo one group.
    pub fn pop_redo(&mut self) -> Option<Vec<Op>> {
        let group = self.redo.pop()?;
        self.undo.push(group.clone());
        self.open = false;
        Some(group)
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
}

/// Merge `op` into `tail` when they extend each other. Returns Some(true) when
/// merged, Some(false) when adjacent-but-unmergeable, None when no tail.
fn try_merge(tail: Option<&mut Op>, op: &Op) -> Option<bool> {
    let tail = tail?;
    let ok = match (&mut *tail, op) {
        (Op::Insert { at, text }, Op::Insert { at: at2, text: t2 }) => {
            let contiguous = *at2 == *at + text.chars().count();
            if contiguous && class_of(t2) == class_of(text) {
                text.push_str(t2);
                true
            } else {
                false
            }
        }
        (Op::Delete { at, text }, Op::Delete { at: at2, text: t2 }) => {
            // Backspace run: each delete lands one char left of the last.
            let contiguous = *at2 + t2.chars().count() == *at;
            if contiguous && class_of(t2) == class_of(text) {
                *at = *at2;
                let mut merged = t2.clone();
                merged.push_str(text);
                *text = merged;
                true
            } else {
                false
            }
        }
        _ => false,
    };
    Some(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn typed(h: &mut History, b: &mut Buffer, at: usize, ch: char, t: u64) {
        let op = Op::Insert { at, text: ch.to_string() };
        op.apply(b);
        h.record(op, true, t);
    }

    #[test]
    fn a_typed_word_undoes_as_one_group() {
        let (mut h, mut b) = (History::default(), Buffer::default());
        for (i, ch) in "hello".chars().enumerate() {
            typed(&mut h, &mut b, i, ch, 100 + i as u64 * 50);
        }
        assert_eq!(h.undo_depth(), 1, "five keystrokes, one group");
        for op in h.pop_undo().unwrap() {
            op.apply(&mut b);
        }
        assert_eq!(b.to_string(), "");
    }

    #[test]
    fn class_change_and_time_gap_both_split_groups() {
        let (mut h, mut b) = (History::default(), Buffer::default());
        typed(&mut h, &mut b, 0, 'a', 0);
        typed(&mut h, &mut b, 1, ' ', 10); // word → space: new group
        typed(&mut h, &mut b, 2, 'b', 20);
        typed(&mut h, &mut b, 3, 'c', 20 + COALESCE_WINDOW_MS + 1); // stale: new group
        assert_eq!(h.undo_depth(), 4, "a | ' ' | b | c — every boundary split");
    }

    #[test]
    fn backspace_run_coalesces_and_restores() {
        let (mut h, mut b) = (History::default(), Buffer::from_str("word"));
        for i in (0..4).rev() {
            let op = Op::delete_range(&b, i, i + 1);
            op.apply(&mut b);
            h.record(op, true, 100 + (4 - i as u64) * 10);
        }
        assert_eq!(b.to_string(), "");
        assert_eq!(h.undo_depth(), 1);
        for op in h.pop_undo().unwrap() {
            op.apply(&mut b);
        }
        assert_eq!(b.to_string(), "word");
    }

    #[test]
    fn redo_replays_and_new_edit_clears_redo() {
        let (mut h, mut b) = (History::default(), Buffer::default());
        typed(&mut h, &mut b, 0, 'x', 0);
        for op in h.pop_undo().unwrap() {
            op.apply(&mut b);
        }
        for op in h.pop_redo().unwrap() {
            op.apply(&mut b);
        }
        assert_eq!(b.to_string(), "x");
        typed(&mut h, &mut b, 1, 'y', 5000);
        assert!(h.pop_redo().is_none(), "new edit invalidates redo");
    }
}
