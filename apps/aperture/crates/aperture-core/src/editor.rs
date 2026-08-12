//! editor — the command surface the shell drives.
//!
//! Owns the buffer, one [`Selection`], the [`History`], and a monotonically
//! increasing `version` (the ghost-completion generation gate keys off it).
//! Selection is anchor/head as char indices — a value, not a widget — so the
//! multi-cursor extension is `Vec<Selection>` over the same command bodies,
//! not a redesign. Time enters only as a `now_ms` parameter (coalescing),
//! keeping every command deterministic under test.

use crate::buffer::{Buffer, Pos};
use crate::history::History;
use crate::op::Op;

pub const TAB_WIDTH: usize = 4;

/// anchor == head ⇒ a caret; otherwise the ordered pair is the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Selection { anchor: at, head: at }
    }
    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }
    pub fn min(&self) -> usize {
        self.anchor.min(self.head)
    }
    pub fn max(&self) -> usize {
        self.anchor.max(self.head)
    }
}

pub struct Editor {
    pub buffer: Buffer,
    pub sel: Selection,
    history: History,
    /// Bumps on every text mutation. The completion engine's generation gate.
    pub version: u64,
    /// Sticky column for vertical motion, in chars.
    goal_col: Option<usize>,
}

impl Editor {
    pub fn from_str(s: &str) -> Self {
        Editor {
            buffer: Buffer::from_str(s),
            sel: Selection::caret(0),
            history: History::default(),
            version: 0,
            goal_col: None,
        }
    }

    pub fn caret_pos(&self) -> Pos {
        self.buffer.char_to_pos(self.sel.head)
    }

    // ── mutation ────────────────────────────────────────────────────────────

    fn apply_record(&mut self, op: Op, coalesce: bool, now_ms: u64) {
        op.apply(&mut self.buffer);
        let caret = op.caret_after();
        self.history.record(op, coalesce, now_ms);
        self.sel = Selection::caret(self.buffer.clamp(caret));
        self.version += 1;
        self.goal_col = None;
    }

    /// If a selection exists, delete it (recorded) and leave the caret at its
    /// start. Returns the ops so callers composing a gesture can group them.
    fn delete_selection_op(&mut self) -> Option<Op> {
        if self.sel.is_caret() {
            return None;
        }
        let (a, b) = (self.sel.min(), self.sel.max());
        let op = Op::delete_range(&self.buffer, a, b);
        op.apply(&mut self.buffer);
        self.sel = Selection::caret(a);
        self.version += 1;
        Some(op)
    }

    /// Type `text` at the caret (replacing any selection). Single graphemes
    /// coalesce in history; pastes and multi-char inserts do not.
    pub fn insert_text(&mut self, text: &str, now_ms: u64) {
        if text.is_empty() {
            return;
        }
        let deleted = self.delete_selection_op();
        let at = self.sel.head;
        let ins = Op::Insert { at, text: text.to_string() };
        match deleted {
            Some(del) => {
                ins.apply(&mut self.buffer);
                self.sel = Selection::caret(ins.caret_after());
                self.version += 1;
                self.history.record_group(vec![del, ins]);
                self.goal_col = None;
            }
            None => {
                let coalesce = text.chars().count() == 1 && text != "\n";
                self.apply_record(ins, coalesce, now_ms);
            }
        }
    }

    /// Enter: newline + copy of the current line's leading whitespace.
    pub fn newline(&mut self, now_ms: u64) {
        let line = self.buffer.char_to_pos(self.sel.min()).line;
        let text = self.buffer.line(line);
        let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let mut s = String::with_capacity(1 + indent.len());
        s.push('\n');
        s.push_str(&indent);
        self.insert_text(&s, now_ms);
    }

