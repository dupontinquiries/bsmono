//! notebook_cells — the notebook pane's per-cell state: what a block classified as, and
//! whether the operator has folded it.
//!
//! Two things the pane needed and did not have.
//!
//! ## The classification was recomputed every frame
//!
//! `notebook::classify` is not cheap — it tries a markdown table, then CSV, then TSV, then
//! whitespace-aligned columns, then two key/value forms, over the block's whole output. The
//! pane called it inside the scroll loop, for every block, on every frame. A session with
//! forty blocks and a few hundred kilobytes of build log re-parsed all of it sixty times a
//! second to draw a list that had not changed. A block's output only grows while it is the
//! running one, so the cache key is the block's id and the length of its output: an
//! appended byte re-classifies, a repaint does not.
//!
//! Length rather than a hash of the bytes: the output of a finished block does not change
//! in place, and hashing it every frame is the cost the cache exists to avoid. The one case
//! length misses — an output edited to the same length — cannot happen to a captured
//! transcript.
//!
//! ## Cells could not be folded
//!
//! `TODO_notebook_view.md` names "touch gestures compartmentalize cells (collapse/expand/
//! pin)" as step 3. Collapse is the useful half and is nearly free: a set of folded ids,
//! and a fold is remembered per block so scrolling away and back does not unfold it. A
//! folded cell still shows its command and a line saying what is inside, because a fold
//! that hides what it hid is a row you have to open to identify.
//!
//! Pure: no egui, no clock. The pane owns one of these and asks it.

use crate::notebook::Structured;

/// A cached classification, and what it was computed from.
#[derive(Debug, Clone)]
struct Cached {
    /// The output length the classification was computed at.
    len: usize,
    what: Structured,
}

/// Per-cell state for the notebook pane.
#[derive(Debug, Default)]
pub struct NotebookCells {
    cache: std::collections::HashMap<u32, Cached>,
    folded: std::collections::HashSet<u32>,
    /// Blocks the operator pinned, in the order they were pinned.
    ///
    /// A `Vec`, not a set: pins are shown together at the top of the pane and the order has
    /// to be stable, while a `HashSet` would reshuffle them whenever the map rehashed —
    /// which looks like the pane losing track of what you pinned.
    pinned: Vec<u32>,
    /// How many times a classification was computed rather than reused, for the test that
    /// pins the cache actually caching.
    misses: u64,
}

impl NotebookCells {
    pub fn new() -> NotebookCells {
        NotebookCells::default()
    }

    /// The classification for block `id` with this `output`, computing it only when the
    /// output has grown (or is new).
    pub fn classify(&mut self, id: u32, output: &str) -> &Structured {
        let len = output.len();
        let fresh = match self.cache.get(&id) {
            Some(c) if c.len == len => false,
            _ => true,
        };
        if fresh {
            self.misses += 1;
            let what = crate::notebook::classify(output);
            self.cache.insert(id, Cached { len, what });
        }
        &self.cache.get(&id).expect("just inserted").what
    }

    /// Blocks that no longer exist are forgotten — a session that scrolls past its block
    /// cap would otherwise keep every classification it ever made.
    pub fn retain_ids(&mut self, live: impl Fn(u32) -> bool) {
        self.cache.retain(|id, _| live(*id));
        self.folded.retain(|id| live(*id));
        // A pin dies with its block, like a fold. Scrollback is finite, and a pin surviving
        // the block it points at would leave the pane offering to show a cell that no
        // longer exists.
        self.pinned.retain(|id| live(*id));
    }

    pub fn is_pinned(&self, id: u32) -> bool {
        self.pinned.contains(&id)
    }

    /// Pin or unpin. A newly pinned block goes to the END, so the pinned strip reads in the
    /// order they were chosen rather than in block order — the operator's sequence is the
    /// information, and block order they already have from the pane itself.
    pub fn toggle_pin(&mut self, id: u32) {
        match self.pinned.iter().position(|p| *p == id) {
            Some(i) => {
                self.pinned.remove(i);
            }
            None => self.pinned.push(id),
        }
    }

    /// The pinned blocks, oldest pin first.
    pub fn pinned(&self) -> &[u32] {
        &self.pinned
    }

    /// PINNING UNFOLDS. A pinned cell is one the operator wants to keep looking at, and a
    /// pinned-and-folded cell is a row that says only "42 lines hidden" in the one place
    /// meant for what matters. Unpinning does NOT re-fold: it was open when they pinned it
    /// and putting it back would undo a state they may have changed since.
    pub fn pin_and_show(&mut self, id: u32) {
        if !self.is_pinned(id) {
            self.pinned.push(id);
        }
        self.folded.remove(&id);
    }

    pub fn is_folded(&self, id: u32) -> bool {
        self.folded.contains(&id)
    }

    pub fn toggle_fold(&mut self, id: u32) {
        if !self.folded.remove(&id) {
            self.folded.insert(id);
        }
    }

    /// Fold or unfold everything the pane is showing — the header's one control, for a
    /// long session where folding forty cells by hand is not a thing anyone does.
    pub fn set_all(&mut self, ids: impl Iterator<Item = u32>, folded: bool) {
        if folded {
            self.folded.extend(ids);
        } else {
            self.folded.clear();
        }
    }

    pub fn folded_count(&self) -> usize {
        self.folded.len()
    }

    /// How many classifications were actually computed. For tests and nothing else.
    pub fn computed(&self) -> u64 {
        self.misses
    }
}

