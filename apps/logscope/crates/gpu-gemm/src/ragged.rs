//! ragged.rs — advancing several sequences through one decode step.
//!
//! Colibri's `decode_batch.h` names the shape: *"`base` belongs to one sequence's KV state. Keeping
//! this arithmetic in a model-independent seam makes **ragged decode row ownership** directly
//! testable."*
//!
//! This engine already produces the sequences. A panel runs 3–4 members over a shared prefix and
//! `lanes.rs` says what happens next — *"two long-running director tasks serialize (the fan's
//! round-robin)"*. Each member decodes **alone, at m=1**, one after another, so the weights are read
//! once per member per token instead of once for all of them.
//!
//! `MATVEC_SMALL_Q4` already handles m≤8 by reading each nibble once and accumulating all m rows,
//! bit-exact per row. A 3–4 member panel sits inside that window exactly.
//!
//! ## Why a panel is a better batch than speculation
//!
//! Every row is a sequence someone asked for. There is no accept/reject, so no round is wasted; no
//! draft model to fit; and the batch size is whatever the panel is rather than something to tune.
//!
//! ## Why "ragged" is the hard part
//!
//! Members finish at different lengths. A batch that pads to the longest wastes the saving it exists
//! to create, and one that reorders rows loses the mapping back to whose KV cache is whose. So the
//! batch **shrinks**, and the row→member mapping is explicit rather than positional.

/// One member's decode state within a batch step.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Seat {
    /// Which panel member this row belongs to. Stable across steps even as rows are removed.
    pub member: usize,
    /// How many tokens this member's KV cache already holds — its `base`.
    pub pos: usize,
    /// Set when the member has produced its stop token or hit its cap.
    pub done: bool,
}

/// The live rows of a decode step, in row order.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Batch {
    seats: Vec<Seat>,
}

impl Batch {
    pub fn new(members: usize, start_pos: usize) -> Batch {
        Batch { seats: (0..members).map(|member| Seat { member, pos: start_pos, done: false }).collect() }
    }

    /// Rows that will be computed this step, in the order they occupy the batch.
    ///
    /// Excludes finished members: a done seat contributes no row, which is what makes the batch
    /// ragged rather than padded.
    pub fn live(&self) -> Vec<&Seat> { self.seats.iter().filter(|s| !s.done).collect() }

    /// `m` for this step — the number of rows to pass to the matvec.
    pub fn m(&self) -> usize { self.seats.iter().filter(|s| !s.done).count() }

    /// Which member owns row `r` of this step's output.
    ///
    /// Explicit rather than positional, because rows shift as members finish. A scatter that assumed
    /// row `i` belongs to member `i` would deliver the wrong token to the wrong sequence on the first
    /// step after anyone stops — silently, since both are valid tokens.
    pub fn owner(&self, row: usize) -> Option<usize> {
        self.seats.iter().filter(|s| !s.done).nth(row).map(|s| s.member)
    }

    /// The KV base for row `r`: how many rows that member's cache already holds.
    pub fn base(&self, row: usize) -> Option<usize> {
        self.seats.iter().filter(|s| !s.done).nth(row).map(|s| s.pos)
    }

    /// Record that every live row produced a token, advancing each member's position.
    pub fn advance(&mut self) {
        for s in self.seats.iter_mut() { if !s.done { s.pos += 1; } }
    }

    /// Mark a member finished. Its row disappears from the next step.
    pub fn finish(&mut self, member: usize) {
        if let Some(s) = self.seats.iter_mut().find(|s| s.member == member) { s.done = true; }
    }

    pub fn all_done(&self) -> bool { self.seats.iter().all(|s| s.done) }

    /// Positions of the live rows, for a kernel that takes a per-row base.
    pub fn bases(&self) -> Vec<usize> { self.live().iter().map(|s| s.pos).collect() }

