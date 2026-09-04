//! multicursor — milestone 4: `Selection` becomes `Vec<Selection>` and edits
//! fan out.
//!
//! The seam DESIGN.md already cut. What it does not say, because it is the part
//! you only find by writing it, is that **the hard problem is not holding
//! several cursors — it is that every edit invalidates every other cursor's
//! offsets.**
//!
//! ## Apply back-to-front, always
//!
//! Selections are **char indices**, the same unit `Op`, `Buffer` and `Editor`
//! use — this module's prose used to say "byte offsets", and one line of its
//! code believed it (see `after_insert`). Insert three characters at index 10
//! and every cursor after 10 is now three characters wrong. Apply the edits in
//! **descending** order and the ones not yet applied are all *before* the edit
//! point, so their indices are still valid when their turn comes.
//!
//! Front-to-back would need every later cursor shifted after every edit — the
//! same answer, arrived at with an O(n²) correction pass and one off-by-one per
//! multi-byte character. Ordering is the cheaper correctness.
//!
//! ## Overlaps must merge before anything is applied
//!
//! Two cursors inside one word, both asked to delete it, would delete it twice
//! — the second deletion eating whatever moved into place. Merging overlapping
//! selections first makes that unrepresentable rather than guarded against.
//!
//! ## The primary cursor is the one the viewport follows
//!
//! With eight cursors on screen, "scroll to the cursor" is ambiguous, and
//! scrolling to all of them is scrolling to none. The primary is the one that
//! was there first — the one the user placed deliberately before adding the
//! rest.

use crate::buffer::Buffer;
use crate::editor::Selection;

/// Several cursors, kept sorted and non-overlapping.
///
/// The invariants are maintained on construction rather than checked at use:
/// an unsorted or overlapping set is not a state any caller should have to
/// reason about.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cursors {
    sels: Vec<Selection>,
    /// Index into `sels` of the cursor the viewport follows.
    primary: usize,
}

impl Cursors {
    /// One cursor — the ordinary case, and the one every existing call site
    /// still means.
    pub fn single(s: Selection) -> Cursors {
        Cursors { sels: vec![s], primary: 0 }
    }

    /// Build from many, normalising as it goes.
    pub fn new(mut sels: Vec<Selection>) -> Cursors {
        if sels.is_empty() {
            return Cursors::single(Selection::caret(0));
        }
        // The first one given is the primary: it is the cursor the user placed
        // before adding the rest, so it is the one they are looking at.
        let primary_pos = sels[0].head;
        sels.sort_by_key(|s| (s.min(), s.max()));
        let merged = merge_overlaps(sels);
        let primary = merged.iter().position(|s| s.min() <= primary_pos && primary_pos <= s.max())
            .unwrap_or(0);
        Cursors { sels: merged, primary }
    }

    pub fn len(&self) -> usize { self.sels.len() }
    pub fn is_empty(&self) -> bool { self.sels.is_empty() }
    pub fn all(&self) -> &[Selection] { &self.sels }
    pub fn is_multi(&self) -> bool { self.sels.len() > 1 }

    /// The cursor the viewport follows.
    ///
    /// With eight cursors on screen "scroll to the cursor" is ambiguous, and
    /// scrolling to all of them is scrolling to none.
    pub fn primary(&self) -> Selection { self.sels[self.primary] }

    /// Add a cursor, merging if it lands inside an existing one.
    pub fn add(&mut self, s: Selection) {
        let primary_pos = self.primary().head;
        let mut next = self.sels.clone();
        next.push(s);
        next.sort_by_key(|x| (x.min(), x.max()));
        self.sels = merge_overlaps(next);
        self.primary = self.sels.iter()
            .position(|x| x.min() <= primary_pos && primary_pos <= x.max())
            .unwrap_or(0);
    }

    /// Collapse back to one cursor — what Escape does.
    ///
    /// Keeps the PRIMARY rather than the first or last, so Escape returns the
    /// user to where they started rather than to wherever the last added
    /// cursor happened to be.
    pub fn collapse(&mut self) {
        let p = self.primary();
        self.sels = vec![p];
        self.primary = 0;
    }

