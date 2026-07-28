//! msgview.rs — a virtualized view model for the message list.
//!
//! The render loop used to iterate `&mut Vec<Msg>` directly, which forced two bad choices: collect
//! mutations and apply them after the borrow ends, or clone every message to read it. Both are
//! symptoms of the renderer owning the model.
//!
//! A text editor does not do this. It keeps the document in one place and hands the painter a window of
//! cheap ROW DESCRIPTORS — enough to lay out and draw, nothing more — and takes edits back as events.
//! The same shape applies here:
//!
//! * A `Row` holds an INDEX and the few derived values a paint needs. It never holds the body.
//! * Rows are built for the VISIBLE RANGE only, so a ten-thousand-message conversation costs the same
//!   to render as a ten-message one.
//! * The painter emits `RowAction`; nothing mutates during the pass.
//!
//! The body is borrowed from the store at paint time, for the handful of rows actually on screen. That
//! is the difference between borrowing briefly and cloning everything.

/// What a row needs in order to be laid out and drawn.
///
/// Deliberately small and `Clone` — this is the thing that gets copied, and it must stay cheap enough
/// that copying it is never worth avoiding. The body is NOT here.
#[derive(Clone, PartialEq, Debug)]
pub struct Row {
    /// Index into the message store. The row's identity.
    pub idx: usize,
    pub is_user: bool,
    /// Rendered collapsed, with `label` shown instead of the body.
    pub collapsed: bool,
    /// Chevron text when collapsed. Empty otherwise, so an expanded row carries no string at all.
    pub label: String,
    /// Lines in the body, for scroll-height estimation without touching the text again.
    pub lines: usize,
}

/// What the painter observed. Applied by the owner after the pass, with no borrow held.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowAction { Expand(usize), Collapse(usize), Edit(usize), Rerun(usize), Delete(usize) }

/// The minimum a store must expose for rows to be built. Keeps this module free of the egui types.
pub trait MessageStore {
    fn len(&self) -> usize;
    fn is_user(&self, i: usize) -> bool;
    fn line_count(&self, i: usize) -> usize;
    /// A collapsed row's label. Only called for rows that will actually collapse.
    fn summary(&self, i: usize) -> String;
    fn is_empty(&self) -> bool { self.len() == 0 }
}

/// Build descriptors for a visible window.
///
/// `visible` is a half-open index range. Clamped rather than trusted: a scroll position can outrun the
/// store when a message is deleted mid-frame, and panicking on that would turn a race into a crash.
///
/// `summary` is only computed for rows that collapse — it is the one potentially expensive call here,
/// and computing it for expanded rows would pay for a string nobody displays.
pub fn build_rows(
    store: &impl MessageStore,
    visible: std::ops::Range<usize>,
    expanded: &std::collections::HashSet<usize>,
    collapse_above: usize,
) -> Vec<Row> {
    let n = store.len();
    let start = visible.start.min(n);
    let end = visible.end.min(n);
    (start..end).map(|i| {
        let lines = store.line_count(i);
        let is_latest = i + 1 == n;
        let collapsed = !store.is_user(i) && !is_latest && lines > collapse_above && !expanded.contains(&i);
        Row {
            idx: i,
            is_user: store.is_user(i),
            collapsed,
            label: if collapsed { store.summary(i) } else { String::new() },
            lines,
        }
    }).collect()
}