    /// Enter's smart form: between an unseparated `{` and `}` (the pair
    /// auto-pairs itself would have produced), split into three lines — the
    /// opener, an empty deeper-indented line with the caret, the closer at
    /// the OPENER's indent — instead of one plain newline that would leave
    /// the closer hugging the opener. One `insert_text` call (one undo
    /// step), then the caret is placed by hand: `insert_text` naturally
    /// lands it at the END of the inserted text (after the closer's line),
    /// so the middle line needs an explicit `Selection` afterward.
    /// Anything else (caret not between an adjacent `{}`) falls through to
    /// plain `newline`'s ordinary auto-indent.
    pub fn newline_smart(&mut self, now_ms: u64) {
        if !self.sel.is_caret() {
            return self.newline(now_ms);
        }
        let at = self.sel.head;
        let (prev, next) = (self.buffer.char_at(at.wrapping_sub(1)), self.buffer.char_at(at));
        if at == 0 || prev != Some('{') || next != Some('}') {
            return self.newline(now_ms);
        }
        let line = self.buffer.char_to_pos(at).line;
        let text = self.buffer.line(line);
        let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let inner = format!("{indent}{}", " ".repeat(TAB_WIDTH));
        let mut s = String::with_capacity(2 + indent.len() * 2 + inner.len());
        s.push('\n');
        s.push_str(&inner);
        s.push('\n');
        s.push_str(&indent);
        let caret_at = at + 1 + inner.chars().count();
        self.insert_text(&s, now_ms);
        self.sel = Selection::caret(caret_at);
    }

    pub fn insert_tab(&mut self, now_ms: u64) {
        let col = self.caret_pos().col;
        let n = TAB_WIDTH - (col % TAB_WIDTH);
        self.insert_text(&" ".repeat(n), now_ms);
    }

    pub fn backspace(&mut self, now_ms: u64) {
        if let Some(del) = self.delete_selection_op() {
            self.history.record_group(vec![del]);
            self.goal_col = None;
            return;
        }
        let h = self.sel.head;
        if h == 0 {
            return;
        }
        let op = Op::delete_range(&self.buffer, h - 1, h);
        let coalesce = !op_text(&op).contains('\n');
        self.apply_record(op, coalesce, now_ms);
    }

    pub fn delete_forward(&mut self, now_ms: u64) {
        if let Some(del) = self.delete_selection_op() {
            self.history.record_group(vec![del]);
            self.goal_col = None;
            return;
        }
        let h = self.sel.head;
        if h >= self.buffer.len_chars() {
            return;
        }
        let op = Op::delete_range(&self.buffer, h, h + 1);
        self.apply_record(op, false, now_ms);
    }