    /// Edits for inserting `text` at every cursor, in apply order.
    ///
    /// Descending, so each edit's offsets are still valid when its turn comes.
    pub fn insert_edits(&self, text: &str) -> Vec<Edit> {
        let mut e: Vec<Edit> = self.sels.iter()
            .map(|s| Edit { at: s.min(), remove: s.max() - s.min(), insert: text.to_string() })
            .collect();
        e.sort_by(|a, b| b.at.cmp(&a.at));
        e
    }

    /// Where the cursors end up after inserting `text` at each.
    ///
    /// Computed rather than re-derived from the buffer, because after a fan-out
    /// edit the buffer no longer tells you which cursor was which.
    pub fn after_insert(&self, text: &str) -> Cursors {
        // CHARS, not bytes. `Selection` is a char index everywhere else in the
        // crate — `Op::caret_after` counts `text.chars()`, `Buffer::insert`
        // takes a char index — and `text.len()` here made every cursor after
        // the first land N bytes past where the buffer actually put it for any
        // non-ASCII insertion. Invisible for ASCII, which is why 16 tests
        // passed over it; the module also had no `mod` line for months, so no
        // caller ever typed an accented character through it.
        let grow = text.chars().count() as isize;
        // SIGNED, and the test caught why: replacing a 4-byte selection with 2
        // bytes is a net shift of MINUS two, and a usize accumulator panics on
        // the subtraction. Every later cursor depends on this running total, so
        // an overflow here is not one bad cursor — it is the whole fan-out.
        let mut shift: isize = 0;
        let mut out = Vec::with_capacity(self.sels.len());
        // Ascending: each cursor moves by the net change of every edit BEFORE
        // it, which is only knowable front-to-back.
        for s in &self.sels {
            let removed = (s.max() - s.min()) as isize;
            let at = s.min() as isize + shift + grow;
            // Clamped at zero: a caret cannot precede the buffer, and a
            // negative offset would wrap to an enormous one on the cast back.
            out.push(Selection::caret(at.max(0) as usize));
            shift += grow - removed;
        }
        Cursors { sels: out, primary: self.primary }
    }
}

/// One buffer edit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Edit {
    pub at: usize,
    pub remove: usize,
    pub insert: String,
}

/// Merge selections that touch or overlap.
///
/// Touching counts, not just overlapping: two carets at the same offset are one
/// caret, and keeping both would type every character twice.
pub fn merge_overlaps(sorted: Vec<Selection>) -> Vec<Selection> {
    let mut out: Vec<Selection> = Vec::with_capacity(sorted.len());
    for s in sorted {
        match out.last_mut() {
            Some(prev) if s.min() <= prev.max() => {
                // Extend, preserving the direction of the earlier selection so
                // a shift-select that grew leftward does not silently flip.
                let (lo, hi) = (prev.min().min(s.min()), prev.max().max(s.max()));
                let forward = prev.head >= prev.anchor;
                *prev = if forward { Selection { anchor: lo, head: hi } }
                        else { Selection { anchor: hi, head: lo } };
            }
            _ => out.push(s),
        }
    }
    out
}

// ── deriving new cursors from the buffer ────────────────────────────────────
//
// Everything above is pure over `Selection` and never looks at text. The three
// gestures that actually CREATE cursors have to: "the next one like this" and
// "one line down from the lowest" are both questions about the document.
//
// They live here rather than in the shell because they are the part with an
// answer that can be wrong — off-by-one at a word boundary, a wrapped search
// that finds itself, a column that falls off the end of a shorter line — and
// the shell has no cheap way to test any of that. The shell's remaining job is
// one keybinding each.

/// Char index of `needle`'s next occurrence at or after `from`, wrapping once.
///
/// Wrapping rather than stopping: the gesture is "give me another one", and
/// stopping at the end of the file means the last few matches in a file are
/// only reachable by scrolling up and starting over.
///
/// O(n) in the document per call — it materialises the rope. That is a
/// keystroke-rate operation on a file that already fits in memory, and the
/// alternative (a rope-aware streaming matcher) is a real chunk of code to
/// write and test. Revisit if it ever shows up in a profile.
pub fn find_occurrence(buf: &Buffer, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let doc = buf.to_string();
    let from_byte = char_to_byte(&doc, from);
    doc[from_byte..]
        .find(needle)
        .map(|b| from_byte + b)
        .or_else(|| doc[..from_byte].find(needle))
        .map(|b| doc[..b].chars().count())
}