/// Apply the painter's actions to an expansion set. Returns whether anything changed.
///
/// Separated from the painter so the mutation happens in one place with no borrow outstanding — which is
/// what the whole arrangement exists to make possible.
pub fn apply_actions(expanded: &mut std::collections::HashSet<usize>, actions: &[RowAction]) -> bool {
    let mut changed = false;
    for a in actions {
        match a {
            RowAction::Expand(i) => changed |= expanded.insert(*i),
            RowAction::Collapse(i) => changed |= expanded.remove(i),
            _ => {}
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    struct Store { rows: Vec<(bool, usize)>, summaries_asked: std::cell::RefCell<Vec<usize>> }
    impl Store {
        fn new(rows: &[(bool, usize)]) -> Self {
            Store { rows: rows.to_vec(), summaries_asked: Default::default() }
        }
    }
    impl MessageStore for Store {
        fn len(&self) -> usize { self.rows.len() }
        fn is_user(&self, i: usize) -> bool { self.rows[i].0 }
        fn line_count(&self, i: usize) -> usize { self.rows[i].1 }
        fn summary(&self, i: usize) -> String {
            self.summaries_asked.borrow_mut().push(i);
            format!("summary of {i}")
        }
    }

    #[test]
    fn a_row_never_carries_the_body() {
        // The point of the whole module. If Row held the text, building rows would be a clone of the
        // conversation and virtualizing it would save nothing.
        let s = Store::new(&[(false, 500)]);
        let r = &build_rows(&s, 0..1, &HashSet::new(), 12)[0];
        assert_eq!(r.idx, 0);
        assert_eq!(r.lines, 500);
        // the only owned string is the collapsed label, and only when collapsed
        assert!(r.label.is_empty(), "the sole message is the latest, so it does not collapse");
    }

    #[test]
    fn only_the_VISIBLE_window_is_built() {
        // A ten-thousand-message conversation must cost the same to render as a ten-message one.
        let s = Store::new(&vec![(false, 100); 10_000]);
        let rows = build_rows(&s, 9990..10_000, &HashSet::new(), 12);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0].idx, 9990);
    }

    #[test]
    fn a_summary_is_computed_ONLY_for_rows_that_collapse() {
        // summary() is the one potentially expensive call. Computing it for expanded rows pays for a
        // string nobody displays, once per frame.
        let s = Store::new(&[(false, 500), (false, 3), (false, 500), (false, 500)]);
        let mut exp = HashSet::new();
        exp.insert(2usize);
        let _ = build_rows(&s, 0..4, &exp, 12);
        let asked = s.summaries_asked.borrow().clone();
        assert_eq!(asked, vec![0], "0 collapses; 1 is short, 2 is expanded, 3 is latest");
    }

    #[test]
    fn a_visible_range_past_the_end_is_clamped_not_a_panic() {
        // A scroll position can outrun the store when a message is deleted mid-frame. Panicking there
        // turns a race into a crash.
        let s = Store::new(&[(true, 1), (false, 2)]);
        assert_eq!(build_rows(&s, 0..99, &HashSet::new(), 12).len(), 2);
        assert!(build_rows(&s, 50..99, &HashSet::new(), 12).is_empty());
        assert!(build_rows(&Store::new(&[]), 0..10, &HashSet::new(), 12).is_empty());
    }

    #[test]
    fn user_messages_and_the_latest_never_collapse() {
        let s = Store::new(&[(true, 900), (false, 900), (false, 900)]);
        let rows = build_rows(&s, 0..3, &HashSet::new(), 12);
        assert!(!rows[0].collapsed, "user messages are the reader's own words");
        assert!(rows[1].collapsed);
        assert!(!rows[2].collapsed, "collapsing what someone is reading is hostile");
    }

    #[test]
    fn actions_are_applied_OUTSIDE_the_paint_and_report_whether_anything_moved() {
        // The mutation happens in one place with no borrow outstanding — which is what the arrangement
        // exists to make possible. Reporting "changed" lets the caller skip a repaint.
        let mut exp = HashSet::new();
        assert!(apply_actions(&mut exp, &[RowAction::Expand(3)]));
        assert!(!apply_actions(&mut exp, &[RowAction::Expand(3)]), "already expanded: nothing moved");
        assert!(apply_actions(&mut exp, &[RowAction::Collapse(3)]));
        // non-expansion actions are the owner's business, not this set's
        assert!(!apply_actions(&mut exp, &[RowAction::Edit(1), RowAction::Delete(2)]));
    }

    #[test]
    fn row_identity_survives_a_body_change_at_the_same_index() {
        // Editing a message must not invalidate the row's identity, or scroll position and expansion
        // state would jump every time a token streams in.
        let a = build_rows(&Store::new(&[(false, 10), (false, 900), (false, 5)]), 0..3, &HashSet::new(), 12);
        let b = build_rows(&Store::new(&[(false, 10), (false, 901), (false, 5)]), 0..3, &HashSet::new(), 12);
        assert_eq!(a[1].idx, b[1].idx);
        assert_eq!(a[1].collapsed, b[1].collapsed);
    }
}