    /// The whole selected range (or caret line) as one recorded replacement —
    /// what ghost-completion acceptance and paste both are.
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str) {
        let mut ops = Vec::new();
        if start < end {
            let del = Op::delete_range(&self.buffer, start, end);
            del.apply(&mut self.buffer);
            ops.push(del);
        }
        let ins = Op::Insert { at: start, text: text.to_string() };
        ins.apply(&mut self.buffer);
        let caret = ins.caret_after();
        if !text.is_empty() {
            ops.push(ins);
        }
        self.history.record_group(ops);
        self.sel = Selection::caret(self.buffer.clamp(caret));
        self.version += 1;
        self.goal_col = None;
    }

    /// Apply many non-overlapping `(start, end, text)` replacements as ONE
    /// undo step — "replace all," not "replace, then replace, then replace
    /// N-1 more times." Ranges are sorted and applied HIGH-TO-LOW: an edit
    /// at a later offset never shifts an earlier one, so processing back to
    /// front means every range in the input can be computed once, up front,
    /// against the ORIGINAL text — no caller-side offset bookkeeping. Caret
    /// lands at the last (lowest-offset) replacement's end.
    pub fn replace_many(&mut self, mut ranges: Vec<(usize, usize, String)>, now_ms: u64) {
        if ranges.is_empty() {
            return;
        }
        let _ = now_ms; // batched edits don't coalesce with typing either way
        ranges.sort_by(|a, b| b.0.cmp(&a.0));
        let mut ops = Vec::with_capacity(ranges.len() * 2);
        let mut caret = self.sel.head;
        for (start, end, text) in &ranges {
            let (start, end) = (*start, *end);
            if start < end {
                let del = Op::delete_range(&self.buffer, start, end);
                del.apply(&mut self.buffer);
                ops.push(del);
            }
            let ins = Op::Insert { at: start, text: text.clone() };
            ins.apply(&mut self.buffer);
            caret = ins.caret_after();
            if !text.is_empty() {
                ops.push(ins);
            }
        }
        self.history.record_group(ops);
        self.sel = Selection::caret(self.buffer.clamp(caret));
        self.version += 1;
        self.goal_col = None;
    }

    pub fn undo(&mut self) {
        if let Some(ops) = self.history.pop_undo() {
            for op in &ops {
                op.apply(&mut self.buffer);
            }
            if let Some(last) = ops.last() {
                self.sel = Selection::caret(self.buffer.clamp(last.caret_after()));
            }
            self.version += 1;
            self.goal_col = None;
        }
    }

    pub fn redo(&mut self) {
        if let Some(ops) = self.history.pop_redo() {
            for op in &ops {
                op.apply(&mut self.buffer);
            }
            if let Some(last) = ops.last() {
                self.sel = Selection::caret(self.buffer.clamp(last.caret_after()));
            }
            self.version += 1;
            self.goal_col = None;
        }
    }

    // ── motion ──────────────────────────────────────────────────────────────

    fn set_head(&mut self, head: usize, extend: bool) {
        let head = self.buffer.clamp(head);
        self.sel = if extend {
            Selection { anchor: self.sel.anchor, head }
        } else {
            Selection::caret(head)
        };
        self.history.seal();
    }

    pub fn move_left(&mut self, extend: bool) {
        // Plain ← with a selection collapses to its start (the convention).
        if !extend && !self.sel.is_caret() {
            let m = self.sel.min();
            self.set_head(m, false);
        } else {
            self.set_head(self.sel.head.saturating_sub(1), extend);
        }
        self.goal_col = None;
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && !self.sel.is_caret() {
            let m = self.sel.max();
            self.set_head(m, false);
        } else {
            self.set_head(self.sel.head + 1, extend);
        }
        self.goal_col = None;
    }

    pub fn move_vertical(&mut self, dy: isize, extend: bool) {
        let pos = self.buffer.char_to_pos(self.sel.head);
        let goal = *self.goal_col.get_or_insert(pos.col);
        let line = if dy < 0 {
            pos.line.saturating_sub(dy.unsigned_abs())
        } else {
            (pos.line + dy as usize).min(self.buffer.len_lines().saturating_sub(1))
        };
        let head = self.buffer.pos_to_char(Pos::new(line, goal));
        self.set_head(head, extend);
        // set_head cleared nothing; keep goal sticky across repeated moves.
        self.goal_col = Some(goal);
    }

    pub fn move_home(&mut self, extend: bool) {
        // Smart home: first press → first non-blank, second → column 0.
        let pos = self.caret_pos();
        let text = self.buffer.line(pos.line);
        let first_nb = text.chars().take_while(|c| c.is_whitespace()).count();
        let col = if pos.col == first_nb { 0 } else { first_nb };
        let head = self.buffer.pos_to_char(Pos::new(pos.line, col));
        self.set_head(head, extend);
        self.goal_col = None;
    }

    pub fn move_end(&mut self, extend: bool) {
        let pos = self.caret_pos();
        let head = self.buffer.pos_to_char(Pos::new(pos.line, self.buffer.line_len(pos.line)));
        self.set_head(head, extend);
        self.goal_col = None;
    }

    pub fn move_word(&mut self, dir: isize, extend: bool) {
        let head = if dir < 0 {
            word_left(&self.buffer, self.sel.head)
        } else {
            word_right(&self.buffer, self.sel.head)
        };
        self.set_head(head, extend);
        self.goal_col = None;
    }

    pub fn move_doc(&mut self, to_end: bool, extend: bool) {
        let head = if to_end { self.buffer.len_chars() } else { 0 };
        self.set_head(head, extend);
        self.goal_col = None;
    }

    pub fn select_all(&mut self) {
        self.sel = Selection { anchor: 0, head: self.buffer.len_chars() };
        self.history.seal();
    }

    pub fn goto_line(&mut self, line: usize) {
        let head = self.buffer.pos_to_char(Pos::new(line, 0));
        self.set_head(head, false);
        self.goal_col = None;
    }

    pub fn selected_text(&self) -> String {
        self.buffer.slice_to_string(self.sel.min(), self.sel.max())
    }
}