/// Every occurrence of `needle`, as selections, in document order.
pub fn all_occurrences(buf: &Buffer, needle: &str) -> Vec<Selection> {
    if needle.is_empty() {
        return Vec::new();
    }
    let doc = buf.to_string();
    let n = needle.chars().count();
    let mut out = Vec::new();
    let mut at = 0usize; // byte
    while let Some(rel) = doc[at..].find(needle) {
        let b = at + rel;
        let c = doc[..b].chars().count();
        out.push(Selection { anchor: c, head: c + n });
        // Step by one CHAR, not one byte, and not by the needle's length:
        // overlapping matches ("aa" in "aaa") are real matches, and a byte
        // step would land mid-codepoint and slice-panic on the next `find`.
        at = b + needle.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    out
}

/// The identifier-ish word containing or immediately before `at`.
///
/// "Immediately before" matters: the caret sits *after* the last character of
/// a word you just finished typing, and `Ctrl+D` there obviously means that
/// word. Expanding only forward would select nothing in the single most
/// common position for the caret to be in.
pub fn word_at(buf: &Buffer, at: usize) -> Selection {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let n = buf.len_chars();
    let at = at.min(n);
    let mut start = at;
    let mut end = at;
    while end < n && buf.char_at(end).map(is_word).unwrap_or(false) {
        end += 1;
    }
    while start > 0 && buf.char_at(start - 1).map(is_word).unwrap_or(false) {
        start -= 1;
    }
    Selection { anchor: start, head: end }
}

/// `Ctrl+D`: select the word under the caret, or add the next occurrence of
/// what is already selected.
///
/// Returns `None` when there is nothing to add — no word under a lone caret,
/// or a needle whose only occurrence is the one already selected. `None` is
/// "the gesture did nothing", which the shell can report; it is deliberately
/// not "here are your cursors back, unchanged", so a status message is
/// possible without diffing two `Cursors`.
pub fn add_next_occurrence(buf: &Buffer, c: &Cursors) -> Option<Cursors> {
    let p = c.primary();
    if p.is_caret() {
        let w = word_at(buf, p.head);
        if w.is_caret() {
            return None;
        }
        // First press only promotes the caret to a word — it does not also
        // jump somewhere else. Two things happening on one keystroke is how
        // you lose track of where you are.
        let mut sels = vec![w];
        sels.extend(c.all().iter().skip(1).copied());
        return Some(Cursors::new(sels));
    }
    let needle = buf.slice_to_string(p.min(), p.max());
    let last_end = c.all().iter().map(|s| s.max()).max().unwrap_or(p.max());
    let at = find_occurrence(buf, &needle, last_end)?;
    if c.all().iter().any(|s| s.min() == at) {
        return None; // wrapped onto a cursor we already have
    }
    let fresh = Selection { anchor: at, head: at + needle.chars().count() };
    // Fresh first: `Cursors::new` takes the primary from the head of the list,
    // and the viewport should follow the cursor that was just created —
    // otherwise Ctrl+D silently adds cursors off-screen.
    let mut sels = vec![fresh];
    sels.extend(c.all().iter().copied());
    Some(Cursors::new(sels))
}

/// `Ctrl+Shift+L`: a cursor on every occurrence of the current selection.
pub fn select_all_occurrences(buf: &Buffer, c: &Cursors) -> Option<Cursors> {
    let p = c.primary();
    let base = if p.is_caret() { word_at(buf, p.head) } else { p };
    if base.is_caret() {
        return None;
    }
    let needle = buf.slice_to_string(base.min(), base.max());
    let hits = all_occurrences(buf, &needle);
    if hits.len() < 2 {
        return None;
    }
    // The primary stays wherever the user already was, not the first hit in
    // the file: this gesture should not scroll you to the top.
    let mut sels = vec![base];
    sels.extend(hits);
    Some(Cursors::new(sels))
}

/// `Ctrl+Alt+Up` / `Ctrl+Alt+Down`: a caret one line above or below.
///
/// Grown from the OUTERMOST cursor in the direction of travel, so holding the
/// key extends the column downward instead of fighting over the same gap.
pub fn add_vertical(buf: &Buffer, c: &Cursors, dy: isize) -> Option<Cursors> {
    let heads = c.all().iter().map(|s| s.head);
    let edge = if dy < 0 { heads.min() } else { heads.max() }?;
    let p = buf.char_to_pos(edge);
    let line = p.line as isize + dy;
    if line < 0 || line as usize >= buf.len_lines() {
        return None;
    }
    let line = line as usize;
    // Clamp rather than skip: a short line gets a caret at its end, which is
    // what every editor does and what makes a column of cursors down a ragged
    // block behave.
    let col = p.col.min(buf.line_len(line));
    let at = buf.pos_to_char(crate::buffer::Pos::new(line, col));
    let mut sels = vec![Selection::caret(at)];
    sels.extend(c.all().iter().copied());
    Some(Cursors::new(sels))
}

fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(a: usize, h: usize) -> Selection { Selection { anchor: a, head: h } }

    #[test]
    fn edits_apply_back_to_front() {
        // Selections are byte offsets: insert at 10 and every cursor after 10
        // is wrong. Descending order means the not-yet-applied edits are all
        // BEFORE the edit point, so their offsets are still valid.
        let c = Cursors::new(vec![Selection::caret(5), Selection::caret(20),
                                  Selection::caret(50)]);
        let edits = c.insert_edits("x");
        let ats: Vec<usize> = edits.iter().map(|e| e.at).collect();
        assert_eq!(ats, vec![50, 20, 5], "edits must apply back to front");
    }

    #[test]
    fn a_selection_edit_removes_what_it_covers() {
        // Typing over a selection replaces it, at every cursor.
        let c = Cursors::new(vec![sel(2, 6), sel(10, 14)]);
        let edits = c.insert_edits("z");
        assert_eq!(edits[0], Edit { at: 10, remove: 4, insert: "z".into() });
        assert_eq!(edits[1], Edit { at: 2, remove: 4, insert: "z".into() });
    }

    #[test]
    fn overlapping_cursors_merge_before_any_edit_is_produced() {
        // Two cursors inside one word, both asked to delete it, would delete it
        // TWICE — the second eating whatever moved into place. Merging first
        // makes that unrepresentable.
        let c = Cursors::new(vec![sel(2, 8), sel(5, 12)]);
        assert_eq!(c.len(), 1);
        assert_eq!(c.all()[0].min(), 2);
        assert_eq!(c.all()[0].max(), 12);
    }

    #[test]
    fn two_carets_at_the_same_offset_are_one_caret() {
        // Keeping both would type every character twice, which is the most
        // confusing possible symptom.
        let c = Cursors::new(vec![Selection::caret(7), Selection::caret(7)]);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn adjacent_selections_merge_because_touching_counts() {
        // [2,5) and [5,9) share a boundary; leaving them separate produces two
        // edits at the same point and the second lands inside the first's
        // inserted text.
        let c = Cursors::new(vec![sel(2, 5), sel(5, 9)]);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn merging_preserves_the_direction_of_the_earlier_selection() {
        // A shift-select that grew leftward must not silently flip, or the next
        // shift-arrow extends the wrong end.
        let c = Cursors::new(vec![sel(8, 2), sel(6, 12)]);
        let s = c.all()[0];
        assert!(s.head < s.anchor, "a backward selection flipped: {s:?}");
        assert_eq!((s.min(), s.max()), (2, 12));
    }

    #[test]
    fn cursors_are_kept_in_document_order() {
        // Out of order, the descending sort in insert_edits would not actually
        // be descending by position.
        let c = Cursors::new(vec![Selection::caret(30), Selection::caret(5),
                                  Selection::caret(17)]);
        let heads: Vec<usize> = c.all().iter().map(|s| s.head).collect();
        assert_eq!(heads, vec![5, 17, 30]);
    }

    #[test]
    fn the_primary_is_the_cursor_the_user_placed_first() {
        // With eight cursors "scroll to the cursor" is ambiguous, and scrolling
        // to all of them is scrolling to none.
        let c = Cursors::new(vec![Selection::caret(30), Selection::caret(5)]);
        assert_eq!(c.primary().head, 30, "sorting stole the primary");
    }

    #[test]
    fn escape_returns_to_the_primary_not_to_the_first() {
        // Collapsing to the first would jump the user somewhere they did not
        // choose.
        let mut c = Cursors::new(vec![Selection::caret(40), Selection::caret(2)]);
        c.collapse();
        assert_eq!(c.len(), 1);
        assert_eq!(c.primary().head, 40);
    }

    #[test]
    fn adding_a_cursor_inside_an_existing_one_merges_rather_than_duplicating() {
        let mut c = Cursors::single(sel(4, 12));
        c.add(Selection::caret(8));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn adding_keeps_the_primary_pointing_at_the_same_place() {
        // The viewport must not jump because a cursor was added elsewhere.
        let mut c = Cursors::single(Selection::caret(50));
        c.add(Selection::caret(3));
        assert_eq!(c.primary().head, 50);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn cursors_move_by_the_net_change_of_every_edit_before_them() {
        // Each caret shifts by what earlier edits inserted MINUS what they
        // removed. Getting this wrong leaves later cursors progressively more
        // misplaced, which looks like drift rather than a bug.
        let c = Cursors::new(vec![Selection::caret(0), Selection::caret(10)]);
        let after = c.after_insert("ab");
        assert_eq!(after.all()[0].head, 2);
        assert_eq!(after.all()[1].head, 14, "the second cursor did not follow the first's insert");
    }

    #[test]
    fn replacing_selections_accounts_for_what_was_removed() {
        // Insert two, remove four: everything after moves back by two.
        let c = Cursors::new(vec![sel(0, 4), sel(10, 14)]);
        let after = c.after_insert("ab");
        assert_eq!(after.all()[0].head, 2);
        assert_eq!(after.all()[1].head, 10, "the removal was not accounted for");
    }

    #[test]
    fn every_cursor_becomes_a_caret_after_typing() {
        // Typing over a selection leaves a caret, not a selection of the text
        // just inserted — otherwise the next keystroke would replace it.
        let c = Cursors::new(vec![sel(2, 8)]);
        assert!(c.after_insert("x").all()[0].is_caret());
    }

    #[test]
    fn a_single_cursor_behaves_exactly_as_before() {
        // Every existing call site still means one cursor, and this layer must
        // be invisible to them.
        let c = Cursors::single(Selection::caret(9));
        assert!(!c.is_multi());
        assert_eq!(c.primary(), Selection::caret(9));
        assert_eq!(c.insert_edits("q").len(), 1);
    }

    #[test]
    fn cursors_shift_by_characters_not_bytes() {
        // The bug this module shipped with. `é` is one char and TWO bytes, so
        // a byte-counting `grow` moved every later cursor one past where
        // `Buffer::insert` (which takes a char index) actually put the text.
        // Every existing test used ASCII, where the two agree.
        let c = Cursors::new(vec![Selection::caret(0), Selection::caret(10)]);
        let after = c.after_insert("é");
        // 1 = its own insert; 12 = 10 + the first cursor's insert + its own.
        // Under the byte-counting version these were 2 and 14.
        assert_eq!(after.all()[0].head, 1);
        assert_eq!(after.all()[1].head, 12, "the second cursor shifted by bytes, not chars");
    }

    #[test]
    fn a_lone_caret_first_selects_the_word_it_sits_in() {
        // One keystroke, one visible effect: promote to a word. Also jumping
        // to another occurrence on the same press is how you lose your place.
        let b = Buffer::from_str("let total = total + 1;");
        let c = Cursors::single(Selection::caret(6)); // inside "total"
        let next = add_next_occurrence(&b, &c).expect("a word is under the caret");
        assert_eq!(next.len(), 1);
        assert_eq!((next.all()[0].min(), next.all()[0].max()), (4, 9));
    }

    #[test]
    fn the_caret_after_a_word_still_selects_that_word() {
        // Where the caret actually is when you finish typing a name. Expanding
        // only forward would select nothing in the commonest position.
        let b = Buffer::from_str("total = 1");
        let w = word_at(&b, 5); // just past the 'l'
        assert_eq!((w.min(), w.max()), (0, 5));
    }

    #[test]
    fn the_second_press_adds_the_next_occurrence_and_follows_it() {
        let b = Buffer::from_str("let total = total + 1;");
        let c = Cursors::new(vec![sel(4, 9)]); // "total"
        let next = add_next_occurrence(&b, &c).expect("a second 'total' exists");
        assert_eq!(next.len(), 2);
        assert_eq!(next.primary().min(), 12, "the viewport must follow the NEW cursor");
    }

    #[test]
    fn a_needle_with_only_one_occurrence_adds_nothing() {
        // `None` is "the gesture did nothing", so the shell can say so without
        // diffing two cursor sets.
        let b = Buffer::from_str("let total = 1;");
        let c = Cursors::new(vec![sel(4, 9)]);
        assert!(add_next_occurrence(&b, &c).is_none());
    }

    #[test]
    fn the_search_wraps_but_will_not_re_add_a_cursor_it_already_has() {
        // Wrapping is what makes the last matches in a file reachable; without
        // the second half of that rule, the wrap lands back on cursor one and
        // silently does nothing while looking like it worked.
        let b = Buffer::from_str("total total");
        let c = Cursors::new(vec![sel(0, 5), sel(6, 11)]);
        assert!(add_next_occurrence(&b, &c).is_none());
    }

    #[test]
    fn occurrences_can_overlap() {
        // "aa" occurs twice in "aaa". Stepping by the needle's length would
        // report one, and stepping by a byte would slice mid-codepoint.
        let b = Buffer::from_str("aaa");
        assert_eq!(all_occurrences(&b, "aa").len(), 2);
    }

    #[test]
    fn occurrence_indices_are_chars_not_bytes() {
        // The same unit error as `after_insert`, in the other direction: a
        // byte offset here would put the cursor inside a multi-byte character.
        let b = Buffer::from_str("héllo world héllo");
        let hits = all_occurrences(&b, "héllo");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].min(), 12, "byte offsets leaked into a char index");
    }

    #[test]
    fn select_all_occurrences_keeps_the_primary_where_the_user_was() {
        // Taking the first hit as primary scrolls you to the top of the file
        // as a side effect of a gesture that is about the word you're on.
        let b = Buffer::from_str("x = 1\ny = 2\nx = 3");
        let c = Cursors::single(Selection::caret(12)); // the second "x"
        let all = select_all_occurrences(&b, &c).expect("two x's");
        assert_eq!(all.len(), 2);
        assert_eq!(all.primary().min(), 12);
    }

    #[test]
    fn a_cursor_below_lands_on_the_same_column() {
        let b = Buffer::from_str("abcdef\nabcdef\n");
        let c = Cursors::single(Selection::caret(3)); // line 0, col 3
        let down = add_vertical(&b, &c, 1).expect("a line below exists");
        assert_eq!(down.len(), 2);
        assert_eq!(b.char_to_pos(down.all()[1].head), crate::buffer::Pos::new(1, 3));
    }

    #[test]
    fn a_short_line_gets_a_caret_at_its_end_rather_than_none() {
        // Skipping short lines makes a column of cursors down ragged code
        // silently incomplete; clamping is what every editor does.
        let b = Buffer::from_str("abcdef\nab\n");
        let c = Cursors::single(Selection::caret(5)); // line 0, col 5
        let down = add_vertical(&b, &c, 1).expect("a line below exists");
        assert_eq!(b.char_to_pos(down.all()[1].head), crate::buffer::Pos::new(1, 2));
    }

    #[test]
    fn vertical_growth_extends_from_the_outermost_cursor() {
        // Otherwise holding the key fights over the same gap: every press
        // would re-derive from the primary and merge into an existing cursor.
        let b = Buffer::from_str("a\nb\nc\nd\n");
        let c = Cursors::new(vec![Selection::caret(0), Selection::caret(2)]);
        let down = add_vertical(&b, &c, 1).expect("line 2 exists");
        assert_eq!(down.len(), 3);
        assert_eq!(b.char_to_pos(down.all()[2].head).line, 2);
    }

    #[test]
    fn there_is_no_line_above_the_first_one() {
        let b = Buffer::from_str("only\n");
        let c = Cursors::single(Selection::caret(2));
        assert!(add_vertical(&b, &c, -1).is_none());
    }

    #[test]
    fn an_empty_set_is_not_representable() {
        // A buffer with no cursor has nowhere to type, and every caller would
        // need to handle it. One caret at zero is the honest default.
        let c = Cursors::new(vec![]);
        assert_eq!(c.len(), 1);
        assert_eq!(c.primary().head, 0);
    }
}
