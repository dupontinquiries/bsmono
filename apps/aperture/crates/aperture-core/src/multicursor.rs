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
//! Selections are byte offsets. Insert three characters at offset 10 and every
//! cursor after 10 is now three bytes wrong. Apply the edits in **descending**
//! order and the ones not yet applied are all *before* the edit point, so their
//! offsets are still valid when their turn comes.
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
        let grow = text.len() as isize;
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
    fn an_empty_set_is_not_representable() {
        // A buffer with no cursor has nowhere to type, and every caller would
        // need to handle it. One caret at zero is the honest default.
        let c = Cursors::new(vec![]);
        assert_eq!(c.len(), 1);
        assert_eq!(c.primary().head, 0);
    }
}