    /// Should this step run batched, or fall back to one row at a time?
    ///
    /// Above `max_m` the q4 path leaves `MATVEC_SMALL_Q4` for the strided `LINEAR_Q4`, which re-reads
    /// the weights per tile and undoes the saving — so a batch too large is worse than no batch. And
    /// at m=1 there is nothing to amortise, so the ordinary path is exactly as good.
    pub fn should_batch(&self, max_m: usize) -> bool {
        let m = self.m();
        m > 1 && m <= max_m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_four_member_panel_batches_four_rows() {
        let b = Batch::new(4, 100);
        assert_eq!(b.m(), 4);
        assert!(b.should_batch(8), "a panel sits inside MATVEC_SMALL_Q4's window");
        assert_eq!(b.bases(), vec![100, 100, 100, 100], "a shared prefix, so equal bases");
    }

    #[test]
    fn a_finished_member_LEAVES_the_batch_rather_than_padding_it() {
        // Padding to the longest wastes the saving the batch exists to create.
        let mut b = Batch::new(4, 10);
        b.finish(1);
        assert_eq!(b.m(), 3);
        assert_eq!(b.live().len(), 3);
    }

    #[test]
    fn ownership_is_EXPLICIT_because_rows_shift_when_someone_finishes() {
        // The bug this prevents: after member 1 stops, row 1 belongs to member 2. A scatter assuming
        // row i belongs to member i would deliver member 2's token to member 1 — silently, since both
        // are valid tokens for their own sequences.
        let mut b = Batch::new(4, 0);
        assert_eq!(b.owner(1), Some(1));
        b.finish(1);
        assert_eq!(b.owner(1), Some(2), "row 1 is now member 2's");
        assert_eq!(b.owner(2), Some(3));
        assert_eq!(b.owner(3), None, "and there is no fourth row");
    }

    #[test]
    fn positions_DIVERGE_once_members_finish_at_different_lengths() {
        // The "ragged" part. Members that stopped keep their final position; the rest advance, so the
        // bases passed to the kernel differ per row.
        let mut b = Batch::new(3, 5);
        b.advance();
        b.finish(0);
        b.advance();
        b.advance();
        assert_eq!(b.bases(), vec![8, 8], "the two live members are at 8");
        // Member 0 stopped at 6 and stayed there — its KV cache is not extended.
        assert_eq!(b.owner(0), Some(1));
    }

    #[test]
    fn a_single_live_member_does_NOT_batch() {
        // At m=1 there is nothing to amortise, and the ordinary path is exactly as good — so a batch
        // step here is pure overhead.
        let mut b = Batch::new(3, 0);
        b.finish(0);
        b.finish(1);
        assert_eq!(b.m(), 1);
        assert!(!b.should_batch(8));
    }

    #[test]
    fn a_batch_LARGER_than_the_kernel_window_does_not_batch_either() {
        // Above m=8 the q4 path leaves MATVEC_SMALL_Q4 for the strided LINEAR_Q4, which re-reads the
        // weights per tile — so an oversized batch is worse than none. That threshold is the kernel's,
        // not a preference.
        let b = Batch::new(12, 0);
        assert_eq!(b.m(), 12);
        assert!(!b.should_batch(8));
        assert!(b.should_batch(16), "and it batches when the window is wider");
    }

    #[test]
    fn a_fully_finished_panel_reports_done_rather_than_an_empty_batch() {
        // So a caller stops rather than dispatching a zero-row matvec.
        let mut b = Batch::new(2, 0);
        b.finish(0);
        b.finish(1);
        assert!(b.all_done());
        assert_eq!(b.m(), 0);
        assert!(!b.should_batch(8));
    }

    #[test]
    fn finishing_the_same_member_twice_is_idempotent() {
        // A stop token and a token cap can both fire on the same step, and the second must not remove
        // a different member's row.
        let mut b = Batch::new(3, 0);
        b.finish(2);
        b.finish(2);
        assert_eq!(b.m(), 2);
        assert_eq!(b.owner(0), Some(0));
        assert_eq!(b.owner(1), Some(1));
    }
}