/// One line describing what a folded cell is hiding, so a fold is still identifiable.
pub fn fold_summary(what: &Structured, output: &str) -> String {
    match what {
        Structured::Table(t) => format!("table — {} rows \u{d7} {} cols", t.rows.len(), t.headers.len()),
        Structured::Map(pairs) => format!("key/value — {} pairs", pairs.len()),
        Structured::Plain => {
            let lines = output.lines().filter(|l| !l.trim().is_empty()).count();
            match lines {
                0 => "no output".to_string(),
                1 => "1 line".to_string(),
                n => format!("{n} lines"),
            }
        }
    }
}

#[cfg(test)]
mod tests {

    /// Pins are the operator's sequence, so they keep the order they were made in — a
    /// `HashSet` would reshuffle them on a rehash, which looks like the pane losing track.
    #[test]
    fn pins_keep_the_order_they_were_made_in() {
        let mut c = NotebookCells::new();
        c.toggle_pin(7);
        c.toggle_pin(2);
        c.toggle_pin(9);
        assert_eq!(c.pinned(), &[7, 2, 9], "block order, not pin order");
        assert!(c.is_pinned(2) && !c.is_pinned(3));

        // Unpinning removes just that one and leaves the rest in order.
        c.toggle_pin(2);
        assert_eq!(c.pinned(), &[7, 9]);
        // And re-pinning puts it at the END: it is the most recent choice.
        c.toggle_pin(2);
        assert_eq!(c.pinned(), &[7, 9, 2]);
    }

    /// A pinned-and-folded cell is a row saying "42 lines hidden" in the one place meant for
    /// what matters.
    #[test]
    fn pinning_unfolds_and_unpinning_does_not_refold() {
        let mut c = NotebookCells::new();
        c.toggle_fold(4);
        assert!(c.is_folded(4));

        c.pin_and_show(4);
        assert!(c.is_pinned(4));
        assert!(!c.is_folded(4), "a pinned cell showing nothing is the wrong kind of pin");

        // Unpinning leaves it open. It was open when they pinned it, and re-folding would
        // undo a state they may have changed since.
        c.toggle_pin(4);
        assert!(!c.is_pinned(4));
        assert!(!c.is_folded(4), "unpinning re-folded a cell the operator had opened");

        // Pinning twice is not two pins.
        c.pin_and_show(5);
        c.pin_and_show(5);
        assert_eq!(c.pinned(), &[5]);
    }

    /// Scrollback is finite. A pin outliving its block would leave the pane offering to show
    /// a cell that no longer exists.
    #[test]
    fn a_pin_dies_with_its_block() {
        let mut c = NotebookCells::new();
        c.toggle_pin(1);
        c.toggle_pin(2);
        c.toggle_fold(2);
        c.classify(2, "a\tb\n");

        // Block 1 scrolled out of history.
        c.retain_ids(|id| id != 1);
        assert_eq!(c.pinned(), &[2], "a pin survived its block");
        assert!(c.is_folded(2), "an unrelated fold was dropped");

        // Everything gone.
        c.retain_ids(|_| false);
        assert!(c.pinned().is_empty() && c.folded_count() == 0);
    }
    use super::*;

    #[test]
    fn a_block_is_classified_once_and_again_only_when_its_output_grows() {
        let mut cells = NotebookCells::new();
        let out = "a,b\n1,2\n3,4\n";
        assert!(matches!(cells.classify(1, out), Structured::Table(_)));
        assert_eq!(cells.computed(), 1);
        for _ in 0..60 {
            let _ = cells.classify(1, out);
        }
        assert_eq!(cells.computed(), 1, "a repaint must not re-parse the output");
        // the running block appends
        let grown = format!("{out}5,6\n");
        let _ = cells.classify(1, &grown);
        assert_eq!(cells.computed(), 2);
        // a different block is its own entry
        let _ = cells.classify(2, "plain prose here");
        assert_eq!(cells.computed(), 3);
        assert!(matches!(cells.classify(2, "plain prose here"), Structured::Plain));
        assert_eq!(cells.computed(), 3);
    }

    #[test]
    fn folds_are_per_block_and_survive_until_the_block_does() {
        let mut cells = NotebookCells::new();
        assert!(!cells.is_folded(7));
        cells.toggle_fold(7);
        assert!(cells.is_folded(7) && !cells.is_folded(8));
        cells.toggle_fold(7);
        assert!(!cells.is_folded(7));
        cells.set_all([1u32, 2, 3].into_iter(), true);
        assert_eq!(cells.folded_count(), 3);
        cells.set_all(std::iter::empty(), false);
        assert_eq!(cells.folded_count(), 0);
        // a dropped block takes its state with it
        cells.toggle_fold(1);
        let _ = cells.classify(1, "x");
        let _ = cells.classify(2, "y");
        cells.retain_ids(|id| id == 2);
        assert!(!cells.is_folded(1));
        let before = cells.computed();
        let _ = cells.classify(2, "y");
        assert_eq!(cells.computed(), before, "the surviving block kept its classification");
        let _ = cells.classify(1, "x");
        assert_eq!(cells.computed(), before + 1, "the dropped one is recomputed if it returns");
    }

    #[test]
    fn a_folded_cell_says_what_it_is_hiding() {
        let mut cells = NotebookCells::new();
        let table = "a,b\n1,2\n3,4\n";
        assert_eq!(fold_summary(cells.classify(1, table), table), "table — 2 rows × 2 cols");
        let kv = "HOME=/home/k\nSHELL=/bin/sh\nTERM=xterm\n";
        let s = fold_summary(cells.classify(2, kv), kv);
        assert!(s.starts_with("key/value — 3 pairs"), "{s}");
        assert_eq!(fold_summary(&Structured::Plain, "one\ntwo\n\nthree\n"), "3 lines");
        assert_eq!(fold_summary(&Structured::Plain, "just this"), "1 line");
        assert_eq!(fold_summary(&Structured::Plain, "   \n"), "no output");
    }
}