fn op_text(op: &Op) -> &str {
    match op {
        Op::Insert { text, .. } | Op::Delete { text, .. } => text,
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_left(buf: &Buffer, mut i: usize) -> usize {
    // Skip any non-word run, then the word run — landing at the word's start.
    while i > 0 && !buf.char_at(i - 1).map(is_word).unwrap_or(false) {
        i -= 1;
    }
    while i > 0 && buf.char_at(i - 1).map(is_word).unwrap_or(false) {
        i -= 1;
    }
    i
}

fn word_right(buf: &Buffer, mut i: usize) -> usize {
    let n = buf.len_chars();
    while i < n && !buf.char_at(i).map(is_word).unwrap_or(false) {
        i += 1;
    }
    while i < n && buf.char_at(i).map(is_word).unwrap_or(false) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_selection_replacement_and_undo_are_one_gesture() {
        let mut e = Editor::from_str("hello world");
        e.sel = Selection { anchor: 6, head: 11 };
        e.insert_text("rust", 0);
        assert_eq!(e.buffer.to_string(), "hello rust");
        e.undo();
        assert_eq!(e.buffer.to_string(), "hello world");
        assert_eq!(e.sel.head, 11, "undo restores the replaced text and caret");
        e.redo();
        assert_eq!(e.buffer.to_string(), "hello rust");
    }

    #[test]
    fn newline_copies_indentation() {
        let mut e = Editor::from_str("    let x = 1;");
        e.move_end(false);
        e.newline(0);
        assert_eq!(e.buffer.to_string(), "    let x = 1;\n    ");
        assert_eq!(e.caret_pos(), Pos::new(1, 4));
    }

    #[test]
    fn sticky_column_survives_a_short_line() {
        let mut e = Editor::from_str("longer line\nab\nanother long");
        e.sel = Selection::caret(8); // col 8 on line 0
        e.move_vertical(1, false);
        assert_eq!(e.caret_pos(), Pos::new(1, 2), "clamped to short line");
        e.move_vertical(1, false);
        assert_eq!(e.caret_pos(), Pos::new(2, 8), "goal column restored");
    }

    #[test]
    fn word_motion_lands_on_boundaries() {
        let mut e = Editor::from_str("fn main_loop(x: u32)");
        e.move_word(1, false);
        assert_eq!(e.sel.head, 2); // after "fn"
        e.move_word(1, false);
        assert_eq!(e.sel.head, 12); // after "main_loop"
        e.move_word(-1, false);
        assert_eq!(e.sel.head, 3); // start of "main_loop"
    }

    #[test]
    fn smart_home_toggles_between_indent_and_column_zero() {
        let mut e = Editor::from_str("    body");
        e.move_end(false);
        e.move_home(false);
        assert_eq!(e.caret_pos().col, 4);
        e.move_home(false);
        assert_eq!(e.caret_pos().col, 0);
        e.move_home(false);
        assert_eq!(e.caret_pos().col, 4);
    }

    #[test]
    fn version_bumps_only_on_mutation() {
        let mut e = Editor::from_str("abc");
        let v = e.version;
        e.move_right(false);
        assert_eq!(e.version, v, "motion is not an edit");
        e.insert_text("x", 0);
        assert_eq!(e.version, v + 1);
    }

    #[test]
    fn tab_advances_to_next_stop() {
        let mut e = Editor::from_str("ab");
        e.move_end(false);
        e.insert_tab(0);
        assert_eq!(e.buffer.line_len(0), 4, "col 2 → pad 2 to reach stop 4");
    }
    #[test]
    fn newline_smart_splits_an_adjacent_brace_pair() {
        let mut e = Editor::from_str("fn main() {}\n");
        // Caret between { and }.
        e.sel = Selection::caret(11);
        assert_eq!((e.buffer.char_at(10), e.buffer.char_at(11)), (Some('{'), Some('}')));
        e.newline_smart(0);
        assert_eq!(e.buffer.to_string(), "fn main() {\n    \n}\n");
        // Caret lands on the empty deeper-indented middle line.
        let pos = e.caret_pos();
        assert_eq!((pos.line, pos.col), (1, 4));
    }

    #[test]
    fn newline_smart_preserves_the_openers_indent_for_the_closer() {
        let mut e = Editor::from_str("    if x {}\n");
        e.sel = Selection::caret(10); // between { and } on an indented line
        e.newline_smart(0);
        assert_eq!(e.buffer.to_string(), "    if x {\n        \n    }\n");
    }

    #[test]
    fn newline_smart_is_one_undo_step() {
        let mut e = Editor::from_str("fn f() {}\n");
        e.sel = Selection::caret(8);
        e.newline_smart(0);
        assert_eq!(e.buffer.to_string(), "fn f() {\n    \n}\n");
        e.undo();
        assert_eq!(e.buffer.to_string(), "fn f() {}\n", "one undo removes the whole split, not one line at a time");
    }

    #[test]
    fn newline_smart_falls_through_when_not_between_braces() {
        let mut e = Editor::from_str("let x = 1;\n");
        e.sel = Selection::caret(5);
        e.newline_smart(0);
        assert_eq!(e.buffer.to_string(), "let x\n = 1;\n", "ordinary newline, no split");

        let mut e2 = Editor::from_str("{}\n");
        e2.sel = Selection { anchor: 0, head: 1 }; // a real selection, not a caret
        e2.newline_smart(0);
        assert_eq!(e2.buffer.to_string(), "\n}\n", "a selection falls through too, never brace-splits");
    }

    #[test]
    fn replace_many_applies_all_ranges_correctly() {
        let mut e = Editor::from_str("cat cat cat\n");
        // Three non-overlapping "cat" -> "dog" replacements.
        let ranges = vec![
            (0usize, 3usize, "dog".to_string()),
            (4, 7, "dog".to_string()),
            (8, 11, "dog".to_string()),
        ];
        e.replace_many(ranges, 0);
        assert_eq!(e.buffer.to_string(), "dog dog dog\n");
    }

    #[test]
    fn replace_many_is_one_undo_step_for_every_match() {
        let mut e = Editor::from_str("aa aa aa aa\n");
        let ranges: Vec<(usize, usize, String)> = (0..4).map(|i| (i * 3, i * 3 + 2, "bb".to_string())).collect();
        e.replace_many(ranges, 0);
        assert_eq!(e.buffer.to_string(), "bb bb bb bb\n");
        e.undo();
        assert_eq!(e.buffer.to_string(), "aa aa aa aa\n", "ONE undo restores every replacement, not one at a time");
        e.redo();
        assert_eq!(e.buffer.to_string(), "bb bb bb bb\n", "and one redo replays all four");
    }

    #[test]
    fn replace_many_handles_different_length_replacement_text() {
        // Shrinking and growing replacements in the same batch — offsets
        // are computed against the ORIGINAL text (high-to-low application),
        // so a growing replacement at a lower offset can't corrupt a
        // higher-offset one that's already been applied.
        let mut e = Editor::from_str("x-y-z\n");
        let ranges = vec![
            (0usize, 1usize, "LONG".to_string()),  // x -> LONG (grows)
            (4, 5, "z".to_string()),                // z -> z (no-op length)
        ];
        e.replace_many(ranges, 0);
        assert_eq!(e.buffer.to_string(), "LONG-y-z\n");
    }

    #[test]
    fn replace_many_empty_is_a_true_noop() {
        let mut e = Editor::from_str("unchanged\n");
        e.replace_many(vec![], 0);
        assert_eq!(e.buffer.to_string(), "unchanged\n");
    }

}
